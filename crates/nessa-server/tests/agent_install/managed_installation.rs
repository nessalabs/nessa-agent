use super::{
    AdmissionResult, ManagedInstallation, ManagedInstallationError, PendingReclamation,
    ReclamationObservation, ReclamationOperation, ReclamationUpdate, ReclamationWork,
    RemovalPermit,
};
use crate::agent_install::domain::{
    AgentName, ArchiveDigest, ArchivePath, FileRole, InstallFailureEvidence, InstallFailureKind,
    InstallRequest, ReclamationAdmission, ReclamationAuditState, ReclamationEvent,
    ReclamationObligation, ReclamationOperationId, ReclamationPhysicalOutcome, ReclamationTrigger,
    ReleaseContents, ReleaseFile, ReleaseVersion, ReplacementReceipt, ReplacementSettlementState,
    RuntimeArtifact,
};

fn artifact(name: char) -> RuntimeArtifact {
    RuntimeArtifact::new(
        ReleaseVersion::parse(&format!("1.{}.0", name as u32)).unwrap(),
        ArchiveDigest::parse(&name.to_string().repeat(64)).unwrap(),
        contents("bin/opencode"),
    )
}

fn artifact_with_entry(name: char, entry: &str) -> RuntimeArtifact {
    RuntimeArtifact::new(
        ReleaseVersion::parse(&format!("1.{}.0", name as u32)).unwrap(),
        ArchiveDigest::parse(&name.to_string().repeat(64)).unwrap(),
        contents(entry),
    )
}

fn contents(entry: &str) -> ReleaseContents {
    ReleaseContents::new(vec![ReleaseFile::new(
        ArchivePath::parse(entry).unwrap(),
        FileRole::Launch,
    )])
    .unwrap()
}

fn request(id: &str) -> InstallRequest {
    InstallRequest::new("account", id).unwrap()
}

fn operation(id: &str) -> ReclamationOperationId {
    ReclamationOperationId::new(id).unwrap()
}

fn failure(detail: &str) -> InstallFailureEvidence {
    InstallFailureEvidence::capture(InstallFailureKind::Unwritable, detail)
}

fn agent(name: &str) -> AgentName {
    AgentName::parse(name).unwrap()
}

fn installation(current: char) -> ManagedInstallation {
    ManagedInstallation::new(agent("opencode"), artifact(current))
}

fn restored_admission(
    owner: &str,
    id: &str,
    obligation: ReclamationObligation,
    current: RuntimeArtifact,
) -> ReclamationAdmission {
    ReclamationAdmission::restore(
        agent(owner),
        operation(id),
        obligation,
        current,
        ReclamationTrigger::ProcessRecovery,
    )
    .unwrap()
}

fn fresh_permit(
    installation: &mut ManagedInstallation,
    replacement_request: &InstallRequest,
    operation_id: &str,
    trigger: ReclamationTrigger,
) -> RemovalPermit {
    match installation
        .admit_reclamation(replacement_request, operation(operation_id), trigger)
        .unwrap()
    {
        AdmissionResult::Fresh { permit, .. } => *permit,
        AdmissionResult::Existing(_) => panic!("expected fresh admission"),
    }
}

#[test]
fn exact_settlement_receipt_retires_only_its_completed_obligation() {
    let mut installation = installation('a');
    let replacement = request("a-to-b");
    installation
        .record_replacement(artifact('b'), replacement.clone())
        .unwrap();
    let receipt = installation
        .retain_replacement_receipt("delivery-a-to-b", &replacement)
        .unwrap();
    let permit = fresh_permit(
        &mut installation,
        &replacement,
        "cleanup-a",
        ReclamationTrigger::ReplacementFollowUp,
    );
    installation.confirm_removed(permit).unwrap();
    installation
        .acknowledge_audit(&operation("cleanup-a"))
        .unwrap();

    assert!(matches!(
        installation.work(&replacement),
        Ok(ReclamationWork::SettlementPending(_))
    ));
    assert_eq!(
        installation.acknowledge_replacement_settlement("other-delivery", &replacement),
        Err(ManagedInstallationError::DeliveryIdentity)
    );
    assert_eq!(installation.replacement_receipt(), Some(&receipt));
    let settled = installation
        .acknowledge_replacement_settlement("delivery-a-to-b", &replacement)
        .unwrap();

    assert_eq!(settled.settlement(), ReplacementSettlementState::Settled);
    assert!(installation.pending().is_empty());
    installation
        .record_replacement(artifact('c'), request("b-to-c"))
        .expect("an exact settled receipt allows the successor replacement");
}

