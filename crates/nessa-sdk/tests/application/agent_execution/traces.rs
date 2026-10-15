use super::*;
use crate::application::agent_execution::traces::{
    ContentCapture, ExporterList, FinishReason, ModelChannel, PermissionFact, ToolFact,
    ToolProgress, TraceRecorder, TurnIdentity,
};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn at(millis: u64) -> Duration {
    Duration::from_millis(millis)
}

fn identity() -> TurnIdentity {
    TurnIdentity::new(
        "conversation-1",
        "execution-1",
        "anthropic",
        "claude",
        "claude-sonnet",
        Some(false),
    )
}

fn turn(store: &Arc<LocalTraceStore>) -> TraceRecorder {
    let exporter: Arc<dyn SpanExporter> = store.clone();
    TraceRecorder::new(exporter, store.capture())
}

fn operations(store: &LocalTraceStore) -> Vec<String> {
    store
        .read("conversation-1")
        .into_iter()
        .map(|span| span.operation().to_owned())
        .collect()
}

#[test]
fn a_text_turn_is_an_agent_span_and_one_chat_span() {
    let store = LocalTraceStore::new();
    let mut recorder = turn(&store);
    recorder.begin_turn(at(1), identity(), "hello secret-marker");
    recorder.model_output(
        at(2),
        ModelChannel::Text,
        Some("msg-1"),
        "answer secret-marker",
    );
    recorder.model_output(at(3), ModelChannel::Thought, None, "thought secret-marker");
    recorder.finish(at(4), FinishReason::Stop);
    let spans = store.read("conversation-1");
    assert_eq!(operations(&store), ["chat", "invoke_agent"]);
    let chat = &spans[0];
    let agent = &spans[1];
    assert_eq!(chat.parent_span_id(), Some(agent.span_id()));
    assert_eq!(chat.trace_id(), agent.trace_id());
    assert_eq!(chat.name(), "chat claude-sonnet");
    assert_eq!(agent.name(), "invoke_agent claude");
    assert_eq!(chat.string("gen_ai.provider.name"), Some("anthropic"));
    assert_eq!(chat.string("gen_ai.operation.name"), Some("chat"));
    assert_eq!(
        agent.string("gen_ai.conversation.id"),
        Some("conversation-1")
    );
    assert_eq!(agent.string("nessa.execution.id"), Some("execution-1"));
    assert_eq!(agent.bool("nessa.hook.native"), Some(false));
    assert_eq!(
        chat.strings("gen_ai.response.finish_reasons"),
        Some(["stop".to_owned()].as_slice())
    );
    assert_eq!(chat.string("gen_ai.response.id"), Some("msg-1"));
    let encoded = format!("{spans:?}");
    assert!(!encoded.contains("secret-marker"));
    assert!(chat.string("gen_ai.input.messages").is_none());
    assert!(chat.string("gen_ai.output.messages").is_none());
}

#[test]
fn content_capture_keeps_text_and_still_drops_thoughts() {
    let store = LocalTraceStore::with_capture(ContentCapture::on());
    let mut recorder = turn(&store);
    recorder.begin_turn(at(1), identity(), "user secret-marker");
    recorder.model_output(at(2), ModelChannel::Thought, None, "thought secret-marker");
    recorder.model_output(at(3), ModelChannel::Text, None, "assistant secret-marker");
    recorder.tool(
        at(4),
        ToolFact::new(
            "tool-1",
            "Read",
            "function",
            ToolProgress::Completed,
            Some("{\"path\":\"arg secret-marker\"}".into()),
            Some("result secret-marker".into()),
        ),
    );
    recorder.finish(at(5), FinishReason::Stop);
    let spans = store.read("conversation-1");
    let encoded = format!("{spans:?}");
    assert!(encoded.contains("user secret-marker"));
    assert!(encoded.contains("assistant secret-marker"));
    assert!(encoded.contains("arg secret-marker"));
    assert!(encoded.contains("result secret-marker"));
    assert!(!encoded.contains("thought secret-marker"));
}

#[test]
fn a_tool_call_closes_the_chat_span_and_a_later_reply_opens_another() {
    let store = LocalTraceStore::new();
    let mut recorder = turn(&store);
    recorder.begin_turn(at(1), identity(), "");
    recorder.model_output(at(2), ModelChannel::Text, None, "calling");
    recorder.tool(
        at(3),
        ToolFact::new(
            "tool-1",
            "Read",
            "function",
            ToolProgress::Running,
            None,
            None,
        ),
    );
    recorder.model_output(at(4), ModelChannel::Text, None, "during");
    recorder.tool(
        at(5),
        ToolFact::new(
            "tool-2",
            "Grep",
            "extension",
            ToolProgress::Running,
            None,
            None,
        ),
    );
    recorder.tool(
        at(6),
        ToolFact::new(
            "tool-1",
            "Read",
            "function",
            ToolProgress::Completed,
            None,
            None,
        ),
    );
    recorder.tool(
        at(7),
        ToolFact::new(
            "tool-2",
            "Grep",
            "extension",
            ToolProgress::Failed,
            None,
            None,
        ),
    );
    recorder.model_output(at(8), ModelChannel::Text, None, "after");
    recorder.finish(at(9), FinishReason::Stop);
    assert_eq!(
        operations(&store),
        [
            "chat",
            "execute_tool",
            "execute_tool",
            "chat",
            "invoke_agent"
        ]
    );
    let spans = store.read("conversation-1");
    assert_eq!(
        spans[0].strings("gen_ai.response.finish_reasons"),
        Some(["tool_calls".to_owned()].as_slice())
    );
    assert_eq!(spans[1].string("gen_ai.tool.name"), Some("Read"));
    assert_eq!(spans[1].string("gen_ai.tool.type"), Some("function"));
    assert_eq!(spans[1].parent_span_id(), Some(spans[4].span_id()));
    assert_eq!(spans[2].string("gen_ai.tool.name"), Some("Grep"));
    assert!(matches!(
        spans[2].status(),
        SpanStatus::Error { error_type } if error_type == "tool_failed"
    ));
    assert_eq!(
        spans[3].strings("gen_ai.response.finish_reasons"),
        Some(["stop".to_owned()].as_slice())
    );
}

