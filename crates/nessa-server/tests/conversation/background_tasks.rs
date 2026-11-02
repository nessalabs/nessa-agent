//! Real conversation/store/provider orchestration with a controlled process port.
use super::*;
use crate::conversation::{
    application::{
        background_tasks::{
            BackgroundCallMatcher, BackgroundRecord, BackgroundRun, BackgroundRunner,
            BackgroundStore, BackgroundTaskError,
        },
        AgentToolAudit, AgentToolAuditRecord, ConversationFuture,
    },
    domain::{BackgroundCommand, BackgroundTaskId, TaskCause, TaskDelivery, TaskOutcome, TaskStop},
    infrastructure::LocalBackgroundTasks,
};
use nessa_sdk::{
    application::agent_execution::executions::ExecutionUpdate,
    domain::agent_execution::{
        prompts::MessageSender,
        tools::{McpTool, ToolCallId, ToolCallInput, ToolCallUpdate, ToolStatus},
    },
};
use std::sync::atomic::AtomicUsize;
use tokio::sync::{watch, Notify};

#[derive(Default)]
struct Audit;
impl AgentToolAudit for Audit {
    fn record(&self, _: AgentToolAuditRecord) -> ConversationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}
struct Matcher;
impl BackgroundCallMatcher for Matcher {
    fn matches(
        &self,
        arguments: &str,
        task: &BackgroundTaskId,
        command: &BackgroundCommand,
    ) -> bool {
        arguments
            == format!(
                "{{\"taskId\":\"{}\",\"command\":\"{}\"}}",
                task.as_str(),
                command.text()
            )
    }
}
#[derive(Default)]
struct Runner {
    runs: AtomicUsize,
    started: Notify,
    released: Notify,
    stopped: AtomicUsize,
    shutdowns: AtomicUsize,
    hold_cleanup: std::sync::atomic::AtomicBool,
    cleanup_entered: Notify,
    release_cleanup: Notify,
}
impl BackgroundRunner for Runner {
    fn run(
        &self,
        _: BackgroundCommand,
        _: String,
        mut stop: watch::Receiver<Option<TaskStop>>,
    ) -> BackgroundRun<'_, TaskOutcome> {
        Box::pin(async move {
            self.runs.fetch_add(1, Ordering::SeqCst);
            self.started.notify_one();
            let cancelled = tokio::select! {
                _ = self.released.notified() => false,
                _ = async { if stop.borrow().is_none() { let _ = stop.changed().await; } } => true,
            };
            if cancelled {
                self.stopped.fetch_add(1, Ordering::SeqCst);
                if self.hold_cleanup.load(Ordering::SeqCst) {
                    self.cleanup_entered.notify_one();
                    self.release_cleanup.notified().await;
                }
            }
            outcome(if cancelled {
                TaskCause::Cancelled
            } else {
                TaskCause::Exited
            })
        })
    }
    fn shutdown(&self) -> BackgroundRun<'_, Result<(), BackgroundTaskError>> {
        Box::pin(async {
            self.shutdowns.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }
}
fn outcome(cause: TaskCause) -> TaskOutcome {
    TaskOutcome {
        cause,
        scope: Some(42),
        process: Some(43),
        os_pid: Some(44),
        exit_code: Some(0),
        signal: None,
        cleanup_verified: true,
        stdout: "actual command output".into(),
        stderr: String::new(),
        omitted_bytes: 0,
        output_complete: true,
        error: None,
    }
}
fn wire(task: &str, command: &str) -> String {
    format!("{{\"taskId\":\"{task}\",\"command\":\"{command}\"}}")
}
fn update(native: &str, task: &str, command: &str) -> ExecutionUpdate {
    ExecutionUpdate::Tool(
        ToolCallUpdate::new(
            ToolCallId::new(native).unwrap(),
            Some("Background".into()),
            None,
            Some(ToolStatus::Running),
            None,
            None,
        )
        .with_mcp_tool(McpTool::new("nessa-background", "start").unwrap())
        .with_input(ToolCallInput::new(wire(task, command)).unwrap()),
    )
}
async fn turn(
    service: &ConversationService,
    provider: &ProviderFactory,
    target: &ConversationId,
    execution: &str,
    updates: Vec<ExecutionUpdate>,
) -> oneshot::Sender<()> {
    let (release, gate) = oneshot::channel();
    *provider.after_updates_gate.lock().unwrap() = Some(gate);
    provider.execution_updates.lock().unwrap().extend(updates);
    let notified = provider.updates_sent.notified();
    send(service, target, execution, "start a task").await;
    tokio::time::timeout(Duration::from_secs(5), notified)
        .await
        .unwrap();
    release
}
async fn setup(
    root: &Path,
    provider: Arc<ProviderFactory>,
) -> (
    ConversationService,
    Arc<RecordStorage>,
    Arc<LocalConversationStore>,
    Arc<LocalBackgroundTasks>,
    Arc<Runner>,
) {
    let (mut service, storage, metadata) = fixture(root, provider);
    Arc::get_mut(&mut service.inner)
        .expect("new service has one owner")
        .workspace = Some(root.to_str().unwrap().into());
    let tasks = Arc::new(LocalBackgroundTasks::open(&root.join("tasks.sqlite3")).unwrap());
    let runner = Arc::new(Runner::default());
    service.bind_agent_tool_audit(Arc::new(Audit));
    service.bind_background_tasks(
        tasks.clone(),
        runner.clone(),
        Arc::new(Matcher),
        Arc::new(crate::timer::TokioTimer::new()),
    );
    service.resume_background_tasks().await.unwrap();
    (service, storage, metadata, tasks, runner)
}
async fn start(
    service: &ConversationService,
    conversation: &ConversationId,
    task: &str,
    command: &str,
) -> Result<BackgroundRecord, BackgroundTaskError> {
    let (_kept, stop) = watch::channel(false);
    service
        .start_background_task(
            conversation.clone(),
            "relay-call".into(),
            BackgroundTaskId::new(task).unwrap(),
            BackgroundCommand::new(command, 120).unwrap(),
            stop,
        )
        .await
}
async fn wait_delivery(
    store: &LocalBackgroundTasks,
    conversation: &ConversationId,
    task: &str,
) -> BackgroundRecord {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let record = store
                .read(conversation, &BackgroundTaskId::new(task).unwrap())
                .await
                .unwrap()
                .unwrap();
            if record.lifecycle.delivery() != TaskDelivery::Waiting {
                return record;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("task delivered")
}

#[tokio::test]
async fn background_task_keeps_original_call_and_delivers_one_hidden_continuation() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata, tasks, runner) = setup(&root, provider.clone()).await;
    let target = id();
    opened(&service, &provider, &target).await;
    let release = turn(
        &service,
        &provider,
        &target,
        "first",
        vec![update("original", "build", "make")],
    )
    .await;
    let accepted = start(&service, &target, "build", "make").await.unwrap();
    runner.started.notified().await;
    let retry = start(&service, &target, "build", "make").await.unwrap();
    assert_eq!(accepted.source, retry.source);
    assert!(matches!(
        start(&service, &target, "build", "other").await,
        Err(BackgroundTaskError::Conflict)
    ));
    release.send(()).unwrap();
    let live = service.live(&target).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while live.agent.active_execution_id().is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let view = service.read(target.clone(), caller("view")).await.unwrap();
    let original = view
        .tools
        .iter()
        .find(|tool| tool.tool_id == "original")
        .unwrap();
    assert_eq!(original.status, "running");
    assert_eq!(original.background_task_id.as_deref(), Some("build"));
    // Retrying during a later turn cannot steal the original waterfall row.
    let later = turn(
        &service,
        &provider,
        &target,
        "later",
        vec![update("retry-row", "build", "make")],
    )
    .await;
    let retried = start(&service, &target, "build", "make").await.unwrap();
    assert_eq!(retried.source.tool_call, "original");
    runner.released.notify_one();
    let record = wait_delivery(&tasks, &target, "build").await;
    assert_eq!(record.lifecycle.delivery(), TaskDelivery::Delivered);
    assert_eq!(runner.runs.load(Ordering::SeqCst), 1);
    // The continuation queues behind a busy turn instead of interrupting it.
    assert!(!provider
        .executions
        .lock()
        .unwrap()
        .contains(&record.continuation_id()));
    later.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if provider
                .executions
                .lock()
                .unwrap()
                .contains(&record.continuation_id())
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let delivered = provider
        .messages
        .lock()
        .unwrap()
        .iter()
        .find(|m| matches!(m.sender(), MessageSender::Internal))
        .cloned()
        .unwrap();
    assert!(delivered
        .text()
        .unwrap()
        .as_str()
        .contains("actual command output"));
    let view = service.read(target.clone(), caller("view")).await.unwrap();
    let original = view
        .tools
        .iter()
        .find(|tool| tool.tool_id == "original")
        .unwrap();
    assert_eq!(original.status, "completed");
    assert!(original.details.contains("actual command output"));
    assert!(view
        .messages
        .iter()
        .all(|m| !m.user_text.contains("actual command output")));
    assert!(view
        .pending
        .iter()
        .all(|m| !m.text.contains("actual command output")));
    drop(live);
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn ambiguous_calls_and_pre_admission_cancellation_launch_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata, _tasks, runner) = setup(&root, provider.clone()).await;
    let target = id();
    opened(&service, &provider, &target).await;
    let release = turn(
        &service,
        &provider,
        &target,
        "first",
        vec![
            update("one", "build", "make"),
            update("two", "build", "make"),
        ],
    )
    .await;
    assert!(matches!(
        start(&service, &target, "build", "make").await,
        Err(BackgroundTaskError::UnobservedCall)
    ));
    let (_stop, stopped) = watch::channel(true);
    assert!(service
        .start_background_task(
            target.clone(),
            "cancelled".into(),
            BackgroundTaskId::new("cancelled").unwrap(),
            BackgroundCommand::new("make", 120).unwrap(),
            stopped
        )
        .await
        .is_err());
    assert_eq!(runner.runs.load(Ordering::SeqCst), 0);
    release.send(()).unwrap();
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn background_completion_reopens_the_same_parked_provider_session() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    provider.session_resume.store(true, Ordering::SeqCst);
    let (service, storage, metadata, tasks, runner) = setup(&root, provider.clone()).await;
    let target = id();
    opened(&service, &provider, &target).await;
    let release = turn(
        &service,
        &provider,
        &target,
        "parked-start",
        vec![update("original", "parked", "make")],
    )
    .await;
    start(&service, &target, "parked", "make").await.unwrap();
    runner.started.notified().await;
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while service.park_idle(Duration::ZERO).await == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("idle provider parks while its background command runs");
    assert!(service.live(&target).await.is_none());
    assert_eq!(runner.stopped.load(Ordering::SeqCst), 0);
    runner.released.notify_one();
    let record = wait_delivery(&tasks, &target, "parked").await;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if provider
                .executions
                .lock()
                .unwrap()
                .contains(&record.continuation_id())
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    {
        let opens = provider.provider_opens.lock().unwrap();
        assert_eq!(opens.len(), 2);
        assert_eq!(opens[1].1.as_ref(), Some(&opens[0].0));
    }
    assert_eq!(runner.runs.load(Ordering::SeqCst), 1);
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn background_shutdown_stops_process_and_preserves_undelivered_evidence() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata, tasks, runner) = setup(&root, provider.clone()).await;
    let target = id();
    opened(&service, &provider, &target).await;
    let release = turn(
        &service,
        &provider,
        &target,
        "start",
        vec![update("original", "shutdown", "make")],
    )
    .await;
    start(&service, &target, "shutdown", "make").await.unwrap();
    runner.started.notified().await;
    release.send(()).unwrap();
    service.shutdown().await.unwrap();
    let record = tasks
        .read(&target, &BackgroundTaskId::new("shutdown").unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(runner.stopped.load(Ordering::SeqCst), 1);
    assert_eq!(
        record.lifecycle.outcome().unwrap().cause,
        TaskCause::Cancelled
    );
    assert!(record.lifecycle.outcome().unwrap().cleanup_verified);
    assert!(matches!(
        record.lifecycle.stop(),
        Some(crate::conversation::domain::TaskStop::GatewayStopped(_))
    ));
    assert_eq!(record.lifecycle.delivery(), TaskDelivery::Waiting);
    assert!(!provider
        .messages
        .lock()
        .unwrap()
        .iter()
        .any(|message| matches!(message.sender(), MessageSender::Internal)));
    drop(service);
    storage.shutdown().await.unwrap();
    drop(metadata);
}

