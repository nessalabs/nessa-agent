use super::*;
use crate::agent_install::{
    application::{AuditAcknowledgement, AuditFailureStage, AuditRecordEvidence},
    domain::{
        ArchiveDigest, ArchivePath, FileRole, InstallAttempt, InstallAttemptError,
        InstallEventIdentity, InstallEventSlot, InstallFailureEvidence, InstallFailureKind,
        InstallRequest, RecoveryFailureEvidence, RecoveryState, ReleaseContents, ReleaseFile,
        ReleaseVersion, RollbackState, RuntimeArtifact,
    },
};
use crate::agent_install_test_support::{
    OTHER_DIGEST, PINNED_DIGEST, agent, platform, release, request, temporary_root,
};
use nessa_auth::application::ports::Clock;
use std::sync::{Arc, Barrier};
use uuid::Uuid;

struct FixedClock;

impl Clock for FixedClock {
    fn unix_milliseconds(&self) -> u64 {
        42
    }
}

fn target() -> RuntimeArtifact {
    RuntimeArtifact::for_release(&release("1.18.31", PINNED_DIGEST, &platform()))
}

fn audit_at(root: &std::path::Path) -> DurableInstallAudit {
    DurableInstallAudit::new(root, std::path::Path::new("audit"), Arc::new(FixedClock)).unwrap()
}

fn json_records(directory: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut records = std::fs::read_dir(directory)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect::<Vec<_>>();
    records.sort();
    records
}

#[test]
fn a_record_contains_stable_event_identity_full_artifact_and_physical_metadata() {
    let root = temporary_root();
    let directory = root.path().join("audit");
    let audit = audit_at(root.path());
    let (_, started) = InstallAttempt::start(agent(), target(), request());

    assert_eq!(
        audit.record(started).unwrap(),
        AuditAcknowledgement::Recorded
    );

    let records = json_records(&directory);
    assert_eq!(records.len(), 1);
    let record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&records[0]).unwrap()).unwrap();
    assert_eq!(record["observedAtMs"], 42);
    assert_eq!(record["sequence"], 1);
    assert!(Uuid::parse_str(record["recordId"].as_str().unwrap()).is_ok());
    assert_eq!(record["event"]["accountId"], "unix:501");
    assert_eq!(record["event"]["requestId"], "install-request-1");
    assert_eq!(record["event"]["slot"], "started");
    assert_eq!(
        record["transition"]["target"]["files"][0]["path"],
        "package/bin/opencode"
    );
}

#[test]
fn exact_replay_after_later_evidence_resyncs_the_original_record_without_append() {
    let root = temporary_root();
    let directory = root.path().join("audit");
    let audit = audit_at(root.path());
    let (mut attempt, started) = InstallAttempt::start(agent(), target(), request());
    audit.record(started.clone()).unwrap();
    audit.record(attempt.verified().unwrap()).unwrap();
    let original = std::fs::read(directory.join("00000000000000000001.json")).unwrap();

    assert_eq!(
        audit.record(started).unwrap(),
        AuditAcknowledgement::Replayed
    );

    assert_eq!(json_records(&directory).len(), 2);
    assert_eq!(
        std::fs::read(directory.join("00000000000000000001.json")).unwrap(),
        original
    );
}

