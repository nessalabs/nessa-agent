//! Durable install evidence: one private, monotonically sequenced file per
//! transition. One retained directory authority and a stable named lock bind
//! scanning, publication, replay acknowledgement, and append to one journal.
use std::{
    ffi::{OsStr, OsString},
    fs::File,
    io::{Read, Seek, Write},
    path::Path,
    sync::Arc,
};

use nessa_auth::application::ports::Clock;
use nessa_local_storage::{
    OpenMode, PrivateDirectory, PrivateDirectoryTempFile, PrivateFileType,
    PrivatePublicationFailure, PrivatePublicationStage, PublishedPrivateFile,
    create_private_directory_tree_beneath,
};
use uuid::Uuid;

use super::record::StoredRecord;

use crate::agent_install::{
    application::{
        AuditAcknowledgement, AuditFailure, AuditFailureStage, AuditRecordEvidence, InstallAudit,
        PublishedAuditRecord,
    },
    domain::{InstallAttempt, InstallAttemptError, InstallEventAdmission, InstallTransition},
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

    fn record_with_acknowledger(
        &self,
        transition: InstallTransition,
        acknowledge: impl FnOnce(
            &PrivateDirectory,
            &PublishedPrivateFile,
            &PublishedAuditRecord,
        ) -> Result<(), AuditFailure>,
    ) -> Result<AuditAcknowledgement, AuditFailure> {
        let current_lock = self
            .directory
            .open_file(OsStr::new(LOCK_NAME), OpenMode::ReadWrite)
            .map_err(|error| audit_failure(AuditFailureStage::AcquireLock, error))?;
        current_lock
            .lock()
            .map_err(|error| audit_failure(AuditFailureStage::AcquireLock, error))?;
        self.verify_authority(&current_lock)?;

        let mut journal = scan_journal(&self.directory)?;
        self.verify_authority(&current_lock)?;
        if let Some(existing) = journal
            .records
            .iter()
            .find(|record| record.transition.event_identity() == transition.event_identity())
        {
            if existing.transition != transition {
                let conflict = if existing.transition.agent() != transition.agent()
                    || existing.transition.target() != transition.target()
                    || existing.transition.request() != transition.request()
                {
                    InstallAttemptError::ConflictingAttempt
                } else {
                    InstallAttemptError::ConflictingEvent
                };
                return Err(AuditFailure::semantic_conflict(
                    AuditFailureStage::ReconcileRecord,
                    "the logical install event already has different durable facts".into(),
                    Some(AuditRecordEvidence::ExistingConflict(
                        existing.published.clone(),
                    )),
                    conflict,
                ));
            }
            reacknowledge_existing(&self.directory, existing, &current_lock, |lock| {
                self.verify_authority(lock)
            })?;
            return Ok(AuditAcknowledgement::Replayed);
        }

        admit_transition(
            &mut journal.attempts,
            transition.clone(),
            AuditFailureStage::ReconcileRecord,
        )?;
        let sequence = journal.next_sequence;
        let record_id = Uuid::new_v4().to_string();
        let destination = format!("{sequence:020}.json");
        let logical_record = PublishedAuditRecord::new(
            transition.event_identity(),
            record_id.clone(),
            sequence,
            destination.clone(),
        );
        let stored = StoredRecord::from_transition(
            record_id,
            sequence,
            self.clock.unix_milliseconds(),
            &transition,
        );
        let mut encoded = serde_json::to_vec(&stored)
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
        acknowledge(&self.directory, &published, &logical_record)?;
        self.verify_authority(&current_lock)
            .map_err(|error| published_failure(logical_record, error))?;
        Ok(AuditAcknowledgement::Recorded)
    }
}

impl InstallAudit for DurableInstallAudit {
    fn record(&self, transition: InstallTransition) -> Result<AuditAcknowledgement, AuditFailure> {
        self.record_with_acknowledger(transition, acknowledge_published)
    }
}

#[derive(Debug)]
struct ScannedRecord {
    name: OsString,
    stored: StoredRecord,
    transition: InstallTransition,
    published: PublishedAuditRecord,
}

struct Journal {
    next_sequence: u64,
    records: Vec<ScannedRecord>,
    attempts: Vec<InstallAttempt>,
}

fn scan_journal(directory: &PrivateDirectory) -> Result<Journal, AuditFailure> {
    let entries = directory
        .entries()
        .map_err(|error| audit_failure(AuditFailureStage::ReadJournal, error))?;
    let mut names = Vec::new();
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
        names.push((canonical_sequence(name)?, entry.name().to_owned()));
    }
    names.sort_by_key(|(sequence, _)| *sequence);
    let mut records = Vec::with_capacity(names.len());
    let mut attempts = Vec::new();
    for (index, (sequence, name)) in names.into_iter().enumerate() {
        let expected = u64::try_from(index).unwrap_or(u64::MAX) + 1;
        if sequence != expected {
            return Err(journal_failure(format!(
                "audit sequence is not contiguous at {expected}"
            )));
        }
        let mut file = directory
            .open_file(&name, OpenMode::Read)
            .map_err(|error| audit_failure(AuditFailureStage::ReadJournal, error))?;
        ensure_named_file(directory, &name, &file, AuditFailureStage::ReadJournal)?;
        let stored = read_stored(&mut file, &name, sequence)?;
        ensure_named_file(directory, &name, &file, AuditFailureStage::ReadJournal)?;
        let transition = stored.restore_transition()?;
        admit_transition(
            &mut attempts,
            transition.clone(),
            AuditFailureStage::ReadJournal,
        )?;
        let published = stored.published(&transition, &name)?;
        records.push(ScannedRecord {
            name,
            stored,
            transition,
            published,
        });
    }
    let next_sequence = u64::try_from(records.len())
        .ok()
        .and_then(|last| last.checked_add(1))
        .ok_or_else(|| journal_failure("audit sequence exhausted"))?;
    Ok(Journal {
        next_sequence,
        records,
        attempts,
    })
}

