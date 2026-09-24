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

#[test]
fn replay_accepts_only_the_exact_durable_reclamation_event() {
    let (_temporary, root) = audit_root();
    let audit = DurableReclamationAudit::new(&root).unwrap();
    let removed = event(ReclamationPhysicalOutcome::Removed);

    audit.record(&removed).unwrap();
    let path = root.join(DurableReclamationAudit::record_path(&removed));
    let original = std::fs::read(&path).unwrap();
    audit.record(&removed).unwrap();

    assert_eq!(std::fs::read(&path).unwrap(), original);
}

#[test]
fn conflicting_facts_for_one_operation_are_refused_without_replacing_the_original() {
    let (_temporary, root) = audit_root();
    let audit = DurableReclamationAudit::new(&root).unwrap();
    let removed = event(ReclamationPhysicalOutcome::Removed);
    let still_present = event(ReclamationPhysicalOutcome::StillPresent);
    audit.record(&removed).unwrap();
    let path = root.join(DurableReclamationAudit::record_path(&removed));
    let original = std::fs::read(&path).unwrap();

    let failure = audit.record(&still_present).unwrap_err();

    assert_eq!(
        failure.detail(),
        "reclamation operation identity has conflicting audit facts"
    );
    assert_eq!(std::fs::read(&path).unwrap(), original);
}
