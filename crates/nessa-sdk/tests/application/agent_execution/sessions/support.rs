//! Saved histories the session tests share: turns, snapshots, and the record
//! log a snapshot keeps, including a message steered natively into a running
//! turn. `app_sources` and `steering_position` both judge these histories.
use crate::application::agent_execution::{
    executions::{ExecutionEvent, ExecutionRequest, ExecutionUpdate},
    permissions::ActionContext,
    providers::ProviderIdentity,
    sessions::{
        InvocationRecord, InvocationSchedulingEvent, ProviderContext, QueueHistoryRecord,
        SessionChange, SessionSnapshot, SubmissionAcknowledgement,
    },
};
use crate::domain::agent_execution::{
    executions::{
        ExecutionId, ExecutionOutcome, InvocationKind, InvocationStage, QueueMutation,
        SchedulingCause, SubmissionMode,
    },
    prompts::{McpAppSource, MessageSender, PromptText, UserMessage},
    sessions::{ExecutionSessionId, SessionId},
    tools::{McpTool, ToolCallId, ToolCallUpdate},
};

/// The turn whose tool call `call-1` was to `charts/show` in the app tests,
/// and the running turn a message is steered into.
pub(super) const DRAWN: &str = "turn-1";
/// The steered message's own turn.
pub(super) const STEERED: &str = "steered";

pub(super) fn mcp(server: &str, tool: &str) -> McpTool {
    McpTool::new(server, tool).unwrap()
}
pub(super) fn app(turn: &str, tool_id: &str, tool: McpTool) -> McpAppSource {
    McpAppSource::new(
        ExecutionId::new(turn).unwrap(),
        ToolCallId::new(tool_id).unwrap(),
        tool,
    )
    .unwrap()
}
/// The app [`DRAWN`]'s `call-1` drew.
pub(super) fn drawn() -> McpAppSource {
    app(DRAWN, "call-1", mcp("charts", "show"))
}
pub(super) fn text() -> UserMessage {
    UserMessage::text_only(PromptText::new("plot").unwrap())
}
pub(super) fn from(app: McpAppSource) -> UserMessage {
    text().sent_by(MessageSender::App(app))
}
pub(super) fn tool_call(id: &str) -> ToolCallUpdate {
    ToolCallUpdate::new(ToolCallId::new(id).unwrap(), None, None, None, None, None)
}

/// A completed turn `id` whose message is `message` and whose tool calls
/// are `tools`, observed in that order.
pub(super) fn turn(id: &str, message: UserMessage, tools: Vec<ToolCallUpdate>) -> InvocationRecord {
    let id = ExecutionId::new(id).unwrap();
    let mut events: Vec<_> = tools
        .into_iter()
        .map(|update| ExecutionEvent::new(id.clone(), ExecutionUpdate::Tool(update)))
        .collect();
    events.push(ExecutionEvent::new(
        id.clone(),
        ExecutionUpdate::Finished(ExecutionOutcome::Completed),
    ));
    InvocationRecord {
        target_event_offset: None,
        submission: SubmissionMode::Immediate,
        request: ExecutionRequest {
            execution_id: id,
            user_message: message,
            estimated_input_tokens: 1,
            reserved_output_tokens: 1,
        },
        actor: ActionContext::new("user", "test", "invoke").unwrap(),
        acknowledgement: SubmissionAcknowledgement::Pending,
        events,
        scheduling: Vec::new(),
        provider_report: None,
        local_cancellation: None,
        local_outcome: Some(ExecutionOutcome::Completed),
        cancellation: None,
        result: Some(Ok(ExecutionOutcome::Completed)),
    }
}
pub(super) fn snapshot(invocations: Vec<InvocationRecord>) -> SessionSnapshot {
    SessionSnapshot {
        id: SessionId::new("session").unwrap(),
        provider: ProviderIdentity::new("provider", "model", "").unwrap(),
        provider_context: ProviderContext::Recorded(ExecutionSessionId::new("provider").unwrap()),
        invocations,
        queue_history: Vec::new(),
        lease: None,
    }
}
/// The changes a record log keeps for `snapshot`, in its order.
pub(super) fn record_log(snapshot: &SessionSnapshot) -> Vec<SessionChange> {
    let mut changes = vec![SessionChange::Opened {
        id: snapshot.id.clone(),
        provider: snapshot.provider.clone(),
        context: snapshot.provider_context.clone(),
    }];
    for record in &snapshot.invocations {
        let mut input = record.clone();
        input.events.clear();
        input.result = None;
        input.local_outcome = None;
        changes.push(SessionChange::InputAccepted(Box::new(input)));
        changes.extend(
            record
                .events
                .iter()
                .cloned()
                .map(SessionChange::ProviderObservation),
        );
        changes.push(SessionChange::LocalSettlement {
            execution_id: record.request.execution_id.clone(),
            before: None,
            after: record.result.clone().unwrap(),
            local_outcome: record.local_outcome,
        });
    }
    changes
}