#[tokio::test]
async fn background_deletion_stops_process_without_resuming_deleted_conversation() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata, tasks, runner) = setup(&root, provider.clone()).await;
    let target = id();
    opened(&service, &provider, &target).await;
    let release = turn(
        &service,
        &provider,
        &target,
        "start",
        vec![update("original", "deleted", "make")],
    )
    .await;
    start(&service, &target, "deleted", "make").await.unwrap();
    runner.started.notified().await;
    release.send(()).unwrap();
    let answer = tokio::time::timeout(
        Duration::from_secs(10),
        service.delete(target.clone(), caller("delete-background")),
    )
    .await
    .expect("deletion must not wait on its own continuation");
    // The fixture has no provider-session eraser; background cleanup still happens first.
    assert!(matches!(
        answer,
        Ok(_) | Err(ConversationError::DeletionIncomplete(_))
    ));
    assert_eq!(runner.stopped.load(Ordering::SeqCst), 1);
    assert!(tasks
        .read(&target, &BackgroundTaskId::new("deleted").unwrap())
        .await
        .unwrap()
        .is_none());
    assert!(!provider
        .messages
        .lock()
        .unwrap()
        .iter()
        .any(|message| matches!(message.sender(), MessageSender::Internal)));
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn background_restart_records_unknown_cleanup_without_reexecuting_command() {
    use crate::conversation::application::background_tasks::BackgroundSource;
    use crate::conversation::domain::TaskLifecycle;
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let target = id();
    opened(&service, &provider, &target).await;
    let tasks = Arc::new(LocalBackgroundTasks::open(&root.join("tasks.sqlite3")).unwrap());
    let runner = Arc::new(Runner::default());
    let record = BackgroundRecord {
        id: BackgroundTaskId::new("recovered").unwrap(),
        command: BackgroundCommand::new("make", 120).unwrap(),
        source: BackgroundSource {
            conversation: target.clone(),
            execution: "lost-turn".into(),
            tool_call: "lost-call".into(),
            caller: caller("start"),
            workspace: root.to_str().unwrap().into(),
        },
        lifecycle: TaskLifecycle::admitted(0),
    };
    tasks.save(record, None).await.unwrap();
    service.bind_background_tasks(
        tasks.clone(),
        runner.clone(),
        Arc::new(Matcher),
        Arc::new(crate::timer::TokioTimer::new()),
    );
    assert_eq!(service.resume_background_tasks().await.unwrap(), 1);
    assert_eq!(service.resume_background_tasks().await.unwrap(), 0);
    let recovered = wait_delivery(&tasks, &target, "recovered").await;
    assert_eq!(runner.runs.load(Ordering::SeqCst), 0);
    let outcome = recovered.lifecycle.outcome().unwrap();
    assert_eq!(outcome.cause, TaskCause::Interrupted);
    assert!(!outcome.cleanup_verified);
    assert!(recovered.lifecycle.retains_capacity());
    assert!(service.shutdown().await.is_err());
    drop(service);
    storage.shutdown().await.unwrap();
    drop(metadata);
}

