use std::sync::Mutex;

use super::*;
use crate::agent_install::application::{
    InstallDeliveryFailure, InstallationDeliverySession, PendingInstallationDelivery,
    PreparedInstallation,
};
use crate::agent_install::domain::{
    ArchiveDigest, ArchivePath, FileRole, InstallAttempt, PublicationOutcome,
    PublicationPreparation, PublicationSettlement, ReclamationAuditState,
    ReclamationPhysicalOutcome, ReleaseContents, ReleaseFile, ReleaseVersion,
};

struct ScriptedLease {
    retained: Option<ManagedInstallation>,
    fail_at: Option<ReclamationPersistenceStage>,
    fail_on_occurrence: Option<(ReclamationPersistenceStage, usize)>,
    stages: Vec<ReclamationPersistenceStage>,
    effects: Vec<(AgentName, RuntimeArtifact, RuntimeArtifact)>,
    observations: Vec<(AgentName, RuntimeArtifact, RuntimeArtifact)>,
    effect: RuntimeReclamationEffect,
}

impl ScriptedLease {
    fn removing() -> Self {
        Self {
            retained: None,
            fail_at: None,
            fail_on_occurrence: None,
            stages: Vec::new(),
            effects: Vec::new(),
            observations: Vec::new(),
            effect: RuntimeReclamationEffect::Removed,
        }
    }
}

impl PublicationLease for ScriptedLease {
    fn load_reclamation(
        &mut self,
    ) -> Result<Option<ManagedInstallation>, ReclamationPersistenceFailure> {
        Ok(self.retained.as_ref().map(|installation| {
            ManagedInstallation::restore(
                installation.agent().clone(),
                installation.current().clone(),
                installation.pending().to_vec(),
                installation.replacement_receipt().cloned(),
            )
            .expect("retained test state remains valid")
        }))
    }

    fn retain_reclamation(
        &mut self,
        installation: &ManagedInstallation,
        stage: ReclamationPersistenceStage,
    ) -> Result<(), ReclamationPersistenceFailure> {
        self.stages.push(stage);
        let occurrence = self
            .stages
            .iter()
            .filter(|recorded| **recorded == stage)
            .count();
        if self.fail_at == Some(stage) || self.fail_on_occurrence == Some((stage, occurrence)) {
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
                installation.replacement_receipt().cloned(),
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
        self.observations
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

    fn event_for(
        &self,
        operation_id: &ReclamationOperationId,
    ) -> Result<Option<ReclamationEvent>, ReclamationAuditFailure> {
        Ok(self
            .records
            .lock()
            .unwrap()
            .iter()
            .find(|event| event.admission().operation_id() == operation_id)
            .cloned())
    }
}

struct RefusingUnretainedAudit;

impl ReclamationAudit for RefusingUnretainedAudit {
    fn record(&self, _: &ReclamationEvent) -> Result<(), ReclamationAuditFailure> {
        Err(ReclamationAuditFailure::new(
            "audit failed before durable publication".into(),
        ))
    }

    fn event_for(
        &self,
        _: &ReclamationOperationId,
    ) -> Result<Option<ReclamationEvent>, ReclamationAuditFailure> {
        Ok(None)
    }
}

struct LookupFailureAudit;

impl ReclamationAudit for LookupFailureAudit {
    fn record(&self, _: &ReclamationEvent) -> Result<(), ReclamationAuditFailure> {
        Ok(())
    }

    fn event_for(
        &self,
        _: &ReclamationOperationId,
    ) -> Result<Option<ReclamationEvent>, ReclamationAuditFailure> {
        Err(ReclamationAuditFailure::new(
            "audit lookup is unavailable".into(),
        ))
    }
}

fn operation_ids() -> &'static dyn ReclamationOperationIds {
    crate::agent_install_test_support::reclamation_operation_ids()
}

struct RefusingOperationIds;

impl ReclamationOperationIds for RefusingOperationIds {
    fn next(&self) -> Result<ReclamationOperationId, ReclamationPersistenceFailure> {
        Err(ReclamationPersistenceFailure::new(
            ReclamationPersistenceStage::RetainAdmission,
            "operation identity source unavailable".into(),
        ))
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
    PreparedInstallation,
) {
    let agent = AgentName::parse("opencode").unwrap();
    let previous = artifact("1.0.0", 'a');
    let current = artifact("2.0.0", 'b');
    let request = InstallRequest::new("account-a", "replace-a-with-b").unwrap();
    let (mut attempt, _) = InstallAttempt::start(agent, current.clone(), request.clone());
    let verified = attempt.verified().unwrap();
    assert_eq!(verified.target(), &current);
    assert_eq!(verified.request(), &request);
    let preparation = PublicationPreparation::new(verified).unwrap();
    let prepared = PreparedInstallation::new("replacement-delivery".into(), preparation);
    let terminal = attempt.replaced(previous.clone()).unwrap();
    (terminal, previous, current, request, prepared)
}

fn publication_preparation(
    target: RuntimeArtifact,
    request: InstallRequest,
) -> PublicationPreparation {
    let (mut attempt, _) =
        InstallAttempt::start(AgentName::parse("opencode").unwrap(), target, request);
    PublicationPreparation::new(attempt.verified().unwrap()).unwrap()
}

struct SettledDelivery {
    prepared: PreparedInstallation,
    settlement: PublicationSettlement,
}

impl InstallationDeliverySession for SettledDelivery {
    fn pending(&mut self) -> Result<Option<PendingInstallationDelivery>, InstallDeliveryFailure> {
        panic!("reclamation recovery only queries exact settled evidence")
    }

