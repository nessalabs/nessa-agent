//! Secret-free records of a linger choice, under the app's trusted config root.
//!
//! The sink mints the correlation. An intent file can exist without an outcome:
//! that is a process that quit while the prompt was open. The next process does
//! not read these files to decide the screen.

use std::{io::Write, path::PathBuf};

use nessa_local_storage::{
    create_private_directory_tree_beneath, sync_directory_beneath, PrivateTempFile,
};
use serde::Serialize;

use crate::linger::{
    application::{LingerAudit, LingerAuditFailure},
    domain::{
        LingerCause, LingerCorrelation, LingerDecline, LingerInitiator, LingerIntent,
        LingerObservation, LingerOutcome,
    },
};

/// Used when composition cannot resolve a directory beneath the trusted root.
///
/// An accept then cannot call logind, because the intent would not be recorded.
pub(crate) struct UnavailableLingerAudit;

impl LingerAudit for UnavailableLingerAudit {
    fn record_intent(&self, _: &LingerIntent) -> Result<LingerCorrelation, LingerAuditFailure> {
        Err(LingerAuditFailure)
    }

    fn record_outcome(
        &self,
        _: &LingerCorrelation,
        _: &LingerOutcome,
    ) -> Result<(), LingerAuditFailure> {
        Err(LingerAuditFailure)
    }

    fn record_decline(&self, _: &LingerDecline) -> Result<(), LingerAuditFailure> {
        Err(LingerAuditFailure)
    }
}

pub(crate) struct FileLingerAudit {
    trusted_root: PathBuf,
    relative_directory: PathBuf,
}

impl FileLingerAudit {
    pub(crate) fn beneath(trusted_root: PathBuf, relative_directory: PathBuf) -> Self {
        Self {
            trusted_root,
            relative_directory,
        }
    }

    fn publish<T: Serialize>(
        &self,
        correlation: &LingerCorrelation,
        suffix: &str,
        record: &T,
    ) -> Result<(), LingerAuditFailure> {
        create_private_directory_tree_beneath(&self.trusted_root, &self.relative_directory)
            .map_err(|_| LingerAuditFailure)?;
        let destination = self
            .relative_directory
            .join(format!("{}-{suffix}.json", correlation.as_str()));
        let bytes = serde_json::to_vec(record).map_err(|_| LingerAuditFailure)?;
        let mut temporary =
            PrivateTempFile::new_beneath(&self.trusted_root, &self.relative_directory)
                .map_err(|_| LingerAuditFailure)?;
        temporary
            .as_file_mut()
            .write_all(&bytes)
            .and_then(|()| temporary.as_file().sync_all())
            .map_err(|_| LingerAuditFailure)?;
        temporary
            .publish_new_beneath(&destination)
            .and_then(|()| sync_directory_beneath(&self.trusted_root, &self.relative_directory))
            .map_err(|_| LingerAuditFailure)
    }
}

impl LingerAudit for FileLingerAudit {
    fn record_intent(
        &self,
        intent: &LingerIntent,
    ) -> Result<LingerCorrelation, LingerAuditFailure> {
        let correlation = mint()?;
        self.publish(
            &correlation,
            "intent",
            &IntentRecord::from(intent, &correlation),
        )?;
        Ok(correlation)
    }

    fn record_outcome(
        &self,
        correlation: &LingerCorrelation,
        outcome: &LingerOutcome,
    ) -> Result<(), LingerAuditFailure> {
        self.publish(
            correlation,
            "outcome",
            &OutcomeRecord::from(outcome, correlation),
        )
    }

    fn record_decline(&self, decline: &LingerDecline) -> Result<(), LingerAuditFailure> {
        let correlation = mint()?;
        self.publish(
            &correlation,
            "decline",
            &DeclineRecord::from(decline, &correlation),
        )
    }
}

fn mint() -> Result<LingerCorrelation, LingerAuditFailure> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| LingerAuditFailure)?;
    let value = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    LingerCorrelation::parse(value).ok_or(LingerAuditFailure)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct IntentRecord<'a> {
    correlation: &'a str,
    user: u32,
    before: &'static str,
    cause: &'static str,
    initiator: &'static str,
}