#[tokio::test]
async fn background_delivery_save_retries_without_repeating_model_continuation() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata, tasks, runner) = setup(&root, provider.clone()).await;
    let target = id();
    opened(&service, &provider, &target).await;
    let release = turn(
        &service,
        &provider,
        &target,
        "start",
        vec![update("original", "delivery", "make")],
    )
    .await;
    let record = start(&service, &target, "delivery", "make").await.unwrap();
    runner.started.notified().await;
    let connection =
        nessa_local_database::rusqlite::Connection::open(root.join("tasks.sqlite3")).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_delivery BEFORE INSERT ON transitions WHEN NEW.revision = 2 BEGIN SELECT RAISE(ABORT, 'injected delivery save failure'); END;").unwrap();
    release.send(()).unwrap();
    runner.released.notify_one();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if provider
                .executions
                .lock()
                .unwrap()
                .contains(&record.continuation_id())
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        tasks
            .read(&target, &record.id)
            .await
            .unwrap()
            .unwrap()
            .lifecycle
            .delivery(),
        TaskDelivery::Waiting
    );
    connection
        .execute_batch("DROP TRIGGER fail_delivery")
        .unwrap();
    let delivered = wait_delivery(&tasks, &target, "delivery").await;
    assert_eq!(delivered.lifecycle.delivery(), TaskDelivery::Delivered);
    assert_eq!(
        provider
            .executions
            .lock()
            .unwrap()
            .iter()
            .filter(|execution| **execution == record.continuation_id())
            .count(),
        1
    );
    assert_eq!(runner.runs.load(Ordering::SeqCst), 1);
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn background_failed_stop_audit_still_cleans_every_owner_and_supervisor() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata, tasks, runner) = setup(&root, provider.clone()).await;
    let target = id();
    opened(&service, &provider, &target).await;
    let release = turn(
        &service,
        &provider,
        &target,
        "start",
        vec![
            update("one", "first", "make"),
            update("two", "second", "make"),
        ],
    )
    .await;
    for task in ["first", "second"] {
        start(&service, &target, task, "make").await.unwrap();
        runner.started.notified().await;
    }
    let connection =
        nessa_local_database::rusqlite::Connection::open(root.join("tasks.sqlite3")).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_stop BEFORE INSERT ON transitions WHEN NEW.task = 'first' AND NEW.revision = 1 BEGIN SELECT RAISE(ABORT, 'injected stop audit failure'); END;").unwrap();
    release.send(()).unwrap();
    assert!(service.shutdown().await.is_err());
    assert_eq!(runner.stopped.load(Ordering::SeqCst), 2);
    assert_eq!(runner.shutdowns.load(Ordering::SeqCst), 1);
    let later = tasks
        .read(&target, &BackgroundTaskId::new("second").unwrap())
        .await
        .unwrap()
        .unwrap();
    assert!(later.lifecycle.outcome().unwrap().cleanup_verified);
    let failed = tasks
        .read(&target, &BackgroundTaskId::new("first").unwrap())
        .await
        .unwrap()
        .unwrap();
    assert!(failed.lifecycle.outcome().is_none());
    assert!(failed.lifecycle.retains_capacity());
    drop(service);
    storage.shutdown().await.unwrap();
    drop(metadata);
}

