//! Shared public-workflow assertions against a conforming, checkpoint-driven provider.
//! Default cleanup releases owned execution; explicit fault reports vary resource
//! status independently of diagnostic text. Native input keeps its target's outcome.
use super::*;
use std::{
    future::{poll_fn, Future},
    sync::atomic::AtomicBool,
    task::Poll,
};
use tokio::{sync::Notify, task::JoinHandle, time::timeout};

#[derive(Clone, Copy, Debug)]
enum Mode {
    Direct,
    Queued,
    Boundary,
}
impl Mode {
    const ALL: [Self; 3] = [Self::Direct, Self::Queued, Self::Boundary];

    fn start(
        self,
        agent: Agent,
        input: ExecutionRequest,
    ) -> JoinHandle<Result<ExecutionOutcome, AgentError>> {
        tokio::spawn(async move {
            match self {
                Self::Direct => agent.invoke(input, actor()).await,
                Self::Queued => agent.enqueue(input, actor()).await?.wait().await,
                Self::Boundary => agent.enqueue_steering(input, actor()).await?.wait().await,
            }
        })
    }
}

struct WorkflowProvider(Arc<WorkflowBackend>);
#[derive(Default)]
struct WorkflowAudit {
    records: Mutex<Vec<ExecutionAuditRecord>>,
    reject: AtomicBool,
    panic: AtomicBool,
    pause: Mutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>,
}
impl ExecutionAudit for WorkflowAudit {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        Box::pin(async move {
            self.records.lock().unwrap().push(record);
            if self.panic.load(Ordering::SeqCst) {
                panic!("audit acknowledgement panicked");
            }
            let pause = self.pause.lock().unwrap().take();
            if let Some((entered, release)) = pause {
                let _ = entered.send(());
                let _ = release.await;
            }
            if self.reject.load(Ordering::SeqCst) {
                Err(AgentError::AuditFailure)
            } else {
                Ok(())
            }
        })
    }
}
struct WorkflowEvents(mpsc::UnboundedReceiver<Option<ExecutionEvent>>);
struct WorkflowBackend {
    audit: Arc<WorkflowAudit>,
    output: mpsc::UnboundedSender<Option<ExecutionEvent>>,
    receiver: Mutex<Option<mpsc::UnboundedReceiver<Option<ExecutionEvent>>>>,
    dispatched: Notify,
    execution_gate: Mutex<Option<oneshot::Receiver<()>>>,
    target: Mutex<Option<ExecutionId>>,
    executions: Mutex<Vec<ExecutionId>>,
    steering: Mutex<Vec<(ExecutionId, ExecutionId)>>,
    steering_admitted: Notify,
    steering_gate: Mutex<Option<oneshot::Receiver<()>>>,
    closed: watch::Sender<bool>,
    cleanup: Mutex<CleanupReport>,
    execution_fault: Mutex<Option<ProviderOperationFailure>>,
    execution_report: Mutex<Option<ExecutionReport>>,
    exhaust_prepare_budget: AtomicUsize,
    shutdowns: Mutex<Vec<SessionCloseRequest>>,
}
impl AgentProvider for WorkflowProvider {
    fn identity(&self) -> ProviderIdentity {
        ProviderIdentity::new("workflow-conformance", "fixture", "local").unwrap()
    }
    fn open(&self, _: Option<ExecutionSessionId>) -> ProviderOpenFuture<'_> {
        Box::pin(async {
            Ok(OpenedProviderSession {
                session: ProviderSession::new(
                    ExecutionSessionId::new("workflow-context").unwrap(),
                    self.0.clone(),
                    capabilities(),
                    self.0.audit.clone(),
                ),
                events: Box::new(WorkflowEvents(
                    self.0.receiver.lock().unwrap().take().unwrap(),
                )),
            })
        })
    }
}
impl ExecutionEventStream for WorkflowEvents {
    fn next(&mut self) -> ProviderObservationFuture<'_> {
        Box::pin(async { Ok(self.0.recv().await.flatten()) })
    }
}
impl ProviderSessionBackend for WorkflowBackend {
    fn operation_capabilities(&self) -> OperationCapabilities {
        OperationCapabilities {
            native_steering: true,
            session_resume: true,
            image_input: false,
        }
    }
    fn prepare_invocation(&self) -> ProviderOperationFuture<'_, ()> {
        Box::pin(async {
            self.closed.send_replace(false);
            if self.exhaust_prepare_budget.load(Ordering::SeqCst) != 0 {
                poll_fn(|cx| {
                    for _ in 0..256 {
                        let mut budget = Box::pin(tokio::task::consume_budget());
                        if budget.as_mut().poll(cx).is_pending() {
                            return Poll::Ready(());
                        }
                    }
                    panic!("test must exhaust the Tokio task budget");
                })
                .await;
            }
            Ok(())
        })
    }
    fn execute(&self, input: ExecutionRequest) -> ProviderExecutionFuture<'_> {
        Box::pin(async move {
            assert!(self
                .target
                .lock()
                .unwrap()
                .replace(input.execution_id.clone())
                .is_none());
            self.executions
                .lock()
                .unwrap()
                .push(input.execution_id.clone());
            let gate = self.execution_gate.lock().unwrap().take();
            let mut closed = self.closed.subscribe();
            self.dispatched.notify_one();
            if let Some(gate) = gate {
                tokio::select! { _ = gate => {}, _ = closed.changed() => {} }
            }
            self.target.lock().unwrap().take();
            if let Some(report) = self.execution_report.lock().unwrap().clone() {
                self.output.send(None).unwrap();
                return ProviderExecutionReply::Finished(report);
            }
            let fault = self.execution_fault.lock().unwrap().clone();
            match fault {
                Some(fault) => {
                    let (error, attachment) = fault.into_parts();
                    self.output.send(None).unwrap();
                    ProviderExecutionReply::Finished(ExecutionReport::new(
                        Some(Err(error)),
                        None,
                        attachment,
                    ))
                }
                None => {
                    // This fixture's provider really reports Completed even when
                    // its completion races local close; cleanup cannot rewrite it.
                    self.output
                        .send(Some(ExecutionEvent::new(
                            input.execution_id,
                            ExecutionUpdate::Finished(ExecutionOutcome::Completed),
                        )))
                        .unwrap();
                    ProviderExecutionReply::Finished(ExecutionReport::new(
                        Some(Ok(ExecutionOutcome::Completed)),
                        None,
                        ProviderSessionState::Usable,
                    ))
                }
            }
        })
    }
    fn steer(
        &self,
        target: ExecutionId,
        input: ExecutionRequest,
    ) -> ProviderOperationFuture<'_, SteeringOutcome> {
        Box::pin(async move {
            if self.target.lock().unwrap().as_ref() != Some(&target) {
                return Ok(SteeringOutcome::PromptRequired);
            }
            // WorkStatus is recorded independently of the response future's life.
            self.steering
                .lock()
                .unwrap()
                .push((target, input.execution_id));
            let gate = self.steering_gate.lock().unwrap().take();
            let mut closed = self.closed.subscribe();
            self.steering_admitted.notify_one();
            if let Some(gate) = gate {
                tokio::select! { _ = gate => {}, _ = closed.changed() => {} }
            }
            Ok(SteeringOutcome::Injected)
        })
    }
    fn answer_permission(
        &self,
        _: PermissionAnswer,
    ) -> ProviderOperationFuture<'_, PermissionResolution> {
        Box::pin(async {
            Err(ProviderOperationFailure::new(
                AgentError::StalePermission,
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
                AgentError::StalePermission,
                ProviderSessionState::Usable,
            ))
        })
    }
    fn close(&self, request: SessionCloseRequest) -> CleanupFuture<'_> {
        Box::pin(async move {
            self.shutdowns.lock().unwrap().push(request);
            let report = self.cleanup.lock().unwrap().clone();
            if report.is_confirmed() {
                self.closed.send_replace(true);
            }
            report
        })
    }
}
async fn workflow() -> (Agent, Arc<WorkflowBackend>, MemoryStorage) {
    workflow_from_storage(MemoryStorage::default()).await
}
async fn workflow_from_storage(
    storage: MemoryStorage,
) -> (Agent, Arc<WorkflowBackend>, MemoryStorage) {
    let backend = workflow_backend();
    let agent = Agent::new(
        Arc::new(WorkflowProvider(backend.clone())),
        storage.manager().await,
    )
    .await
    .unwrap();
    (agent, backend, storage)
}
fn workflow_backend() -> Arc<WorkflowBackend> {
    let (output, receiver) = mpsc::unbounded_channel();
    Arc::new(WorkflowBackend {
        audit: Arc::new(WorkflowAudit::default()),
        output,
        receiver: Mutex::new(Some(receiver)),
        dispatched: Notify::new(),
        execution_gate: Mutex::new(None),
        target: Mutex::new(None),
        executions: Mutex::new(Vec::new()),
        steering: Mutex::new(Vec::new()),
        steering_admitted: Notify::new(),
        steering_gate: Mutex::new(None),
        closed: watch::channel(false).0,
        cleanup: Mutex::new(CleanupReport::confirmed(CloseOutcome { forced: false })),
        execution_fault: Mutex::new(None),
        execution_report: Mutex::new(None),
        exhaust_prepare_budget: AtomicUsize::new(0),
        shutdowns: Mutex::new(Vec::new()),
    })
}
async fn bounded<T>(future: impl Future<Output = T>) -> T {
    timeout(Duration::from_secs(2), future)
        .await
        .expect("workflow checkpoint must progress")
}

