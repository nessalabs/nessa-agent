//! Source-bound admission shares the real mutation-command fixture and provider.
use super::*;
use crate::composition::conversation_tools::ConversationTools;
use crate::conversation::application::{AgentToolAudit, AgentToolAuditRecord, ConversationFuture};
use crate::mcp_servers::{
    domain::ConfigurationKey,
    infrastructure::{
        built_in_digest, read_line, write_line, Answer, ConversationGrants, Hello, OsTokens, Relay,
    },
};
use nessa_sdk::{
    application::agent_execution::executions::{
        ExecutionUpdate, SubmissionMode as InvocationSubmissionMode,
    },
    domain::agent_execution::{
        sessions::SessionId,
        tools::{McpTool, ToolCallId, ToolCallUpdate, ToolStatus},
    },
    infrastructure::{acp::sessions::StandInGrants, clock::RuntimeClock, mcp::McpServers},
};
use serde_json::{json, Value};
use std::sync::{atomic::AtomicBool, Mutex};
use tokio::io::{BufReader, DuplexStream, ReadHalf, WriteHalf};

#[derive(Default)]
struct Audit {
    records: Mutex<Vec<AgentToolAuditRecord>>,
    fail: bool,
}
impl AgentToolAudit for Audit {
    fn record(&self, record: AgentToolAuditRecord) -> ConversationFuture<'_, ()> {
        self.records.lock().unwrap().push(record);
        Box::pin(async move {
            if self.fail {
                Err(ConversationError::Audit)
            } else {
                Ok(())
            }
        })
    }
}

#[tokio::test]
async fn source_admission_preserves_the_running_owner_actor_and_target() {
    let directory = tempfile::tempdir().unwrap();
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(directory.path(), provider.clone());
    let source = id();
    let target = id();
    opened(&service, &provider, &source).await;
    let release = hold_turn(&service, &provider, &source, "source-turn").await;
    let audit = Arc::new(Audit::default());
    service.bind_agent_tool_audit(audit.clone());
    let (_, stop) = tokio::sync::watch::channel(false);
    let admitted = service
        .admit_agent_tool(
            source.clone(),
            "tool-call".into(),
            "send_message".into(),
            Some(target.clone()),
            "stable-request".into(),
            stop,
        )
        .await
        .unwrap();
    assert_eq!(
        admitted.caller().principal_id,
        caller("expected").principal_id
    );
    assert_eq!(admitted.caller().surface_id, "panel");
    assert_eq!(admitted.caller().action_id, "stable-request");
    let records = audit.records.lock().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].source, source);
    assert_eq!(records[0].target, Some(target));
    assert_eq!(records[0].execution, "source-turn");
    assert!(records[0].allowed);
    assert_eq!(records[0].policies, ["owner.agent_tools"]);
    drop(records);
    drop(release);
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn source_without_a_running_turn_and_a_cancelled_call_admit_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(directory.path(), provider.clone());
    let source = id();
    opened(&service, &provider, &source).await;
    let audit = Arc::new(Audit::default());
    service.bind_agent_tool_audit(audit.clone());
    for cancelled in [false, true] {
        let (_, stop) = tokio::sync::watch::channel(cancelled);
        assert!(service
            .admit_agent_tool(
                source.clone(),
                "call".into(),
                "create_thread".into(),
                None,
                "request".into(),
                stop
            )
            .await
            .is_err());
    }
    assert!(audit.records.lock().unwrap().is_empty());
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn admission_audit_failure_is_visible_before_a_target_command_can_be_called() {
    let directory = tempfile::tempdir().unwrap();
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(directory.path(), provider.clone());
    let source = id();
    opened(&service, &provider, &source).await;
    let release = hold_turn(&service, &provider, &source, "source-turn").await;
    service.bind_agent_tool_audit(Arc::new(Audit {
        fail: true,
        ..Default::default()
    }));
    let (_, stop) = tokio::sync::watch::channel(false);
    assert!(matches!(
        service
            .admit_agent_tool(
                source,
                "call".into(),
                "create_thread".into(),
                Some(id()),
                "request".into(),
                stop
            )
            .await,
        Err(ConversationError::Audit)
    ));
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);
    drop(release);
    retire(service, storage, metadata).await;
}

