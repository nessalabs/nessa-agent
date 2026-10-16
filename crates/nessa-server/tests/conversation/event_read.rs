//! Metadata projection and source-correlation regressions.
use super::*;
use nessa_sdk::application::agent_execution::{agents::AgentError, executions::ExecutionEvent};
use nessa_sdk::domain::agent_execution::{
    executions::ExecutionId,
    tools::{McpTool, ToolCallId, ToolCallUpdate},
};
use nessa_sync::replication::domain::Id;

fn conversation() -> ConversationId {
    ConversationId::new("00000000-0000-4000-8000-000000000001").unwrap()
}
fn cursor(id: &ConversationId, index: u64) -> CommittedEventCursor {
    CommittedEventCursor::new(
        conversation_session(id),
        Id::new("incarnation").unwrap(),
        1,
        Some(index),
    )
}

#[test]
fn projection_omits_sensitive_content_and_provider_finished_is_not_local_end() {
    let id = conversation();
    let execution = ExecutionId::new("turn").unwrap();
    let tool = ToolCallUpdate::new(
        ToolCallId::new("tool").unwrap(),
        Some("shell SECRET ARGUMENT".into()),
        None,
        Some(ToolStatus::Running),
        None,
        None,
    )
    .with_mcp_tool(McpTool::new("nessa", "shell").unwrap());
    let events = vec![
        CommittedEvent::change(
            cursor(&id, 1),
            SessionChange::ProviderObservation(ExecutionEvent::new(
                execution.clone(),
                ExecutionUpdate::Tool(tool),
            )),
        ),
        CommittedEvent::change(
            cursor(&id, 2),
            SessionChange::ProviderObservation(ExecutionEvent::new(
                execution,
                ExecutionUpdate::Finished(ExecutionOutcome::Completed),
            )),
        ),
    ];
    let projected = project_events(&id, &events).unwrap();
    assert_eq!(projected.len(), 1);
    assert_eq!(projected[0].kind, "tool.updated");
    assert_eq!(projected[0].name.as_deref(), Some("shell"));
    let json = serde_json::to_string(&projected).unwrap();
    assert!(!json.contains("SECRET"));
    assert!(!json.contains("arguments"));
}

#[test]
fn settlement_correction_updates_state_without_a_second_turn_end() {
    let id = conversation();
    let execution = ExecutionId::new("turn").unwrap();
    let events = vec![
        CommittedEvent::change(
            cursor(&id, 1),
            SessionChange::LocalSettlement {
                execution_id: execution.clone(),
                before: None,
                after: Ok(ExecutionOutcome::Completed),
                local_outcome: None,
            },
        ),
        CommittedEvent::change(
            cursor(&id, 2),
            SessionChange::LocalSettlement {
                execution_id: execution,
                before: Some(Ok(ExecutionOutcome::Completed)),
                after: Err(AgentError::Closed),
                local_outcome: Some(ExecutionOutcome::Completed),
            },
        ),
    ];
    let projected = project_events(&id, &events).unwrap();
    assert_eq!(projected[0].kind, "turn.ended");
    assert_eq!(projected[1].kind, "state.changed");
    assert_eq!(projected[1].status, Some("settlement_updated"));
}

#[test]
fn foreign_source_reply_and_history_gap_are_explicit_refusals() {
    let id = conversation();
    let foreign = ConversationId::new("00000000-0000-4000-8000-000000000002").unwrap();
    let foreign_event = CommittedEvent::gap(cursor(&foreign, 0));
    assert!(matches!(
        project_events(&id, &[foreign_event]),
        Err(ConversationEventError::Events(
            EventReadError::IdentityChanged
        ))
    ));
    let gap = CommittedEvent::gap(cursor(&id, 0));
    assert!(matches!(
        project_events(&id, &[gap]),
        Err(ConversationEventError::Gap)
    ));
}