#[test]
fn settled_receipt_keeps_acknowledged_incomplete_cleanup_pending() {
    let mut installation = installation('a');
    let replacement = request("a-to-b");
    installation
        .record_replacement(artifact('b'), replacement.clone())
        .unwrap();
    installation
        .retain_replacement_receipt("delivery-a-to-b", &replacement)
        .unwrap();
    let permit = fresh_permit(
        &mut installation,
        &replacement,
        "cleanup-a",
        ReclamationTrigger::ReplacementFollowUp,
    );
    installation.record_still_present(permit).unwrap();
    installation
        .acknowledge_audit(&operation("cleanup-a"))
        .unwrap();

    installation
        .acknowledge_replacement_settlement("delivery-a-to-b", &replacement)
        .expect("durable unsuccessful cleanup evidence does not make publication ambiguous");

    assert_eq!(installation.pending().len(), 1);
    assert_eq!(
        installation.replacement_receipt().unwrap().settlement(),
        ReplacementSettlementState::Settled
    );
}

#[test]
fn reactivation_receipt_uses_the_authoritative_original_obligation_identity() {
    let mut installation = installation('a');
    let a_to_b = request("a-to-b");
    installation
        .record_replacement(artifact('b'), a_to_b.clone())
        .unwrap();
    installation
        .retain_replacement_receipt("delivery-a-to-b", &a_to_b)
        .unwrap();
    let permit = fresh_permit(
        &mut installation,
        &a_to_b,
        "cleanup-a-deferred",
        ReclamationTrigger::ReplacementFollowUp,
    );
    installation.record_still_present(permit).unwrap();
    installation
        .acknowledge_audit(&operation("cleanup-a-deferred"))
        .unwrap();
    installation
        .acknowledge_replacement_settlement("delivery-a-to-b", &a_to_b)
        .unwrap();

    let b_to_a = request("b-to-a");
    installation
        .record_replacement(artifact('a'), b_to_a.clone())
        .unwrap();
    installation
        .retain_replacement_receipt("delivery-b-to-a", &b_to_a)
        .unwrap();
    let permit = fresh_permit(
        &mut installation,
        &b_to_a,
        "cleanup-b",
        ReclamationTrigger::ReplacementFollowUp,
    );
    installation.confirm_removed(permit).unwrap();
    installation
        .acknowledge_audit(&operation("cleanup-b"))
        .unwrap();
    installation
        .acknowledge_replacement_settlement("delivery-b-to-a", &b_to_a)
        .unwrap();

    let a_to_c = request("a-to-c");
    installation
        .record_replacement(artifact('c'), a_to_c.clone())
        .unwrap();
    let receipt = installation
        .retain_replacement_receipt("delivery-a-to-c", &a_to_c)
        .unwrap();

    assert_eq!(receipt.delivery_id(), "delivery-a-to-c");
    assert_eq!(
        receipt.obligation().origin().request().request_id(),
        "a-to-b"
    );
    assert_eq!(
        receipt.obligation().activation().request().request_id(),
        "a-to-c"
    );
    assert_eq!(receipt.obligation().superseded(), &artifact('a'));
}

#[test]
fn restore_rejects_receipt_current_and_pending_obligation_contradictions() {
    let obligation =
        ReclamationObligation::after_replacement(artifact('a'), artifact('b'), request("a-to-b"))
            .unwrap();
    let receipt = ReplacementReceipt::new("delivery-a-to-b", obligation.clone()).unwrap();
    assert!(matches!(
        ManagedInstallation::restore(
            agent("opencode"),
            artifact('c'),
            vec![PendingReclamation::restore(obligation.clone(), None, None).unwrap()],
            Some(receipt.clone()),
        ),
        Err(ManagedInstallationError::ReceiptCurrentMismatch)
    ));
    assert!(matches!(
        ManagedInstallation::restore(agent("opencode"), artifact('b'), Vec::new(), Some(receipt),),
        Err(ManagedInstallationError::ReceiptObligationMissing)
    ));
}

