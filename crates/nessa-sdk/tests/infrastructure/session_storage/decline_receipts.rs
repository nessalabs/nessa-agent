//! Original decision receipt survives both storage representations and atomic refusal.
use super::{history_fixture, Snapshot, SnapshotRef};
use crate::application::agent_execution::{
    permissions::ActionContext,
    sessions::{
        records::continuation::Continuation, DeclineDecision, InvocationSchedulingEvent,
        SessionChange,
    },
};
use crate::domain::agent_execution::executions::{
    InvocationKind, InvocationStage, SchedulingCause,
};

#[test]
fn decline_receipt_checkpoint_semantic_and_atomic_rollback_preserve_original_owner() {
    let mut original = history_fixture(1);
    let record = &mut original.invocations[0];
    record.submission = crate::application::agent_execution::executions::SubmissionMode::Queued;
    let execution_id = record.request.execution_id.clone();
    record.scheduling = vec![
        InvocationSchedulingEvent {
            kind: InvocationKind::Queued,
            target: None,
            before: None,
            stage: InvocationStage::Queued,
            cause: SchedulingCause::Submitted,
            actor: Some(record.actor.clone()),
        },
        InvocationSchedulingEvent {
            kind: InvocationKind::Queued,
            target: None,
            before: Some(InvocationStage::Queued),
            stage: InvocationStage::Cancelled,
            cause: SchedulingCause::Interrupted,
            actor: None,
        },
    ];
    let event = InvocationSchedulingEvent {
        kind: InvocationKind::Queued,
        target: None,
        before: Some(InvocationStage::Cancelled),
        stage: InvocationStage::Cancelled,
        cause: SchedulingCause::NotResumed,
        actor: Some(ActionContext::new("original", "panel", "first-close").unwrap()),
    };
    let prepare = SessionChange::DeclinePrepared {
        execution_id: execution_id.clone(),
        decision: DeclineDecision::new(event.clone(), false),
    };
    let terminal = SessionChange::SchedulingTransition {
        execution_id: execution_id.clone(),
        event,
    };
    let ack = SessionChange::DeclineAcknowledged { execution_id };
    let mut continuation = Continuation::restore(Some(original.clone())).unwrap();
    let bytes = continuation.snapshot_bytes;
    assert!(continuation
        .apply_unit(&[prepare.clone(), ack.clone()])
        .is_err());
    assert_eq!(continuation.snapshot.as_ref(), Some(&original));
    assert_eq!(
        continuation.snapshot_bytes, bytes,
        "refused unit leaves no retained preparation"
    );
    for change in [prepare.clone(), terminal, ack.clone()] {
        let encoded = super::super::semantic::encode_batch(std::slice::from_ref(&change)).unwrap();
        let decoded =
            super::super::semantic::decode_batch(&encoded, &original.provider_context).unwrap();
        assert_eq!(decoded, vec![change]);
        continuation.apply_unit(&decoded).unwrap();
        let snapshot = continuation.snapshot.as_ref().unwrap();
        let checkpoint = serde_json::to_vec(&SnapshotRef(snapshot)).unwrap();
        let restored: Snapshot = serde_json::from_slice(&checkpoint).unwrap();
        assert_eq!(&restored.decode().unwrap(), snapshot);
        let before = snapshot.clone();
        let retained = continuation.snapshot_bytes;
        let refused = if before.invocations[0]
            .decline
            .as_ref()
            .unwrap()
            .acknowledged()
        {
            ack.clone()
        } else {
            prepare.clone()
        };
        assert!(continuation.apply_unit(&[refused]).is_err());
        assert_eq!(continuation.snapshot.as_ref(), Some(&before));
        assert_eq!(continuation.snapshot_bytes, retained);
    }
}

