use super::agents::MemoryStorage;
use super::support::*;
use nessa_sdk::application::agent_execution::hooks::{
    AfterInvocation, AfterInvocationEvent, BeforeInvocation, HookError, HookFailure,
    InvocationContext, InvocationHook,
};
use nessa_sdk::application::agent_execution::providers::ProviderOpenFuture;
use nessa_sdk::application::agent_execution::{agents::Agent, providers::ProviderIdentity};
use std::{future::poll_fn, task::Poll, time::Duration};
use tokio::sync::{watch, Notify};

type Calls = Arc<
    Mutex<
        Vec<(
            usize,
            String,
            String,
            Option<Result<ExecutionOutcome, AgentError>>,
        )>,
    >,
>;

struct RecordingHook {
    index: usize,
    calls: Calls,
    reject: bool,
    fail_after: bool,
    panic_after: bool,
}
impl InvocationHook for RecordingHook {
    fn before_invocation(&self, context: &InvocationContext<'_>) -> Result<(), HookError> {
        self.record(context, None);
        if self.reject {
            Err(HookError::Failed("gate unavailable".into()))
        } else {
            Ok(())
        }
    }
    fn after_invocation(
        &self,
        context: &InvocationContext<'_>,
        result: &Result<ExecutionOutcome, AgentError>,
    ) -> Result<(), HookError> {
        self.record(context, Some(result.clone()));
        assert!(!self.panic_after, "hook panicked");
        if self.fail_after {
            Err(HookError::Failed("notification unavailable".into()))
        } else {
            Ok(())
        }
    }
}
impl RecordingHook {
    fn record(
        &self,
        context: &InvocationContext<'_>,
        result: Option<Result<ExecutionOutcome, AgentError>>,
    ) {
        self.calls.lock().unwrap().push((
            self.index,
            context.session_id.as_str().into(),
            context.request.execution_id.as_str().into(),
            result,
        ));
    }
}
fn hook(index: usize, calls: &Calls) -> RecordingHook {
    RecordingHook {
        index,
        calls: calls.clone(),
        reject: false,
        fail_after: false,
        panic_after: false,
    }
}
fn request(id: &str) -> ExecutionRequest {
    ExecutionRequest {
        execution_id: ExecutionId::new(id).unwrap(),
        user_message: UserMessage::text_only(PromptText::new("hello").unwrap()),
        estimated_input_tokens: 1,
        reserved_output_tokens: 100,
    }
}
struct HookProvider(Arc<dyn ProviderSessionBackend>);
struct EmptyEvents;
impl ExecutionEventStream for EmptyEvents {
    fn next(&mut self) -> ProviderObservationFuture<'_> {
        Box::pin(async { Ok(None) })
    }
}
impl AgentProvider for HookProvider {
    fn identity(&self) -> ProviderIdentity {
        ProviderIdentity::new("hook-fixture", "fixture", "test").unwrap()
    }
    fn capabilities(&self) -> &EffectiveCapabilities {
        capabilities_ref()
    }
    fn open(&self, restore: Option<ExecutionSessionId>) -> ProviderOpenFuture<'_> {
        Box::pin(async move {
            Ok(OpenedProviderSession {
                session: ProviderSession::new(
                    restore.unwrap_or_else(|| ExecutionSessionId::new("context").unwrap()),
                    self.0.clone(),
                    capabilities(),
                ),
                events: Box::new(EmptyEvents),
            })
        })
    }
}
async fn client(
    backend: Arc<dyn ProviderSessionBackend>,
    hooks: Vec<Arc<dyn InvocationHook>>,
) -> Agent {
    let agent = attached_agent(
        Arc::new(HookProvider(backend)),
        MemoryStorage::default().manager().await,
    )
    .await
    .unwrap();
    for hook in hooks {
        agent.add_invocation_hook(hook);
    }
    agent
}
fn recording() -> Arc<RecordingSession> {
    Arc::new(RecordingSession {
        prompts: AtomicUsize::new(0),
    })
}

