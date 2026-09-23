//! Durable install evidence: one private, monotonically sequenced file per
//! transition. One retained directory authority and a stable named lock bind
//! scanning, publication, and acknowledgement to the same journal object.
use std::{
    ffi::OsStr,
    fs::File,
    io::{Read, Write},
    path::Path,
    sync::Arc,
};

use nessa_auth::application::ports::Clock;
use nessa_local_storage::{
    create_private_directory_tree_beneath, OpenMode, PrivateDirectory, PrivateDirectoryTempFile,
    PrivateFileType, PrivatePublicationFailure, PrivatePublicationStage, PublishedPrivateFile,
};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::agent_install::{
    application::{AuditFailure, AuditFailureStage, InstallAudit, PublishedAuditRecord},
    domain::{
        InstallFailureEvidence, InstallFailureKind, InstallTransition, InstallTransitionKind,
        RecoveryState, RollbackState, RuntimeArtifact,
    },
};

const LOCK_NAME: &str = "audit.lock";

/// Private host audit storage for agent-runtime installation transitions.
pub struct DurableInstallAudit {
    directory: PrivateDirectory,
    original_lock: File,
    clock: Arc<dyn Clock>,
}

impl DurableInstallAudit {
    pub fn new(root: &Path, directory: &Path, clock: Arc<dyn Clock>) -> Result<Self, AuditFailure> {
        create_private_directory_tree_beneath(root, directory)
            .map_err(|error| audit_failure(AuditFailureStage::Initialize, error))?;
        let directory = PrivateDirectory::open_beneath(root, directory)
            .map_err(|error| audit_failure(AuditFailureStage::Initialize, error))?;
        let original_lock = directory
            .open_file(OsStr::new(LOCK_NAME), OpenMode::OpenOrCreate)
            .map_err(|error| audit_failure(AuditFailureStage::Initialize, error))?;
        original_lock
            .sync_all()
            .map_err(|error| audit_failure(AuditFailureStage::Initialize, error))?;
        ensure_named_file(
            &directory,
            OsStr::new(LOCK_NAME),
            &original_lock,
            AuditFailureStage::Initialize,
        )?;
        directory
            .sync()
            .map_err(|error| audit_failure(AuditFailureStage::Initialize, error))?;
        ensure_named_file(
            &directory,
            OsStr::new(LOCK_NAME),
            &original_lock,
            AuditFailureStage::Initialize,
        )?;
        Ok(Self {
            directory,
            original_lock,
            clock,
        })
    }
}

impl InstallAudit for DurableInstallAudit {
    fn record(&self, transition: InstallTransition) -> Result<(), AuditFailure> {
        let current_lock = self
            .directory
            .open_file(OsStr::new(LOCK_NAME), OpenMode::ReadWrite)
            .map_err(|error| audit_failure(AuditFailureStage::AcquireLock, error))?;
        current_lock
            .lock()
            .map_err(|error| audit_failure(AuditFailureStage::AcquireLock, error))?;
        self.verify_authority(&current_lock)?;

        let sequence = next_sequence(&self.directory)?;
        self.verify_authority(&current_lock)?;

        let record_id = Uuid::new_v4().to_string();
        let destination = format!("{sequence:020}.json");
        let logical_record =
            PublishedAuditRecord::new(record_id.clone(), sequence, destination.clone());
        let mut value = record_value(&transition);
        value["recordId"] = json!(record_id);
        value["sequence"] = json!(sequence);
        value["observedAtMs"] = json!(self.clock.unix_milliseconds());
        let mut encoded = serde_json::to_vec(&value)
            .map_err(|error| audit_failure(AuditFailureStage::WriteRecord, error))?;
        encoded.push(b'\n');

        let mut reservation = self
            .directory
            .reserve_temp()
            .map_err(|error| audit_failure(AuditFailureStage::WriteRecord, error))?;
        if let Err(error) = reservation.as_file_mut().write_all(&encoded) {
            return Err(discard_failure(
                reservation,
                AuditFailureStage::WriteRecord,
                error,
            ));
        }
        if let Err(error) = self.verify_authority(&current_lock) {
            return Err(discard_failure(
                reservation,
                AuditFailureStage::VerifyAuthority,
                error,
            ));
        }

        let published = match reservation.publish_new(OsStr::new(&destination)) {
            Ok(published) => published,
            Err(failure) => return Err(map_publication_failure(failure, logical_record)),
        };
        if let Err(error) = self.verify_authority(&current_lock) {
            return Err(published_failure(logical_record, error));
        }
        acknowledge_published(&self.directory, &published, &logical_record)?;
        self.verify_authority(&current_lock)
            .map_err(|error| published_failure(logical_record, error))
    }
}

