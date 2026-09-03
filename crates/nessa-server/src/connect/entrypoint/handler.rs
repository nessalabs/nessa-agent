use crate::app::state::AppState;
use crate::connect::middleware;
use crate::protocol::{connect_success_message, ConnectParams, OutgoingMessage, ResponseFrame};
use crate::server::entrypoint::session::WsSession;

/// Handle the `connect` RPC: run all gates, then return `HelloOk`.
///
/// Gate order (lazy): already connected → protocol → metadata → nonce → token.
/// On success this marks the session connected.
pub fn handle_connect(
    state: &AppState,
    session: &mut WsSession,
    request_id: &str,
    params: ConnectParams,
) -> OutgoingMessage {
    if session.is_connected() {
        return OutgoingMessage::Response(ResponseFrame::failure(
            request_id,
            "already_connected",
            "session already completed connect",
        ));
    }
    if let Some(response) = middleware::protocol::validate_protocol_range(request_id, &params) {
        return OutgoingMessage::Response(response);
    }
    if let Some(response) = middleware::metadata::validate_connect_metadata(request_id, &params) {
        return OutgoingMessage::Response(response);
    }
    if let Some(response) =
        middleware::challenge::validate_challenge_nonce(request_id, &params, session)
    {
        return OutgoingMessage::Response(response);
    }
    if let Some(response) = middleware::auth::validate_auth_token(request_id, &params, state) {
        return OutgoingMessage::Response(response);
    }

    let response = connect_success_message(request_id, state.version()).unwrap_or_else(|error| {
        tracing::error!(%error, "failed to encode connect response");
        OutgoingMessage::Response(ResponseFrame::failure(
            request_id,
            "internal_error",
            "failed to encode response",
        ))
    });

    if response.is_success() {
        session.mark_connected();
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::{Environment, MockEnv};
    use crate::protocol::{AuthToken, ClientInfo, ClientRole, SurfaceInfo, SurfaceKind};

    fn test_state() -> AppState {
        AppState::from_environment(&Environment::load(&MockEnv::new()).expect("defaults"))
    }

    fn session_with_nonce(nonce: &str) -> WsSession {
        let mut session = WsSession::new();
        session.set_challenge_nonce(nonce.to_string());
        session
    }

    fn connect_params(token: &str, min: i64, max: i64) -> ConnectParams {
        ConnectParams {
            min_protocol: min,
            max_protocol: max,
            role: ClientRole::Surface,
            surface: SurfaceInfo {
                kind: SurfaceKind::Panel,
                instance: "test".to_string(),
            },
            client: ClientInfo {
                id: "test".to_string(),
                version: "0.1.0".to_string(),
                platform: "test".to_string(),
            },
            auth: AuthToken {
                token: token.to_string(),
                nonce: "test-nonce".to_string(),
            },
        }
    }

    fn response_error_code(response: &OutgoingMessage) -> &str {
        match response {
            OutgoingMessage::Response(frame) => {
                frame.error.as_ref().expect("error frame").code.as_str()
            }
            OutgoingMessage::Event(_) => panic!("expected response frame"),
        }
    }

    fn response_is_ok(response: &OutgoingMessage) -> bool {
        matches!(response, OutgoingMessage::Response(frame) if frame.ok)
    }

    #[test]
    fn rejects_bad_protocol_range() {
        let mut session = session_with_nonce("test-nonce");
        let response = handle_connect(
            &test_state(),
            &mut session,
            "1",
            connect_params("dev-token", 2, 2),
        );
        assert!(!response_is_ok(&response));
        assert_eq!(response_error_code(&response), "protocol_mismatch");
        assert!(!session.is_connected());
    }

    #[test]
    fn rejects_bad_token() {
        let mut session = session_with_nonce("test-nonce");
        let response = handle_connect(
            &test_state(),
            &mut session,
            "1",
            connect_params("wrong", 1, 1),
        );
        assert!(!response_is_ok(&response));
        assert_eq!(response_error_code(&response), "unauthorized");
        assert!(!session.is_connected());
    }

    #[test]
    fn accepts_valid_connect() {
        let mut session = session_with_nonce("test-nonce");
        let response = handle_connect(
            &test_state(),
            &mut session,
            "1",
            connect_params("dev-token", 1, 1),
        );
        assert!(response_is_ok(&response));
        assert!(session.is_connected());
    }

    #[test]
    fn rejects_repeat_connect() {
        let mut session = session_with_nonce("test-nonce");
        let first = handle_connect(
            &test_state(),
            &mut session,
            "1",
            connect_params("dev-token", 1, 1),
        );
        assert!(response_is_ok(&first));

        let second = handle_connect(
            &test_state(),
            &mut session,
            "2",
            connect_params("dev-token", 1, 1),
        );
        assert!(!response_is_ok(&second));
        assert_eq!(response_error_code(&second), "already_connected");
    }

    #[test]
    fn rejects_nonce_mismatch() {
        let mut session = session_with_nonce("expected-nonce");
        let response = handle_connect(
            &test_state(),
            &mut session,
            "1",
            connect_params("dev-token", 1, 1),
        );
        assert!(!response_is_ok(&response));
        assert_eq!(response_error_code(&response), "invalid_challenge");
    }
}