#[test]
fn abandoned_private_reservation_is_preserved_while_append_and_replay_stay_contiguous() {
    let root = temporary_root();
    let directory_path = root.path().join("audit");
    let audit = audit_at(root.path());
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "interrupted_reservation_child"])
        .env("NESSA_AUDIT_RESERVATION_ROOT", root.path())
        .status()
        .unwrap();
    assert!(status.success());
    let reservation_name = std::fs::read_to_string(root.path().join("reservation-name")).unwrap();
    let cleanup_failure_orphan = ".nessa-ffffffffffffffffffffffffffffffff.tmp";
    std::fs::write(
        directory_path.join(cleanup_failure_orphan),
        b"reservation whose cleanup failed",
    )
    .unwrap();
    let (mut attempt, started) = InstallAttempt::start(agent(), target(), request());

    assert_eq!(
        audit.record(started.clone()).unwrap(),
        AuditAcknowledgement::Recorded
    );
    assert_eq!(
        audit.record(attempt.verified().unwrap()).unwrap(),
        AuditAcknowledgement::Recorded
    );
    assert_eq!(
        audit.record(started).unwrap(),
        AuditAcknowledgement::Replayed
    );

    assert!(directory_path.join(reservation_name).is_file());
    assert_eq!(
        std::fs::read(directory_path.join(cleanup_failure_orphan)).unwrap(),
        b"reservation whose cleanup failed"
    );
    assert_eq!(json_records(&directory_path).len(), 2);
    assert!(directory_path.join("00000000000000000001.json").is_file());
    assert!(directory_path.join("00000000000000000002.json").is_file());
}

#[test]
#[ignore = "subprocess helper invoked by abandoned_private_reservation_is_preserved"]
fn interrupted_reservation_child() {
    let Some(root) = std::env::var_os("NESSA_AUDIT_RESERVATION_ROOT") else {
        return;
    };
    let directory =
        PrivateDirectory::open_beneath(std::path::Path::new(&root), std::path::Path::new("audit"))
            .unwrap();
    let mut reservation = directory.reserve_temp().unwrap();
    reservation.as_file_mut().write_all(b"interrupted").unwrap();
    reservation.as_file().sync_all().unwrap();
    std::fs::write(
        std::path::Path::new(&root).join("reservation-name"),
        reservation.name().to_string_lossy().as_bytes(),
    )
    .unwrap();
    std::process::exit(0);
}

#[test]
fn post_publish_ack_failure_retries_to_one_physical_record() {
    let root = temporary_root();
    let directory = root.path().join("audit");
    let audit = audit_at(root.path());
    let (_, started) = InstallAttempt::start(agent(), target(), request());

    let failure = audit
        .record_with_acknowledger(started.clone(), |_, _, published| {
            Err(published_failure(
                published.clone(),
                "injected acknowledgement failure",
            ))
        })
        .unwrap_err();
    assert_eq!(failure.stage(), AuditFailureStage::AcknowledgeRecord);
    assert!(matches!(
        failure.record(),
        Some(AuditRecordEvidence::IncomingPublished(_))
    ));

    assert_eq!(
        audit.record(started).unwrap(),
        AuditAcknowledgement::Replayed
    );
    assert_eq!(json_records(&directory).len(), 1);
}

#[test]
fn truncated_failure_evidence_roundtrips_and_replays_with_retained_bytes() {
    let root = temporary_root();
    let directory = root.path().join("audit");
    let audit = audit_at(root.path());
    let (mut attempt, started) = InstallAttempt::start(agent(), target(), request());
    audit.record(started).unwrap();
    audit.record(attempt.verified().unwrap()).unwrap();
    let detail = "é".repeat(InstallFailureEvidence::MAX_DETAIL_BYTES);
    let publication = InstallFailureEvidence::capture(InstallFailureKind::Unwritable, &detail);
    let confirmation = InstallFailureEvidence::capture(InstallFailureKind::Unreadable, &detail);
    let failures =
        RecoveryFailureEvidence::new(publication, None, None, Some(confirmation)).unwrap();
    let recovery = attempt
        .recovery_incomplete(RecoveryState::Unconfirmed, failures)
        .unwrap();

    assert_eq!(
        audit.record(recovery.clone()).unwrap(),
        AuditAcknowledgement::Recorded
    );
    let encoded = std::fs::read(directory.join("00000000000000000003.json")).unwrap();
    assert!(encoded.len() < MAX_AUDIT_RECORD_BYTES);
    let stored: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(
        stored["transition"]["facts"]["failures"]["publication"]["detail"]
            .as_str()
            .unwrap()
            .len(),
        InstallFailureEvidence::MAX_DETAIL_BYTES
    );
    assert_eq!(
        stored["transition"]["facts"]["failures"]["publication"]["truncated"],
        true
    );

    let reopened = audit_at(root.path());
    assert_eq!(
        reopened.record(recovery).unwrap(),
        AuditAcknowledgement::Replayed
    );
    assert_eq!(json_records(&directory).len(), 3);
}