#[test]
fn deferred_a_to_b_survives_restart_and_later_c_with_original_correlation() {
    let mut original = installation('a');
    let replace_a = request("a-to-b");
    original
        .record_replacement(artifact('b'), replace_a.clone())
        .unwrap();
    let permit = fresh_permit(
        &mut original,
        &replace_a,
        "cleanup-a-1",
        ReclamationTrigger::ReplacementFollowUp,
    );
    assert!(matches!(
        original.work(&replace_a),
        Ok(ReclamationWork::EffectInProgress(_))
    ));
    let fresh_before = original.pending().to_vec();
    assert_eq!(
        original.record_observation(
            &operation("cleanup-a-1"),
            ReclamationObservation::StillPresent,
        ),
        Err(ManagedInstallationError::UnknownOperation)
    );
    assert_eq!(original.pending(), fresh_before);
    assert_eq!(
        original.record_replacement(artifact('c'), request("b-to-c")),
        Err(ManagedInstallationError::EffectPending)
    );

    let mut restored = ManagedInstallation::restore(
        original.agent().clone(),
        original.current().clone(),
        original.pending().to_vec(),
        None,
    )
    .unwrap();
    let restored_before = restored.pending().to_vec();
    assert_eq!(
        restored.confirm_removed(permit),
        Err(ManagedInstallationError::UnknownOperation)
    );
    assert_eq!(restored.pending(), restored_before);
    assert!(matches!(
        restored.work(&replace_a),
        Ok(ReclamationWork::Observe(admission))
            if admission.operation_id() == &operation("cleanup-a-1")
    ));
    let observed = restored
        .record_observation(
            &operation("cleanup-a-1"),
            ReclamationObservation::StillPresent,
        )
        .unwrap();
    assert_eq!(
        observed.outcome(),
        &ReclamationPhysicalOutcome::StillPresent
    );
    restored
        .acknowledge_audit(&operation("cleanup-a-1"))
        .unwrap();
    restored
        .record_replacement(artifact('c'), request("b-to-c"))
        .unwrap();

    assert_eq!(restored.pending().len(), 2);
    assert_eq!(
        restored.pending()[0]
            .obligation()
            .origin()
            .request()
            .request_id(),
        "a-to-b"
    );
    assert_eq!(
        restored.pending()[1]
            .obligation()
            .origin()
            .request()
            .request_id(),
        "b-to-c"
    );
}

#[test]
fn returning_to_a_fences_its_obligation_and_later_reactivation_preserves_origin() {
    let mut installation = installation('a');
    let replace_a = request("a-to-b");
    installation
        .record_replacement(artifact('b'), replace_a.clone())
        .unwrap();
    installation
        .record_replacement(artifact('a'), request("b-to-a"))
        .unwrap();

    assert_eq!(
        installation.work(&replace_a),
        Ok(ReclamationWork::BlockedCurrent)
    );
    assert_eq!(installation.pending().len(), 2);

    let replace_c = request("a-to-c");
    let update = installation
        .record_replacement(artifact('c'), replace_c.clone())
        .unwrap();
    let ReclamationUpdate::Reactivated {
        obligation,
        activation,
    } = update
    else {
        panic!("the preserved A obligation was not reactivated");
    };
    assert_eq!(obligation.origin().request().request_id(), "a-to-b");
    assert_eq!(obligation.origin().replacement(), &artifact('b'));
    assert_eq!(activation.request().request_id(), "a-to-c");
    assert_eq!(activation.replacement(), &artifact('c'));
    assert_eq!(obligation.activation(), &activation);
    assert_eq!(
        installation.work(&replace_a),
        Err(ManagedInstallationError::UnknownObligation)
    );
    assert_eq!(installation.work(&replace_c), Ok(ReclamationWork::Ready));
}

#[test]
fn archive_entry_difference_never_authorizes_current_physical_removal() {
    let current = artifact_with_entry('a', "bin/opencode");
    let mut installation = ManagedInstallation::new(AgentName::parse("opencode").unwrap(), current);

    assert_eq!(
        installation.record_replacement(
            artifact_with_entry('a', "other/opencode"),
            request("same-physical"),
        ),
        Err(ManagedInstallationError::CurrentArtifact)
    );
}

