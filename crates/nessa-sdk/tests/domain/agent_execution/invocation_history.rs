use nessa_sdk::domain::agent_execution::executions::{
    ExecutionId, ExecutionOutcome, InvocationCancellation, InvocationHistory, InvocationKind,
    InvocationObservation, InvocationStage, SchedulingCause, SchedulingInitiator,
    SchedulingTransition, SubmissionMode,
};

fn id(value: &str) -> ExecutionId {
    ExecutionId::new(value).unwrap()
}
fn edge(
    kind: InvocationKind,
    target: Option<ExecutionId>,
    before: Option<InvocationStage>,
    stage: InvocationStage,
    cause: SchedulingCause,
) -> SchedulingTransition {
    let initiator = if matches!(
        cause,
        SchedulingCause::Submitted | SchedulingCause::SessionClosed | SchedulingCause::Withdrawn
    ) {
        SchedulingInitiator::Caller
    } else {
        SchedulingInitiator::Automatic
    };
    SchedulingTransition::new(kind, target, before, stage, cause, initiator).unwrap()
}
fn scheduled(mode: SubmissionMode) -> InvocationHistory {
    let mut evidence = InvocationHistory::new(id("run"), mode);
    let kind = if mode == SubmissionMode::Queued {
        InvocationKind::Queued
    } else {
        InvocationKind::Steering
    };
    evidence
        .schedule(edge(
            kind,
            None,
            None,
            InvocationStage::Queued,
            SchedulingCause::Submitted,
        ))
        .unwrap();
    evidence
        .schedule(edge(
            kind,
            None,
            Some(InvocationStage::Queued),
            InvocationStage::Running,
            SchedulingCause::Dispatched,
        ))
        .unwrap();
    evidence
}

#[test]
fn all_delivery_modes_share_terminal_ordering_and_settlement_rules() {
    for mode in [
        SubmissionMode::Immediate,
        SubmissionMode::Queued,
        SubmissionMode::BoundarySteering,
        SubmissionMode::Steering,
    ] {
        for outcome in [
            ExecutionOutcome::Completed,
            ExecutionOutcome::Cancelled,
            ExecutionOutcome::Refused,
            ExecutionOutcome::OutputLimit,
            ExecutionOutcome::RequestLimit,
        ] {
            let mut evidence = if mode == SubmissionMode::Immediate {
                InvocationHistory::new(id("run"), mode)
            } else {
                scheduled(mode)
            };
            evidence
                .observe(&id("run"), InvocationObservation::Output)
                .unwrap();
            evidence
                .observe(&id("run"), InvocationObservation::Finished(outcome))
                .unwrap();
            assert!(evidence
                .observe(&id("other"), InvocationObservation::PermissionCancellation)
                .is_err());
            assert!(evidence
                .observe(&id("run"), InvocationObservation::Output)
                .is_err());
            assert!(evidence
                .observe(&id("run"), InvocationObservation::Finished(outcome))
                .is_err());
            evidence
                .observe(&id("run"), InvocationObservation::PermissionCancellation)
                .unwrap();
            let different = if outcome == ExecutionOutcome::Completed {
                ExecutionOutcome::Cancelled
            } else {
                ExecutionOutcome::Completed
            };
            assert!(evidence.record_local_result(Ok(different)).is_err());
            evidence.record_local_result(Ok(outcome)).unwrap();
            // Later local observation/storage failure does not rewrite terminal evidence.
            evidence.record_local_result(Err(())).unwrap();
            evidence.validate_checkpoint().unwrap();
        }
    }
}

#[test]
fn restored_settlement_is_checked_against_later_replayed_terminal_observation() {
    let mut evidence = InvocationHistory::new(id("run"), SubmissionMode::Immediate);
    evidence
        .record_local_result(Ok(ExecutionOutcome::Completed))
        .unwrap();
    assert!(evidence
        .observe(
            &id("run"),
            InvocationObservation::Finished(ExecutionOutcome::Refused)
        )
        .is_err());
    evidence
        .observe(
            &id("run"),
            InvocationObservation::Finished(ExecutionOutcome::Completed),
        )
        .unwrap();
}

#[test]
fn scheduling_preserves_intent_target_and_continuity() {
    let admission = edge(
        InvocationKind::Queued,
        None,
        None,
        InvocationStage::Queued,
        SchedulingCause::Submitted,
    );
    let mut immediate = InvocationHistory::new(id("run"), SubmissionMode::Immediate);
    assert!(immediate.schedule(admission.clone()).is_err());
    let mut queue = InvocationHistory::new(id("run"), SubmissionMode::Queued);
    assert!(queue.validate_checkpoint().is_err());
    assert!(queue
        .observe(&id("run"), InvocationObservation::Output)
        .is_err());
    queue.schedule(admission.clone()).unwrap();
    assert!(queue.schedule(admission).is_err());
    queue.validate_checkpoint().unwrap();
    for mode in [SubmissionMode::BoundarySteering, SubmissionMode::Steering] {
        let mut evidence = InvocationHistory::new(id("run"), mode);
        assert!(evidence
            .schedule(edge(
                InvocationKind::Steering,
                Some(id("run")),
                None,
                InvocationStage::Queued,
                SchedulingCause::Submitted
            ))
            .is_err());
    }
    let mut boundary = InvocationHistory::new(id("run"), SubmissionMode::BoundarySteering);
    assert!(boundary
        .schedule(edge(
            InvocationKind::Steering,
            Some(id("target")),
            None,
            InvocationStage::Queued,
            SchedulingCause::Submitted
        ))
        .is_err());
    boundary
        .schedule(edge(
            InvocationKind::Steering,
            None,
            None,
            InvocationStage::Queued,
            SchedulingCause::Submitted,
        ))
        .unwrap();
    assert!(boundary
        .schedule(edge(
            InvocationKind::Steering,
            Some(id("target")),
            Some(InvocationStage::Queued),
            InvocationStage::Injected,
            SchedulingCause::SteeringInjected
        ))
        .is_err());
    let mut native = InvocationHistory::new(id("run"), SubmissionMode::Steering);
    native
        .schedule(edge(
            InvocationKind::Steering,
            Some(id("target")),
            None,
            InvocationStage::Queued,
            SchedulingCause::Submitted,
        ))
        .unwrap();
    assert!(native
        .schedule(edge(
            InvocationKind::Steering,
            Some(id("other")),
            Some(InvocationStage::Queued),
            InvocationStage::Running,
            SchedulingCause::Dispatched
        ))
        .is_err());
    native
        .schedule(edge(
            InvocationKind::Steering,
            Some(id("target")),
            Some(InvocationStage::Queued),
            InvocationStage::Injected,
            SchedulingCause::SteeringInjected,
        ))
        .unwrap();
    assert!(native
        .record_local_result(Ok(ExecutionOutcome::Completed))
        .is_err());
    native.record_local_result(Err(())).unwrap();
    native.validate_checkpoint().unwrap();
}

