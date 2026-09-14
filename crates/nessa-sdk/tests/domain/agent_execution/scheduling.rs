use nessa_sdk::domain::agent_execution::executions::{
    ExecutionId, InvocationKind, InvocationQueue, InvocationStage, SchedulingCause,
    SchedulingError, SchedulingInitiator, SchedulingTransition, SchedulingTransitionError,
};

fn id(value: &str) -> ExecutionId {
    ExecutionId::new(value).unwrap()
}

#[test]
fn scheduling_requires_positive_capacity_and_handles_empty_queue() {
    assert!(matches!(
        InvocationQueue::new(0),
        Err(SchedulingError::InvalidCapacity)
    ));
    let mut queue = InvocationQueue::new(1).unwrap();
    assert!(queue.is_empty());
    assert_eq!(queue.len(), 0);
    assert_eq!(queue.pop_next(), None);
    assert!(queue.drain().is_empty());
}

#[test]
fn scheduling_prioritizes_steering_and_preserves_fifo_within_each_kind() {
    let mut queue = InvocationQueue::new(6).unwrap();
    for (name, kind) in [
        ("queued-1", InvocationKind::Queued),
        ("steering-1", InvocationKind::Steering),
        ("queued-2", InvocationKind::Queued),
        ("steering-2", InvocationKind::Steering),
    ] {
        queue.enqueue(id(name), kind).unwrap();
    }
    assert_eq!(queue.len(), 4);
    assert_eq!(
        queue.pop_next(),
        Some((id("steering-1"), InvocationKind::Steering))
    );
    queue
        .enqueue(id("steering-3"), InvocationKind::Steering)
        .unwrap();
    assert_eq!(
        queue.drain(),
        vec![
            (id("steering-2"), InvocationKind::Steering),
            (id("steering-3"), InvocationKind::Steering),
            (id("queued-1"), InvocationKind::Queued),
            (id("queued-2"), InvocationKind::Queued),
        ]
    );
    assert!(queue.is_empty());
}

#[test]
fn scheduling_full_rejections_do_not_consume_identity_or_change_pending_work() {
    let mut queue = InvocationQueue::new(1).unwrap();
    queue.enqueue(id("first"), InvocationKind::Queued).unwrap();
    assert_eq!(
        queue.enqueue(id("second"), InvocationKind::Steering),
        Err(SchedulingError::Full)
    );
    assert_eq!(queue.len(), 1);
    assert_eq!(
        queue.pop_next(),
        Some((id("first"), InvocationKind::Queued))
    );
    queue
        .enqueue(id("second"), InvocationKind::Steering)
        .unwrap();
    assert_eq!(
        queue.pop_next(),
        Some((id("second"), InvocationKind::Steering))
    );
}

#[test]
fn scheduling_identity_is_unique_after_dispatch_drain_and_priority_changes() {
    let mut queue = InvocationQueue::new(1).unwrap();
    queue.enqueue(id("first"), InvocationKind::Queued).unwrap();
    // Duplicate takes precedence over capacity and cannot change priority.
    assert_eq!(
        queue.enqueue(id("first"), InvocationKind::Steering),
        Err(SchedulingError::Duplicate)
    );
    assert_eq!(
        queue.pop_next(),
        Some((id("first"), InvocationKind::Queued))
    );
    assert_eq!(
        queue.enqueue(id("first"), InvocationKind::Queued),
        Err(SchedulingError::Duplicate)
    );
    queue
        .enqueue(id("second"), InvocationKind::Steering)
        .unwrap();
    assert_eq!(
        queue.drain(),
        vec![(id("second"), InvocationKind::Steering)]
    );
    assert_eq!(
        queue.enqueue(id("second"), InvocationKind::Queued),
        Err(SchedulingError::Duplicate)
    );
    assert!(queue.drain().is_empty());
    queue.enqueue(id("third"), InvocationKind::Queued).unwrap();
    assert_eq!(queue.len(), 1);
}