#[tokio::test]
async fn invocation_modes_keep_owned_work_and_real_settlement_after_waiter_loss_and_close() {
    for mode in Mode::ALL {
        let (agent, backend, storage) = workflow().await;
        let (_release, gate) = oneshot::channel();
        *backend.execution_gate.lock().unwrap() = Some(gate);
        let caller = mode.start(agent.clone(), request("owned"));
        bounded(backend.dispatched.notified()).await;
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        assert_eq!(
            agent.invoke(request("overlap"), actor()).await,
            Err(AgentError::Busy)
        );
        bounded(agent.close(actor())).await.unwrap();
        let saved = storage.snapshot();
        assert_eq!(saved.invocations.len(), 1, "{mode:?}");
        assert_eq!(
            saved.invocations[0].result,
            Some(Ok(ExecutionOutcome::Completed)),
            "{mode:?}"
        );
        let settlement = saved.invocations[0].provider_report.as_ref().unwrap();
        assert_eq!(
            settlement.provider_result(),
            Some(&Ok(ExecutionOutcome::Completed))
        );
        assert_eq!(settlement.failure(), None);
        assert_eq!(settlement.session_state(), &ProviderSessionState::Usable);
        assert_eq!(backend.executions.lock().unwrap().len(), 1);
        if !matches!(mode, Mode::Direct) {
            assert_eq!(
                bounded(mode.start(agent.clone(), request("owned")))
                    .await
                    .unwrap(),
                Ok(ExecutionOutcome::Completed)
            );
            assert_eq!(
                backend.executions.lock().unwrap().len(),
                1,
                "retry cannot redispatch"
            );
        }
        assert_eq!(
            bounded(agent.invoke(request("resumed"), actor())).await,
            Ok(ExecutionOutcome::Completed)
        );
        bounded(agent.close(actor())).await.unwrap();
    }
}