/// The record log of the completed turns `earlier`, then `target` running,
/// observing `before`, then `message` steered natively into it as
/// [`STEERED`] (its `target_event_offset` the count of `before`), then
/// `target` observing `after`.
pub(super) fn steered_log(
    earlier: &[InvocationRecord],
    target: &str,
    message: UserMessage,
    before: Vec<ToolCallUpdate>,
    after: Vec<ToolCallUpdate>,
) -> Vec<SessionChange> {
    let target = ExecutionId::new(target).unwrap();
    let event = |kind, target: Option<&ExecutionId>, before, stage, cause, actor| {
        InvocationSchedulingEvent {
            kind,
            target: target.cloned(),
            before,
            stage,
            cause,
            actor,
        }
    };
    let actor = || Some(ActionContext::new("user", "test", "invoke").unwrap());
    let mut running = turn(target.as_str(), text(), Vec::new());
    running.events.clear();
    running.result = None;
    running.local_outcome = None;
    running.submission = SubmissionMode::Queued;
    running.scheduling = vec![event(
        InvocationKind::Queued,
        None,
        None,
        InvocationStage::Queued,
        SchedulingCause::Submitted,
        actor(),
    )];
    let mut steering = turn(STEERED, message, Vec::new());
    steering.events.clear();
    steering.result = None;
    steering.local_outcome = None;
    steering.submission = SubmissionMode::Steering;
    steering.target_event_offset = Some(before.len());
    steering.scheduling = vec![event(
        InvocationKind::Steering,
        Some(&target),
        None,
        InvocationStage::Queued,
        SchedulingCause::Submitted,
        actor(),
    )];
    let observed = |tools: Vec<ToolCallUpdate>| {
        tools.into_iter().map(|update| {
            SessionChange::ProviderObservation(ExecutionEvent::new(
                target.clone(),
                ExecutionUpdate::Tool(update),
            ))
        })
    };
    // Opened, then each earlier turn whole.
    let mut changes = record_log(&snapshot(earlier.to_vec()));
    changes.extend([
        SessionChange::InputAccepted(Box::new(running)),
        SessionChange::QueueDecision(QueueHistoryRecord {
            mutation: QueueMutation::Admitted {
                id: target.clone(),
                kind: InvocationKind::Queued,
            },
            actor: actor(),
            scheduling_length: Some(1),
        }),
        SessionChange::QueueDecision(QueueHistoryRecord {
            mutation: QueueMutation::Selected { id: target.clone() },
            actor: None,
            scheduling_length: Some(1),
        }),
        SessionChange::SchedulingTransition {
            execution_id: target.clone(),
            event: event(
                InvocationKind::Queued,
                None,
                Some(InvocationStage::Queued),
                InvocationStage::Running,
                SchedulingCause::Dispatched,
                None,
            ),
        },
    ]);
    changes.extend(observed(before));
    changes.push(SessionChange::InputAccepted(Box::new(steering)));
    changes.push(SessionChange::SchedulingTransition {
        execution_id: ExecutionId::new(STEERED).unwrap(),
        event: event(
            InvocationKind::Steering,
            Some(&target),
            Some(InvocationStage::Queued),
            InvocationStage::Injected,
            SchedulingCause::SteeringInjected,
            None,
        ),
    });
    changes.extend(observed(after));
    changes
}

/// The saved invocation `id` of `snapshot`.
pub(super) fn invocation_mut<'a>(
    snapshot: &'a mut SessionSnapshot,
    id: &str,
) -> &'a mut InvocationRecord {
    snapshot
        .invocations
        .iter_mut()
        .find(|record| record.request.execution_id.as_str() == id)
        .unwrap()
}