#[test]
fn scheduling_removal_preserves_identity_history_and_remaining_priority_order() {
    let mut queue = InvocationQueue::new(5).unwrap();
    for (name, kind) in [
        ("queued-1", InvocationKind::Queued),
        ("steering-1", InvocationKind::Steering),
        ("queued-2", InvocationKind::Queued),
        ("steering-2", InvocationKind::Steering),
    ] {
        queue.enqueue(id(name), kind).unwrap();
    }
    assert_eq!(queue.remove(&id("missing")), None);
    assert_eq!(
        queue.remove(&id("steering-1")),
        Some(InvocationKind::Steering)
    );
    assert_eq!(queue.remove(&id("queued-1")), Some(InvocationKind::Queued));
    assert_eq!(queue.remove(&id("steering-1")), None);
    assert_eq!(
        queue.enqueue(id("steering-1"), InvocationKind::Queued),
        Err(SchedulingError::Duplicate)
    );
    assert_eq!(queue.len(), 2);
    assert_eq!(
        queue.pop_next(),
        Some((id("steering-2"), InvocationKind::Steering))
    );
    assert_eq!(queue.remove(&id("steering-2")), None);
    assert_eq!(
        queue.drain(),
        vec![(id("queued-2"), InvocationKind::Queued)]
    );
    assert_eq!(queue.remove(&id("queued-2")), None);
}

#[test]
fn transitions_reject_impossible_initial_and_terminal_edges() {
    for before in [
        None,
        Some(InvocationStage::Injected),
        Some(InvocationStage::Settled),
        Some(InvocationStage::Cancelled),
    ] {
        assert!(SchedulingTransition::new(
            InvocationKind::Steering,
            Some(id("target")),
            before,
            InvocationStage::Injected,
            SchedulingCause::SteeringInjected,
            SchedulingInitiator::Automatic
        )
        .is_err());
    }
    let value = SchedulingTransition::new(
        InvocationKind::Steering,
        Some(id("target")),
        Some(InvocationStage::Queued),
        InvocationStage::Injected,
        SchedulingCause::SteeringInjected,
        SchedulingInitiator::Automatic,
    )
    .unwrap();
    assert_eq!(value.kind(), InvocationKind::Steering);
    assert_eq!(value.target(), Some(&id("target")));
    assert_eq!(value.before(), Some(InvocationStage::Queued));
    assert_eq!(value.stage(), InvocationStage::Injected);
    assert_eq!(value.cause(), SchedulingCause::SteeringInjected);
    assert_eq!(value.initiator(), SchedulingInitiator::Automatic);
}

#[test]
fn transitions_require_correct_cause_target_and_attribution() {
    for (kind, target, stage, cause, initiator) in [
        (
            InvocationKind::Steering,
            None,
            InvocationStage::Injected,
            SchedulingCause::SteeringInjected,
            SchedulingInitiator::Automatic,
        ),
        (
            InvocationKind::Queued,
            Some(id("target")),
            InvocationStage::Injected,
            SchedulingCause::SteeringInjected,
            SchedulingInitiator::Automatic,
        ),
        (
            InvocationKind::Queued,
            None,
            InvocationStage::Cancelled,
            SchedulingCause::Withdrawn,
            SchedulingInitiator::Automatic,
        ),
        (
            InvocationKind::Queued,
            None,
            InvocationStage::Cancelled,
            SchedulingCause::RunnerStopped,
            SchedulingInitiator::Caller,
        ),
        (
            InvocationKind::Queued,
            None,
            InvocationStage::Cancelled,
            SchedulingCause::SessionClosed,
            SchedulingInitiator::Automatic,
        ),
        (
            InvocationKind::Queued,
            None,
            InvocationStage::Running,
            SchedulingCause::ExecutionSettled,
            SchedulingInitiator::Automatic,
        ),
    ] {
        assert!(SchedulingTransition::new(
            kind,
            target,
            Some(InvocationStage::Queued),
            stage,
            cause,
            initiator
        )
        .is_err());
    }
    assert_eq!(
        SchedulingTransitionError.to_string(),
        "invalid scheduling transition evidence"
    );
}

#[test]
fn scheduling_errors_describe_each_rejection() {
    assert_eq!(
        SchedulingError::Duplicate.to_string(),
        "invocation identity was already admitted"
    );
    assert_eq!(
        SchedulingError::Full.to_string(),
        "pending invocation queue is full"
    );
    assert_eq!(
        SchedulingError::InvalidCapacity.to_string(),
        "pending invocation capacity must be positive"
    );
}