#[test]
fn tokens_are_usage_and_context_occupancy_is_not() {
    let store = LocalTraceStore::new();
    let mut recorder = turn(&store);
    recorder.usage(
        ReportedUsage::new()
            .with_context_used(12)
            .with_context_size(100)
            .with_input_tokens(3)
            .with_cost("0.01", "USD"),
    );
    recorder.begin_turn(at(1), identity(), "");
    recorder.usage(
        ReportedUsage::new()
            .with_output_tokens(4)
            .with_reasoning_tokens(1),
    );
    recorder.finish(at(2), FinishReason::Length);
    let spans = store.read("conversation-1");
    let chat = &spans[0];
    let agent = &spans[1];
    assert_eq!(chat.int("gen_ai.usage.input_tokens"), Some(3));
    assert_eq!(chat.int("gen_ai.usage.output_tokens"), Some(4));
    assert_eq!(chat.int("gen_ai.usage.reasoning.output_tokens"), Some(1));
    assert_eq!(agent.int("gen_ai.usage.input_tokens"), Some(3));
    assert_eq!(agent.int("gen_ai.usage.output_tokens"), Some(4));
    assert_eq!(agent.int("nessa.context.used_tokens"), Some(12));
    assert_eq!(agent.int("nessa.context.size_tokens"), Some(100));
    assert_eq!(agent.string("nessa.context.cost.amount"), Some("0.01"));
    assert_eq!(agent.string("nessa.context.cost.currency"), Some("USD"));
    assert_ne!(agent.int("gen_ai.usage.input_tokens"), Some(12));
    assert!(matches!(
        agent.status(),
        SpanStatus::Error { error_type } if error_type == "output_limit"
    ));
}

#[test]
fn usage_after_a_turn_closes_does_not_land_on_the_next_turn() {
    let store = LocalTraceStore::new();
    let mut recorder = turn(&store);
    recorder.usage(ReportedUsage::new().with_input_tokens(1));
    recorder.begin_turn(at(1), identity(), "");
    recorder.usage(ReportedUsage::new().with_input_tokens(3));
    recorder.finish(at(2), FinishReason::Stop);
    recorder.usage(
        ReportedUsage::new()
            .with_input_tokens(9)
            .with_context_used(40),
    );
    recorder.accept_usage_before_turn();
    recorder.usage(ReportedUsage::new().with_input_tokens(5));
    recorder.begin_turn(
        at(3),
        TurnIdentity::new(
            "conversation-1",
            "execution-2",
            "anthropic",
            "claude",
            "claude-sonnet",
            Some(false),
        ),
        "",
    );
    recorder.finish(at(4), FinishReason::Stop);
    let spans = store.read("conversation-1");
    let agent = |execution: &str| {
        spans
            .iter()
            .find(|span| {
                span.operation() == "invoke_agent"
                    && span.string("nessa.execution.id") == Some(execution)
            })
            .unwrap()
    };
    let first = agent("execution-1");
    let second = agent("execution-2");
    assert_eq!(first.int("gen_ai.usage.input_tokens"), Some(4));
    assert_eq!(second.int("gen_ai.usage.input_tokens"), Some(5));
    assert!(spans
        .iter()
        .all(|span| span.int("nessa.context.used_tokens") != Some(40)));
    assert!(spans
        .iter()
        .all(|span| span.int("gen_ai.usage.input_tokens") != Some(9)));
}

#[test]
fn an_unrepresentable_token_count_is_omitted() {
    let store = LocalTraceStore::new();
    let mut recorder = turn(&store);
    let too_big = u64::try_from(i64::MAX).unwrap() + 1;
    recorder.begin_turn(at(1), identity(), "");
    recorder.usage(
        ReportedUsage::new()
            .with_input_tokens(too_big)
            .with_output_tokens(4)
            .with_context_used(too_big)
            .with_context_size(100),
    );
    recorder.finish(at(2), FinishReason::Stop);
    let spans = store.read("conversation-1");
    let agent = spans
        .iter()
        .find(|span| span.operation() == "invoke_agent")
        .unwrap();
    let chat = spans
        .iter()
        .find(|span| span.operation() == "chat")
        .unwrap();
    assert_eq!(agent.int("gen_ai.usage.input_tokens"), None);
    assert_eq!(chat.int("gen_ai.usage.input_tokens"), None);
    assert_eq!(agent.int("gen_ai.usage.output_tokens"), Some(4));
    assert_eq!(chat.int("gen_ai.usage.output_tokens"), Some(4));
    assert_eq!(agent.int("nessa.context.used_tokens"), None);
    assert_eq!(agent.int("nessa.context.size_tokens"), Some(100));
    assert!(spans
        .iter()
        .all(|span| span.int("gen_ai.usage.input_tokens") != Some(i64::MAX)));
}

