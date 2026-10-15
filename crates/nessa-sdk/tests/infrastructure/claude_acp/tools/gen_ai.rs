use super::{retained_tool_arguments, retained_tool_name, tool_call};
use crate::application::agent_execution::providers::ApprovalMode;
use crate::application::agent_execution::traces::{
    ContentCapture, FinishReason, LocalTraceStore, ToolFact, ToolProgress, TraceRecorder,
    TurnIdentity,
};
use crate::infrastructure::acp::executions::usage::reported_usage;
use crate::infrastructure::acp::profile::AcpProfile;
use crate::infrastructure::claude_acp::sessions::profile::ClaudeProfile;
use serde_json::json;
use std::collections::HashMap;
use std::time::Duration;

#[test]
fn a_claude_read_becomes_an_execute_tool_span_without_its_content() {
    let mut names = HashMap::new();
    let pending = json!({
        "toolCallId": "call-1",
        "status": "pending",
        "kind": "read",
        "title": "Read a file",
        "_meta": {"claudeCode": {"toolName": "Read"}},
        "rawInput": {"file_path": "secret-marker.txt"},
    });
    let update = tool_call(&pending, &mut names, &[]).unwrap();
    assert_eq!(
        retained_tool_name(&names, "call-1").as_deref(),
        Some("Read")
    );
    let arguments = retained_tool_arguments(&names, "call-1");
    assert!(arguments.as_deref().unwrap().contains("secret-marker"));
    let store = LocalTraceStore::new();
    let mut recorder = TraceRecorder::new(store.clone(), store.capture());
    recorder.begin_turn(
        Duration::from_millis(1),
        TurnIdentity::new(
            "conversation-1",
            "execution-1",
            "anthropic",
            "claude",
            "claude-sonnet",
            Some(false),
        ),
        "prompt secret-marker",
    );
    recorder.usage(reported_usage(&json!({
        "used": 12,
        "size": 200,
        "usage": {"inputTokens": 3, "outputTokens": 4, "thoughtTokens": 1}
    })));
    recorder.tool(
        Duration::from_millis(2),
        ToolFact::new(
            update.id().as_str(),
            retained_tool_name(&names, update.id().as_str()).unwrap(),
            "function",
            ToolProgress::Completed,
            arguments,
            Some("body secret-marker".into()),
        ),
    );
    recorder.finish(Duration::from_millis(3), FinishReason::Stop);
    let spans = store.read("conversation-1");
    let tool = spans
        .iter()
        .find(|span| span.operation() == "execute_tool")
        .unwrap();
    let agent = spans
        .iter()
        .find(|span| span.operation() == "invoke_agent")
        .unwrap();
    assert_eq!(tool.string("gen_ai.tool.name"), Some("Read"));
    assert_eq!(tool.string("gen_ai.tool.call.id"), Some("call-1"));
    assert_eq!(tool.string("gen_ai.provider.name"), Some("anthropic"));
    assert_eq!(tool.string("gen_ai.operation.name"), Some("execute_tool"));
    assert_eq!(agent.string("gen_ai.agent.name"), Some("claude"));
    assert_eq!(agent.int("gen_ai.usage.input_tokens"), Some(3));
    assert_eq!(agent.int("gen_ai.usage.output_tokens"), Some(4));
    assert_eq!(agent.int("nessa.context.used_tokens"), Some(12));
    assert_ne!(agent.int("gen_ai.usage.input_tokens"), Some(12));
    assert!(!format!("{spans:?}").contains("secret-marker"));
}

#[test]
fn a_finished_claude_call_traces_arguments_the_review_cache_drops() {
    for status in ["completed", "failed"] {
        let mut profile = ClaudeProfile::new(None, ApprovalMode::Ask, None);
        let frame = json!({
            "toolCallId": "call-1",
            "status": status,
            "kind": "read",
            "_meta": {"claudeCode": {"toolName": "Read"}},
            "rawInput": {"file_path": "secret-marker.txt"},
        });
        let update = profile.tool_call(&frame).unwrap();
        let arguments = profile.traced_tool_arguments(update.id().as_str());
        assert!(
            arguments.as_deref().unwrap().contains("secret-marker.txt"),
            "{status}"
        );
        assert!(profile
            .permission_input(&json!({"toolCall": {"toolCallId": "call-1"}}))
            .is_err());
        let store = LocalTraceStore::with_capture(ContentCapture::on());
        let mut recorder = TraceRecorder::new(store.clone(), ContentCapture::on());
        recorder.begin_turn(
            Duration::from_millis(1),
            TurnIdentity::new(
                "conversation-1",
                "execution-1",
                "anthropic",
                "claude",
                "claude-sonnet",
                Some(false),
            ),
            "",
        );
        let progress = if status == "failed" {
            ToolProgress::Failed
        } else {
            ToolProgress::Completed
        };
        recorder.tool(
            Duration::from_millis(2),
            ToolFact::new(
                update.id().as_str(),
                profile.traced_tool_name(update.id().as_str()).unwrap(),
                "function",
                progress,
                arguments,
                None,
            ),
        );
        recorder.finish(Duration::from_millis(3), FinishReason::Stop);
        let tool = store
            .read("conversation-1")
            .into_iter()
            .find(|span| span.operation() == "execute_tool")
            .unwrap();
        assert!(
            tool.string("gen_ai.tool.call.arguments")
                .unwrap()
                .contains("secret-marker.txt"),
            "{status}"
        );
    }
}