impl DurableInstallAudit {
    fn verify_authority(&self, current_lock: &File) -> Result<(), AuditFailure> {
        self.directory
            .verify_binding()
            .map_err(|error| audit_failure(AuditFailureStage::VerifyAuthority, error))?;
        ensure_named_file(
            &self.directory,
            OsStr::new(LOCK_NAME),
            &self.original_lock,
            AuditFailureStage::VerifyAuthority,
        )?;
        ensure_named_file(
            &self.directory,
            OsStr::new(LOCK_NAME),
            current_lock,
            AuditFailureStage::VerifyAuthority,
        )
    }
}

fn ensure_named_file(
    directory: &PrivateDirectory,
    name: &OsStr,
    file: &File,
    stage: AuditFailureStage,
) -> Result<(), AuditFailure> {
    match directory.named_file_is(name, file) {
        Ok(true) => Ok(()),
        Ok(false) => Err(AuditFailure::new(
            stage,
            "the audit authority no longer names the opened file".into(),
            None,
            None,
        )),
        Err(error) => Err(audit_failure(stage, error)),
    }
}

fn acknowledge_published(
    directory: &PrivateDirectory,
    published: &PublishedPrivateFile,
    logical_record: &PublishedAuditRecord,
) -> Result<(), AuditFailure> {
    match directory.named_file_is(published.name(), published.as_file()) {
        Ok(true) => {}
        Ok(false) => {
            return Err(AuditFailure::new(
                AuditFailureStage::AcknowledgeRecord,
                "the published audit destination no longer names the published file".into(),
                Some(logical_record.clone()),
                None,
            ));
        }
        Err(error) => return Err(published_failure(logical_record.clone(), error)),
    }
    directory
        .verify_binding()
        .map_err(|error| published_failure(logical_record.clone(), error))
}

fn next_sequence(directory: &PrivateDirectory) -> Result<u64, AuditFailure> {
    let mut sequences = Vec::new();
    let entries = directory
        .entries()
        .map_err(|error| audit_failure(AuditFailureStage::ReadJournal, error))?;
    for entry in entries {
        let entry = entry.map_err(|error| audit_failure(AuditFailureStage::ReadJournal, error))?;
        if entry.name() == OsStr::new(LOCK_NAME) {
            if entry.file_type() != PrivateFileType::RegularFile {
                return Err(journal_failure("the audit lock is not a regular file"));
            }
            continue;
        }
        if entry.file_type() != PrivateFileType::RegularFile {
            return Err(journal_failure(format!(
                "unexpected non-regular audit entry {:?}",
                entry.name()
            )));
        }
        let name = entry
            .name()
            .to_str()
            .ok_or_else(|| journal_failure("audit record name is not UTF-8"))?;
        let sequence = canonical_sequence(name)?;
        let mut record = directory
            .open_file(entry.name(), OpenMode::Read)
            .map_err(|error| audit_failure(AuditFailureStage::ReadJournal, error))?;
        ensure_named_file(
            directory,
            entry.name(),
            &record,
            AuditFailureStage::ReadJournal,
        )?;
        let mut encoded = Vec::new();
        record
            .read_to_end(&mut encoded)
            .map_err(|error| audit_failure(AuditFailureStage::ReadJournal, error))?;
        let value: Value = serde_json::from_slice(&encoded)
            .map_err(|error| journal_failure(format!("invalid audit record {name}: {error}")))?;
        if value["sequence"].as_u64() != Some(sequence) {
            return Err(journal_failure(format!(
                "audit record {name} disagrees with its sequence"
            )));
        }
        sequences.push(sequence);
    }
    sequences.sort_unstable();
    for (index, sequence) in sequences.iter().enumerate() {
        let expected = u64::try_from(index).unwrap_or(u64::MAX) + 1;
        if *sequence != expected {
            return Err(journal_failure(format!(
                "audit sequence is not contiguous at {expected}"
            )));
        }
    }
    u64::try_from(sequences.len())
        .ok()
        .and_then(|last| last.checked_add(1))
        .ok_or_else(|| journal_failure("audit sequence exhausted"))
}