#[test]
fn oversized_persisted_failure_detail_is_rejected_instead_of_normalized() {
    let root = temporary_root();
    let directory = root.path().join("audit");
    let audit = audit_at(root.path());
    let (mut attempt, started) = InstallAttempt::start(agent(), target(), request());
    audit.record(started).unwrap();
    audit.record(attempt.verified().unwrap()).unwrap();
    let failures = RecoveryFailureEvidence::new(
        InstallFailureEvidence::capture(InstallFailureKind::Unwritable, "publication"),
        None,
        None,
        Some(InstallFailureEvidence::capture(
            InstallFailureKind::Unreadable,
            "confirmation",
        )),
    )
    .unwrap();
    audit
        .record(
            attempt
                .recovery_incomplete(RecoveryState::Unconfirmed, failures)
                .unwrap(),
        )
        .unwrap();
    let record_path = directory.join("00000000000000000003.json");
    let mut stored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&record_path).unwrap()).unwrap();
    stored["transition"]["facts"]["failures"]["publication"]["detail"] = "x"
        .repeat(InstallFailureEvidence::MAX_DETAIL_BYTES + 1)
        .into();
    std::fs::write(&record_path, serde_json::to_vec(&stored).unwrap()).unwrap();
    let (_, next) = InstallAttempt::start(
        agent(),
        target(),
        InstallRequest::new("unix:501", "next").unwrap(),
    );

    let failure = audit.record(next).unwrap_err();
    assert_eq!(failure.stage(), AuditFailureStage::ReadJournal);
    assert!(failure.detail().contains("exceeds its byte limit"));
}

#[test]
fn restored_recovery_rejects_both_confirmation_state_contradictions() {
    for confirmed in [true, false] {
        let root = temporary_root();
        let directory = root.path().join("audit");
        let audit = audit_at(root.path());
        let (mut attempt, started) = InstallAttempt::start(agent(), target(), request());
        audit.record(started).unwrap();
        audit.record(attempt.verified().unwrap()).unwrap();
        let cleanup = InstallFailureEvidence::capture(InstallFailureKind::Unwritable, "cleanup");
        let failures = if confirmed {
            RecoveryFailureEvidence::new(
                InstallFailureEvidence::capture(InstallFailureKind::Unwritable, "publication"),
                Some(cleanup),
                None,
                None,
            )
            .unwrap()
        } else {
            RecoveryFailureEvidence::new(
                InstallFailureEvidence::capture(InstallFailureKind::Unwritable, "publication"),
                Some(InstallFailureEvidence::capture(
                    InstallFailureKind::Unwritable,
                    "withdrawal",
                )),
                None,
                Some(cleanup),
            )
            .unwrap()
        };
        let state = if confirmed {
            RecoveryState::Confirmed(RollbackState::NoInstalledRuntime)
        } else {
            RecoveryState::Unconfirmed
        };
        audit
            .record(attempt.recovery_incomplete(state, failures).unwrap())
            .unwrap();
        let record_path = directory.join("00000000000000000003.json");
        let mut stored: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&record_path).unwrap()).unwrap();
        if confirmed {
            let publication = stored["transition"]["facts"]["failures"]["publication"].clone();
            stored["transition"]["facts"]["failures"]["confirmation"] = publication;
        } else {
            stored["transition"]["facts"]["failures"]["confirmation"] = serde_json::Value::Null;
        }
        let decoded: StoredRecord = serde_json::from_value(stored.clone()).unwrap();
        let fixture_failure = decoded.restore_transition().unwrap_err();
        let expected = if confirmed {
            "confirmed recovery cannot contain a confirmation failure"
        } else {
            "unconfirmed recovery requires a confirmation failure"
        };
        assert!(
            fixture_failure.detail().contains(expected),
            "unexpected direct restoration refusal: {}",
            fixture_failure.detail()
        );
        let encoded = serde_json::to_vec(&stored).unwrap();
        let retained =
            PrivateDirectory::open_beneath(root.path(), std::path::Path::new("audit")).unwrap();
        let mut file = retained
            .open_file(
                std::ffi::OsStr::new("00000000000000000003.json"),
                OpenMode::ReadWrite,
            )
            .unwrap();
        file.set_len(0).unwrap();
        file.rewind().unwrap();
        file.write_all(&encoded).unwrap();
        file.sync_all().unwrap();
        retained.sync().unwrap();
        let (_, next) = InstallAttempt::start(
            agent(),
            target(),
            InstallRequest::new("unix:501", "next").unwrap(),
        );

        let failure = audit.record(next).unwrap_err();
        assert_eq!(failure.stage(), AuditFailureStage::ReadJournal);
        assert!(
            failure.detail().contains(expected),
            "unexpected journal restoration refusal: {}",
            failure.detail()
        );
    }
}