#[test]
fn execution_settlement_requires_dispatch_but_dispatch_failure_does_not() {
    assert!(SchedulingTransition::new(
        InvocationKind::Queued,
        None,
        Some(InvocationStage::Queued),
        InvocationStage::Settled,
        SchedulingCause::ExecutionSettled,
        SchedulingInitiator::Automatic
    )
    .is_err());
    assert!(SchedulingTransition::new(
        InvocationKind::Queued,
        None,
        Some(InvocationStage::Queued),
        InvocationStage::Settled,
        SchedulingCause::DispatchFailed,
        SchedulingInitiator::Automatic
    )
    .is_ok());
}

#[test]
fn native_steering_target_is_retained_after_fallback_or_withdrawal() {
    for (before, stage, cause, initiator) in [
        (
            None,
            InvocationStage::Queued,
            SchedulingCause::Submitted,
            SchedulingInitiator::Caller,
        ),
        (
            Some(InvocationStage::Queued),
            InvocationStage::Running,
            SchedulingCause::Dispatched,
            SchedulingInitiator::Automatic,
        ),
        (
            Some(InvocationStage::Running),
            InvocationStage::Settled,
            SchedulingCause::ExecutionSettled,
            SchedulingInitiator::Automatic,
        ),
        (
            Some(InvocationStage::Queued),
            InvocationStage::Cancelled,
            SchedulingCause::Withdrawn,
            SchedulingInitiator::Caller,
        ),
    ] {
        let transition = SchedulingTransition::new(
            InvocationKind::Steering,
            Some(id("prior")),
            before,
            stage,
            cause,
            initiator,
        )
        .unwrap();
        assert_eq!(transition.target(), Some(&id("prior")));
    }
}

#[test]
fn shutdown_cancellations_require_their_actual_initiator() {
    for before in [InvocationStage::Queued, InvocationStage::Running] {
        for (cause, initiator) in [
            (SchedulingCause::SessionClosed, SchedulingInitiator::Caller),
            (
                SchedulingCause::RunnerStopped,
                SchedulingInitiator::Automatic,
            ),
        ] {
            assert!(SchedulingTransition::new(
                InvocationKind::Queued,
                None,
                Some(before),
                InvocationStage::Cancelled,
                cause,
                initiator
            )
            .is_ok());
        }
    }
}

#[test]
fn history_validation_rejects_discontinuous_stages_and_changed_correlation() {
    let transition = |kind, target, before, stage, cause, initiator| {
        SchedulingTransition::new(kind, target, before, stage, cause, initiator).unwrap()
    };
    let submitted = transition(
        InvocationKind::Steering,
        Some(id("target")),
        None,
        InvocationStage::Queued,
        SchedulingCause::Submitted,
        SchedulingInitiator::Caller,
    );
    let dispatched = transition(
        InvocationKind::Steering,
        Some(id("target")),
        Some(InvocationStage::Queued),
        InvocationStage::Running,
        SchedulingCause::Dispatched,
        SchedulingInitiator::Automatic,
    );
    let settled = transition(
        InvocationKind::Steering,
        Some(id("target")),
        Some(InvocationStage::Running),
        InvocationStage::Settled,
        SchedulingCause::ExecutionSettled,
        SchedulingInitiator::Automatic,
    );
    assert_eq!(SchedulingTransition::validate_history(&[]), Ok(()));
    assert_eq!(
        SchedulingTransition::validate_history(&[
            submitted.clone(),
            dispatched.clone(),
            settled.clone()
        ]),
        Ok(())
    );
    // All these transitions are individually valid; their order/correlation is not.
    for history in [
        vec![dispatched.clone()],
        vec![submitted.clone(), settled.clone()],
        vec![
            submitted.clone(),
            dispatched.clone(),
            settled,
            submitted.clone(),
        ],
        vec![
            submitted.clone(),
            transition(
                InvocationKind::Steering,
                Some(id("other")),
                Some(InvocationStage::Queued),
                InvocationStage::Running,
                SchedulingCause::Dispatched,
                SchedulingInitiator::Automatic,
            ),
        ],
        vec![
            submitted.clone(),
            transition(
                InvocationKind::Steering,
                None,
                Some(InvocationStage::Queued),
                InvocationStage::Running,
                SchedulingCause::Dispatched,
                SchedulingInitiator::Automatic,
            ),
        ],
        vec![
            transition(
                InvocationKind::Queued,
                None,
                None,
                InvocationStage::Queued,
                SchedulingCause::Submitted,
                SchedulingInitiator::Caller,
            ),
            transition(
                InvocationKind::Steering,
                None,
                Some(InvocationStage::Queued),
                InvocationStage::Running,
                SchedulingCause::Dispatched,
                SchedulingInitiator::Automatic,
            ),
        ],
    ] {
        assert_eq!(
            SchedulingTransition::validate_history(&history),
            Err(SchedulingTransitionError)
        );
    }
    let injected = transition(
        InvocationKind::Steering,
        Some(id("target")),
        Some(InvocationStage::Queued),
        InvocationStage::Injected,
        SchedulingCause::SteeringInjected,
        SchedulingInitiator::Automatic,
    );
    assert_eq!(
        SchedulingTransition::validate_history(&[submitted, injected]),
        Ok(())
    );
}

