use super::*;
use crate::agent_install::application::{PublicationCleanupFailure, RollbackChange};
use crate::agent_install::domain::{
    InstallTransitionKind, Libc, RecoveryState, ReleasePlatform, ReleaseRequirements,
    RollbackState, RuntimeArtifact,
};
use crate::agent_install_test_support::{
    agent, audit, host, host_of, platform, release, release_needing, request, FakeSource,
    FakeStore, RecordingAudit, OTHER_DIGEST, PINNED_DIGEST,
};
use std::sync::{mpsc, Mutex};
use std::time::Duration;

struct BlockingAudit {
    target: InstallTransitionKind,
    entered: mpsc::SyncSender<()>,
    release: Mutex<mpsc::Receiver<()>>,
    fail: bool,
}

impl InstallAudit for BlockingAudit {
    fn record(&self, transition: InstallTransition) -> Result<(), AuditFailure> {
        if transition.kind() == self.target {
            self.entered.send(()).unwrap();
            self.release.lock().unwrap().recv().unwrap();
            if self.fail {
                return Err(AuditFailure("sink refused".into()));
            }
        }
        Ok(())
    }
}

#[test]
fn a_matching_archive_is_published() {
    let root = tempfile::tempdir().expect("temporary root");
    let source = FakeSource::serving(b"archive bytes");
    let store = FakeStore::empty(root.path());
    let platform = platform();
    let host = host();

    let installed = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: audit(),
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform),
        &host,
        &request(),
    )
    .expect("a matching archive installs");

    assert_eq!(installed.version.as_str(), "1.18.31");
    assert!(installed.downloaded);
    assert_eq!(store.published(), vec!["opencode".to_string()]);
}

#[test]
fn a_mismatched_archive_is_never_unpacked() {
    // The whole point of the pin. A download that is not the tested one must
    // not have its contents read, let alone placed somewhere Nessa will launch
    // from — so `publish` must not have been called at all.
    let root = tempfile::tempdir().expect("temporary root");
    let source = FakeSource::serving(b"someone else's bytes");
    let store = FakeStore::empty(root.path()).hashing(OTHER_DIGEST);
    let platform = platform();
    let host = host();

    let failure = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: audit(),
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform),
        &host,
        &request(),
    )
    .expect_err("a mismatched archive is refused");

    match failure {
        InstallFailure::Rejected(rejection) => {
            assert_eq!(rejection.expected().as_str(), PINNED_DIGEST);
            assert_eq!(rejection.actual().as_str(), OTHER_DIGEST);
        }
        other => panic!("expected a rejection, got {other:?}"),
    }
    assert!(
        store.published().is_empty(),
        "nothing may be published from an archive that was not the pinned one"
    );
}

#[test]
fn a_rejected_archive_is_discarded() {
    // A refused download is the largest thing this operation writes. Leaving it
    // behind would let a run of failures fill the disk.
    let root = tempfile::tempdir().expect("temporary root");
    let source = FakeSource::serving(b"someone else's bytes");
    let store = FakeStore::empty(root.path()).hashing(OTHER_DIGEST);
    let platform = platform();
    let host = host();

    let _ = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: audit(),
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform),
        &host,
        &request(),
    );

    assert_eq!(store.discarded().len(), 1, "the archive is discarded");
}

#[test]
fn a_successful_install_discards_its_archive_too() {
    let root = tempfile::tempdir().expect("temporary root");
    let source = FakeSource::serving(b"archive bytes");
    let store = FakeStore::empty(root.path());
    let platform = platform();
    let host = host();

    InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: audit(),
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform),
        &host,
        &request(),
    )
    .expect("a matching archive installs");

    assert_eq!(store.discarded().len(), 1);
}

#[test]
fn installing_what_is_already_installed_downloads_nothing() {
    let root = tempfile::tempdir().expect("temporary root");
    let source = FakeSource::serving(b"archive bytes");
    let store = FakeStore::holding(root.path());
    let platform = platform();
    let host = host();

    let installed = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: audit(),
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform),
        &host,
        &request(),
    )
    .expect("an installed runtime is reported as installed");

    assert!(!installed.downloaded);
    assert!(
        source.requested().is_empty(),
        "a runtime already at the pinned version is not fetched again"
    );
    assert!(store.published().is_empty());
}

