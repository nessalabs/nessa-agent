//! Fold an observation the worker already decoded into a GenAI span.
//! The recorder does not read the harness. These functions only name what
//! the profile and the session update already said.

use crate::application::agent_execution::executions::{ExecutionEvent, ExecutionUpdate};
use crate::application::agent_execution::permissions::{ApprovalBasis, CancellationOrigin};
use crate::application::agent_execution::traces::{
    FinishReason, ModelChannel, PermissionFact, ToolFact, ToolProgress, TraceRecorder,
};
use crate::domain::agent_execution::executions::{ExecutionOutcome, MessageKind};
use crate::domain::agent_execution::tools::{ToolCallUpdate, ToolContentView, ToolStatus};
use crate::infrastructure::acp::tools::wire::{kind_from_spelling, kind_spelling};
use std::time::Duration;

pub(super) fn observe_event(recorder: &mut TraceRecorder, at: Duration, event: &ExecutionEvent) {
    match event.update() {
        ExecutionUpdate::Finished(outcome) => recorder.finish(at, finish_reason(*outcome)),
        ExecutionUpdate::Message(chunk) => {
            let channel = match chunk.kind() {
                MessageKind::Text => ModelChannel::Text,
                MessageKind::Thought => ModelChannel::Thought,
            };
            let text = if recorder.captures_content() && channel == ModelChannel::Text {
                chunk.as_str()
            } else {
                ""
            };
            recorder.model_output(at, channel, chunk.message_id(), text);
        }
        ExecutionUpdate::Tool(_) => {}
        ExecutionUpdate::PermissionRequested {
            id,
            tool_id,
            observation,
            input,
            ..
        } => {
            let arguments = if recorder.captures_content() {
                Some(input.arguments_json.clone())
            } else {
                None
            };
            let tool_type = if observation.mcp_tool().is_some() {
                "extension"
            } else {
                "function"
            };
            recorder.permission(
                at,
                PermissionFact::Requested {
                    id: id.as_str().to_owned(),
                    tool_id: tool_id.as_str().to_owned(),
                    category_name: kind_from_spelling(&input.name).is_some(),
                    tool_name: input.name.clone(),
                    arguments,
                    tool_type: tool_type.to_owned(),
                },
            );
        }
        ExecutionUpdate::PermissionCancelled(record) => {
            let decider = match record.origin() {
                CancellationOrigin::Client(_) => "person",
                CancellationOrigin::Deadline(_) => "deadline",
                CancellationOrigin::Provider => "provider",
                CancellationOrigin::Runtime => "runtime",
            };
            recorder.permission(
                at,
                PermissionFact::Cancelled {
                    id: record.request().id().as_str().to_owned(),
                    tool_id: record.request().tool_id().as_str().to_owned(),
                    decider: decider.to_owned(),
                },
            );
        }
        ExecutionUpdate::ReviewDeclined(observation) => {
            recorder.permission(
                at,
                PermissionFact::Declined {
                    id: observation.id().as_str().to_owned(),
                    tool_name: observation.decline().declared().map(str::to_owned),
                },
            );
        }
        ExecutionUpdate::DeadlineAnswered { id, effect, .. } => {
            recorder.permission(
                at,
                PermissionFact::Deadline {
                    id: id.as_str().to_owned(),
                    effect: effect.as_str().to_owned(),
                },
            );
        }
        ExecutionUpdate::QuestionAsked { .. } | ExecutionUpdate::QuestionClosed { .. } => {}
    }
}