#[test]
fn permission_grant_hook_and_subagent_notes_land_on_the_spans() {
    let store = LocalTraceStore::new();
    store.note_grant("conversation-1", "execution-1", "grant-1", 7, true);
    store.note_hook(
        "conversation-1",
        "execution-1",
        "before_permission_review",
        "allow",
    );
    store.note_hook(
        "conversation-1",
        "execution-1",
        "before_permission_review",
        "refuse",
    );
    store.note_subagent("conversation-1", "execution-1", "child-1", "parent-1");
    let mut recorder = turn(&store);
    recorder.begin_turn(at(1), identity(), "");
    recorder.permission(PermissionFact::Requested {
        id: "review-1".into(),
        tool_id: "tool-1".into(),
        tool_name: "Read".into(),
        arguments: Some("secret-marker".into()),
        tool_type: "function".into(),
        category_name: false,
    });
    recorder.permission(PermissionFact::Answered {
        id: "review-1".into(),
        tool_id: "tool-1".into(),
        effect: "allow".into(),
        decider: "person".into(),
    });
    recorder.tool(
        at(2),
        ToolFact::new(
            "tool-1",
            "Read",
            "function",
            ToolProgress::Completed,
            None,
            None,
        ),
    );
    recorder.finish(at(3), FinishReason::Stop);
    let spans = store.read("conversation-1");
    let tool = spans
        .iter()
        .find(|span| span.operation() == "execute_tool")
        .unwrap();
    let agent = spans
        .iter()
        .find(|span| span.operation() == "invoke_agent")
        .unwrap();
    assert_eq!(tool.string("nessa.permission.id"), Some("review-1"));
    assert_eq!(tool.string("nessa.permission.state"), Some("answered"));
    assert_eq!(tool.string("nessa.permission.effect"), Some("allow"));
    assert_eq!(tool.string("nessa.permission.decider"), Some("person"));
    assert_eq!(tool.string("gen_ai.tool.name"), Some("Read"));
    assert!(!format!("{tool:?}").contains("secret-marker"));
    assert_eq!(agent.string("nessa.grant.id"), Some("grant-1"));
    assert_eq!(agent.int("nessa.grant.policy_revision"), Some(7));
    assert_eq!(agent.bool("nessa.grant.tools_checked"), Some(true));
    assert_eq!(
        agent.string("nessa.hook.event"),
        Some("before_permission_review")
    );
    assert_eq!(agent.string("nessa.hook.verdict"), Some("refuse"));
    assert_eq!(agent.int("nessa.hook.count"), Some(2));
    assert_eq!(agent.string("nessa.subagent.id"), Some("child-1"));
    assert_eq!(agent.string("nessa.subagent.parent"), Some("parent-1"));
}

#[test]
fn a_note_after_the_span_is_stored_replaces_that_span() {
    let store = LocalTraceStore::new();
    let mut recorder = turn(&store);
    recorder.begin_turn(at(1), identity(), "");
    recorder.finish(at(2), FinishReason::Stop);
    store.note_subagent("conversation-1", "execution-1", "child-2", "parent-2");
    let agent = store
        .read("conversation-1")
        .into_iter()
        .find(|span| span.operation() == "invoke_agent")
        .unwrap();
    assert_eq!(agent.string("nessa.subagent.id"), Some("child-2"));
}

#[test]
fn dropping_an_open_turn_closes_it_as_cancelled() {
    let store = LocalTraceStore::new();
    {
        let mut recorder = turn(&store);
        recorder.begin_turn(at(5), identity(), "");
    }
    let agent = store
        .read("conversation-1")
        .into_iter()
        .find(|span| span.operation() == "invoke_agent")
        .unwrap();
    assert_eq!(
        agent.strings("gen_ai.response.finish_reasons"),
        Some(["cancelled".to_owned()].as_slice())
    );
}

#[test]
fn the_extra_exporter_sees_the_finished_agent_span() {
    let store = LocalTraceStore::new();
    let seen = Arc::new(Mutex::new(Vec::new()));
    struct Remember(Arc<Mutex<Vec<GenAiSpan>>>);
    impl SpanExporter for Remember {
        fn export(&self, span: &GenAiSpan) {
            self.0.lock().unwrap().push(span.clone());
        }
    }
    store.set_exporter(Arc::new(Remember(seen.clone())));
    store.note_grant("conversation-1", "execution-1", "grant-9", 1, false);
    let mut recorder = turn(&store);
    recorder.begin_turn(at(1), identity(), "");
    recorder.finish(at(2), FinishReason::Stop);
    let exported = seen.lock().unwrap();
    let agent = exported
        .iter()
        .find(|span| span.operation() == "invoke_agent")
        .unwrap();
    assert_eq!(agent.string("nessa.grant.id"), Some("grant-9"));
    let _ = ExporterList::new(vec![store.clone()]);
}

#[test]
fn an_open_turn_is_absent_until_the_agent_span_closes() {
    let store = LocalTraceStore::new();
    let mut recorder = turn(&store);
    recorder.begin_turn(at(1), identity(), "");
    recorder.tool(
        at(2),
        ToolFact::new(
            "tool-1",
            "Read",
            "function",
            ToolProgress::Completed,
            None,
            None,
        ),
    );
    assert!(store.read("conversation-1").is_empty());
    recorder.finish(at(3), FinishReason::Stop);
    assert_eq!(operations(&store), ["chat", "execute_tool", "invoke_agent"]);
}

#[test]
fn the_same_execution_id_in_two_conversations_keeps_separate_notes() {
    let store = LocalTraceStore::new();
    store.note_grant("conversation-1", "execution-1", "grant-a", 1, true);
    store.note_grant("conversation-2", "execution-1", "grant-b", 2, false);
    let mut recorder = turn(&store);
    recorder.begin_turn(at(1), identity(), "");
    recorder.finish(at(2), FinishReason::Stop);
    recorder.begin_turn(
        at(3),
        TurnIdentity::new(
            "conversation-2",
            "execution-1",
            "openai",
            "codex",
            "gpt",
            None,
        ),
        "",
    );
    recorder.finish(at(4), FinishReason::Stop);
    let first = store
        .read("conversation-1")
        .into_iter()
        .find(|span| span.operation() == "invoke_agent")
        .unwrap();
    let second = store
        .read("conversation-2")
        .into_iter()
        .find(|span| span.operation() == "invoke_agent")
        .unwrap();
    assert_eq!(first.string("nessa.grant.id"), Some("grant-a"));
    assert_eq!(second.string("nessa.grant.id"), Some("grant-b"));
    assert_ne!(first.trace_id(), second.trace_id());
}