#[test]
fn a_release_the_store_does_not_hold_is_downloaded() {
    // The store answers about one release, so "nothing installed" here covers
    // an empty machine and one holding a different version alike. Either way
    // the pinned release is fetched.
    let root = tempfile::tempdir().expect("temporary root");
    let source = FakeSource::serving(b"archive bytes");
    let store = FakeStore::empty(root.path());
    let platform = platform();
    let host = host();

    let installed = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: audit(),
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform),
        &host,
        &request(),
    )
    .expect("a newly pinned version installs over an older one");

    assert!(installed.downloaded);
    assert_eq!(installed.version.as_str(), "1.18.31");
    assert_eq!(source.requested().len(), 1);
}

#[test]
fn what_is_measured_is_what_is_unpacked() {
    // The reason the three middle steps share one open file. A store that was
    // handed a path could be measuring one file and unpacking another, and
    // every assertion about ordering above would still pass.
    let root = tempfile::tempdir().expect("temporary root");
    let source = FakeSource::serving(b"archive bytes");
    let store = FakeStore::empty(root.path());
    let platform = platform();
    let host = host();

    InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: audit(),
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform),
        &host,
        &request(),
    )
    .expect("a matching archive installs");

    assert_eq!(store.measured(), vec![b"archive bytes".to_vec()]);
    assert_eq!(store.unpacked(), store.measured());
}

#[test]
fn a_store_that_cannot_stage_a_download_fails_before_fetching() {
    // No file to download into is not a network problem, and asking for a
    // hundred megabytes with nowhere to put them helps nobody.
    let root = tempfile::tempdir().expect("temporary root");
    let source = FakeSource::serving(b"archive bytes");
    let unwritable = StoreFailure::Unwritable("no room".into());
    let store = FakeStore::empty(root.path()).failing_to_stage(unwritable.clone());
    let platform = platform();
    let host = host();

    let failure = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: audit(),
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform),
        &host,
        &request(),
    )
    .expect_err("a store with nowhere to stage fails the install");

    assert_eq!(failure, InstallFailure::Store(unwritable));
    assert!(source.requested().is_empty());
    assert!(
        store.discarded().is_empty(),
        "there is nothing to discard when nothing was staged"
    );
}

#[test]
fn a_release_for_another_platform_is_refused_before_anything_is_fetched() {
    let root = tempfile::tempdir().expect("temporary root");
    let source = FakeSource::serving(b"archive bytes");
    let store = FakeStore::empty(root.path());
    let elsewhere = host_of(
        &ReleasePlatform::new("linux", "x86_64").expect("usable platform"),
        Some(Libc::Gnu),
        true,
    );

    let failure = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: audit(),
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform()),
        &elsewhere,
        &request(),
    )
    .expect_err("a release for another platform is refused");

    assert_eq!(failure, InstallFailure::UnsupportedPlatform(elsewhere));
    assert!(source.requested().is_empty());
}

#[test]
fn a_download_failure_is_reported_as_one() {
    let root = tempfile::tempdir().expect("temporary root");
    let source = FakeSource::failing(SourceFailure::Refused(404));
    let store = FakeStore::empty(root.path());
    let platform = platform();
    let host = host();

    let failure = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: audit(),
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform),
        &host,
        &request(),
    )
    .expect_err("a refused download fails the install");

    assert_eq!(
        failure,
        InstallFailure::Download(SourceFailure::Refused(404))
    );
    assert!(store.published().is_empty());
}

#[test]
fn an_archive_without_the_pinned_executable_fails_as_a_store_problem() {
    // A verified archive that does not contain what the pin says it does is a
    // fault in the pin, and it must not be reported as a corrupted download —
    // retrying that would never help.
    let root = tempfile::tempdir().expect("temporary root");
    let source = FakeSource::serving(b"archive bytes");
    let missing = StoreFailure::MissingExecutable("package/bin/opencode".into());
    let store = FakeStore::empty(root.path()).failing_to_publish(missing.clone());
    let platform = platform();
    let host = host();

    let failure = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: audit(),
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform),
        &host,
        &request(),
    )
    .expect_err("an archive missing its executable fails the install");

    assert_eq!(failure, InstallFailure::Store(missing));
}

