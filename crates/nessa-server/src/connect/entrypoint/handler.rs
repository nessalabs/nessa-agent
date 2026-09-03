use crate::app::state::AppState;
use crate::connect::middleware;
use crate::protocol::{connect_success_message, ConnectParams, OutgoingMessage, ResponseFrame};

/// Handle the `connect` RPC: validate, then return `HelloOk`.
pub fn handle_connect(
    state: &AppState,
    request_id: &str,
    params: ConnectParams,
) -> OutgoingMessage {
    if let Some(response) = middleware::protocol::validate_protocol_range(request_id, &params) {
        return OutgoingMessage::Response(response);
    }
    if let Some(response) = middleware::auth::validate_auth_token(request_id, &params, state) {
        return OutgoingMessage::Response(response);
    }

    connect_success_message(request_id, state.version())
        .unwrap_or_else(|error| {
            tracing::error!(%error, "failed to encode connect response");
            OutgoingMessage::Response(ResponseFrame::failure(
                request_id,
                "internal_error",
                "failed to encode response",
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::{Environment, MockEnv};
    use crate::protocol::{
        AuthToken, ClientInfo, ClientRole, SurfaceInfo, SurfaceKind,
    };

    fn test_state() -> AppState {
        AppState::from_environment(&Environment::load(&MockEnv::new()).expect("defaults"))
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
            OutgoingMessage::Response(frame) => frame
                .error
                .as_ref()
                .expect("error frame")
                .code
                .as_str(),
            OutgoingMessage::Event(_) => panic!("expected response frame"),
        }
    }

    fn response_is_ok(response: &OutgoingMessage) -> bool {
        matches!(
            response,
            OutgoingMessage::Response(frame) if frame.ok
        )
    }

    #[test]
    fn rejects_bad_protocol_range() {
        let response = handle_connect(&test_state(), "1", connect_params("dev-token", 2, 2));
        assert!(!response_is_ok(&response));
        assert_eq!(response_error_code(&response), "protocol_mismatch");
    }

    #[test]
    fn rejects_bad_token() {
        let response = handle_connect(&test_state(), "1", connect_params("wrong", 1, 1));
        assert!(!response_is_ok(&response));
        assert_eq!(response_error_code(&response), "unauthorized");
    }

    #[test]
    fn accepts_valid_connect() {
        let response = handle_connect(&test_state(), "1", connect_params("dev-token", 1, 1));
        assert!(response_is_ok(&response));
    }
}