#[tokio::test]
async fn background_failed_cancel_save_preserves_first_initiator_on_retry() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata, tasks, runner) = setup(&root, provider.clone()).await;
    let target = id();
    opened(&service, &provider, &target).await;
    let release = turn(
        &service,
        &provider,
        &target,
        "start",
        vec![update("one", "first", "make")],
    )
    .await;
    let task = start(&service, &target, "first", "make").await.unwrap();
    runner.started.notified().await;
    let (_keep, stop) = watch::channel(false);
    let first = service
        .admit_agent_tool(
            target.clone(),
            "cancel-first".into(),
            "background.cancel".into(),
            None,
            crate::conversation::application::AgentToolRequest {
                logical: "first".into(),
                command: "cancel-first".into(),
            },
            stop.clone(),
        )
        .await
        .unwrap();
    let second = service
        .admit_agent_tool(
            target.clone(),
            "cancel-second".into(),
            "background.cancel".into(),
            None,
            crate::conversation::application::AgentToolRequest {
                logical: "first".into(),
                command: "cancel-second".into(),
            },
            stop,
        )
        .await
        .unwrap();
    let connection =
        nessa_local_database::rusqlite::Connection::open(root.join("tasks.sqlite3")).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_stop BEFORE INSERT ON transitions WHEN NEW.revision = 1 BEGIN SELECT RAISE(ABORT, 'injected stop audit failure'); END;").unwrap();
    assert_eq!(
        service.cancel_background_task(&first, &task.id).await,
        Err(BackgroundTaskError::Storage)
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        while runner.stopped.load(Ordering::SeqCst) != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    connection.execute_batch("DROP TRIGGER fail_stop").unwrap();
    let retried = service
        .cancel_background_task(&second, &task.id)
        .await
        .unwrap();
    let TaskStop::Requested { initiator, .. } = retried.lifecycle.stop().unwrap() else {
        panic!("expected explicit stop");
    };
    assert_eq!(initiator.action(), first.caller().action_id);
    assert_ne!(first.caller().action_id, second.caller().action_id);
    release.send(()).unwrap();
    let delivered = wait_delivery(&tasks, &target, "first").await;
    assert_eq!(delivered.lifecycle.stop(), retried.lifecycle.stop());
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn background_start_reuses_admission_when_export_has_queued() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata, _tasks, runner) = setup(&root, provider.clone()).await;
    let target = id();
    opened(&service, &provider, &target).await;
    let release = turn(
        &service,
        &provider,
        &target,
        "start",
        vec![update("one", "export", "make")],
    )
    .await;
    let deleting = service.inner.deletions.lock(&target).await;
    let worker = service.clone();
    let task_target = target.clone();
    let starting =
        tokio::spawn(async move { start(&worker, &task_target, "export", "make").await });
    tokio::time::timeout(Duration::from_secs(5), async {
        while service.inner.admission.try_write().is_ok() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let mut export = Box::pin(service.hold_for_export());
    assert!(futures_util::poll!(export.as_mut()).is_pending());
    drop(deleting);
    tokio::time::timeout(Duration::from_secs(5), starting)
        .await
        .expect("start must not take a second read behind export")
        .unwrap()
        .unwrap();
    let cut = tokio::time::timeout(Duration::from_secs(5), export)
        .await
        .expect("export must join the admitted start")
        .unwrap();
    runner.started.notified().await;
    assert_eq!(runner.runs.load(Ordering::SeqCst), 1);
    drop(cut);
    release.send(()).unwrap();
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn background_deletion_does_not_join_admission_blocked_delivery() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata, tasks, runner) = setup(&root, provider.clone()).await;
    runner.hold_cleanup.store(true, Ordering::SeqCst);
    let target = id();
    opened(&service, &provider, &target).await;
    let release = turn(
        &service,
        &provider,
        &target,
        "start",
        vec![update("one", "delete-export", "make")],
    )
    .await;
    start(&service, &target, "delete-export", "make")
        .await
        .unwrap();
    runner.started.notified().await;
    release.send(()).unwrap();
    let deleting_service = service.clone();
    let deleting_target = target.clone();
    let deleting = tokio::spawn(async move {
        deleting_service
            .delete(deleting_target, caller("delete-export"))
            .await
    });
    runner.cleanup_entered.notified().await;
    let mut export = Box::pin(service.hold_for_export());
    assert!(futures_util::poll!(export.as_mut()).is_pending());
    runner.release_cleanup.notify_one();
    let answer = tokio::time::timeout(Duration::from_secs(5), deleting)
        .await
        .expect("deletion must not join delivery behind export")
        .unwrap();
    match answer {
        Ok(_) => (),
        Err(ConversationError::DeletionIncomplete(failures)) => {
            assert!(failures.background_tasks.is_none(), "{failures:?}")
        }
        Err(error) => panic!("unexpected deletion failure: {error:?}"),
    }
    assert!(tasks
        .read(&target, &BackgroundTaskId::new("delete-export").unwrap())
        .await
        .unwrap()
        .is_none());
    let cut = tokio::time::timeout(Duration::from_secs(5), export)
        .await
        .unwrap()
        .unwrap();
    drop(cut);
    assert_eq!(runner.stopped.load(Ordering::SeqCst), 1);
    assert!(!provider
        .messages
        .lock()
        .unwrap()
        .iter()
        .any(|message| matches!(message.sender(), MessageSender::Internal)));
    retire(service, storage, metadata).await;
}