#[test]
fn contradictory_facts_in_one_domain_slot_are_rejected_without_claiming_incoming_publication() {
    let root = temporary_root();
    let audit = audit_at(root.path());
    let (mut accepted, started) = InstallAttempt::start(agent(), target(), request());
    audit.record(started).unwrap();
    audit.record(accepted.verified().unwrap()).unwrap();
    let (mut conflicting, _) = InstallAttempt::start(agent(), target(), request());
    let rejected = conflicting
        .rejected(ArchiveDigest::parse(OTHER_DIGEST).unwrap())
        .unwrap();

    let failure = audit.record(rejected).unwrap_err();

    assert_eq!(failure.stage(), AuditFailureStage::ReconcileRecord);
    assert_eq!(
        failure.semantic_conflict_kind(),
        Some(InstallAttemptError::ConflictingEvent)
    );
    assert!(matches!(
        failure.record(),
        Some(AuditRecordEvidence::ExistingConflict(record))
            if record.event().slot() == InstallEventSlot::VerificationOutcome
    ));
}

#[test]
fn replacing_the_named_lock_is_detected_before_append() {
    let root = temporary_root();
    let directory = root.path().join("audit");
    let audit = audit_at(root.path());
    std::fs::remove_file(directory.join(LOCK_NAME)).unwrap();
    std::fs::write(directory.join(LOCK_NAME), b"").unwrap();
    let (_, started) = InstallAttempt::start(agent(), target(), request());

    let failure = audit.record(started).unwrap_err();

    assert!(matches!(
        failure.stage(),
        AuditFailureStage::AcquireLock | AuditFailureStage::VerifyAuthority
    ));
    assert!(json_records(&directory).is_empty());
}

#[test]
fn renaming_the_retained_directory_cannot_redirect_append_to_a_replacement() {
    let root = temporary_root();
    let directory = root.path().join("audit");
    let moved = root.path().join("moved-audit");
    let audit = audit_at(root.path());
    std::fs::rename(&directory, &moved).unwrap();
    std::fs::create_dir(&directory).unwrap();
    let (_, started) = InstallAttempt::start(agent(), target(), request());

    let failure = audit.record(started).unwrap_err();

    assert!(matches!(
        failure.stage(),
        AuditFailureStage::AcquireLock | AuditFailureStage::VerifyAuthority
    ));
    assert!(json_records(&directory).is_empty());
    assert!(json_records(&moved).is_empty());
}

