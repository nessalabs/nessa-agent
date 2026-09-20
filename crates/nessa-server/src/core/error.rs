//! Fatal process errors — config, bind, and serve failures.
//!
//! Returned from [`super::bootstrap::run`] and [`crate::composition::CompositionRoot::serve`].
//! [`report`] turns one into how the process ends: a logged sentence, an exit
//! status launchd reads, and — when starting again would not help — a record
//! the desktop host reads instead of that status.
//!
//! Re-exported at `crate::core::RunError`.

use crate::conversation::application::ConversationError;
use crate::env::EnvironmentError;
use nessa_auth::adapters::local::LocalStoreError;
use std::fmt;
use std::io::{self, ErrorKind};
use std::path::Path;

use super::restart::{self, Restart};
use super::startup_failure;

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

/// End the process on `error`: say why, decide whether launchd should start it
/// again, and leave the reason where the desktop host will find it.
///
/// The log line is for a person reading it later. The exit status is for
/// launchd, and through it for the desktop host, which has no other way to
/// learn why this process stopped. A failure that retrying cannot fix exits
/// zero, because `KeepAlive: { SuccessfulExit: false }` is the only exit
/// condition launchd has and zero is the only way to say "do not start me
/// again" to it — so the reason that status cannot carry is written down
/// instead, in `logs`. A process with no log directory has nowhere to write it
/// and still stops; the host then has only the generic sentence.
pub(super) fn report(error: RunError, logs: Option<&Path>) -> std::process::ExitCode {
    let ending = ending(&error);
    tracing::error!(
        %error,
        exit_code = ending.status,
        reason = super::exit_code::reason(&error),
        restart = ?restart::restart(&error),
        "nessa failed"
    );
    if ending.recorded {
        match logs {
            Some(logs) => startup_failure::record(&error, logs),
            None => tracing::error!("no log directory to record why the gateway stopped for good"),
        }
    }
    std::process::ExitCode::from(ending.status)
}

/// How a run ends, as two facts rather than a number with a secret.
#[derive(Debug, PartialEq, Eq)]
struct Ending {
    /// What the process exits with, and what launchd therefore reads.
    status: u8,
    /// Whether the reason has to be written down because that status cannot
    /// carry it.
    recorded: bool,
}

fn ending(error: &RunError) -> Ending {
    match restart::restart(error) {
        Restart::Worthwhile => Ending {
            status: super::exit_code::exit_code(error),
            recorded: false,
        },
        // Zero is not success here. It is the only thing
        // `KeepAlive: { SuccessfulExit: false }` reads as "do not start me
        // again", which is why the reason goes somewhere the status cannot.
        Restart::Pointless => Ending {
            status: 0,
            recorded: true,
        },
    }
}

#[cfg(test)]
mod tests {
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

    use super::*;
    use crate::env::{EnvironmentError, HOST};

    /// The two endings, told apart. A registry this build cannot read stops
    /// the relaunch loop by exiting zero and leaves its reason behind; a port
    /// somebody else is holding keeps the code launchd reports and is started
    /// again.
    #[test]
    fn a_failure_that_retrying_cannot_fix_stops_the_service_and_says_why_elsewhere() {
        assert_eq!(
            ending(&RunError::Registry(LocalStoreError::Corrupt)),
            Ending {
                status: 0,
                recorded: true
            }
        );
        assert_eq!(
            ending(&RunError::Bind {
                addr: "127.0.0.1:7420".into(),
                source: io::Error::from(ErrorKind::AddrInUse),
            }),
            Ending {
                status: super::super::exit_code::exit_code(&RunError::Bind {
                    addr: "127.0.0.1:7420".into(),
                    source: io::Error::from(ErrorKind::AddrInUse),
                }),
                recorded: false
            }
        );
        // Whatever the code is, a retryable failure never exits successfully:
        // launchd would read that as a service that meant to stop.
        assert_ne!(
            ending(&RunError::Serve(io::Error::from(ErrorKind::BrokenPipe))).status,
            0
        );
    }

    #[test]
    fn display_environment_error() {
        let error = RunError::Environment(EnvironmentError::Empty { variable: HOST });
        assert!(error.to_string().contains(HOST));
    }
}