#[test]
fn a_store_that_cannot_say_what_is_installed_does_not_download() {
    // "Could not tell" is not "nothing is installed". Downloading on the
    // strength of an unanswered question would replace a working runtime
    // because a directory happened to be unreadable for a moment.
    let root = tempfile::tempdir().expect("temporary root");
    let source = FakeSource::serving(b"archive bytes");
    let unreadable = StoreFailure::Unreadable("runtime directory".into());
    let store = FakeStore::empty(root.path()).failing_to_read(unreadable.clone());
    let platform = platform();
    let host = host();

    let failure = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: audit(),
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform),
        &host,
        &request(),
    )
    .expect_err("an unreadable store fails the install");

    assert_eq!(failure, InstallFailure::Store(unreadable));
    assert!(source.requested().is_empty());
}

#[test]
fn the_pinned_url_is_what_gets_fetched() {
    let root = tempfile::tempdir().expect("temporary root");
    let source = FakeSource::serving(b"archive bytes");
    let store = FakeStore::empty(root.path());
    let platform = platform();
    let host = host();
    let release = release("1.18.31", PINNED_DIGEST, &platform);

    InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: audit(),
    }
    .execute(&agent(), &release, &host, &request())
    .expect("a matching archive installs");

    assert_eq!(
        source.requested(),
        vec![release.archive_url().as_str().to_string()]
    );
}

#[test]
fn the_fake_store_stages_the_way_the_real_one_does() {
    // A test double is only as good as the part of the port it models. The real
    // store creates a name of its own each time so that two installs at once
    // cannot truncate each other's download, and `StagedArchive` makes that a
    // promise of the port — so a fake that staged over one fixed path would let
    // a test assert an ordering guarantee while quietly modelling the thing the
    // guarantee exists to prevent.
    let root = tempfile::tempdir().expect("temporary root");
    let store = FakeStore::empty(root.path());

    let first = store.stage(&agent()).expect("a staged file");
    let second = store.stage(&agent()).expect("a second staged file");

    assert_ne!(
        first.path(),
        second.path(),
        "two downloads shared one staged file"
    );
}

#[test]
fn a_build_this_machine_cannot_run_is_refused_before_anything_is_fetched() {
    // The other half of the refusal, and the one this change introduced: the
    // platform matches and the *requirements* do not. A musl build on a glibc
    // machine dies in the loader, and an AVX2 build on a processor without it
    // dies on an illegal instruction, so neither is worth a hundred megabytes
    // first.
    //
    // The pre-existing half — a release for another operating system — is
    // covered above; the old code caught that one, and only that one.
    //
    // Linux x86-64 rather than the suite's usual macOS, because both scenarios
    // below need a platform where they could actually happen: macOS has one C
    // library and aarch64 has no AVX2, so a release making either demand there
    // is one `PinnedRelease::new` refuses to assemble at all.
    let platform = ReleasePlatform::new("linux", "x86_64").expect("usable platform");
    for (named, host, requirements) in [
        (
            "a musl build on a glibc machine",
            host_of(&platform, Some(Libc::Gnu), true),
            ReleaseRequirements::new(Some(Libc::Musl), false),
        ),
        (
            "a glibc build on a machine with neither",
            host_of(&platform, None, true),
            ReleaseRequirements::new(Some(Libc::Gnu), false),
        ),
        (
            "an avx2 build on a processor without it",
            host_of(&platform, Some(Libc::Gnu), false),
            ReleaseRequirements::new(Some(Libc::Gnu), true),
        ),
    ] {
        let root = tempfile::tempdir().expect("temporary root");
        let source = FakeSource::serving(b"archive bytes");
        let store = FakeStore::empty(root.path());

        let failure = InstallAgentRuntime {
            source: &source,
            store: &store,
            audit: audit(),
        }
        .execute(
            &agent(),
            &release_needing("1.18.31", PINNED_DIGEST, &platform, requirements),
            &host,
            &request(),
        )
        .expect_err(named);

        assert_eq!(failure, InstallFailure::UnsupportedPlatform(host));
        assert!(
            source.requested().is_empty(),
            "{named} was downloaded before it was refused"
        );
    }
}