fn admit_transition(
    attempts: &mut Vec<InstallAttempt>,
    transition: InstallTransition,
    stage: AuditFailureStage,
) -> Result<(), AuditFailure> {
    let admission = match attempts
        .iter_mut()
        .find(|attempt| attempt.request() == transition.request())
    {
        Some(attempt) => attempt.admit(transition),
        None => {
            attempts.push(
                InstallAttempt::from_started(transition)
                    .map_err(|error| semantic_failure(stage, error, None))?,
            );
            return Ok(());
        }
    }
    .map_err(|error| semantic_failure(stage, error, None))?;
    if admission == InstallEventAdmission::Replay {
        return Err(journal_failure(
            "the journal contains the same logical install event twice",
        ));
    }
    Ok(())
}

fn reacknowledge_existing(
    directory: &PrivateDirectory,
    existing: &ScannedRecord,
    current_lock: &File,
    verify_authority: impl Fn(&File) -> Result<(), AuditFailure>,
) -> Result<(), AuditFailure> {
    let mut file = directory
        .open_file(&existing.name, OpenMode::ReadWrite)
        .map_err(|error| existing_failure(existing, error))?;
    ensure_named_file(
        directory,
        &existing.name,
        &file,
        AuditFailureStage::AcknowledgeRecord,
    )
    .map_err(|error| existing_failure(existing, error))?;
    let stored = read_stored(&mut file, &existing.name, existing.published.sequence())?;
    if stored != existing.stored || stored.restore_transition()? != existing.transition {
        return Err(existing_failure(
            existing,
            "the audit record changed while it was being re-acknowledged",
        ));
    }
    file.sync_all()
        .map_err(|error| existing_failure(existing, error))?;
    directory
        .sync()
        .map_err(|error| existing_failure(existing, error))?;
    verify_authority(current_lock).map_err(|error| existing_failure(existing, error))?;
    ensure_named_file(
        directory,
        &existing.name,
        &file,
        AuditFailureStage::AcknowledgeRecord,
    )
    .map_err(|error| existing_failure(existing, error))
}

fn read_stored(file: &mut File, name: &OsStr, sequence: u64) -> Result<StoredRecord, AuditFailure> {
    file.rewind()
        .map_err(|error| audit_failure(AuditFailureStage::ReadJournal, error))?;
    let mut encoded = Vec::new();
    file.read_to_end(&mut encoded)
        .map_err(|error| audit_failure(AuditFailureStage::ReadJournal, error))?;
    let stored: StoredRecord = serde_json::from_slice(&encoded)
        .map_err(|error| journal_failure(format!("invalid audit record {:?}: {error}", name)))?;
    if stored.sequence != sequence {
        return Err(journal_failure(format!(
            "audit record {:?} disagrees with its sequence",
            name
        )));
    }
    if Uuid::parse_str(&stored.record_id).is_err() {
        return Err(journal_failure(format!(
            "audit record {:?} has an invalid record id",
            name
        )));
    }
    Ok(stored)
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
            return Err(published_failure(
                logical_record.clone(),
                "the published audit destination no longer names the published file",
            ));
        }
        Err(error) => return Err(published_failure(logical_record.clone(), error)),
    }
    published
        .as_file()
        .sync_all()
        .map_err(|error| published_failure(logical_record.clone(), error))?;
    directory
        .sync()
        .map_err(|error| published_failure(logical_record.clone(), error))?;
    directory
        .verify_binding()
        .map_err(|error| published_failure(logical_record.clone(), error))?;
    match directory.named_file_is(published.name(), published.as_file()) {
        Ok(true) => Ok(()),
        Ok(false) => Err(published_failure(
            logical_record.clone(),
            "the published audit destination changed during acknowledgement",
        )),
        Err(error) => Err(published_failure(logical_record.clone(), error)),
    }
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
        published.then_some(AuditRecordEvidence::IncomingPublished(logical_record)),
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
        Some(AuditRecordEvidence::IncomingPublished(logical_record)),
        None,
    )
}

fn existing_failure(existing: &ScannedRecord, error: impl std::fmt::Display) -> AuditFailure {
    AuditFailure::new(
        AuditFailureStage::AcknowledgeRecord,
        error.to_string(),
        Some(AuditRecordEvidence::IncomingPublished(
            existing.published.clone(),
        )),
        None,
    )
}

fn audit_failure(stage: AuditFailureStage, error: impl std::fmt::Display) -> AuditFailure {
    AuditFailure::new(stage, error.to_string(), None, None)
}

fn journal_failure(error: impl std::fmt::Display) -> AuditFailure {
    audit_failure(AuditFailureStage::ReadJournal, error)
}

fn semantic_failure(
    stage: AuditFailureStage,
    error: InstallAttemptError,
    record: Option<AuditRecordEvidence>,
) -> AuditFailure {
    AuditFailure::semantic_conflict(stage, error.to_string(), record, error)
}

#[cfg(test)]
#[path = "../../../../tests/agent_install/audit.rs"]
mod tests;