#[test]
fn an_answer_recorded_before_the_request_stays_answered() {
    let store = LocalTraceStore::new();
    let mut recorder = turn(&store);
    recorder.begin_turn(at(1), identity(), "");
    recorder.permission(PermissionFact::Answered {
        id: "review-1".into(),
        tool_id: "tool-1".into(),
        effect: "deny".into(),
        decider: "rule".into(),
    });
    recorder.permission(PermissionFact::Requested {
        id: "review-1".into(),
        tool_id: "tool-1".into(),
        tool_name: "Read".into(),
        arguments: None,
        tool_type: "function".into(),
        category_name: false,
    });
    recorder.finish(at(2), FinishReason::Stop);
    let tool = store
        .read("conversation-1")
        .into_iter()
        .find(|span| span.operation() == "execute_tool")
        .unwrap();
    assert_eq!(tool.string("nessa.permission.state"), Some("answered"));
    assert_eq!(tool.string("nessa.permission.effect"), Some("deny"));
    assert_eq!(tool.string("nessa.permission.decider"), Some("rule"));
    assert_eq!(tool.string("gen_ai.tool.name"), Some("Read"));
}

#[test]
fn request_arguments_land_on_a_tool_the_answer_opened() {
    let store = LocalTraceStore::with_capture(ContentCapture::on());
    let mut recorder = turn(&store);
    recorder.begin_turn(at(1), identity(), "");
    recorder.permission(PermissionFact::Answered {
        id: "review-1".into(),
        tool_id: "tool-1".into(),
        effect: "deny".into(),
        decider: "rule".into(),
    });
    recorder.permission(PermissionFact::Requested {
        id: "review-1".into(),
        tool_id: "tool-1".into(),
        tool_name: "Read".into(),
        arguments: Some("{\"path\":\"secret-marker\"}".into()),
        tool_type: "function".into(),
        category_name: false,
    });
    recorder.finish(at(2), FinishReason::Stop);
    let tool = store
        .read("conversation-1")
        .into_iter()
        .find(|span| span.operation() == "execute_tool")
        .unwrap();
    assert_eq!(tool.string("nessa.permission.state"), Some("answered"));
    assert_eq!(tool.string("gen_ai.tool.name"), Some("Read"));
    assert_eq!(
        tool.string("gen_ai.tool.call.arguments"),
        Some("{\"path\":\"secret-marker\"}")
    );
}

#[test]
fn retiring_a_note_before_the_turn_leaves_the_span_without_it() {
    let store = LocalTraceStore::new();
    store.note_grant("conversation-1", "execution-1", "grant-1", 1, true);
    store.retire("conversation-1", "execution-1");
    let mut recorder = turn(&store);
    recorder.begin_turn(at(1), identity(), "");
    recorder.finish(at(2), FinishReason::Stop);
    let agent = store
        .read("conversation-1")
        .into_iter()
        .find(|span| span.operation() == "invoke_agent")
        .unwrap();
    assert!(agent.string("nessa.grant.id").is_none());
}

#[test]
fn retiring_a_grant_leaves_a_hook_note_on_the_same_execution() {
    let store = LocalTraceStore::new();
    store.note_grant("conversation-1", "execution-1", "grant-1", 1, true);
    store.note_hook(
        "conversation-1",
        "execution-1",
        "before_permission_review",
        "allow",
    );
    store.retire("conversation-1", "execution-1");
    let mut recorder = turn(&store);
    recorder.begin_turn(at(1), identity(), "");
    recorder.finish(at(2), FinishReason::Stop);
    let agent = store
        .read("conversation-1")
        .into_iter()
        .find(|span| span.operation() == "invoke_agent")
        .unwrap();
    assert!(agent.string("nessa.grant.id").is_none());
    assert_eq!(agent.string("nessa.hook.verdict"), Some("allow"));
}

#[test]
fn pending_notes_drop_the_oldest_past_capacity() {
    let store = LocalTraceStore::new();
    for index in 0..=2048 {
        store.note_grant(
            "conversation-1",
            &format!("execution-{index}"),
            "grant",
            1,
            true,
        );
    }
    let mut recorder = turn(&store);
    recorder.begin_turn(
        at(1),
        TurnIdentity::new(
            "conversation-1",
            "execution-0",
            "anthropic",
            "claude",
            "claude-sonnet",
            None,
        ),
        "",
    );
    recorder.finish(at(2), FinishReason::Stop);
    recorder.begin_turn(
        at(3),
        TurnIdentity::new(
            "conversation-1",
            "execution-2048",
            "anthropic",
            "claude",
            "claude-sonnet",
            None,
        ),
        "",
    );
    recorder.finish(at(4), FinishReason::Stop);
    let spans = store.read("conversation-1");
    let oldest = spans
        .iter()
        .find(|span| {
            span.operation() == "invoke_agent"
                && span.string("nessa.execution.id") == Some("execution-0")
        })
        .unwrap();
    let newest = spans
        .iter()
        .find(|span| {
            span.operation() == "invoke_agent"
                && span.string("nessa.execution.id") == Some("execution-2048")
        })
        .unwrap();
    assert!(oldest.string("nessa.grant.id").is_none());
    assert_eq!(newest.string("nessa.grant.id"), Some("grant"));
}

