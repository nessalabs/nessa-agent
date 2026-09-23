//! Injected audit panics cannot discard receipts or the actual child process.
use super::*;
use crate::application::agent_execution::executions::ExecutionRequest;
use crate::application::agent_execution::permissions::{
    ActionContext, ApprovalAttribution, ApprovalBasis, PermissionAnswer,
};
use crate::application::agent_execution::providers::ProviderCleanup;
use crate::application::agent_execution::tools::ToolReviewInput;
use crate::domain::agent_execution::permissions::{PermissionOptionId, PermissionStateView};
use crate::domain::agent_execution::prompts::{PromptText, UserMessage};
use crate::domain::agent_execution::tools::ToolCallUpdate;
use crate::infrastructure::acp::executions::event_queue::EventReceiver;
use std::{pin::Pin, sync::Mutex, task::Context};

#[derive(Clone, Copy)]
enum PanicAt {
    Call,
    Poll,
    Drop,
}
struct PanicFuture;
impl Future for PanicFuture {
    type Output = Result<(), AgentError>;
    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
        Poll::Pending
    }
}
impl Drop for PanicFuture {
    fn drop(&mut self) {
        panic!("audit drop panic")
    }
}
struct PanicAudit {
    mode: PanicAt,
    finished_only: bool,
    records: Mutex<Vec<ExecutionAuditRecord>>,
}
impl ExecutionAudit for PanicAudit {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        let panic = !self.finished_only || matches!(record, ExecutionAuditRecord::Finished(_));
        self.records.lock().unwrap().push(record);
        if !panic {
            return Box::pin(async { Ok(()) });
        }
        match self.mode {
            PanicAt::Call => panic!("audit construction panic"),
            PanicAt::Poll => Box::pin(async { panic!("audit poll panic") }),
            PanicAt::Drop => Box::pin(PanicFuture),
        }
    }
}
struct RunningWorker {
    root: tempfile::TempDir,
    commands: mpsc::Sender<Command>,
    close: watch::Sender<Option<SessionCloseRequest>>,
    completion: watch::Receiver<Option<Completion>>,
    events: EventReceiver,
    task: tokio::task::JoinHandle<()>,
    recovery: Arc<ProcessCleanup>,
}
async fn start_worker(audit: Arc<PanicAudit>, fail_cleanup: bool) -> RunningWorker {
    let (worker, startup) = spawn_worker(
        audit,
        fail_cleanup,
        TestAcpProfile {
            reject_startup: false,
            reject_session: false,
        },
    );
    startup.await.unwrap().unwrap();
    worker
}
fn spawn_worker<P: AcpProfile>(
    audit: Arc<PanicAudit>,
    fail_cleanup: bool,
    profile: P,
) -> (
    RunningWorker,
    oneshot::Receiver<Result<ExecutionSessionId, AgentError>>,
) {
    let (root, config, capabilities) = profile_setup();
    let mut command = tokio::process::Command::new(&config.executable);
    command
        .args(&config.arguments)
        .env_clear()
        .current_dir(&config.workspace);
    let mut scope = ProcessScope::spawn(command).unwrap();
    if fail_cleanup {
        scope.fail_next_cleanup();
    }
    let (commands, receiver) = mpsc::channel(16);
    let (close, close_receiver) = watch::channel(None);
    let (finished, completion) = watch::channel(None);
    let (events, stream) = EventQueueBudget::new().channel(16);
    let (ready, startup) = oneshot::channel();
    let (operations, _) = watch::channel(ProviderOperationCapabilities::default());
    let recovery = Arc::new(ProcessCleanup::new(config.clone()));
    let task = tokio::spawn(run(
        scope,
        profile,
        config,
        capabilities,
        receiver,
        close_receiver,
        finished,
        events,
        ready,
        audit,
        None,
        Arc::new(AtomicU64::new(0)),
        operations,
        recovery.clone(),
    ));
    (
        RunningWorker {
            root,
            commands,
            close,
            completion,
            events: stream,
            task,
            recovery,
        },
        startup,
    )
}
async fn begin(
    worker: &mut RunningWorker,
) -> (oneshot::Receiver<ProviderExecutionReply>, PermissionId) {
    let (reply, result) = oneshot::channel();
    worker
        .commands
        .send(Command::ExecutionRequest(
            dispatched(
                ExecutionRequest {
                    execution_id: ExecutionId::new("run").unwrap(),
                    user_message: UserMessage::text_only(PromptText::new("read").unwrap()),
                    estimated_input_tokens: 1,
                    reserved_output_tokens: 10,
                },
                None,
            ),
            reply,
        ))
        .await
        .unwrap();
    loop {
        let event = worker.events.recv().await.unwrap();
        if let ExecutionUpdate::PermissionRequested { id, .. } = event.update() {
            return (result, id.clone());
        }
    }
}
fn process_id(root: &tempfile::TempDir) -> i32 {
    std::fs::read_to_string(root.path().join("pid"))
        .unwrap()
        .parse()
        .unwrap()
}