#[test]
fn confirmed_removal_with_audit_pending_replays_as_audit_only() {
    let mut installation = installation('a');
    let replacement = request("a-to-b");
    installation
        .record_replacement(artifact('b'), replacement.clone())
        .unwrap();
    let trigger = ReclamationTrigger::ReplacementFollowUp;
    let permit = fresh_permit(
        &mut installation,
        &replacement,
        "cleanup-a",
        trigger.clone(),
    );
    let event = installation.confirm_removed(permit).unwrap();

    assert_eq!(event.outcome(), &ReclamationPhysicalOutcome::Removed);
    assert!(matches!(
        installation
            .admit_reclamation(&replacement, operation("cleanup-a"), trigger)
            .unwrap(),
        AdmissionResult::Existing(ReclamationWork::Audit(existing))
            if *existing == event
    ));
    assert!(matches!(
        installation.work(&replacement),
        Ok(ReclamationWork::Audit(_))
    ));
}

#[test]
fn permit_from_conflicting_same_agent_snapshot_changes_neither_aggregate() {
    let shared_request = request("shared-replacement");
    let shared_trigger = ReclamationTrigger::ReplacementFollowUp;
    let mut first = installation('a');
    first
        .record_replacement(artifact('b'), shared_request.clone())
        .unwrap();
    let first_permit = fresh_permit(
        &mut first,
        &shared_request,
        "shared-operation",
        shared_trigger.clone(),
    );
    let mut second = installation('c');
    second
        .record_replacement(artifact('d'), shared_request.clone())
        .unwrap();
    let second_permit = fresh_permit(
        &mut second,
        &shared_request,
        "shared-operation",
        shared_trigger,
    );
    let first_before = first.pending().to_vec();
    let second_before = second.pending().to_vec();

    assert_eq!(
        second.confirm_removed(first_permit),
        Err(ManagedInstallationError::UnknownOperation)
    );
    assert_eq!(first.pending(), first_before);
    assert_eq!(second.pending(), second_before);
    assert!(matches!(
        first.work(&shared_request),
        Ok(ReclamationWork::EffectInProgress(_))
    ));
    assert_eq!(
        second.confirm_removed(second_permit).unwrap().outcome(),
        &ReclamationPhysicalOutcome::Removed
    );
}

#[test]
fn permit_from_another_agent_changes_neither_aggregate() {
    let shared_request = request("shared-replacement");
    let shared_trigger = ReclamationTrigger::ReplacementFollowUp;
    let mut first = ManagedInstallation::new(agent("opencode"), artifact('a'));
    first
        .record_replacement(artifact('b'), shared_request.clone())
        .unwrap();
    let first_permit = fresh_permit(
        &mut first,
        &shared_request,
        "shared-operation",
        shared_trigger.clone(),
    );
    let mut second = ManagedInstallation::new(agent("other-agent"), artifact('a'));
    second
        .record_replacement(artifact('b'), shared_request.clone())
        .unwrap();
    let second_permit = fresh_permit(
        &mut second,
        &shared_request,
        "shared-operation",
        shared_trigger,
    );
    let first_before = first.pending().to_vec();
    let second_before = second.pending().to_vec();

    assert_eq!(
        second.confirm_removed(first_permit),
        Err(ManagedInstallationError::UnknownOperation)
    );
    assert_eq!(first.pending(), first_before);
    assert_eq!(second.pending(), second_before);
    assert!(matches!(
        first.work(&shared_request),
        Ok(ReclamationWork::EffectInProgress(_))
    ));
    let event = second.confirm_removed(second_permit).unwrap();
    assert_eq!(event.admission().agent(), &agent("other-agent"));
    assert_eq!(event.outcome(), &ReclamationPhysicalOutcome::Removed);
}