impl<'a> IntentRecord<'a> {
    fn from(intent: &'a LingerIntent, correlation: &'a LingerCorrelation) -> Self {
        Self {
            correlation: correlation.as_str(),
            user: intent.user().get(),
            before: observation_name(intent.before()),
            cause: cause_name(intent.cause()),
            initiator: initiator_name(intent.initiator()),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OutcomeRecord<'a> {
    correlation: &'a str,
    user: u32,
    before: &'static str,
    after: &'static str,
    cause: &'static str,
    initiator: &'static str,
}

impl<'a> OutcomeRecord<'a> {
    fn from(outcome: &'a LingerOutcome, correlation: &'a LingerCorrelation) -> Self {
        Self {
            correlation: correlation.as_str(),
            user: outcome.user().get(),
            before: observation_name(outcome.before()),
            after: observation_name(outcome.after()),
            cause: cause_name(outcome.cause()),
            initiator: initiator_name(outcome.initiator()),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeclineRecord<'a> {
    correlation: &'a str,
    user: u32,
    before: &'static str,
    after: &'static str,
    cause: &'static str,
    initiator: &'static str,
}

impl<'a> DeclineRecord<'a> {
    fn from(decline: &'a LingerDecline, correlation: &'a LingerCorrelation) -> Self {
        Self {
            correlation: correlation.as_str(),
            user: decline.user().get(),
            before: observation_name(decline.before()),
            after: observation_name(decline.after()),
            cause: cause_name(decline.cause()),
            initiator: initiator_name(decline.initiator()),
        }
    }
}

fn observation_name(observation: LingerObservation) -> &'static str {
    match observation {
        LingerObservation::Enabled => "enabled",
        LingerObservation::Disabled => "disabled",
        LingerObservation::Unsupported => "unsupported",
        LingerObservation::Unreadable => "unreadable",
    }
}

fn cause_name(cause: LingerCause) -> &'static str {
    match cause {
        LingerCause::Enable => "enable",
        LingerCause::Succeeded => "succeeded",
        LingerCause::Cancelled => "cancelled",
        LingerCause::NotAuthorized => "not-authorized",
        LingerCause::TimedOut => "timed-out",
        LingerCause::Unavailable => "unavailable",
        LingerCause::Declined => "declined",
    }
}

fn initiator_name(initiator: LingerInitiator) -> &'static str {
    match initiator {
        LingerInitiator::Setup => "setup",
    }
}

#[cfg(test)]
mod tests {
    use super::{FileLingerAudit, UnavailableLingerAudit};
    use crate::linger::{
        application::LingerAudit,
        domain::{
            LingerCall, LingerDecline, LingerIntent, LingerObservation, LingerOutcome, LoginUserId,
        },
    };

    fn audit(root: &std::path::Path) -> FileLingerAudit {
        let trusted = root.join("private");
        nessa_local_storage::create_directory(&trusted).unwrap();
        FileLingerAudit::beneath(trusted, "linger-audit".into())
    }

    #[test]
    fn intent_and_outcome_name_the_user_and_keep_the_call_off_the_read() {
        let root = tempfile::tempdir().unwrap();
        let sink = audit(root.path());
        let intent =
            LingerIntent::enable(LoginUserId::new(1000), LingerObservation::Disabled).unwrap();
        let correlation = sink.record_intent(&intent).unwrap();
        let outcome = LingerOutcome::new(intent, LingerCall::Succeeded, LingerObservation::Enabled);
        sink.record_outcome(&correlation, &outcome).unwrap();

        let directory = root.path().join("private").join("linger-audit");
        let intent_file = directory.join(format!("{}-intent.json", correlation.as_str()));
        let outcome_file = directory.join(format!("{}-outcome.json", correlation.as_str()));
        let intent_json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&intent_file).unwrap()).unwrap();
        assert_eq!(intent_json["user"], 1000);
        assert_eq!(intent_json["before"], "disabled");
        assert_eq!(intent_json["cause"], "enable");
        assert_eq!(intent_json["initiator"], "setup");
        assert_eq!(intent_json["correlation"], correlation.as_str());
        let outcome_json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&outcome_file).unwrap()).unwrap();
        assert_eq!(outcome_json["after"], "enabled");
        assert_eq!(outcome_json["cause"], "succeeded");
        assert_eq!(outcome_json["before"], "disabled");
        assert_eq!(outcome_json["initiator"], "setup");
    }

    #[test]
    fn an_intent_can_exist_without_an_outcome() {
        let root = tempfile::tempdir().unwrap();
        let sink = audit(root.path());
        let intent =
            LingerIntent::enable(LoginUserId::new(4), LingerObservation::Disabled).unwrap();
        let correlation = sink.record_intent(&intent).unwrap();
        let directory = root.path().join("private").join("linger-audit");
        assert!(directory
            .join(format!("{}-intent.json", correlation.as_str()))
            .is_file());
        assert!(std::fs::read_dir(&directory).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with("-outcome.json")));
    }

    #[test]
    fn a_decline_records_no_call() {
        let root = tempfile::tempdir().unwrap();
        let sink = audit(root.path());
        let decline = LingerDecline::new(LoginUserId::new(7), LingerObservation::Disabled).unwrap();
        sink.record_decline(&decline).unwrap();
        let directory = root.path().join("private").join("linger-audit");
        let file = std::fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .ends_with("-decline.json")
            })
            .unwrap();
        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
        assert_eq!(json["cause"], "declined");
        assert_eq!(json["before"], "disabled");
        assert_eq!(json["after"], "disabled");
        assert_eq!(json["user"], 7);
        assert_eq!(json["initiator"], "setup");
    }

    #[test]
    fn a_trusted_root_that_is_a_file_does_not_record() {
        let root = tempfile::tempdir().unwrap();
        let trusted = root.path().join("private");
        nessa_local_storage::create_directory(&trusted).unwrap();
        let file = trusted.join("not-a-directory");
        std::fs::write(&file, b"x").unwrap();
        let sink = FileLingerAudit::beneath(file, "linger-audit".into());
        let intent =
            LingerIntent::enable(LoginUserId::new(1), LingerObservation::Disabled).unwrap();
        assert!(sink.record_intent(&intent).is_err());
    }

    #[test]
    fn an_unavailable_sink_records_nothing() {
        let sink = UnavailableLingerAudit;
        let intent =
            LingerIntent::enable(LoginUserId::new(1), LingerObservation::Disabled).unwrap();
        assert!(sink.record_intent(&intent).is_err());
        let decline = LingerDecline::new(LoginUserId::new(1), LingerObservation::Disabled).unwrap();
        assert!(sink.record_decline(&decline).is_err());
    }
}
