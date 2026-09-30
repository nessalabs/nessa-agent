//! Replay undo preserves priority, position, and the admitted identity lifetime.
use super::InvocationQueue;
use crate::domain::agent_execution::executions::{
    ExecutionId, InvocationKind, QueueMutation, QueueRemovalCause, SchedulingError,
};

fn id(value: &str) -> ExecutionId {
    ExecutionId::new(value).unwrap()
}

#[test]
fn removed_steering_and_ordinary_inputs_restore_their_positions_and_history() {
    let mut queue = InvocationQueue::new(6).unwrap();
    for (name, kind) in [
        ("s1", InvocationKind::Steering),
        ("s2", InvocationKind::Steering),
        ("s3", InvocationKind::Steering),
        ("q1", InvocationKind::Queued),
        ("q2", InvocationKind::Queued),
    ] {
        queue.enqueue(id(name), kind).unwrap();
    }
    let before = queue.pending();
    let bytes = queue.retained_bytes();
    let mut undos = Vec::new();
    for name in ["s2", "q2", "s1"] {
        undos.push(
            queue
                .apply_reversible_mutation(&QueueMutation::Removed {
                    id: id(name),
                    cause: QueueRemovalCause::Withdrawn,
                })
                .unwrap(),
        );
        assert_eq!(
            queue.validate_enqueue(&id(name)),
            Err(SchedulingError::Duplicate)
        );
    }
    assert_eq!(
        queue.pending(),
        vec![
            (id("s3"), InvocationKind::Steering),
            (id("q1"), InvocationKind::Queued)
        ]
    );
    let remaining = queue.pending();
    assert!(queue
        .apply_reversible_mutation(&QueueMutation::Selected { id: id("q1") })
        .is_err());
    assert!(queue
        .apply_reversible_mutation(&QueueMutation::Removed {
            id: id("absent"),
            cause: QueueRemovalCause::Withdrawn
        })
        .is_err());
    assert_eq!(queue.pending(), remaining);
    for undo in undos.into_iter().rev() {
        queue.restore_mutation(undo);
    }
    assert_eq!(queue.pending(), before);
    assert_eq!(queue.retained_bytes(), bytes);
    assert_eq!(queue.drain(), before);
    assert_eq!(
        queue.validate_enqueue(&id("s2")),
        Err(SchedulingError::Duplicate)
    );
}
