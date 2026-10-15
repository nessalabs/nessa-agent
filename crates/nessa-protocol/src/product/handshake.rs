use super::generated::{SessionTermination, PRODUCT_VERSION};
use crate::product_contract::generated::SessionCloseReason;
use serde_json::Value;

/// `SessionTermination.retryAfterMs` maximum from `protocol/product/v1.json`.
pub const MAX_TERMINATION_RETRY_AFTER_MS: u64 = 60_000;

/// What the client does next after a close. The wire reason stays
/// [`SessionCloseReason`]; this is the only projection of that reason, or of
/// a close that carried none. `retryable()` is not copied here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionOutcome {
    /// Try the same authorization again.
    Retryable,
    /// Stop. Do not retry this authorization.
    TerminalRevoke,
    /// The peer said the server shut down. The authorization stays.
    TerminalShutdown,
    /// The protocol does not match. The authorization stays.
    TerminalProtocol,
}

/// Classify a close. `None` is a plain EOF: retryable, and not a revoke.
pub fn connection_outcome(reason: Option<SessionCloseReason>) -> ConnectionOutcome {
    match reason {
        None
        | Some(
            SessionCloseReason::HandshakeTimeout
            | SessionCloseReason::TemporaryUnavailable
            | SessionCloseReason::GatewayRestarting
            | SessionCloseReason::GatewayOverloaded
            | SessionCloseReason::TransportInterrupted,
        ) => ConnectionOutcome::Retryable,
        Some(
            SessionCloseReason::AuthenticationFailed
            | SessionCloseReason::CredentialRevoked
            | SessionCloseReason::CredentialExpired
            | SessionCloseReason::AuthorizationLost,
        ) => ConnectionOutcome::TerminalRevoke,
        Some(SessionCloseReason::ServerShutdown) => ConnectionOutcome::TerminalShutdown,
        Some(SessionCloseReason::ProtocolIncompatible) => ConnectionOutcome::TerminalProtocol,
    }
}

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

#[cfg(test)]
mod tests {
    use super::{connection_outcome, ConnectionOutcome};
    use crate::product_contract::generated::SessionCloseReason;

    #[test]
    fn connection_outcome_is_the_only_close_class() {
        assert_eq!(connection_outcome(None), ConnectionOutcome::Retryable);
        let cases = [
            (
                SessionCloseReason::AuthenticationFailed,
                ConnectionOutcome::TerminalRevoke,
            ),
            (
                SessionCloseReason::CredentialRevoked,
                ConnectionOutcome::TerminalRevoke,
            ),
            (
                SessionCloseReason::CredentialExpired,
                ConnectionOutcome::TerminalRevoke,
            ),
            (
                SessionCloseReason::AuthorizationLost,
                ConnectionOutcome::TerminalRevoke,
            ),
            (
                SessionCloseReason::ProtocolIncompatible,
                ConnectionOutcome::TerminalProtocol,
            ),
            (
                SessionCloseReason::HandshakeTimeout,
                ConnectionOutcome::Retryable,
            ),
            (
                SessionCloseReason::TemporaryUnavailable,
                ConnectionOutcome::Retryable,
            ),
            (
                SessionCloseReason::GatewayRestarting,
                ConnectionOutcome::Retryable,
            ),
            (
                SessionCloseReason::GatewayOverloaded,
                ConnectionOutcome::Retryable,
            ),
            (
                SessionCloseReason::ServerShutdown,
                ConnectionOutcome::TerminalShutdown,
            ),
            (
                SessionCloseReason::TransportInterrupted,
                ConnectionOutcome::Retryable,
            ),
        ];
        for (reason, class) in cases {
            assert_eq!(connection_outcome(Some(reason)), class);
            assert_eq!(
                class == ConnectionOutcome::Retryable,
                reason.retryable(),
                "{reason:?} must not be retryable unless its class is"
            );
        }
    }
}
