//! Production storage and MCP regressions for report binding and final settlement.
use super::*;
use crate::conversation::application::ThreadReportStore;
use crate::conversation::infrastructure::{DurableAgentToolAudit, LocalThreadReports};
use nessa_sdk::application::agent_execution::{
    commands::MutationFailure,
    hooks::{HookError, InvocationContext, InvocationHook},
};
use nessa_sdk::domain::agent_execution::executions::ExecutionOutcome;
use std::sync::Condvar;

struct HeldAfterHook {
    entered: Arc<tokio::sync::Notify>,
    release: Arc<(Mutex<bool>, Condvar)>,
}
impl InvocationHook for HeldAfterHook {
    fn after_invocation(
        &self,
        _: &InvocationContext<'_>,
        _: &Result<ExecutionOutcome, AgentError>,
    ) -> Result<(), HookError> {
        self.entered.notify_one();
        let (lock, condition) = &*self.release;
        let guard = lock.lock().unwrap();
        let _ = condition
            .wait_timeout_while(guard, Duration::from_secs(20), |ready| !*ready)
            .unwrap();
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_report_waits_for_final_settlement_after_a_slow_hook() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    provider.turn_cancel.store(true, Ordering::SeqCst);
    let (service, storage, metadata) = fixture(&root, provider.clone());
    service.bind_commands(storage.clone());
    let store = Arc::new(LocalThreadReports::open(&root.join("reports.sqlite3")).unwrap());
    service.bind_thread_reports(store.clone());
    let parent = id();
    let child = id();
    opened(&service, &provider, &parent).await;
    opened(&service, &provider, &child).await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    let live = service.resolve(&child, &caller("probe")).await.unwrap();
    live.agent.add_invocation_hook(Arc::new(HeldAfterHook {
        entered: entered.clone(),
        release: release.clone(),
    }));
    assert_eq!(
        service
            .report_thread_turn(
                child.clone(),
                "slow".into(),
                ThreadReport {
                    parent: parent.clone(),
                    caller: caller("report-probe"),
                    execution: "slow-report".into()
                }
            )
            .await,
        ReportRegistration::Registered
    );
    send(&service, &child, "slow", "synthetic").await;
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_secs(8)).await;
    {
        let (lock, condition) = &*release;
        *lock.lock().unwrap() = true;
        condition.notify_all();
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let snapshot = live.agent.session_manager().snapshot().await.unwrap();
            let record = snapshot
                .invocations
                .iter()
                .find(|r| r.request.execution_id.as_str() == "slow")
                .unwrap();
            if record.scheduling.last().map(|e| e.stage) == Some(InvocationStage::Settled) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(2), reports_in(&service, &parent, 1)).await;
    let parent_live = service.resolve(&parent, &caller("probe")).await.unwrap();
    let snapshot = parent_live
        .agent
        .session_manager()
        .snapshot()
        .await
        .unwrap();
    let reports = snapshot
        .invocations
        .iter()
        .filter(|r| {
            r.request
                .user_message
                .text_str()
                .starts_with("[Thread report]")
        })
        .count();
    let pending = store.pending().await.unwrap().len();
    eprintln!(
        "slow settled: parent_reports={reports}, durable_pending={pending}, in_memory_pending={}",
        service.inner.thread_reports.waiting()
    );
    service.forget_thread_report(&child, "slow-report").await;
    drop(parent_live);
    drop(live);
    retire(service, storage, metadata).await;
    assert_eq!(
        reports, 1,
        "a completed turn must report after slow settlement"
    );
}

#[tokio::test]
async fn failed_send_cleanup_for_another_child_preserves_the_saved_report() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    provider.turn_cancel.store(true, Ordering::SeqCst);
    let (service, storage, metadata) = fixture(&root, provider.clone());
    service.bind_commands(storage.clone());
    let store = Arc::new(LocalThreadReports::open(&root.join("reports.sqlite3")).unwrap());
    service.bind_thread_reports(store.clone());
    let parent = id();
    let child = id();
    let other = id();
    opened(&service, &provider, &parent).await;
    opened(&service, &provider, &child).await;
    opened(&service, &provider, &other).await;
    let release = hold_turn(&service, &provider, &child, "ahead").await;
    let report = ThreadReport {
        parent,
        caller: caller("report-binding-action"),
        execution: "report-binding".into(),
    };
    assert_eq!(
        service
            .report_thread_turn(child.clone(), "work".into(), report.clone())
            .await,
        ReportRegistration::Registered
    );
    service
        .submit_command(
            storage.clone(),
            child.clone(),
            caller("send-binding"),
            Initiation::owner(),
            false,
            "work".into(),
            message("synthetic"),
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    assert_eq!(store.pending().await.unwrap().len(), 1);
    service
        .report_thread_turn(other.clone(), "work".into(), report)
        .await;
    let refused = service
        .submit_command(
            storage.clone(),
            other.clone(),
            caller("send-binding"),
            Initiation::owner(),
            false,
            "work".into(),
            message("synthetic"),
            SubmissionMode::Queue,
        )
        .await;
    assert!(
        matches!(refused, Err(MutationFailure::Conflict)),
        "changed target must produce typed conflict"
    );
    service
        .settle_failed_thread_report(other, "work", "report-binding", &caller("send-binding"))
        .await;
    let pending = store.pending().await.unwrap().len();
    eprintln!(
        "private conflicting target: durable_pending={pending}, in_memory_pending={}",
        service.inner.thread_reports.waiting()
    );
    service.forget_thread_report(&child, "report-binding").await;
    drop(release);
    retire(service, storage, metadata).await;
    assert_eq!(
        pending, 1,
        "conflicting retry must preserve original durable report"
    );
}