#[test]
fn scheduling_terminal_edges_require_consistent_settlement() {
    let mut evidence = scheduled(SubmissionMode::Queued);
    evidence
        .schedule(edge(
            InvocationKind::Queued,
            None,
            Some(InvocationStage::Running),
            InvocationStage::Settled,
            SchedulingCause::ExecutionSettled,
        ))
        .unwrap();
    assert!(evidence.validate_checkpoint().is_err());
    evidence
        .record_local_result(Ok(ExecutionOutcome::Completed))
        .unwrap();
    evidence.validate_checkpoint().unwrap();
    for cause in [
        SchedulingCause::DispatchFailed,
        SchedulingCause::SessionClosed,
    ] {
        let mut evidence = scheduled(SubmissionMode::Queued);
        let stage = if cause == SchedulingCause::DispatchFailed {
            InvocationStage::Settled
        } else {
            InvocationStage::Cancelled
        };
        evidence
            .schedule(edge(
                InvocationKind::Queued,
                None,
                Some(InvocationStage::Running),
                stage,
                cause,
            ))
            .unwrap();
        assert!(evidence
            .record_local_result(Ok(ExecutionOutcome::Completed))
            .is_err());
        if cause == SchedulingCause::SessionClosed {
            evidence
                .record_local_result(Ok(ExecutionOutcome::Cancelled))
                .unwrap();
        }
        evidence.record_local_result(Err(())).unwrap();
        evidence.validate_checkpoint().unwrap();
    }
    let mut evidence = scheduled(SubmissionMode::Queued);
    evidence
        .record_local_result(Ok(ExecutionOutcome::Completed))
        .unwrap();
    assert!(evidence
        .schedule(edge(
            InvocationKind::Queued,
            None,
            Some(InvocationStage::Running),
            InvocationStage::Settled,
            SchedulingCause::DispatchFailed
        ))
        .is_err());
    evidence
        .schedule(edge(
            InvocationKind::Queued,
            None,
            Some(InvocationStage::Running),
            InvocationStage::Settled,
            SchedulingCause::ExecutionSettled,
        ))
        .unwrap();
}

#[test]
fn provider_facts_require_dispatch_and_conflicts_require_local_failure() {
    let mut waiting = InvocationHistory::new(id("run"), SubmissionMode::Queued);
    assert_eq!(
        waiting
            .record_provider_result(None)
            .unwrap_err()
            .to_string(),
        "provider settlement targets an undispatched invocation"
    );
    for actual in [Ok(ExecutionOutcome::Refused), Err(())] {
        let mut evidence = scheduled(SubmissionMode::Queued);
        evidence
            .observe(
                &id("run"),
                InvocationObservation::Finished(ExecutionOutcome::Completed),
            )
            .unwrap();
        evidence.record_provider_result(Some(actual)).unwrap();
        assert!(evidence.has_provider_conflict());
        // A transient checkpoint retains both observations before SDK settlement.
        evidence.validate_checkpoint().unwrap();
        assert!(evidence
            .record_local_result(Ok(ExecutionOutcome::Completed))
            .is_err());
        assert!(evidence
            .record_local_result(Ok(ExecutionOutcome::Refused))
            .is_err());
        evidence.record_local_result(Err(())).unwrap();
        evidence.validate_checkpoint().unwrap();
        evidence.record_provider_result(Some(actual)).unwrap();
        assert!(evidence.record_provider_result(None).is_err());
    }
    let mut evidence = InvocationHistory::new(id("run"), SubmissionMode::Immediate);
    evidence.record_provider_result(None).unwrap();
    assert!(!evidence.has_provider_conflict());
    evidence
        .record_local_result(Ok(ExecutionOutcome::Completed))
        .unwrap();
    assert!(evidence
        .record_provider_result(Some(Ok(ExecutionOutcome::Refused)))
        .is_err());
    evidence
        .record_provider_result(Some(Ok(ExecutionOutcome::Completed)))
        .unwrap();
    evidence
        .observe(
            &id("run"),
            InvocationObservation::Finished(ExecutionOutcome::Completed),
        )
        .unwrap();
    assert!(!evidence.has_provider_conflict());
}

