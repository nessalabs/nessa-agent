//! An exhausted invocation cannot acquire fresh output during a later run.
use super::*;

#[tokio::test]
async fn settled_observation_boundary_rejects_old_output_in_each_delivery_mode() {
    for mode in Mode::ALL {
        for update in [
            ExecutionUpdate::Message(MessageChunk::text("stale text")),
            ExecutionUpdate::Message(MessageChunk::thought("stale thought")),
            ExecutionUpdate::Finished(ExecutionOutcome::Completed),
        ] {
            let (agent, backend, storage) = workflow().await;
            // No terminal observation: this reply ends A's stream with Ok(None).
            *backend.execution_report.lock().unwrap() = Some(ExecutionReport::new(
                Some(Ok(ExecutionOutcome::Completed)),
                None,
                ProviderSessionState::Usable,
            ));
            assert_eq!(
                bounded(mode.start(agent.clone(), request("first")))
                    .await
                    .unwrap(),
                Ok(ExecutionOutcome::Completed)
            );
            assert!(storage.snapshot().invocations[0].events.is_empty());
            *backend.execution_report.lock().unwrap() = None;
            let (_release, gate) = oneshot::channel();
            *backend.execution_gate.lock().unwrap() = Some(gate);
            // Consume the first invocation's notification before waiting for B.
            bounded(backend.dispatched.notified()).await;
            let running = mode.start(agent.clone(), request("second"));
            bounded(backend.dispatched.notified()).await;
            backend
                .output
                .send(Some(ExecutionEvent::new(
                    ExecutionId::new("first").unwrap(),
                    update,
                )))
                .unwrap();
            let result = bounded(running).await.unwrap();
            assert!(
                matches!(&result, Err(AgentError::ExecutionObservation { error, .. }) if matches!(error.as_ref(), AgentError::Protocol(_))),
                "mode={mode:?}, result={result:?}"
            );
            let snapshot = storage.snapshot();
            assert!(snapshot.invocations[0].events.is_empty());
            assert_eq!(
                snapshot.invocations[0].result,
                Some(Ok(ExecutionOutcome::Completed))
            );
            assert_eq!(snapshot.invocations[1].result, Some(result));
            assert!(matches!(
                backend.shutdowns.lock().unwrap().first(),
                Some(SessionCloseRequest::ExecutionFailed)
            ));
            bounded(agent.close(actor())).await.unwrap();
        }
    }
}

#[tokio::test]
async fn settled_observation_boundary_keeps_only_correlated_trailing_cancellation() {
    let (agent, backend, storage) = workflow().await;
    let execution = ExecutionId::new("first").unwrap();
    let tool = ToolCallId::new("reviewed-tool").unwrap();
    let decision = PermissionDecision::new(PermissionEffect::Allow, PermissionScope::request());
    let options = PermissionOptions::new(
        vec![PermissionOption::new(
            PermissionOptionId::new("allow").unwrap(),
            "Allow",
            decision.clone(),
        )
        .unwrap()],
        &PermissionOfferPolicy::new(vec![decision]).unwrap(),
    )
    .unwrap();
    let review = PermissionRequest::new(
        PermissionId::new("review").unwrap(),
        execution.clone(),
        tool.clone(),
        options.clone(),
    );
    let review_input = ToolReviewInput {
        name: "Read".into(),
        arguments_json: "{}".into(),
    };
    let mut domain = ExecutionSession::new(ExecutionSessionId::new("workflow-context").unwrap());
    domain.begin_execution(execution.clone()).unwrap();
    let tool_update = ToolCallUpdate::new(tool.clone(), None, None, None, None, None);
    domain
        .observe_tool(&execution, tool_update.clone())
        .unwrap();
    domain.request_permission(review).unwrap();
    let cancelled = domain
        .finish_execution(&execution, Ok(ExecutionOutcome::Completed))
        .unwrap()
        .1
        .remove(0);
    let cancellation = PermissionCancellation::from_record(
        domain.id().clone(),
        cancelled,
        review_input.clone(),
        CancellationOrigin::Runtime,
    )
    .unwrap();
    let (_release, gate) = oneshot::channel();
    *backend.execution_gate.lock().unwrap() = Some(gate);
    let first = Mode::Direct.start(agent.clone(), request("first"));
    bounded(backend.dispatched.notified()).await;
    for update in [
        ExecutionUpdate::Tool(tool_update),
        ExecutionUpdate::PermissionRequested {
            id: PermissionId::new("review").unwrap(),
            tool_id: tool,
            options,
            input: review_input,
            observation: Default::default(),
        },
    ] {
        backend
            .output
            .send(Some(ExecutionEvent::new(execution.clone(), update)))
            .unwrap();
    }
    // Release normal completion after the ready review observations have drained.
    _release.send(()).unwrap();
    assert_eq!(
        bounded(first).await.unwrap(),
        Ok(ExecutionOutcome::Completed)
    );
    backend
        .output
        .send(Some(ExecutionEvent::new(
            execution,
            ExecutionUpdate::PermissionCancelled(cancellation),
        )))
        .unwrap();
    assert_eq!(
        bounded(agent.invoke(request("second"), actor())).await,
        Ok(ExecutionOutcome::Completed)
    );
    let saved = storage.snapshot();
    assert!(matches!(
        saved.invocations[0].events.last().unwrap().update(),
        ExecutionUpdate::PermissionCancelled(_)
    ));
    assert_eq!(
        saved.invocations[0].result,
        Some(Ok(ExecutionOutcome::Completed))
    );
    bounded(agent.close(actor())).await.unwrap();
}