#[test]
fn a_span_finished_while_an_exporter_runs_is_kept() {
    use std::sync::mpsc;
    use std::thread;

    let store = LocalTraceStore::new();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    struct Hold {
        entered: Mutex<Option<mpsc::Sender<()>>>,
        release: Mutex<mpsc::Receiver<()>>,
    }
    impl SpanExporter for Hold {
        fn export(&self, _span: &GenAiSpan) {
            let entered = self.entered.lock().unwrap().take();
            if let Some(entered) = entered {
                let _ = entered.send(());
                let _ = self.release.lock().unwrap().recv();
            }
        }
    }
    store.set_exporter(Arc::new(Hold {
        entered: Mutex::new(Some(entered_tx)),
        release: Mutex::new(release_rx),
    }));
    let exporter = store.clone();
    let handle = thread::spawn(move || {
        let mut recorder = turn(&exporter);
        recorder.begin_turn(at(1), identity(), "");
        recorder.finish(at(2), FinishReason::Stop);
    });
    entered_rx.recv().unwrap();
    let mut other = turn(&store);
    other.begin_turn(
        at(3),
        TurnIdentity::new(
            "conversation-2",
            "execution-2",
            "openai",
            "codex",
            "gpt",
            None,
        ),
        "",
    );
    other.finish(at(4), FinishReason::Stop);
    release_tx.send(()).unwrap();
    handle.join().unwrap();
    assert!(store
        .read("conversation-2")
        .iter()
        .any(|span| span.operation() == "invoke_agent"));
    assert!(store
        .read("conversation-1")
        .iter()
        .any(|span| span.operation() == "invoke_agent"));
}

#[test]
fn captured_text_stays_within_the_byte_budget() {
    let budget = 8 * 1024 * 1024;
    let store = LocalTraceStore::with_capture(ContentCapture::on());
    let mut recorder = turn(&store);
    recorder.begin_turn(at(1), identity(), "short-prompt");
    recorder.tool(
        at(2),
        ToolFact::new(
            "tool-1",
            "Read",
            "function",
            ToolProgress::Completed,
            Some("small-args".into()),
            Some("R".repeat(budget + 1)),
        ),
    );
    recorder.finish(at(3), FinishReason::Stop);
    let spans = store.read("conversation-1");
    let chat = spans
        .iter()
        .find(|span| span.operation() == "chat")
        .unwrap();
    let tool = spans
        .iter()
        .find(|span| span.operation() == "execute_tool")
        .unwrap();
    assert!(chat
        .string("gen_ai.input.messages")
        .is_some_and(|text| text.contains("short-prompt")));
    assert_eq!(
        tool.string("gen_ai.tool.call.arguments"),
        Some("small-args")
    );
    assert!(tool.string("gen_ai.tool.call.result").is_none());

    let mut older = turn(&store);
    older.begin_turn(
        at(4),
        TurnIdentity::new(
            "conversation-1",
            "execution-old",
            "anthropic",
            "claude",
            "claude-sonnet",
            None,
        ),
        &format!("old-marker{}", "o".repeat(budget / 2)),
    );
    older.finish(at(5), FinishReason::Stop);
    let mut newer = turn(&store);
    newer.begin_turn(
        at(6),
        TurnIdentity::new(
            "conversation-1",
            "execution-new",
            "anthropic",
            "claude",
            "claude-sonnet",
            None,
        ),
        &format!("new-marker{}", "n".repeat(budget / 2)),
    );
    newer.finish(at(7), FinishReason::Stop);
    let spans = store.read("conversation-1");
    assert!(spans.iter().all(|span| {
        span.string("gen_ai.input.messages")
            .is_none_or(|text| !text.contains("old-marker"))
    }));
    assert!(spans.iter().any(|span| {
        span.string("gen_ai.input.messages")
            .is_some_and(|text| text.contains("new-marker"))
    }));
}

#[test]
fn a_long_tool_list_keeps_the_newest_call() {
    let store = LocalTraceStore::new();
    let mut recorder = turn(&store);
    recorder.begin_turn(at(1), identity(), "");
    for index in 0..=2048 {
        recorder.tool(
            at(2 + u64::try_from(index).unwrap()),
            ToolFact::new(
                format!("tool-{index}"),
                format!("tool-{index}"),
                "function",
                ToolProgress::Completed,
                None,
                None,
            ),
        );
    }
    recorder.finish(at(3000), FinishReason::Stop);
    let spans = store.read("conversation-1");
    assert!(spans.len() <= 2048);
    assert!(spans.iter().all(|span| {
        span.string("gen_ai.tool.call.id")
            .is_none_or(|id| id != "tool-0")
    }));
    assert!(spans
        .iter()
        .any(|span| span.string("gen_ai.tool.call.id") == Some("tool-2048")));
}

