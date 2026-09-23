//! Storage preserves provider outcomes, local cancellation, and cleanup independently.
use super::*;
use nessa_sdk::application::agent_execution::providers::{
    CleanupReport, ExecutionReport, ExecutionReportSource, ProviderSessionState, ResourceCleanup,
};
use nessa_sdk::domain::agent_execution::executions::SchedulingCause;

#[tokio::test]
async fn provider_diagnostic_presence_and_text_round_trip_exactly() {
    let root = tempfile::tempdir().unwrap();
    let stores: Vec<Arc<dyn SessionStorage>> = vec![
        Arc::new(InMemoryStorage::new()),
        Arc::new(LocalFileStorage::new(root.path().join("provider-diagnostic")).unwrap()),
    ];
    for storage in stores {
        let lease = storage
            .open(SessionId::new("provider-diagnostic").unwrap())
            .await
            .unwrap();
        for diagnostic in [
            None,
            Some(ProviderDiagnostic::new("")),
            Some(ProviderDiagnostic::new("provider quota was exhausted")),
        ] {
            let expected = AgentError::Provider {
                code: -32000,
                diagnostic,
            };
            let mut value = snapshot("provider-diagnostic");
            value.invocations[0].events.clear();
            value.invocations[0].result = Some(Err(expected.clone()));
            lease.save(value).await.unwrap();
            let restored = lease.load().await.unwrap().unwrap();
            assert_eq!(restored.invocations[0].result, Some(Err(expected)));
        }
    }
}

#[tokio::test]
async fn explicit_settlement_facts_round_trip_all_outcomes_and_cleanup_statuses() {
    let root = tempfile::tempdir().unwrap();
    let stores: Vec<Arc<dyn SessionStorage>> = vec![
        Arc::new(InMemoryStorage::new()),
        Arc::new(LocalFileStorage::new(root.path().join("private")).unwrap()),
    ];
    let outcomes = [
        ExecutionOutcome::Completed,
        ExecutionOutcome::Cancelled,
        ExecutionOutcome::Refused,
        ExecutionOutcome::OutputLimit,
        ExecutionOutcome::RequestLimit,
    ];
    for storage in stores {
        let lease = storage
            .open(SessionId::new("settlement-facts").unwrap())
            .await
            .unwrap();
        let mut reports = Vec::new();
        let resources = [
            ResourceCleanup::Confirmed(CloseOutcome { forced: false }),
            ResourceCleanup::Confirmed(CloseOutcome { forced: true }),
            ResourceCleanup::Unconfirmed(AgentError::CleanupUncertain),
        ];
        let mut attachments = vec![
            ProviderSessionState::Usable,
            ProviderSessionState::CleanupRequired,
        ];
        for resources in resources {
            for audit in [Ok(()), Err(AgentError::AuditFailure)] {
                for operation in [
                    None,
                    Some(AgentError::Transport("failed command write".into())),
                ] {
                    for completion in [None, Some(AgentError::CleanupUncertain)] {
                        let cleanup = CleanupReport::new(resources.clone(), audit.clone())
                            .with_operation_failure(operation.clone())
                            .with_completion_failure(completion);
                        reports.push(ExecutionReport::cancelled_locally(cleanup.clone()));
                        attachments.push(ProviderSessionState::CleanupReported(cleanup));
                    }
                }
            }
        }
        for attachment in attachments {
            for result in outcomes
                .into_iter()
                .map(Ok)
                .chain([Err(AgentError::Transport("provider failure".into()))])
                .map(Some)
                .chain([None])
            {
                for failure in [None, Some(AgentError::AuditFailure)] {
                    reports.push(ExecutionReport::new(
                        result.clone(),
                        failure,
                        attachment.clone(),
                    ));
                }
            }
        }
        for report in reports {
            let mut value = snapshot("settlement-facts");
            value.invocations[0].events.clear();
            // A later local failure cannot erase an earlier provider outcome.
            value.invocations[0].result =
                Some(Err(AgentError::Transport("later local failure".into())));
            value.invocations[0].provider_report = Some(report.clone());
            if report.source() == ExecutionReportSource::LocalCancellation {
                value.invocations[0].local_cancellation = Some(InvocationCancellationEvent {
                    cause: SchedulingCause::RunnerStopped,
                    actor: None,
                });
            }
            lease.save(value).await.unwrap();
            let restored = lease.load().await.unwrap().unwrap();
            assert_eq!(restored.invocations[0].provider_report, Some(report));
            assert_eq!(
                restored.invocations[0].result,
                Some(Err(AgentError::Transport("later local failure".into())))
            );
        }
    }
}