#[tokio::test]
async fn ordered_hooks_correlate_sequential_calls_and_share_registration_across_clones() {
    let calls = Calls::default();
    let backend = recording();
    let session = client(
        backend.clone(),
        vec![Arc::new(hook(0, &calls)), Arc::new(hook(1, &calls))],
    )
    .await;
    for id in ["first", "second"] {
        assert_eq!(
            session.clone().invoke(request(id), close_action()).await,
            Ok(ExecutionOutcome::Completed)
        );
    }
    assert_eq!(backend.prompts.load(Ordering::SeqCst), 2);
    let expected: Vec<_> = ["first", "second"]
        .into_iter()
        .flat_map(|id| {
            [None, Some(Ok(ExecutionOutcome::Completed))]
                .into_iter()
                .flat_map(move |result| {
                    [0, 1].map(|index| (index, "context".to_owned(), id.to_owned(), result.clone()))
                })
        })
        .collect();
    assert_eq!(*calls.lock().unwrap(), expected);
    client(recording(), vec![])
        .await
        .invoke(request("isolated"), close_action())
        .await
        .unwrap();
    assert_eq!(calls.lock().unwrap().len(), 8);
}

#[tokio::test]
async fn before_failure_stops_dispatch_and_later_hooks_without_changing_backend_state() {
    let calls = Calls::default();
    let backend = recording();
    let mut rejection = hook(1, &calls);
    rejection.reject = true;
    let session = client(
        backend.clone(),
        vec![
            Arc::new(hook(0, &calls)),
            Arc::new(rejection),
            Arc::new(hook(2, &calls)),
        ],
    )
    .await;
    assert_eq!(
        session.invoke(request("rejected"), close_action()).await,
        Err(AgentError::BeforeInvocationHook(HookFailure {
            index: 1,
            error: HookError::Failed("gate unavailable".into())
        }))
    );
    assert_eq!(backend.prompts.load(Ordering::SeqCst), 0);
    assert_eq!(calls.lock().unwrap().len(), 2);
    assert!(calls.lock().unwrap().iter().all(|call| call.3.is_none()));
    session.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn invalid_capabilities_never_reach_hooks_or_backend() {
    let calls = Calls::default();
    let backend = recording();
    let session = client(backend.clone(), vec![Arc::new(hook(0, &calls))]).await;
    let mut input = request("invalid");
    input.reserved_output_tokens = 0;
    assert!(matches!(
        session.invoke(input, close_action()).await,
        Err(AgentError::InvalidInput(_))
    ));
    assert!(calls.lock().unwrap().is_empty());
    assert_eq!(backend.prompts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn after_failures_and_panics_preserve_success_and_notify_remaining_hooks() {
    let calls = Calls::default();
    let mut failed = hook(0, &calls);
    failed.fail_after = true;
    let mut panicked = hook(1, &calls);
    panicked.panic_after = true;
    let session = client(
        recording(),
        vec![
            Arc::new(failed),
            Arc::new(panicked),
            Arc::new(hook(2, &calls)),
        ],
    )
    .await;
    let Err(AgentError::AfterInvocationHooks {
        failures,
        execution_result,
    }) = session.invoke(request("done"), close_action()).await
    else {
        panic!("expected hook failures")
    };
    assert_eq!(*execution_result, Ok(ExecutionOutcome::Completed));
    assert_eq!(
        failures,
        vec![
            HookFailure {
                index: 0,
                error: HookError::Failed("notification unavailable".into())
            },
            HookFailure {
                index: 1,
                error: HookError::Panicked
            }
        ]
    );
    assert_eq!(calls.lock().unwrap().len(), 6);
}

struct FailingBackend(AgentError);
impl ProviderSessionBackend for FailingBackend {
    fn prepare_invocation(&self) -> ProviderOperationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn execute(&self, _: ExecutionRequest) -> ProviderExecutionFuture<'_> {
        Box::pin(async move {
            let result: Result<ExecutionOutcome, AgentError> =
                { async { Err(self.0.clone()) } }.await;
            ProviderExecutionReply::Finished(ExecutionReport::new(
                Some(result),
                None,
                ProviderSessionState::Usable,
            ))
        })
    }
    fn answer_permission(
        &self,
        _: PermissionAnswer,
    ) -> ProviderOperationFuture<'_, PermissionResolution> {
        Box::pin(async {
            Err(ProviderOperationFailure::new(
                AgentError::Closed,
                ProviderSessionState::Usable,
            ))
        })
    }
    fn cancel_permission(
        &self,
        _: PermissionCancellationRequest,
    ) -> ProviderOperationFuture<'_, PermissionCancellation> {
        Box::pin(async {
            Err(ProviderOperationFailure::new(
                AgentError::Closed,
                ProviderSessionState::Usable,
            ))
        })
    }
    fn close(&self, _: SessionCloseRequest) -> CleanupFuture<'_> {
        Box::pin(async { CleanupReport::confirmed(CloseOutcome { forced: false }) })
    }
}