#[test]
fn concurrent_instances_serialize_distinct_attempts_into_contiguous_records() {
    let root = temporary_root();
    let directory = root.path().join("audit");
    let first = audit_at(root.path());
    let second = audit_at(root.path());
    let barrier = Barrier::new(3);
    let (_, first_transition) = InstallAttempt::start(
        agent(),
        target(),
        InstallRequest::new("unix:501", "first").unwrap(),
    );
    let (_, second_transition) = InstallAttempt::start(
        agent(),
        target(),
        InstallRequest::new("unix:501", "second").unwrap(),
    );

    std::thread::scope(|threads| {
        threads.spawn(|| {
            barrier.wait();
            first.record(first_transition).unwrap();
        });
        threads.spawn(|| {
            barrier.wait();
            second.record(second_transition).unwrap();
        });
        barrier.wait();
    });

    assert_eq!(json_records(&directory).len(), 2);
    assert!(directory.join("00000000000000000001.json").is_file());
    assert!(directory.join("00000000000000000002.json").is_file());
}

#[test]
fn corrupt_or_non_regular_entries_stop_reconciliation() {
    let root = temporary_root();
    let directory = root.path().join("audit");
    let audit = audit_at(root.path());
    std::fs::write(directory.join("00000000000000000001.json"), b"{").unwrap();
    let (_, started) = InstallAttempt::start(agent(), target(), request());
    assert_eq!(
        audit.record(started).unwrap_err().stage(),
        AuditFailureStage::ReadJournal
    );

    std::fs::remove_file(directory.join("00000000000000000001.json")).unwrap();
    std::fs::create_dir(directory.join("00000000000000000001.json")).unwrap();
    let (_, started) = InstallAttempt::start(agent(), target(), request());
    assert_eq!(
        audit.record(started).unwrap_err().stage(),
        AuditFailureStage::ReadJournal
    );
}

#[test]
fn temporary_near_misses_and_non_regular_matching_names_are_refused() {
    for name in [
        ".nessa-0123456789abcdef0123456789abcdeg.tmp",
        ".nessa-0123456789abcdef0123456789abcdef0.tmp",
    ] {
        let root = temporary_root();
        let directory = root.path().join("audit");
        let audit = audit_at(root.path());
        std::fs::write(directory.join(name), b"orphan").unwrap();
        let (_, started) = InstallAttempt::start(agent(), target(), request());
        assert_eq!(
            audit.record(started).unwrap_err().stage(),
            AuditFailureStage::ReadJournal
        );
    }

    let root = temporary_root();
    let directory = root.path().join("audit");
    let audit = audit_at(root.path());
    std::fs::create_dir(directory.join(".nessa-0123456789abcdef0123456789abcdef.tmp")).unwrap();
    let (_, started) = InstallAttempt::start(agent(), target(), request());
    assert_eq!(
        audit.record(started).unwrap_err().stage(),
        AuditFailureStage::ReadJournal
    );
}

#[test]
fn oversized_record_is_refused_by_the_bounded_reader_before_decode() {
    let root = temporary_root();
    let audit = audit_at(root.path());
    let retained =
        PrivateDirectory::open_beneath(root.path(), std::path::Path::new("audit")).unwrap();
    let mut file = retained
        .open_file(
            std::ffi::OsStr::new("00000000000000000001.json"),
            OpenMode::CreateNew,
        )
        .unwrap();
    file.write_all(&vec![b' '; MAX_AUDIT_RECORD_BYTES + 1])
        .unwrap();
    file.sync_all().unwrap();
    retained.sync().unwrap();
    let (_, started) = InstallAttempt::start(agent(), target(), request());

    let failure = audit.record(started).unwrap_err();
    assert_eq!(failure.stage(), AuditFailureStage::ReadJournal);
    assert!(failure.detail().contains("exceeds its byte limit"));
}