#[test]
fn a_decline_by_name_attaches_only_when_one_tool_has_that_name() {
    let store = LocalTraceStore::new();
    let mut recorder = turn(&store);
    recorder.begin_turn(at(1), identity(), "");
    recorder.tool(
        at(2),
        ToolFact::new(
            "tool-1",
            "Read",
            "function",
            ToolProgress::Running,
            None,
            None,
        ),
    );
    recorder.tool(
        at(3),
        ToolFact::new(
            "tool-2",
            "Read",
            "function",
            ToolProgress::Running,
            None,
            None,
        ),
    );
    recorder.permission(PermissionFact::Declined {
        id: "review-ambiguous".into(),
        tool_name: Some("Read".into()),
    });
    recorder.tool(
        at(4),
        ToolFact::new(
            "tool-3",
            "Grep",
            "function",
            ToolProgress::Running,
            None,
            None,
        ),
    );
    recorder.permission(PermissionFact::Declined {
        id: "review-grep".into(),
        tool_name: Some("Grep".into()),
    });
    recorder.finish(at(5), FinishReason::Stop);
    let spans = store.read("conversation-1");
    let agent = spans
        .iter()
        .find(|span| span.operation() == "invoke_agent")
        .unwrap();
    assert_eq!(
        agent.string("nessa.permission.id"),
        Some("review-ambiguous")
    );
    assert_eq!(agent.string("nessa.permission.state"), Some("declined"));
    for span in spans
        .iter()
        .filter(|span| span.string("gen_ai.tool.name") == Some("Read"))
    {
        assert!(span.string("nessa.permission.id").is_none());
    }
    let grep = spans
        .iter()
        .find(|span| span.string("gen_ai.tool.name") == Some("Grep"))
        .unwrap();
    assert_eq!(grep.string("nessa.permission.id"), Some("review-grep"));
    assert_eq!(grep.string("nessa.permission.state"), Some("declined"));
}

#[test]
fn a_kind_fallback_tool_name_is_replaced_by_the_provider_name() {
    let store = LocalTraceStore::new();
    let mut recorder = turn(&store);
    recorder.begin_turn(at(1), identity(), "");
    recorder.tool(
        at(2),
        ToolFact::new(
            "call-1",
            "execute",
            "function",
            ToolProgress::Running,
            None,
            None,
        )
        .with_category_name(true),
    );
    recorder.tool(
        at(3),
        ToolFact::new(
            "call-1",
            "exec_command",
            "function",
            ToolProgress::Completed,
            None,
            None,
        ),
    );
    recorder.tool(
        at(4),
        ToolFact::new(
            "call-2",
            "exec_command",
            "function",
            ToolProgress::Running,
            None,
            None,
        ),
    );
    recorder.tool(
        at(5),
        ToolFact::new(
            "call-2",
            "execute",
            "function",
            ToolProgress::Completed,
            None,
            None,
        )
        .with_category_name(true),
    );
    recorder.tool(
        at(6),
        ToolFact::new(
            "call-3",
            "execute",
            "function",
            ToolProgress::Running,
            None,
            None,
        )
        .with_category_name(true),
    );
    recorder.permission(PermissionFact::Answered {
        id: "review-3".into(),
        tool_id: "call-3".into(),
        effect: "allow".into(),
        decider: "person".into(),
    });
    recorder.permission(PermissionFact::Requested {
        id: "review-3".into(),
        tool_id: "call-3".into(),
        tool_name: "exec_command".into(),
        arguments: None,
        tool_type: "function".into(),
        category_name: false,
    });
    recorder.tool(
        at(7),
        ToolFact::new(
            "call-3",
            "execute",
            "function",
            ToolProgress::Completed,
            None,
            None,
        )
        .with_category_name(true),
    );
    recorder.finish(at(8), FinishReason::Stop);
    let spans = store.read("conversation-1");
    let name = |id: &str| {
        spans
            .iter()
            .find(|span| span.string("gen_ai.tool.call.id") == Some(id))
            .unwrap()
            .string("gen_ai.tool.name")
    };
    assert_eq!(name("call-1"), Some("exec_command"));
    assert_eq!(name("call-2"), Some("exec_command"));
    assert_eq!(name("call-3"), Some("exec_command"));
}

#[test]
fn an_exporter_panic_keeps_the_span_and_later_exports() {
    let store = LocalTraceStore::new();
    struct Boom {
        seen: Mutex<Vec<String>>,
    }
    impl SpanExporter for Boom {
        fn export(&self, span: &GenAiSpan) {
            self.seen.lock().unwrap().push(span.operation().to_owned());
            if span.operation() == "chat" {
                panic!("exporter failed");
            }
        }
    }
    let boom = Arc::new(Boom {
        seen: Mutex::new(Vec::new()),
    });
    store.set_exporter(boom.clone());
    let mut recorder = turn(&store);
    recorder.begin_turn(at(1), identity(), "");
    recorder.finish(at(2), FinishReason::Stop);
    assert!(store
        .read("conversation-1")
        .iter()
        .any(|span| span.operation() == "invoke_agent"));
    assert_eq!(
        boom.seen.lock().unwrap().as_slice(),
        ["chat", "invoke_agent"]
    );
}

#[test]
fn a_closed_turn_keeps_its_agent_span_when_open_turns_hold_captured_text() {
    let budget = 8 * 1024 * 1024;
    let store = LocalTraceStore::with_capture(ContentCapture::on());
    let mut closed = turn(&store);
    closed.begin_turn(
        at(1),
        TurnIdentity::new(
            "conversation-closed",
            "execution-closed",
            "anthropic",
            "claude",
            "claude-sonnet",
            None,
        ),
        "closed-prompt",
    );
    closed.finish(at(2), FinishReason::Stop);

    let mut open_a = turn(&store);
    open_a.begin_turn(
        at(3),
        TurnIdentity::new(
            "conversation-open-a",
            "execution-open-a",
            "anthropic",
            "claude",
            "claude-sonnet",
            None,
        ),
        &format!("open-a{}", "a".repeat(budget / 2)),
    );
    open_a.tool(
        at(4),
        ToolFact::new(
            "tool-a",
            "Read",
            "function",
            ToolProgress::Completed,
            None,
            None,
        ),
    );

    let mut open_b = turn(&store);
    open_b.begin_turn(
        at(5),
        TurnIdentity::new(
            "conversation-open-b",
            "execution-open-b",
            "anthropic",
            "claude",
            "claude-sonnet",
            None,
        ),
        &format!("open-b{}", "b".repeat(budget / 2)),
    );
    open_b.tool(
        at(6),
        ToolFact::new(
            "tool-b",
            "Read",
            "function",
            ToolProgress::Completed,
            None,
            None,
        ),
    );

    assert!(store
        .read("conversation-closed")
        .iter()
        .any(|span| span.operation() == "invoke_agent"));
}

