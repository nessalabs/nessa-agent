use super::session::WsSession;
use super::ws_error::WsReadFailure;
use crate::app::state::AppState;
use crate::connect::entrypoint::handler as connect_handler;
use crate::conversation::entrypoint::handler as conversation_handler;
use crate::health::entrypoint::handler as health_handler;
use crate::ping::entrypoint::handler as ping_handler;
use crate::protocol::{
    connect_challenge_message, decode_client_request, decode_error_response, error_message,
    wire_method, ClientRequest, OutgoingMessage, MAX_PAYLOAD_BYTES,
};
use axum::extract::ws::{Message, WebSocket};
use futures_util::StreamExt;
use uuid::Uuid;

/// Run the WebSocket session: challenge, then route decoded client RPCs.
pub async fn handle_socket(mut socket: WebSocket, state: AppState) {
    let mut session = WsSession::new();

    let nonce = Uuid::new_v4().to_string();
    session.set_challenge_nonce(nonce.clone());
    let challenge = connect_challenge_message(nonce, session.next_seq()).unwrap_or_else(|error| {
        tracing::error!(%error, "failed to encode connect.challenge");
        error_message("", "internal_error", "failed to encode challenge")
    });
    if send_outgoing_message(&mut socket, challenge).await.is_err() {
        return;
    }

    while let Some(result) = socket.next().await {
        let message = match result {
            Ok(message) => message,
            Err(error) => {
                match WsReadFailure::classify(&error) {
                    WsReadFailure::Disconnected => {
                        tracing::debug!(%error, "websocket disconnected");
                    }
                    WsReadFailure::Unexpected => {
                        tracing::warn!(%error, "websocket read error");
                    }
                }
                break;
            }
        };

        let text = match message {
            Message::Text(text) => text,
            Message::Ping(_) | Message::Pong(_) => continue,
            Message::Binary(bytes) => {
                tracing::warn!(bytes = bytes.len(), "refusing binary WebSocket frame");
                continue;
            }
            Message::Close(_) => break,
        };

        if is_oversized_frame(text.len()) {
            tracing::warn!(
                bytes = text.len(),
                limit = MAX_PAYLOAD_BYTES,
                "rejecting oversized WebSocket text frame"
            );
            break;
        }

        let request = match decode_client_request(&text) {
            Ok(request) => request,
            Err(error) => {
                let Some(response) = decode_error_response(&text, error) else {
                    continue;
                };
                if send_outgoing_message(&mut socket, response).await.is_err() {
                    break;
                }
                continue;
            }
        };

        let Some(response) = dispatch_client_request(&state, &mut session, request) else {
            continue;
        };

        if send_outgoing_message(&mut socket, response).await.is_err() {
            break;
        }
    }
}

/// Route a decoded client RPC to the feature handler.
fn dispatch_client_request(
    state: &AppState,
    session: &mut WsSession,
    request: ClientRequest,
) -> Option<OutgoingMessage> {
    match request {
        ClientRequest::Connect { request_id, params } => Some(connect_handler::handle_connect(
            state,
            session,
            &request_id,
            params,
        )),
        ClientRequest::ServerHealth { request_id, .. } => {
            if !session.is_connected() {
                return Some(error_message(
                    &request_id,
                    "not_connected",
                    "connect required before server.health",
                ));
            }
            Some(health_handler::handle_health_check(state, &request_id))
        }
        ClientRequest::ServerPing {
            request_id,
            params,
        } => {
            // ADR 0006: ping is only composed on stage=dev — otherwise unknown_method.
            if !state.offers_server_ping() {
                return Some(error_message(
                    &request_id,
                    "unknown_method",
                    wire_method::SERVER_PING,
                ));
            }
            if !session.is_connected() {
                return Some(error_message(
                    &request_id,
                    "not_connected",
                    "connect required before server.ping",
                ));
            }
            Some(ping_handler::handle_ping(&request_id, params))
        }
        ClientRequest::ConversationEcho {
            request_id,
            params,
        } => {
            if !session.is_connected() {
                return Some(error_message(
                    &request_id,
                    "not_connected",
                    "connect required before conversation.echo",
                ));
            }
            Some(conversation_handler::handle_echo(&request_id, params))
        }
        ClientRequest::UnknownMethod { request_id, method } => {
            Some(error_message(&request_id, "unknown_method", &method))
        }
    }
}

/// Serialize and send one server message on the WebSocket.
async fn send_outgoing_message(socket: &mut WebSocket, message: OutgoingMessage) -> Result<(), ()> {
    let text = message.to_wire_text().map_err(|_| ())?;
    socket
        .send(Message::Text(text.into()))
        .await
        .map_err(|_| ())
}

fn is_oversized_frame(byte_len: usize) -> bool {
    byte_len > MAX_PAYLOAD_BYTES as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::{Environment, MockEnv, STAGE, TOKEN};
    use crate::protocol::PingParams;

    fn test_state() -> AppState {
        AppState::from_environment(&Environment::load(&MockEnv::new()).expect("defaults"))
    }

    fn prod_state() -> AppState {
        AppState::from_environment(
            &Environment::load(&MockEnv::new().set(STAGE, "prod").set(TOKEN, "secret"))
                .expect("prod"),
        )
    }

    #[test]
    fn health_before_connect_returns_not_connected() {
        let mut session = WsSession::new();
        let response = dispatch_client_request(
            &test_state(),
            &mut session,
            ClientRequest::ServerHealth {
                request_id: "7".to_string(),
            },
        )
        .expect("response");

        let OutgoingMessage::Response(frame) = response else {
            panic!("expected response frame");
        };
        assert!(!frame.ok);
        assert_eq!(
            frame.error.as_ref().map(|error| error.code.as_str()),
            Some("not_connected")
        );
    }

    #[test]
    fn ping_on_dev_echoes_nonce_after_connect() {
        let mut session = WsSession::new();
        session.mark_connected();
        let response = dispatch_client_request(
            &test_state(),
            &mut session,
            ClientRequest::ServerPing {
                request_id: "3".to_string(),
                params: PingParams {
                    nonce: "probe-1".to_string(),
                },
            },
        )
        .expect("response");

        let OutgoingMessage::Response(frame) = response else {
            panic!("expected response frame");
        };
        assert!(frame.ok);
        let payload = frame.payload.expect("payload");
        assert_eq!(payload["ok"], true);
        assert_eq!(payload["nonce"], "probe-1");
    }

    #[test]
    fn ping_absent_on_non_dev_is_unknown_method() {
        let mut session = WsSession::new();
        session.mark_connected();
        let response = dispatch_client_request(
            &prod_state(),
            &mut session,
            ClientRequest::ServerPing {
                request_id: "3".to_string(),
                params: PingParams {
                    nonce: "probe-1".to_string(),
                },
            },
        )
        .expect("response");

        let OutgoingMessage::Response(frame) = response else {
            panic!("expected response frame");
        };
        assert!(!frame.ok);
        assert_eq!(
            frame.error.as_ref().map(|error| error.code.as_str()),
            Some("unknown_method")
        );
    }

    #[test]
    fn rejects_oversized_wire_text() {
        assert!(is_oversized_frame(MAX_PAYLOAD_BYTES as usize + 1));
        assert!(!is_oversized_frame(MAX_PAYLOAD_BYTES as usize));
    }
}