#[tokio::test]
async fn audit_panics_attempt_all_cleanup_records_and_retain_process_for_retry() {
    for mode in [PanicAt::Call, PanicAt::Poll, PanicAt::Drop] {
        for fail_cleanup in [false, true] {
            let audit = Arc::new(PanicAudit {
                mode,
                finished_only: false,
                records: Mutex::new(Vec::new()),
            });
            let mut worker = start_worker(audit.clone(), fail_cleanup).await;
            let pid = process_id(&worker.root);
            let (result, permission_id) = begin(&mut worker).await;
            worker
                .close
                .send_replace(Some(SessionCloseRequest::Explicit(
                    ActionContext::new("owner", "test", "close").unwrap(),
                )));
            let reply = timeout(Duration::from_secs(5), result)
                .await
                .unwrap()
                .unwrap();
            assert!(reply.into_result().is_err());
            timeout(Duration::from_secs(5), worker.task)
                .await
                .unwrap()
                .expect("worker panic must be supervised");
            let completed = worker.completion.borrow().clone().unwrap();
            assert_eq!(
                completed.cleanup.audit(),
                &Err(AgentError::MultipleOperationFailures {
                    first_error: Box::new(AgentError::AuditFailure),
                    subsequent_error: Box::new(AgentError::MultipleOperationFailures {
                        first_error: Box::new(AgentError::AuditFailure),
                        subsequent_error: Box::new(AgentError::AuditFailure),
                    }),
                })
            );
            assert_eq!(completed.cleanup.is_confirmed(), !fail_cleanup);
            {
                let records = audit.records.lock().unwrap();
                assert_eq!(
                    records.len(),
                    3,
                    "every cleanup record attempted exactly once"
                );
                let origin = CancellationOrigin::Client(
                    ActionContext::new("owner", "test", "close").unwrap(),
                );
                for record in records.iter() {
                    match record {
                        ExecutionAuditRecord::SessionClosed(record) => {
                            assert_eq!(record.closure().session_id().as_str(), "fixture-context");
                            assert_eq!(record.closure().execution_id().unwrap().as_str(), "run");
                            assert_eq!(
                                record.closure().reason(),
                                &PermissionCancellationReason::session_closed()
                            );
                            assert_eq!(record.origin(), &origin);
                        }
                        ExecutionAuditRecord::Cancelled(record) => {
                            assert_eq!(record.session_id().as_str(), "fixture-context");
                            assert_eq!(record.request().execution_id().as_str(), "run");
                            assert_eq!(record.request().id(), &permission_id);
                            assert_eq!(
                                record.request().state(),
                                PermissionStateView::Cancelled {
                                    reason: &PermissionCancellationReason::session_closed()
                                }
                            );
                            assert_eq!(record.origin(), &origin);
                        }
                        ExecutionAuditRecord::Finished(record) => {
                            assert_eq!(record.session_id().as_str(), "fixture-context");
                            assert_eq!(record.execution_id().as_str(), "run");
                            assert_eq!(
                                record.result(),
                                &Err(PermissionCancellationReason::session_closed())
                            );
                        }
                        ExecutionAuditRecord::Answered(_) => {
                            panic!("explicit close did not answer a review")
                        }
                        ExecutionAuditRecord::QueueReordered(_) => {
                            panic!("explicit close did not reorder pending work")
                        }
                        ExecutionAuditRecord::Attachment(_)
                        | ExecutionAuditRecord::QueueAdmitted(_)
                        | ExecutionAuditRecord::QueueSettled(_)
                        | ExecutionAuditRecord::SteeringAcknowledged(_) => {
                            panic!("provider cleanup emitted SDK admission evidence")
                        }
                        ExecutionAuditRecord::ReviewDeclined(_) => {
                            panic!("explicit close did not refuse a review")
                        }
                    }
                }
                for kind in 0..3 {
                    assert_eq!(
                        records
                            .iter()
                            .filter(|record| matches!(
                                (kind, record),
                                (0, ExecutionAuditRecord::SessionClosed(_))
                                    | (1, ExecutionAuditRecord::Cancelled(_))
                                    | (2, ExecutionAuditRecord::Finished(_))
                            ))
                            .count(),
                        1
                    );
                }
            }
            if fail_cleanup {
                assert!(worker.recovery.retry_cleanup().await.is_confirmed());
            }
            assert_ne!(
                unsafe { libc::kill(pid, 0) },
                0,
                "confirmed cleanup reaps actual child"
            );
        }
    }
}