#[tokio::test]
async fn after_hooks_receive_original_admission_lifecycle_and_audit_failures() {
    for error in [
        AgentError::Busy,
        AgentError::Closed,
        AgentError::Deadline,
        AgentError::Backpressure,
        AgentError::AuditFailure,
        AgentError::AuditAndCleanupFailure,
        AgentError::CleanupUncertain,
        AgentError::Provider {
            code: 12,
            diagnostic: None,
        },
    ] {
        let calls = Calls::default();
        let mut failed = hook(0, &calls);
        failed.fail_after = true;
        let session = client(
            Arc::new(FailingBackend(error.clone())),
            vec![Arc::new(failed)],
        )
        .await;
        let Err(AgentError::AfterInvocationHooks {
            execution_result, ..
        }) = session.invoke(request("failed"), close_action()).await
        else {
            panic!("expected retained failure")
        };
        assert_eq!(*execution_result, Err(error.clone()));
        assert_eq!(calls.lock().unwrap()[1].3, Some(Err(error)));
        session.close(close_action()).await.unwrap();
    }
}

#[tokio::test]
async fn successful_hooks_preserve_backend_failure() {
    let calls = Calls::default();
    let session = client(Arc::new(OfflineSession), vec![Arc::new(hook(0, &calls))]).await;
    assert_eq!(
        session.invoke(request("closed"), close_action()).await,
        Err(AgentError::Closed)
    );
    assert_eq!(calls.lock().unwrap()[1].3, Some(Err(AgentError::Closed)));
}

struct CloseSettledBackend {
    started: Notify,
    closed: watch::Sender<bool>,
}
impl ProviderSessionBackend for CloseSettledBackend {
    fn prepare_invocation(&self) -> ProviderOperationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn execute(&self, _: ExecutionRequest) -> ProviderExecutionFuture<'_> {
        Box::pin(async move {
            let result: Result<ExecutionOutcome, AgentError> = {
                async {
                    let mut closed = self.closed.subscribe();
                    self.started.notify_one();
                    while !*closed.borrow_and_update() {
                        closed.changed().await.map_err(|_| AgentError::Closed)?;
                    }
                    Ok(ExecutionOutcome::Cancelled)
                }
            }
            .await;
            ProviderExecutionReply::Finished(ExecutionReport::new(
                Some(result),
                None,
                ProviderSessionState::Usable,
            ))
        })
    }
    fn answer_permission(
        &self,
        _: PermissionAnswer,
    ) -> ProviderOperationFuture<'_, PermissionResolution> {
        Box::pin(async {
            Err(ProviderOperationFailure::new(
                AgentError::Closed,
                ProviderSessionState::Usable,
            ))
        })
    }
    fn cancel_permission(
        &self,
        _: PermissionCancellationRequest,
    ) -> ProviderOperationFuture<'_, PermissionCancellation> {
        Box::pin(async {
            Err(ProviderOperationFailure::new(
                AgentError::Closed,
                ProviderSessionState::Usable,
            ))
        })
    }
    fn close(&self, _: SessionCloseRequest) -> CleanupFuture<'_> {
        Box::pin(async move {
            let result: Result<CloseOutcome, AgentError> = {
                async {
                    // Confirmed cleanup must release admitted execution, even when the
                    // event stream is exhausted and the caller has dropped its waiter.
                    self.closed.send_replace(true);
                    Ok(CloseOutcome { forced: false })
                }
            }
            .await;
            match result {
                Ok(outcome) => CleanupReport::confirmed(outcome),
                Err(error) => CleanupReport::unconfirmed(error),
            }
        })
    }
}

