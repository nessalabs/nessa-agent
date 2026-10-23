//! Source-bound admission shares the real mutation-command fixture and provider.
use super::*;
#[cfg(unix)]
use crate::composition::conversation_tools::ConversationTools;
use crate::conversation::application::{
    AgentToolAudit, AgentToolAuditRecord, AgentToolRequest, ConversationFuture,
};
#[cfg(unix)]
use crate::conversation::application::{ReportRegistration, ThreadReport};
#[cfg(unix)]
use crate::mcp_servers::{
    domain::ConfigurationKey,
    infrastructure::{
        built_in_digest, read_line, write_line, Answer, ConversationGrants, Hello, OsTokens, Relay,
    },
};
#[cfg(unix)]
use nessa_sdk::application::agent_execution::providers;
#[cfg(unix)]
use nessa_sdk::{
    application::agent_execution::executions::{
        ExecutionUpdate, SubmissionMode as InvocationSubmissionMode,
    },
    domain::agent_execution::{
        executions::MessageChunk,
        sessions::SessionId,
        tools::{McpTool, ToolCallId, ToolCallUpdate, ToolStatus},
    },
    infrastructure::{acp::sessions::StandInGrants, clock::RuntimeClock, mcp::McpServers},
};
#[cfg(unix)]
use serde_json::{json, Value};
use std::sync::{atomic::AtomicBool, Mutex};
#[cfg(unix)]
use tokio::io::{AsyncBufReadExt, BufReader, DuplexStream, ReadHalf, WriteHalf};

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
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(&root, provider.clone());
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
            AgentToolRequest {
                logical: "logical-request".into(),
                command: "stable-request".into(),
            },
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
    {
        let records = audit.records.lock().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].source, source);
        assert_eq!(records[0].target, Some(target));
        assert_eq!(records[0].execution, "source-turn");
        assert_eq!(records[0].request, "logical-request");
        assert_eq!(records[0].command, "stable-request");
        assert!(records[0].allowed);
        assert_eq!(records[0].policies, ["owner.agent_tools"]);
    }
    drop(release);
    retire(service, storage, metadata).await;
}

#[derive(Clone, Copy, Debug)]
enum SourceChange {
    Unchanged,
    Fenced,
    Cancelled,
    MetadataFailure,
}

/// The initial metadata reply is stale while live() waits; a real deletion
/// fence commits without stopping the retained Agent or changing its snapshot.
#[tokio::test]
async fn source_admission_reloads_metadata_after_waiting_for_live_evidence() {
    for change in [
        SourceChange::Unchanged,
        SourceChange::Fenced,
        SourceChange::Cancelled,
        SourceChange::MetadataFailure,
    ] {
        let (service, provider, metadata, _storage) =
            crate::conversation_test_support::fixture(ConversationLimits::default());
        let source = id();
        let target = id();
        opened(&service, &provider, &source).await;
        let release_turn = hold_turn(&service, &provider, &source, "source-turn").await;
        let live = service.resolve(&source, &caller("capture")).await.unwrap();
        let snapshot = live.agent.session_manager().snapshot().await.unwrap();
        let lease = snapshot
            .lease
            .as_ref()
            .unwrap()
            .held()
            .unwrap()
            .id()
            .as_str()
            .to_owned();
        let opens = provider.open_calls.load(Ordering::SeqCst);
        let audit = Arc::new(Audit::default());
        service.bind_agent_tool_audit(audit.clone());
        let (entered, initial_read) = oneshot::channel();
        let (release, initial_release) = oneshot::channel();
        *metadata.metadata_load_gate.lock().unwrap() = Some((entered, initial_release));
        let (stop, stopped) = tokio::sync::watch::channel(false);
        let pending = {
            let service = service.clone();
            let source = source.clone();
            let target = target.clone();
            tokio::spawn(async move {
                let admitted = service
                    .admit_agent_tool(
                        source,
                        "delivery-call".into(),
                        "create_thread".into(),
                        Some(target.clone()),
                        AgentToolRequest {
                            logical: "raw-logical".into(),
                            command: "scoped-command".into(),
                        },
                        stopped,
                    )
                    .await?;
                service
                    .create(target, admitted.caller(), RequestedConversation::default())
                    .await
            })
        };
        initial_read.await.unwrap();
        // The captured initial reply cannot become source authority merely
        // because live() was delayed behind an unrelated map holder.
        let map = service.inner.conversations.lock().await;
        release.send(()).unwrap();
        let fence = match change {
            SourceChange::Fenced => {
                let fence = service
                    .fence(&source, &caller("delete-source"))
                    .await
                    .unwrap();
                assert!(metadata
                    .records
                    .lock()
                    .unwrap()
                    .get(&source)
                    .unwrap()
                    .deletion()
                    .is_some());
                assert_eq!(
                    live.agent.active_execution_id().unwrap().as_str(),
                    "source-turn"
                );
                Some(fence)
            }
            SourceChange::Cancelled => {
                stop.send_replace(true);
                None
            }
            SourceChange::MetadataFailure => {
                *metadata.metadata_load_failure.lock().unwrap() = Some(ConversationError::Metadata);
                None
            }
            SourceChange::Unchanged => None,
        };
        drop(map);
        let result = tokio::time::timeout(TURN_HOLD_HANG, pending)
            .await
            .unwrap()
            .unwrap();
        match change {
            SourceChange::Unchanged => assert!(result.is_ok(), "{result:?}"),
            SourceChange::Fenced => assert!(
                matches!(&result, Err(ConversationError::NotFound)),
                "{result:?}"
            ),
            SourceChange::Cancelled => {
                assert!(
                    matches!(&result, Err(ConversationError::Unavailable)),
                    "{result:?}"
                )
            }
            SourceChange::MetadataFailure => {
                assert!(
                    matches!(&result, Err(ConversationError::Metadata)),
                    "{result:?}"
                )
            }
        }
        let created = metadata.records.lock().unwrap().contains_key(&target);
        assert_eq!(
            created,
            matches!(change, SourceChange::Unchanged),
            "{change:?}"
        );
        if !created {
            assert_eq!(
                provider.open_calls.load(Ordering::SeqCst),
                opens,
                "no target provider dispatch: {change:?}"
            );
        }
        {
            let records = audit.records.lock().unwrap();
            if matches!(change, SourceChange::MetadataFailure) {
                assert!(
                    records.is_empty(),
                    "an unreadable source cannot invent a policy decision"
                );
            } else {
                assert_eq!(records.len(), 1);
                let record = &records[0];
                assert_eq!(record.source, source);
                assert_eq!(record.target, Some(target));
                assert_eq!(record.execution, "source-turn");
                assert_eq!(record.lease, lease);
                assert_eq!(record.call, "delivery-call");
                assert_eq!(record.request, "raw-logical");
                assert_eq!(record.command, "scoped-command");
                assert_eq!(record.tool, "create_thread");
                assert_eq!(record.principal, "person");
                assert_eq!(record.surface, "panel");
                assert!(record.owner_initiated);
                assert_eq!(record.allowed, !matches!(change, SourceChange::Fenced));
                if record.allowed {
                    assert_eq!(record.policies, ["owner.agent_tools"]);
                    assert_eq!(record.reason, None);
                } else {
                    assert!(record.policies.is_empty());
                    assert_eq!(record.reason.as_deref(), Some("not_permitted"));
                }
            }
        }
        drop(fence);
        drop(release_turn);
        service.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn source_without_a_running_turn_and_a_cancelled_call_admit_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(&root, provider.clone());
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
                AgentToolRequest {
                    logical: "request".into(),
                    command: "command".into()
                },
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
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(&root, provider.clone());
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
                AgentToolRequest {
                    logical: "request".into(),
                    command: "command".into()
                },
                stop
            )
            .await,
        Err(ConversationError::Audit)
    ));
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);
    drop(release);
    retire(service, storage, metadata).await;
}