    fn settled(
        &mut self,
        delivery_id: &str,
    ) -> Result<Option<(PreparedInstallation, PublicationSettlement)>, InstallDeliveryFailure> {
        Ok((delivery_id == self.prepared.record_id())
            .then(|| (self.prepared.clone(), self.settlement.clone())))
    }

    fn prepare(
        &mut self,
        _: PublicationPreparation,
    ) -> Result<PreparedInstallation, InstallDeliveryFailure> {
        panic!("reclamation recovery never prepares publication delivery")
    }

    fn retain_outcome(
        &mut self,
        _: &PreparedInstallation,
        _: &PublicationOutcome,
    ) -> Result<(), InstallDeliveryFailure> {
        panic!("reclamation recovery never retains publication outcomes")
    }

    fn settle(
        &mut self,
        _: &PreparedInstallation,
        _: &PublicationSettlement,
    ) -> Result<(), InstallDeliveryFailure> {
        panic!("reclamation recovery never settles publication delivery")
    }
}

fn settled_delivery(
    prepared: &PreparedInstallation,
    terminal: &InstallTransition,
) -> SettledDelivery {
    let outcome = PublicationOutcome::terminal(prepared.preparation(), terminal.clone()).unwrap();
    SettledDelivery {
        prepared: prepared.clone(),
        settlement: PublicationSettlement::new(prepared.preparation(), outcome).unwrap(),
    }
}

#[test]
fn replacement_retains_exact_obligation_before_effect_and_audits_exact_outcome() {
    let (terminal, previous, current, request, prepared) = replacement();
    let mut lease = ScriptedLease::removing();
    let audit = RecordingReclamationAudit::default();

    let warnings = retain_replacement_and_reclaim_with_lease(
        &mut lease,
        &audit,
        operation_ids(),
        &terminal,
        &prepared,
    )
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
    let (terminal, _, _, _, prepared) = replacement();
    let mut lease = ScriptedLease {
        fail_at: Some(ReclamationPersistenceStage::RetainAdmission),
        ..ScriptedLease::removing()
    };
    let audit = RecordingReclamationAudit::default();

    let warnings = retain_replacement_and_reclaim_with_lease(
        &mut lease,
        &audit,
        operation_ids(),
        &terminal,
        &prepared,
    )
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
fn operation_identity_failure_prevents_removal_and_audit() {
    let (terminal, _, _, _, prepared) = replacement();
    let mut lease = ScriptedLease::removing();
    let audit = RecordingReclamationAudit::default();

    let warnings = retain_replacement_and_reclaim_with_lease(
        &mut lease,
        &audit,
        &RefusingOperationIds,
        &terminal,
        &prepared,
    )
    .unwrap();

    assert_eq!(
        warnings,
        vec![ReclamationWarning::Persistence(
            ReclamationPersistenceFailure::new(
                ReclamationPersistenceStage::RetainAdmission,
                "operation identity source unavailable".into(),
            )
        )]
    );
    assert!(lease.effects.is_empty());
    assert!(audit.records.lock().unwrap().is_empty());
}

#[test]
fn audit_failure_retries_only_the_retained_event_after_restart() {
    let (terminal, _, _, request, prepared) = replacement();
    let mut lease = ScriptedLease::removing();
    let refusing = RecordingReclamationAudit {
        records: Mutex::new(Vec::new()),
        failure: Some(ReclamationAuditFailure::new("audit unavailable".into())),
    };

    let warnings = retain_replacement_and_reclaim_with_lease(
        &mut lease,
        &refusing,
        operation_ids(),
        &terminal,
        &prepared,
    )
    .expect("the cleanup evidence is retained");
    let ReclamationWarning::Audit { event, failure } = &warnings[0] else {
        panic!("the audit refusal stays typed")
    };
    assert_eq!(failure.detail(), "audit unavailable");
    assert_eq!(event.outcome(), &ReclamationPhysicalOutcome::Removed);
    assert_eq!(lease.effects.len(), 1);

    let accepting = RecordingReclamationAudit::default();
    let mut delivery = settled_delivery(&prepared, &terminal);
    let warnings = recover_reclamation(
        &mut lease,
        &accepting,
        operation_ids(),
        &mut delivery,
        terminal.agent(),
        &request,
    )
    .expect("the exact settlement agrees with the retained receipt");

    assert!(warnings.is_empty());
    assert_eq!(lease.effects.len(), 1, "recovery must not repeat removal");
    assert_eq!(
        accepting.records.lock().unwrap().as_slice(),
        std::slice::from_ref(event)
    );
    assert!(lease.retained.as_ref().unwrap().pending().is_empty());
}

#[test]
fn removal_and_audit_failures_remain_independent_typed_facts() {
    let (terminal, _, _, _, prepared) = replacement();
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

    let warnings = retain_replacement_and_reclaim_with_lease(
        &mut lease,
        &audit,
        operation_ids(),
        &terminal,
        &prepared,
    )
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
    assert_eq!(
        audit.records.lock().unwrap().as_slice(),
        std::slice::from_ref(event)
    );
}

#[test]
fn outcome_retention_failure_still_attempts_exact_independent_audit() {
    let (terminal, _, _, _, prepared) = replacement();
    let mut lease = ScriptedLease {
        fail_at: Some(ReclamationPersistenceStage::RetainOutcome),
        ..ScriptedLease::removing()
    };
    let audit = RecordingReclamationAudit::default();

    let warnings = retain_replacement_and_reclaim_with_lease(
        &mut lease,
        &audit,
        operation_ids(),
        &terminal,
        &prepared,
    )
    .expect("the cleanup obligation itself is durable");

    assert_eq!(lease.effects.len(), 1);
    let recorded = audit.records.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].outcome(), &ReclamationPhysicalOutcome::Removed);
    assert_eq!(
        warnings,
        vec![ReclamationWarning::OutcomePersistence {
            event: recorded[0].clone(),
            failure: ReclamationPersistenceFailure::new(
                ReclamationPersistenceStage::RetainOutcome,
                "injected persistence refusal".into(),
            ),
        }]
    );
}

#[test]
fn outcome_retention_and_audit_failures_preserve_the_same_exact_event() {
    let (terminal, _, _, _, prepared) = replacement();
    let mut lease = ScriptedLease {
        fail_at: Some(ReclamationPersistenceStage::RetainOutcome),
        ..ScriptedLease::removing()
    };
    let audit = RecordingReclamationAudit {
        records: Mutex::new(Vec::new()),
        failure: Some(ReclamationAuditFailure::new("audit journal refused".into())),
    };

    let warnings = retain_replacement_and_reclaim_with_lease(
        &mut lease,
        &audit,
        operation_ids(),
        &terminal,
        &prepared,
    )
    .expect("the obligation was retained before the effect");

    let [ReclamationWarning::OutcomePersistence {
        event: retained_event,
        failure: retained_failure,
    }, ReclamationWarning::Audit {
        event: audited_event,
        failure: audit_failure,
    }] = warnings.as_slice()
    else {
        panic!("both independent sinks must report their exact failures")
    };
    assert_eq!(retained_event, audited_event);
    assert_eq!(
        retained_failure.stage(),
        ReclamationPersistenceStage::RetainOutcome
    );
    assert_eq!(audit_failure.detail(), "audit journal refused");
    assert_eq!(lease.effects.len(), 1);
}

#[test]
fn both_unretained_sinks_recover_by_observation_without_repeating_removal() {
    let (terminal, _, _, request, prepared) = replacement();
    let mut lease = ScriptedLease {
        fail_at: Some(ReclamationPersistenceStage::RetainOutcome),
        ..ScriptedLease::removing()
    };
    let warnings = retain_replacement_and_reclaim_with_lease(
        &mut lease,
        &RefusingUnretainedAudit,
        operation_ids(),
        &terminal,
        &prepared,
    )
    .unwrap();
    assert!(matches!(
        warnings.as_slice(),
        [
            ReclamationWarning::OutcomePersistence { .. },
            ReclamationWarning::Audit { .. }
        ]
    ));
    assert_eq!(lease.effects.len(), 1);
    assert!(lease.observations.is_empty());

    lease.fail_at = None;
    let accepting = RecordingReclamationAudit::default();
    let mut delivery = settled_delivery(&prepared, &terminal);
    let warnings = recover_reclamation(
        &mut lease,
        &accepting,
        operation_ids(),
        &mut delivery,
        terminal.agent(),
        &request,
    )
    .unwrap();

    assert!(warnings.is_empty());
    assert_eq!(lease.effects.len(), 1);
    assert_eq!(lease.observations.len(), 1);
    let records = accepting.records.lock().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].outcome(),
        &ReclamationPhysicalOutcome::AlreadyAbsent
    );
}