#[tokio::test]
async fn invocation_modes_follow_explicit_resource_status_independently_of_error_text() {
    for mode in Mode::ALL {
        for unconfirmed in [false, true] {
            let (agent, backend, storage) = workflow().await;
            let error = AgentError::Provider { code: -32077 };
            let attachment = if unconfirmed {
                ProviderSessionState::CleanupRequired
            } else {
                ProviderSessionState::CleanupReported(CleanupReport::confirmed(CloseOutcome {
                    forced: false,
                }))
            };
            *backend.execution_fault.lock().unwrap() = Some(ProviderOperationFailure::new(
                error.clone(),
                attachment.clone(),
            ));
            assert_eq!(
                bounded(mode.start(agent.clone(), request("fault")))
                    .await
                    .unwrap(),
                Err(error.clone())
            );
            let saved = storage.snapshot();
            assert_eq!(saved.invocations[0].result, Some(Err(error.clone())));
            let settlement = saved.invocations[0].provider_report.as_ref().unwrap();
            assert_eq!(settlement.provider_result(), Some(&Err(error)));
            assert_eq!(settlement.session_state(), &attachment);
            *backend.execution_fault.lock().unwrap() = None;
            // CleanupRequired initially fences dispatch, but this conforming
            // backend confirms owned cleanup and audit before the result returns.
            assert_eq!(
                bounded(agent.invoke(request("recovered"), actor())).await,
                Ok(ExecutionOutcome::Completed)
            );
            bounded(agent.close(actor())).await.unwrap();
        }
    }
}