fn canonical_sequence(name: &str) -> Result<u64, AuditFailure> {
    let Some(number) = name.strip_suffix(".json") else {
        return Err(journal_failure(format!("unexpected audit entry {name}")));
    };
    if number.len() != 20 || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(journal_failure(format!("invalid audit record name {name}")));
    }
    let sequence = number
        .parse::<u64>()
        .map_err(|_| journal_failure(format!("invalid audit record name {name}")))?;
    if format!("{sequence:020}.json") != name {
        return Err(journal_failure(format!(
            "non-canonical audit record name {name}"
        )));
    }
    Ok(sequence)
}

fn discard_failure(
    reservation: PrivateDirectoryTempFile<'_>,
    stage: AuditFailureStage,
    error: impl std::fmt::Display,
) -> AuditFailure {
    let cleanup = reservation.discard().err().map(|error| error.to_string());
    AuditFailure::new(stage, error.to_string(), None, cleanup)
}

fn map_publication_failure(
    failure: PrivatePublicationFailure,
    logical_record: PublishedAuditRecord,
) -> AuditFailure {
    let (stage, source, published, cleanup) = failure.into_parts();
    publication_failure(
        stage,
        source.to_string(),
        logical_record,
        published.is_some(),
        cleanup.map(|error| error.to_string()),
    )
}

fn publication_failure(
    storage_stage: PrivatePublicationStage,
    detail: String,
    logical_record: PublishedAuditRecord,
    published: bool,
    cleanup: Option<String>,
) -> AuditFailure {
    let stage = if published {
        AuditFailureStage::AcknowledgeRecord
    } else {
        match storage_stage {
            PrivatePublicationStage::FlushAfterRename
            | PrivatePublicationStage::ValidatePublishedDestination
            | PrivatePublicationStage::VerifyPublishedBinding
            | PrivatePublicationStage::SyncDirectory => AuditFailureStage::AcknowledgeRecord,
            PrivatePublicationStage::ValidateDestination
            | PrivatePublicationStage::VerifyOriginBinding
            | PrivatePublicationStage::ValidateReservation
            | PrivatePublicationStage::FlushBeforeRename
            | PrivatePublicationStage::Rename => AuditFailureStage::PublishRecord,
        }
    };
    AuditFailure::new(
        stage,
        format!("storage publication failed at {storage_stage:?}: {detail}"),
        published.then_some(logical_record),
        cleanup,
    )
}

fn published_failure(
    logical_record: PublishedAuditRecord,
    error: impl std::fmt::Display,
) -> AuditFailure {
    AuditFailure::new(
        AuditFailureStage::AcknowledgeRecord,
        error.to_string(),
        Some(logical_record),
        None,
    )
}

fn audit_failure(stage: AuditFailureStage, error: impl std::fmt::Display) -> AuditFailure {
    AuditFailure::new(stage, error.to_string(), None, None)
}

fn journal_failure(error: impl std::fmt::Display) -> AuditFailure {
    audit_failure(AuditFailureStage::ReadJournal, error)
}

fn artifact(value: &RuntimeArtifact) -> Value {
    json!({
        "version": value.version().as_str(),
        "digest": value.digest().as_str(),
        "executable": value.executable().as_str(),
    })
}