/// Parking asks the task owner whether a background command of a
/// conversation still runs: yes from its start until its end is settled, and
/// while it does, a thread its agent closed is not parked.
#[tokio::test]
async fn a_running_background_command_is_reported_until_its_end_is_settled() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    // An agent that resumes its session, so closing its thread is taken.
    provider
        .session_resume
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let (service, _storage, _metadata, tasks, runner) = setup(&root, provider.clone()).await;
    let target = id();
    opened(&service, &provider, &target).await;
    let running = || {
        service
            .inner
            .background_tasks
            .get()
            .unwrap()
            .running_for(&target)
    };
    assert!(!running());
    let release = turn(
        &service,
        &provider,
        &target,
        "first",
        vec![update("original", "build", "make")],
    )
    .await;
    start(&service, &target, "build", "make").await.unwrap();
    runner.started.notified().await;
    assert!(running(), "the command runs");
    assert!(!service
        .inner
        .background_tasks
        .get()
        .unwrap()
        .running_for(&id()));
    release.send(()).unwrap();
    // Its turn ends while the command runs: a thread its agent closed is
    // still in use, so no sweep parks it.
    service.close_after_turn(&target).await.unwrap();
    for _ in 0..20 {
        assert_eq!(
            service.sweep(None).await,
            0,
            "parked while its command runs"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(running());
    runner.released.notify_one();
    wait_delivery(&tasks, &target, "build").await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while running() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("its end is settled");
}
