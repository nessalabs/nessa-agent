//! Owner-authorized committed event readers and their bounded metadata projection.
//! Readers own no Agent, writer lease, or callbacks into execution.

use super::{
    conversation_session, load_conversation, ConversationCaller, ConversationError,
    ConversationService,
};
use crate::conversation::application::error_code;
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::application::agent_execution::{
    executions::ExecutionUpdate,
    sessions::{
        CommittedEvent, CommittedEventCursor, CommittedEventData, CommittedEventReader,
        EventReadError, EventStart, SessionChange, StorageError,
    },
};
use nessa_sdk::domain::agent_execution::{
    executions::{ExecutionOutcome, InvocationStage},
    tools::ToolStatus,
};
use serde::Serialize;

/// A refused read, preserving access failure separately from cursor/storage failure.
#[derive(Debug)]
pub enum ConversationEventError {
    /// Existing owner admission or metadata failure.
    Conversation(ConversationError),
    /// Committed reader failure, never a reason to restart provider work.
    Events(EventReadError),
    /// Saved history contains a gap; automation must choose how to recover.
    Gap,
}

/// Stable public refusal codes for gateway and agent-tool consumers.
pub fn event_error_code(error: &ConversationEventError) -> &'static str {
    match error {
        ConversationEventError::Conversation(error) => error_code(error).as_str(),
        ConversationEventError::Events(error) => match error {
            EventReadError::Unsupported => "events_unsupported",
            EventReadError::NotFound => "conversation_not_found",
            EventReadError::InvalidCursor => "event_cursor_invalid",
            EventReadError::IdentityChanged => "event_history_changed",
            EventReadError::HistoryPruned => "event_history_pruned",
            EventReadError::Capacity => "subscription_capacity",
            EventReadError::ItemTooLarge => "event_too_large",
            EventReadError::Closed => "event_source_closed",
            EventReadError::Storage(StorageError::Corrupt(_)) => "event_history_gap",
            EventReadError::Storage(_) => "unavailable",
        },
        ConversationEventError::Gap => "event_history_gap",
    }
}

/// Metadata from one committed semantic change. Content and thinking stay out.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadEvent {
    /// Stable identity, identical on replay.
    pub id: String,
    /// Resume strictly after this item.
    pub cursor: String,
    /// Semantic kind, independent from private storage representation.
    pub kind: &'static str,
    /// Execution whose change was committed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_id: Option<String>,
    /// Tool identity within the execution, when applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_id: Option<String>,
    /// MCP tool name, never a display title containing command arguments.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Observed state, when present in this sparse change.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<&'static str>,
    /// Local terminal outcome, when applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<&'static str>,
}

impl ConversationService {
    /// Check current owner access without opening an Agent or writer.
    pub async fn authorize_events(
        &self,
        id: &ConversationId,
        caller: &ConversationCaller,
    ) -> Result<(), ConversationError> {
        let record = load_conversation(self.inner.metadata.as_ref(), id)
            .await?
            .ok_or(ConversationError::NotFound)?;
        self.access().admit(&record, caller)?;
        Ok(())
    }

    /// Open an independent bounded semantic reader after owner admission.
    /// Dropping it releases reader state; it stops no execution.
    pub async fn open_events(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
        start: EventStart,
    ) -> Result<Box<dyn CommittedEventReader>, ConversationEventError> {
        let _admission = self
            .admit()
            .await
            .map_err(|error| ConversationEventError::Conversation(error.into()))?;
        self.authorize_events(&id, &caller)
            .await
            .map_err(ConversationEventError::Conversation)?;
        self.inner
            .storage
            .events(conversation_session(&id), start)
            .await
            .map_err(ConversationEventError::Events)
    }
}