#[test]
fn audit_lookup_failure_is_not_treated_as_authoritative_absence() {
    let (terminal, _, _, request, prepared) = replacement();
    let mut lease = ScriptedLease {
        fail_at: Some(ReclamationPersistenceStage::RetainOutcome),
        ..ScriptedLease::removing()
    };
    retain_replacement_and_reclaim_with_lease(
        &mut lease,
        &RefusingUnretainedAudit,
        operation_ids(),
        &terminal,
        &prepared,
    )
    .unwrap();
    lease.fail_at = None;
    let mut delivery = settled_delivery(&prepared, &terminal);

    let warnings = recover_reclamation(
        &mut lease,
        &LookupFailureAudit,
        operation_ids(),
        &mut delivery,
        terminal.agent(),
        &request,
    )
    .expect_err("unknown audit state must block settlement acknowledgement");

    assert!(matches!(
        warnings.first(),
        Some(ReclamationWarning::AuditLookup { failure, .. })
            if failure.detail() == "audit lookup is unavailable"
    ));
    assert!(lease.observations.is_empty());
    assert_eq!(lease.effects.len(), 1);
}

#[test]
fn fresh_recovery_uses_exact_audit_event_before_any_physical_observation() {
    let (terminal, _, _, request, prepared) = replacement();
    let mut lease = ScriptedLease {
        fail_at: Some(ReclamationPersistenceStage::RetainOutcome),
        ..ScriptedLease::removing()
    };
    let audit = RecordingReclamationAudit::default();

    let warnings = retain_replacement_and_reclaim_with_lease(
        &mut lease,
        &audit,
        operation_ids(),
        &terminal,
        &prepared,
    )
    .expect("the original obligation was retained");
    assert!(matches!(
        warnings.as_slice(),
        [ReclamationWarning::OutcomePersistence { .. }]
    ));
    assert_eq!(lease.effects.len(), 1);

    lease.fail_at = None;
    let mut delivery = settled_delivery(&prepared, &terminal);
    let warnings = recover_reclamation(
        &mut lease,
        &audit,
        operation_ids(),
        &mut delivery,
        terminal.agent(),
        &request,
    )
    .expect("the exact settlement agrees with the retained receipt");

    assert!(warnings.is_empty());
    assert_eq!(
        lease.effects.len(),
        1,
        "an exact immutable audit event prevents guessed observation or repeated removal"
    );
    assert!(lease.observations.is_empty());
}