#[test]
fn already_absent_observation_does_not_fabricate_confirmed_removal() {
    let mut installation = installation('a');
    let replacement = request("a-to-b");
    installation
        .record_replacement(artifact('b'), replacement.clone())
        .unwrap();
    let _permit = fresh_permit(
        &mut installation,
        &replacement,
        "cleanup-a",
        ReclamationTrigger::ReplacementFollowUp,
    );
    let mut restored = ManagedInstallation::restore(
        installation.agent().clone(),
        installation.current().clone(),
        installation.pending().to_vec(),
        None,
    )
    .unwrap();

    let event = restored
        .record_observation(
            &operation("cleanup-a"),
            ReclamationObservation::AlreadyAbsent,
        )
        .unwrap();
    assert_eq!(event.outcome(), &ReclamationPhysicalOutcome::AlreadyAbsent);
    assert_ne!(event.outcome(), &ReclamationPhysicalOutcome::Removed);
}

#[test]
fn failed_and_uncertain_attempts_require_a_new_identity_after_audit() {
    for (suffix, uncertain) in [("failed", false), ("uncertain", true)] {
        let mut installation = installation('a');
        let replacement = request(&format!("replace-{suffix}"));
        installation
            .record_replacement(artifact('b'), replacement.clone())
            .unwrap();
        let first_id = format!("cleanup-{suffix}");
        let permit = fresh_permit(
            &mut installation,
            &replacement,
            &first_id,
            ReclamationTrigger::ReplacementFollowUp,
        );
        if uncertain {
            installation
                .record_removal_sync_uncertain(permit, failure("sync"))
                .unwrap();
        } else {
            installation
                .record_removal_failure(permit, failure("remove"))
                .unwrap();
        }
        installation
            .acknowledge_audit(&operation(&first_id))
            .unwrap();

        assert_eq!(installation.work(&replacement), Ok(ReclamationWork::Ready));
        assert!(matches!(
            installation.admit_reclamation(
                &replacement,
                operation(&first_id),
                ReclamationTrigger::ProcessRecovery,
            ),
            Err(ManagedInstallationError::DuplicateOperationId)
        ));
        assert!(matches!(
            installation
                .admit_reclamation(
                    &replacement,
                    operation(&format!("{first_id}-retry")),
                    ReclamationTrigger::ProcessRecovery,
                )
                .unwrap(),
            AdmissionResult::Fresh { .. }
        ));
    }
}

#[test]
fn recorded_history_keeps_its_snapshot_when_a_is_reinstalled() {
    let mut installation = installation('a');
    let replace_a = request("a-to-b");
    installation
        .record_replacement(artifact('b'), replace_a.clone())
        .unwrap();
    let permit = fresh_permit(
        &mut installation,
        &replace_a,
        "cleanup-a",
        ReclamationTrigger::ReplacementFollowUp,
    );
    let event = installation.confirm_removed(permit).unwrap();
    installation
        .record_replacement(artifact('a'), request("b-to-a"))
        .unwrap();

    assert_eq!(event.admission().current(), &artifact('b'));
    assert_eq!(event.admission().obligation().superseded(), &artifact('a'));
    assert!(matches!(
        installation.work(&replace_a),
        Ok(ReclamationWork::Audit(existing)) if *existing == event
    ));
    assert_eq!(
        installation.record_replacement(artifact('c'), request("a-to-c")),
        Err(ManagedInstallationError::OperationInProgress)
    );
    let acknowledged = installation
        .acknowledge_audit(&operation("cleanup-a"))
        .unwrap();
    assert_eq!(acknowledged.audit(), ReclamationAuditState::Acknowledged);
    assert_eq!(installation.current(), &artifact('a'));
    let ReclamationUpdate::Created(new_a_obligation) = installation
        .record_replacement(artifact('c'), request("a-to-c"))
        .unwrap()
    else {
        panic!("acknowledged history should allow a new A obligation");
    };
    assert_eq!(new_a_obligation.origin().request().request_id(), "a-to-c");
    assert_eq!(
        event
            .admission()
            .obligation()
            .origin()
            .request()
            .request_id(),
        "a-to-b"
    );
}

