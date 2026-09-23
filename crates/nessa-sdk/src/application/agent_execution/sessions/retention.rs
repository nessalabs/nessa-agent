//! Check the minimum live retention consistent with saved observations.
//!
//! Successful answers have a separate audit trail and are absent from snapshots.
//! Only a later cancellation proves that a requested review stayed pending.
//! Other reviews may have been answered immediately. A disposable controller
//! reuses live admission, sparse tool merging, and seen-identity accounting; its
//! hypothetical releases never become session evidence or provider actions.
use super::{InvocationRecord, SessionSnapshot};
use crate::application::agent_execution::{
    agents::AgentError,
    executions::{ExecutionController, ExecutionUpdate},
};
use crate::domain::agent_execution::tools::ToolCallUpdate;
use std::collections::{HashMap, HashSet};

pub(super) fn validate(
    snapshot: &SessionSnapshot,
    invocation: &InvocationRecord,
) -> Result<(), AgentError> {
    if invocation.events.is_empty() {
        return Ok(());
    }
    let cancelled: HashSet<_> = invocation
        .events
        .iter()
        .filter_map(|event| match event.update() {
            ExecutionUpdate::PermissionCancelled(record) => Some(record.request().id()),
            _ => None,
        })
        .collect();
    let mut controller = ExecutionController::new(
        snapshot
            .provider_context
            .recorded()
            .expect("validated provider observations require context")
            .clone(),
    );
    controller.begin_execution(invocation.request.execution_id.clone())?;
    let mut retained = HashMap::new();
    for event in &invocation.events {
        match event.update() {
            ExecutionUpdate::Tool(update) => {
                controller.validate_tool_retention(&invocation.request.execution_id, update)?;
                controller.tool_event(&invocation.request.execution_id, update.clone())?;
            }
            ExecutionUpdate::PermissionRequested {
                id,
                tool_id,
                observation,
                input,
                options,
            } => {
                controller.validate_review_retention(
                    &invocation.request.execution_id,
                    id,
                    tool_id,
                    input,
                    options,
                )?;
                ExecutionController::validate_tool_payload(observation.payload_bytes())?;
                let update = ToolCallUpdate::new(
                    tool_id.clone(),
                    observation.title().clone(),
                    *observation.kind(),
                    *observation.status(),
                    observation.locations().clone(),
                    observation.content().clone(),
                );
                controller.request_permission(
                    &invocation.request.execution_id,
                    id.clone(),
                    update,
                    input.clone(),
                    options.clone(),
                )?;
                // Validated options are nonempty. The choice is only a witness
                // that release was possible, never a claimed historical answer.
                let option = options.choices()[0].id();
                if cancelled.contains(id) {
                    retained.insert(id, option);
                } else {
                    controller.release_review_for_retention_validation(
                        &invocation.request.execution_id,
                        id,
                        option,
                    )?;
                }
            }
            ExecutionUpdate::PermissionCancelled(record) => {
                if let Some(option) = retained.remove(record.request().id()) {
                    controller.release_review_for_retention_validation(
                        &invocation.request.execution_id,
                        record.request().id(),
                        option,
                    )?;
                }
            }
            // Keep this disposable execution alive through trailing cancellation
            // evidence. The evidence validator separately checks terminal order.
            _ => {}
        }
    }
    Ok(())
}