#[test]
fn settlement_mark_failure_recovers_from_exact_delivery_without_repeating_removal() {
    let (terminal, _, _, request, prepared) = replacement();
    let mut lease = ScriptedLease::removing();
    let audit = RecordingReclamationAudit::default();
    let warnings = retain_replacement_and_reclaim_with_lease(
        &mut lease,
        &audit,
        operation_ids(),
        &terminal,
        &prepared,
    )
    .unwrap();
    assert!(warnings.is_empty());
    assert_eq!(lease.effects.len(), 1);
    let mut delivery = settled_delivery(&prepared, &terminal);
    lease.fail_on_occurrence = Some((ReclamationPersistenceStage::RetainAuditAcknowledgement, 2));

    let failure = acknowledge_replacement_settlement(&mut lease, &prepared, &terminal)
        .expect_err("the settled receipt acknowledgement is not durable");

    assert_eq!(
        failure.stage(),
        ReclamationPersistenceStage::RetainAuditAcknowledgement
    );
    assert_eq!(
        lease
            .retained
            .as_ref()
            .unwrap()
            .replacement_receipt()
            .unwrap()
            .settlement(),
        ReplacementSettlementState::Pending
    );
    lease.fail_on_occurrence = None;
    let warnings = recover_reclamation(
        &mut lease,
        &audit,
        operation_ids(),
        &mut delivery,
        terminal.agent(),
        &request,
    )
    .expect("fresh recovery re-acknowledges the exact settled delivery");

    assert!(warnings.is_empty());
    assert_eq!(lease.effects.len(), 1, "recovery must not repeat removal");
    let recovered = lease.retained.as_ref().unwrap();
    assert!(recovered.pending().is_empty());
    assert_eq!(
        recovered.replacement_receipt().unwrap().settlement(),
        ReplacementSettlementState::Settled
    );
}

