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
