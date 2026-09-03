//! Decode WebSocket JSON text into typed client RPCs.

use super::encode::error_message;
use super::frames::{OutgoingMessage, RequestFrame};
use super::generated_catalog::method;
use super::generated_types::{ConnectParams, HealthParams};
use serde::Deserialize;

/// Supported client RPCs after decoding a WebSocket text message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientRequest {
    Connect {
        request_id: String,
        params: ConnectParams,
    },
    ServerHealth {
        request_id: String,
    },
    UnknownMethod {
        request_id: String,
        method: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    InvalidJson,
    NotARequest,
    InvalidParams {
        request_id: String,
        method: String,
        message: String,
    },
}

/// Build a correlated error response when the request id can be recovered.
pub fn decode_error_response(text: &str, error: DecodeError) -> Option<OutgoingMessage> {
    let request_id = extract_request_id(text)?;
    Some(match error {
        DecodeError::InvalidJson => error_message(&request_id, "invalid_json", "malformed JSON"),
        DecodeError::NotARequest => {
            error_message(&request_id, "invalid_request", "expected req frame")
        }
        DecodeError::InvalidParams { method, message, .. } => error_message(
            &request_id,
            "invalid_params",
            &format!("{method}: {message}"),
        ),
    })
}

fn extract_request_id(text: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    value
        .get("id")
        .and_then(serde_json::Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
}

/// Decode a WebSocket text message into a typed client request.
pub fn decode_client_request(text: &str) -> Result<ClientRequest, DecodeError> {
    let frame = decode_request_frame(text)?;
    decode_request_from_frame(frame)
}

fn decode_request_frame(text: &str) -> Result<RequestFrame, DecodeError> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|_| DecodeError::InvalidJson)?;
    if value.get("type").and_then(serde_json::Value::as_str) != Some("req") {
        return Err(DecodeError::NotARequest);
    }
    let frame: RequestFrame =
        serde_json::from_value(value).map_err(|_| DecodeError::InvalidJson)?;
    validate_request_frame(&frame)?;
    Ok(frame)
}

fn validate_request_frame(frame: &RequestFrame) -> Result<(), DecodeError> {
    if frame.id.is_empty() {
        return Err(DecodeError::InvalidParams {
            request_id: frame.id.clone(),
            method: frame.method.clone(),
            message: "id must not be empty".to_string(),
        });
    }
    if frame.method.is_empty() {
        return Err(DecodeError::InvalidParams {
            request_id: frame.id.clone(),
            method: frame.method.clone(),
            message: "method must not be empty".to_string(),
        });
    }
    Ok(())
}

fn decode_request_from_frame(frame: RequestFrame) -> Result<ClientRequest, DecodeError> {
    match frame.method.as_str() {
        method::CONNECT => {
            let params = parse_params::<ConnectParams>(&frame, method::CONNECT)?;
            Ok(ClientRequest::Connect {
                request_id: frame.id.clone(),
                params,
            })
        }
        method::SERVER_HEALTH => {
            parse_params::<HealthParams>(&frame, method::SERVER_HEALTH)?;
            Ok(ClientRequest::ServerHealth {
                request_id: frame.id.clone(),
            })
        }
        other => Ok(ClientRequest::UnknownMethod {
            request_id: frame.id.clone(),
            method: other.to_string(),
        }),
    }
}

fn parse_params<T: for<'de> Deserialize<'de>>(
    frame: &RequestFrame,
    method: &str,
) -> Result<T, DecodeError> {
    serde_json::from_value(frame.params.clone()).map_err(|error| DecodeError::InvalidParams {
        request_id: frame.id.clone(),
        method: method.to_string(),
        message: error.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::OutgoingMessage;

    #[test]
    fn decodes_connect_request() {
        let text = r#"{
            "type": "req",
            "id": "1",
            "method": "connect",
            "params": {
                "minProtocol": 1,
                "maxProtocol": 1,
                "role": "surface",
                "surface": { "kind": "panel", "instance": "main" },
                "client": { "id": "nessa-panel", "version": "0.1.0", "platform": "macos" },
                "auth": { "token": "dev-token", "nonce": "test-nonce" }
            }
        }"#;

        let request = decode_client_request(text).expect("connect");
        assert!(matches!(request, ClientRequest::Connect { .. }));
    }

    #[test]
    fn rejects_non_request_json() {
        let error = decode_client_request(r#"{ "type": "res", "id": "1", "ok": true }"#)
            .expect_err("not a req");
        assert_eq!(error, DecodeError::NotARequest);
    }

    #[test]
    fn decodes_unknown_method_without_failing() {
        let request = decode_client_request(
            r#"{ "type": "req", "id": "9", "method": "conversation.open", "params": {} }"#,
        )
        .expect("unknown");
        assert!(matches!(
            request,
            ClientRequest::UnknownMethod {
                method,
                ..
            } if method == "conversation.open"
        ));
    }

    #[test]
    fn decode_error_response_for_invalid_params() {
        let text = r#"{ "type": "req", "id": "42", "method": "connect", "params": {} }"#;
        let error = decode_client_request(text).expect_err("invalid params");
        let response = decode_error_response(text, error).expect("correlated response");
        let OutgoingMessage::Response(frame) = response else {
            panic!("expected response");
        };
        assert_eq!(frame.id, "42");
        assert!(!frame.ok);
        assert_eq!(
            frame.error.as_ref().map(|error| error.code.as_str()),
            Some("invalid_params")
        );
    }

    #[test]
    fn rejects_empty_request_id() {
        let error = decode_client_request(
            r#"{ "type": "req", "id": "", "method": "connect", "params": {} }"#,
        )
        .expect_err("empty id");
        assert!(matches!(error, DecodeError::InvalidParams { .. }));
    }

    #[test]
    fn rejects_unknown_client_fields() {
        let error = decode_client_request(
            r#"{
                "type": "req",
                "id": "1",
                "method": "connect",
                "params": {
                    "minProtocol": 1,
                    "maxProtocol": 1,
                    "role": "surface",
                    "surface": { "kind": "panel", "instance": "main" },
                    "client": { "id": "nessa-panel", "version": "0.1.0", "platform": "macos", "extra": true },
                    "auth": { "token": "dev-token", "nonce": "test-nonce" }
                }
            }"#,
        )
        .expect_err("unknown nested field");
        assert!(matches!(error, DecodeError::InvalidJson | DecodeError::InvalidParams { .. }));
    }

    #[test]
    fn rejects_unknown_connect_fields() {
        let error = decode_client_request(
            r#"{
                "type": "req",
                "id": "1",
                "method": "connect",
                "params": {
                    "minProtocol": 1,
                    "maxProtocol": 1,
                    "role": "surface",
                    "surface": { "kind": "panel", "instance": "main" },
                    "client": { "id": "nessa-panel", "version": "0.1.0", "platform": "macos" },
                    "auth": { "token": "dev-token", "nonce": "test-nonce" },
                    "unexpected": true
                }
            }"#,
        )
        .expect_err("unknown field");
        assert!(matches!(error, DecodeError::InvalidJson | DecodeError::InvalidParams { .. }));
    }

    #[test]
    fn decode_error_response_skips_uncorrelatable_json() {
        let response = decode_error_response("{ not json", DecodeError::InvalidJson);
        assert!(response.is_none());
    }
}
