use crate::agent_install::domain::{
    AgentName, ArchiveDigest, ArchivePath, FileRole, InstallRequest, ReclamationActivation,
    ReclamationAdmission, ReclamationCause, ReclamationError, ReclamationObligation,
    ReclamationOperationId, ReclamationTrigger, ReleaseContents, ReleaseFile, ReleaseVersion,
    RuntimeArtifact,
};

fn artifact(version: &str, digest: char, executable: &str) -> RuntimeArtifact {
    RuntimeArtifact::new(
        ReleaseVersion::parse(version).unwrap(),
        ArchiveDigest::parse(&digest.to_string().repeat(64)).unwrap(),
        ReleaseContents::new(vec![ReleaseFile::new(
            ArchivePath::parse(executable).unwrap(),
            FileRole::Launch,
        )])
        .unwrap(),
    )
}

fn request(id: &str) -> InstallRequest {
    InstallRequest::new("account", id).unwrap()
}

#[test]
fn obligation_rejects_the_current_physical_artifact_despite_entry_difference() {
    assert_eq!(
        ReclamationObligation::after_replacement(
            artifact("1.18.31", 'a', "bin/opencode"),
            artifact("1.18.31", 'a', "other/opencode"),
            request("replace"),
        ),
        Err(ReclamationError::CurrentArtifact)
    );
}

#[test]
fn restored_obligation_validates_origin_and_activation_as_one_fact() {
    let superseded = artifact("1.17.0", 'a', "bin/opencode");
    let origin =
        ReclamationActivation::new(artifact("1.18.0", 'b', "bin/opencode"), request("a-to-b"));
    let activation =
        ReclamationActivation::new(artifact("1.19.0", 'c', "bin/opencode"), request("a-to-c"));
    let restored =
        ReclamationObligation::restore(superseded.clone(), origin.clone(), activation.clone())
            .unwrap();

    assert_eq!(restored.origin(), &origin);
    assert_eq!(restored.activation(), &activation);
    assert_eq!(
        ReclamationObligation::restore(
            superseded.clone(),
            origin.clone(),
            ReclamationActivation::new(artifact("1.19.0", 'c', "bin/opencode"), request("a-to-b"),),
        ),
        Err(ReclamationError::ConflictingActivation)
    );
    assert_eq!(
        ReclamationObligation::restore(
            superseded.clone(),
            origin,
            ReclamationActivation::new(
                artifact("1.17.0", 'a', "other/opencode"),
                request("a-to-a"),
            ),
        ),
        Err(ReclamationError::CurrentArtifact)
    );
}

#[test]
fn restored_obligation_and_triggers_cannot_cross_owner_accounts() {
    let superseded = artifact("1.17.0", 'a', "bin/opencode");
    let replacement = artifact("1.18.0", 'b', "bin/opencode");
    let origin = ReclamationActivation::new(replacement.clone(), request("a-to-b"));
    let other_owner = InstallRequest::new("other-account", "a-to-c").unwrap();
    assert_eq!(
        ReclamationObligation::restore(
            superseded.clone(),
            origin.clone(),
            ReclamationActivation::new(
                artifact("1.19.0", 'c', "bin/opencode"),
                other_owner.clone(),
            ),
        ),
        Err(ReclamationError::OwnerMismatch)
    );

    let obligation = ReclamationObligation::restore(superseded, origin.clone(), origin).unwrap();
    assert_eq!(
        ReclamationAdmission::restore(
            AgentName::parse("opencode").unwrap(),
            ReclamationOperationId::new("cross-owner-trigger").unwrap(),
            obligation,
            artifact("1.19.0", 'c', "bin/opencode"),
            ReclamationTrigger::LaterInstallation(other_owner),
        ),
        Err(ReclamationError::OwnerMismatch)
    );
}

#[test]
fn admission_retains_original_and_actual_cleanup_correlations() {
    let obligation = ReclamationObligation::after_replacement(
        artifact("1.17.0", 'a', "bin/opencode"),
        artifact("1.18.31", 'b', "bin/opencode"),
        request("replace"),
    )
    .unwrap();
    let caller = request("retry");
    let admission = ReclamationAdmission::restore(
        AgentName::parse("opencode").unwrap(),
        ReclamationOperationId::new("cleanup-1").unwrap(),
        obligation,
        artifact("1.19.0", 'c', "bin/opencode"),
        ReclamationTrigger::CallerRetry(caller.clone()),
    )
    .unwrap();

    assert_eq!(
        admission.obligation().origin().request(),
        &request("replace")
    );
    assert_eq!(admission.agent(), &AgentName::parse("opencode").unwrap());
    assert_eq!(admission.trigger().cause(), ReclamationCause::Retry);
    assert_eq!(admission.trigger().caller(), Some(&caller));
    assert_eq!(
        admission.current().version(),
        &ReleaseVersion::parse("1.19.0").unwrap()
    );
}

#[test]
fn operation_identity_is_plain_and_bounded() {
    assert_eq!(
        ReclamationOperationId::new(""),
        Err(ReclamationError::OperationId)
    );
    assert_eq!(
        ReclamationOperationId::new("bad\nidentity"),
        Err(ReclamationError::OperationId)
    );
}
