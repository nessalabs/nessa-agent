//! Claude tool observations retain supported content while bounding opaque variants.
use super::support::*;
use crate::domain::agent_execution::sessions::SessionId;
use crate::domain::agent_execution::tools::ToolContent;
use crate::infrastructure::acp::sessions::{
    StandInGrant, StandInGrants, StandInSessions, StdioMcpServer,
};
use crate::infrastructure::acp::tools::wire::UNSUPPORTED_TOOL_CONTENT;
use crate::infrastructure::mcp::ForwardedResults;

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

/// #435 W1–W4: Claude's harness reports an MCP result as JSON text only. The
/// structured result its stand-in forwarded under the call's id is appended
/// to that call's terminal update — not to the PostToolUse frame, which has
/// no content — and taken; a call with none forwarded keeps its text alone.
#[tokio::test]
async fn a_forwarded_structured_result_reaches_the_terminal_update_of_its_call() {
    let _process_slot = process_test_slot().await;
    let (_root, mut config, model) = test_acp_configuration("forwarded-result", 32);
    config.mcp_servers = vec![StdioMcpServer {
        name: "mcptest".into(),
        command: "/bin/stand-in".into(),
        args: vec!["mcp-relay".into(), "mcptest".into()],
    }];
    let forwarded = ForwardedResults::default();
    forwarded.record(
        "toolu_rows".into(),
        ToolContent::structured(r#"{"rows":[1,2]}"#).unwrap(),
    );
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
    let said = ToolContent::text(r#"{"rows":[1,2]}"#);
    let of = |id: &str| -> Vec<_> {
        updates
            .iter()
            .filter(|update| update.id().as_str() == id)
            .map(|update| update.content().clone())
            .collect()
    };
    // Pending, update, PostToolUse (no content), terminal.
    assert_eq!(
        of("toolu_rows"),
        vec![
            Some(vec![]),
            Some(vec![]),
            None,
            Some(vec![
                said.clone(),
                ToolContent::structured(r#"{"rows":[1,2]}"#).unwrap()
            ]),
        ]
    );
    assert_eq!(
        of("toolu_plain"),
        vec![Some(vec![]), Some(vec![]), None, Some(vec![said])]
    );
    // Taken: nothing is left to attach twice.
    assert_eq!(forwarded.take("toolu_rows"), None);
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
}
