//! How a run of this process ends, for whatever is supervising it.
//!
//! An exit status is a number with one meaning for a person reading a script
//! and another for launchd, which is told `KeepAlive: { SuccessfulExit: false }`
//! and therefore reads zero — and only zero — as "do not start me again". That
//! makes the status a decision rather than a formality for the service launchd
//! supervises, and nothing at all for a `nessa` command someone typed. Both
//! endings are decided here, together, from [`super::Launch`].
//!
//! ```text
//!                      managed by launchd            standalone
//!  served, asked to    non-zero: come back unless     0: what a command
//!  stop                the job was unloaded              that worked exits
//!  failed, retrying    the table's code               the table's code
//!  could help
//!  failed, retrying    0 — but only once the reason   the table's code
//!  cannot help         is durably recorded
//! ```
use std::process::ExitCode;

use super::restart::{self, Restart};
use super::{exit_code, startup_failure, Launch, RunError};

/// How a run ends, as two facts rather than a number with a secret.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Ending {
    /// What the process exits with, and what launchd therefore reads.
    pub status: u8,
    /// Whether the reason this status could not carry is durably on disk.
    pub recorded: bool,
}

/// End the process on `outcome`: leave any reason where the desktop host will
/// find it, decide whether launchd should start this service again, and say so.
///
/// The log line is for a person reading it later. The exit status is for
/// whatever is supervising this process, and for launchd that is the only way
/// it learns what to do next.
pub(super) fn report(outcome: Result<(), RunError>, launch: &Launch) -> ExitCode {
    let ending = match &outcome {
        Ok(()) => {
            let ending = stopped(launch);
            tracing::info!(
                exit_code = ending.status,
                starts_again = ending.status != 0,
                "nessa stopped"
            );
            ending
        }
        Err(error) => {
            let ending = failed(error, launch);
            tracing::error!(
                %error,
                exit_code = ending.status,
                reason = exit_code::reason(error),
                restart = ?restart::restart(error),
                recorded = ending.recorded,
                "nessa failed"
            );
            ending
        }
    };
    ExitCode::from(ending.status)
}

/// The ending of a run that served and was then asked to stop.
///
/// For a command someone ran themselves that is success, and success is zero.
///
/// For the service launchd supervises it is not, because being asked to stop is
/// not the same as being told to stay stopped, and zero is the only way to say
/// the second. `launchctl bootout` is how that service is stopped: it unloads
/// the job, so this status cannot resurrect a service that was being removed —
/// verified against launchd on macOS 26, where a job booted out while exiting
/// non-zero is gone from launchd with one run to its name. A SIGTERM from
/// anywhere else — Activity Monitor, `kill`, `launchctl kill` — brings the
/// gateway back a throttle interval later, which is what it did before this
/// exit status meant anything, and is what keeps a service that was merely
/// killed from needing a hand-run `bootout` to come back at all.
fn stopped(launch: &Launch) -> Ending {
    Ending {
        status: match launch.managed() {
            Some(_) => exit_code::code(exit_code::STOPPED_ON_REQUEST),
            None => 0,
        },
        recorded: false,
    }
}