#[test]
fn a_new_install_records_one_correlated_legal_sequence() {
    let root = tempfile::tempdir().unwrap();
    let source = FakeSource::serving(b"archive bytes");
    let store = FakeStore::empty(root.path());
    let audit = RecordingAudit::default();
    InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: &audit,
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform()),
        &host(),
        &request(),
    )
    .unwrap();

    let records = audit.records();
    assert_eq!(
        records
            .iter()
            .map(InstallTransition::kind)
            .collect::<Vec<_>>(),
        [
            InstallTransitionKind::Started,
            InstallTransitionKind::Verified,
            InstallTransitionKind::Installed,
        ]
    );
    assert!(records
        .iter()
        .all(|record| record.request().request_id() == "install-request-1"));
}

#[test]
fn replacement_evidence_uses_the_artifact_seen_under_the_publication_lock() {
    let root = tempfile::tempdir().unwrap();
    let source = FakeSource::serving(b"archive bytes");
    let previous = RuntimeArtifact::for_release(&release("1.17.0", OTHER_DIGEST, &platform()));
    let store = FakeStore::empty(root.path()).replacing(previous.clone());
    let audit = RecordingAudit::default();
    InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: &audit,
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform()),
        &host(),
        &request(),
    )
    .unwrap();

    let records = audit.records();
    let replaced = records.last().unwrap();
    assert_eq!(replaced.kind(), InstallTransitionKind::Replaced);
    assert_eq!(replaced.previous(), Some(&previous));
}

#[test]
fn digest_rejection_is_audited_and_the_archive_is_discarded() {
    let root = tempfile::tempdir().unwrap();
    let source = FakeSource::serving(b"other bytes");
    let store = FakeStore::empty(root.path()).hashing(OTHER_DIGEST);
    let audit = RecordingAudit::default();
    let failure = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: &audit,
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform()),
        &host(),
        &request(),
    )
    .unwrap_err();

    assert!(matches!(failure, InstallFailure::Rejected(_)));
    assert_eq!(
        audit.records().last().unwrap().kind(),
        InstallTransitionKind::DigestRejected
    );
    assert_eq!(store.discarded().len(), 1);
}

#[test]
fn store_rollback_is_audited_without_hiding_the_store_failure() {
    let root = tempfile::tempdir().unwrap();
    let source = FakeSource::serving(b"archive bytes");
    let store_failure = StoreFailure::Unwritable("directory sync failed".into());
    let store = FakeStore::empty(root.path())
        .failing_after_rollback(store_failure.clone(), RollbackChange::NoInstalledRuntime);
    let audit = RecordingAudit::default();
    let failure = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: &audit,
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform()),
        &host(),
        &request(),
    )
    .unwrap_err();

    assert_eq!(failure, InstallFailure::Store(store_failure));
    assert_eq!(
        audit.records().last().unwrap().kind(),
        InstallTransitionKind::RolledBack
    );
    assert_eq!(store.discarded().len(), 1);
}

#[test]
fn audit_failure_after_verification_is_visible_and_prevents_publication() {
    let root = tempfile::tempdir().unwrap();
    let source = FakeSource::serving(b"archive bytes");
    let store = FakeStore::empty(root.path());
    let audit = RecordingAudit::failing_on(InstallTransitionKind::Verified, "sink refused");
    let failure = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: &audit,
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform()),
        &host(),
        &request(),
    )
    .unwrap_err();

    assert!(matches!(
        failure,
        InstallFailure::Audit {
            runtime_state: RuntimeStateEvidence::Unchanged,
            ..
        }
    ));
    assert!(store.published().is_empty());
    assert_eq!(store.discarded().len(), 1);
}

#[test]
fn audit_failure_at_started_prevents_install_effects() {
    let root = tempfile::tempdir().unwrap();
    let source = FakeSource::serving(b"archive bytes");
    let store = FakeStore::empty(root.path());
    let audit = RecordingAudit::failing_on(InstallTransitionKind::Started, "sink refused");

    let failure = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: &audit,
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform()),
        &host(),
        &request(),
    )
    .unwrap_err();

    assert!(matches!(
        failure,
        InstallFailure::Audit {
            runtime_state: RuntimeStateEvidence::Unchanged,
            ..
        }
    ));
    assert!(source.requested().is_empty());
    assert!(store.published().is_empty());
}