#[cfg(unix)]
struct McpClient {
    input: BufReader<ReadHalf<DuplexStream>>,
    output: WriteHalf<DuplexStream>,
    next: u64,
}
#[cfg(unix)]
impl McpClient {
    async fn connect(
        relay: Arc<Relay>,
        digest: String,
        token: String,
    ) -> (Self, tokio::task::JoinHandle<()>) {
        let (client, server) = tokio::io::duplex(1 << 20);
        let serving = tokio::spawn(async move { relay.serve(server).await });
        let (input, mut output) = tokio::io::split(client);
        let mut input = BufReader::new(input);
        write_line(
            &mut output,
            &Hello {
                server: "nessa-agents".into(),
                configuration: digest,
                session: token,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            read_line::<Answer>(&mut input).await.unwrap(),
            Answer::Accepted
        );
        (
            Self {
                input,
                output,
                next: 0,
            },
            serving,
        )
    }
    async fn call(&mut self, tool: &str, arguments: Value) -> Value {
        self.next += 1;
        write_line(&mut self.output, &json!({"jsonrpc":"2.0","id":self.next,"method":"tools/call","params":{"name":tool,"arguments":arguments}})).await.unwrap();
        let answer = tokio::time::timeout(Duration::from_secs(30), async {
            // An answer is not a hello: `read_line` takes at most a hello's
            // bytes, and a page of events is longer than that.
            let mut line = Vec::new();
            self.input.read_until(b'\n', &mut line).await.unwrap();
            serde_json::from_slice::<Value>(&line).unwrap()
        })
        .await
        .unwrap();
        assert_eq!(answer["id"], self.next);
        answer["result"].clone()
    }
}

/// The model-facing path carries only a source grant, then uses actual service
/// command coordinators and committed reads. A fast worker emits a tool event.
#[cfg(unix)]
#[tokio::test]
async fn agent_tools_relay_create_send_replay_and_refuse_a_foreign_target() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    provider.turn_cancel.store(true, Ordering::SeqCst);
    let (service, storage, metadata) = fixture(&root, provider.clone());
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
    let grant = grants.grant(&SessionId::new(source.to_string()).unwrap());
    let key = ConfigurationKey::new([9; 32]);
    let digest = built_in_digest(&key, tools.as_ref());
    let relay = Arc::new(Relay::new(servers, grants.clone(), key).with_built_in(tools));
    let (mut client, serving) = McpClient::connect(
        relay.clone(),
        digest.clone(),
        grant.environment()[0].1.clone(),
    )
    .await;
    let before_unknown_agent = audit.records.lock().unwrap().len();
    let unknown_agent = client
        .call(
            "create_thread",
            json!({"requestId":"unknown-agent","agent":"not-an-agent"}),
        )
        .await;
    assert_eq!(
        unknown_agent["structuredContent"]["error"], "invalid_request",
        "{unknown_agent}"
    );
    assert_eq!(audit.records.lock().unwrap().len(), before_unknown_agent);
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
    for replay in [true, false] {
        let invalid_replay = client
            .call(
                "subscribe",
                json!({"threadId":worker,"after":after,"replay":replay}),
            )
            .await;
        assert_eq!(
            invalid_replay["structuredContent"]["error"], "invalid_request",
            "after and replay:{replay} are mutually exclusive: {invalid_replay}"
        );
    }
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
    let before_null_stop = audit.records.lock().unwrap().len();
    let before_null_cancel = provider.cancel_calls.load(Ordering::SeqCst);
    let null_stop = client
        .call(
            "stop_thread",
            json!({"threadId":worker,"executionId":null,"requestId":"null-stop"}),
        )
        .await;
    assert_eq!(
        null_stop["structuredContent"]["error"], "invalid_request",
        "{null_stop}"
    );
    assert_eq!(audit.records.lock().unwrap().len(), before_null_stop);
    assert_eq!(
        provider.cancel_calls.load(Ordering::SeqCst),
        before_null_cancel
    );
    assert_eq!(
        service
            .active_execution(worker_id.clone(), caller("null-stop-check"))
            .await
            .unwrap(),
        Some(active_execution.clone())
    );
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
        InvocationSubmissionMode::BoundarySteering
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
        std::slice::from_ref(&active_execution)
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
    // The fallback steer is ordinary queued work. Let it settle before reusing
    // this fixture's shared provider gate for a different source conversation.
    let steering_turn = ExecutionId::new(steering_execution).unwrap();
    tokio::time::timeout(TURN_HOLD_HANG, async {
        while super::super::turn_is_final(&live, &steering_turn).await != Some(true) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("queued steering settles before another source uses the provider gate");
    // The sends above report back into the source. Let those land, so the
    // refusal below meets no other command of the same person.
    tokio::time::timeout(TURN_HOLD_HANG, async {
        while service.inner.thread_reports.waiting() > 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("reports of the earlier sends were queued");
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
    // The same explicit logical request stays scoped to each tool command.
    let same_request_create = client
        .call("create_thread", json!({"requestId":"worker-send"}))
        .await;
    assert_eq!(
        same_request_create["isError"], false,
        "{same_request_create}"
    );
    let same_request_stop = client
        .call(
            "stop_thread",
            json!({"threadId":worker,"executionId":execution,"requestId":"worker-send"}),
        )
        .await;
    assert_eq!(same_request_stop["isError"], false, "{same_request_stop}");
    let same_request_note = client.call("send_message", json!({"threadId":worker,"message":"separate note operation","mode":"passive","requestId":"worker-send"})).await;
    assert_eq!(same_request_note["isError"], false, "{same_request_note}");
    {
        let records = audit.records.lock().unwrap();
        assert!(records
            .iter()
            .all(|record| record.source == source && record.execution == "source-turn"));
        for (request, tool) in [
            ("worker-create", "create_thread"),
            ("worker-send", "send_message"),
            ("worker-stop", "stop_thread"),
            ("worker-note", "send_message"),
            ("worker-steer", "send_message"),
        ] {
            let matching: Vec<_> = records
                .iter()
                .filter(|record| record.request == request && record.tool == tool)
                .collect();
            assert!(
                !matching.is_empty(),
                "missing raw request {request} for {tool}"
            );
            assert!(matching
                .iter()
                .all(|record| record.command == matching[0].command));
            assert!(uuid::Uuid::parse_str(&matching[0].command).is_ok());
        }
        let command = |tool| {
            records
                .iter()
                .find(|record| record.request == "worker-send" && record.tool == tool)
                .unwrap()
                .command
                .clone()
        };
        assert_ne!(command("create_thread"), command("send_message"));
        assert_ne!(command("stop_thread"), command("send_message"));
        assert_ne!(command("create_thread"), command("stop_thread"));
    }
    // A second live source uses the same Relay and raw request, but owns a
    // different command and created target. No model-facing source argument exists.
    let other_source = id();
    opened(&service, &provider, &other_source).await;
    let release_other = hold_turn(&service, &provider, &other_source, "other-source-turn").await;
    let other_live = service
        .resolve(&other_source, &caller("other-read"))
        .await
        .unwrap();
    tokio::time::timeout(TURN_HOLD_HANG, async {
        while other_live
            .agent
            .active_execution_id()
            .as_ref()
            .map(ExecutionId::as_str)
            != Some("other-source-turn")
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the second source is authoritatively active before its Relay call");
    let other_grant = grants.grant(&SessionId::new(other_source.to_string()).unwrap());
    let (mut other_client, other_serving) =
        McpClient::connect(relay, digest, other_grant.environment()[0].1.clone()).await;
    let other_created = other_client
        .call("create_thread", json!({"requestId":"worker-create"}))
        .await;
    assert_eq!(other_created["isError"], false, "{other_created}");
    assert_ne!(
        other_created["structuredContent"]["threadId"],
        created["structuredContent"]["threadId"]
    );
    {
        let records = audit.records.lock().unwrap();
        let original = records
            .iter()
            .find(|record| record.source == source && record.request == "worker-create")
            .unwrap();
        let other = records
            .iter()
            .find(|record| record.source == other_source && record.request == "worker-create")
            .unwrap();
        assert_ne!(original.command, other.command);
        assert_eq!(other.execution, "other-source-turn");
    }
    drop(other_client);
    drop(other_grant);
    other_serving.await.unwrap();
    drop(release_other);
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
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(&root, provider.clone());
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
                    AgentToolRequest {
                        logical: "request".into(),
                        command: "command".into(),
                    },
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

/// The source's saved turns whose message is a thread report, once there are
/// `count` of them and no report waits.
#[cfg(unix)]
async fn reports_in(
    service: &ConversationService,
    source: &ConversationId,
    count: usize,
) -> Vec<String> {
    tokio::time::timeout(TURN_HOLD_HANG, async {
        loop {
            let live = service.resolve(source, &caller("reports")).await.unwrap();
            let snapshot = live.agent.session_manager().snapshot().await.unwrap();
            let reports: Vec<_> = snapshot
                .invocations
                .iter()
                .map(|record| record.request.user_message.text_str().to_owned())
                .filter(|text| text.starts_with("[Thread report]"))
                .collect();
            if reports.len() >= count && service.inner.thread_reports.waiting() == 0 {
                return reports;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the reports were queued into the source")
}

/// A turn an agent sends to another thread reports back when it ends: its
/// outcome and final reply are queued into the sender as a new turn, once,
/// however often the send is retried; a steering send reports as well.
#[cfg(unix)]
#[tokio::test]
async fn a_thread_turn_reports_its_outcome_and_reply_back_to_the_sender() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    provider.turn_cancel.store(true, Ordering::SeqCst);
    let (service, storage, metadata) = fixture(&root, provider.clone());
    service.bind_commands(storage.clone());
    service.bind_agent_tool_audit(Arc::new(Audit::default()));
    let source = id();
    opened(&service, &provider, &source).await;
    let release = hold_turn(&service, &provider, &source, "source-turn").await;
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
    let created = client
        .call("create_thread", json!({"requestId":"report-create"}))
        .await;
    assert_eq!(created["isError"], false, "{created}");
    let worker = created["structuredContent"]["threadId"]
        .as_str()
        .unwrap()
        .to_owned();
    let worker_id = ConversationId::new(&worker).unwrap();
    provider.execution_updates.lock().unwrap().extend([
        ExecutionUpdate::Message(MessageChunk::thought("weighing it")),
        ExecutionUpdate::Message(MessageChunk::text("Let me look that up.")),
        ExecutionUpdate::Tool(ToolCallUpdate::new(
            ToolCallId::new("lookup").unwrap(),
            None,
            None,
            Some(ToolStatus::Completed),
            None,
            None,
        )),
        ExecutionUpdate::Message(MessageChunk::text("The answer ")),
        ExecutionUpdate::Message(MessageChunk::text("is 42.")),
    ]);
    let sent = client
        .call(
            "send_message",
            json!({"threadId":worker,"message":"work","requestId":"report-send"}),
        )
        .await;
    assert_eq!(sent["isError"], false, "{sent}");
    assert_eq!(sent["structuredContent"]["reportsBack"], true);
    let execution = sent["structuredContent"]["executionId"]
        .as_str()
        .unwrap()
        .to_owned();
    let reports = reports_in(&service, &source, 1).await;
    assert_eq!(reports.len(), 1, "{reports:?}");
    assert!(reports[0].contains(&worker), "{}", reports[0]);
    assert!(reports[0].contains(&execution), "{}", reports[0]);
    assert!(reports[0].contains(": completed."), "{}", reports[0]);
    assert!(
        reports[0].contains("Its reply:\nThe answer is 42."),
        "the final visible answer is carried: {}",
        reports[0]
    );
    assert!(
        !reports[0].contains("Let me look") && !reports[0].contains("weighing"),
        "text before the last tool step and thinking stay out: {}",
        reports[0]
    );
    // A retry of the finished send answers from the same report.
    let retried = client
        .call(
            "send_message",
            json!({"threadId":worker,"message":"work","requestId":"report-send"}),
        )
        .await;
    assert_eq!(retried["structuredContent"]["executionId"], execution);
    assert_eq!(reports_in(&service, &source, 1).await.len(), 1);
    // A refused retry of the same request leaves nothing waiting.
    let conflict = client
        .call(
            "send_message",
            json!({"threadId":worker,"message":"changed","requestId":"report-send"}),
        )
        .await;
    assert_eq!(conflict["structuredContent"]["error"], "request_conflict");
    assert_eq!(reports_in(&service, &source, 1).await.len(), 1);

    // A steer taken into a running turn reports that turn's outcome and
    // reply once it settles, though a queued turn admitted in between is
    // still waiting.
    provider.native_steering.store(true, Ordering::SeqCst);
    provider.inject_steers.store(true, Ordering::SeqCst);
    let release_worker = hold_turn(&service, &provider, &worker_id, "worker-active").await;
    send(&service, &worker_id, "worker-queued", "after").await;
    let steered = client
        .call(
            "send_message",
            json!({"threadId":worker,"message":"also this","mode":"steering","requestId":"report-steer"}),
        )
        .await;
    assert_eq!(steered["isError"], false, "{steered}");
    assert_eq!(steered["structuredContent"]["reportsBack"], true);
    let steer = steered["structuredContent"]["executionId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(service.inner.thread_reports.waiting(), 1);
    provider
        .execution_updates
        .lock()
        .unwrap()
        .push(ExecutionUpdate::Message(MessageChunk::text("Host answer.")));
    drop(release_worker);
    let reports = reports_in(&service, &source, 2).await;
    assert_eq!(reports.len(), 2, "{reports:?}");
    assert!(reports[1].contains(&steer), "{}", reports[1]);
    assert!(reports[1].contains(": completed."), "{}", reports[1]);
    assert!(
        reports[1].contains("Its reply:\nHost answer."),
        "the host turn's reply is the steer's: {}",
        reports[1]
    );
    let worker_turns = service
        .resolve(&worker_id, &caller("reports"))
        .await
        .unwrap()
        .agent
        .session_manager()
        .snapshot()
        .await
        .unwrap();
    let steer_record = worker_turns
        .invocations
        .iter()
        .find(|record| record.request.execution_id.as_str() == steer)
        .unwrap();
    assert_eq!(
        steer_record.scheduling.last().map(|event| event.stage),
        Some(InvocationStage::Injected),
        "the steer was taken into the running turn"
    );
    // The queued turn after it runs and adds no second report.
    tokio::time::timeout(TURN_HOLD_HANG, async {
        while !provider
            .executions
            .lock()
            .unwrap()
            .iter()
            .any(|execution| execution == "worker-queued")
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the queued turn ran");
    assert_eq!(reports_in(&service, &source, 2).await.len(), 2);

    // A queued turn the thread drops because the turn ahead of it failed
    // reports as cancelled, though no invocation follows the drop.
    *provider.execution_reply.lock().unwrap() = Some(providers::ProviderExecutionReply::Finished(
        providers::ExecutionReport::new(
            Some(Err(AgentError::Unsupported("worker failed".into()))),
            None,
            providers::ProviderSessionState::Usable,
        ),
    ));
    *provider.execution_observation_failure.lock().unwrap() =
        Some(providers::ObservationFailure::new(
            AgentError::Unsupported("worker failed".into()),
            providers::ObservationFailureCause::ExecutionFailed,
        ));
    let release_failing = hold_turn(&service, &provider, &worker_id, "worker-fails").await;
    let dropped = client
        .call(
            "send_message",
            json!({"threadId":worker,"message":"after the failure","requestId":"report-dropped"}),
        )
        .await;
    assert_eq!(dropped["isError"], false, "{dropped}");
    let dropped = dropped["structuredContent"]["executionId"]
        .as_str()
        .unwrap()
        .to_owned();
    drop(release_failing);
    let reports = reports_in(&service, &source, 3).await;
    assert_eq!(reports.len(), 3, "{reports:?}");
    assert!(reports[2].contains(&dropped), "{}", reports[2]);
    assert!(reports[2].contains(": cancelled."), "{}", reports[2]);
    assert!(
        reports[2].contains("It gave no reply text."),
        "{}",
        reports[2]
    );

    // A queued turn withdrawn while the turn ahead still runs reports as
    // cancelled, though no invocation ends.
    let release_ahead = hold_turn(&service, &provider, &worker_id, "worker-ahead").await;
    let withdrawn = client
        .call(
            "send_message",
            json!({"threadId":worker,"message":"withdraw me","requestId":"report-withdrawn"}),
        )
        .await;
    assert_eq!(withdrawn["isError"], false, "{withdrawn}");
    let withdrawn = withdrawn["structuredContent"]["executionId"]
        .as_str()
        .unwrap()
        .to_owned();
    let stopped = client
        .call(
            "stop_thread",
            json!({"threadId":worker,"executionId":withdrawn,"requestId":"report-withdraw"}),
        )
        .await;
    assert_eq!(stopped["isError"], false, "{stopped}");
    let reports = reports_in(&service, &source, 4).await;
    assert_eq!(reports.len(), 4, "{reports:?}");
    assert!(reports[3].contains(&withdrawn), "{}", reports[3]);
    assert!(reports[3].contains(": cancelled."), "{}", reports[3]);
    drop(release_ahead);

    // The same request id sent to another thread is refused before anything
    // is sent or waits, so two threads never share one report.
    let other = client
        .call("create_thread", json!({"requestId":"report-create-other"}))
        .await;
    assert_eq!(other["isError"], false, "{other}");
    let other = other["structuredContent"]["threadId"]
        .as_str()
        .unwrap()
        .to_owned();
    let elsewhere = client
        .call(
            "send_message",
            json!({"threadId":other,"message":"work","requestId":"report-send"}),
        )
        .await;
    assert_eq!(elsewhere["structuredContent"]["error"], "request_conflict");
    assert_eq!(reports_in(&service, &source, 4).await.len(), 4);

    // A thread cannot report to itself.
    assert_eq!(
        service
            .report_thread_turn(
                source.clone(),
                "own".into(),
                ThreadReport {
                    parent: source.clone(),
                    caller: caller("own"),
                    execution: "own-report".into(),
                },
            )
            .await,
        ReportRegistration::Refused
    );
    drop(client);
    drop(grant);
    serving.await.unwrap();
    drop(release);
    retire(service, storage, metadata).await;
}

/// One subscribe call naming the sent turn waits for it and returns its
/// reply, and the turn's report is then not sent; a turn that already ended
/// answers at once. Read as events, pages of message text are read past, never
/// answered as an empty page, and the ended turn carries its reply.
#[cfg(unix)]
#[tokio::test]
async fn one_subscribe_naming_the_sent_turn_returns_its_reply_in_place_of_the_report() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    provider.turn_cancel.store(true, Ordering::SeqCst);
    let (service, storage, metadata) = fixture(&root, provider.clone());
    service.bind_commands(storage.clone());
    service.bind_agent_tool_audit(Arc::new(Audit::default()));
    let source = id();
    opened(&service, &provider, &source).await;
    let release = hold_turn(&service, &provider, &source, "source-turn").await;
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
    let created = client
        .call("create_thread", json!({"requestId":"await-create"}))
        .await;
    assert_eq!(created["isError"], false, "{created}");
    let worker = created["structuredContent"]["threadId"]
        .as_str()
        .unwrap()
        .to_owned();
    // Many more streamed chunks than one read takes, none of them an event.
    let mut updates: Vec<ExecutionUpdate> = (0..60)
        .map(|index| ExecutionUpdate::Message(MessageChunk::text(format!("chunk {index} "))))
        .collect();
    updates.push(ExecutionUpdate::Tool(ToolCallUpdate::new(
        ToolCallId::new("lookup").unwrap(),
        None,
        None,
        Some(ToolStatus::Completed),
        None,
        None,
    )));
    updates.push(ExecutionUpdate::Message(MessageChunk::text("Hello back.")));
    provider.execution_updates.lock().unwrap().extend(updates);
    let (finish, gate) = oneshot::channel();
    *provider.execution_gate.lock().unwrap() = Some(gate);
    let sent = client
        .call(
            "send_message",
            json!({"threadId":worker,"message":"say hi","requestId":"await-send"}),
        )
        .await;
    assert_eq!(sent["isError"], false, "{sent}");
    let execution = sent["structuredContent"]["executionId"]
        .as_str()
        .unwrap()
        .to_owned();
    // The provider's own text follows the last chunk; the chunks before
    // the tool step are not the answer.
    let said = |turn: &Value| {
        turn["reply"]
            .as_str()
            .is_some_and(|reply| reply.starts_with("Hello back.") && !reply.contains("chunk"))
    };
    // The call starts while the turn is held, and returns once it ends.
    let (waited, ()) = tokio::join!(
        client.call(
            "subscribe",
            json!({"threadId":worker,"executionId":execution}),
        ),
        async {
            tokio::time::sleep(Duration::from_millis(300)).await;
            let _ = finish.send(());
        }
    );
    assert_eq!(waited["isError"], false, "{waited}");
    let turn = &waited["structuredContent"]["turn"];
    assert_eq!(turn["executionId"], execution.as_str(), "{waited}");
    assert_eq!(turn["finished"], true, "{waited}");
    assert_eq!(turn["outcome"], "completed", "{waited}");
    assert!(said(turn), "{waited}");
    assert_eq!(waited["structuredContent"]["timedOut"], false, "{waited}");
    // The reply reached the sender in its turn: no report follows.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(reports_in(&service, &source, 0).await.is_empty());

    // Asked again after it ended, the same call answers at once.
    let started = std::time::Instant::now();
    let again = client
        .call(
            "subscribe",
            json!({"threadId":worker,"executionId":execution}),
        )
        .await;
    assert!(said(&again["structuredContent"]["turn"]), "{again}");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );

    // Read as events from the start: no empty page says more is waiting, and
    // the ended turn comes with its reply.
    let mut after: Option<String> = None;
    let mut ended = None;
    for _ in 0..20 {
        let args = match &after {
            None => json!({"threadId":worker,"replay":true}),
            Some(cursor) => json!({"threadId":worker,"after":cursor}),
        };
        let batch = client.call("subscribe", args).await;
        assert_eq!(batch["isError"], false, "{batch}");
        let batch = &batch["structuredContent"];
        let events = batch["events"].as_array().unwrap();
        assert!(
            !(events.is_empty() && batch["more"] == true),
            "an empty page while more waits: {batch}"
        );
        if events.iter().any(|event| event["kind"] == "turn.ended") {
            ended = Some(batch["turn"].clone());
        }
        after = Some(batch["cursor"].as_str().unwrap().into());
        if batch["more"] == false {
            break;
        }
    }
    let ended = ended.expect("the turn's end was read");
    assert!(said(&ended), "{ended}");
    assert_eq!(ended["outcome"], "completed", "{ended}");

    // A call that gives up before the turn ends leaves the report to come.
    let (finish, gate) = oneshot::channel();
    *provider.execution_gate.lock().unwrap() = Some(gate);
    provider
        .execution_updates
        .lock()
        .unwrap()
        .push(ExecutionUpdate::Message(MessageChunk::text("Later.")));
    let later = client
        .call(
            "send_message",
            json!({"threadId":worker,"message":"later","requestId":"await-later"}),
        )
        .await;
    let later_execution = later["structuredContent"]["executionId"]
        .as_str()
        .unwrap()
        .to_owned();
    let short = client
        .call(
            "subscribe",
            json!({"threadId":worker,"executionId":later_execution,"waitMs":200}),
        )
        .await;
    assert_eq!(short["structuredContent"]["timedOut"], true, "{short}");
    assert_eq!(
        short["structuredContent"]["turn"]["finished"], false,
        "{short}"
    );
    let _ = finish.send(());
    let reports = reports_in(&service, &source, 1).await;
    assert_eq!(reports.len(), 1, "{reports:?}");
    assert!(reports[0].contains("Its reply:\nLater."), "{}", reports[0]);
    // Named after its report was already queued, the call still answers;
    // the report stays, so the reply comes twice, never not at all.
    let late = client
        .call(
            "subscribe",
            json!({"threadId":worker,"executionId":later_execution}),
        )
        .await;
    assert_eq!(
        late["structuredContent"]["turn"]["finished"], true,
        "{late}"
    );
    assert_eq!(reports_in(&service, &source, 1).await.len(), 1);

    // Reading the thread's events past a turn's end leaves its report to
    // come: only a call naming the turn takes the reply in its place.
    provider
        .execution_updates
        .lock()
        .unwrap()
        .push(ExecutionUpdate::Message(MessageChunk::text("Streamed.")));
    let streamed = client
        .call(
            "send_message",
            json!({"threadId":worker,"message":"streamed","requestId":"await-streamed"}),
        )
        .await;
    let streamed_execution = streamed["structuredContent"]["executionId"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut after: Option<String> = None;
    let mut seen_end = false;
    for _ in 0..40 {
        let args = match &after {
            None => json!({"threadId":worker,"replay":true,"waitMs":2000}),
            Some(cursor) => json!({"threadId":worker,"after":cursor,"waitMs":2000}),
        };
        let batch = client.call("subscribe", args).await;
        let batch = &batch["structuredContent"];
        if batch["turn"]["executionId"] == streamed_execution.as_str() {
            assert!(
                batch["turn"]["reply"]
                    .as_str()
                    .unwrap()
                    .starts_with("Streamed."),
                "{batch}"
            );
            seen_end = true;
            break;
        }
        after = Some(batch["cursor"].as_str().unwrap().into());
    }
    assert!(seen_end, "the streamed turn's end was read");
    let reports = reports_in(&service, &source, 2).await;
    assert!(
        reports[1].contains("Its reply:\nStreamed."),
        "{}",
        reports[1]
    );

    // executionId and replay do not go together.
    let both = client
        .call(
            "subscribe",
            json!({"threadId":worker,"executionId":execution,"replay":true}),
        )
        .await;
    assert_eq!(both["structuredContent"]["error"], "invalid_request");

    drop(client);
    drop(grant);
    serving.await.unwrap();
    drop(release);
    retire(service, storage, metadata).await;
}

/// Reports saved by one run of the gateway are delivered by the next: a turn
/// that finished before the restart reports its outcome, and one the earlier
/// run never finished reports that it was interrupted. Each leaves the store
/// once its parent has it.
#[cfg(unix)]
#[tokio::test]
async fn saved_thread_reports_are_delivered_after_a_restart() {
    use crate::conversation::application::ThreadReportStore;
    use crate::conversation::infrastructure::LocalThreadReports;
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let private = directory.path().join("reports");
    nessa_local_storage::create_directory(&private).unwrap();
    let saved = private.join("thread-reports.sqlite3");
    let source = id();
    let worker = id();
    let report = |execution: &str| ThreadReport {
        parent: source.clone(),
        caller: caller(&format!("{execution}-action")),
        execution: format!("{execution}-report"),
    };
    {
        let provider = Arc::new(ProviderFactory::default());
        let (service, storage, metadata) = fixture(&root, provider.clone());
        service.bind_commands(storage.clone());
        service.bind_thread_reports(Arc::new(LocalThreadReports::open(&saved).unwrap()));
        opened(&service, &provider, &source).await;
        opened(&service, &provider, &worker).await;
        finish_turn(&service, &provider, &worker, "done").await;
        // Registered with no look after it, as a run that stopped before
        // it could deliver leaves them.
        for execution in ["done", "never"] {
            assert_eq!(
                service
                    .report_thread_turn(worker.clone(), execution.into(), report(execution))
                    .await,
                ReportRegistration::Registered
            );
        }
        retire(service, storage, metadata).await;
    }
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(&root, provider.clone());
    service.bind_commands(storage.clone());
    let store = Arc::new(LocalThreadReports::open(&saved).unwrap());
    service.bind_thread_reports(store.clone());
    // A retry of the send before the start reads its report keeps it the
    // earlier run's.
    assert_eq!(
        service
            .report_thread_turn(worker.clone(), "never".into(), report("never"))
            .await,
        ReportRegistration::Registered
    );
    assert_eq!(service.resume_thread_reports().await.unwrap(), 2);
    let reports = reports_in(&service, &source, 2).await;
    assert_eq!(reports.len(), 2, "{reports:?}");
    let done = reports
        .iter()
        .find(|text| text.contains("execution done)"))
        .unwrap();
    assert!(done.contains(": completed."), "{done}");
    let never = reports
        .iter()
        .find(|text| text.contains("execution never)"))
        .unwrap();
    assert!(never.contains(": interrupted."), "{never}");
    assert!(
        never.contains("The gateway restarted before the turn finished."),
        "{never}"
    );
    tokio::time::timeout(TURN_HOLD_HANG, async {
        while !store.pending().await.unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("delivered reports leave the store");
    retire(service, storage, metadata).await;
}

/// A report is whole within the parent's input limit, however long the reply.
#[test]
fn a_long_reply_is_cut_so_the_whole_report_fits_the_input_limit() {
    let limit = ConversationLimits::default().max_input_bytes;
    let text = crate::conversation::application::service::report_text_for_test(
        &id(),
        "turn",
        "completed",
        "é".repeat(limit),
        limit,
    );
    assert!(text.len() <= limit, "{} > {limit}", text.len());
    assert!(text.ends_with("\n[cut to fit]"));
    // A limit smaller than the report's own heading still holds.
    for limit in [0, 5, 40] {
        let text = crate::conversation::application::service::report_text_for_test(
            &id(),
            "turn",
            "completed",
            "reply".into(),
            limit,
        );
        assert!(text.len() <= limit, "{} > {limit}", text.len());
    }
}

/// The report's first line is a contract with the desktop, which shows a
/// report as one line naming the chat and how its turn ended
/// (`src/desktop/workspace/model/thread-report.ts`). Both read the same cases.
#[test]
fn a_report_heading_is_the_one_the_desktop_reads() {
    let cases: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/thread-report.json"))
            .expect("the shared cases parse");
    for case in cases["cases"].as_array().expect("cases") {
        let child = ConversationId::new(case["child"].as_str().unwrap()).unwrap();
        let reply = case["reply"].as_str().map(str::to_owned);
        let text = crate::conversation::application::service::report_text_for_test(
            &child,
            case["execution"].as_str().unwrap(),
            case["outcome"].as_str().unwrap(),
            reply.unwrap_or_default(),
            ConversationLimits::default().max_input_bytes,
        );
        assert_eq!(text.lines().next(), case["header"].as_str());
    }
}

/// Holds a save that wrote its row, so a test can act between a send's save
/// and its registration. A save that found the row already there is not held.
#[cfg(unix)]
struct HeldSave {
    store: Arc<crate::conversation::infrastructure::LocalThreadReports>,
    saved: tokio::sync::Notify,
    release: tokio::sync::Semaphore,
}
#[cfg(unix)]
impl crate::conversation::application::ThreadReportStore for HeldSave {
    fn save(
        &self,
        report: &crate::conversation::application::PendingReport,
    ) -> crate::conversation::application::ConversationFuture<'_, bool> {
        let report = report.clone();
        Box::pin(async move {
            let wrote = self.store.save(&report).await?;
            self.saved.notify_one();
            if wrote {
                let _ = self.release.acquire().await.unwrap();
            }
            Ok(wrote)
        })
    }
    fn settle(
        &self,
        child: &ConversationId,
        report: &str,
    ) -> crate::conversation::application::ConversationFuture<'_, ()> {
        self.store.settle(child, report)
    }
    fn forget_conversation(
        &self,
        id: &ConversationId,
    ) -> crate::conversation::application::ConversationFuture<'_, ()> {
        self.store.forget_conversation(id)
    }
    fn pending(
        &self,
    ) -> crate::conversation::application::ConversationFuture<
        '_,
        Vec<crate::conversation::application::PendingReport>,
    > {
        self.store.pending()
    }
}

/// A start that reads the store between a send's save and its registration
/// leaves that report to the send: it is not reported interrupted.
#[cfg(unix)]
#[tokio::test]
async fn a_start_reading_a_report_this_run_saved_leaves_it_to_its_send() {
    use crate::conversation::application::ThreadReportStore;
    use crate::conversation::infrastructure::LocalThreadReports;
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let private = directory.path().join("reports");
    nessa_local_storage::create_directory(&private).unwrap();
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(&root, provider.clone());
    service.bind_commands(storage.clone());
    let store = Arc::new(HeldSave {
        store: Arc::new(LocalThreadReports::open(&private.join("thread-reports.sqlite3")).unwrap()),
        saved: tokio::sync::Notify::new(),
        release: tokio::sync::Semaphore::new(0),
    });
    service.bind_thread_reports(store.clone());
    let source = id();
    let worker = id();
    opened(&service, &provider, &source).await;
    opened(&service, &provider, &worker).await;
    let saved = store.saved.notified();
    let registering = {
        let service = service.clone();
        let (worker, source) = (worker.clone(), source.clone());
        tokio::spawn(async move {
            service
                .report_thread_turn(
                    worker,
                    "later".into(),
                    ThreadReport {
                        parent: source,
                        caller: caller("later-action"),
                        execution: "later-report".into(),
                    },
                )
                .await
        })
    };
    saved.await;
    assert_eq!(service.resume_thread_reports().await.unwrap(), 1);
    store.release.add_permits(1);
    assert_eq!(registering.await.unwrap(), ReportRegistration::Registered);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(service.inner.thread_reports.waiting(), 1);
    assert_eq!(
        store.pending().await.unwrap().len(),
        1,
        "the report still waits for its turn"
    );
    retire(service, storage, metadata).await;
}

/// Two registrations of the same report at once: the second waits for the
/// first, and the report is this run's, not an earlier run's restored one.
#[cfg(unix)]
#[tokio::test]
async fn a_second_registration_of_a_report_at_once_does_not_mark_it_restored() {
    use crate::conversation::infrastructure::LocalThreadReports;
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let private = directory.path().join("reports");
    nessa_local_storage::create_directory(&private).unwrap();
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(&root, provider.clone());
    service.bind_commands(storage.clone());
    let store = Arc::new(HeldSave {
        store: Arc::new(LocalThreadReports::open(&private.join("thread-reports.sqlite3")).unwrap()),
        saved: tokio::sync::Notify::new(),
        release: tokio::sync::Semaphore::new(0),
    });
    service.bind_thread_reports(store.clone());
    let source = id();
    let worker = id();
    opened(&service, &provider, &source).await;
    opened(&service, &provider, &worker).await;
    let register = || {
        let service = service.clone();
        let (worker, source) = (worker.clone(), source.clone());
        tokio::spawn(async move {
            service
                .report_thread_turn(
                    worker,
                    "later".into(),
                    ThreadReport {
                        parent: source,
                        caller: caller("later-action"),
                        execution: "later-report".into(),
                    },
                )
                .await
        })
    };
    let saved = store.saved.notified();
    let first = register();
    saved.await;
    let second = register();
    tokio::time::sleep(Duration::from_millis(100)).await;
    store.release.add_permits(1);
    assert_eq!(first.await.unwrap(), ReportRegistration::Registered);
    assert_eq!(second.await.unwrap(), ReportRegistration::AlreadyWaiting);
    service.wake_thread_reports(&worker);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        service.inner.thread_reports.waiting(),
        1,
        "a turn not yet sent is not reported interrupted"
    );
    retire(service, storage, metadata).await;
}

#[cfg(unix)]
#[path = "thread_report_settlement.rs"]
mod thread_report_settlement_tests;