#[tokio::test]
async fn native_stop_keeps_target_settlement_and_never_replays_interrupted_delivery() {
    let (agent, backend, storage) = workflow().await;
    let (_release, gate) = oneshot::channel();
    *backend.execution_gate.lock().unwrap() = Some(gate);
    let target = Mode::Direct.start(agent.clone(), request("target"));
    bounded(backend.dispatched.notified()).await;
    let (_steering_release, gate) = oneshot::channel();
    *backend.steering_gate.lock().unwrap() = Some(gate);
    let steering = tokio::spawn({
        let agent = agent.clone();
        async move { agent.steer(request("correction"), actor()).await }
    });
    bounded(backend.steering_admitted.notified()).await;
    bounded(agent.close(actor())).await.unwrap();
    assert!(matches!(
        bounded(steering).await.unwrap(),
        Err(AgentError::Closed)
    ));
    assert_eq!(
        bounded(target).await.unwrap(),
        Ok(ExecutionOutcome::Completed)
    );
    assert_eq!(
        *backend.executions.lock().unwrap(),
        vec![ExecutionId::new("target").unwrap()]
    );
    assert_eq!(
        *backend.steering.lock().unwrap(),
        vec![(
            ExecutionId::new("target").unwrap(),
            ExecutionId::new("correction").unwrap()
        )]
    );
    let saved = storage.snapshot();
    assert_eq!(
        saved.invocations[0].result,
        Some(Ok(ExecutionOutcome::Completed))
    );
    assert_eq!(
        saved.invocations[0]
            .provider_report
            .as_ref()
            .unwrap()
            .provider_result(),
        Some(&Ok(ExecutionOutcome::Completed))
    );
    assert!(saved.invocations[1].provider_report.is_none());
    assert!(saved.invocations[1].events.is_empty());
    assert_eq!(saved.invocations[1].result, Some(Err(AgentError::Closed)));
    let stop = saved.invocations[1].scheduling.last().unwrap();
    assert_eq!(stop.before, Some(InvocationStage::Queued));
    assert_eq!(stop.stage, InvocationStage::Cancelled);
    assert_eq!(stop.cause, SchedulingCause::SessionClosed);
    assert_eq!(stop.actor, Some(actor()));
    assert_eq!(stop.target, Some(ExecutionId::new("target").unwrap()));
    assert!(matches!(
        agent.steer(request("correction"), actor()).await,
        Err(AgentError::Closed)
    ));
    assert_eq!(backend.steering.lock().unwrap().len(), 1);
    assert_eq!(backend.executions.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn invocation_modes_retain_provider_settlement_when_its_save_panics() {
    for mode in Mode::ALL {
        for commit_first in [false, true] {
            let (agent, backend, storage) = workflow().await;
            storage.0.lock().unwrap().panic_provider_settlement = Some(commit_first);
            let result = bounded(mode.start(agent.clone(), request("save-panic")))
                .await
                .unwrap();
            assert!(
                matches!(
                    &result,
                    Err(AgentError::ExecutionObservation { execution_result: Some(outcome), .. })
                        if **outcome == Ok(ExecutionOutcome::Completed)
                ),
                "mode={mode:?}, committed={commit_first}, result={result:?}"
            );
            let saved = storage.snapshot();
            assert_eq!(saved.invocations[0].result, Some(result.clone()));
            let settlement = saved.invocations[0].provider_report.as_ref().unwrap();
            assert_eq!(
                settlement.provider_result(),
                Some(&Ok(ExecutionOutcome::Completed))
            );
            assert_eq!(settlement.session_state(), &ProviderSessionState::Usable);
            assert!(storage
                .0
                .lock()
                .unwrap()
                .panic_provider_settlement
                .is_none());
            assert!(matches!(
                saved.invocations[0].events.last().unwrap().update(),
                ExecutionUpdate::Finished(ExecutionOutcome::Completed)
            ));
            if !matches!(mode, Mode::Direct) {
                assert_eq!(
                    bounded(mode.start(agent.clone(), request("save-panic")))
                        .await
                        .unwrap(),
                    result
                );
            }
            assert_eq!(backend.executions.lock().unwrap().len(), 1);
            bounded(agent.close(actor())).await.unwrap();
            assert_eq!(
                bounded(agent.invoke(request("recovered"), actor())).await,
                Ok(ExecutionOutcome::Completed)
            );
            bounded(agent.close(actor())).await.unwrap();
        }
    }
}

#[path = "conformance/automatic_cleanup.rs"]
mod automatic_cleanup;

mod unconfirmed_drain;

mod local_cancellation;

mod observation_boundary;

mod queue_order;