#[test]
fn a_tool_name_longer_than_the_budget_is_not_retained() {
    let budget = 8 * 1024 * 1024;
    let store = LocalTraceStore::new();
    let mut recorder = turn(&store);
    recorder.begin_turn(at(1), identity(), "");
    recorder.tool(
        at(2),
        ToolFact::new(
            "tool-1",
            "n".repeat(budget + 1),
            "function",
            ToolProgress::Completed,
            None,
            None,
        ),
    );
    recorder.finish(at(3), FinishReason::Stop);
    let spans = store.read("conversation-1");
    assert!(spans.iter().any(|span| span.operation() == "invoke_agent"));
    let retained: usize = spans.iter().map(stored_text_bytes).sum();
    assert!(retained <= budget);
    assert!(spans.iter().all(|span| {
        span.string("gen_ai.tool.name")
            .is_none_or(|name| name.len() < 64)
    }));
}

#[test]
fn one_exporter_panic_does_not_skip_the_next() {
    struct Boom;
    impl SpanExporter for Boom {
        fn export(&self, _span: &GenAiSpan) {
            panic!("exporter failed");
        }
    }
    struct Keep {
        seen: Mutex<u32>,
    }
    impl SpanExporter for Keep {
        fn export(&self, _span: &GenAiSpan) {
            *self.seen.lock().unwrap() += 1;
        }
    }
    let keep = Arc::new(Keep {
        seen: Mutex::new(0),
    });
    let list = Arc::new(ExporterList::new(vec![Arc::new(Boom), keep.clone()]));
    let mut recorder = TraceRecorder::new(list, ContentCapture::off());
    recorder.begin_turn(at(1), identity(), "");
    recorder.finish(at(2), FinishReason::Stop);
    assert!(*keep.seen.lock().unwrap() >= 1);
}

#[test]
fn a_result_on_a_running_update_stays_when_completion_omits_it() {
    let store = LocalTraceStore::with_capture(ContentCapture::on());
    let mut recorder = turn(&store);
    recorder.begin_turn(at(1), identity(), "");
    recorder.tool(
        at(2),
        ToolFact::new(
            "call-1",
            "Read",
            "function",
            ToolProgress::Running,
            Some("{\"path\":\"a\"}".into()),
            Some("secret-marker".into()),
        ),
    );
    recorder.tool(
        at(3),
        ToolFact::new(
            "call-1",
            "Read",
            "function",
            ToolProgress::Completed,
            None,
            None,
        ),
    );
    recorder.finish(at(4), FinishReason::Stop);
    let tool = store
        .read("conversation-1")
        .into_iter()
        .find(|span| span.operation() == "execute_tool")
        .unwrap();
    assert_eq!(
        tool.string("gen_ai.tool.call.result"),
        Some("secret-marker")
    );
}

#[test]
fn an_empty_completion_result_replaces_the_one_seen_while_running() {
    let store = LocalTraceStore::with_capture(ContentCapture::on());
    let mut recorder = turn(&store);
    recorder.begin_turn(at(1), identity(), "");
    recorder.tool(
        at(2),
        ToolFact::new(
            "call-1",
            "Read",
            "function",
            ToolProgress::Running,
            None,
            Some("secret-marker".into()),
        ),
    );
    recorder.tool(
        at(3),
        ToolFact::new(
            "call-1",
            "Read",
            "function",
            ToolProgress::Completed,
            None,
            Some(String::new()),
        ),
    );
    recorder.finish(at(4), FinishReason::Stop);
    let tool = store
        .read("conversation-1")
        .into_iter()
        .find(|span| span.operation() == "execute_tool")
        .unwrap();
    assert_eq!(tool.string("gen_ai.tool.call.result"), Some(""));
    assert!(!format!("{tool:?}").contains("secret-marker"));
}

#[test]
fn a_completion_result_replaces_the_one_seen_while_running() {
    let store = LocalTraceStore::with_capture(ContentCapture::on());
    let mut recorder = turn(&store);
    recorder.begin_turn(at(1), identity(), "");
    recorder.tool(
        at(2),
        ToolFact::new(
            "call-1",
            "Read",
            "function",
            ToolProgress::Running,
            None,
            Some("earlier".into()),
        ),
    );
    recorder.tool(
        at(3),
        ToolFact::new(
            "call-1",
            "Read",
            "function",
            ToolProgress::Completed,
            None,
            Some("later".into()),
        ),
    );
    recorder.finish(at(4), FinishReason::Stop);
    let tool = store
        .read("conversation-1")
        .into_iter()
        .find(|span| span.operation() == "execute_tool")
        .unwrap();
    assert_eq!(tool.string("gen_ai.tool.call.result"), Some("later"));
}

#[test]
fn a_result_seen_while_running_is_kept_when_the_turn_ends() {
    let store = LocalTraceStore::with_capture(ContentCapture::on());
    let mut recorder = turn(&store);
    recorder.begin_turn(at(1), identity(), "");
    recorder.tool(
        at(2),
        ToolFact::new(
            "call-1",
            "Read",
            "function",
            ToolProgress::Running,
            None,
            Some("still-open".into()),
        ),
    );
    recorder.finish(at(3), FinishReason::Cancelled);
    let tool = store
        .read("conversation-1")
        .into_iter()
        .find(|span| span.operation() == "execute_tool")
        .unwrap();
    assert_eq!(tool.string("gen_ai.tool.call.result"), Some("still-open"));
}