#[tokio::test]
async fn confirmed_cleanup_supervision_failure_survives_restoration() {
    let root = tempfile::tempdir().unwrap();
    let stores: Vec<Arc<dyn SessionStorage>> = vec![
        Arc::new(InMemoryStorage::new()),
        Arc::new(LocalFileStorage::new(root.path().join("cleanup-supervision")).unwrap()),
    ];
    for storage in stores {
        let lease = storage
            .open(SessionId::new("cleanup-supervision").unwrap())
            .await
            .unwrap();
        let report = CleanupReport::confirmed(CloseOutcome { forced: false })
            .with_completion_failure(Some(AgentError::CleanupUncertain));
        let mut value = snapshot("cleanup-supervision");
        value.invocations[0].events.clear();
        value.invocations[0].result = Some(Err(AgentError::CleanupUncertain));
        value.invocations[0].local_cancellation = Some(InvocationCancellationEvent {
            cause: SchedulingCause::SessionClosed,
            actor: None,
        });
        value.invocations[0].provider_report =
            Some(ExecutionReport::cancelled_locally(report.clone()));
        lease.save(value).await.unwrap();

        let restored = lease.load().await.unwrap().unwrap();
        let restored = restored.invocations[0]
            .provider_report
            .clone()
            .expect("restored cleanup report");
        assert_eq!(
            restored.session_state(),
            &ProviderSessionState::CleanupReported(report)
        );
        assert_eq!(restored.into_result(), Err(AgentError::CleanupUncertain));
    }
}

/// A saved startup failure must keep the step it names. Collapsing every phase
/// onto one label would make a restored record unable to say whether saved
/// context was being restored when the budget ran out.
#[tokio::test]
async fn saved_startup_deadlines_retain_the_step_that_expired() {
    let root = tempfile::tempdir().unwrap();
    let stores: Vec<Arc<dyn SessionStorage>> = vec![
        Arc::new(InMemoryStorage::new()),
        Arc::new(LocalFileStorage::new(root.path().join("private")).unwrap()),
    ];
    for storage in stores {
        let lease = storage
            .open(SessionId::new("startup-deadline-phase").unwrap())
            .await
            .unwrap();
        for phase in [
            AgentStartupPhase::Initialize,
            AgentStartupPhase::Session,
            AgentStartupPhase::Configure,
        ] {
            // Both facts must survive independently: a restoration can expire
            // during any step, so the context is not recoverable from the step.
            for context in [AgentStartupContext::New, AgentStartupContext::Restored] {
                let step = AgentStartupStep::new(phase, context);
                let mut value = snapshot("startup-deadline-phase");
                value.invocations[0].events.clear();
                value.invocations[0].result = Some(Err(AgentError::StartupDeadline(step)));
                lease.save(value).await.unwrap();
                let restored = lease.load().await.unwrap().unwrap();
                assert_eq!(
                    restored.invocations[0].result,
                    Some(Err(AgentError::StartupDeadline(step)))
                );
                let Some(Err(AgentError::StartupDeadline(saved))) = restored.invocations[0].result
                else {
                    panic!("startup deadline retained")
                };
                assert_eq!(saved.phase(), phase);
                assert_eq!(saved.context(), context);
            }
        }
    }
}

#[tokio::test]
async fn successful_local_result_cannot_contradict_independent_provider_facts() {
    let storage = InMemoryStorage::new();
    let lease = storage
        .open(SessionId::new("settlement-conflict").unwrap())
        .await
        .unwrap();
    let mut value = snapshot("settlement-conflict");
    value.invocations[0].events.clear();
    value.invocations[0].provider_report = Some(ExecutionReport::new(
        Some(Ok(ExecutionOutcome::Refused)),
        None,
        ProviderSessionState::Usable,
    ));
    value.invocations[0].result = Some(Ok(ExecutionOutcome::Completed));
    assert!(matches!(
        lease.save(value.clone()).await,
        Err(StorageError::Corrupt(_))
    ));
    value.invocations[0].result = Some(Ok(ExecutionOutcome::Refused));
    lease.save(value).await.unwrap();
    assert_eq!(
        lease.load().await.unwrap().unwrap().invocations[0].result,
        Some(Ok(ExecutionOutcome::Refused))
    );
}

