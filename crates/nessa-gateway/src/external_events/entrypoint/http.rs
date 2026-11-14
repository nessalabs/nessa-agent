use crate::external_events::{
    application::{AdmissionFailure, EventBinding},
    domain::EventId,
};
use axum::{
    body::{to_bytes, Body},
    extract::{Request, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use hmac::{Hmac, Mac};
use nessa_auth::application::ports::AccessError;
use nessa_sdk::application::agent_execution::commands::MutationFailure;
use serde::Deserialize;
use serde_json::json;
use sha2::Sha256;
use std::{error::Error, sync::Arc, time::Duration};
use tokio::sync::Semaphore;

/// Maximum received JSON body bytes, before parsing and without decompression.
pub const MAX_EVENT_BYTES: usize = 16 * 1024;
/// Private constructor-supplied key; deliberately no Debug or serialization.
pub struct SigningKey(Box<[u8]>);
/// Signing-key length is outside the supported constructor bound.
#[derive(Debug, PartialEq, Eq)]
pub struct InvalidSigningKey;
impl SigningKey {
    /// Retain 32–1024 key bytes in compact storage. Supply random key material
    /// through composition; this constructor verifies size, not entropy.
    pub fn new(bytes: Vec<u8>) -> Result<Self, InvalidSigningKey> {
        if !(32..=1024).contains(&bytes.len()) {
            return Err(InvalidSigningKey);
        }
        Ok(Self(bytes.into_boxed_slice()))
    }
}
struct Ingress {
    binding: EventBinding,
    key: SigningKey,
    slots: Arc<Semaphore>,
}
/// Construct an unmounted router for one stable binding, using a distinct key.
/// No production listener or provider registration is created.
pub fn router(binding: EventBinding, key: SigningKey) -> Router {
    Router::new()
        .route("/events", post(receive))
        .with_state(Arc::new(Ingress {
            binding,
            key,
            slots: Arc::new(Semaphore::new(8)),
        }))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    id: String,
    payload: serde_json::Value,
}
fn failure(status: StatusCode, code: &str) -> Response {
    (status, Json(json!({"error": code}))).into_response()
}
async fn receive(State(state): State<Arc<Ingress>>, request: Request<Body>) -> Response {
    let Ok(slot) = state.slots.clone().try_acquire_owned() else {
        return failure(StatusCode::SERVICE_UNAVAILABLE, "busy");
    };
    let (parts, body) = request.into_parts();
    if parts
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        != Some("application/json")
    {
        return failure(StatusCode::UNSUPPORTED_MEDIA_TYPE, "content_type");
    }
    if ["x-nessa-timestamp", "x-nessa-signature"]
        .iter()
        .any(|name| parts.headers.get_all(*name).iter().count() != 1)
    {
        return failure(StatusCode::UNAUTHORIZED, "signature");
    }
    let timestamp = parts
        .headers
        .get("x-nessa-timestamp")
        .and_then(|v| v.to_str().ok());
    let signature = parts
        .headers
        .get("x-nessa-signature")
        .and_then(|v| v.to_str().ok());
    let (Some(timestamp), Some(signature)) = (timestamp, signature) else {
        return failure(StatusCode::UNAUTHORIZED, "signature");
    };
    let Ok(seconds) = timestamp.parse::<u64>() else {
        return failure(StatusCode::UNAUTHORIZED, "signature");
    };
    if seconds.to_string() != timestamp
        || state.binding.clock.unix_seconds().abs_diff(seconds) > 300
    {
        return failure(StatusCode::UNAUTHORIZED, "signature");
    }
    if signature.len() != 44 {
        return failure(StatusCode::UNAUTHORIZED, "signature");
    }
    let Ok(signature) = STANDARD.decode(signature) else {
        return failure(StatusCode::UNAUTHORIZED, "signature");
    };
    if signature.len() != 32 {
        return failure(StatusCode::UNAUTHORIZED, "signature");
    }
    let bytes =
        match tokio::time::timeout(Duration::from_secs(5), to_bytes(body, MAX_EVENT_BYTES)).await {
            Ok(Ok(bytes)) => bytes,
            Ok(Err(error))
                if error
                    .source()
                    .is_some_and(|source| source.is::<http_body_util::LengthLimitError>()) =>
            {
                return failure(StatusCode::PAYLOAD_TOO_LARGE, "body_limit")
            }
            Ok(Err(_)) => return failure(StatusCode::BAD_REQUEST, "body_read"),
            Err(_) => return failure(StatusCode::REQUEST_TIMEOUT, "body_timeout"),
        };
    let mut mac =
        Hmac::<Sha256>::new_from_slice(&state.key.0).expect("HMAC accepts constructor-bounded key");
    mac.update(b"nessa-event\n");
    mac.update(state.binding.id.as_str().as_bytes());
    mac.update(b"\n");
    mac.update(timestamp.as_bytes());
    mac.update(b"\n");
    mac.update(&bytes);
    if mac.verify_slice(&signature).is_err() {
        return failure(StatusCode::UNAUTHORIZED, "signature");
    }
    let envelope: Envelope = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => return failure(StatusCode::BAD_REQUEST, "invalid_event"),
    };
    let Ok(event) = EventId::new(envelope.id) else {
        return failure(StatusCode::BAD_REQUEST, "invalid_event");
    };
    let text = json!({"eventId": event.as_str(), "payload": envelope.payload}).to_string();
    // The command owner can outlive this HTTP response future. Transfer the
    // slot before awaiting it, so caller loss does not create uncounted work.
    let admission = tokio::spawn(async move {
        let _slot = slot;
        state.binding.admit(&event, text).await
    })
    .await;
    let result = match admission {
        Ok(result) => result,
        Err(error) => {
            tracing::error!(
                task_panicked = error.is_panic(),
                task_cancelled = error.is_cancelled(),
                "external event admission task failed"
            );
            return failure(StatusCode::SERVICE_UNAVAILABLE, "dispatch_unconfirmed");
        }
    };
    match result {
        Ok(receipt) => (
            StatusCode::ACCEPTED,
            Json(json!({"executionId": receipt.execution_id, "status": "dispatched"})),
        )
            .into_response(),
        Err(AdmissionFailure::Access(AccessError::Unavailable)) => {
            failure(StatusCode::SERVICE_UNAVAILABLE, "authority_unavailable")
        }
        Err(AdmissionFailure::Access(_)) => failure(StatusCode::FORBIDDEN, "forbidden"),
        Err(AdmissionFailure::Submit(MutationFailure::Conflict)) => {
            failure(StatusCode::CONFLICT, "event_conflict")
        }
        Err(AdmissionFailure::Submit(MutationFailure::Interrupted(_))) => {
            failure(StatusCode::CONFLICT, "interrupted")
        }
        Err(_) => failure(StatusCode::SERVICE_UNAVAILABLE, "dispatch_unconfirmed"),
    }
}

#[cfg(test)]
mod tests {
    use super::SigningKey;
    #[test]
    fn a_key_discards_the_supplied_spare_capacity() {
        let mut input = Vec::with_capacity(1024 * 1024);
        input.extend_from_slice(&[42; 32]);
        let compact: Box<[u8]> = SigningKey::new(input).unwrap().0;
        assert_eq!(&*compact, &[42; 32]);
    }
}