#[test]
fn large_ordinary_and_mixed_queues_drain_in_priority_fifo_order() {
    // No timing threshold: exercise large configured capacities while checking
    // every returned identity. Each pop is structurally at most two deque-front
    // operations, including the ordinary-only case that formerly rescanned all
    // remaining ordinary work on every pop.
    const ORDINARY: usize = 50_000;
    for steering_every in [None, Some(1), Some(997)] {
        let mut queue = InvocationQueue::new(ORDINARY * 2).unwrap();
        let mut expected_steering = Vec::new();
        let mut expected_ordinary = Vec::new();
        for index in 0..ORDINARY {
            let ordinary = id(&format!("ordinary-{index}"));
            queue
                .enqueue(ordinary.clone(), InvocationKind::Queued)
                .unwrap();
            expected_ordinary.push((ordinary, InvocationKind::Queued));
            if steering_every.is_some_and(|every| index % every == 0) {
                let steering = id(&format!("steering-{index}"));
                queue
                    .enqueue(steering.clone(), InvocationKind::Steering)
                    .unwrap();
                expected_steering.push((steering, InvocationKind::Steering));
            }
        }
        let mut expected = expected_steering;
        expected.extend(expected_ordinary);
        assert_eq!(queue.len(), expected.len());
        assert_eq!(queue.drain(), expected);
        assert!(queue.is_empty());
        assert_eq!(
            queue.enqueue(id("ordinary-0"), InvocationKind::Steering),
            Err(SchedulingError::Duplicate)
        );
        queue
            .enqueue(id("after-drain"), InvocationKind::Queued)
            .unwrap();
        assert_eq!(
            queue.pop_next(),
            Some((id("after-drain"), InvocationKind::Queued))
        );
        assert_eq!(queue.pop_next(), None);
    }
}

#[test]
fn execution_failure_only_ends_running_work_with_automatic_attribution() {
    for kind in [InvocationKind::Queued, InvocationKind::Steering] {
        for before in [
            None,
            Some(InvocationStage::Queued),
            Some(InvocationStage::Running),
            Some(InvocationStage::Injected),
            Some(InvocationStage::Settled),
            Some(InvocationStage::Cancelled),
        ] {
            for stage in [
                InvocationStage::Queued,
                InvocationStage::Running,
                InvocationStage::Injected,
                InvocationStage::Settled,
                InvocationStage::Cancelled,
            ] {
                for initiator in [SchedulingInitiator::Caller, SchedulingInitiator::Automatic] {
                    let target = (kind == InvocationKind::Steering).then(|| id("prior"));
                    let transition = SchedulingTransition::new(
                        kind,
                        target.clone(),
                        before,
                        stage,
                        SchedulingCause::ExecutionFailed,
                        initiator,
                    );
                    let legal = before == Some(InvocationStage::Running)
                        && stage == InvocationStage::Settled
                        && initiator == SchedulingInitiator::Automatic;
                    assert_eq!(
                        transition.is_ok(),
                        legal,
                        "{kind:?}/{before:?}/{stage:?}/{initiator:?}"
                    );
                    if let Ok(transition) = transition {
                        assert_eq!(transition.target(), target.as_ref());
                    }
                }
            }
        }
    }
}