#[tokio::test]
async fn contradictory_provider_observations_are_retained_as_failed_evidence() {
    let storage = InMemoryStorage::new();
    let lease = storage
        .open(SessionId::new("provider-conflict").unwrap())
        .await
        .unwrap();
    let mut value = snapshot("provider-conflict");
    let id = value.invocations[0].request.execution_id.clone();
    value.invocations[0].events = vec![ExecutionEvent::new(
        id,
        ExecutionUpdate::Finished(ExecutionOutcome::Completed),
    )];
    value.invocations[0].provider_report = Some(ExecutionReport::new(
        Some(Ok(ExecutionOutcome::Refused)),
        None,
        ProviderSessionState::Usable,
    ));
    for result in [
        None,
        Some(Err(AgentError::Protocol(
            "conflicting provider observations".into(),
        ))),
    ] {
        value.invocations[0].result = result;
        lease.save(value.clone()).await.unwrap();
        let restored = lease.load().await.unwrap().unwrap();
        assert_eq!(restored.invocations[0].events, value.invocations[0].events);
        assert_eq!(
            restored.invocations[0].provider_report,
            value.invocations[0].provider_report
        );
    }
    for outcome in [ExecutionOutcome::Completed, ExecutionOutcome::Refused] {
        value.invocations[0].result = Some(Ok(outcome));
        assert!(matches!(
            lease.save(value.clone()).await,
            Err(StorageError::Corrupt(_))
        ));
    }
}

#[tokio::test]
async fn undispatched_saved_input_cannot_claim_execution_settlement() {
    let storage = InMemoryStorage::new();
    let lease = storage
        .open(SessionId::new("undispatched-report").unwrap())
        .await
        .unwrap();
    let mut value = snapshot("undispatched-report");
    let record = &mut value.invocations[0];
    record.submission = SubmissionMode::Queued;
    record.events.clear();
    record.result = None;
    record.scheduling = vec![InvocationSchedulingEvent {
        kind: InvocationKind::Queued,
        target: None,
        before: None,
        stage: InvocationStage::Queued,
        cause: SchedulingCause::Submitted,
        actor: Some(record.actor.clone()),
    }];
    record.provider_report = Some(ExecutionReport::new(
        None,
        None,
        ProviderSessionState::Usable,
    ));
    assert!(matches!(
        lease.save(value).await,
        Err(StorageError::Corrupt(_))
    ));
}

#[tokio::test]
async fn local_settlement_requires_matching_stop_at_every_storage_boundary() {
    let root = tempfile::tempdir().unwrap();
    let stores: Vec<Arc<dyn SessionStorage>> = vec![
        Arc::new(InMemoryStorage::new()),
        Arc::new(LocalFileStorage::new(root.path().join("local-stop")).unwrap()),
    ];
    for storage in stores {
        let lease = storage.open(id("stop-evidence")).await.unwrap();
        let mut valid = snapshot("stop-evidence");
        valid.invocations[0].events.clear();
        valid.invocations[0].provider_report = Some(ExecutionReport::cancelled_locally(
            CleanupReport::confirmed(CloseOutcome { forced: false }),
        ));
        valid.invocations[0].local_cancellation = Some(InvocationCancellationEvent {
            cause: SchedulingCause::SessionClosed,
            actor: Some(ActionContext::new("closer", "phone", "close").unwrap()),
        });
        valid.invocations[0].result = Some(Ok(ExecutionOutcome::Cancelled));
        lease.save(valid.clone()).await.unwrap();
        let restored = lease.load().await.unwrap().unwrap();
        assert_eq!(
            restored.invocations[0].local_cancellation,
            valid.invocations[0].local_cancellation
        );
        custom_storage::assert_custom_retention_admission(valid.clone(), true).await;
        for change in 0..5 {
            let mut invalid = valid.clone();
            let record = &mut invalid.invocations[0];
            match change {
                0 => record.local_cancellation = None,
                1 => record.local_cancellation.as_mut().unwrap().actor = None,
                2 => record.provider_report = None,
                3 => {
                    record.provider_report = Some(ExecutionReport::new(
                        Some(Ok(ExecutionOutcome::Cancelled)),
                        None,
                        ProviderSessionState::Usable,
                    ))
                }
                _ => record.result = Some(Ok(ExecutionOutcome::Completed)),
            }
            assert!(
                matches!(
                    lease.save(invalid.clone()).await,
                    Err(StorageError::Corrupt(_))
                ),
                "change {change}"
            );
            let unchanged = lease.load().await.unwrap().unwrap();
            assert_eq!(
                unchanged.invocations[0].local_cancellation,
                valid.invocations[0].local_cancellation
            );
            assert_eq!(
                unchanged.invocations[0].provider_report,
                valid.invocations[0].provider_report
            );
            custom_storage::assert_custom_retention_admission(invalid, false).await;
        }
    }
}

