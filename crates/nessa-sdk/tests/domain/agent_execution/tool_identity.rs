//! Internal identity guards complement public session-boundary tests.
use super::ToolCall;
use crate::domain::agent_execution::{
    executions::ExecutionId,
    tools::{ToolCallId, ToolCallUpdate},
    ExecutionError,
};

#[test]
fn rejected_internal_updates_preserve_tool_identity_and_observation() {
    let execution = ExecutionId::new("execution").unwrap();
    let id = ToolCallId::new("tool").unwrap();
    let update = |id| ToolCallUpdate::new(id, Some("retained".into()), None, None, None, None);
    let mut tool = ToolCall::new(execution.clone(), update(id.clone()));
    let before = tool.observation().clone();
    assert_eq!(
        tool.apply(&ExecutionId::new("other").unwrap(), update(id.clone())),
        Err(ExecutionError::DifferentExecution)
    );
    assert_eq!(
        tool.apply(&execution, update(ToolCallId::new("other").unwrap())),
        Err(ExecutionError::DifferentTool)
    );
    assert_eq!(tool.id(), &id);
    assert_eq!(tool.execution_id(), &execution);
    assert_eq!(tool.observation(), &before);
    let replacement = ToolCallUpdate::new(id, Some("replacement".into()), None, None, None, None);
    let predicted = tool.payload_bytes_after(&replacement);
    tool.apply(&execution, replacement).unwrap();
    assert_eq!(tool.observation().title().as_deref(), Some("replacement"));
    assert_eq!(tool.payload_bytes(), predicted);
    assert_eq!(
        ToolCall::initial_payload_bytes(&execution, &update(tool.id().clone())),
        execution.as_str().len() + tool.id().as_str().len() + "retained".len()
    );
}
