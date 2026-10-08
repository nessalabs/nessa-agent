//! Claude tool observations retain supported content while bounding opaque variants.
use super::support::*;
use crate::domain::agent_execution::sessions::SessionId;
use crate::domain::agent_execution::tools::{McpCallArguments, ToolCallId, ToolContent};
use crate::infrastructure::acp::sessions::{
    ForwardedResults, McpServerList, StandInGrant, StandInGrants, StandInSessions, StdioMcpServer,
};
use crate::infrastructure::acp::tools::wire::UNSUPPORTED_TOOL_CONTENT;

#[tokio::test]
async fn unsupported_tool_content_is_visible_and_does_not_end_the_session() {
    let _process_slot = process_test_slot().await;
    let (root, binding) = test_acp_binding("unsupported-tool-content", 32);
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();

    for input in ["first", "second"] {
        let running = start(&opened, input).await;
        let mut contents = Vec::new();
        let mut continued = false;
        loop {
            match next(&mut opened).await {
                ExecutionUpdate::Tool(update) => {
                    if let Some(content) = update.content() {
                        contents.extend(content.iter().cloned());
                    }
                }
                ExecutionUpdate::Message(chunk) => {
                    assert_eq!(chunk, MessageChunk::text(format!("continued:{input}")));
                    continued = true;
                }
                ExecutionUpdate::Finished(ExecutionOutcome::Completed) => break,
                update => panic!("unexpected update: {update:?}"),
            }
        }
        assert_eq!(running.await.unwrap(), Ok(ExecutionOutcome::Completed));
        assert!(continued);
        assert_eq!(contents.first(), Some(&ToolContent::text("before")));
        assert_eq!(contents.last(), Some(&ToolContent::text("after")));
        assert_eq!(
            contents
                .iter()
                .filter(|content| **content == ToolContent::text(UNSUPPORTED_TOOL_CONTENT))
                .count(),
            15
        );
        assert_eq!(contents.len(), 17);
        assert!(contents.iter().all(|content| {
            content != &ToolContent::text("i".repeat(3000))
                && content != &ToolContent::text("z".repeat(180))
        }));
    }

    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    assert_gone(&root, "pid");
}

/// Grants holding `forwarded` as their stand-ins' forwarded results.
struct ForwardingGrants(ForwardedResults);
impl StandInGrants for ForwardingGrants {
    fn grant(&self, _: &SessionId) -> StandInGrant {
        StandInGrant::new(Vec::new(), Box::new(())).with_forwarded(self.0.clone())
    }
}

/// The frames Claude ACP 0.76.0 sent for MCP calls, recorded live.
const RECORDING: &str = include_str!("../../claude_acp/tools/fixtures/mcp_live_frames.json");

