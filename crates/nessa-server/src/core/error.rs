//! Fatal process errors — config, bind, and serve failures.
//!
//! Returned from [`super::bootstrap::run`] and [`crate::composition::CompositionRoot::serve`].
//! [`super::ending`] turns one into how the process ends: a logged sentence, an
//! exit status launchd reads, and — when starting again would not help — a
//! record the desktop host reads instead of that status.
//!
//! Re-exported at `crate::core::RunError`.

use crate::conversation::application::ConversationError;
use crate::env::EnvironmentError;
use nessa_auth::adapters::local::LocalStoreError;
use std::fmt;
use std::io::{self, ErrorKind};

/// Fatal errors that stop the server process.
#[derive(Debug)]
pub enum RunError {
    Environment(EnvironmentError),
    /// The credential registry on disk could not be opened. Kept typed rather
    /// than flattened into `Authentication`, because it is the one setup
    /// failure the desktop host reports in its own words, and it learns which
    /// failure this was from the exit code this variant chooses.
    Registry(LocalStoreError),
    /// Product authentication failed to initialize; contains no credential material.
    Authentication(String),
    /// Invalid or unavailable configured agent provider.
    Agent(String),
    /// The prepared runtime this process was handed is missing, unreadable, or
    /// not the one its registration was fingerprinted against. Typed apart from
    /// `Agent` because nothing about starting again changes any of that, and
    /// the host has its own sentence for it.
    Runtime(String),
    Bind {
        addr: String,
        source: io::Error,
    },
    Serve(io::Error),
    /// Conversations did not confirm cleanup and audit delivery on the way down.
    /// The HTTP server itself finished; this is what shutdown could not prove.
    /// `None` means shutdown never reported at all — unknown, which is its own
    /// fact and not the same as a reported failure.
    Shutdown(Option<ConversationError>),
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Agent(message) => write!(f, "agent setup failed: {message}"),
            Self::Runtime(message) => write!(f, "prepared runtime unusable: {message}"),
            // Same sentence a flattened registry error used to produce: this
            // is still what authentication setup failed on.
            Self::Registry(error) => write!(f, "authentication setup failed: {error}"),
            Self::Authentication(message) => write!(f, "authentication setup failed: {message}"),
            Self::Environment(error) => write!(f, "invalid configuration: {error}"),
            Self::Bind { addr, source } => match source.kind() {
                ErrorKind::AddrInUse => write!(
                    f,
                    "port already in use at {addr}; stop the other process or set NESSA_PORT"
                ),
                _ => write!(f, "failed to bind {addr}: {source}"),
            },
            Self::Serve(source) => write!(f, "server stopped: {source}"),
            Self::Shutdown(Some(error)) => {
                write!(f, "shutdown did not confirm all cleanup: {error}")
            }
            Self::Shutdown(None) => {
                write!(f, "shutdown never reported whether cleanup completed")
            }
        }
    }
}

impl std::error::Error for RunError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Environment(error) => Some(error),
            Self::Registry(error) => Some(error),
            Self::Authentication(_) | Self::Agent(_) | Self::Runtime(_) => None,
            Self::Bind { source, .. } => Some(source),
            Self::Serve(source) => Some(source),
            Self::Shutdown(error) => error.as_ref().map(|error| error as _),
        }
    }
}

impl From<EnvironmentError> for RunError {
    fn from(value: EnvironmentError) -> Self {
        Self::Environment(value)
    }
}

impl From<io::Error> for RunError {
    fn from(value: io::Error) -> Self {
        Self::Serve(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::{EnvironmentError, HOST};

    #[test]
    fn an_unconfirmed_shutdown_says_which_kind_it_was() {
        let reported = RunError::Shutdown(Some(ConversationError::Audit));
        assert!(reported.to_string().contains("did not confirm all cleanup"));
        // The typed failure is the source, so a caller can match on it.
        assert!(std::error::Error::source(&reported).is_some());

        let silent = RunError::Shutdown(None);
        assert!(silent.to_string().contains("never reported"));
        // Nothing was reported, so there is nothing to be the source.
        assert!(std::error::Error::source(&silent).is_none());
    }

    #[test]
    fn display_environment_error() {
        let error = RunError::Environment(EnvironmentError::Empty { variable: HOST });
        assert!(error.to_string().contains(HOST));
    }
}
