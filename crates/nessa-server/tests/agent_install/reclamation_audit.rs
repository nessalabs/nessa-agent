#[cfg(unix)]
use std::os::unix::fs::symlink;

use super::*;
use crate::agent_install::domain::{
    AgentName, ArchiveDigest, ArchivePath, FileRole, InstallRequest, ReclamationActivation,
    ReclamationAdmission, ReclamationAuditState, ReclamationObligation, ReclamationOperationId,
    ReclamationPhysicalOutcome, ReclamationTrigger, ReleaseContents, ReleaseFile, ReleaseVersion,
    RuntimeArtifact,
};

fn artifact(version: &str, digest: char) -> RuntimeArtifact {
    RuntimeArtifact::new(
        ReleaseVersion::parse(version).unwrap(),
        ArchiveDigest::parse(&digest.to_string().repeat(64)).unwrap(),
        ReleaseContents::new(vec![ReleaseFile::new(
            ArchivePath::parse("bin/opencode").unwrap(),
            FileRole::Launch,
        )])
        .unwrap(),
    )
}

fn event(outcome: ReclamationPhysicalOutcome) -> ReclamationEvent {
    let previous = artifact("1.0.0", 'a');
    let current = artifact("2.0.0", 'b');
    let request = InstallRequest::new("account-a", "replace-a-with-b").unwrap();
    let obligation = ReclamationObligation::restore(
        previous,
        ReclamationActivation::new(current.clone(), request),
        ReclamationActivation::new(
            current.clone(),
            InstallRequest::new("account-a", "replace-a-with-b").unwrap(),
        ),
    )
    .unwrap();
    ReclamationEvent::restore(
        ReclamationAdmission::restore(
            AgentName::parse("opencode").unwrap(),
            ReclamationOperationId::new("remove-a-1").unwrap(),
            obligation,
            current,
            ReclamationTrigger::ProcessRecovery,
        )
        .unwrap(),
        outcome,
        ReclamationAuditState::Pending,
    )
}

fn audit_root() -> (tempfile::TempDir, std::path::PathBuf) {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("reclamation-audit");
    (temporary, root)
}

fn record_path(event: &ReclamationEvent) -> std::path::PathBuf {
    std::path::Path::new(RECORDS).join(DurableReclamationAudit::operation_name(
        event.admission().operation_id(),
    ))
}

#[test]
fn replay_accepts_only_the_exact_durable_reclamation_event() {
    let (_temporary, root) = audit_root();
    let audit = DurableReclamationAudit::new(&root).unwrap();
    let removed = event(ReclamationPhysicalOutcome::Removed);

    audit.record(&removed).unwrap();
    let path = root.join(record_path(&removed));
    let original = std::fs::read(&path).unwrap();
    audit.record(&removed).unwrap();

    assert_eq!(std::fs::read(&path).unwrap(), original);
}

#[test]
fn replay_requires_file_sync_and_post_publish_directory_sync_is_recoverable() {
    let (_temporary, root) = audit_root();
    let audit = DurableReclamationAudit::new(&root).unwrap();
    let removed = event(ReclamationPhysicalOutcome::Removed);
    audit.record(&removed).unwrap();
    let path = root.join(record_path(&removed));
    let original = std::fs::read(&path).unwrap();

    let replay = audit
        .record_with(
            &removed,
            |_| Err(std::io::Error::other("injected replay file sync failure")),
            || Ok(()),
        )
        .unwrap_err();
    assert_eq!(replay.detail(), "injected replay file sync failure");
    assert_eq!(std::fs::read(&path).unwrap(), original);

    let (_temporary, root) = audit_root();
    let audit = DurableReclamationAudit::new(&root).unwrap();
    let publish = audit
        .record_with(&removed, std::fs::File::sync_all, || {
            Err(std::io::Error::other(
                "injected record directory sync failure",
            ))
        })
        .unwrap_err();
    assert_eq!(publish.detail(), "injected record directory sync failure");
    let path = root.join(record_path(&removed));
    let published = std::fs::read(&path).unwrap();
    audit
        .record(&removed)
        .expect("exact replay re-syncs the immutable published record");
    assert_eq!(std::fs::read(path).unwrap(), published);
}

#[test]
fn conflicting_facts_for_one_operation_are_refused_without_replacing_the_original() {
    let (_temporary, root) = audit_root();
    let audit = DurableReclamationAudit::new(&root).unwrap();
    let removed = event(ReclamationPhysicalOutcome::Removed);
    let still_present = event(ReclamationPhysicalOutcome::StillPresent);
    audit.record(&removed).unwrap();
    let path = root.join(record_path(&removed));
    let original = std::fs::read(&path).unwrap();

    let failure = audit.record(&still_present).unwrap_err();

    assert_eq!(
        failure.detail(),
        "reclamation operation identity has conflicting audit facts"
    );
    assert_eq!(std::fs::read(&path).unwrap(), original);
}