#[tokio::test]
async fn finish_audit_panic_preserves_known_provider_outcome_and_answer_receipt() {
    let audit = Arc::new(PanicAudit {
        mode: PanicAt::Poll,
        finished_only: true,
        records: Mutex::new(Vec::new()),
    });
    let mut worker = start_worker(audit.clone(), false).await;
    let pid = process_id(&worker.root);
    let (result, id) = begin(&mut worker).await;
    let (reply, answer) = oneshot::channel();
    worker
        .commands
        .send(Command::Answer(
            PermissionAnswer {
                execution_id: ExecutionId::new("run").unwrap(),
                id,
                option_id: PermissionOptionId::new("allow").unwrap(),
                attribution: ApprovalAttribution::new(
                    ActionContext::new("owner", "test", "allow").unwrap(),
                    ApprovalBasis::Explicit,
                ),
            },
            reply,
        ))
        .await
        .unwrap();
    answer.await.unwrap().unwrap();
    let reply = timeout(Duration::from_secs(5), result)
        .await
        .unwrap()
        .unwrap();
    let ProviderExecutionReply::Finished(report) = reply else {
        panic!("dispatched result must remain owned")
    };
    assert_eq!(
        report.provider_result(),
        Some(&Ok(ExecutionOutcome::Completed))
    );
    assert!(report.into_result().is_err());
    worker.task.await.unwrap();
    assert!(worker
        .completion
        .borrow()
        .as_ref()
        .unwrap()
        .cleanup
        .is_confirmed());
    assert_ne!(unsafe { libc::kill(pid, 0) }, 0);
}

struct PanicProfile {
    inner: TestAcpProfile,
    startup: bool,
}
impl AcpProfile for PanicProfile {
    fn validate_initialize(&self, value: &Value) -> Result<(), AgentError> {
        self.inner.validate_initialize(value)
    }
    fn new_session_params(
        &self,
        config: &AcpConfig,
        capabilities: &EffectiveCapabilities,
    ) -> Value {
        self.inner.new_session_params(config, capabilities)
    }
    fn session_configuration(&self, id: &str) -> Vec<Value> {
        self.inner.session_configuration(id)
    }
    fn verify_session(
        &self,
        value: &Value,
        capabilities: &EffectiveCapabilities,
        configured: bool,
    ) -> Result<(), AgentError> {
        if self.startup {
            panic!("profile startup panic")
        }
        self.inner.verify_session(value, capabilities, configured)
    }
    fn verify_update(
        &self,
        kind: &str,
        value: &Value,
        capabilities: &EffectiveCapabilities,
        configured: bool,
    ) -> Result<(), AgentError> {
        self.inner
            .verify_update(kind, value, capabilities, configured)
    }
    fn validate_execution(
        &self,
        request: &ExecutionRequest,
        capabilities: &EffectiveCapabilities,
    ) -> Result<(), AgentError> {
        self.inner.validate_execution(request, capabilities)
    }
    fn begin_execution(&mut self) {
        panic!("profile execution panic")
    }
    fn tool_call(&mut self, value: &Value) -> Result<ToolCallUpdate, AgentError> {
        self.inner.tool_call(value)
    }
    fn permission_input(&self, request: &Value) -> Result<ToolReviewInput, AgentError> {
        self.inner.permission_input(request)
    }
}
#[tokio::test]
async fn worker_phase_panics_preserve_scope_and_startup_or_execution_receipt() {
    for startup_failure in [false, true] {
        let audit = Arc::new(PanicAudit {
            mode: PanicAt::Poll,
            finished_only: true,
            records: Mutex::new(Vec::new()),
        });
        let (worker, startup) = spawn_worker(
            audit,
            true,
            PanicProfile {
                inner: TestAcpProfile {
                    reject_startup: false,
                    reject_session: false,
                },
                startup: startup_failure,
            },
        );
        if startup_failure {
            assert!(timeout(Duration::from_secs(5), startup)
                .await
                .unwrap()
                .unwrap()
                .is_err());
        } else {
            startup.await.unwrap().unwrap();
            let (reply, result) = oneshot::channel();
            worker
                .commands
                .send(Command::ExecutionRequest(
                    dispatched(
                        ExecutionRequest {
                            execution_id: ExecutionId::new("panic-run").unwrap(),
                            user_message: UserMessage::text_only(PromptText::new("read").unwrap()),
                            estimated_input_tokens: 1,
                            reserved_output_tokens: 10,
                        },
                        None,
                    ),
                    reply,
                ))
                .await
                .unwrap();
            let reply = timeout(Duration::from_secs(5), result)
                .await
                .unwrap()
                .unwrap();
            assert!(matches!(reply, ProviderExecutionReply::Finished(_)));
            assert!(reply.into_result().is_err());
        }
        timeout(Duration::from_secs(5), worker.task)
            .await
            .unwrap()
            .unwrap();
        assert!(!worker
            .completion
            .borrow()
            .as_ref()
            .unwrap()
            .cleanup
            .is_confirmed());
        assert!(worker.recovery.retry_cleanup().await.is_confirmed());
        assert_ne!(unsafe { libc::kill(process_id(&worker.root), 0) }, 0);
    }
}