#[test]
fn sdk_close_acknowledgement_requires_exact_lease_operation_target_and_atomic_history() {
    use crate::application::agent_execution::agents::OwnedCloseScope;
    use crate::application::agent_execution::sessions::{CurrentLease, LeaseRecord};
    use crate::domain::agent_execution::leases::{
        AgentWork, EnvironmentRef, LeaseDeadline, LeaseEndCause, LeaseGrants, LeaseId,
        LeaseRevision, LeaseTerms, LeaseWork, SandboxProfile,
    };
    let id = |value: &str| LeaseId::new(value).unwrap();
    let issued = |lease: &str, revision: u64| LeaseRecord::Issued {
        lease: id(lease),
        revision: LeaseRevision::new(revision).unwrap(),
        terms: LeaseTerms {
            environment: EnvironmentRef::Here,
            work: LeaseWork::Agent(AgentWork::new("claude", "sonnet").unwrap()),
            sandbox: SandboxProfile::HarnessDefault,
            grants: LeaseGrants::Opening,
            deadline: LeaseDeadline::UntilEnded,
        },
        actor: ActionContext::new("person", "panel", "open").unwrap(),
    };
    let ending = |lease: &str, cause| LeaseRecord::Ending {
        lease: id(lease),
        cause,
        actor: None,
    };
    let fold = |records: &[LeaseRecord]| {
        records
            .iter()
            .try_fold(None, |current, record| {
                CurrentLease::apply(current.as_ref(), record).map(Some)
            })
            .map(|current| current.unwrap())
    };
    use crate::application::agent_execution::sessions::{
        records::continuation::Continuation, SessionChange,
    };
    use crate::domain::agent_execution::{
        sessions::SessionId,
        subagents::{AgentLifetimeId, CloseOperationId},
    };
    use crate::infrastructure::session_storage::snapshot::{
        checkpoint::{history_fixture, Snapshot, SnapshotRef},
        decode_semantic_batch, encode_semantic_batch,
    };
    let lifetime = AgentLifetimeId::new("target").unwrap();
    let operation = CloseOperationId::new("first-close").unwrap();
    let scope = OwnedCloseScope::new(
        lifetime.clone(),
        operation.clone(),
        SessionId::new("conversation").unwrap(),
    );
    let proof = LeaseRecord::SdkCloseAcknowledged {
        lease: id("lease-1"),
        lifetime,
        operation,
    };
    assert!(
        fold(&[issued("lease-1", 1), proof.clone()]).is_err(),
        "a live lease has no close acknowledgement"
    );
    let before = fold(&[
        issued("lease-1", 1),
        ending("lease-1", LeaseEndCause::Closed),
    ])
    .unwrap();
    let mut snapshot = history_fixture(0);
    snapshot.lease = Some(before);
    let mut continuation = Continuation::restore(Some(snapshot.clone())).unwrap();
    let bytes = continuation.snapshot_bytes;
    let bad = LeaseRecord::SdkCloseAcknowledged {
        lease: id("another-lease"),
        lifetime: scope.lifetime().clone(),
        operation: scope.operation().clone(),
    };
    assert!(continuation
        .apply_unit(&[
            SessionChange::Lease(proof.clone()),
            SessionChange::Lease(bad)
        ])
        .is_err());
    assert_eq!(continuation.snapshot.as_ref(), Some(&snapshot));
    assert_eq!(continuation.snapshot_bytes, bytes);
    let change = SessionChange::Lease(proof);
    let encoded = encode_semantic_batch(std::slice::from_ref(&change)).unwrap();
    let decoded = decode_semantic_batch(&encoded, &snapshot.provider_context).unwrap();
    assert_eq!(decoded, vec![change]);
    continuation.apply_unit(&decoded).unwrap();
    let acknowledged = continuation.snapshot.as_ref().unwrap();
    let current = acknowledged.lease.as_ref().unwrap();
    assert!(current.sdk_close_acknowledged(&scope));
    for wrong in [
        OwnedCloseScope::new(
            scope.lifetime().clone(),
            CloseOperationId::new("later-close").unwrap(),
            scope.session().clone(),
        ),
        OwnedCloseScope::new(
            AgentLifetimeId::new("other-target").unwrap(),
            scope.operation().clone(),
            scope.session().clone(),
        ),
    ] {
        assert!(!current.sdk_close_acknowledged(&wrong));
    }
    let encoded = serde_json::to_vec(&SnapshotRef(acknowledged)).unwrap();
    let decoded: Snapshot = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(&decoded.decode().unwrap(), acknowledged);
    assert!(continuation
        .apply_unit(&[SessionChange::Lease(
            acknowledged
                .lease
                .as_ref()
                .unwrap()
                .records()
                .last()
                .unwrap()
                .clone()
        )])
        .is_err());
}

#[test]
fn cancellation_receipt_semantic_checkpoint_and_refusal_keep_original_input() {
    use crate::application::agent_execution::sessions::CancellationDecision;
    let mut original = history_fixture(1);
    let record = &mut original.invocations[0];
    record.submission = crate::application::agent_execution::executions::SubmissionMode::Queued;
    record.scheduling = vec![InvocationSchedulingEvent {
        kind: InvocationKind::Queued,
        target: None,
        before: None,
        stage: InvocationStage::Queued,
        cause: SchedulingCause::Submitted,
        actor: Some(record.actor.clone()),
    }];
    let id = record.request.execution_id.clone();
    let event = InvocationSchedulingEvent {
        kind: InvocationKind::Queued,
        target: None,
        before: Some(InvocationStage::Queued),
        stage: InvocationStage::Cancelled,
        cause: SchedulingCause::SessionClosed,
        actor: Some(ActionContext::new("first-person", "panel", "original-cancel").unwrap()),
    };
    let prep = SessionChange::CancellationPrepared {
        execution_id: id.clone(),
        decision: CancellationDecision::new(event.clone(), false),
    };
    let ack = SessionChange::CancellationAcknowledged {
        execution_id: id.clone(),
    };
    let mut state = Continuation::restore(Some(original.clone())).unwrap();
    assert!(state.apply_unit(std::slice::from_ref(&ack)).is_err());
    assert_eq!(state.snapshot.as_ref(), Some(&original));
    for change in [
        prep.clone(),
        ack.clone(),
        SessionChange::SchedulingTransition {
            execution_id: id,
            event: event.clone(),
        },
    ] {
        let encoded = super::super::semantic::encode_batch(std::slice::from_ref(&change)).unwrap();
        let decoded =
            super::super::semantic::decode_batch(&encoded, &original.provider_context).unwrap();
        assert_eq!(decoded, vec![change]);
        state.apply_unit(&decoded).unwrap();
        let snapshot = state.snapshot.as_ref().unwrap();
        let bytes = serde_json::to_vec(&SnapshotRef(snapshot)).unwrap();
        let restored: Snapshot = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(&restored.decode().unwrap(), snapshot);
    }
    let before = state.snapshot.clone();
    let bytes = state.snapshot_bytes;
    assert!(state.apply_unit(&[ack, prep]).is_err());
    assert_eq!(state.snapshot, before);
    assert_eq!(state.snapshot_bytes, bytes);
    let receipt = state
        .snapshot
        .unwrap()
        .invocations
        .remove(0)
        .cancellation_delivery
        .unwrap();
    assert_eq!(receipt.event(), &event);
    assert!(receipt.acknowledged());
}