#[test]
fn writer_refuses_an_encoded_record_over_its_reader_limit_before_reservation() {
    let root = temporary_root();
    let directory = root.path().join("audit");
    let audit = audit_at(root.path());
    let files = std::iter::once(ReleaseFile::new(
        ArchivePath::parse("launch").unwrap(),
        FileRole::Launch,
    ))
    .chain((0..1_500).map(|index| {
        ReleaseFile::new(
            ArchivePath::parse(&format!("document-{index:04}-{}", "x".repeat(24))).unwrap(),
            FileRole::Document,
        )
    }))
    .collect();
    let oversized_target = RuntimeArtifact::new(
        ReleaseVersion::parse("1.18.31").unwrap(),
        ArchiveDigest::parse(PINNED_DIGEST).unwrap(),
        ReleaseContents::new(files).unwrap(),
    );
    let (_, started) = InstallAttempt::start(agent(), oversized_target, request());

    let failure = audit.record(started).unwrap_err();
    assert_eq!(failure.stage(), AuditFailureStage::WriteRecord);
    assert!(json_records(&directory).is_empty());
    assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
}

#[cfg(unix)]
#[test]
fn a_symlink_record_is_never_followed() {
    let root = temporary_root();
    let directory = root.path().join("audit");
    let outside = root.path().join("outside.json");
    let audit = audit_at(root.path());
    std::fs::write(&outside, b"outside").unwrap();
    std::os::unix::fs::symlink(&outside, directory.join("00000000000000000001.json")).unwrap();
    let (_, started) = InstallAttempt::start(agent(), target(), request());

    assert_eq!(
        audit.record(started).unwrap_err().stage(),
        AuditFailureStage::ReadJournal
    );
    assert_eq!(std::fs::read(&outside).unwrap(), b"outside");
}

#[test]
fn domain_restore_preserves_replacement_artifacts_and_rejects_duplicate_physical_events() {
    let root = temporary_root();
    let directory = root.path().join("audit");
    let audit = audit_at(root.path());
    let previous = RuntimeArtifact::for_release(&release("1.17.0", OTHER_DIGEST, &platform()));
    let (mut attempt, started) = InstallAttempt::start(agent(), target(), request());
    audit.record(started).unwrap();
    audit.record(attempt.verified().unwrap()).unwrap();
    let replaced = attempt.replaced(previous).unwrap();
    audit.record(replaced).unwrap();

    let mut third: serde_json::Value = serde_json::from_slice(
        &std::fs::read(directory.join("00000000000000000003.json")).unwrap(),
    )
    .unwrap();
    third["sequence"] = 4.into();
    std::fs::write(
        directory.join("00000000000000000004.json"),
        serde_json::to_vec(&third).unwrap(),
    )
    .unwrap();
    let (_, next) = InstallAttempt::start(
        agent(),
        target(),
        InstallRequest::new("unix:501", "next").unwrap(),
    );
    assert_eq!(
        audit.record(next).unwrap_err().stage(),
        AuditFailureStage::ReadJournal
    );
}

#[test]
fn nested_audit_directory_is_created_beneath_the_trusted_root() {
    let root = temporary_root();
    let audit = DurableInstallAudit::new(
        root.path(),
        std::path::Path::new("audit/agent-install"),
        Arc::new(FixedClock),
    )
    .unwrap();
    let (_, started) = InstallAttempt::start(agent(), target(), request());
    audit.record(started).unwrap();
    assert!(
        root.path()
            .join("audit/agent-install/00000000000000000001.json")
            .is_file()
    );
}

