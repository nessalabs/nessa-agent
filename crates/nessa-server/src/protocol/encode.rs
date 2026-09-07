use super::frames::{OutgoingMessage, ResponseFrame};
use super::generated_types::{EchoResult, HealthResult, RuntimeStatus};

pub const MAX_PAYLOAD_BYTES: i64 = 65_536;

/// Successful `server.health` RPC reply.
pub fn health_check_message(
    request_id: &str,
    uptime_ms: u64,
) -> Result<OutgoingMessage, serde_json::Error> {
    let payload = HealthResult {
        ok: true,
        runtime_status: RuntimeStatus::Ready,
        uptime_ms: i64::try_from(uptime_ms).unwrap_or(i64::MAX),
    };
    Ok(OutgoingMessage::Response(ResponseFrame::success(
        request_id, &payload,
    )?))
}

/// Successful `conversation.echo` RPC reply.
pub fn echo_message(request_id: &str, text: String) -> Result<OutgoingMessage, serde_json::Error> {
    let payload = EchoResult { text };
    Ok(OutgoingMessage::Response(ResponseFrame::success(
        request_id, &payload,
    )?))
}

/// Failed RPC reply for any method.
pub fn error_message(request_id: &str, code: &str, message: &str) -> OutgoingMessage {
    OutgoingMessage::Response(ResponseFrame::failure(request_id, code, message))
}