#[test]
fn local_result_repeats_or_degrades_but_never_changes_or_recovers_success() {
    let results = [
        Ok(ExecutionOutcome::Completed),
        Ok(ExecutionOutcome::Cancelled),
        Ok(ExecutionOutcome::Refused),
        Ok(ExecutionOutcome::OutputLimit),
        Ok(ExecutionOutcome::RequestLimit),
        Err(()),
    ];
    for mode in [
        SubmissionMode::Immediate,
        SubmissionMode::Queued,
        SubmissionMode::BoundarySteering,
        SubmissionMode::Steering,
    ] {
        for prior in results {
            for next in results {
                let mut history = if mode == SubmissionMode::Immediate {
                    InvocationHistory::new(id("run"), mode)
                } else {
                    scheduled(mode)
                };
                // No provider/terminal fact can incidentally enforce this local rule.
                history.record_local_result(prior).unwrap();
                let accepted = prior == next || next.is_err();
                assert_eq!(
                    history.record_local_result(next).is_ok(),
                    accepted,
                    "{mode:?}: {prior:?} -> {next:?}"
                );
                if accepted {
                    history.record_local_result(next).unwrap();
                } else {
                    // Rejection cannot mutate history: repeating the rejected write
                    // still fails, while the original result remains idempotent.
                    assert!(history.record_local_result(next).is_err());
                    history.record_local_result(prior).unwrap();
                }
                history.validate_checkpoint().unwrap();
            }
        }
    }
}

#[test]
fn execution_settled_checkpoints_require_exact_outcomes_from_any_retained_source() {
    for mode in [
        SubmissionMode::Queued,
        SubmissionMode::BoundarySteering,
        SubmissionMode::Steering,
    ] {
        let kind = if mode == SubmissionMode::Queued {
            InvocationKind::Queued
        } else {
            InvocationKind::Steering
        };
        for outcome in [
            ExecutionOutcome::Completed,
            ExecutionOutcome::Cancelled,
            ExecutionOutcome::Refused,
            ExecutionOutcome::OutputLimit,
            ExecutionOutcome::RequestLimit,
        ] {
            for source in ["local", "provider", "terminal", "unknown", "provider-error"] {
                for result_first in [false, true] {
                    let mut history = scheduled(mode);
                    if source == "provider" {
                        history.record_provider_result(Some(Ok(outcome))).unwrap();
                    }
                    if source == "provider-error" {
                        history.record_provider_result(Some(Err(()))).unwrap();
                    }
                    if source == "terminal" {
                        history
                            .observe(&id("run"), InvocationObservation::Finished(outcome))
                            .unwrap();
                    }
                    let local = if source == "local" {
                        Ok(outcome)
                    } else {
                        Err(())
                    };
                    if result_first {
                        history.record_local_result(local).unwrap();
                    }
                    // Saving a result before the final scheduling edge remains valid.
                    history.validate_checkpoint().unwrap();
                    history
                        .schedule(edge(
                            kind,
                            None,
                            Some(InvocationStage::Running),
                            InvocationStage::Settled,
                            SchedulingCause::ExecutionSettled,
                        ))
                        .unwrap();
                    if !result_first {
                        assert!(history.validate_checkpoint().is_err());
                        history.record_local_result(local).unwrap();
                    }
                    assert_eq!(
                        history.validate_checkpoint().is_ok(),
                        matches!(source, "local" | "provider" | "terminal"),
                        "{mode:?}/{source}/{outcome:?}/result_first={result_first}"
                    );
                }
            }
        }
    }
}

#[test]
fn execution_failure_cause_requires_local_failure_without_a_known_outcome() {
    for mode in [
        SubmissionMode::Queued,
        SubmissionMode::BoundarySteering,
        SubmissionMode::Steering,
    ] {
        let kind = if mode == SubmissionMode::Queued {
            InvocationKind::Queued
        } else {
            InvocationKind::Steering
        };
        for source in ["unknown", "provider-error", "local", "provider", "terminal"] {
            let mut history = scheduled(mode);
            assert_eq!(history.settlement_cause(), None);
            match source {
                "provider-error" => history.record_provider_result(Some(Err(()))).unwrap(),
                "provider" => history
                    .record_provider_result(Some(Ok(ExecutionOutcome::Completed)))
                    .unwrap(),
                "terminal" => history
                    .observe(
                        &id("run"),
                        InvocationObservation::Finished(ExecutionOutcome::Completed),
                    )
                    .unwrap(),
                "local" => history
                    .record_local_result(Ok(ExecutionOutcome::Completed))
                    .unwrap(),
                _ => {}
            }
            let known = matches!(source, "local" | "provider" | "terminal");
            assert_eq!(
                history.settlement_cause(),
                known.then_some(SchedulingCause::ExecutionSettled)
            );
            if source != "local" {
                history.record_local_result(Err(())).unwrap();
            }
            assert_eq!(
                history.settlement_cause(),
                Some(if known {
                    SchedulingCause::ExecutionSettled
                } else {
                    SchedulingCause::ExecutionFailed
                })
            );
            let transition = edge(
                kind,
                None,
                Some(InvocationStage::Running),
                InvocationStage::Settled,
                SchedulingCause::ExecutionFailed,
            );
            if known {
                assert!(history.schedule(transition).is_err());
            } else {
                history.schedule(transition).unwrap();
                assert_eq!(history.validate_checkpoint().is_ok(), !known);
                assert!(history
                    .record_local_result(Ok(ExecutionOutcome::Completed))
                    .is_err());
            }
        }
        let mut unrecorded = scheduled(mode);
        unrecorded
            .schedule(edge(
                kind,
                None,
                Some(InvocationStage::Running),
                InvocationStage::Settled,
                SchedulingCause::ExecutionFailed,
            ))
            .unwrap();
        assert!(unrecorded.validate_checkpoint().is_err());
        // The failure edge may be assembled before its local result, but a
        // checkpoint is complete only after the result is retained.
        assert!(unrecorded
            .record_local_result(Ok(ExecutionOutcome::Completed))
            .is_err());
        unrecorded.record_local_result(Err(())).unwrap();
        unrecorded.validate_checkpoint().unwrap();
    }
}