/// #435, through the worker, on Claude's recorded frames: the structured
/// result a stand-in forwarded under a call's id is appended to that call's
/// completed update (W1) — not to the PostToolUse frame, which has no content
/// (W2) — and taken; an `isError` call, reported `failed`, takes nothing (W7).
#[tokio::test]
async fn a_forwarded_structured_result_reaches_the_completed_update_of_its_call() {
    let recording: serde_json::Value = serde_json::from_str(RECORDING).unwrap();
    let last = |tool: &str| {
        recording["calls"][tool]
            .as_array()
            .unwrap()
            .last()
            .unwrap()
            .clone()
    };
    let (rows, fails) = (
        last("mcp__mcptest__report_rows"),
        last("mcp__mcptest__always_fails"),
    );
    let id =
        |frame: &serde_json::Value| ToolCallId::new(frame["toolCallId"].as_str().unwrap()).unwrap();
    let _process_slot = process_test_slot().await;
    let (_root, mut config, model) = test_acp_configuration("forwarded-result", 32);
    config.mcp_servers = McpServerList::fixed(vec![StdioMcpServer {
        name: "mcptest".into(),
        command: "/bin/stand-in".into(),
        args: vec!["mcp-relay".into(), "mcptest".into()],
    }]);
    let structured = ToolContent::structured(rows["rawOutput"].as_str().unwrap()).unwrap();
    let refusal = ToolContent::structured(r#"{"reason":"on purpose"}"#).unwrap();
    let arguments = McpCallArguments::new(r#"{"city":"Oslo"}"#).unwrap();
    let left = McpCallArguments::new(r#"{"city":"Left"}"#).unwrap();
    let left_id = ToolCallId::new("toolu_left").unwrap();
    let forwarded = ForwardedResults::new();
    forwarded.record(id(&rows), "mcptest", structured.clone());
    forwarded.record(id(&fails), "mcptest", refusal.clone());
    forwarded.record_arguments(id(&rows), "mcptest", arguments.clone());
    forwarded.record_arguments(id(&fails), "mcptest", arguments.clone());
    // No update of this turn names it. The worker drops it when the turn
    // ends, so the next turn cannot be told it.
    forwarded.record_arguments(left_id.clone(), "mcptest", left.clone());
    assert_eq!(forwarded.arguments_len(), 3);
    config.stand_ins = StandInSessions::granted_by(Arc::new(ForwardingGrants(forwarded.clone())));
    let binding = ClaudeAcpProvider::new(
        config,
        &model,
        TokenLimits::new(900, 100).unwrap(),
        Arc::new(RecordingAudit::default()),
    )
    .unwrap();
    let (_, _, control) = ProviderOpenRequest::without_startup_control(None).into_parts();
    let request = ProviderOpenRequest::new(SessionId::new("conversation").unwrap(), None, control);
    let mut opened = binding.open(request).await.unwrap();
    let running = start(&opened, "call").await;
    let mut updates = Vec::new();
    loop {
        match next(&mut opened).await {
            ExecutionUpdate::Tool(update) => updates.push(update),
            ExecutionUpdate::Message(_) => {}
            ExecutionUpdate::Finished(ExecutionOutcome::Completed) => break,
            update => panic!("unexpected update: {update:?}"),
        }
    }
    assert_eq!(running.await.unwrap(), Ok(ExecutionOutcome::Completed));
    let of = |frame: &serde_json::Value| -> Vec<_> {
        updates
            .iter()
            .filter(|update| *update.id() == id(frame))
            .map(|update| update.content().clone())
            .collect()
    };
    let said = |frame: &serde_json::Value| {
        ToolContent::text(frame["content"][0]["content"]["text"].as_str().unwrap())
    };
    // Every update before the completed one is as the harness sent it: the
    // PostToolUse frame among them, without content, takes nothing.
    let rows_updates = of(&rows);
    let (completed, before) = rows_updates.split_last().unwrap();
    assert!(before.contains(&None));
    assert!(before
        .iter()
        .flatten()
        .all(|content| !content.contains(&structured)));
    assert_eq!(completed, &Some(vec![said(&rows), structured]));
    assert_eq!(of(&fails).last().unwrap(), &Some(vec![said(&fails)]));
    let carried = |frame: &serde_json::Value| {
        updates
            .iter()
            .any(|update| *update.id() == id(frame) && update.mcp_arguments() == Some(&arguments))
    };
    // The worker attached the arguments the stand-in kept, including on the
    // failed call: they were the request. Taken once.
    assert!(carried(&rows));
    assert!(carried(&fails));
    assert!(updates
        .iter()
        .all(|update| update.mcp_arguments() != Some(&left)));
    assert_eq!(forwarded.take_arguments(&id(&rows), "mcptest"), None);
    assert_eq!(forwarded.take_arguments(&id(&fails), "mcptest"), None);
    // The reply is sent after the turn drops what no update took.
    assert_eq!(forwarded.take_arguments(&left_id, "mcptest"), None);
    assert_eq!(forwarded.arguments_len(), 0);
    // Taken once; the failed call's result is left to be dropped.
    assert_eq!(forwarded.take(&id(&rows), "mcptest"), None);
    assert_eq!(forwarded.take(&id(&fails), "mcptest"), Some(refusal));
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
}

/// A16. `mode`'s first turn ends by leaving the worker; the grant stays.
/// Arguments no update took are gone before the next generation runs, and
/// the result kept beside them is not.
async fn arguments_left_by_a_worker_that_exits_do_not_reach_the_next_generation(
    mode: &str,
    first: Result<ExecutionOutcome, AgentError>,
) {
    let _process_slot = process_test_slot().await;
    let (root, mut config, model) = test_acp_configuration(mode, 16);
    let forwarded = ForwardedResults::new();
    let left_id = ToolCallId::new("toolu_left").unwrap();
    let kept = ToolContent::structured(r#"{"kept":true}"#).unwrap();
    forwarded.record(left_id.clone(), "mcptest", kept.clone());
    forwarded.record_arguments(
        left_id.clone(),
        "mcptest",
        McpCallArguments::new(r#"{"city":"Left"}"#).unwrap(),
    );
    assert_eq!(forwarded.arguments_len(), 1);
    config.stand_ins = StandInSessions::granted_by(Arc::new(ForwardingGrants(forwarded.clone())));
    let binding = ClaudeAcpProvider::new(
        config,
        &model,
        TokenLimits::new(900, 100).unwrap(),
        Arc::new(RecordingAudit::default()),
    )
    .unwrap();
    let (_, _, control) = ProviderOpenRequest::without_startup_control(None).into_parts();
    let request = ProviderOpenRequest::new(SessionId::new("conversation").unwrap(), None, control);
    let opened = binding.open(request).await.unwrap();
    assert_eq!(start(&opened, "first").await.await.unwrap(), first);
    assert_eq!(forwarded.take_arguments(&left_id, "mcptest"), None);
    assert_eq!(forwarded.arguments_len(), 0);
    assert_eq!(forwarded.take(&left_id, "mcptest"), Some(kept));
    // The failed turn's evidence stays on the stream. The next turn is
    // admitted on the same grant, which is what would have inherited it.
    let running = start(&opened, "second").await;
    assert_eq!(
        timeout(Duration::from_secs(5), running)
            .await
            .expect("second turn")
            .unwrap(),
        Ok(ExecutionOutcome::Completed)
    );
    let launches: Vec<i32> =
        serde_json::from_str(&std::fs::read_to_string(root.path().join("launches")).unwrap())
            .unwrap();
    assert_eq!(launches.len(), 2);
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn a_failed_prompt_drops_arguments_before_the_next_generation() {
    arguments_left_by_a_worker_that_exits_do_not_reach_the_next_generation(
        "provider-error-once",
        Err(AgentError::Provider {
            code: -32001,
            diagnostic: Some(ProviderDiagnostic::new(
                "provider plan does not allow this request",
            )),
        }),
    )
    .await;
}

#[tokio::test]
async fn an_unsolicited_cancellation_drops_arguments_before_the_next_generation() {
    arguments_left_by_a_worker_that_exits_do_not_reach_the_next_generation(
        "cancelled-once",
        Ok(ExecutionOutcome::Cancelled),
    )
    .await;
}
