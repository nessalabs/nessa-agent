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

use super::restart::{self, Restart};
use super::{startup_failure, Launch};

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

/// End the process on `error`: leave the reason where the desktop host will
/// find it, decide whether launchd should start this service again, and say
/// why.
///
/// The log line is for a person reading it later. The exit status is for
/// launchd, and through it for the desktop host, which has no other way to
/// learn why this process stopped.
pub(super) fn report(error: RunError, launch: &Launch) -> std::process::ExitCode {
    let ending = end(&error, launch);
    tracing::error!(
        %error,
        exit_code = ending.status,
        reason = super::exit_code::reason(&error),
        restart = ?restart::restart(&error),
        recorded = ending.recorded,
        "nessa failed"
    );
    std::process::ExitCode::from(ending.status)
}

/// How a run ends, as two facts rather than a number with a secret.
#[derive(Debug, PartialEq, Eq)]
struct Ending {
    /// What the process exits with, and what launchd therefore reads.
    status: u8,
    /// Whether the reason this status could not carry is durably on disk.
    recorded: bool,
}

/// Publish the recovery evidence, then choose the ending it permits.
///
/// Exiting zero is how this process tells launchd not to start it again —
/// `KeepAlive: { SuccessfulExit: false }` is launchd's only exit condition, and
/// zero is the only thing it reads as "stop". Nothing else will start the
/// service afterwards except the desktop host's next reconciliation, and the
/// only thing that authorizes *that* is the record. So the record is published
/// first, and a publication that fails keeps the non-zero code from the shared
/// table: launchd goes on retrying, which is the old loop and is survivable,
/// where a silent exit zero would be a service nobody can start again.
///
/// A standalone run never takes this path at all. Its exit status is the one
/// the table gives it, for whatever ran it.
fn end(error: &RunError, launch: &Launch) -> Ending {
    let code = super::exit_code::exit_code(error);
    let retryable = Ending {
        status: code,
        recorded: false,
    };
    let (Restart::Pointless, Some(managed)) = (restart::restart(error), launch.managed()) else {
        return retryable;
    };
    match startup_failure::record(error, managed) {
        Ok(()) => Ending {
            status: 0,
            recorded: true,
        },
        Err(failure) => {
            tracing::error!(
                %failure,
                "could not record why the gateway stopped for good; leaving it retryable so it can still be recovered"
            );
            retryable
        }
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

    use super::super::launch::Managed;
    use nessa_auth::adapters::local::LocalStoreError;

    const GENERATION: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn managed(logs: &std::path::Path) -> Launch {
        Launch::Managed(Managed::new(GENERATION.into(), logs.join("logs")))
    }
    fn taken_port() -> RunError {
        RunError::Bind {
            addr: "127.0.0.1:7420".into(),
            source: io::Error::from(ErrorKind::AddrInUse),
        }
    }

    /// The two endings of a managed launch, told apart. A registry this build
    /// cannot read stops the relaunch loop by exiting zero and leaves its
    /// reason behind; a port somebody else is holding keeps the code launchd
    /// reports and is started again.
    #[test]
    fn a_failure_that_retrying_cannot_fix_stops_the_service_and_says_why_elsewhere() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let launch = managed(directory.path());
        assert_eq!(
            end(&RunError::Registry(LocalStoreError::Corrupt), &launch),
            Ending {
                status: 0,
                recorded: true
            }
        );
        assert_eq!(
            end(&taken_port(), &launch),
            Ending {
                status: super::super::exit_code::exit_code(&taken_port()),
                recorded: false
            }
        );
        // Whatever the code is, a retryable failure never exits successfully:
        // launchd would read that as a service that meant to stop.
        assert_ne!(
            end(
                &RunError::Serve(io::Error::from(ErrorKind::BrokenPipe)),
                &launch
            )
            .status,
            0
        );
    }

    /// Exiting zero is only for the service launchd supervises. A `nessa`
    /// command someone typed reports the same failure with the code the shared
    /// table gives it, or a script and a supervisor read the failure as a
    /// success.
    #[test]
    fn a_command_nobody_supervises_never_reports_a_failure_as_success() {
        let configuration = || {
            RunError::Environment(EnvironmentError::Empty {
                variable: crate::env::HOST,
            })
        };
        let standalone = end(&configuration(), &Launch::Standalone);
        assert_eq!(
            standalone,
            Ending {
                status: super::super::exit_code::exit_code(&configuration()),
                recorded: false
            }
        );
        assert_ne!(standalone.status, 0);

        // The same failure under launchd is the one that stops the loop.
        let directory = tempfile::tempdir().expect("temporary directory");
        assert_eq!(
            end(&configuration(), &managed(directory.path())),
            Ending {
                status: 0,
                recorded: true
            }
        );
    }

    /// The record is the only way back for a service launchd has been told not
    /// to restart, so an ending that could not publish one must not be the
    /// ending that stops the restarts. Retrying is the old loop; it is
    /// survivable, and a service nobody can start again is not.
    #[test]
    fn a_failure_that_could_not_be_recorded_stays_retryable() {
        let directory = tempfile::tempdir().expect("temporary directory");
        // A file where the log directory should be: nothing can be published
        // under it, on any machine, without arranging permissions or a full disk.
        std::fs::write(directory.path().join("logs"), b"not a directory").expect("obstruction");
        let ending = end(
            &RunError::Registry(LocalStoreError::Corrupt),
            &managed(directory.path()),
        );
        assert!(!ending.recorded);
        assert_eq!(
            ending.status,
            super::super::exit_code::exit_code(&RunError::Registry(LocalStoreError::Corrupt))
        );
        assert_ne!(ending.status, 0);
    }

    /// The thing someone does while diagnosing a gateway that will not start
    /// is run `nessa server` in the same data directory. That process must not
    /// take away, or write over, the record the desktop host is going to
    /// recover the registered service by.
    #[test]
    fn a_standalone_run_leaves_a_registration_s_recovery_record_alone() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let registered = managed(directory.path());
        let logs = directory.path().join("logs");
        startup_failure::record(
            &RunError::Registry(LocalStoreError::Corrupt),
            registered.managed().expect("managed"),
        )
        .expect("record published");
        let record = logs.join("gateway-startup-failure.json");
        let published = std::fs::read(&record).expect("record");

        // A whole failing standalone start in that namespace: it forgets on
        // the way in, and reports the same permanent failure on the way out.
        Launch::Standalone.forget_startup_failure();
        let ending = end(
            &RunError::Environment(EnvironmentError::Empty { variable: HOST }),
            &Launch::Standalone,
        );

        assert_eq!(std::fs::read(&record).expect("record"), published);
        assert!(!ending.recorded);
        assert_ne!(ending.status, 0);
        // And the registration's own launch still finds what authorizes its
        // retry: the generation it names, unchanged.
        assert!(String::from_utf8_lossy(&published).contains(GENERATION));
    }

    #[test]
    fn display_environment_error() {
        let error = RunError::Environment(EnvironmentError::Empty { variable: HOST });
        assert!(error.to_string().contains(HOST));
    }
}
