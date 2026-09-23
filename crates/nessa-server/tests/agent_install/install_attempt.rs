use super::*;
use crate::agent_install::domain::{
    InstallAttemptError, InstallFailureEvidence, InstallFailureKind, InstallTransitionError,
    InstallTransitionKind, RecoveryFailureEvidence, RecoveryState, RollbackState, RuntimeArtifact,
};
use crate::agent_install_test_support::{
    agent, platform, release, request, OTHER_DIGEST, PINNED_DIGEST,
};

fn artifact(version: &str, digest: &str) -> RuntimeArtifact {
    RuntimeArtifact::for_release(&release(version, digest, &platform()))
}

#[test]
fn one_attempt_enforces_the_evidence_sequence() {
    let target = artifact("1.18.31", PINNED_DIGEST);
    let (mut attempt, started) = InstallAttempt::start(agent(), target, request());
    assert_eq!(started.kind(), InstallTransitionKind::Started);

    let verified = attempt.verified().expect("started may be verified");
    assert_eq!(verified.kind(), InstallTransitionKind::Verified);
    let installed = attempt.installed().expect("verified may be installed");
    assert_eq!(installed.kind(), InstallTransitionKind::Installed);
    assert_eq!(attempt.installed(), Err(InstallAttemptError::WrongStage));
}

#[test]
fn a_matching_digest_cannot_be_recorded_as_rejected() {
    let target = artifact("1.18.31", PINNED_DIGEST);
    let (mut attempt, _) = InstallAttempt::start(agent(), target.clone(), request());

    assert!(matches!(
        attempt.rejected(target.digest().clone()),
        Err(InstallAttemptError::Contradictory(_))
    ));
    assert!(attempt.verified().is_ok(), "rejection changed prior state");
}

#[test]
fn an_artifact_cannot_replace_or_restore_itself() {
    let target = artifact("1.18.31", PINNED_DIGEST);
    let (mut replacement, _) = InstallAttempt::start(agent(), target.clone(), request());
    assert_eq!(
        replacement.verified().unwrap().kind(),
        InstallTransitionKind::Verified
    );
    assert!(matches!(
        replacement.replaced(target.clone()),
        Err(InstallAttemptError::Contradictory(_))
    ));

    let (mut rollback, _) = InstallAttempt::start(agent(), target.clone(), request());
    assert_eq!(
        rollback.verified().unwrap().kind(),
        InstallTransitionKind::Verified
    );
    assert!(matches!(
        rollback.rolled_back(RollbackState::Restored(target)),
        Err(InstallAttemptError::Contradictory(_))
    ));
}

#[test]
fn rejection_requires_the_started_state_and_preserves_both_digests() {
    let target = artifact("1.18.31", PINNED_DIGEST);
    let (mut attempt, _) = InstallAttempt::start(agent(), target.clone(), request());
    let actual = ArchiveDigest::parse(OTHER_DIGEST).unwrap();
    let rejected = attempt.rejected(actual.clone()).unwrap();

    assert_eq!(rejected.target(), &target);
    assert_eq!(rejected.actual_digest(), Some(&actual));
    assert_eq!(
        attempt.verified(),
        Err(InstallAttemptError::ConflictingEvent)
    );
}

#[test]
fn incomplete_recovery_rejects_a_target_reported_as_restored_without_ending_the_attempt() {
    let target = artifact("1.18.31", PINNED_DIGEST);
    let (mut attempt, _) = InstallAttempt::start(agent(), target.clone(), request());
    assert_eq!(
        attempt.verified().unwrap().kind(),
        InstallTransitionKind::Verified
    );
    let failures = RecoveryFailureEvidence::new(
        InstallFailureEvidence::capture(InstallFailureKind::Unwritable, "publish"),
        Some(InstallFailureEvidence::capture(
            InstallFailureKind::Unwritable,
            "withdrawal",
        )),
        None,
        None,
    )
    .unwrap();

    assert_eq!(
        attempt.recovery_incomplete(
            RecoveryState::Confirmed(RollbackState::Restored(target)),
            failures,
        ),
        Err(InstallAttemptError::Contradictory(
            InstallTransitionError::TargetReportedRestored
        ))
    );
    assert!(attempt
        .rolled_back(RollbackState::NoInstalledRuntime)
        .is_ok());
}

#[test]
fn incomplete_recovery_requires_at_least_one_cleanup_failure() {
    let failure = RecoveryFailureEvidence::new(
        InstallFailureEvidence::capture(InstallFailureKind::Unwritable, "publish"),
        None,
        None,
        None,
    );

    assert_eq!(failure, Err(InstallTransitionError::MissingCleanupFailure));
}