#[tokio::test]
async fn a_pending_mcp_report_survives_a_conflicting_target_retry() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    provider.turn_cancel.store(true, Ordering::SeqCst);
    let (service, storage, metadata) = fixture(&root, provider.clone());
    service.bind_commands(storage.clone());
    super::super::parent_stop::bind(&service, &storage, &root, true).await;
    let store = Arc::new(LocalThreadReports::open(&root.join("reports.sqlite3")).unwrap());
    service.bind_thread_reports(store.clone());
    service.bind_agent_tool_audit(Arc::new(
        DurableAgentToolAudit::new(root.join("agent-audit"), Arc::new(TestClock)).unwrap(),
    ));
    let source = id();
    opened(&service, &provider, &source).await;
    let release_source = hold_turn(&service, &provider, &source, "source-turn").await;
    let tools = ConversationTools::new();
    tools.attach(service.clone());
    let servers = McpServers::new(Vec::new(), Arc::new(RuntimeClock::new())).unwrap();
    let grants = ConversationGrants::new(servers.clone(), Arc::new(OsTokens));
    let grant = grants.grant(&SessionId::new(source.to_string()).unwrap());
    let key = ConfigurationKey::new([9; 32]);
    let digest = built_in_digest(&key, tools.as_ref());
    let relay = Arc::new(Relay::new(servers, grants.clone(), key).with_built_in(tools));
    let (mut client, serving) =
        McpClient::connect(relay, digest, grant.environment()[0].1.clone()).await;
    let first = client
        .call("create_thread", json!({"requestId":"pending-create-first"}))
        .await;
    let other = client
        .call("create_thread", json!({"requestId":"pending-create-other"}))
        .await;
    assert_eq!(first["isError"], false, "{first}");
    assert_eq!(other["isError"], false, "{other}");
    let child = first["structuredContent"]["threadId"]
        .as_str()
        .unwrap()
        .to_owned();
    let other = other["structuredContent"]["threadId"]
        .as_str()
        .unwrap()
        .to_owned();
    let child_id = ConversationId::new(&child).unwrap();
    let release_child = hold_turn(&service, &provider, &child_id, "ahead").await;
    let sent = client
        .call(
            "send_message",
            json!({"threadId":child,"message":"work","requestId":"pending-send"}),
        )
        .await;
    assert_eq!(sent["isError"], false, "{sent}");
    assert_eq!(sent["structuredContent"]["reportsBack"], true);
    let before = store.pending().await.unwrap();
    assert_eq!(before.len(), 1);
    let report_id = before[0].report.execution.clone();
    let conflict = client
        .call(
            "send_message",
            json!({"threadId":other,"message":"work","requestId":"pending-send"}),
        )
        .await;
    assert_eq!(
        conflict["structuredContent"]["error"], "request_conflict",
        "{conflict}"
    );
    let remaining = store.pending().await.unwrap();
    let count = remaining.len();
    eprintln!("MCP pending conflicting target: durable_before=1, durable_after={count}, in_memory_pending={}",service.inner.thread_reports.waiting());
    assert_eq!(
        count, 1,
        "MCP conflicting retry must preserve the first pending durable row"
    );
    assert_eq!(remaining[0].child, before[0].child);
    assert_eq!(remaining[0].execution, before[0].execution);
    assert_eq!(remaining[0].report.execution, report_id);
    let retried = client
        .call(
            "send_message",
            json!({"threadId":child,"message":"work","requestId":"pending-send"}),
        )
        .await;
    assert_eq!(retried["isError"], false, "{retried}");
    assert_eq!(
        retried["structuredContent"]["executionId"],
        sent["structuredContent"]["executionId"]
    );
    assert_eq!(store.pending().await.unwrap().len(), 1);
    // The queued child's cancellation uses the same report and does not stop its host.
    let execution = sent["structuredContent"]["executionId"].as_str().unwrap();
    let stopped = client
        .call(
            "stop_thread",
            json!({"threadId":child,"executionId":execution,"requestId":"pending-stop"}),
        )
        .await;
    assert_eq!(stopped["isError"], false, "{stopped}");
    let reports = reports_in(&service, &source, 1).await;
    assert_eq!(reports.len(), 1);
    assert!(reports[0].contains(": cancelled."), "{}", reports[0]);
    assert!(!provider
        .executions
        .lock()
        .unwrap()
        .iter()
        .any(|ran| ran == execution));
    let retried = client
        .call(
            "send_message",
            json!({"threadId":child,"message":"work","requestId":"pending-send"}),
        )
        .await;
    assert_eq!(retried["isError"], false, "{retried}");
    assert_eq!(reports_in(&service, &source, 1).await.len(), 1);
    assert!(store.pending().await.unwrap().is_empty());
    // Recreate the durable state of a crash after parent delivery but before row cleanup.
    assert!(store.save(&before[0]).await.unwrap());
    drop(client);
    drop(grant);
    serving.await.unwrap();
    drop(release_child);
    drop(release_source);
    retire(service, storage, metadata).await;
    drop(store);
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(&root, provider);
    service.bind_commands(storage.clone());
    let store = Arc::new(LocalThreadReports::open(&root.join("reports.sqlite3")).unwrap());
    service.bind_thread_reports(store.clone());
    assert_eq!(service.resume_thread_reports().await.unwrap(), 1);
    assert_eq!(reports_in(&service, &source, 1).await.len(), 1);
    assert!(store.pending().await.unwrap().is_empty());
    retire(service, storage, metadata).await;
}
