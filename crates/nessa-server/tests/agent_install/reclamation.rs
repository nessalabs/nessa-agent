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

#[test]
fn removal_and_audit_failures_remain_independent_typed_facts() {
    let (terminal, _, _, _) = replacement();
    let mut lease = ScriptedLease {
        effect: RuntimeReclamationEffect::Failed(StoreFailure::Unwritable(
            "artifact directory refused removal".into(),
        )),
        ..ScriptedLease::removing()
    };
    let audit = RecordingReclamationAudit {
        records: Mutex::new(Vec::new()),
        failure: Some(ReclamationAuditFailure::new("audit journal refused".into())),
    };

    let warnings = retain_replacement_and_reclaim_with_lease(&mut lease, &audit, &terminal)
        .expect("the obligation and physical outcome are retained");

    let ReclamationWarning::Audit { event, failure } = &warnings[0] else {
        panic!("both failures remain in the audit warning")
    };
    let ReclamationPhysicalOutcome::RemovalFailed(effect_failure) = event.outcome() else {
        panic!("the physical removal failure remains typed")
    };
    assert_eq!(effect_failure.kind(), InstallFailureKind::Unwritable);
    assert_eq!(
        effect_failure.detail(),
        "artifact directory refused removal"
    );
    assert_eq!(failure.detail(), "audit journal refused");
    assert_eq!(lease.effects.len(), 1);
    assert_eq!(audit.records.lock().unwrap().as_slice(), &[event.clone()]);
}

#[test]
fn outcome_retention_failure_does_not_claim_an_audited_attempt() {
    let (terminal, _, _, _) = replacement();
    let mut lease = ScriptedLease {
        fail_at: Some(ReclamationPersistenceStage::RetainOutcome),
        ..ScriptedLease::removing()
    };
    let audit = RecordingReclamationAudit::default();

    let warnings = retain_replacement_and_reclaim_with_lease(&mut lease, &audit, &terminal)
        .expect("the cleanup obligation itself is durable");

    assert_eq!(lease.effects.len(), 1);
    assert!(audit.records.lock().unwrap().is_empty());
    assert_eq!(
        warnings,
        vec![ReclamationWarning::Persistence(
            ReclamationPersistenceFailure::new(
                ReclamationPersistenceStage::RetainOutcome,
                "injected persistence refusal".into(),
            )
        )]
    );
}

#[test]
fn substituted_reclamation_identity_facts_are_refused_before_effects() {
    let (terminal, previous, current, request) = replacement();
    let audit = RecordingReclamationAudit::default();

    let wrong_owner =
        ManagedInstallation::new(AgentName::parse("codex").unwrap(), previous.clone());
    let mut lease = ScriptedLease {
        retained: Some(wrong_owner),
        ..ScriptedLease::removing()
    };
    let failure =
        retain_replacement_and_reclaim_with_lease(&mut lease, &audit, &terminal).unwrap_err();
    assert_eq!(
        failure.stage(),
        ReclamationPersistenceStage::RetainObligation
    );
    assert_eq!(
        failure.detail(),
        "retained reclamation state belongs to another agent"
    );
    assert!(lease.effects.is_empty());

    let wrong_current = ManagedInstallation::new(terminal.agent().clone(), artifact("3.0.0", 'c'));
    let mut lease = ScriptedLease {
        retained: Some(wrong_current),
        ..ScriptedLease::removing()
    };
    let failure =
        retain_replacement_and_reclaim_with_lease(&mut lease, &audit, &terminal).unwrap_err();
    assert_eq!(
        failure.detail(),
        "retained current artifact contradicts the replacement predecessor"
    );
    assert!(lease.effects.is_empty());

    let mut wrong_request = ManagedInstallation::new(terminal.agent().clone(), previous);
    wrong_request
        .record_replacement(
            current,
            InstallRequest::new("account-a", "another-replacement").unwrap(),
        )
        .unwrap();
    let mut lease = ScriptedLease {
        retained: Some(wrong_request),
        ..ScriptedLease::removing()
    };
    let failure =
        retain_replacement_and_reclaim_with_lease(&mut lease, &audit, &terminal).unwrap_err();
    assert_eq!(
        failure.detail(),
        "retained current artifact contradicts the replacement predecessor"
    );
    assert!(lease.effects.is_empty());
    assert!(audit.records.lock().unwrap().is_empty());
    assert_eq!(terminal.request(), &request);
}
