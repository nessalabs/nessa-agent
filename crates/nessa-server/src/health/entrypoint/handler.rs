use crate::app::state::AppState;
use crate::protocol::{health_check_message, OutgoingMessage, ResponseFrame};

/// HTTP probe used by smoke tests and load balancers.
pub async fn handle_http_health() -> axum::http::StatusCode {
    axum::http::StatusCode::OK
}

/// Handle the `server.health` RPC.
pub fn handle_health_check(state: &AppState, request_id: &str) -> OutgoingMessage {
    health_check_message(request_id, state.uptime_ms()).unwrap_or_else(|error| {
        tracing::error!(%error, "failed to encode health response");
        OutgoingMessage::Response(ResponseFrame::failure(
            request_id,
            "internal_error",
            "failed to encode response",
        ))
    })
}