struct McpClient {
    input: BufReader<ReadHalf<DuplexStream>>,
    output: WriteHalf<DuplexStream>,
    next: u64,
}
impl McpClient {
    async fn call(&mut self, tool: &str, arguments: Value) -> Value {
        self.next += 1;
        write_line(&mut self.output, &json!({"jsonrpc":"2.0","id":self.next,"method":"tools/call","params":{"name":tool,"arguments":arguments}})).await.unwrap();
        let answer =
            tokio::time::timeout(Duration::from_secs(30), read_line::<Value>(&mut self.input))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(answer["id"], self.next);
        answer["result"].clone()
    }
}

/// The model-facing path carries only a source grant, then uses actual service
/// command coordinators and committed reads. A fast worker emits a tool event.
#[tokio::test]
async fn agent_tools_relay_create_send_replay_and_refuse_a_foreign_target() {
    let directory = tempfile::tempdir().unwrap();
    let provider = Arc::new(ProviderFactory::default());
    provider.turn_cancel.store(true, Ordering::SeqCst);
    let (service, storage, metadata) = fixture(directory.path(), provider.clone());
    service.bind_commands(storage.clone());
    let audit = Arc::new(Audit::default());
    service.bind_agent_tool_audit(audit.clone());
    let source = id();
    opened(&service, &provider, &source).await;
    let release = hold_turn(&service, &provider, &source, "source-turn").await;
    let tools = ConversationTools::new();
    tools.attach(service.clone());
    let servers = McpServers::new(Vec::new(), Arc::new(RuntimeClock::new())).unwrap();
    let grants = ConversationGrants::new(servers.clone(), Arc::new(OsTokens));
    let grant = grants.grant(&SessionId::new(&source.to_string()).unwrap());
    let key = ConfigurationKey::new([9; 32]);
    let digest = built_in_digest(&key, tools.as_ref());
    let relay = Arc::new(Relay::new(servers, grants, key).with_built_in(tools));
    let (client, server) = tokio::io::duplex(1 << 20);
    let serving = tokio::spawn(async move { relay.serve(server).await });
    let (input, mut output) = tokio::io::split(client);
    let mut input = BufReader::new(input);
    write_line(
        &mut output,
        &Hello {
            server: "nessa-agents".into(),
            configuration: digest,
            session: grant.environment()[0].1.clone(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        read_line::<Answer>(&mut input).await.unwrap(),
        Answer::Accepted
    );
    let mut client = McpClient {
        input,
        output,
        next: 0,
    };
    let created = client
        .call("create_thread", json!({"requestId":"worker-create"}))
        .await;
    assert_eq!(created["isError"], false, "{created}");
    let worker = created["structuredContent"]["threadId"]
        .as_str()
        .unwrap()
        .to_owned();
    let before_passive = provider.executions.lock().unwrap().clone();
    let passive = client.call("send_message",json!({"threadId":worker,"message":"context only","mode":"passive","requestId":"worker-note"})).await;
    assert_eq!(passive["isError"], false, "{passive}");
    assert_eq!(passive["structuredContent"]["deliveryState"], "recorded");
    assert!(passive["structuredContent"].get("executionId").is_none());
    let passive_retry = client.call("send_message",json!({"threadId":worker,"message":"context only","mode":"passive","requestId":"worker-note"})).await;
    assert_eq!(
        passive_retry["structuredContent"]["messageId"],
        passive["structuredContent"]["messageId"]
    );
    let changed_passive = client.call("send_message",json!({"threadId":worker,"message":"different context","mode":"passive","requestId":"worker-note"})).await;
    assert_eq!(
        changed_passive["structuredContent"]["error"],
        "idempotency_conflict"
    );
    assert_eq!(
        *provider.executions.lock().unwrap(),
        before_passive,
        "passive delivery starts no model turn"
    );
    provider
        .execution_updates
        .lock()
        .unwrap()
        .push(ExecutionUpdate::Tool(
            ToolCallUpdate::new(
                ToolCallId::new("worker-tool").unwrap(),
                None,
                None,
                Some(ToolStatus::Completed),
                None,
                None,
            )
            .with_mcp_tool(McpTool::new("worker-mcp", "echo").unwrap()),
        ));
    let sent = client
        .call(
            "send_message",
            json!({"threadId":worker,"message":"work","requestId":"worker-send"}),
        )
        .await;
    assert_eq!(sent["isError"], false, "{sent}");
    let execution = sent["structuredContent"]["executionId"]
        .as_str()
        .unwrap()
        .to_owned();
    let worker_id = ConversationId::new(&worker).unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let submission = service
                .current_submission(worker_id.clone(), caller("check"), execution.clone())
                .await
                .unwrap();
            if matches!(submission.disposition, ConversationDisposition::Settled) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let mut after: Option<String> = None;
    let mut kinds = Vec::new();
    let mut observed_tool = false;
    loop {
        let args = match &after {
            None => json!({"threadId":worker,"replay":true}),
            Some(cursor) => json!({"threadId":worker,"after":cursor}),
        };
        let batch = client.call("subscribe", args).await;
        assert_eq!(batch["isError"], false, "{batch}");
        let batch = &batch["structuredContent"];
        for event in batch["events"].as_array().unwrap() {
            kinds.push(event["kind"].as_str().unwrap().to_owned());
            if event["toolId"] == "worker-tool" {
                observed_tool = true;
                assert_eq!(event["name"], "echo");
            }
        }
        after = Some(batch["cursor"].as_str().unwrap().into());
        if batch["more"] == false {
            break;
        }
    }
    assert!(kinds.iter().any(|kind| kind == "turn.accepted"));
    assert!(kinds.iter().any(|kind| kind == "turn.started"));
    assert!(kinds.iter().any(|kind| kind == "turn.ended"));
    assert!(observed_tool);
    let duplicate = client
        .call(
            "send_message",
            json!({"threadId":worker,"message":"work","requestId":"worker-send"}),
        )
        .await;
    assert_eq!(duplicate["structuredContent"]["executionId"], execution);
    assert_eq!(
        provider
            .executions
            .lock()
            .unwrap()
            .iter()
            .filter(|turn| **turn == execution)
            .count(),
        1
    );
    let changed_active_mode = client
        .call(
            "send_message",
            json!({"threadId":worker,"message":"work","mode":"steering","requestId":"worker-send"}),
        )
        .await;
    assert_eq!(
        changed_active_mode["structuredContent"]["error"],
        "request_conflict"
    );
    let stopped = client
        .call(
            "stop_thread",
            json!({"threadId":worker,"executionId":execution,"requestId":"worker-stop"}),
        )
        .await;
    assert_eq!(stopped["isError"], false, "{stopped}");
    assert_eq!(
        stopped["structuredContent"]["command"]["outcome"],
        "alreadyFinal"
    );
    assert_eq!(provider.cancel_calls.load(Ordering::SeqCst), 0);
    let no_active = client.call("stop_thread", json!({"threadId":worker})).await;
    assert_eq!(no_active["structuredContent"]["error"], "no_active_turn");
    let invalid_replay = client
        .call(
            "subscribe",
            json!({"threadId":worker,"after":after,"replay":true}),
        )
        .await;
    assert_eq!(
        invalid_replay["structuredContent"]["error"],
        "invalid_request"
    );
    let invalid_wait = client
        .call("subscribe", json!({"threadId":worker,"waitMs":30001}))
        .await;
    assert_eq!(
        invalid_wait["structuredContent"]["error"],
        "invalid_request"
    );
    // Hold a real worker execution, steer through the provider's existing
    // non-native fallback, then stop the captured running parent through MCP.
    let (release_active, active_gate) = oneshot::channel();
    *provider.execution_gate.lock().unwrap() = Some(active_gate);
    let active = client
        .call(
            "send_message",
            json!({"threadId":worker,"message":"stay active","requestId":"worker-active"}),
        )
        .await;
    assert_eq!(active["isError"], false, "{active}");
    let active_execution = active["structuredContent"]["executionId"]
        .as_str()
        .unwrap()
        .to_owned();
    tokio::time::timeout(Duration::from_secs(10), async {
        while !provider
            .executions
            .lock()
            .unwrap()
            .contains(&active_execution)
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let steering = client.call("send_message",json!({"threadId":worker,"message":"adjust direction","mode":"steering","requestId":"worker-steer"})).await;
    assert_eq!(steering["isError"], false, "{steering}");
    let steering_execution = steering["structuredContent"]["executionId"]
        .as_str()
        .unwrap()
        .to_owned();
    let live = service
        .resolve(&worker_id, &caller("verify-steer"))
        .await
        .unwrap();
    let snapshot = live.agent.session_manager().snapshot().await.unwrap();
    let recorded_steer = snapshot
        .invocations
        .iter()
        .find(|record| record.request.execution_id.as_str() == steering_execution)
        .unwrap();
    assert_eq!(
        recorded_steer.submission,
        InvocationSubmissionMode::Steering
    );
    assert!(
        live.agent
            .queued_ids()
            .await
            .iter()
            .any(|queued| queued.as_str() == steering_execution),
        "the provider's non-native steer fallback stays queued behind its running parent"
    );
    let steer_retry = client.call("send_message",json!({"threadId":worker,"message":"adjust direction","mode":"steering","requestId":"worker-steer"})).await;
    assert_eq!(
        steer_retry["structuredContent"]["executionId"],
        steering_execution
    );
    let changed_steer = client.call("send_message",json!({"threadId":worker,"message":"adjust direction","mode":"queued","requestId":"worker-steer"})).await;
    assert_eq!(
        changed_steer["structuredContent"]["error"],
        "request_conflict"
    );
    let active_stop = client
        .call(
            "stop_thread",
            json!({"threadId":worker,"requestId":"stop-active"}),
        )
        .await;
    assert_eq!(active_stop["isError"], false, "{active_stop}");
    assert_eq!(
        active_stop["structuredContent"]["executionId"],
        active_execution
    );
    assert_eq!(
        active_stop["structuredContent"]["command"]["outcome"],
        "cancelled"
    );
    assert_eq!(provider.cancel_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        provider.cancel_turns.lock().unwrap().as_slice(),
        [active_execution.clone()]
    );
    let retry_stop = client
        .call(
            "stop_thread",
            json!({"threadId":worker,"executionId":active_execution,"requestId":"stop-active"}),
        )
        .await;
    assert_eq!(
        retry_stop["structuredContent"]["command"]["outcome"],
        "cancelled"
    );
    assert_eq!(provider.cancel_calls.load(Ordering::SeqCst), 1);
    drop(release_active);
    let foreign = id();
    let mut stranger = caller("foreign-create");
    stranger.principal_id = PrincipalId::new("stranger").unwrap();
    service
        .create(foreign.clone(), stranger, RequestedConversation::default())
        .await
        .unwrap();
    let refused = client
        .call(
            "send_message",
            json!({"threadId":foreign.to_string(),"message":"unauthorized"}),
        )
        .await;
    assert_eq!(refused["isError"], true);
    assert_eq!(
        refused["structuredContent"]["error"],
        "conversation_not_found"
    );
    assert!(audit
        .records
        .lock()
        .unwrap()
        .iter()
        .all(|record| record.source == source && record.execution == "source-turn"));
    drop(client);
    drop(grant);
    serving.await.unwrap();
    drop(release);
    retire(service, storage, metadata).await;
}

struct HeldAudit {
    entered: tokio::sync::Notify,
    release: Mutex<Option<oneshot::Receiver<()>>>,
    recorded: AtomicBool,
}
impl AgentToolAudit for HeldAudit {
    fn record(&self, _: AgentToolAuditRecord) -> ConversationFuture<'_, ()> {
        let release = self.release.lock().unwrap().take().unwrap();
        self.entered.notify_one();
        Box::pin(async move {
            release.await.map_err(|_| ConversationError::Audit)?;
            self.recorded.store(true, Ordering::SeqCst);
            Ok(())
        })
    }
}

#[tokio::test]
async fn admitted_agent_tool_audit_finishes_after_its_response_caller_is_lost() {
    let directory = tempfile::tempdir().unwrap();
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(directory.path(), provider.clone());
    let source = id();
    opened(&service, &provider, &source).await;
    let release_turn = hold_turn(&service, &provider, &source, "source-turn").await;
    let (release, wait) = oneshot::channel();
    let audit = Arc::new(HeldAudit {
        entered: tokio::sync::Notify::new(),
        release: Mutex::new(Some(wait)),
        recorded: false.into(),
    });
    service.bind_agent_tool_audit(audit.clone());
    let entered = audit.entered.notified();
    let pending = {
        let service = service.clone();
        tokio::spawn(async move {
            let (_sender, stop) = tokio::sync::watch::channel(false);
            service
                .admit_agent_tool(
                    source,
                    "call".into(),
                    "create_thread".into(),
                    Some(id()),
                    "request".into(),
                    stop,
                )
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(10), entered)
        .await
        .unwrap();
    pending.abort();
    let _ = pending.await;
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while !audit.recorded.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    drop(release_turn);
    retire(service, storage, metadata).await;
}
