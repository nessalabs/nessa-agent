use crate::protocol::{ping_echo_message, OutgoingMessage, PingParams, ResponseFrame};

/// Handle `server.ping`: echo the client nonce.
pub fn handle_ping(request_id: &str, params: PingParams) -> OutgoingMessage {
    ping_echo_message(request_id, params.nonce).unwrap_or_else(|error| {
        tracing::error!(%error, "failed to encode ping response");
        OutgoingMessage::Response(ResponseFrame::failure(
            request_id,
            "internal_error",
            "failed to encode response",
        ))
    })
}
