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
//! **This file is not a diagnostic.** It is the only thing that will ever get a
//! service that gave up started again: launchd has been told not to, so the
//! desktop host's next reconciliation is the last route back, and this record
//! is what authorizes it. A record that could not be published therefore has to
//! change how the process ends rather than be logged and forgotten — see
//! [`super::error::report`], which publishes before it chooses an exit status.
//!
//! ```text
//! run gives up ──► published? ──yes──► exit 0, host reads it, retries once
//!                       │
//!                       └──no──► keep the non-zero code; launchd keeps trying
//! ```
//!
//! Only a managed launch has a record at all, and only the one its own
//! generation wrote. A standalone `nessa server` sharing the same data
//! directory — the thing someone runs while diagnosing exactly this problem —
//! must not publish evidence in a registration's name or erase the evidence
//! that registration is relying on. [`super::Launch`] is where that is decided.
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::launch::Managed;
use super::{exit_code, RunError};

/// The record's name inside the stage's log directory. The desktop host looks
/// for this name beside `gateway.log`, so the two sides agree on it here.
const FILE: &str = "gateway-startup-failure.json";

/// Why the last run of this gateway stopped for good.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StartupFailure {
    /// The name in the shared exit-code table — `credentialRegistryInvalid`,
    /// `configuration` — which is what the host turns into a sentence.
    reason: String,
    /// The code this failure would have exited with had retrying been worth it.
    /// Carried so the record and the table can be read against each other; the
    /// host refuses a record where they disagree.
    exit_code: u8,
    /// The failure in its own words, for the app's log. Never parsed.
    message: String,
    /// The launchd service generation this run was registered under. What makes
    /// the record evidence about one registration rather than about whatever
    /// last wrote in this directory.
    service_generation: String,
    /// The process that wrote this, for reading the log beside it.
    process_id: u32,
}

fn path(logs: &Path) -> PathBuf {
    logs.join(FILE)
}

/// Publish why this run gave up, replacing whatever the last one left.
///
/// Returns once the record is durably on disk under its own name, with the
/// directory synced, because the caller's next decision is whether launchd may
/// stop restarting this service — and it may only do that if this succeeded.
pub(super) fn record(error: &RunError, managed: &Managed) -> io::Result<()> {
    let record = StartupFailure {
        reason: exit_code::reason(error).to_owned(),
        exit_code: exit_code::exit_code(error),
        message: error.to_string(),
        service_generation: managed.generation().to_owned(),
        process_id: std::process::id(),
    };
    let logs = managed.logs();
    nessa_local_storage::create_directory(logs)?;
    let mut file = nessa_local_storage::PrivateTempFile::new_in(logs)?;
    serde_json::to_writer(file.as_file_mut(), &record)?;
    file.as_file().sync_all()?;
    file.persist(&path(logs))?;
    nessa_local_storage::sync_directory(logs)
}

/// Forget the record this launch supersedes, on the way into serving.
///
/// Only the one this launch's own generation wrote. A record another
/// registration left is not this process's to erase — it is what that
/// registration will be recovered by — and one this build cannot read is left
/// where it is rather than removed on a guess.
pub(super) fn forget(managed: &Managed) {
    let path = path(managed.logs());
    match read(&path) {
        // No record is the ordinary case: most runs did not give up.
        Ok(None) => return,
        Ok(Some(record)) if record.service_generation == managed.generation() => {}
        // A record that outlives a start it did not belong to is not wrong,
        // but it is evidence the host may act on later, so it is said out loud
        // rather than passed over in silence.
        Ok(Some(record)) => {
            return tracing::info!(
                generation = record.service_generation,
                "leaving a gateway startup failure recorded by another registration"
            )
        }
        Err(error) => {
            return tracing::error!(
                %error,
                "could not read the last gateway startup failure; leaving it where it is"
            )
        }
    }
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        // Gone between reading it and removing it is the outcome either way.
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            tracing::error!(%error, "could not forget the last gateway startup failure");
        }
    }
}