#[test]
fn arguments_supplied_on_completion_are_kept_when_the_running_update_omitted_them() {
    let store = LocalTraceStore::with_capture(ContentCapture::on());
    let mut recorder = turn(&store);
    recorder.begin_turn(at(1), identity(), "");
    recorder.tool(
        at(2),
        ToolFact::new(
            "call-1",
            "Read",
            "function",
            ToolProgress::Running,
            None,
            None,
        ),
    );
    recorder.tool(
        at(3),
        ToolFact::new(
            "call-1",
            "Read",
            "function",
            ToolProgress::Completed,
            Some("{\"path\":\"secret-marker\"}".into()),
            None,
        ),
    );
    recorder.finish(at(4), FinishReason::Stop);
    let tool = tool_named(&store, "call-1");
    assert_eq!(
        tool.string("gen_ai.tool.call.arguments"),
        Some("{\"path\":\"secret-marker\"}")
    );
}

#[test]
fn arguments_seen_while_running_stay_when_completion_omits_them() {
    let store = LocalTraceStore::with_capture(ContentCapture::on());
    let mut recorder = turn(&store);
    recorder.begin_turn(at(1), identity(), "");
    recorder.tool(
        at(2),
        ToolFact::new(
            "call-1",
            "Read",
            "function",
            ToolProgress::Running,
            Some("{\"path\":\"secret-marker\"}".into()),
            None,
        ),
    );
    recorder.tool(
        at(3),
        ToolFact::new(
            "call-1",
            "Read",
            "function",
            ToolProgress::Completed,
            None,
            None,
        ),
    );
    recorder.finish(at(4), FinishReason::Stop);
    let tool = tool_named(&store, "call-1");
    assert_eq!(
        tool.string("gen_ai.tool.call.arguments"),
        Some("{\"path\":\"secret-marker\"}")
    );
}

#[test]
fn an_empty_completion_argument_replaces_the_one_seen_while_running() {
    let store = LocalTraceStore::with_capture(ContentCapture::on());
    let mut recorder = turn(&store);
    recorder.begin_turn(at(1), identity(), "");
    recorder.tool(
        at(2),
        ToolFact::new(
            "call-1",
            "Read",
            "function",
            ToolProgress::Running,
            Some("{\"path\":\"secret-marker\"}".into()),
            None,
        ),
    );
    recorder.tool(
        at(3),
        ToolFact::new(
            "call-1",
            "Read",
            "function",
            ToolProgress::Completed,
            Some(String::new()),
            None,
        ),
    );
    recorder.finish(at(4), FinishReason::Stop);
    let tool = tool_named(&store, "call-1");
    assert_eq!(tool.string("gen_ai.tool.call.arguments"), Some(""));
    assert!(!format!("{tool:?}").contains("secret-marker"));
}

#[test]
fn a_permission_that_names_an_mcp_tool_opens_an_extension_span() {
    let store = LocalTraceStore::new();
    let mut recorder = turn(&store);
    recorder.begin_turn(at(1), identity(), "");
    recorder.permission(PermissionFact::Requested {
        id: "review-1".into(),
        tool_id: "call-1".into(),
        tool_name: "show".into(),
        arguments: None,
        tool_type: "extension".into(),
        category_name: false,
    });
    recorder.tool(
        at(2),
        ToolFact::new(
            "call-1",
            "show",
            "function",
            ToolProgress::Completed,
            None,
            None,
        ),
    );
    recorder.finish(at(3), FinishReason::Stop);
    let tool = tool_named(&store, "call-1");
    assert_eq!(tool.string("gen_ai.tool.type"), Some("extension"));
}

#[test]
fn a_later_mcp_identity_upgrades_the_tool_type_and_a_sparse_update_does_not_downgrade_it() {
    let store = LocalTraceStore::new();
    let mut recorder = turn(&store);
    recorder.begin_turn(at(1), identity(), "");
    recorder.tool(
        at(2),
        ToolFact::new(
            "call-1",
            "show",
            "function",
            ToolProgress::Running,
            None,
            None,
        ),
    );
    recorder.tool(
        at(3),
        ToolFact::new(
            "call-1",
            "show",
            "extension",
            ToolProgress::Running,
            None,
            None,
        ),
    );
    recorder.tool(
        at(4),
        ToolFact::new(
            "call-1",
            "show",
            "function",
            ToolProgress::Completed,
            None,
            None,
        ),
    );
    recorder.finish(at(5), FinishReason::Stop);
    let tool = tool_named(&store, "call-1");
    assert_eq!(tool.string("gen_ai.tool.type"), Some("extension"));
}

fn tool_named(store: &LocalTraceStore, id: &str) -> GenAiSpan {
    store
        .read("conversation-1")
        .into_iter()
        .find(|span| span.string("gen_ai.tool.call.id") == Some(id))
        .unwrap()
}

fn stored_text_bytes(span: &GenAiSpan) -> usize {
    let mut total = span.name().len()
        + span.trace_id().len()
        + span.span_id().len()
        + span.operation().len()
        + span.parent_span_id().map(str::len).unwrap_or(0);
    if let SpanStatus::Error { error_type } = span.status() {
        total += error_type.len();
    }
    for value in span.attributes().values() {
        match value {
            AttributeValue::String(text) => total += text.len(),
            AttributeValue::Strings(values) => {
                total += values.iter().map(String::len).sum::<usize>()
            }
            AttributeValue::Int(_) | AttributeValue::Bool(_) => {}
        }
    }
    total
}
