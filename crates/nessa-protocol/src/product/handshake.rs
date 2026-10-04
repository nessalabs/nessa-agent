use super::generated::PRODUCT_VERSION;
use crate::product_contract::generated::SessionCloseReason;

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
