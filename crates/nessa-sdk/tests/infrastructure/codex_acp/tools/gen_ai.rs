use super::{tool_call, ObservedTools};
use crate::application::agent_execution::traces::{
    FinishReason, LocalTraceStore, ToolFact, ToolProgress, TraceRecorder, TurnIdentity,
};
use crate::infrastructure::acp::executions::usage::reported_usage;
use serde_json::json;
use std::time::Duration;

#[test]
fn a_codex_command_uses_its_name_and_not_only_its_kind() {
    let mut tools = ObservedTools::default();
    let frame = json!({
        "sessionUpdate": "tool_call",
        "toolCallId": "command-1",
        "kind": "execute",
        "name": "exec_command",
        "title": "npm test",
        "status": "pending",
        "content": [{"type": "terminal", "terminalId": "command-1"}],
        "rawInput": {"command": "npm test secret-marker", "cwd": "/workspace"},
        "_meta": {"terminal_info": {"cwd": "/workspace", "terminal_id": "command-1"}},
    });
    let update = tool_call(&frame, &mut tools).unwrap();
    let name = tools.traced_name(update.id().as_str()).unwrap();
    assert_eq!(name, "exec_command");
    let arguments = tools.traced_arguments(update.id().as_str());
    assert!(arguments.as_deref().unwrap().contains("secret-marker"));
    let store = LocalTraceStore::new();
    let mut recorder = TraceRecorder::new(store.clone(), store.capture());
    recorder.begin_turn(
        Duration::from_millis(1),
        TurnIdentity::new(
            "conversation-1",
            "execution-1",
            "openai",
            "codex",
            "gpt-5",
            Some(true),
        ),
        "",
    );
    recorder.usage(reported_usage(
        &json!({"input_tokens": 8, "output_tokens": 2, "cachedReadTokens": 5}),
    ));
    recorder.tool(
        Duration::from_millis(2),
        ToolFact::new(
            update.id().as_str(),
            name,
            "function",
            ToolProgress::Completed,
            arguments,
            Some("secret-marker output".into()),
        ),
    );
    recorder.finish(Duration::from_millis(3), FinishReason::Stop);
    let spans = store.read("conversation-1");
    let tool = spans
        .iter()
        .find(|span| span.operation() == "execute_tool")
        .unwrap();
    let chat = spans
        .iter()
        .find(|span| span.operation() == "chat")
        .unwrap();
    let agent = spans
        .iter()
        .find(|span| span.operation() == "invoke_agent")
        .unwrap();
    assert_eq!(tool.string("gen_ai.tool.name"), Some("exec_command"));
    assert_ne!(tool.string("gen_ai.tool.name"), Some("execute"));
    assert_eq!(tool.string("gen_ai.provider.name"), Some("openai"));
    assert_eq!(agent.string("gen_ai.agent.name"), Some("codex"));
    assert_eq!(agent.bool("nessa.hook.native"), Some(true));
    assert_eq!(chat.int("gen_ai.usage.input_tokens"), Some(8));
    assert_eq!(chat.int("gen_ai.usage.cache_read.input_tokens"), Some(5));
    assert!(!format!("{spans:?}").contains("secret-marker"));
}
