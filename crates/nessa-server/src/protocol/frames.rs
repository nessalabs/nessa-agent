//! Wire frame envelopes from `protocol/schemas/v1/frames.json`.

use super::generated_types::GatewayError;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Client → server RPC (`type: "req"`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestFrame {
    #[serde(rename = "type")]
    pub kind: String,
    pub id: String,
    pub method: String,
    pub params: Value,
}

/// Server → client RPC reply (`type: "res"`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResponseFrame {
    #[serde(rename = "type")]
    kind: &'static str,
    pub id: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<GatewayError>,
}

/// Server → client push (`type: "event"`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventFrame {
    #[serde(rename = "type")]
    kind: &'static str,
    pub event: String,
    pub payload: Value,
    pub seq: u64,
    pub state_version: u64,
}

/// Anything the server may send on the WebSocket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum OutgoingMessage {
    Response(ResponseFrame),
    Event(EventFrame),
}

impl ResponseFrame {
    /// Successful RPC reply with a typed payload.
    pub fn success<T: Serialize>(request_id: &str, payload: &T) -> Result<Self, serde_json::Error> {
        Ok(Self {
            kind: "res",
            id: request_id.to_string(),
            ok: true,
            payload: Some(serde_json::to_value(payload)?),
            error: None,
        })
    }

    /// Failed RPC reply with a gateway error code and message.
    pub fn failure(request_id: &str, code: &str, message: &str) -> Self {
        Self {
            kind: "res",
            id: request_id.to_string(),
            ok: false,
            payload: None,
            error: Some(GatewayError {
                code: code.to_string(),
                message: message.to_string(),
                details: None,
            }),
        }
    }
}

impl EventFrame {
    /// Push event with a typed payload.
    pub fn push<T: Serialize>(
        event: &str,
        payload: &T,
        seq: u64,
        state_version: u64,
    ) -> Result<Self, serde_json::Error> {
        Ok(Self {
            kind: "event",
            event: event.to_string(),
            payload: serde_json::to_value(payload)?,
            seq,
            state_version,
        })
    }
}

impl OutgoingMessage {
    /// Serialize to the JSON text sent on the WebSocket.
    pub fn to_wire_text(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// True when this is a successful RPC response frame.
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Response(frame) if frame.ok)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn success_response_uses_payload_field() {
        let frame = ResponseFrame::success("1", &serde_json::json!({ "ok": true })).expect("frame");
        assert!(frame.ok);
        assert!(frame.payload.is_some());
        assert!(frame.error.is_none());
    }
}