#[test]
fn failure_evidence_bounds_utf8_without_changing_restored_truncation_metadata() {
    let exact = "é".repeat(InstallFailureEvidence::MAX_DETAIL_BYTES / 2);
    let captured = InstallFailureEvidence::capture(InstallFailureKind::Unreadable, &exact);
    assert_eq!(captured.detail(), exact);
    assert!(!captured.truncated());

    let oversized = format!("{exact}é");
    let captured = InstallFailureEvidence::capture(InstallFailureKind::Unreadable, &oversized);
    assert_eq!(
        captured.detail().len(),
        InstallFailureEvidence::MAX_DETAIL_BYTES
    );
    assert!(captured.truncated());
    assert_eq!(
        InstallFailureEvidence::restore(
            captured.kind(),
            captured.detail().to_owned(),
            captured.truncated(),
        ),
        Ok(captured)
    );
    assert_eq!(
        InstallFailureEvidence::restore(
            InstallFailureKind::Unreadable,
            "x".repeat(InstallFailureEvidence::MAX_DETAIL_BYTES + 1),
            false,
        ),
        Err(InstallTransitionError::FailureDetailTooLong)
    );
}

#[test]
fn incomplete_recovery_confirmation_fact_must_agree_with_its_state() {
    let target = artifact("1.18.31", PINNED_DIGEST);
    let publication = InstallFailureEvidence::capture(InstallFailureKind::Unwritable, "publish");
    let cleanup = InstallFailureEvidence::capture(InstallFailureKind::Unwritable, "cleanup");
    let confirmed_with_failure = RecoveryFailureEvidence::new(
        publication.clone(),
        Some(cleanup.clone()),
        None,
        Some(cleanup.clone()),
    )
    .unwrap();
    let unconfirmed_without_failure =
        RecoveryFailureEvidence::new(publication.clone(), Some(cleanup.clone()), None, None)
            .unwrap();

    for (state, failures, expected) in [
        (
            RecoveryState::Confirmed(RollbackState::NoInstalledRuntime),
            confirmed_with_failure,
            InstallTransitionError::ConfirmedWithConfirmationFailure,
        ),
        (
            RecoveryState::Unconfirmed,
            unconfirmed_without_failure,
            InstallTransitionError::UnconfirmedWithoutConfirmationFailure,
        ),
    ] {
        let (mut attempt, _) = InstallAttempt::start(agent(), target.clone(), request());
        assert_eq!(
            attempt.verified().unwrap().kind(),
            InstallTransitionKind::Verified
        );
        assert_eq!(
            attempt.recovery_incomplete(state, failures),
            Err(InstallAttemptError::Contradictory(expected))
        );
    }

    for (state, failures) in [
        (
            RecoveryState::Confirmed(RollbackState::NoInstalledRuntime),
            RecoveryFailureEvidence::new(
                publication.clone(),
                Some(cleanup.clone()),
                Some(cleanup.clone()),
                None,
            )
            .unwrap(),
        ),
        (
            RecoveryState::Unconfirmed,
            RecoveryFailureEvidence::new(publication, None, None, Some(cleanup)).unwrap(),
        ),
    ] {
        let (mut attempt, _) = InstallAttempt::start(agent(), target.clone(), request());
        assert_eq!(
            attempt.verified().unwrap().kind(),
            InstallTransitionKind::Verified
        );
        assert!(attempt.recovery_incomplete(state, failures).is_ok());
    }
}

#[test]
fn restored_history_uses_the_same_admission_rule_and_accepts_exact_replay_after_terminal() {
    let target = artifact("1.18.31", PINNED_DIGEST);
    let (mut live, started) = InstallAttempt::start(agent(), target, request());
    let verified = live.verified().unwrap();
    let installed = live.installed().unwrap();
    let mut restored = InstallAttempt::from_started(started.clone()).unwrap();

    assert_eq!(
        restored.admit(verified.clone()),
        Ok(InstallEventAdmission::Added)
    );
    assert_eq!(restored.admit(installed), Ok(InstallEventAdmission::Added));
    assert_eq!(restored.admit(started), Ok(InstallEventAdmission::Replay));
    assert_eq!(restored.admit(verified), Ok(InstallEventAdmission::Replay));
}

#[test]
fn one_slot_rejects_contradictory_facts_and_attempt_facts_are_immutable() {
    let target = artifact("1.18.31", PINNED_DIGEST);
    let (mut live, started) = InstallAttempt::start(agent(), target.clone(), request());
    let verified = live.verified().unwrap();
    let (mut conflicting, _) = InstallAttempt::start(agent(), target.clone(), request());
    let rejected = conflicting
        .rejected(ArchiveDigest::parse(OTHER_DIGEST).unwrap())
        .unwrap();
    let mut restored = InstallAttempt::from_started(started).unwrap();
    restored.admit(verified).unwrap();

    assert_eq!(
        restored.admit(rejected),
        Err(InstallAttemptError::ConflictingEvent)
    );

    let other_agent = AgentName::parse("codex").unwrap();
    let (_, other_started) = InstallAttempt::start(other_agent, target, request());
    assert_eq!(
        restored.admit(other_started),
        Err(InstallAttemptError::ConflictingAttempt)
    );
}

#[test]
fn account_identity_is_part_of_the_stable_event_identity() {
    let target = artifact("1.18.31", PINNED_DIGEST);
    let (_, first) = InstallAttempt::start(agent(), target.clone(), request());
    let (_, second) = InstallAttempt::start(
        agent(),
        target,
        InstallRequest::new("unix:502", "install-request-1").unwrap(),
    );

    assert_ne!(first.event_identity(), second.event_identity());
}