/// Project observed metadata only. A gap is explicit rather than silently skipped.
pub fn project_events(
    id: &ConversationId,
    events: &[CommittedEvent],
) -> Result<Vec<ThreadEvent>, ConversationEventError> {
    for event in events {
        validate_event_cursor(id, event.cursor())?;
    }
    events
        .iter()
        .filter_map(|event| {
            let change = match event.data() {
                CommittedEventData::Change(change) => change,
                CommittedEventData::Gap(_) => return Some(Err(ConversationEventError::Gap)),
            };
            let mut projected = ThreadEvent {
                id: event.cursor().encode(),
                cursor: event.cursor().encode(),
                kind: "state.changed",
                execution_id: None,
                tool_id: None,
                name: None,
                status: None,
                outcome: None,
            };
            match change {
                SessionChange::InputAccepted(record) => {
                    projected.kind = "turn.accepted";
                    projected.execution_id = Some(record.request.execution_id.as_str().to_owned());
                    projected.status = Some("accepted");
                }
                SessionChange::SchedulingTransition {
                    execution_id,
                    event,
                } => {
                    projected.execution_id = Some(execution_id.as_str().to_owned());
                    projected.status = Some(match event.stage {
                        InvocationStage::Queued => "queued",
                        InvocationStage::Running => "running",
                        InvocationStage::Injected => "injected",
                        InvocationStage::Settled => "settled",
                        InvocationStage::Cancelled => "cancelled",
                    });
                    if event.stage == InvocationStage::Running {
                        projected.kind = "turn.started";
                    }
                }
                SessionChange::LocalSettlement {
                    execution_id,
                    before,
                    after,
                    ..
                } => {
                    projected.kind = if before.is_none() {
                        "turn.ended"
                    } else {
                        "state.changed"
                    };
                    projected.status = Some(if before.is_none() {
                        "settled"
                    } else {
                        "settlement_updated"
                    });
                    projected.execution_id = Some(execution_id.as_str().to_owned());
                    projected.outcome = Some(match after {
                        Ok(ExecutionOutcome::Completed) => "completed",
                        Ok(ExecutionOutcome::OutputLimit) => "output_limit",
                        Ok(ExecutionOutcome::RequestLimit) => "request_limit",
                        Ok(ExecutionOutcome::Refused) => "refused",
                        Ok(ExecutionOutcome::Cancelled) => "cancelled",
                        Err(_) => "failed",
                    });
                }
                SessionChange::ProviderObservation(event) => {
                    projected.execution_id = Some(event.execution_id().as_str().to_owned());
                    match event.update() {
                        ExecutionUpdate::Tool(tool) => {
                            projected.kind = "tool.updated";
                            projected.tool_id = Some(tool.id().as_str().to_owned());
                            projected.name = tool.mcp_tool().map(|tool| tool.tool().to_owned());
                            projected.status = tool.status().map(|status| match status {
                                ToolStatus::Pending => "pending",
                                ToolStatus::Running => "running",
                                ToolStatus::Completed => "completed",
                                ToolStatus::Failed => "failed",
                            });
                        }
                        ExecutionUpdate::PermissionRequested { .. } => {
                            projected.status = Some("waiting_permission")
                        }
                        ExecutionUpdate::PermissionCancelled(_) => {
                            projected.status = Some("permission_cancelled")
                        }
                        ExecutionUpdate::ReviewDeclined(_) => {
                            projected.status = Some("permission_declined")
                        }
                        ExecutionUpdate::DeadlineAnswered { .. } => {
                            projected.status = Some("permission_deadline_answered")
                        }
                        ExecutionUpdate::QuestionAsked { .. } => {
                            projected.status = Some("waiting_question")
                        }
                        ExecutionUpdate::QuestionClosed { .. } => {
                            projected.status = Some("question_closed")
                        }
                        ExecutionUpdate::Finished(_) | ExecutionUpdate::Message(_) => return None,
                    }
                }
                SessionChange::Opened { .. } => projected.status = Some("opened"),
                SessionChange::StopDecision { execution_id, .. } => {
                    projected.execution_id = Some(execution_id.as_str().to_owned());
                    projected.status = Some("stop_requested");
                }
                _ => return None,
            }
            Some(Ok(projected))
        })
        .collect()
}

/// Correlate custom source replies before publishing data or checkpoints.
pub fn validate_event_cursor(
    id: &ConversationId,
    cursor: &CommittedEventCursor,
) -> Result<(), ConversationEventError> {
    if cursor.session() != &conversation_session(id) {
        return Err(ConversationEventError::Events(
            EventReadError::IdentityChanged,
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../tests/conversation/event_read.rs"]
mod tests;
