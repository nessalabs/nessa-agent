//! Wire frame envelopes from `protocol/schemas/v1/frames.json`.

use super::generated_types::{
    wire_shape_event_frame, wire_shape_res_frame, GatewayError, RESPONSE_PRESENCE,
};
use super::json::unique_value;
use serde::{de::Error, Deserialize, Deserializer, Serialize};
use serde_json::Value;

/// Client → server RPC (`type: "req"`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RequestFrame {
    #[serde(rename = "type")]
    pub kind: String,
    pub id: String,
    pub method: String,
    pub params: Value,
}

impl RequestFrame {
    /// Construct a typed request envelope; the method owns its parameter DTO.
    pub fn new<T: Serialize>(
        id: &str,
        method: &str,
        params: &T,
    ) -> Result<Self, serde_json::Error> {
        Ok(Self {
            kind: "req".into(),
            id: id.into(),
            method: method.into(),
            params: serde_json::to_value(params)?,
        })
    }

    /// Decode one client frame from its wire text.
    ///
    /// `params` stays an untyped value until the method that owns it decodes it,
    /// so a policy-bearing name repeated in the original JSON would otherwise be
    /// collapsed to one value before anything could object. Decoding rejects
    /// repeated names at every depth, which is why no client text reaches
    /// dispatch — or error correlation — through `serde_json::from_str`.
    pub fn decode(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_value(unique_value(text)?)
    }
}

/// Server → client RPC reply (`type: "res"`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResponseFrame {
    #[serde(rename = "type")]
    kind: String,
    pub id: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default, deserialize_with = "present")]
    pub payload: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default, deserialize_with = "present")]
    pub error: Option<GatewayError>,
}

fn present<'de, T: Deserialize<'de>, D: Deserializer<'de>>(
    decoder: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(decoder).map(Some)
}

/// Server → client push (`type: "event"`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventFrame {
    #[serde(rename = "type")]
    kind: String,
    pub event: String,
    pub payload: Value,
    pub seq: u64,
    pub state_version: u64,
}

/// Anything the server may send on the WebSocket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OutgoingMessage {
    Response(ResponseFrame),
    Event(EventFrame),
}

impl ResponseFrame {
    /// Successful RPC reply with a typed payload.
    pub fn success<T: Serialize>(request_id: &str, payload: &T) -> Result<Self, serde_json::Error> {
        Ok(Self {
            kind: "res".into(),
            id: request_id.to_string(),
            ok: true,
            payload: Some(serde_json::to_value(payload)?),
            error: None,
        })
    }

    /// Failed RPC reply with a gateway error code and message.
    pub fn failure(request_id: &str, code: &str, message: &str) -> Self {
        Self::failure_with_details(request_id, code, message, None)
    }

    /// Failed RPC reply with optional typed method-specific details.
    pub fn failure_with_details(
        request_id: &str,
        code: &str,
        message: &str,
        details: Option<Value>,
    ) -> Self {
        Self {
            kind: "res".into(),
            id: request_id.to_string(),
            ok: false,
            payload: None,
            error: Some(GatewayError {
                code: code.to_string(),
                message: message.to_string(),
                details,
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
            kind: "event".into(),
            event: event.to_string(),
            payload: serde_json::to_value(payload)?,
            seq,
            state_version,
        })
    }
}

impl OutgoingMessage {
    /// Decode trusted server envelopes through the same unique-key budget owner.
    pub fn decode(text: &str) -> Result<Self, serde_json::Error> {
        let value = unique_value(text)?;
        if !wire_shape_res_frame(&value) && !wire_shape_event_frame(&value) {
            return Err(serde_json::Error::custom("invalid server frame shape"));
        }
        let frame: Self = serde_json::from_value(value)?;
        let valid = match &frame {
            Self::Response(frame) => {
                let presence = usize::from(frame.ok) * 4
                    + usize::from(frame.payload.is_some()) * 2
                    + usize::from(frame.error.is_some());
                RESPONSE_PRESENCE[presence]
            }
            Self::Event(_) => true,
        };
        if !valid {
            return Err(serde_json::Error::custom("contradictory server frame"));
        }
        Ok(frame)
    }

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
    fn response_presence_schema_agreement() {
        for index in 0..8 {
            let mut frame = serde_json::json!({"type":"res", "id":"1", "ok": index & 4 != 0});
            if index & 2 != 0 {
                frame["payload"] = Value::Null;
            }
            if index & 1 != 0 {
                frame["error"] = serde_json::json!({"code":"refused", "message":""});
            }
            assert_eq!(
                OutgoingMessage::decode(&frame.to_string()).is_ok(),
                index == 1 || index == 6,
                "presence {index}"
            );
        }
        let null =
            OutgoingMessage::decode(r#"{"type":"res","id":"1","ok":true,"payload":null}"#).unwrap();
        assert!(
            matches!(null, OutgoingMessage::Response(frame) if frame.payload == Some(Value::Null))
        );
    }

    #[test]
    fn scalar_shape_and_response_presence_are_consumed_together() {
        let missing_payload = serde_json::json!({"type":"res","id":"1","ok":true});
        assert!(wire_shape_res_frame(&missing_payload));
        assert!(OutgoingMessage::decode(&missing_payload.to_string()).is_err());
        let invalid_scalar = serde_json::json!({"type":"res","id":"","ok":true,"payload":null});
        assert!(!wire_shape_res_frame(&invalid_scalar));
        assert!(OutgoingMessage::decode(&invalid_scalar.to_string()).is_err());
        let supported_null = serde_json::json!({"type":"res","id":"1","ok":true,"payload":null});
        assert!(wire_shape_res_frame(&supported_null));
        assert!(OutgoingMessage::decode(&supported_null.to_string()).is_ok());
    }

    #[test]
    fn server_decode_rejects_duplicate_and_malformed_response_evidence() {
        for text in [
            r#"{"type":"res","id":"1","ok":true,"ok":false,"payload":null}"#,
            r#"{"type":"res","id":"1","ok":false,"error":null}"#,
            r#"{"type":"res","id":"","ok":true,"payload":null}"#,
        ] {
            assert!(OutgoingMessage::decode(text).is_err());
        }
    }

    #[test]
    fn success_response_uses_payload_field() {
        let frame = ResponseFrame::success("1", &serde_json::json!({ "ok": true })).expect("frame");
        assert!(frame.ok);
        assert!(frame.payload.is_some());
        assert!(frame.error.is_none());
    }
}