/// Publish the recovery evidence, then choose the ending it permits.
///
/// Nothing will start the service again after a zero except the desktop host's
/// next reconciliation, and the only thing that authorizes *that* is the
/// record. So the record is published first, and a publication that fails keeps
/// the non-zero code from the shared table: launchd goes on retrying, which is
/// the old loop and is survivable, where a silent exit zero would be a service
/// nobody can start again.
///
/// A standalone run never takes this path at all. Its exit status is the one
/// the table gives it, for whatever ran it.
fn failed(error: &RunError, launch: &Launch) -> Ending {
    let retryable = Ending {
        status: exit_code::exit_code(error),
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
    use super::exit_code::{code, exit_code, STOPPED_ON_REQUEST};
    use super::startup_failure::record;
    use super::{failed, stopped, Ending, Launch, RunError};
    use crate::core::launch::Managed;
    use crate::env::{EnvironmentError, HOST};
    use nessa_auth::adapters::local::LocalStoreError;
    use std::io::{Error, ErrorKind};
    use std::path::{Path, PathBuf};

    const GENERATION: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const RECORD: &str = "gateway-startup-failure.json";

    fn managed(home: &Path) -> Launch {
        Launch::Managed(Managed::new(GENERATION.into(), home.join("logs")))
    }
    fn configuration() -> RunError {
        RunError::Environment(EnvironmentError::Empty { variable: HOST })
    }
    fn taken_port() -> RunError {
        RunError::Bind {
            addr: "127.0.0.1:7420".into(),
            source: Error::from(ErrorKind::AddrInUse),
        }
    }
    fn obstructed() -> tempfile::TempDir {
        let home = tempfile::tempdir().expect("temporary directory");
        // A file where the log directory should be: nothing can be published
        // under it, on any machine, without arranging permissions or a full disk.
        std::fs::write(home.path().join("logs"), b"not a directory").expect("obstruction");
        home
    }

    /// Being asked to stop is not being told to stay stopped. Under
    /// `KeepAlive: { SuccessfulExit: false }` a zero here is the second, so a
    /// gateway someone kills from Activity Monitor would never come back and
    /// nothing but a hand-run `launchctl bootout` could make it startable
    /// again. `bootout` unloads the job before this status is read, so the
    /// service that is meant to stop still stops.
    #[test]
    fn a_managed_gateway_asked_to_stop_is_not_asking_to_stay_stopped() {
        let home = tempfile::tempdir().expect("temporary directory");
        let ending = stopped(&managed(home.path()));
        assert_ne!(ending.status, 0);
        assert_eq!(ending.status, code(STOPPED_ON_REQUEST));
        assert!(!ending.recorded);
        // Nothing is written down for it: the service is coming back, so there
        // is no recovery for the host to authorize.
        assert!(!home.path().join("logs").join(RECORD).exists());

        // A server someone ran themselves stopped because they stopped it.
        assert_eq!(
            stopped(&Launch::Standalone),
            Ending {
                status: 0,
                recorded: false
            }
        );
    }

    /// The two failing endings of a managed launch, told apart. A registry
    /// this build cannot read stops the relaunch loop by exiting zero and
    /// leaves its reason behind; a port somebody else is holding keeps the
    /// code launchd reports and is started again.
    #[test]
    fn a_failure_that_retrying_cannot_fix_stops_the_service_and_says_why_elsewhere() {
        let home = tempfile::tempdir().expect("temporary directory");
        let launch = managed(home.path());
        assert_eq!(
            failed(&RunError::Registry(LocalStoreError::Corrupt), &launch),
            Ending {
                status: 0,
                recorded: true
            }
        );
        assert_eq!(
            failed(&taken_port(), &launch),
            Ending {
                status: exit_code(&taken_port()),
                recorded: false
            }
        );
        // Whatever the code is, a retryable failure never exits successfully:
        // launchd would read that as a service that meant to stop.
        assert_ne!(
            failed(
                &RunError::Serve(Error::from(ErrorKind::BrokenPipe)),
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
        let standalone = failed(&configuration(), &Launch::Standalone);
        assert_eq!(
            standalone,
            Ending {
                status: exit_code(&configuration()),
                recorded: false
            }
        );
        assert_ne!(standalone.status, 0);

        // The same failure under launchd is the one that stops the loop.
        let home = tempfile::tempdir().expect("temporary directory");
        assert_eq!(
            failed(&configuration(), &managed(home.path())),
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
        let home = obstructed();
        let corrupt = || RunError::Registry(LocalStoreError::Corrupt);
        let ending = failed(&corrupt(), &managed(home.path()));
        assert!(!ending.recorded);
        assert_eq!(ending.status, exit_code(&corrupt()));
        assert_ne!(ending.status, 0);
    }

    /// The thing someone does while diagnosing a gateway that will not start
    /// is run `nessa server` in the same data directory. That process must not
    /// take away, or write over, the record the desktop host is going to
    /// recover the registered service by.
    ///
    /// This is the pair of decisions such a run makes — `forget` on the way in
    /// and its ending on the way out — not the whole of `serve_runtime`, which
    /// needs a listening server to reach them.
    #[test]
    fn a_standalone_run_leaves_a_registration_s_recovery_record_alone() {
        let home = tempfile::tempdir().expect("temporary directory");
        let registered = managed(home.path());
        record(
            &RunError::Registry(LocalStoreError::Corrupt),
            registered.managed().expect("managed"),
        )
        .expect("record published");
        let path: PathBuf = home.path().join("logs").join(RECORD);
        let published = std::fs::read(&path).expect("record");

        Launch::Standalone.forget_startup_failure();
        let ending = failed(&configuration(), &Launch::Standalone);

        assert_eq!(std::fs::read(&path).expect("record"), published);
        assert!(!ending.recorded);
        assert_ne!(ending.status, 0);
        // And the registration's own launch still finds what authorizes its
        // retry: the generation it names, unchanged.
        assert!(String::from_utf8_lossy(&published).contains(GENERATION));
    }
}
