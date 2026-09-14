pub(super) use nessa_sdk::domain::agent_execution::{
    executions::*, permissions::*, prompts::*, tools::*, ExecutionError,
};

pub(super) fn update(id: &str) -> ToolCallUpdate {
    ToolCallUpdate::new(ToolCallId::new(id).unwrap(), None, None, None, None, None)
}

pub(super) fn choice(id: &str, decision: PermissionDecision) -> PermissionOption {
    PermissionOption::new(
        PermissionOptionId::new(id).unwrap(),
        format!("Choice {id}"),
        decision,
    )
    .unwrap()
}