#[cfg(unix)]
#[test]
fn cross_process_writers_use_one_file_lock_and_contiguous_sequence() {
    let root = temporary_root();
    let directory = root.path().join("audit");
    let _audit = audit_at(root.path());
    let executable = std::env::current_exe().unwrap();
    let mut children = ["process-one", "process-two"].map(|request_id| {
        std::process::Command::new(&executable)
            .args(["--ignored", "cross_process_writer_child"])
            .env("NESSA_AUDIT_CHILD_ROOT", root.path())
            .env("NESSA_AUDIT_CHILD_REQUEST", request_id)
            .spawn()
            .unwrap()
    });
    for child in &mut children {
        assert!(child.wait().unwrap().success());
    }
    assert_eq!(json_records(&directory).len(), 2);
    assert!(directory.join("00000000000000000001.json").is_file());
    assert!(directory.join("00000000000000000002.json").is_file());
}

#[cfg(unix)]
#[test]
#[ignore = "subprocess helper invoked by cross_process_writers_use_one_file_lock_and_contiguous_sequence"]
fn cross_process_writer_child() {
    let Some(root) = std::env::var_os("NESSA_AUDIT_CHILD_ROOT") else {
        return;
    };
    let request_id = std::env::var("NESSA_AUDIT_CHILD_REQUEST").unwrap();
    let audit = audit_at(std::path::Path::new(&root));
    let (_, started) = InstallAttempt::start(
        agent(),
        target(),
        InstallRequest::new("unix:501", request_id).unwrap(),
    );
    assert_eq!(
        audit.record(started).unwrap(),
        AuditAcknowledgement::Recorded
    );
}

#[test]
fn storage_publication_stages_preserve_post_rename_identity_and_cleanup_independently() {
    let event = InstallEventIdentity::new(request(), InstallEventSlot::Started);
    for stage in [
        PrivatePublicationStage::ValidateDestination,
        PrivatePublicationStage::VerifyOriginBinding,
        PrivatePublicationStage::ValidateReservation,
        PrivatePublicationStage::FlushBeforeRename,
        PrivatePublicationStage::Rename,
    ] {
        let failure = publication_failure(
            stage,
            "injected".into(),
            PublishedAuditRecord::new(
                event.clone(),
                Uuid::new_v4().to_string(),
                1,
                "00000000000000000001.json".into(),
            ),
            false,
            Some("cleanup".into()),
        );
        assert_eq!(failure.stage(), AuditFailureStage::PublishRecord);
        assert!(failure.record().is_none());
        assert_eq!(failure.cleanup(), Some("cleanup"));
    }
    for stage in [
        PrivatePublicationStage::FlushAfterRename,
        PrivatePublicationStage::ValidatePublishedDestination,
        PrivatePublicationStage::VerifyPublishedBinding,
        PrivatePublicationStage::SyncDirectory,
    ] {
        let failure = publication_failure(
            stage,
            "injected".into(),
            PublishedAuditRecord::new(
                event.clone(),
                Uuid::new_v4().to_string(),
                1,
                "00000000000000000001.json".into(),
            ),
            true,
            None,
        );
        assert_eq!(failure.stage(), AuditFailureStage::AcknowledgeRecord);
        assert!(matches!(
            failure.record(),
            Some(AuditRecordEvidence::IncomingPublished(_))
        ));
    }
}

#[cfg(unix)]
#[test]
fn a_non_utf8_record_name_is_rejected_without_lossy_aliasing() {
    use std::os::unix::ffi::OsStringExt;

    let root = temporary_root();
    let directory = root.path().join("audit");
    let audit = audit_at(root.path());
    let name = std::ffi::OsString::from_vec(vec![b'0', 0xff, b'.', b'j', b's', b'o', b'n']);
    match std::fs::write(directory.join(name), b"{}") {
        Ok(()) => {
            let (_, started) = InstallAttempt::start(agent(), target(), request());
            assert_eq!(
                audit.record(started).unwrap_err().stage(),
                AuditFailureStage::ReadJournal
            );
        }
        Err(error) => {
            assert_eq!(
                error.raw_os_error(),
                Some(92),
                "the platform neither created the non-UTF-8 name nor refused it as EILSEQ"
            );
        }
    }
    assert!(json_records(&directory).is_empty());
}
