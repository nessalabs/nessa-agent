use super::*;
use crate::agent_install::domain::{
    ArchiveDigest, ArchivePath, FileRole, InstallAttempt, ReclamationAuditState, ReleaseContents,
    ReleaseFile, ReleaseVersion,
};
use std::sync::Mutex;

struct ScriptedLease {
    retained: Option<ManagedInstallation>,
    fail_at: Option<ReclamationPersistenceStage>,
    stages: Vec<ReclamationPersistenceStage>,
    effects: Vec<(AgentName, RuntimeArtifact, RuntimeArtifact)>,
    effect: RuntimeReclamationEffect,
}

impl ScriptedLease {
    fn removing() -> Self {
        Self {
            retained: None,
            fail_at: None,
            stages: Vec::new(),
            effects: Vec::new(),
            effect: RuntimeReclamationEffect::Removed,
        }
    }
}

impl PublicationLease for ScriptedLease {
    fn load_reclamation(
        &mut self,
    ) -> Result<Option<ManagedInstallation>, ReclamationPersistenceFailure> {
        Ok(self.retained.take())
    }

    fn retain_reclamation(
        &mut self,
        installation: &ManagedInstallation,
        stage: ReclamationPersistenceStage,
    ) -> Result<(), ReclamationPersistenceFailure> {
        self.stages.push(stage);
        if self.fail_at == Some(stage) {
            return Err(ReclamationPersistenceFailure::new(
                stage,
                "injected persistence refusal".into(),
            ));
        }
        self.retained = Some(
            ManagedInstallation::restore(
                installation.agent().clone(),
                installation.current().clone(),
                installation.pending().to_vec(),
            )
            .expect("application only retains valid aggregate state"),
        );
        Ok(())
    }

    fn remove_superseded(
        &mut self,
        agent: &AgentName,
        current: &RuntimeArtifact,
        superseded: &RuntimeArtifact,
    ) -> RuntimeReclamationEffect {
        self.effects
            .push((agent.clone(), current.clone(), superseded.clone()));
        self.effect.clone()
    }

    fn observe_superseded(
        &mut self,
        agent: &AgentName,
        current: &RuntimeArtifact,
        superseded: &RuntimeArtifact,
    ) -> RuntimeReclamationEffect {
        self.effects
            .push((agent.clone(), current.clone(), superseded.clone()));
        self.effect.clone()
    }
}

#[derive(Default)]
struct RecordingReclamationAudit {
    records: Mutex<Vec<ReclamationEvent>>,
    failure: Option<ReclamationAuditFailure>,
}

impl ReclamationAudit for RecordingReclamationAudit {
    fn record(&self, event: &ReclamationEvent) -> Result<(), ReclamationAuditFailure> {
        self.records.lock().unwrap().push(event.clone());
        self.failure.clone().map_or(Ok(()), Err)
    }
}

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

fn replacement() -> (
    InstallTransition,
    RuntimeArtifact,
    RuntimeArtifact,
    InstallRequest,
) {
    let agent = AgentName::parse("opencode").unwrap();
    let previous = artifact("1.0.0", 'a');
    let current = artifact("2.0.0", 'b');
    let request = InstallRequest::new("account-a", "replace-a-with-b").unwrap();
    let (mut attempt, _) = InstallAttempt::start(agent, current.clone(), request.clone());
    attempt.verified().unwrap();
    let terminal = attempt.replaced(previous.clone()).unwrap();
    (terminal, previous, current, request)
}

#[test]
fn replacement_retains_exact_obligation_before_effect_and_audits_exact_outcome() {
    let (terminal, previous, current, request) = replacement();
    let mut lease = ScriptedLease::removing();
    let audit = RecordingReclamationAudit::default();

    let warnings = retain_replacement_and_reclaim_with_lease(&mut lease, &audit, &terminal)
        .expect("the cleanup obligation is durable");

    assert!(warnings.is_empty());
    assert_eq!(
        lease.stages,
        vec![
            ReclamationPersistenceStage::RetainObligation,
            ReclamationPersistenceStage::RetainAdmission,
            ReclamationPersistenceStage::RetainOutcome,
            ReclamationPersistenceStage::RetainAuditAcknowledgement,
        ]
    );
    assert_eq!(
        lease.effects,
        vec![(terminal.agent().clone(), current.clone(), previous.clone())]
    );
    let records = audit.records.lock().unwrap();
    let event = records.first().expect("one attempt audit");
    assert_eq!(event.audit(), ReclamationAuditState::Pending);
    assert_eq!(event.outcome(), &ReclamationPhysicalOutcome::Removed);
    assert_eq!(event.admission().agent(), terminal.agent());
    assert_eq!(event.admission().current(), &current);
    assert_eq!(event.admission().obligation().superseded(), &previous);
    assert_eq!(event.admission().obligation().origin().request(), &request);
    assert_eq!(
        event.admission().trigger(),
        &ReclamationTrigger::ReplacementFollowUp
    );
}

#[test]
fn admission_retention_failure_prevents_removal_and_audit() {
    let (terminal, _, _, _) = replacement();
    let mut lease = ScriptedLease {
        fail_at: Some(ReclamationPersistenceStage::RetainAdmission),
        ..ScriptedLease::removing()
    };
    let audit = RecordingReclamationAudit::default();

    let warnings = retain_replacement_and_reclaim_with_lease(&mut lease, &audit, &terminal)
        .expect("the obligation itself was retained");

    assert_eq!(
        warnings,
        vec![ReclamationWarning::Persistence(
            ReclamationPersistenceFailure::new(
                ReclamationPersistenceStage::RetainAdmission,
                "injected persistence refusal".into(),
            )
        )]
    );
    assert!(lease.effects.is_empty());
    assert!(audit.records.lock().unwrap().is_empty());
}

#[test]
fn audit_failure_retries_only_the_retained_event_after_restart() {
    let (terminal, _, _, request) = replacement();
    let mut lease = ScriptedLease::removing();
    let refusing = RecordingReclamationAudit {
        records: Mutex::new(Vec::new()),
        failure: Some(ReclamationAuditFailure::new("audit unavailable".into())),
    };

    let warnings = retain_replacement_and_reclaim_with_lease(&mut lease, &refusing, &terminal)
        .expect("the cleanup evidence is retained");
    let ReclamationWarning::Audit { event, failure } = &warnings[0] else {
        panic!("the audit refusal stays typed")
    };
    assert_eq!(failure.detail(), "audit unavailable");
    assert_eq!(event.outcome(), &ReclamationPhysicalOutcome::Removed);
    assert_eq!(lease.effects.len(), 1);

    let accepting = RecordingReclamationAudit::default();
    let warnings = recover_reclamation(&mut lease, &accepting, &request);

    assert!(warnings.is_empty());
    assert_eq!(lease.effects.len(), 1, "recovery must not repeat removal");
    assert_eq!(
        accepting.records.lock().unwrap().as_slice(),
        &[event.clone()]
    );
    assert!(lease.retained.as_ref().unwrap().pending().is_empty());
}