#[tokio::test]
async fn dropping_the_wait_preserves_hooks_until_confirmed_close_settlement() {
    let calls = Calls::default();
    let backend = Arc::new(CloseSettledBackend {
        started: Notify::new(),
        closed: watch::channel(false).0,
    });
    let session = client(backend.clone(), vec![Arc::new(hook(0, &calls))]).await;
    let mut execution = session.invoke(request("pending"), close_action());
    poll_fn(|cx| {
        assert!(execution.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    backend.started.notified().await;
    assert_eq!(calls.lock().unwrap().len(), 1);
    drop(execution);
    assert_eq!(calls.lock().unwrap()[0].3, None);
    assert_eq!(
        session
            .session_manager()
            .snapshot()
            .await
            .unwrap()
            .invocations[0]
            .result,
        None
    );
    tokio::time::timeout(Duration::from_secs(2), session.close(close_action()))
        .await
        .unwrap()
        .unwrap();
    {
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].3, None);
        assert_eq!(calls[1].3, Some(Ok(ExecutionOutcome::Cancelled)));
    }
    assert_eq!(
        session
            .session_manager()
            .snapshot()
            .await
            .unwrap()
            .invocations[0]
            .result,
        Some(Ok(ExecutionOutcome::Cancelled))
    );
}

struct PanickingBefore;
impl InvocationHook for PanickingBefore {
    fn before_invocation(&self, _: &InvocationContext<'_>) -> Result<(), HookError> {
        panic!("before hook panicked");
    }
}

#[tokio::test]
async fn before_panic_is_a_typed_failure_without_dispatch() {
    let backend = recording();
    let session = client(backend.clone(), vec![Arc::new(PanickingBefore)]).await;
    assert_eq!(
        session.invoke(request("panic"), close_action()).await,
        Err(AgentError::BeforeInvocationHook(HookFailure {
            index: 0,
            error: HookError::Panicked
        }))
    );
    assert_eq!(backend.prompts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn typed_callback_registration_observes_the_invocation_and_saved_result() {
    let agent = client(recording(), vec![]).await;
    let calls = Arc::new(Mutex::new(Vec::new()));
    let before_calls = calls.clone();
    agent.add_hook(BeforeInvocation, move |context: &InvocationContext<'_>| {
        before_calls
            .lock()
            .unwrap()
            .push(("before", context.request.execution_id.clone(), None));
        Ok(())
    });
    let after_calls = calls.clone();
    agent.add_hook(AfterInvocation, move |event: &AfterInvocationEvent<'_>| {
        after_calls.lock().unwrap().push((
            "after",
            event.context.request.execution_id.clone(),
            Some(event.result.clone()),
        ));
        Ok(())
    });
    agent
        .invoke(request("callbacks"), close_action())
        .await
        .unwrap();
    assert_eq!(
        *calls.lock().unwrap(),
        [
            ("before", ExecutionId::new("callbacks").unwrap(), None),
            (
                "after",
                ExecutionId::new("callbacks").unwrap(),
                Some(Ok(ExecutionOutcome::Completed))
            ),
        ]
    );
    assert_eq!(
        agent
            .session_manager()
            .snapshot()
            .await
            .unwrap()
            .invocations[0]
            .result,
        Some(Ok(ExecutionOutcome::Completed))
    );
}

struct ObservationFailureProvider;
struct FailedEvents(Arc<RecordingSession>);
impl ExecutionEventStream for FailedEvents {
    fn next(&mut self) -> ProviderObservationFuture<'_> {
        Box::pin(async {
            // This fixture produces invocation observations only after execute starts.
            if self.0.prompts.load(Ordering::SeqCst) == 0 {
                return std::future::pending().await;
            }
            Err(ObservationFailure::new(
                AgentError::Transport("reader failed after settlement".into()),
                ObservationFailureCause::ExecutionFailed,
            ))
        })
    }
}
impl AgentProvider for ObservationFailureProvider {
    fn identity(&self) -> ProviderIdentity {
        ProviderIdentity::new("observation-fixture", "test", "test").unwrap()
    }
    fn capabilities(&self) -> &EffectiveCapabilities {
        capabilities_ref()
    }
    fn open(&self, _: Option<ExecutionSessionId>) -> ProviderOpenFuture<'_> {
        Box::pin(async {
            let backend = Arc::new(RecordingSession {
                prompts: AtomicUsize::new(0),
            });
            Ok(OpenedProviderSession {
                session: ProviderSession::new(
                    ExecutionSessionId::new("context").unwrap(),
                    backend.clone(),
                    capabilities(),
                ),
                events: Box::new(FailedEvents(backend)),
            })
        })
    }
}

#[tokio::test]
async fn observation_failure_retains_confirmed_execution_in_storage_and_after_hook() {
    let storage = MemoryStorage::default();
    let agent = attached_agent(
        Arc::new(ObservationFailureProvider),
        storage.manager().await,
    )
    .await
    .unwrap();
    let calls = Calls::default();
    agent.add_invocation_hook(Arc::new(hook(0, &calls)));
    let result = agent.invoke(request("confirmed"), close_action()).await;
    assert!(
        matches!(&result, Err(AgentError::ExecutionObservation { execution_result: Some(confirmed), .. }) if **confirmed == Ok(ExecutionOutcome::Completed))
    );
    assert_eq!(
        storage.snapshot().invocations[0].result,
        Some(result.clone())
    );
    assert_eq!(calls.lock().unwrap()[1].3, Some(result));
    agent.close(close_action()).await.unwrap();
}