#[test]
fn exact_lookup_distinguishes_authoritative_absence_from_a_retained_event() {
    let (_temporary, root) = audit_root();
    let audit = DurableReclamationAudit::new(&root).unwrap();
    let removed = event(ReclamationPhysicalOutcome::Removed);
    let missing = ReclamationOperationId::new("not-recorded").unwrap();

    assert_eq!(audit.event_for(&missing).unwrap(), None);
    audit.record(&removed).unwrap();
    assert_eq!(
        audit.event_for(removed.admission().operation_id()).unwrap(),
        Some(removed)
    );
}

#[test]
fn oversized_record_is_rejected_before_decode() {
    let (_temporary, root) = audit_root();
    let audit = DurableReclamationAudit::new(&root).unwrap();
    let removed = event(ReclamationPhysicalOutcome::Removed);
    audit.record(&removed).unwrap();
    let path = root.join(record_path(&removed));
    std::fs::write(path, vec![b'x'; MAXIMUM_AUDIT_RECORD_BYTES as usize + 1]).unwrap();

    let failure = audit
        .event_for(removed.admission().operation_id())
        .unwrap_err();

    assert_eq!(
        failure.detail(),
        "reclamation audit record exceeds its size bound"
    );
}

#[test]
fn audit_encoder_refuses_the_first_byte_past_its_fixed_capacity() {
    let mut encoded = BoundedAuditBuffer::new();
    encoded
        .write_all(&vec![b'x'; MAXIMUM_AUDIT_RECORD_BYTES as usize])
        .unwrap();
    let failure = encoded.write_all(b"x").unwrap_err();

    assert_eq!(
        failure.to_string(),
        "reclamation audit record exceeds its size bound"
    );
    assert_eq!(encoded.bytes.len(), MAXIMUM_AUDIT_RECORD_BYTES as usize);
}

#[test]
fn replacing_the_retained_records_directory_refuses_lookup_and_append() {
    let (_temporary, root) = audit_root();
    let audit = DurableReclamationAudit::new(&root).unwrap();
    let removed = event(ReclamationPhysicalOutcome::Removed);
    if let Err(error) = std::fs::rename(root.join(RECORDS), root.join("displaced-records")) {
        #[cfg(windows)]
        {
            assert!(
                matches!(error.raw_os_error(), Some(5) | Some(32)),
                "Windows must refuse substitution through access or sharing denial: {error}"
            );
            assert!(!root.join("displaced-records").exists());
            assert!(root.join(RECORDS).is_dir());
            audit.record(&removed).unwrap();
            assert_eq!(
                audit.event_for(removed.admission().operation_id()).unwrap(),
                Some(removed)
            );
            return;
        }
        #[cfg(not(windows))]
        panic!("records directory substitution failed unexpectedly: {error}");
    }
    create_directory(&root.join(RECORDS)).unwrap();

    let lookup = audit
        .event_for(removed.admission().operation_id())
        .unwrap_err();
    let append = audit.record(&removed).unwrap_err();

    assert_eq!(
        lookup.detail(),
        "local storage must be private and owned by the current OS user"
    );
    assert_eq!(append.detail(), lookup.detail());
    assert!(!root.join(record_path(&removed)).exists());
}

#[cfg(unix)]
#[test]
fn replacing_lock_or_records_authority_after_acquisition_refuses_acknowledgement() {
    let (_temporary, root) = audit_root();
    let audit = DurableReclamationAudit::new(&root).unwrap();
    let removed = event(ReclamationPhysicalOutcome::Removed);
    audit.record(&removed).unwrap();
    let stored_record_path = root.join(record_path(&removed));
    let original = std::fs::read(&stored_record_path).unwrap();

    let lock_path = root.join(RECORDS).join(LOCK);
    let lock_failure = audit
        .record_with(&removed, std::fs::File::sync_all, || {
            std::fs::remove_file(&lock_path)?;
            std::fs::write(&lock_path, b"replacement")
        })
        .unwrap_err();
    assert!(lock_failure.detail().contains("replaced"));
    assert_eq!(std::fs::read(&stored_record_path).unwrap(), original);

    let (_temporary, root) = audit_root();
    let audit = DurableReclamationAudit::new(&root).unwrap();
    audit.record(&removed).unwrap();
    let authority_failure = audit
        .record_with(&removed, std::fs::File::sync_all, || {
            std::fs::rename(root.join(RECORDS), root.join("displaced-records"))?;
            create_directory(&root.join(RECORDS))
        })
        .unwrap_err();
    assert_eq!(
        authority_failure.detail(),
        "local storage must be private and owned by the current OS user"
    );
    assert!(!root.join(record_path(&removed)).exists());
}

#[cfg(unix)]
#[test]
fn symlinked_records_directory_is_never_accepted_as_the_retained_authority() {
    let (_temporary, root) = audit_root();
    let audit = DurableReclamationAudit::new(&root).unwrap();
    let removed = event(ReclamationPhysicalOutcome::Removed);
    let outside = root.join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::rename(root.join(RECORDS), root.join("original-records")).unwrap();
    symlink(&outside, root.join(RECORDS)).unwrap();

    assert!(audit.record(&removed).is_err());
    assert!(std::fs::read_dir(outside).unwrap().next().is_none());
}