/// The record already there, if any.
///
/// Absence is an answer; a file that cannot be read or made sense of is not,
/// and it is kept apart from absence so the caller can say which it had.
fn read(path: &Path) -> io::Result<Option<StartupFailure>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(io::Error::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nessa_auth::adapters::local::LocalStoreError;
    use serde_json::Value;

    const GENERATION: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const OTHER: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    /// A stage's log directory that does not exist yet, which is what a first
    /// run finds. Writing the record is what creates it, privately.
    struct Stage(tempfile::TempDir);
    impl Stage {
        fn new() -> Self {
            Self(tempfile::tempdir().expect("temporary directory"))
        }
        fn managed(&self, generation: &str) -> Managed {
            Managed::new(generation.to_owned(), self.0.path().join("logs"))
        }
    }

    fn written(error: &RunError, managed: &Managed) -> Value {
        record(error, managed).expect("record written");
        serde_json::from_slice(&std::fs::read(path(managed.logs())).expect("record")).expect("json")
    }

    /// The reason the host reads is the one the exit code would have carried,
    /// and the code is written down beside it even though the process is about
    /// to exit zero. The generation is what makes it about one registration.
    #[test]
    fn the_record_carries_the_reason_the_exit_code_could_not() {
        let stage = Stage::new();
        let managed = stage.managed(GENERATION);
        let record = written(&RunError::Registry(LocalStoreError::Corrupt), &managed);
        assert_eq!(record["reason"], "credentialRegistryInvalid");
        assert_eq!(record["exitCode"], 28);
        assert_eq!(record["serviceGeneration"], GENERATION);
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
        let managed = stage.managed(GENERATION);
        written(&RunError::Registry(LocalStoreError::Corrupt), &managed);
        let record = written(
            &RunError::Environment(crate::env::EnvironmentError::Empty {
                variable: crate::env::HOST,
            }),
            &managed,
        );
        assert_eq!(record["reason"], "configuration");

        forget(&managed);
        assert!(!path(managed.logs()).exists());
        // Forgetting what is not there is what a first run does.
        forget(&managed);
    }

    /// The record is a registration's way back, and only its own generation
    /// may take it away. A second gateway for another generation sharing this
    /// data directory leaves it exactly where it is.
    #[test]
    fn only_the_generation_a_record_names_may_forget_it() {
        let stage = Stage::new();
        let ours = stage.managed(GENERATION);
        written(&RunError::Registry(LocalStoreError::Corrupt), &ours);
        let before = std::fs::read(path(ours.logs())).expect("record");

        forget(&stage.managed(OTHER));
        assert_eq!(std::fs::read(path(ours.logs())).expect("record"), before);

        // Nor is a record this build cannot read removed on a guess: it is
        // inert evidence, not this process's to decide about.
        std::fs::write(path(ours.logs()), b"not a record").expect("write");
        forget(&ours);
        assert_eq!(
            std::fs::read(path(ours.logs())).expect("record"),
            b"not a record"
        );
    }

    /// Nothing but the record is left behind: a temporary file that outlived
    /// its write would fail the private-file checks the host reads through.
    #[test]
    fn writing_the_record_leaves_no_temporary_behind() {
        let stage = Stage::new();
        let managed = stage.managed(GENERATION);
        written(&RunError::Registry(LocalStoreError::Corrupt), &managed);
        let names: Vec<_> = std::fs::read_dir(managed.logs())
            .expect("directory")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        assert_eq!(names, [FILE]);
        // And it is readable through the same private-file rules the desktop
        // host opens it with.
        assert!(nessa_local_storage::open(
            &path(managed.logs()),
            nessa_local_storage::OpenMode::Read
        )
        .is_ok());
    }

    /// A publication that cannot happen is reported, not swallowed: the whole
    /// ending depends on it.
    #[test]
    fn a_record_that_cannot_be_written_says_so() {
        let stage = Stage::new();
        // A file where the log directory should be: nothing can be created
        // under it, on any machine, without permissions or a full disk.
        std::fs::write(stage.0.path().join("logs"), b"not a directory").expect("obstruction");
        let managed = stage.managed(GENERATION);
        assert!(record(&RunError::Registry(LocalStoreError::Corrupt), &managed).is_err());
    }
}