pub(super) fn tool_fact(
    name: Option<String>,
    arguments: Option<String>,
    update: &ToolCallUpdate,
    capture: bool,
) -> ToolFact {
    let id = update.id().as_str().to_owned();
    // A title is display text. OpenCode builds some of them from model
    // arguments, so it is captured content. A retained provider name and the
    // ACP kind are identities and stay when capture is off.
    let title = capture
        .then(|| update.title().clone())
        .flatten()
        .filter(|title| !title.is_empty());
    let name = name
        .filter(|name| !name.is_empty())
        .or(title)
        .or_else(|| update.kind().map(kind_spelling).map(str::to_owned))
        .unwrap_or_else(|| id.clone());
    let category_name = kind_from_spelling(&name).is_some();
    let tool_type = if update.mcp_tool().is_some() {
        "extension"
    } else {
        "function"
    };
    let progress = match update.status() {
        Some(ToolStatus::Completed) => ToolProgress::Completed,
        Some(ToolStatus::Failed) => ToolProgress::Failed,
        Some(ToolStatus::Pending | ToolStatus::Running) | None => ToolProgress::Running,
    };
    ToolFact::new(
        id,
        name,
        tool_type,
        progress,
        capture.then_some(arguments).flatten(),
        capture.then(|| result_text(update)).flatten(),
    )
    .with_category_name(category_name)
}

pub(super) fn permission_decider(basis: &ApprovalBasis) -> &'static str {
    match basis {
        ApprovalBasis::Explicit => "person",
        ApprovalBasis::Mode(_) => "mode",
        ApprovalBasis::Rule(_) => "rule",
        ApprovalBasis::Deadline(_) => "deadline",
        ApprovalBasis::Hook(_) => "hook",
    }
}

pub(super) fn finish_reason(outcome: ExecutionOutcome) -> FinishReason {
    match outcome {
        ExecutionOutcome::Completed => FinishReason::Stop,
        ExecutionOutcome::OutputLimit => FinishReason::Length,
        ExecutionOutcome::RequestLimit => FinishReason::RequestLimit,
        ExecutionOutcome::Refused => FinishReason::Refused,
        ExecutionOutcome::Cancelled => FinishReason::Cancelled,
    }
}

