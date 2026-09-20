//! What the gateway writes down when starting it again would not help.
//!
//! The exit code is how this process normally says why it stopped: launchd
//! records it and the desktop host reads it back (see [`super::exit_code`]).
//! That channel is unavailable for exactly the failures this file exists for.
//! launchd's only exit condition is `KeepAlive: { SuccessfulExit: false }`, so
//! the one way to tell it not to start this service again is to exit zero — and
//! zero carries no reason at all.
//!
//! So a run that gives up for good leaves the reason in the stage's log
//! directory, beside the log it was writing into, and the host reads that file
//! when launchd has nothing to tell it. The reason is a name from
//! `protocol/defaults/gateway-exit-codes.json`, the same vocabulary the exit
//! code would have carried; the message is prose for a person and decides
//! nothing.
//!
//! ```text
//! run gives up ──► gateway-startup-failure.json ──► desktop host's sentence
//!        │                      ▲
//!   exit status 0       forgotten by the next run that starts serving
//! ```
//!
//! A record is only ever about the run before this one. Every run that reaches
//! the point of serving forgets it first, and the record names the launchd
//! service generation it belonged to, so a host reconciling a different
//! registration cannot be told about a failure that was not its service's.
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::{exit_code, RunError};

/// The record's name inside the stage's log directory. The desktop host looks
/// for this name beside `gateway.log`, so the two sides agree on it here.
const FILE: &str = "gateway-startup-failure.json";

/// Why the last run of this gateway stopped for good.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StartupFailure<'a> {
    /// The name in the shared exit-code table — `credentialRegistryInvalid`,
    /// `configuration` — which is what the host turns into a sentence.
    reason: &'a str,
    /// The code this failure would have exited with had retrying been worth it.
    /// Carried so the record and the table can be read against each other.
    exit_code: u8,
    /// The failure in its own words, for the app's log. Never parsed.
    message: String,
    /// The launchd service generation this run was registered under, from
    /// `NESSA_SERVICE_GENERATION`. Absent for a server nobody registered, whose
    /// failures are nothing for a host to report.
    #[serde(skip_serializing_if = "Option::is_none")]
    service_generation: Option<String>,
    /// The process that wrote this, for reading the log beside it.
    process_id: u32,
}

fn path(logs: &Path) -> PathBuf {
    logs.join(FILE)
}

/// Write down why this run gave up, replacing whatever the last one left.
///
/// A record that cannot be written costs the host its sentence, not the
/// process its ending: the failure is already logged and the exit status is
/// already decided, so this reports and returns.
pub(super) fn record(error: &RunError, logs: &Path) {
    if let Err(failure) = write(&entry(error), logs) {
        tracing::error!(%failure, "could not record why the gateway stopped for good");
    }
}

fn entry(error: &RunError) -> StartupFailure<'static> {
    StartupFailure {
        reason: exit_code::reason(error),
        exit_code: exit_code::exit_code(error),
        message: error.to_string(),
        service_generation: std::env::var("NESSA_SERVICE_GENERATION").ok(),
        process_id: std::process::id(),
    }
}

fn write(record: &StartupFailure<'_>, logs: &Path) -> std::io::Result<()> {
    nessa_local_storage::create_directory(logs)?;
    let mut file = nessa_local_storage::PrivateTempFile::new_in(logs)?;
    serde_json::to_writer(file.as_file_mut(), record)?;
    file.as_file().sync_all()?;
    file.persist(&path(logs))?;
    nessa_local_storage::sync_directory(logs)
}

/// Forget the last run's record, on the way into serving.
///
/// Called by the composition root before it starts, so that a record found
/// afterwards belongs to the run that just ended and not to one from last week.
/// A record that cannot be removed is reported: the host would otherwise be
/// told about a failure that has already been fixed.
pub fn forget(logs: &Path) {
    match std::fs::remove_file(path(logs)) {
        Ok(()) => {}
        // No record is the ordinary case: most runs did not give up.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            tracing::error!(%error, "could not forget the last gateway startup failure");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nessa_auth::adapters::local::LocalStoreError;
    use serde_json::Value;

    /// A stage's log directory that does not exist yet, which is what a first
    /// run finds. Writing the record is what creates it, privately.
    struct Stage(tempfile::TempDir);
    impl Stage {
        fn new() -> Self {
            Self(tempfile::tempdir().expect("temporary directory"))
        }
        fn logs(&self) -> PathBuf {
            self.0.path().join("logs")
        }
    }

    fn written(error: &RunError, logs: &Path) -> Value {
        write(&entry(error), logs).expect("record written");
        serde_json::from_slice(&std::fs::read(path(logs)).expect("record")).expect("json")
    }

    /// The reason the host reads is the one the exit code would have carried,
    /// and the code is written down beside it even though the process is about
    /// to exit zero.
    #[test]
    fn the_record_carries_the_reason_the_exit_code_could_not() {
        let stage = Stage::new();
        let logs = &stage.logs();
        let record = written(&RunError::Registry(LocalStoreError::Corrupt), logs);
        assert_eq!(record["reason"], "credentialRegistryInvalid");
        assert_eq!(record["exitCode"], 28);
        assert!(
            record["message"]
                .as_str()
                .expect("message")
                .contains("authentication setup failed"),
            "{record}"
        );
        assert_eq!(record["processId"], std::process::id());
    }

    /// One record, about the last run. A second failure replaces the first
    /// rather than leaving the host two answers to choose between.
    #[test]
    fn a_later_failure_replaces_the_earlier_one_and_serving_forgets_it() {
        let stage = Stage::new();
        let logs = &stage.logs();
        written(&RunError::Registry(LocalStoreError::Corrupt), logs);
        let record = written(
            &RunError::Environment(crate::env::EnvironmentError::Empty {
                variable: crate::env::HOST,
            }),
            logs,
        );
        assert_eq!(record["reason"], "configuration");

        forget(logs);
        assert!(!path(logs).exists());
        // Forgetting what is not there is what a first run does.
        forget(logs);
    }

    /// Nothing but the record is left behind: a temporary file that outlived
    /// its write would fail the private-file checks the host reads through.
    #[test]
    fn writing_the_record_leaves_no_temporary_behind() {
        let stage = Stage::new();
        let logs = &stage.logs();
        written(&RunError::Registry(LocalStoreError::Corrupt), logs);
        let names: Vec<_> = std::fs::read_dir(logs)
            .expect("directory")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        assert_eq!(names, [FILE]);
        // And it is readable through the same private-file rules the desktop
        // host opens it with.
        assert!(
            nessa_local_storage::open(&path(logs), nessa_local_storage::OpenMode::Read).is_ok()
        );
    }
}
