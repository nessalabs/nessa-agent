//! The private JSON shape for install-audit records and its mapping through
//! domain constructors. Physical journal metadata stays separate from semantic
//! transition facts.

use std::ffi::OsStr;

use serde::{Deserialize, Serialize};

use crate::agent_install::{
    application::{AuditFailure, AuditFailureStage, PublishedAuditRecord},
    domain::{
        AgentName, ArchiveDigest, ArchivePath, FileRole, InstallEventIdentity, InstallEventSlot,
        InstallFailureEvidence, InstallFailureKind, InstallRequest, InstallTransition,
        InstallTransitionFacts, RecoveryFailureEvidence, RecoveryState, ReleaseContents,
        ReleaseFile, ReleaseVersion, RollbackState, RuntimeArtifact,
    },
};

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct StoredRecord {
    pub(super) record_id: String,
    pub(super) sequence: u64,
    observed_at_ms: u64,
    event: StoredEventIdentity,
    transition: StoredTransition,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredEventIdentity {
    account_id: String,
    request_id: String,
    slot: StoredSlot,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum StoredSlot {
    Started,
    VerificationOutcome,
    CompletionOutcome,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::agent_install::infrastructure) struct StoredTransition {
    agent: String,
    target: StoredArtifact,
    request: StoredRequest,
    facts: StoredFacts,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredRequest {
    account_id: String,
    request_id: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredArtifact {
    version: String,
    digest: String,
    files: Vec<StoredFile>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
struct StoredFile {
    path: String,
    role: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum StoredFacts {
    Started,
    Verified,
    DigestRejected {
        actual_digest: String,
    },
    Installed,
    Replaced {
        previous: StoredArtifact,
    },
    RolledBack {
        rollback: StoredRollback,
    },
    RecoveryIncomplete {
        state: StoredRecoveryState,
        failures: StoredRecoveryFailures,
    },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "artifact", rename_all = "snake_case")]
enum StoredRollback {
    Restored(StoredArtifact),
    NoInstalledRuntime,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "rollback", rename_all = "snake_case")]
enum StoredRecoveryState {
    Confirmed(StoredRollback),
    Unconfirmed,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
struct StoredRecoveryFailures {
    publication: StoredFailure,
    withdrawal: Option<StoredFailure>,
    restoration: Option<StoredFailure>,
    confirmation: Option<StoredFailure>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
struct StoredFailure {
    kind: StoredFailureKind,
    detail: String,
    truncated: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum StoredFailureKind {
    Unwritable,
    Unreadable,
    IncompleteArchive,
    MalformedArchive,
}

impl StoredRecord {
    pub(super) fn from_transition(
        record_id: String,
        sequence: u64,
        observed_at_ms: u64,
        transition: &InstallTransition,
    ) -> Self {
        Self {
            record_id,
            sequence,
            observed_at_ms,
            event: StoredEventIdentity::from(transition.event_identity()),
            transition: StoredTransition::from(transition),
        }
    }

    pub(super) fn restore_transition(&self) -> Result<InstallTransition, AuditFailure> {
        let transition = self.transition.restore()?;
        if self.event.restore()? != transition.event_identity() {
            return Err(record_failure(
                "stored event identity disagrees with its transition facts",
            ));
        }
        Ok(transition)
    }

    pub(super) fn published(
        &self,
        transition: &InstallTransition,
        name: &OsStr,
    ) -> Result<PublishedAuditRecord, AuditFailure> {
        let destination = name
            .to_str()
            .ok_or_else(|| record_failure("audit record name is not UTF-8"))?;
        Ok(PublishedAuditRecord::new(
            transition.event_identity(),
            self.record_id.clone(),
            self.sequence,
            destination.to_owned(),
        ))
    }
}

impl From<InstallEventIdentity> for StoredEventIdentity {
    fn from(value: InstallEventIdentity) -> Self {
        Self {
            account_id: value.request().account_id().to_owned(),
            request_id: value.request().request_id().to_owned(),
            slot: match value.slot() {
                InstallEventSlot::Started => StoredSlot::Started,
                InstallEventSlot::VerificationOutcome => StoredSlot::VerificationOutcome,
                InstallEventSlot::CompletionOutcome => StoredSlot::CompletionOutcome,
            },
        }
    }
}

impl StoredEventIdentity {
    fn restore(&self) -> Result<InstallEventIdentity, AuditFailure> {
        let request = InstallRequest::new(self.account_id.clone(), self.request_id.clone())
            .map_err(record_failure)?;
        let slot = match self.slot {
            StoredSlot::Started => InstallEventSlot::Started,
            StoredSlot::VerificationOutcome => InstallEventSlot::VerificationOutcome,
            StoredSlot::CompletionOutcome => InstallEventSlot::CompletionOutcome,
        };
        Ok(InstallEventIdentity::new(request, slot))
    }
}

impl From<&InstallTransition> for StoredTransition {
    fn from(value: &InstallTransition) -> Self {
        Self {
            agent: value.agent().as_str().to_owned(),
            target: StoredArtifact::from(value.target()),
            request: StoredRequest {
                account_id: value.request().account_id().to_owned(),
                request_id: value.request().request_id().to_owned(),
            },
            facts: StoredFacts::from(value.facts()),
        }
    }
}

impl StoredTransition {
    pub(in crate::agent_install::infrastructure) fn restore(
        &self,
    ) -> Result<InstallTransition, AuditFailure> {
        InstallTransition::restore(
            AgentName::parse(&self.agent).map_err(record_failure)?,
            self.target.restore()?,
            InstallRequest::new(
                self.request.account_id.clone(),
                self.request.request_id.clone(),
            )
            .map_err(record_failure)?,
            self.facts.restore()?,
        )
        .map_err(record_failure)
    }
}

impl From<&RuntimeArtifact> for StoredArtifact {
    fn from(value: &RuntimeArtifact) -> Self {
        Self {
            version: value.version().as_str().to_owned(),
            digest: value.digest().as_str().to_owned(),
            files: value
                .contents()
                .files()
                .iter()
                .map(|file| StoredFile {
                    path: file.path().as_str().to_owned(),
                    role: file.role().as_str().to_owned(),
                })
                .collect(),
        }
    }
}

impl StoredArtifact {
    fn restore(&self) -> Result<RuntimeArtifact, AuditFailure> {
        let files = self
            .files
            .iter()
            .map(|file| {
                Ok(ReleaseFile::new(
                    ArchivePath::parse(&file.path).map_err(record_failure)?,
                    FileRole::parse(&file.role).map_err(record_failure)?,
                ))
            })
            .collect::<Result<Vec<_>, AuditFailure>>()?;
        Ok(RuntimeArtifact::new(
            ReleaseVersion::parse(&self.version).map_err(record_failure)?,
            ArchiveDigest::parse(&self.digest).map_err(record_failure)?,
            ReleaseContents::new(files).map_err(record_failure)?,
        ))
    }
}

impl From<&InstallTransitionFacts> for StoredFacts {
    fn from(value: &InstallTransitionFacts) -> Self {
        match value {
            InstallTransitionFacts::Started => Self::Started,
            InstallTransitionFacts::Verified => Self::Verified,
            InstallTransitionFacts::DigestRejected(actual) => Self::DigestRejected {
                actual_digest: actual.as_str().to_owned(),
            },
            InstallTransitionFacts::Installed => Self::Installed,
            InstallTransitionFacts::Replaced(previous) => Self::Replaced {
                previous: StoredArtifact::from(previous),
            },
            InstallTransitionFacts::RolledBack(rollback) => Self::RolledBack {
                rollback: StoredRollback::from(rollback),
            },
            InstallTransitionFacts::RecoveryIncomplete { state, failures } => {
                Self::RecoveryIncomplete {
                    state: StoredRecoveryState::from(state),
                    failures: StoredRecoveryFailures::from(failures),
                }
            }
        }
    }
}

impl StoredFacts {
    fn restore(&self) -> Result<InstallTransitionFacts, AuditFailure> {
        Ok(match self {
            Self::Started => InstallTransitionFacts::Started,
            Self::Verified => InstallTransitionFacts::Verified,
            Self::DigestRejected { actual_digest } => InstallTransitionFacts::DigestRejected(
                ArchiveDigest::parse(actual_digest).map_err(record_failure)?,
            ),
            Self::Installed => InstallTransitionFacts::Installed,
            Self::Replaced { previous } => InstallTransitionFacts::Replaced(previous.restore()?),
            Self::RolledBack { rollback } => {
                InstallTransitionFacts::RolledBack(rollback.restore()?)
            }
            Self::RecoveryIncomplete { state, failures } => {
                InstallTransitionFacts::RecoveryIncomplete {
                    state: state.restore()?,
                    failures: failures.restore()?,
                }
            }
        })
    }
}

impl From<&RollbackState> for StoredRollback {
    fn from(value: &RollbackState) -> Self {
        match value {
            RollbackState::Restored(artifact) => Self::Restored(StoredArtifact::from(artifact)),
            RollbackState::NoInstalledRuntime => Self::NoInstalledRuntime,
        }
    }
}

impl StoredRollback {
    fn restore(&self) -> Result<RollbackState, AuditFailure> {
        Ok(match self {
            Self::Restored(artifact) => RollbackState::Restored(artifact.restore()?),
            Self::NoInstalledRuntime => RollbackState::NoInstalledRuntime,
        })
    }
}

impl From<&RecoveryState> for StoredRecoveryState {
    fn from(value: &RecoveryState) -> Self {
        match value {
            RecoveryState::Confirmed(rollback) => Self::Confirmed(StoredRollback::from(rollback)),
            RecoveryState::Unconfirmed => Self::Unconfirmed,
        }
    }
}

impl StoredRecoveryState {
    fn restore(&self) -> Result<RecoveryState, AuditFailure> {
        Ok(match self {
            Self::Confirmed(rollback) => RecoveryState::Confirmed(rollback.restore()?),
            Self::Unconfirmed => RecoveryState::Unconfirmed,
        })
    }
}

impl From<&RecoveryFailureEvidence> for StoredRecoveryFailures {
    fn from(value: &RecoveryFailureEvidence) -> Self {
        Self {
            publication: StoredFailure::from(value.publication()),
            withdrawal: value.withdrawal().map(StoredFailure::from),
            restoration: value.restoration().map(StoredFailure::from),
            confirmation: value.confirmation().map(StoredFailure::from),
        }
    }
}

impl StoredRecoveryFailures {
    fn restore(&self) -> Result<RecoveryFailureEvidence, AuditFailure> {
        RecoveryFailureEvidence::new(
            self.publication.restore()?,
            self.withdrawal
                .as_ref()
                .map(StoredFailure::restore)
                .transpose()?,
            self.restoration
                .as_ref()
                .map(StoredFailure::restore)
                .transpose()?,
            self.confirmation
                .as_ref()
                .map(StoredFailure::restore)
                .transpose()?,
        )
        .map_err(record_failure)
    }
}

impl From<&InstallFailureEvidence> for StoredFailure {
    fn from(value: &InstallFailureEvidence) -> Self {
        Self {
            kind: match value.kind() {
                InstallFailureKind::Unwritable => StoredFailureKind::Unwritable,
                InstallFailureKind::Unreadable => StoredFailureKind::Unreadable,
                InstallFailureKind::IncompleteArchive => StoredFailureKind::IncompleteArchive,
                InstallFailureKind::MalformedArchive => StoredFailureKind::MalformedArchive,
            },
            detail: value.detail().to_owned(),
            truncated: value.truncated(),
        }
    }
}

impl StoredFailure {
    fn restore(&self) -> Result<InstallFailureEvidence, AuditFailure> {
        InstallFailureEvidence::restore(
            match self.kind {
                StoredFailureKind::Unwritable => InstallFailureKind::Unwritable,
                StoredFailureKind::Unreadable => InstallFailureKind::Unreadable,
                StoredFailureKind::IncompleteArchive => InstallFailureKind::IncompleteArchive,
                StoredFailureKind::MalformedArchive => InstallFailureKind::MalformedArchive,
            },
            self.detail.clone(),
            self.truncated,
        )
        .map_err(record_failure)
    }
}

fn record_failure(error: impl std::fmt::Display) -> AuditFailure {
    AuditFailure::new(
        AuditFailureStage::ReadJournal,
        error.to_string(),
        None,
        None,
    )
}