#[tokio::test]
async fn scheduled_local_settlement_preserves_one_cause_and_exact_closer() {
    let root = tempfile::tempdir().unwrap();
    let stores: Vec<Arc<dyn SessionStorage>> = vec![
        Arc::new(InMemoryStorage::new()),
        Arc::new(LocalFileStorage::new(root.path().join("scheduled-stop")).unwrap()),
    ];
    for storage in stores {
        for mode in [
            SubmissionMode::Queued,
            SubmissionMode::BoundarySteering,
            SubmissionMode::Steering,
        ] {
            let name = format!("scheduled-stop-{mode:?}");
            let lease = storage.open(id(&name)).await.unwrap();
            let closer = ActionContext::new("closer", "phone", "close").unwrap();
            let kind = if mode == SubmissionMode::Queued {
                InvocationKind::Queued
            } else {
                InvocationKind::Steering
            };
            let mut value = snapshot(&name);
            let record = &mut value.invocations[0];
            record.events.clear();
            record.submission = mode;
            record.scheduling = vec![
                InvocationSchedulingEvent {
                    kind,
                    target: None,
                    before: None,
                    stage: InvocationStage::Queued,
                    cause: SchedulingCause::Submitted,
                    actor: Some(record.actor.clone()),
                },
                InvocationSchedulingEvent {
                    kind,
                    target: None,
                    before: Some(InvocationStage::Queued),
                    stage: InvocationStage::Running,
                    cause: SchedulingCause::Dispatched,
                    actor: None,
                },
                InvocationSchedulingEvent {
                    kind,
                    target: None,
                    before: Some(InvocationStage::Running),
                    stage: InvocationStage::Cancelled,
                    cause: SchedulingCause::SessionClosed,
                    actor: Some(closer.clone()),
                },
            ];
            record.provider_report = Some(ExecutionReport::cancelled_locally(
                CleanupReport::confirmed(CloseOutcome { forced: false }),
            ));
            record.local_cancellation = Some(InvocationCancellationEvent {
                cause: SchedulingCause::SessionClosed,
                actor: Some(closer),
            });
            record.result = Some(Ok(ExecutionOutcome::Cancelled));
            let index = if mode == SubmissionMode::Steering {
                let mut target = snapshot(&name).invocations.remove(0);
                let target_id = ExecutionId::new("active-target").unwrap();
                target.request.execution_id = target_id.clone();
                target.events.clear();
                for edge in &mut value.invocations[0].scheduling {
                    edge.target = Some(target_id.clone());
                }
                value.invocations.insert(0, target);
                1
            } else {
                0
            };
            super::scheduling::fixture_dispatches(&mut value);
            lease.save(value.clone()).await.unwrap();
            custom_storage::assert_custom_retention_admission(value.clone(), true).await;
            for change in 0..3 {
                let mut invalid = value.clone();
                let record = &mut invalid.invocations[index];
                match change {
                    0 => {
                        record.local_cancellation = Some(InvocationCancellationEvent {
                            cause: SchedulingCause::RunnerStopped,
                            actor: None,
                        })
                    }
                    1 => {
                        let last = record.scheduling.last_mut().unwrap();
                        last.cause = SchedulingCause::RunnerStopped;
                        last.actor = None;
                    }
                    _ => {
                        record.local_cancellation.as_mut().unwrap().actor =
                            Some(ActionContext::new("other", "pane", "close").unwrap())
                    }
                }
                assert!(
                    matches!(
                        lease.save(invalid.clone()).await,
                        Err(StorageError::Corrupt(_))
                    ),
                    "{mode:?} change {change}"
                );
                custom_storage::assert_custom_retention_admission(invalid, false).await;
                assert_eq!(
                    lease.load().await.unwrap().unwrap().invocations[index].local_cancellation,
                    value.invocations[index].local_cancellation
                );
            }
        }
    }
}
