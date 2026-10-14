//! Fold an observation the worker already decoded into a GenAI span.
//! The recorder does not read the harness. These functions only name what
//! the profile and the session update already said.

use crate::application::agent_execution::executions::{ExecutionEvent, ExecutionUpdate};
use crate::application::agent_execution::permissions::CancellationOrigin;
use crate::application::agent_execution::traces::{
    FinishReason, ModelChannel, PermissionFact, ToolFact, ToolProgress, TraceRecorder,
};
use crate::domain::agent_execution::executions::{ExecutionOutcome, MessageKind};
use crate::domain::agent_execution::tools::{
    ToolCallUpdate, ToolContentView, ToolKind, ToolStatus,
};
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
            id, tool_id, input, ..
        } => {
            let arguments = if recorder.captures_content() {
                Some(input.arguments_json.clone())
            } else {
                None
            };
            recorder.permission(PermissionFact::Requested {
                id: id.as_str().to_owned(),
                tool_id: tool_id.as_str().to_owned(),
                tool_name: input.name.clone(),
                arguments,
            });
        }
        ExecutionUpdate::PermissionCancelled(record) => {
            let decider = match record.origin() {
                CancellationOrigin::Client(_) => "person",
                CancellationOrigin::Deadline(_) => "deadline",
                CancellationOrigin::Provider => "provider",
                CancellationOrigin::Runtime => "runtime",
            };
            recorder.permission(PermissionFact::Cancelled {
                id: record.request().id().as_str().to_owned(),
                tool_id: record.request().tool_id().as_str().to_owned(),
                decider: decider.to_owned(),
            });
        }
        ExecutionUpdate::ReviewDeclined(observation) => {
            recorder.permission(PermissionFact::Declined {
                id: observation.id().as_str().to_owned(),
                tool_name: observation.decline().declared().map(str::to_owned),
            });
        }
        ExecutionUpdate::DeadlineAnswered { id, effect, .. } => {
            recorder.permission(PermissionFact::Deadline {
                id: id.as_str().to_owned(),
                effect: effect.as_str().to_owned(),
            });
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
    let name = name
        .filter(|name| !name.is_empty())
        .or_else(|| update.title().clone().filter(|title| !title.is_empty()))
        .or_else(|| update.kind().as_ref().map(kind_label).map(str::to_owned))
        .unwrap_or_else(|| id.clone());
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

fn kind_label(kind: &ToolKind) -> &'static str {
    match kind {
        ToolKind::Read => "read",
        ToolKind::Edit => "edit",
        ToolKind::Search => "search",
        ToolKind::Fetch => "fetch",
        ToolKind::Execute => "execute",
        ToolKind::Think => "think",
        ToolKind::Delete => "delete",
        ToolKind::Move => "move",
        ToolKind::SwitchMode => "switch_mode",
        ToolKind::Other => "other",
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
    (!text.is_empty()).then_some(text)
}
