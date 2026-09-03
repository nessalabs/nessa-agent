//! Build typed server messages ready to send on the WebSocket.

use super::generated_types::{
    ConnectChallenge, HelloOk, RuntimeStatus, Scope, ServerPolicy, HealthResult,
};
use super::frames::{EventFrame, OutgoingMessage, ResponseFrame};
use super::generated_catalog::event;

pub const PROTOCOL_VERSION: i64 = 1;
pub const MAX_PAYLOAD_BYTES: i64 = 65_536;

/// `connect.challenge` event sent when a client opens the WebSocket.
pub fn connect_challenge_message(nonce: String, seq: u64) -> Result<OutgoingMessage, serde_json::Error> {
    let payload = ConnectChallenge {
        nonce,
        protocol: PROTOCOL_VERSION,
    };
    Ok(OutgoingMessage::Event(EventFrame::push(
        event::CONNECT_CHALLENGE,
        &payload,
        seq,
        0,
    )?))
}

/// Successful `connect` RPC reply.
pub fn connect_success_message(
    request_id: &str,
    server_version: &str,
) -> Result<OutgoingMessage, serde_json::Error> {
    let payload = HelloOk {
        protocol: PROTOCOL_VERSION,
        scopes: vec![Scope::ServerRead],
        server_version: server_version.to_string(),
        runtime_status: RuntimeStatus::Ready,
        policy: ServerPolicy {
            max_payload_bytes: MAX_PAYLOAD_BYTES,
        },
    };
    Ok(OutgoingMessage::Response(ResponseFrame::success(
        request_id,
        &payload,
    )?))
}

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
        request_id,
        &payload,
    )?))
}

/// Failed RPC reply for any method.
pub fn error_message(request_id: &str, code: &str, message: &str) -> OutgoingMessage {
    OutgoingMessage::Response(ResponseFrame::failure(request_id, code, message))
}
