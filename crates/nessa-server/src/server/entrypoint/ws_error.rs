//! Typed classification of WebSocket stream read failures.
//!
//! Axum wraps [`tungstenite::Error`] in [`axum::Error`]. We downcast the cause
//! chain and match variants / [`std::io::ErrorKind`] — never `Display` text.

use std::error::Error as StdError;
use std::io::ErrorKind;

use axum::Error as AxumError;
use tungstenite::error::{Error as TungsteniteError, ProtocolError};

/// Outcome of a failed WebSocket read, for logging only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WsReadFailure {
    /// Peer is gone (reload, quit, reset). Expected teardown, not a server fault.
    Disconnected,
    /// Unexpected protocol or I/O failure worth a warning.
    Unexpected,
}

impl WsReadFailure {
    pub(crate) fn classify(error: &AxumError) -> Self {
        let mut cause: Option<&(dyn StdError + 'static)> = Some(error);
        while let Some(err) = cause {
            if let Some(ws) = err.downcast_ref::<TungsteniteError>() {
                return Self::from_tungstenite(ws);
            }
            if let Some(io) = err.downcast_ref::<std::io::Error>() {
                if is_peer_gone(io.kind()) {
                    return Self::Disconnected;
                }
            }
            cause = err.source();
        }
        Self::Unexpected
    }

    fn from_tungstenite(error: &TungsteniteError) -> Self {
        match error {
            TungsteniteError::ConnectionClosed => Self::Disconnected,
            TungsteniteError::Protocol(ProtocolError::ResetWithoutClosingHandshake) => {
                Self::Disconnected
            }
            TungsteniteError::Io(io) if is_peer_gone(io.kind()) => Self::Disconnected,
            _ => Self::Unexpected,
        }
    }
}

fn is_peer_gone(kind: ErrorKind) -> bool {
    matches!(
        kind,
        ErrorKind::ConnectionReset
            | ErrorKind::ConnectionAborted
            | ErrorKind::BrokenPipe
            | ErrorKind::UnexpectedEof
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn protocol_reset_without_handshake_is_disconnect() {
        let error = TungsteniteError::Protocol(ProtocolError::ResetWithoutClosingHandshake);
        assert_eq!(
            WsReadFailure::from_tungstenite(&error),
            WsReadFailure::Disconnected
        );
    }

    #[test]
    fn normal_close_is_disconnect() {
        assert_eq!(
            WsReadFailure::from_tungstenite(&TungsteniteError::ConnectionClosed),
            WsReadFailure::Disconnected
        );
    }

    #[test]
    fn peer_reset_io_is_disconnect() {
        let error = TungsteniteError::Io(io::Error::new(ErrorKind::ConnectionReset, "reset"));
        assert_eq!(
            WsReadFailure::from_tungstenite(&error),
            WsReadFailure::Disconnected
        );
    }

    #[test]
    fn broken_pipe_io_is_disconnect() {
        let error = TungsteniteError::Io(io::Error::new(ErrorKind::BrokenPipe, "pipe"));
        assert_eq!(
            WsReadFailure::from_tungstenite(&error),
            WsReadFailure::Disconnected
        );
    }

    #[test]
    fn utf8_failure_is_unexpected() {
        let error = TungsteniteError::Utf8("bad".into());
        assert_eq!(
            WsReadFailure::from_tungstenite(&error),
            WsReadFailure::Unexpected
        );
    }

    #[test]
    fn already_closed_is_unexpected() {
        assert_eq!(
            WsReadFailure::from_tungstenite(&TungsteniteError::AlreadyClosed),
            WsReadFailure::Unexpected
        );
    }

    #[test]
    fn classify_walks_axum_wrapper() {
        let inner = TungsteniteError::Protocol(ProtocolError::ResetWithoutClosingHandshake);
        let wrapped = AxumError::new(inner);
        assert_eq!(WsReadFailure::classify(&wrapped), WsReadFailure::Disconnected);
    }

    #[test]
    fn classify_unknown_axum_error_is_unexpected() {
        let wrapped = AxumError::new(io::Error::other("something else"));
        assert_eq!(WsReadFailure::classify(&wrapped), WsReadFailure::Unexpected);
    }
}