const OUTCOMES: [ExecutionOutcome; 5] = [
    ExecutionOutcome::Completed,
    ExecutionOutcome::Cancelled,
    ExecutionOutcome::Refused,
    ExecutionOutcome::OutputLimit,
    ExecutionOutcome::RequestLimit,
];
const QUEUED_MODES: [SubmissionMode; 3] = [
    SubmissionMode::Queued,
    SubmissionMode::BoundarySteering,
    SubmissionMode::Steering,
];
fn kind(mode: SubmissionMode) -> InvocationKind {
    if mode == SubmissionMode::Queued {
        InvocationKind::Queued
    } else {
        InvocationKind::Steering
    }
}

#[test]
fn queued_success_requires_dispatch_except_for_cancelled_pending_work() {
    for mode in QUEUED_MODES {
        for outcome in OUTCOMES {
            for cause in [
                SchedulingCause::Withdrawn,
                SchedulingCause::SessionClosed,
                SchedulingCause::RunnerStopped,
            ] {
                let mut history = InvocationHistory::new(id("run"), mode);
                assert!(history.record_local_result(Ok(outcome)).is_err());
                assert!(history.record_local_outcome(outcome).is_err());
                assert_eq!(history.local_outcome(), None);
                history
                    .schedule(edge(
                        kind(mode),
                        None,
                        None,
                        InvocationStage::Queued,
                        SchedulingCause::Submitted,
                    ))
                    .unwrap();
                assert!(history.record_local_result(Ok(outcome)).is_err());
                assert!(history.record_local_outcome(outcome).is_err());
                history.validate_checkpoint().unwrap();
                history
                    .schedule(edge(
                        kind(mode),
                        None,
                        Some(InvocationStage::Queued),
                        InvocationStage::Cancelled,
                        cause,
                    ))
                    .unwrap();
                assert_eq!(
                    history.record_local_result(Ok(outcome)).is_ok(),
                    outcome == ExecutionOutcome::Cancelled
                );
                if outcome != ExecutionOutcome::Cancelled {
                    assert_eq!(history.local_outcome(), None);
                    history
                        .record_local_result(Ok(ExecutionOutcome::Cancelled))
                        .unwrap();
                }
                history.validate_checkpoint().unwrap();
            }
        }
    }
}

#[test]
fn later_local_failure_preserves_exact_outcome_and_rejects_conflicting_facts() {
    for mode in [
        SubmissionMode::Immediate,
        SubmissionMode::Queued,
        SubmissionMode::BoundarySteering,
        SubmissionMode::Steering,
    ] {
        for outcome in OUTCOMES {
            let original = if mode == SubmissionMode::Immediate {
                InvocationHistory::new(id("run"), mode)
            } else {
                scheduled(mode)
            };
            let mut history = original.clone();
            history.record_local_result(Ok(outcome)).unwrap();
            history.record_local_result(Err(())).unwrap();
            assert_eq!(history.local_outcome(), Some(outcome));
            assert_eq!(
                history.settlement_cause(),
                Some(SchedulingCause::ExecutionSettled)
            );
            for conflicting in OUTCOMES.into_iter().filter(|other| *other != outcome) {
                assert!(history.record_local_outcome(conflicting).is_err());
                assert!(history
                    .record_provider_result(Some(Ok(conflicting)))
                    .is_err());
                assert!(history
                    .observe(&id("run"), InvocationObservation::Finished(conflicting))
                    .is_err());
                assert_eq!(history.local_outcome(), Some(outcome));
                history.validate_checkpoint().unwrap();
            }
            assert!(history.record_provider_result(Some(Err(()))).is_err());
            history.record_provider_result(Some(Ok(outcome))).unwrap();
            history
                .observe(&id("run"), InvocationObservation::Finished(outcome))
                .unwrap();
            history.validate_checkpoint().unwrap();
            let mut restored = original;
            restored.record_local_outcome(outcome).unwrap();
            restored.record_local_outcome(outcome).unwrap();
            assert!(restored.validate_checkpoint().is_err());
            let different = if outcome == ExecutionOutcome::Completed {
                ExecutionOutcome::Refused
            } else {
                ExecutionOutcome::Completed
            };
            assert!(restored.record_local_result(Ok(different)).is_err());
            restored.record_local_result(Err(())).unwrap();
            assert_eq!(restored.local_outcome(), Some(outcome));
            assert_eq!(
                restored.settlement_cause(),
                Some(SchedulingCause::ExecutionSettled)
            );
            restored.validate_checkpoint().unwrap();
        }
    }
}