fn result_text(update: &ToolCallUpdate) -> Option<String> {
    let content = update.content().as_ref()?;
    let mut text = String::new();
    for item in content {
        if !text.is_empty() {
            text.push('\n');
        }
        match item.view() {
            ToolContentView::Text(value) | ToolContentView::Structured(value) => {
                text.push_str(value)
            }
            ToolContentView::Diff { path, new, .. } => {
                text.push_str(path.as_str());
                text.push('\n');
                text.push_str(new);
            }
        }
    }
    // Present content that renders nothing is an explicit empty result.
    // Omitted content stays `None`, so an earlier result is left in place.
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::{observe_event, permission_decider, tool_fact};
    use crate::application::agent_execution::executions::{ExecutionEvent, ExecutionUpdate};
    use crate::application::agent_execution::permissions::{
        ActionContext, ApprovalBasis, ApprovalModeSnapshot, ApprovalRuleReference, HookRefusal,
    };
    use crate::application::agent_execution::tools::ToolReviewInput;
    use crate::application::agent_execution::traces::{
        ContentCapture, FinishReason, GenAiSpan, LocalTraceStore, TraceRecorder, TurnIdentity,
    };
    use crate::domain::agent_execution::executions::ExecutionId;
    use crate::domain::agent_execution::permissions::{
        PermissionDecision, PermissionEffect, PermissionId, PermissionOfferPolicy,
        PermissionOption, PermissionOptionId, PermissionOptions, PermissionScope,
    };
    use crate::domain::agent_execution::tools::{
        McpTool, ToolCallId, ToolCallUpdate, ToolContent, ToolKind, ToolObservation, ToolStatus,
    };
    use std::time::Duration;

    fn update(title: Option<&str>, kind: Option<ToolKind>) -> ToolCallUpdate {
        ToolCallUpdate::new(
            ToolCallId::new("call-1").unwrap(),
            title.map(str::to_owned),
            kind,
            Some(ToolStatus::Completed),
            None,
            None,
        )
    }

    fn recorded(fact_name: Option<&str>, update: &ToolCallUpdate, capture: bool) -> Vec<GenAiSpan> {
        let store = if capture {
            LocalTraceStore::with_capture(ContentCapture::on())
        } else {
            LocalTraceStore::new()
        };
        let mut recorder = TraceRecorder::new(store.clone(), store.capture());
        recorder.begin_turn(
            Duration::from_millis(1),
            TurnIdentity::new(
                "conversation-1",
                "execution-1",
                "opencode",
                "agent",
                "model",
                None,
            ),
            "",
        );
        recorder.tool(
            Duration::from_millis(2),
            tool_fact(fact_name.map(str::to_owned), None, update, capture),
        );
        recorder.finish(Duration::from_millis(3), FinishReason::Stop);
        store.read("conversation-1")
    }

    fn tool_span(spans: &[GenAiSpan]) -> &GenAiSpan {
        spans
            .iter()
            .find(|span| span.operation() == "execute_tool")
            .unwrap()
    }

    #[test]
    fn a_display_title_is_not_the_tool_name_when_capture_is_off() {
        let spans = recorded(
            None,
            &update(Some("websearch secret-marker"), Some(ToolKind::Fetch)),
            false,
        );
        let tool = tool_span(&spans);
        assert_eq!(tool.string("gen_ai.tool.name"), Some("fetch"));
        assert_eq!(tool.name(), "execute_tool fetch");
        assert!(!format!("{spans:?}").contains("secret-marker"));
    }

    #[test]
    fn a_display_title_without_a_kind_uses_the_call_id_when_capture_is_off() {
        let spans = recorded(None, &update(Some("websearch secret-marker"), None), false);
        let tool = tool_span(&spans);
        assert_eq!(tool.string("gen_ai.tool.name"), Some("call-1"));
        assert_eq!(tool.name(), "execute_tool call-1");
        assert!(!format!("{spans:?}").contains("secret-marker"));
    }

    #[test]
    fn a_retained_name_stays_when_capture_is_off() {
        let spans = recorded(
            Some("Read"),
            &update(Some("websearch secret-marker"), Some(ToolKind::Read)),
            false,
        );
        let tool = tool_span(&spans);
        assert_eq!(tool.string("gen_ai.tool.name"), Some("Read"));
        assert_eq!(tool.name(), "execute_tool Read");
        assert!(!format!("{spans:?}").contains("secret-marker"));
    }

    #[test]
    fn a_display_title_names_the_tool_when_capture_is_on() {
        let spans = recorded(
            None,
            &update(Some("websearch secret-marker"), Some(ToolKind::Fetch)),
            true,
        );
        let tool = tool_span(&spans);
        assert_eq!(
            tool.string("gen_ai.tool.name"),
            Some("websearch secret-marker")
        );
        assert_eq!(tool.name(), "execute_tool websearch secret-marker");
    }

    #[test]
    fn an_explicit_empty_tool_result_replaces_the_one_seen_while_running() {
        let store = LocalTraceStore::with_capture(ContentCapture::on());
        let mut recorder = TraceRecorder::new(store.clone(), ContentCapture::on());
        recorder.begin_turn(
            Duration::from_millis(1),
            TurnIdentity::new(
                "conversation-1",
                "execution-1",
                "opencode",
                "agent",
                "model",
                None,
            ),
            "",
        );
        let running = ToolCallUpdate::new(
            ToolCallId::new("call-1").unwrap(),
            None,
            Some(ToolKind::Read),
            Some(ToolStatus::Running),
            None,
            Some(vec![ToolContent::text("secret-marker")]),
        );
        let cleared = ToolCallUpdate::new(
            ToolCallId::new("call-1").unwrap(),
            None,
            Some(ToolKind::Read),
            Some(ToolStatus::Completed),
            None,
            Some(vec![]),
        );
        recorder.tool(
            Duration::from_millis(2),
            tool_fact(Some("Read".into()), None, &running, true),
        );
        recorder.tool(
            Duration::from_millis(3),
            tool_fact(Some("Read".into()), None, &cleared, true),
        );
        recorder.finish(Duration::from_millis(4), FinishReason::Stop);
        let spans = store.read("conversation-1");
        let tool = tool_span(&spans);
        assert_eq!(tool.string("gen_ai.tool.call.result"), Some(""));
        assert!(!format!("{tool:?}").contains("secret-marker"));
    }

    #[test]
    fn an_explicit_empty_tool_result_is_recorded_when_nothing_preceded_it() {
        let store = LocalTraceStore::with_capture(ContentCapture::on());
        let mut recorder = TraceRecorder::new(store.clone(), ContentCapture::on());
        recorder.begin_turn(
            Duration::from_millis(1),
            TurnIdentity::new(
                "conversation-1",
                "execution-1",
                "opencode",
                "agent",
                "model",
                None,
            ),
            "",
        );
        let cleared = ToolCallUpdate::new(
            ToolCallId::new("call-1").unwrap(),
            None,
            None,
            Some(ToolStatus::Completed),
            None,
            Some(vec![]),
        );
        recorder.tool(
            Duration::from_millis(2),
            tool_fact(Some("Read".into()), None, &cleared, true),
        );
        recorder.finish(Duration::from_millis(3), FinishReason::Stop);
        let spans = store.read("conversation-1");
        let tool = tool_span(&spans);
        assert_eq!(tool.string("gen_ai.tool.call.result"), Some(""));
    }

    #[test]
    fn an_explicit_empty_running_result_clears_the_one_already_held() {
        let store = LocalTraceStore::with_capture(ContentCapture::on());
        let mut recorder = TraceRecorder::new(store.clone(), ContentCapture::on());
        recorder.begin_turn(
            Duration::from_millis(1),
            TurnIdentity::new(
                "conversation-1",
                "execution-1",
                "opencode",
                "agent",
                "model",
                None,
            ),
            "",
        );
        let running = ToolCallUpdate::new(
            ToolCallId::new("call-1").unwrap(),
            None,
            Some(ToolKind::Read),
            Some(ToolStatus::Running),
            None,
            Some(vec![ToolContent::text("secret-marker")]),
        );
        let cleared = ToolCallUpdate::new(
            ToolCallId::new("call-1").unwrap(),
            None,
            Some(ToolKind::Read),
            Some(ToolStatus::Running),
            None,
            Some(vec![]),
        );
        let status_only = ToolCallUpdate::new(
            ToolCallId::new("call-1").unwrap(),
            None,
            Some(ToolKind::Read),
            Some(ToolStatus::Completed),
            None,
            None,
        );
        recorder.tool(
            Duration::from_millis(2),
            tool_fact(Some("Read".into()), None, &running, true),
        );
        recorder.tool(
            Duration::from_millis(3),
            tool_fact(Some("Read".into()), None, &cleared, true),
        );
        recorder.tool(
            Duration::from_millis(4),
            tool_fact(Some("Read".into()), None, &status_only, true),
        );
        recorder.finish(Duration::from_millis(5), FinishReason::Stop);
        let spans = store.read("conversation-1");
        let tool = tool_span(&spans);
        assert_eq!(tool.string("gen_ai.tool.call.result"), Some(""));
        assert!(!format!("{tool:?}").contains("secret-marker"));
    }

    #[test]
    fn a_kind_spelling_is_replaced_by_a_later_provider_name() {
        let store = LocalTraceStore::new();
        let mut recorder = TraceRecorder::new(store.clone(), ContentCapture::off());
        recorder.begin_turn(
            Duration::from_millis(1),
            TurnIdentity::new(
                "conversation-1",
                "execution-1",
                "openai",
                "codex",
                "model",
                None,
            ),
            "",
        );
        recorder.tool(
            Duration::from_millis(2),
            tool_fact(
                None,
                None,
                &ToolCallUpdate::new(
                    ToolCallId::new("call-1").unwrap(),
                    None,
                    Some(ToolKind::Execute),
                    Some(ToolStatus::Running),
                    None,
                    None,
                ),
                false,
            ),
        );
        recorder.tool(
            Duration::from_millis(3),
            tool_fact(
                Some("exec_command".into()),
                None,
                &update(None, Some(ToolKind::Execute)),
                false,
            ),
        );
        recorder.finish(Duration::from_millis(4), FinishReason::Stop);
        let spans = store.read("conversation-1");
        let tool = tool_span(&spans);
        assert_eq!(tool.string("gen_ai.tool.name"), Some("exec_command"));
    }

    #[test]
    fn a_permission_observation_that_names_an_mcp_tool_opens_an_extension_span() {
        let store = LocalTraceStore::new();
        let mut recorder = TraceRecorder::new(store.clone(), ContentCapture::off());
        recorder.begin_turn(
            Duration::from_millis(1),
            TurnIdentity::new(
                "conversation-1",
                "execution-1",
                "openai",
                "codex",
                "model",
                None,
            ),
            "",
        );
        let id = ToolCallId::new("call-1").unwrap();
        let observation = ToolObservation::default().with_update(
            ToolCallUpdate::new(id.clone(), None, None, None, None, None)
                .with_mcp_tool(McpTool::new("charts", "show").unwrap()),
        );
        let options = PermissionOptions::new(
            vec![PermissionOption::new(
                PermissionOptionId::new("allow").unwrap(),
                "Allow",
                PermissionDecision::new(PermissionEffect::Allow, PermissionScope::request()),
            )
            .unwrap()],
            &PermissionOfferPolicy::once_only(),
        )
        .unwrap();
        observe_event(
            &mut recorder,
            Duration::from_millis(2),
            &ExecutionEvent::new(
                ExecutionId::new("execution-1").unwrap(),
                ExecutionUpdate::PermissionRequested {
                    id: PermissionId::new("review-1").unwrap(),
                    tool_id: id,
                    observation,
                    input: ToolReviewInput {
                        name: "show".into(),
                        arguments_json: "{}".into(),
                    },
                    options,
                    deadline: None,
                },
            ),
        );
        recorder.tool(
            Duration::from_millis(3),
            tool_fact(Some("show".into()), None, &update(None, None), false),
        );
        recorder.finish(Duration::from_millis(4), FinishReason::Stop);
        let spans = store.read("conversation-1");
        let tool = tool_span(&spans);
        assert_eq!(tool.string("gen_ai.tool.type"), Some("extension"));
        assert_eq!(tool.string("gen_ai.tool.name"), Some("show"));
    }

    #[test]
    fn a_later_mcp_update_upgrades_the_tool_type() {
        let store = LocalTraceStore::new();
        let mut recorder = TraceRecorder::new(store.clone(), ContentCapture::off());
        recorder.begin_turn(
            Duration::from_millis(1),
            TurnIdentity::new(
                "conversation-1",
                "execution-1",
                "openai",
                "codex",
                "model",
                None,
            ),
            "",
        );
        recorder.tool(
            Duration::from_millis(2),
            tool_fact(
                Some("show".into()),
                None,
                &ToolCallUpdate::new(
                    ToolCallId::new("call-1").unwrap(),
                    None,
                    None,
                    Some(ToolStatus::Running),
                    None,
                    None,
                ),
                false,
            ),
        );
        recorder.tool(
            Duration::from_millis(3),
            tool_fact(
                Some("show".into()),
                None,
                &update(None, None).with_mcp_tool(McpTool::new("charts", "show").unwrap()),
                false,
            ),
        );
        recorder.finish(Duration::from_millis(4), FinishReason::Stop);
        let spans = store.read("conversation-1");
        let tool = tool_span(&spans);
        assert_eq!(tool.string("gen_ai.tool.type"), Some("extension"));
    }

    #[test]
    fn a_decider_follows_the_answer_basis() {
        let actor = ActionContext::new("owner", "surface", "request").unwrap();
        assert_eq!(permission_decider(&ApprovalBasis::Explicit), "person");
        assert_eq!(
            permission_decider(&ApprovalBasis::Mode(
                ApprovalModeSnapshot::new("ask", "1").unwrap()
            )),
            "mode"
        );
        assert_eq!(
            permission_decider(&ApprovalBasis::Rule(
                ApprovalRuleReference::new("rule", "1", actor).unwrap()
            )),
            "rule"
        );
        assert_eq!(
            permission_decider(&ApprovalBasis::Hook(HookRefusal::new(Some(0), "no"))),
            "hook"
        );
    }
}