#[test]
fn missing_or_contradictory_settlement_blocks_successor_without_repeating_effects() {
    let (terminal, _, _, request, prepared) = replacement();
    let audit = RecordingReclamationAudit::default();
    for contradictory in [false, true] {
        let mut lease = ScriptedLease::removing();
        retain_replacement_and_reclaim_with_lease(
            &mut lease,
            &audit,
            operation_ids(),
            &terminal,
            &prepared,
        )
        .unwrap();
        let mut delivery = if contradictory {
            let other_request = InstallRequest::new("account-a", "other-replacement").unwrap();
            let other_preparation = publication_preparation(artifact("3.0.0", 'c'), other_request);
            let other_terminal = InstallTransition::restore(
                terminal.agent().clone(),
                other_preparation.verified().target().clone(),
                other_preparation.verified().request().clone(),
                crate::agent_install::domain::InstallTransitionFacts::Replaced(
                    terminal.previous().unwrap().clone(),
                ),
            )
            .unwrap();
            let other_outcome =
                PublicationOutcome::terminal(&other_preparation, other_terminal).unwrap();
            SettledDelivery {
                prepared: PreparedInstallation::new(
                    prepared.record_id().to_owned(),
                    other_preparation.clone(),
                ),
                settlement: PublicationSettlement::new(&other_preparation, other_outcome).unwrap(),
            }
        } else {
            let mut absent = settled_delivery(&prepared, &terminal);
            absent.prepared = PreparedInstallation::new(
                "another-delivery".into(),
                absent.prepared.preparation().clone(),
            );
            absent
        };

        let warnings = recover_reclamation(
            &mut lease,
            &audit,
            operation_ids(),
            &mut delivery,
            terminal.agent(),
            &request,
        )
        .expect_err("successor admission requires exact settled replacement evidence");

        let ReclamationWarning::Persistence(failure) = warnings.last().unwrap() else {
            panic!("settlement disagreement remains a typed persistence blocker")
        };
        assert_eq!(failure.stage(), ReclamationPersistenceStage::Read);
        assert!(failure.detail().contains(if contradictory {
            "contradicts"
        } else {
            "missing"
        }));
        assert_eq!(lease.effects.len(), 1, "recovery must not repeat removal");
    }
}