#[test]
fn restore_rejects_conflicting_targets_correlations_and_progress() {
    let obligation_a = ReclamationObligation::after_replacement(
        artifact('a'),
        artifact('b'),
        request("replace-a"),
    )
    .unwrap();
    let conflicting_target = ReclamationObligation::after_replacement(
        artifact('a'),
        artifact('c'),
        request("replace-other"),
    )
    .unwrap();
    let duplicate_correlation = ReclamationObligation::after_replacement(
        artifact('d'),
        artifact('c'),
        request("replace-a"),
    )
    .unwrap();
    let pending_a = PendingReclamation::restore(obligation_a.clone(), None, None).unwrap();

    assert!(matches!(
        ManagedInstallation::restore(
            AgentName::parse("opencode").unwrap(),
            artifact('c'),
            vec![
                pending_a.clone(),
                PendingReclamation::restore(conflicting_target, None, None).unwrap(),
            ],
            None,
        ),
        Err(ManagedInstallationError::DuplicatePhysicalTarget)
    ));
    assert!(matches!(
        ManagedInstallation::restore(
            AgentName::parse("opencode").unwrap(),
            artifact('c'),
            vec![
                pending_a.clone(),
                PendingReclamation::restore(duplicate_correlation, None, None).unwrap(),
            ],
            None,
        ),
        Err(ManagedInstallationError::DuplicateCorrelation)
    ));

    let other_obligation = ReclamationObligation::after_replacement(
        artifact('d'),
        artifact('c'),
        request("replace-d"),
    )
    .unwrap();
    let mismatched_admission = ReclamationAdmission::restore(
        agent("opencode"),
        operation("cleanup"),
        other_obligation,
        artifact('c'),
        ReclamationTrigger::ProcessRecovery,
    )
    .unwrap();
    assert_eq!(
        PendingReclamation::restore(
            obligation_a.clone(),
            Some(ReclamationOperation::EffectPending(mismatched_admission)),
            None,
        ),
        Err(ManagedInstallationError::ObligationMismatch)
    );

    let admission = ReclamationAdmission::restore(
        agent("opencode"),
        operation("cleanup"),
        obligation_a.clone(),
        artifact('c'),
        ReclamationTrigger::ProcessRecovery,
    )
    .unwrap();
    let acknowledged = ReclamationEvent::restore(
        admission,
        ReclamationPhysicalOutcome::StillPresent,
        ReclamationAuditState::Acknowledged,
    );
    assert_eq!(
        PendingReclamation::restore(
            obligation_a.clone(),
            Some(ReclamationOperation::EffectRecorded(acknowledged)),
            None,
        ),
        Err(ManagedInstallationError::AcknowledgedOperationRetained)
    );

    let pending_admission = ReclamationAdmission::restore(
        agent("opencode"),
        operation("pending-current"),
        obligation_a.clone(),
        artifact('b'),
        ReclamationTrigger::ProcessRecovery,
    )
    .unwrap();
    assert!(matches!(
        ManagedInstallation::restore(
            AgentName::parse("opencode").unwrap(),
            artifact('a'),
            vec![PendingReclamation::restore(
                obligation_a.clone(),
                Some(ReclamationOperation::EffectPending(
                    pending_admission.clone()
                )),
                None,
            )
            .unwrap()],
            None,
        ),
        Err(ManagedInstallationError::AdmissionCurrentMismatch)
    ));

    let historical = ReclamationEvent::restore(
        pending_admission,
        ReclamationPhysicalOutcome::StillPresent,
        ReclamationAuditState::Pending,
    );
    let restored = ManagedInstallation::restore(
        AgentName::parse("opencode").unwrap(),
        artifact('a'),
        vec![PendingReclamation::restore(
            obligation_a,
            Some(ReclamationOperation::EffectRecorded(historical.clone())),
            None,
        )
        .unwrap()],
        None,
    )
    .unwrap();
    assert!(matches!(
        restored.work(&request("replace-a")),
        Ok(ReclamationWork::Audit(event)) if *event == historical
    ));
    assert_eq!(restored.current(), &artifact('a'));
    assert_eq!(historical.admission().current(), &artifact('b'));
}