#[test]
fn audit_failure_at_replaced_reports_the_new_runtime_and_prior_evidence() {
    let root = tempfile::tempdir().unwrap();
    let source = FakeSource::serving(b"archive bytes");
    let previous = RuntimeArtifact::for_release(&release("1.17.0", OTHER_DIGEST, &platform()));
    let store = FakeStore::empty(root.path()).replacing(previous.clone());
    let audit = RecordingAudit::failing_on(InstallTransitionKind::Replaced, "sink refused");

    let failure = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: &audit,
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform()),
        &host(),
        &request(),
    )
    .unwrap_err();

    assert!(matches!(
        failure,
        InstallFailure::Audit {
            runtime_state: RuntimeStateEvidence::TargetInstalled,
            ..
        }
    ));
    assert_eq!(audit.records().last().unwrap().previous(), Some(&previous));
}

#[test]
fn audit_failure_at_rollback_preserves_the_publication_failure() {
    let root = tempfile::tempdir().unwrap();
    let source = FakeSource::serving(b"archive bytes");
    let operation = StoreFailure::Unwritable("publish failed".into());
    let store = FakeStore::empty(root.path())
        .failing_after_rollback(operation.clone(), RollbackChange::NoInstalledRuntime);
    let audit = RecordingAudit::failing_on(InstallTransitionKind::RolledBack, "sink refused");

    let failure = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: &audit,
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform()),
        &host(),
        &request(),
    )
    .unwrap_err();

    assert!(matches!(
        failure,
        InstallFailure::Audit {
            operation: Some(inner),
            runtime_state: RuntimeStateEvidence::NoInstalledRuntime,
            ..
        }
            if *inner == InstallFailure::Store(operation)
    ));
}

#[test]
fn cleanup_failure_is_visible_without_replacing_the_publication_failure() {
    let root = tempfile::tempdir().unwrap();
    let source = FakeSource::serving(b"archive bytes");
    let operation = StoreFailure::Unwritable("record sync failed".into());
    let cleanup = StoreFailure::Unwritable("withdrawal failed".into());
    let cleanup_evidence =
        PublicationCleanupFailure::new(Some(cleanup), None, None).expect("one cleanup failure");
    let store = FakeStore::empty(root.path()).failing_after_incomplete_cleanup(
        operation.clone(),
        Some(RollbackChange::NoInstalledRuntime),
        cleanup_evidence.clone(),
    );
    let audit = RecordingAudit::default();

    let failure = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: &audit,
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform()),
        &host(),
        &request(),
    )
    .unwrap_err();

    assert_eq!(
        failure,
        InstallFailure::Recovery {
            operation,
            cleanup: Box::new(cleanup_evidence),
        }
    );
    assert_eq!(
        audit.records().last().unwrap().kind(),
        InstallTransitionKind::RecoveryIncomplete
    );
    let record = audit.records().pop().unwrap();
    let (state, failures) = record.recovery().unwrap();
    assert_eq!(
        state,
        &RecoveryState::Confirmed(RollbackState::NoInstalledRuntime)
    );
    assert_eq!(failures.publication().detail(), "record sync failed");
    assert_eq!(failures.withdrawal().unwrap().detail(), "withdrawal failed");
}

#[test]
fn incomplete_recovery_retains_the_confirmed_prior_artifact() {
    let root = tempfile::tempdir().unwrap();
    let source = FakeSource::serving(b"archive bytes");
    let previous = RuntimeArtifact::for_release(&release("1.17.0", OTHER_DIGEST, &platform()));
    let operation = StoreFailure::Unwritable("record sync failed".into());
    let cleanup = PublicationCleanupFailure::new(
        None,
        Some(StoreFailure::Unwritable("restoration sync failed".into())),
        None,
    )
    .unwrap();
    let store = FakeStore::empty(root.path()).failing_after_incomplete_cleanup(
        operation,
        Some(RollbackChange::Restored(previous.clone())),
        cleanup,
    );
    let audit =
        RecordingAudit::failing_on(InstallTransitionKind::RecoveryIncomplete, "sink refused");

    let failure = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: &audit,
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform()),
        &host(),
        &request(),
    )
    .unwrap_err();

    assert!(matches!(
        failure,
        InstallFailure::Audit {
            operation: Some(_),
            runtime_state: RuntimeStateEvidence::Restored(ref artifact),
            ..
        } if artifact == &previous
    ));
    let transition = audit.records().pop().unwrap();
    assert!(matches!(
        transition.recovery(),
        Some((RecoveryState::Confirmed(RollbackState::Restored(artifact)), _))
            if artifact == &previous
    ));
}