#[test]
fn substituted_reclamation_identity_facts_are_refused_before_effects() {
    let (terminal, previous, current, request, prepared) = replacement();
    let audit = RecordingReclamationAudit::default();

    let wrong_owner =
        ManagedInstallation::new(AgentName::parse("codex").unwrap(), previous.clone());
    let mut lease = ScriptedLease {
        retained: Some(wrong_owner),
        ..ScriptedLease::removing()
    };
    let failure = retain_replacement_and_reclaim_with_lease(
        &mut lease,
        &audit,
        operation_ids(),
        &terminal,
        &prepared,
    )
    .unwrap_err();
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
    let failure = retain_replacement_and_reclaim_with_lease(
        &mut lease,
        &audit,
        operation_ids(),
        &terminal,
        &prepared,
    )
    .unwrap_err();
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
    let failure = retain_replacement_and_reclaim_with_lease(
        &mut lease,
        &audit,
        operation_ids(),
        &terminal,
        &prepared,
    )
    .unwrap_err();
    assert_eq!(
        failure.detail(),
        "retained current artifact contradicts the replacement predecessor"
    );
    assert!(lease.effects.is_empty());
    assert!(audit.records.lock().unwrap().is_empty());
    assert_eq!(terminal.request(), &request);

    let mut wrong_account =
        ManagedInstallation::new(terminal.agent().clone(), artifact("1.0.0", 'a'));
    wrong_account
        .record_replacement(
            artifact("2.0.0", 'b'),
            InstallRequest::new("other-account", "replace-a-with-b").unwrap(),
        )
        .unwrap();
    let mut lease = ScriptedLease {
        retained: Some(wrong_account),
        ..ScriptedLease::removing()
    };
    let failure = retain_replacement_and_reclaim_with_lease(
        &mut lease,
        &audit,
        operation_ids(),
        &terminal,
        &prepared,
    )
    .unwrap_err();
    assert_eq!(
        failure.detail(),
        "retained reclamation state belongs to another owner account"
    );
    assert!(lease.effects.is_empty());
}

#[test]
fn recovered_substituted_agent_or_owner_is_refused_before_lookup_and_effects() {
    let (terminal, previous, current, request, prepared) = replacement();
    let audit = RecordingReclamationAudit::default();
    for wrong_owner in [false, true] {
        let retained_agent = if wrong_owner {
            terminal.agent().clone()
        } else {
            AgentName::parse("codex").unwrap()
        };
        let retained_request = if wrong_owner {
            InstallRequest::new("other-account", "replace-a-with-b").unwrap()
        } else {
            request.clone()
        };
        let mut installation = ManagedInstallation::new(retained_agent, previous.clone());
        installation
            .record_replacement(current.clone(), retained_request)
            .unwrap();
        let mut lease = ScriptedLease {
            retained: Some(installation),
            ..ScriptedLease::removing()
        };
        let mut delivery = settled_delivery(&prepared, &terminal);

        let warnings = recover_reclamation(
            &mut lease,
            &audit,
            operation_ids(),
            &mut delivery,
            terminal.agent(),
            &request,
        )
        .expect_err("a substituted aggregate cannot authorize recovery");

        let [ReclamationWarning::Persistence(failure)] = warnings.as_slice() else {
            panic!("substituted authority remains a typed read failure")
        };
        assert_eq!(failure.stage(), ReclamationPersistenceStage::Read);
        assert_eq!(
            failure.detail(),
            "retained reclamation state disagrees with the selected agent or owner account"
        );
        assert!(lease.effects.is_empty());
        assert!(audit.records.lock().unwrap().is_empty());
    }
}