fn installed(value: &RuntimeArtifact) -> Value {
    json!({"state": "installed", "artifact": artifact(value)})
}

fn failure(value: &InstallFailureEvidence) -> Value {
    let kind = match value.kind() {
        InstallFailureKind::Unwritable => "unwritable",
        InstallFailureKind::Unreadable => "unreadable",
        InstallFailureKind::MissingExecutable => "missing_executable",
        InstallFailureKind::MalformedArchive => "malformed_archive",
    };
    json!({"kind": kind, "detail": value.detail()})
}

fn caller(transition: &InstallTransition) -> Value {
    json!({
        "kind": "local_account",
        "accountId": transition.request().account_id(),
    })
}

fn common(
    transition: &InstallTransition,
    kind: &str,
    before: Value,
    after: Value,
    cause: &str,
) -> Value {
    json!({
        "kind": kind,
        "target": {
            "agent": transition.agent().as_str(),
            "artifact": artifact(transition.target()),
        },
        "transition": {"before": before, "after": after},
        "cause": cause,
        "initiator": caller(transition),
        "correlationId": transition.request().request_id(),
    })
}

fn record_value(transition: &InstallTransition) -> Value {
    match transition.kind() {
        InstallTransitionKind::Started => common(
            transition,
            "agent_runtime_install_started",
            json!({"state": "not_started"}),
            json!({"state": "install_started"}),
            "install_requested",
        ),
        InstallTransitionKind::Verified => common(
            transition,
            "agent_runtime_archive_verified",
            json!({"state": "install_started"}),
            json!({"state": "archive_verified"}),
            "pinned_digest_matched",
        ),
        InstallTransitionKind::DigestRejected => {
            let mut value = common(
                transition,
                "agent_runtime_archive_rejected",
                json!({"state": "install_started"}),
                json!({"state": "archive_rejected"}),
                "pinned_digest_mismatched",
            );
            value["actualDigest"] = json!(transition
                .actual_digest()
                .expect("rejection carries actual digest")
                .as_str());
            value
        }
        InstallTransitionKind::Installed => common(
            transition,
            "agent_runtime_installed",
            json!({"state": "archive_verified"}),
            installed(transition.target()),
            "install_requested",
        ),
        InstallTransitionKind::Replaced => common(
            transition,
            "agent_runtime_replaced",
            installed(
                transition
                    .previous()
                    .expect("replacement carries previous artifact"),
            ),
            installed(transition.target()),
            "install_requested",
        ),
        InstallTransitionKind::RolledBack => {
            let after = match transition
                .rollback()
                .expect("rollback carries restored state")
            {
                RollbackState::Restored(artifact) => installed(artifact),
                RollbackState::NoInstalledRuntime => json!({"state": "not_installed"}),
            };
            common(
                transition,
                "agent_runtime_install_rolled_back",
                json!({"state": "publication_pending", "artifact": artifact(transition.target())}),
                after,
                "publication_failed",
            )
        }
        InstallTransitionKind::RecoveryIncomplete => {
            let (state, failures) = transition
                .recovery()
                .expect("incomplete recovery carries state and failure evidence");
            let after = match state {
                RecoveryState::Confirmed(RollbackState::Restored(artifact)) => installed(artifact),
                RecoveryState::Confirmed(RollbackState::NoInstalledRuntime) => {
                    json!({"state": "not_installed"})
                }
                RecoveryState::Unconfirmed => json!({"state": "unconfirmed"}),
            };
            let mut value = common(
                transition,
                "agent_runtime_install_recovery_incomplete",
                json!({"state": "publication_pending", "artifact": artifact(transition.target())}),
                after,
                "publication_cleanup_failed",
            );
            value["failures"] = json!({
                "publication": failure(failures.publication()),
                "withdrawal": failures.withdrawal().map(failure),
                "restoration": failures.restoration().map(failure),
                "confirmation": failures.confirmation().map(failure),
            });
            value
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/agent_install/audit.rs"]
mod tests;