#[test]
fn restore_enforces_owner_and_current_by_operation_phase() {
    let obligation = ReclamationObligation::after_replacement(
        artifact('a'),
        artifact('b'),
        request("replace-a"),
    )
    .unwrap();
    let effect_pending = restored_admission(
        "opencode",
        "effect-pending",
        obligation.clone(),
        artifact('b'),
    );
    assert!(matches!(
        ManagedInstallation::restore(
            agent("opencode"),
            artifact('c'),
            vec![PendingReclamation::restore(
                obligation.clone(),
                Some(ReclamationOperation::EffectPending(effect_pending)),
                None,
            )
            .unwrap()],
            None,
        ),
        Err(ManagedInstallationError::AdmissionCurrentMismatch)
    ));

    let matching_effect = restored_admission(
        "opencode",
        "matching-effect",
        obligation.clone(),
        artifact('b'),
    );
    let restored = ManagedInstallation::restore(
        agent("opencode"),
        artifact('b'),
        vec![PendingReclamation::restore(
            obligation.clone(),
            Some(ReclamationOperation::EffectPending(matching_effect)),
            None,
        )
        .unwrap()],
        None,
    )
    .unwrap();
    assert!(matches!(
        restored.work(&request("replace-a")),
        Ok(ReclamationWork::Observe(_))
    ));

    let observation_pending = restored_admission(
        "opencode",
        "observation-pending",
        obligation.clone(),
        artifact('b'),
    );
    assert!(matches!(
        ManagedInstallation::restore(
            agent("opencode"),
            artifact('c'),
            vec![PendingReclamation::restore(
                obligation.clone(),
                Some(ReclamationOperation::ObservationPending(
                    observation_pending,
                )),
                None,
            )
            .unwrap()],
            None,
        ),
        Err(ManagedInstallationError::AdmissionCurrentMismatch)
    ));

    let entry_sensitive = restored_admission(
        "opencode",
        "entry-sensitive",
        obligation.clone(),
        artifact_with_entry('b', "bin/opencode"),
    );
    assert!(matches!(
        ManagedInstallation::restore(
            agent("opencode"),
            artifact_with_entry('b', "other/opencode"),
            vec![PendingReclamation::restore(
                obligation.clone(),
                Some(ReclamationOperation::EffectPending(entry_sensitive)),
                None,
            )
            .unwrap()],
            None,
        ),
        Err(ManagedInstallationError::AdmissionCurrentMismatch)
    ));

    let matching_observation = restored_admission(
        "opencode",
        "matching-observation",
        obligation.clone(),
        artifact('b'),
    );
    let restored = ManagedInstallation::restore(
        agent("opencode"),
        artifact('b'),
        vec![PendingReclamation::restore(
            obligation.clone(),
            Some(ReclamationOperation::ObservationPending(
                matching_observation,
            )),
            None,
        )
        .unwrap()],
        None,
    )
    .unwrap();
    assert!(matches!(
        restored.work(&request("replace-a")),
        Ok(ReclamationWork::Observe(_))
    ));

    let wrong_owner_pending = restored_admission(
        "other-agent",
        "wrong-owner-pending",
        obligation.clone(),
        artifact('b'),
    );
    assert!(matches!(
        ManagedInstallation::restore(
            agent("opencode"),
            artifact('b'),
            vec![PendingReclamation::restore(
                obligation.clone(),
                Some(ReclamationOperation::EffectPending(wrong_owner_pending)),
                None,
            )
            .unwrap()],
            None,
        ),
        Err(ManagedInstallationError::AdmissionOwnerMismatch)
    ));

    let wrong_owner_observation = restored_admission(
        "other-agent",
        "wrong-owner-observation",
        obligation.clone(),
        artifact('b'),
    );
    assert!(matches!(
        ManagedInstallation::restore(
            agent("opencode"),
            artifact('b'),
            vec![PendingReclamation::restore(
                obligation.clone(),
                Some(ReclamationOperation::ObservationPending(
                    wrong_owner_observation,
                )),
                None,
            )
            .unwrap()],
            None,
        ),
        Err(ManagedInstallationError::AdmissionOwnerMismatch)
    ));

    let wrong_owner_recorded = restored_admission(
        "other-agent",
        "wrong-owner-recorded",
        obligation.clone(),
        artifact('b'),
    );
    let wrong_owner_event = ReclamationEvent::restore(
        wrong_owner_recorded,
        ReclamationPhysicalOutcome::StillPresent,
        ReclamationAuditState::Pending,
    );
    assert!(matches!(
        ManagedInstallation::restore(
            agent("opencode"),
            artifact('c'),
            vec![PendingReclamation::restore(
                obligation,
                Some(ReclamationOperation::EffectRecorded(wrong_owner_event)),
                None,
            )
            .unwrap()],
            None,
        ),
        Err(ManagedInstallationError::AdmissionOwnerMismatch)
    ));
}
