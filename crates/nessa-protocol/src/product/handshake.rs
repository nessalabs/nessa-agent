use super::generated::{SessionTermination, PRODUCT_VERSION};
use crate::product_contract::generated::SessionCloseReason;
use serde_json::Value;

/// `SessionTermination.retryAfterMs` maximum from `protocol/product/v1.json`.
pub const MAX_TERMINATION_RETRY_AFTER_MS: u64 = 60_000;

/// A `session.terminated` payload both ends accept: the schema shape, a
/// `retryable` flag that matches the reason, and a delay inside the published
/// maximum. Anything else is not a termination.
pub fn accepted_termination(payload: &Value) -> Option<SessionTermination> {
    let termination: SessionTermination = serde_json::from_value(payload.clone()).ok()?;
    if termination.retryable != termination.code.retryable() {
        return None;
    }
    if termination
        .retry_after_ms
        .is_some_and(|delay| delay > MAX_TERMINATION_RETRY_AFTER_MS)
    {
        return None;
    }
    Some(termination)
}

/// The current product version owner decides overlap for both handshake directions.
pub fn supports_product_version(minimum: u64, maximum: u64) -> bool {
    minimum <= PRODUCT_VERSION && PRODUCT_VERSION <= maximum
}

/// Existing handshake refusal-to-close policy, shared by both wire directions.
pub fn authentication_close_reason(code: &str) -> SessionCloseReason {
    match code {
        "protocol_incompatible" => SessionCloseReason::ProtocolIncompatible,
        "temporarily_unavailable" => SessionCloseReason::TemporaryUnavailable,
        "handshake_timeout" => SessionCloseReason::HandshakeTimeout,
        _ => SessionCloseReason::AuthenticationFailed,
    }
}