#[tokio::test]
async fn buffered_stale_output_prevents_next_provider_dispatch() {
    for mode in Mode::ALL {
        for exhausted_budget in [false, true] {
            for update in [
                ExecutionUpdate::Message(MessageChunk::text("buffered stale text")),
                ExecutionUpdate::Message(MessageChunk::thought("buffered stale thought")),
                ExecutionUpdate::Finished(ExecutionOutcome::Completed),
            ] {
                let (agent, backend, storage) = workflow().await;
                *backend.execution_report.lock().unwrap() = Some(ExecutionReport::new(
                    Some(Ok(ExecutionOutcome::Completed)),
                    None,
                    ProviderSessionState::Usable,
                ));
                assert_eq!(
                    bounded(mode.start(agent.clone(), request("first")))
                        .await
                        .unwrap(),
                    Ok(ExecutionOutcome::Completed)
                );
                *backend.execution_report.lock().unwrap() = None;
                backend
                    .exhaust_prepare_budget
                    .store(usize::from(exhausted_budget), Ordering::SeqCst);
                backend
                    .output
                    .send(Some(ExecutionEvent::new(
                        ExecutionId::new("first").unwrap(),
                        update,
                    )))
                    .unwrap();
                let result = bounded(mode.start(agent.clone(), request("second")))
                    .await
                    .unwrap();
                assert!(result.is_err(), "{mode:?}: {result:?}");
                assert_eq!(
                    backend.executions.lock().unwrap().len(),
                    1,
                    "buffered violation must precede dispatch"
                );
                let snapshot = storage.snapshot();
                assert!(snapshot.invocations[0].events.is_empty());
                assert_eq!(
                    snapshot.invocations[0].result,
                    Some(Ok(ExecutionOutcome::Completed))
                );
                assert_eq!(snapshot.invocations[1].result, Some(result));
                assert!(snapshot.invocations[1].provider_report.is_none());
                assert!(matches!(
                    backend.shutdowns.lock().unwrap().first(),
                    Some(SessionCloseRequest::ExecutionFailed)
                ));
                bounded(agent.close(actor())).await.unwrap();
            }
        }
    }
}

struct FailedPreflight {
    backend: Arc<WorkflowBackend>,
    cause: ObservationFailureCause,
}
struct FailedPreflightEvents(ObservationFailureCause);
impl AgentProvider for FailedPreflight {
    fn identity(&self) -> ProviderIdentity {
        ProviderIdentity::new("preflight", "fixture", "local").unwrap()
    }
    fn open(&self, _: Option<ExecutionSessionId>) -> ProviderOpenFuture<'_> {
        Box::pin(async {
            Ok(OpenedProviderSession {
                session: ProviderSession::new(
                    ExecutionSessionId::new("preflight").unwrap(),
                    self.backend.clone(),
                    capabilities(),
                ),
                events: Box::new(FailedPreflightEvents(self.cause)),
            })
        })
    }
}
impl ExecutionEventStream for FailedPreflightEvents {
    fn next(&mut self) -> ProviderObservationFuture<'_> {
        Box::pin(async {
            Err(ObservationFailure::new(
                AgentError::Transport("reader unavailable".into()),
                self.0,
            ))
        })
    }
}

#[tokio::test]
async fn preflight_reader_failure_keeps_its_cause_without_dispatch() {
    for mode in Mode::ALL {
        for cause in [
            ObservationFailureCause::DeadlineExceeded,
            ObservationFailureCause::ExecutionFailed,
        ] {
            let storage = MemoryStorage::default();
            let backend = workflow_backend();
            let agent = Agent::new(
                Arc::new(FailedPreflight {
                    backend: backend.clone(),
                    cause,
                }),
                storage.manager().await,
            )
            .await
            .unwrap();
            let result = bounded(mode.start(agent.clone(), request("not-dispatched")))
                .await
                .unwrap();
            assert_eq!(
                result,
                Err(AgentError::Transport("reader unavailable".into()))
            );
            assert!(backend.executions.lock().unwrap().is_empty());
            assert_eq!(
                backend.shutdowns.lock().unwrap()[0],
                match cause {
                    ObservationFailureCause::DeadlineExceeded =>
                        SessionCloseRequest::DeadlineExceeded,
                    ObservationFailureCause::ExecutionFailed =>
                        SessionCloseRequest::ExecutionFailed,
                }
            );
            let saved = storage.snapshot();
            assert_eq!(saved.invocations[0].result, Some(result));
            assert!(saved.invocations[0].provider_report.is_none());
            assert!(saved.invocations[0].events.is_empty());
            bounded(agent.close(actor())).await.unwrap();
        }
    }
}