fn exact_fact(history: &mut InvocationHistory, source: &str, outcome: ExecutionOutcome) -> bool {
    match source {
        "local" => history.record_local_outcome(outcome).is_ok(),
        "provider" => history.record_provider_result(Some(Ok(outcome))).is_ok(),
        "terminal" => history
            .observe(&id("run"), InvocationObservation::Finished(outcome))
            .is_ok(),
        _ => unreachable!(),
    }
}
#[test]
fn failed_scheduling_causes_reject_exact_outcomes_without_invalidating_checkpoints() {
    for cause in [
        SchedulingCause::DispatchFailed,
        SchedulingCause::ExecutionFailed,
    ] {
        for mode in QUEUED_MODES {
            for outcome in OUTCOMES {
                for source in ["local", "provider", "terminal"] {
                    for failure_first in [false, true] {
                        let mut history = scheduled(mode);
                        history.record_local_result(Err(())).unwrap();
                        let failure = edge(
                            kind(mode),
                            None,
                            Some(InvocationStage::Running),
                            InvocationStage::Settled,
                            cause,
                        );
                        if failure_first {
                            history.schedule(failure).unwrap();
                            assert!(!exact_fact(&mut history, source, outcome));
                            assert_eq!(history.local_outcome(), None);
                            history.validate_checkpoint().unwrap();
                        } else {
                            assert!(exact_fact(&mut history, source, outcome));
                            assert!(history.schedule(failure).is_err());
                            history
                                .schedule(edge(
                                    kind(mode),
                                    None,
                                    Some(InvocationStage::Running),
                                    InvocationStage::Settled,
                                    SchedulingCause::ExecutionSettled,
                                ))
                                .unwrap();
                            history.validate_checkpoint().unwrap();
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn idle_steering_can_complete_without_a_target_but_cannot_claim_injection() {
    let mut history = InvocationHistory::new(id("idle"), SubmissionMode::Steering);
    history
        .schedule(edge(
            InvocationKind::Steering,
            None,
            None,
            InvocationStage::Queued,
            SchedulingCause::Submitted,
        ))
        .unwrap();
    history.validate_checkpoint().unwrap();
    assert!(SchedulingTransition::new(
        InvocationKind::Steering,
        None,
        Some(InvocationStage::Queued),
        InvocationStage::Injected,
        SchedulingCause::SteeringInjected,
        SchedulingInitiator::Automatic,
    )
    .is_err());
    history
        .schedule(edge(
            InvocationKind::Steering,
            None,
            Some(InvocationStage::Queued),
            InvocationStage::Running,
            SchedulingCause::Dispatched,
        ))
        .unwrap();
    history
        .observe(
            &id("idle"),
            InvocationObservation::Finished(ExecutionOutcome::Completed),
        )
        .unwrap();
    history
        .record_provider_result(Some(Ok(ExecutionOutcome::Completed)))
        .unwrap();
    history
        .record_local_result(Ok(ExecutionOutcome::Completed))
        .unwrap();
    history
        .schedule(edge(
            InvocationKind::Steering,
            None,
            Some(InvocationStage::Running),
            InvocationStage::Settled,
            SchedulingCause::ExecutionSettled,
        ))
        .unwrap();
    history.validate_checkpoint().unwrap();
}

#[test]
fn recorded_failure_prevents_dispatch_without_preventing_final_scheduling() {
    for mode in [
        SubmissionMode::Queued,
        SubmissionMode::BoundarySteering,
        SubmissionMode::Steering,
    ] {
        let kind = if mode == SubmissionMode::Queued {
            InvocationKind::Queued
        } else {
            InvocationKind::Steering
        };
        for (stage, cause) in [
            (InvocationStage::Settled, SchedulingCause::DispatchFailed),
            (InvocationStage::Cancelled, SchedulingCause::SessionClosed),
            (InvocationStage::Cancelled, SchedulingCause::Withdrawn),
        ] {
            let mut history = InvocationHistory::new(id("run"), mode);
            history
                .schedule(edge(
                    kind,
                    None,
                    None,
                    InvocationStage::Queued,
                    SchedulingCause::Submitted,
                ))
                .unwrap();
            history.record_local_result(Err(())).unwrap();
            assert!(history
                .schedule(edge(
                    kind,
                    None,
                    Some(InvocationStage::Queued),
                    InvocationStage::Running,
                    SchedulingCause::Dispatched
                ))
                .is_err());
            // Rejected dispatch neither changes the prior stage nor permits provider work.
            assert!(history
                .observe(&id("run"), InvocationObservation::Output)
                .is_err());
            assert!(history
                .record_provider_result(Some(Ok(ExecutionOutcome::Completed)))
                .is_err());
            history
                .schedule(edge(
                    kind,
                    None,
                    Some(InvocationStage::Queued),
                    stage,
                    cause,
                ))
                .unwrap();
            history.validate_checkpoint().unwrap();
        }
    }
}

#[test]
fn dispatched_results_and_restored_final_results_remain_valid() {
    for mode in [
        SubmissionMode::Immediate,
        SubmissionMode::Queued,
        SubmissionMode::BoundarySteering,
        SubmissionMode::Steering,
    ] {
        for result in [Ok(ExecutionOutcome::Completed), Err(())] {
            for restoring in [false, true] {
                let mut history = if mode == SubmissionMode::Immediate {
                    InvocationHistory::new(id("run"), mode)
                } else {
                    scheduled(mode)
                };
                let cause = if result.is_ok() {
                    SchedulingCause::ExecutionSettled
                } else {
                    SchedulingCause::ExecutionFailed
                };
                if !restoring {
                    history.record_local_result(result).unwrap();
                }
                if mode != SubmissionMode::Immediate {
                    let kind = if mode == SubmissionMode::Queued {
                        InvocationKind::Queued
                    } else {
                        InvocationKind::Steering
                    };
                    history
                        .schedule(edge(
                            kind,
                            None,
                            Some(InvocationStage::Running),
                            InvocationStage::Settled,
                            cause,
                        ))
                        .unwrap();
                }
                // Snapshot replay reconstructs scheduling before its final result;
                // it does not claim that result existed before dispatch.
                if restoring {
                    history.record_local_result(result).unwrap();
                }
                history.validate_checkpoint().unwrap();
            }
        }
    }
}

#[test]
fn immediate_cancellation_preserves_cause_and_rejects_provider_work_in_both_orders() {
    for (cause, initiator) in [
        (SchedulingCause::SessionClosed, SchedulingInitiator::Caller),
        (
            SchedulingCause::RunnerStopped,
            SchedulingInitiator::Automatic,
        ),
    ] {
        let cancellation = InvocationCancellation::new(cause, initiator).unwrap();
        assert_eq!(cancellation.cause(), cause);
        assert_eq!(cancellation.initiator(), initiator);
        let mut cancelled = InvocationHistory::new(id("run"), SubmissionMode::Immediate);
        cancelled.record_cancellation(cancellation).unwrap();
        cancelled.record_cancellation(cancellation).unwrap();
        for observation in [
            InvocationObservation::Output,
            InvocationObservation::PermissionCancellation,
            InvocationObservation::Finished(ExecutionOutcome::Cancelled),
        ] {
            assert!(cancelled.observe(&id("run"), observation).is_err());
            let mut observed = InvocationHistory::new(id("run"), SubmissionMode::Immediate);
            observed.observe(&id("run"), observation).unwrap();
            assert!(observed.record_cancellation(cancellation).is_err());
            assert_eq!(observed.cancellation(), None);
            observed.validate_checkpoint().unwrap();
        }
        for result in [None, Some(Err(())), Some(Ok(ExecutionOutcome::Cancelled))] {
            assert!(cancelled.record_provider_result(result).is_err());
            let mut reported = InvocationHistory::new(id("run"), SubmissionMode::Immediate);
            reported.record_provider_result(result).unwrap();
            assert!(reported.record_cancellation(cancellation).is_err());
            assert_eq!(reported.cancellation(), None);
            reported.validate_checkpoint().unwrap();
        }
        for mode in [
            SubmissionMode::Queued,
            SubmissionMode::BoundarySteering,
            SubmissionMode::Steering,
        ] {
            assert!(InvocationHistory::new(id("run"), mode)
                .record_cancellation(cancellation)
                .is_err());
        }
        let different = InvocationCancellation::new(
            if cause == SchedulingCause::SessionClosed {
                SchedulingCause::RunnerStopped
            } else {
                SchedulingCause::SessionClosed
            },
            if initiator == SchedulingInitiator::Caller {
                SchedulingInitiator::Automatic
            } else {
                SchedulingInitiator::Caller
            },
        )
        .unwrap();
        assert!(cancelled.record_cancellation(different).is_err());
        assert_eq!(cancelled.cancellation(), Some(&cancellation));
        cancelled.record_local_result(Err(())).unwrap();
        cancelled.validate_checkpoint().unwrap();
    }
}

#[test]
fn immediate_cancellation_and_local_outcome_must_agree_in_both_orders() {
    let cancellation =
        InvocationCancellation::new(SchedulingCause::SessionClosed, SchedulingInitiator::Caller)
            .unwrap();
    for outcome in [
        ExecutionOutcome::Completed,
        ExecutionOutcome::Cancelled,
        ExecutionOutcome::Refused,
        ExecutionOutcome::OutputLimit,
        ExecutionOutcome::RequestLimit,
    ] {
        for cancellation_first in [false, true] {
            let mut history = InvocationHistory::new(id("run"), SubmissionMode::Immediate);
            if cancellation_first {
                history.record_cancellation(cancellation).unwrap();
                assert_eq!(
                    history.record_local_result(Ok(outcome)).is_ok(),
                    outcome == ExecutionOutcome::Cancelled
                );
                assert_eq!(
                    history.local_outcome(),
                    (outcome == ExecutionOutcome::Cancelled).then_some(outcome)
                );
            } else {
                history.record_local_result(Ok(outcome)).unwrap();
                assert_eq!(
                    history.record_cancellation(cancellation).is_ok(),
                    outcome == ExecutionOutcome::Cancelled
                );
                assert_eq!(
                    history.cancellation().is_some(),
                    outcome == ExecutionOutcome::Cancelled
                );
            }
            history.validate_checkpoint().unwrap();
        }
    }
    for (cause, initiator) in [
        (
            SchedulingCause::SessionClosed,
            SchedulingInitiator::Automatic,
        ),
        (SchedulingCause::RunnerStopped, SchedulingInitiator::Caller),
        (SchedulingCause::Submitted, SchedulingInitiator::Caller),
    ] {
        let error = InvocationCancellation::new(cause, initiator).unwrap_err();
        assert_eq!(
            error.to_string(),
            "invalid invocation cancellation cause or initiator"
        );
    }
}

#[test]
fn local_failure_blocks_every_native_steering_delivery_stage_without_mutation() {
    for (stage, cause) in [
        (InvocationStage::Running, SchedulingCause::Dispatched),
        (InvocationStage::Injected, SchedulingCause::SteeringInjected),
    ] {
        let mut history = InvocationHistory::new(id("steer"), SubmissionMode::Steering);
        history
            .schedule(edge(
                InvocationKind::Steering,
                Some(id("active")),
                None,
                InvocationStage::Queued,
                SchedulingCause::Submitted,
            ))
            .unwrap();
        history.record_local_result(Err(())).unwrap();
        assert!(history
            .schedule(edge(
                InvocationKind::Steering,
                Some(id("active")),
                Some(InvocationStage::Queued),
                stage,
                cause
            ))
            .is_err());
        history
            .schedule(edge(
                InvocationKind::Steering,
                Some(id("active")),
                Some(InvocationStage::Queued),
                InvocationStage::Settled,
                SchedulingCause::DispatchFailed,
            ))
            .unwrap();
        history.validate_checkpoint().unwrap();
    }
}

#[test]
fn failed_dispatch_rejects_output_in_both_orders_but_retains_cleanup_only() {
    for mode in [
        SubmissionMode::Queued,
        SubmissionMode::BoundarySteering,
        SubmissionMode::Steering,
    ] {
        let kind = if mode == SubmissionMode::Queued {
            InvocationKind::Queued
        } else {
            InvocationKind::Steering
        };
        for output_first in [false, true] {
            let mut history = scheduled(mode);
            history.record_local_result(Err(())).unwrap();
            let failed = edge(
                kind,
                None,
                Some(InvocationStage::Running),
                InvocationStage::Settled,
                SchedulingCause::DispatchFailed,
            );
            if output_first {
                history
                    .observe(&id("run"), InvocationObservation::Output)
                    .unwrap();
                assert!(history.schedule(failed).is_err());
                // Rejection keeps Running and its actual execution failure can settle.
                history
                    .schedule(edge(
                        kind,
                        None,
                        Some(InvocationStage::Running),
                        InvocationStage::Settled,
                        SchedulingCause::ExecutionFailed,
                    ))
                    .unwrap();
            } else {
                history.schedule(failed).unwrap();
                assert!(history
                    .observe(&id("run"), InvocationObservation::Output)
                    .is_err());
                // Mandatory cleanup remains valid and cannot become provider output.
                history
                    .observe(&id("run"), InvocationObservation::PermissionCancellation)
                    .unwrap();
            }
            history.validate_checkpoint().unwrap();
        }
        let mut history = scheduled(mode);
        history
            .observe(&id("run"), InvocationObservation::PermissionCancellation)
            .unwrap();
        history.record_local_result(Err(())).unwrap();
        history
            .schedule(edge(
                kind,
                None,
                Some(InvocationStage::Running),
                InvocationStage::Settled,
                SchedulingCause::DispatchFailed,
            ))
            .unwrap();
        history.validate_checkpoint().unwrap();
    }
}

#[test]
fn steering_target_history_requires_possible_dispatch_without_claiming_current_activity() {
    let mut immediate = InvocationHistory::new(id("run"), SubmissionMode::Immediate);
    immediate.validate_steering_target().unwrap();
    immediate.record_local_result(Err(())).unwrap();
    immediate.validate_steering_target().unwrap();
    immediate
        .record_cancellation(
            InvocationCancellation::new(
                SchedulingCause::SessionClosed,
                SchedulingInitiator::Caller,
            )
            .unwrap(),
        )
        .unwrap();
    assert!(immediate.validate_steering_target().is_err());
    for mode in [
        SubmissionMode::Queued,
        SubmissionMode::BoundarySteering,
        SubmissionMode::Steering,
    ] {
        let kind = if mode == SubmissionMode::Queued {
            InvocationKind::Queued
        } else {
            InvocationKind::Steering
        };
        let mut queued = InvocationHistory::new(id("run"), mode);
        queued
            .schedule(edge(
                kind,
                None,
                None,
                InvocationStage::Queued,
                SchedulingCause::Submitted,
            ))
            .unwrap();
        assert!(queued.validate_steering_target().is_err());
        queued
            .schedule(edge(
                kind,
                None,
                Some(InvocationStage::Queued),
                InvocationStage::Cancelled,
                SchedulingCause::SessionClosed,
            ))
            .unwrap();
        assert!(queued.validate_steering_target().is_err());
        let mut dispatched = scheduled(mode);
        dispatched.validate_steering_target().unwrap();
        dispatched.record_local_result(Err(())).unwrap();
        dispatched
            .schedule(edge(
                kind,
                None,
                Some(InvocationStage::Running),
                InvocationStage::Settled,
                SchedulingCause::DispatchFailed,
            ))
            .unwrap();
        dispatched.validate_steering_target().unwrap();
    }
    let mut injected = InvocationHistory::new(id("run"), SubmissionMode::Steering);
    injected
        .schedule(edge(
            InvocationKind::Steering,
            Some(id("prior")),
            None,
            InvocationStage::Queued,
            SchedulingCause::Submitted,
        ))
        .unwrap();
    injected
        .schedule(edge(
            InvocationKind::Steering,
            Some(id("prior")),
            Some(InvocationStage::Queued),
            InvocationStage::Injected,
            SchedulingCause::SteeringInjected,
        ))
        .unwrap();
    assert!(injected.validate_steering_target().is_err());
}

#[test]
fn dispatched_local_cancellation_retains_independent_provider_facts_in_both_orders() {
    for mode in [
        SubmissionMode::Queued,
        SubmissionMode::BoundarySteering,
        SubmissionMode::Steering,
    ] {
        for cause in [
            SchedulingCause::SessionClosed,
            SchedulingCause::RunnerStopped,
        ] {
            for provider in [
                Ok(ExecutionOutcome::Completed),
                Ok(ExecutionOutcome::Cancelled),
                Ok(ExecutionOutcome::Refused),
                Ok(ExecutionOutcome::OutputLimit),
                Ok(ExecutionOutcome::RequestLimit),
                Err(()),
            ] {
                for provider_first in [false, true] {
                    for restored_failure in [false, true] {
                        let mut history = scheduled(mode);
                        if provider_first {
                            history.record_provider_result(Some(provider)).unwrap();
                            if let Ok(outcome) = provider {
                                history
                                    .observe(&id("run"), InvocationObservation::Finished(outcome))
                                    .unwrap();
                            }
                        }
                        history
                            .schedule(edge(
                                if mode == SubmissionMode::Queued {
                                    InvocationKind::Queued
                                } else {
                                    InvocationKind::Steering
                                },
                                None,
                                Some(InvocationStage::Running),
                                InvocationStage::Cancelled,
                                cause,
                            ))
                            .unwrap();
                        if restored_failure {
                            history
                                .record_local_outcome(ExecutionOutcome::Cancelled)
                                .unwrap();
                            history.record_local_result(Err(())).unwrap();
                        } else {
                            history
                                .record_local_result(Ok(ExecutionOutcome::Cancelled))
                                .unwrap();
                        }
                        if !provider_first {
                            history.record_provider_result(Some(provider)).unwrap();
                            if let Ok(outcome) = provider {
                                history
                                    .observe(&id("run"), InvocationObservation::Finished(outcome))
                                    .unwrap();
                            }
                        }
                        assert_eq!(history.local_outcome(), Some(ExecutionOutcome::Cancelled));
                        assert!(!history.has_provider_conflict());
                        history.validate_checkpoint().unwrap();
                        // Later local diagnostics retain both the local decision and provider facts.
                        history.record_local_result(Err(())).unwrap();
                        assert_eq!(history.local_outcome(), Some(ExecutionOutcome::Cancelled));
                        history.validate_checkpoint().unwrap();
                        assert!(history
                            .record_local_result(Ok(ExecutionOutcome::Completed))
                            .is_err());
                    }
                }
            }
        }
    }
}

#[test]
fn local_cancelled_outcome_requires_causal_edge_before_differing_provider_result() {
    for provider_first in [false, true] {
        let mut history = scheduled(SubmissionMode::Queued);
        if provider_first {
            history
                .record_provider_result(Some(Ok(ExecutionOutcome::Completed)))
                .unwrap();
            assert!(history
                .record_local_result(Ok(ExecutionOutcome::Cancelled))
                .is_err());
            assert_eq!(history.local_outcome(), None);
        } else {
            history
                .record_local_result(Ok(ExecutionOutcome::Cancelled))
                .unwrap();
            assert!(history
                .record_provider_result(Some(Ok(ExecutionOutcome::Completed)))
                .is_err());
        }
        history
            .schedule(edge(
                InvocationKind::Queued,
                None,
                Some(InvocationStage::Running),
                InvocationStage::Cancelled,
                SchedulingCause::SessionClosed,
            ))
            .unwrap();
        if provider_first {
            history
                .record_local_result(Ok(ExecutionOutcome::Cancelled))
                .unwrap();
        } else {
            history
                .record_provider_result(Some(Ok(ExecutionOutcome::Completed)))
                .unwrap();
        }
        assert_eq!(history.local_outcome(), Some(ExecutionOutcome::Cancelled));
        history.validate_checkpoint().unwrap();
    }
}

#[test]
fn dispatched_local_cancellation_retains_first_stop_without_inventing_provider_result() {
    let explicit =
        InvocationCancellation::new(SchedulingCause::SessionClosed, SchedulingInitiator::Caller)
            .unwrap();
    let automatic = InvocationCancellation::new(
        SchedulingCause::RunnerStopped,
        SchedulingInitiator::Automatic,
    )
    .unwrap();
    for mode in [
        SubmissionMode::Immediate,
        SubmissionMode::Queued,
        SubmissionMode::BoundarySteering,
        SubmissionMode::Steering,
    ] {
        let mut history = if mode == SubmissionMode::Immediate {
            InvocationHistory::new(id("run"), mode)
        } else {
            scheduled(mode)
        };
        history.record_local_cancellation(explicit).unwrap();
        history.record_local_cancellation(explicit).unwrap();
        assert!(history.record_local_cancellation(automatic).is_err());
        assert!(history
            .record_provider_result(Some(Ok(ExecutionOutcome::Completed)))
            .is_err());
        assert!(history
            .record_local_result(Ok(ExecutionOutcome::Completed))
            .is_err());
        history
            .record_local_result(Ok(ExecutionOutcome::Cancelled))
            .unwrap();
        history.record_local_cancellation(explicit).unwrap();
        history.record_local_result(Err(())).unwrap();
        history.validate_checkpoint().unwrap();
    }
    let mut pending = InvocationHistory::new(id("run"), SubmissionMode::Queued);
    assert!(pending.record_local_cancellation(automatic).is_err());
    let mut cancelled = InvocationHistory::new(id("run"), SubmissionMode::Immediate);
    cancelled.record_cancellation(explicit).unwrap();
    assert!(cancelled.record_local_cancellation(explicit).is_err());
    for provider in [Ok(ExecutionOutcome::Cancelled), Err(())] {
        let mut history = InvocationHistory::new(id("run"), SubmissionMode::Immediate);
        history.record_provider_result(Some(provider)).unwrap();
        assert!(history.record_local_cancellation(automatic).is_err());
        history.validate_checkpoint().unwrap();
    }
    let mut completed = InvocationHistory::new(id("run"), SubmissionMode::Immediate);
    completed
        .record_local_result(Ok(ExecutionOutcome::Completed))
        .unwrap();
    assert!(completed.record_local_cancellation(automatic).is_err());
    assert_eq!(completed.local_outcome(), Some(ExecutionOutcome::Completed));
}

#[test]
fn local_stop_and_scheduling_cancellation_agree_in_both_recording_orders() {
    for mode in [
        SubmissionMode::Queued,
        SubmissionMode::BoundarySteering,
        SubmissionMode::Steering,
    ] {
        for (cause, initiator, other) in [
            (
                SchedulingCause::SessionClosed,
                SchedulingInitiator::Caller,
                SchedulingCause::RunnerStopped,
            ),
            (
                SchedulingCause::RunnerStopped,
                SchedulingInitiator::Automatic,
                SchedulingCause::SessionClosed,
            ),
        ] {
            let kind = if mode == SubmissionMode::Queued {
                InvocationKind::Queued
            } else {
                InvocationKind::Steering
            };
            let stop = InvocationCancellation::new(cause, initiator).unwrap();
            let other_stop = InvocationCancellation::new(
                other,
                if other == SchedulingCause::SessionClosed {
                    SchedulingInitiator::Caller
                } else {
                    SchedulingInitiator::Automatic
                },
            )
            .unwrap();
            for stop_first in [false, true] {
                let mut history = scheduled(mode);
                let valid = edge(
                    kind,
                    None,
                    Some(InvocationStage::Running),
                    InvocationStage::Cancelled,
                    cause,
                );
                if stop_first {
                    history.record_local_cancellation(stop).unwrap();
                    assert!(history
                        .schedule(edge(
                            kind,
                            None,
                            Some(InvocationStage::Running),
                            InvocationStage::Cancelled,
                            other
                        ))
                        .is_err());
                    history.schedule(valid).unwrap();
                } else {
                    history.schedule(valid).unwrap();
                    assert!(history.record_local_cancellation(other_stop).is_err());
                    history.record_local_cancellation(stop).unwrap();
                }
                history
                    .record_local_result(Ok(ExecutionOutcome::Cancelled))
                    .unwrap();
                history.validate_checkpoint().unwrap();
            }
        }
    }
}