#[test]
fn unconfirmed_recovery_and_audit_failure_retain_every_failure() {
    let root = tempfile::tempdir().unwrap();
    let source = FakeSource::serving(b"archive bytes");
    let operation = StoreFailure::Unwritable("record sync failed".into());
    let withdrawal = StoreFailure::Unwritable("withdrawal failed".into());
    let restoration = StoreFailure::Unreadable("restoration failed".into());
    let confirmation = StoreFailure::Unreadable("confirmation failed".into());
    let cleanup_evidence = PublicationCleanupFailure::new(
        Some(withdrawal.clone()),
        Some(restoration.clone()),
        Some(confirmation.clone()),
    )
    .unwrap();
    let store = FakeStore::empty(root.path()).failing_after_incomplete_cleanup(
        operation.clone(),
        None,
        cleanup_evidence.clone(),
    );
    let audit =
        RecordingAudit::failing_on(InstallTransitionKind::RecoveryIncomplete, "sink refused");

    let failure = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: &audit,
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform()),
        &host(),
        &request(),
    )
    .unwrap_err();

    let InstallFailure::Audit {
        operation: Some(audited_operation),
        runtime_state,
        failure: AuditFailure(audit_detail),
    } = failure
    else {
        panic!("incomplete recovery and audit failure were not retained");
    };
    assert_eq!(runtime_state, RuntimeStateEvidence::Unconfirmed);
    assert_eq!(audit_detail, "sink refused");
    assert_eq!(
        *audited_operation,
        InstallFailure::Recovery {
            operation,
            cleanup: Box::new(cleanup_evidence),
        }
    );
    let transition = audit.records().pop().unwrap();
    let (state, failures) = transition.recovery().unwrap();
    assert_eq!(state, &RecoveryState::Unconfirmed);
    assert_eq!(failures.publication().detail(), "record sync failed");
    assert_eq!(failures.withdrawal().unwrap().detail(), "withdrawal failed");
    assert_eq!(
        failures.restoration().unwrap().detail(),
        "restoration failed"
    );
    assert_eq!(
        failures.confirmation().unwrap().detail(),
        "confirmation failed"
    );
}

#[test]
fn audit_failure_after_publication_reports_the_runtime_as_installed() {
    let root = tempfile::tempdir().unwrap();
    let source = FakeSource::serving(b"archive bytes");
    let store = FakeStore::empty(root.path());
    let audit = RecordingAudit::failing_on(InstallTransitionKind::Installed, "sink refused");
    let failure = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: &audit,
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform()),
        &host(),
        &request(),
    )
    .unwrap_err();

    assert!(matches!(
        failure,
        InstallFailure::Audit {
            runtime_state: RuntimeStateEvidence::TargetInstalled,
            ..
        }
    ));
    assert_eq!(store.published(), ["opencode"]);
    assert_eq!(store.discarded().len(), 1);
}

#[test]
fn audit_failure_preserves_digest_rejection_and_cleanup() {
    let root = tempfile::tempdir().unwrap();
    let source = FakeSource::serving(b"archive bytes");
    let store = FakeStore::empty(root.path()).hashing(OTHER_DIGEST);
    let audit = RecordingAudit::failing_on(InstallTransitionKind::DigestRejected, "sink refused");
    let failure = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: &audit,
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform()),
        &host(),
        &request(),
    )
    .unwrap_err();

    assert!(matches!(
        failure,
        InstallFailure::Audit {
            operation: Some(operation),
            runtime_state: RuntimeStateEvidence::Unchanged,
            ..
        } if matches!(*operation, InstallFailure::Rejected(_))
    ));
    assert!(store.published().is_empty());
    assert_eq!(store.discarded().len(), 1);
}

