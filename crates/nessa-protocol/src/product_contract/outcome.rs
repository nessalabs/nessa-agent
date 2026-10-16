//! The one projection of a close reason into what the client does next.
use super::generated::SessionCloseReason;

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
