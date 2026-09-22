use super::*;
use crate::agent_install::domain::{
    InstallAttemptError, InstallTransitionKind, RollbackState, RuntimeArtifact,
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
    replacement.verified().unwrap();
    assert!(matches!(
        replacement.replaced(target.clone()),
        Err(InstallAttemptError::Contradictory(_))
    ));

    let (mut rollback, _) = InstallAttempt::start(agent(), target.clone(), request());
    rollback.verified().unwrap();
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
    assert_eq!(attempt.verified(), Err(InstallAttemptError::WrongStage));
}