#[test]
fn successful_publication_lease_spans_a_failing_audit_and_then_releases() {
    let root = tempfile::tempdir().unwrap();
    let source = FakeSource::serving(b"archive bytes");
    let (lease_dropped, dropped) = mpsc::channel();
    let store = FakeStore::empty(root.path()).signalling_lease_drop(lease_dropped);
    let (entered_send, entered) = mpsc::sync_channel(0);
    let (release, release_recv) = mpsc::sync_channel(0);
    let audit = BlockingAudit {
        target: InstallTransitionKind::Installed,
        entered: entered_send,
        release: Mutex::new(release_recv),
        fail: true,
    };

    std::thread::scope(|threads| {
        let install = threads.spawn(|| {
            InstallAgentRuntime {
                source: &source,
                store: &store,
                audit: &audit,
            }
            .execute(
                &agent(),
                &crate::agent_install_test_support::release("1.18.31", PINNED_DIGEST, &platform()),
                &host(),
                &request(),
            )
        });
        entered.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(dropped.try_recv(), Err(mpsc::TryRecvError::Empty)));
        release.send(()).unwrap();
        assert!(matches!(
            install.join().unwrap(),
            Err(InstallFailure::Audit {
                runtime_state: RuntimeStateEvidence::TargetInstalled,
                ..
            })
        ));
        dropped.recv_timeout(Duration::from_secs(5)).unwrap();
    });
}

#[test]
fn rollback_lease_spans_successful_audit_and_then_releases() {
    let root = tempfile::tempdir().unwrap();
    let source = FakeSource::serving(b"archive bytes");
    let (lease_dropped, dropped) = mpsc::channel();
    let store = FakeStore::empty(root.path())
        .failing_after_rollback(
            StoreFailure::Unwritable("publish failed".into()),
            RollbackChange::NoInstalledRuntime,
        )
        .signalling_lease_drop(lease_dropped);
    let (entered_send, entered) = mpsc::sync_channel(0);
    let (release, release_recv) = mpsc::sync_channel(0);
    let audit = BlockingAudit {
        target: InstallTransitionKind::RolledBack,
        entered: entered_send,
        release: Mutex::new(release_recv),
        fail: false,
    };

    std::thread::scope(|threads| {
        let install = threads.spawn(|| {
            InstallAgentRuntime {
                source: &source,
                store: &store,
                audit: &audit,
            }
            .execute(
                &agent(),
                &crate::agent_install_test_support::release("1.18.31", PINNED_DIGEST, &platform()),
                &host(),
                &request(),
            )
        });
        entered.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(dropped.try_recv(), Err(mpsc::TryRecvError::Empty)));
        release.send(()).unwrap();
        assert!(matches!(
            install.join().unwrap(),
            Err(InstallFailure::Store(_))
        ));
        dropped.recv_timeout(Duration::from_secs(5)).unwrap();
    });
}

#[test]
fn uncertain_recovery_lease_spans_failing_audit_and_then_releases() {
    let root = tempfile::tempdir().unwrap();
    let source = FakeSource::serving(b"archive bytes");
    let cleanup = PublicationCleanupFailure::new(
        Some(StoreFailure::Unwritable("withdrawal failed".into())),
        None,
        Some(StoreFailure::Unreadable("confirmation failed".into())),
    )
    .unwrap();
    let (lease_dropped, dropped) = mpsc::channel();
    let store = FakeStore::empty(root.path())
        .failing_after_incomplete_cleanup(
            StoreFailure::Unwritable("publish failed".into()),
            None,
            cleanup,
        )
        .signalling_lease_drop(lease_dropped);
    let (entered_send, entered) = mpsc::sync_channel(0);
    let (release, release_recv) = mpsc::sync_channel(0);
    let audit = BlockingAudit {
        target: InstallTransitionKind::RecoveryIncomplete,
        entered: entered_send,
        release: Mutex::new(release_recv),
        fail: true,
    };

    std::thread::scope(|threads| {
        let install = threads.spawn(|| {
            InstallAgentRuntime {
                source: &source,
                store: &store,
                audit: &audit,
            }
            .execute(
                &agent(),
                &crate::agent_install_test_support::release("1.18.31", PINNED_DIGEST, &platform()),
                &host(),
                &request(),
            )
        });
        entered.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(dropped.try_recv(), Err(mpsc::TryRecvError::Empty)));
        release.send(()).unwrap();
        assert!(matches!(
            install.join().unwrap(),
            Err(InstallFailure::Audit {
                operation: Some(_),
                runtime_state: RuntimeStateEvidence::Unconfirmed,
                ..
            })
        ));
        dropped.recv_timeout(Duration::from_secs(5)).unwrap();
    });
}
