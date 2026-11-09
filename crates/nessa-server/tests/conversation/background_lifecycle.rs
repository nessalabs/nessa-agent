use super::*;
use crate::conversation::domain::TaskCause;
fn result(clean: bool) -> TaskOutcome {
    TaskOutcome {
        cause: TaskCause::Exited,
        scope: Some(1),
        process: Some(2),
        os_pid: Some(3),
        exit_code: Some(0),
        signal: None,
        cleanup_verified: clean,
        stdout: "output".into(),
        stderr: String::new(),
        omitted_bytes: 0,
        output_complete: true,
        error: None,
    }
}
fn initiator() -> TaskInitiator {
    TaskInitiator::new("owner".into(), "desktop".into(), "stop".into()).unwrap()
}
#[test]
fn first_stop_and_terminal_evidence_are_immutable() {
    let task = TaskLifecycle::admitted(100);
    let stop = TaskStop::ConversationDeleted(initiator());
    let stopping = task.request_stop(stop.clone()).unwrap();
    let stopped_again = stopping
        .request_stop(TaskStop::GatewayStopped(initiator()))
        .unwrap();
    assert_eq!(stopping, stopped_again);
    assert_eq!(stopping.stop(), Some(&stop));
    let finished = stopping.settle(result(true), 200).unwrap();
    assert!(finished.follows(&stopping));
    assert_eq!(finished.stop(), Some(&stop));
    assert_eq!(
        finished.settle(result(false), 200),
        Err(TaskTransitionError::AlreadySettled)
    );
    assert_eq!(
        finished.settle(result(true), 201),
        Err(TaskTransitionError::AlreadySettled)
    );
    assert_eq!(finished.settle(result(true), 200).unwrap(), finished);
    assert_eq!(task.outcome(), None);
    assert_eq!(task.stop(), None);
}
#[test]
fn cleanup_and_delivery_independently_retain_capacity() {
    let admitted = TaskLifecycle::admitted(100);
    assert!(admitted.retains_capacity());
    assert_eq!(
        admitted.finish_delivery(TaskDelivery::Delivered),
        Err(TaskTransitionError::OutcomeMissing)
    );
    for clean in [false, true] {
        let terminal = admitted.settle(result(clean), 200).unwrap();
        assert!(terminal.retains_capacity());
        let delivered = terminal.finish_delivery(TaskDelivery::Delivered).unwrap();
        assert!(delivered.follows(&terminal));
        assert_eq!(delivered.retains_capacity(), !clean);
        assert_eq!(
            delivered.finish_delivery(TaskDelivery::Waiting),
            Err(TaskTransitionError::AlreadySettled)
        );
        assert_eq!(
            delivered.finish_delivery(TaskDelivery::Discarded),
            Err(TaskTransitionError::AlreadySettled)
        );
    }
}
#[test]
fn restore_and_successor_refuse_contradictory_or_rewritten_lifecycle() {
    assert_eq!(
        TaskLifecycle::restore(
            100,
            None,
            Some(result(true)),
            None,
            TaskDelivery::Waiting,
            1
        ),
        Err(TaskTransitionError::Contradiction)
    );
    assert_eq!(
        TaskLifecycle::restore(
            100,
            Some(99),
            Some(result(true)),
            None,
            TaskDelivery::Waiting,
            1
        ),
        Err(TaskTransitionError::Contradiction)
    );
    assert_eq!(
        TaskLifecycle::restore(100, None, None, None, TaskDelivery::Delivered, 1),
        Err(TaskTransitionError::Contradiction)
    );
    let task = TaskLifecycle::admitted(100);
    assert_eq!(
        task.settle(result(true), 99),
        Err(TaskTransitionError::ClockOrder)
    );
    let terminal = task.settle(result(true), 200).unwrap();
    let forged = TaskLifecycle::restore(
        101,
        Some(200),
        Some(result(true)),
        None,
        TaskDelivery::Delivered,
        2,
    )
    .unwrap();
    assert!(!forged.follows(&terminal));
}

#[test]
fn impossible_outcomes_cannot_settle_or_restore_released_capacity() {
    let mut interrupted = TaskOutcome::interrupted();
    interrupted.cleanup_verified = true;
    let mut exit = result(true);
    exit.process = None;
    let mut unowned = result(true);
    unowned.scope = None;
    for impossible in [interrupted, exit, unowned] {
        assert_eq!(
            TaskLifecycle::admitted(1).settle(impossible.clone(), 2),
            Err(TaskTransitionError::Contradiction)
        );
        assert_eq!(
            TaskLifecycle::restore(
                1,
                Some(2),
                Some(impossible),
                None,
                TaskDelivery::Delivered,
                2
            ),
            Err(TaskTransitionError::Contradiction)
        );
    }
    let interrupted = TaskLifecycle::admitted(1)
        .settle(TaskOutcome::interrupted(), 2)
        .unwrap();
    assert!(interrupted
        .finish_delivery(TaskDelivery::Delivered)
        .unwrap()
        .retains_capacity());
}
