//! Minimum retention compatible with observations, without inventing answers.
//! The disposable controller owns sparse tools, seen IDs and admission budgets.
//! Request-point intervals derive the retrospective cost proved by cancellations.
mod intervals;
use crate::application::agent_execution::{
    agents::AgentError,
    executions::{
        ExecutionController, ExecutionEvent, ExecutionUpdate, HistoricalReviewCost,
        RetentionControllerUndo,
    },
};
use crate::domain::agent_execution::{
    executions::ExecutionId, permissions::PermissionId, sessions::ExecutionSessionId,
};
use intervals::{IntervalUndo, Intervals};
use std::{collections::HashMap, mem::size_of};

#[derive(Clone, Copy)]
struct RequestPoint {
    index: usize,
    cost: HistoricalReviewCost,
}
pub(super) struct Witness {
    controller: ExecutionController,
    intervals: Intervals,
    requests: HashMap<PermissionId, RequestPoint>,
    identity_bytes: usize,
}
pub(super) struct WitnessUndo {
    controller: Option<RetentionControllerUndo>,
    interval: Option<IntervalUndo>,
    inserted: Option<PermissionId>,
}
impl Witness {
    pub(super) fn new(
        context: ExecutionSessionId,
        execution: ExecutionId,
    ) -> Result<Self, AgentError> {
        let mut controller = ExecutionController::new(context);
        controller.begin_execution(execution)?;
        Ok(Self {
            controller,
            intervals: Intervals::default(),
            requests: HashMap::new(),
            identity_bytes: 0,
        })
    }
    pub(super) fn observe(&mut self, event: &ExecutionEvent) -> Result<WitnessUndo, AgentError> {
        let mut undo = WitnessUndo {
            controller: None,
            interval: None,
            inserted: None,
        };
        match event.update() {
            ExecutionUpdate::Tool(update) => {
                self.controller
                    .validate_tool_retention(event.execution_id(), update)?;
                undo.controller = Some(
                    self.controller
                        .retain_historical_tool(event.execution_id(), update.clone())?,
                );
            }
            ExecutionUpdate::PermissionRequested {
                id,
                tool_id,
                observation,
                input,
                options,
            } => {
                ExecutionController::validate_tool_payload(observation.payload_bytes())?;
                let update = observation.as_update(tool_id.clone());
                let (cost, controller) = self.controller.retain_historical_review(
                    event.execution_id(),
                    id.clone(),
                    update,
                    input,
                    options,
                )?;
                let (index, interval) = match self.intervals.append(cost.bytes()) {
                    Ok(value) => value,
                    Err(error) => {
                        self.controller.restore_historical_retention(controller);
                        return Err(error);
                    }
                };
                self.identity_bytes = self.identity_bytes.saturating_add(id.as_str().len());
                self.requests
                    .insert(id.clone(), RequestPoint { index, cost });
                undo.controller = Some(controller);
                undo.interval = Some(interval);
                undo.inserted = Some(id.clone());
            }
            ExecutionUpdate::PermissionCancelled(record) => {
                if let Some(point) = self.requests.get(record.request().id()) {
                    undo.interval = Some(
                        self.intervals
                            .retain_until_now(point.index, point.cost.bytes())?,
                    );
                }
            }
            // This hypothetical execution stays open through terminal/cancellation
            // evidence. The same InvocationHistory owner checks actual ordering.
            _ => {}
        }
        Ok(undo)
    }
    pub(super) fn restore(&mut self, undo: WitnessUndo) {
        if let Some(id) = undo.inserted {
            self.requests.remove(&id);
            self.identity_bytes -= id.as_str().len();
        }
        if let Some(interval) = undo.interval {
            self.intervals.restore(interval);
        }
        if let Some(controller) = undo.controller {
            self.controller.restore_historical_retention(controller);
        }
    }
    pub(super) fn retained_bytes(&self) -> usize {
        self.intervals
            .retained_bytes()
            .saturating_add(
                self.requests
                    .capacity()
                    .saturating_mul(size_of::<(PermissionId, RequestPoint)>() + size_of::<usize>()),
            )
            .saturating_add(self.identity_bytes)
            .saturating_add(self.controller.historical_retained_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::agent_execution::{
        permissions::{CancellationOrigin, PermissionCancellation},
        tools::ToolReviewInput,
    };
    use crate::domain::agent_execution::{
        permissions::{
            PermissionCancellationReason, PermissionDecision, PermissionEffect,
            PermissionOfferPolicy, PermissionOption, PermissionOptionId, PermissionOptions,
            PermissionRequest, PermissionScope,
        },
        sessions::ExecutionSession,
        tools::{ToolCallId, ToolCallUpdate, ToolObservation},
    };
    fn execution() -> ExecutionId {
        ExecutionId::new("execution").unwrap()
    }
    fn context() -> ExecutionSessionId {
        ExecutionSessionId::new("context").unwrap()
    }
    fn request(id: &str, bytes: usize) -> ExecutionEvent {
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
        ExecutionEvent::new(
            execution(),
            ExecutionUpdate::PermissionRequested {
                id: PermissionId::new(id).unwrap(),
                tool_id: ToolCallId::new(id).unwrap(),
                observation: ToolObservation::default(),
                input: ToolReviewInput {
                    name: "tool".into(),
                    arguments_json: "x".repeat(bytes),
                },
                options,
            },
        )
    }
    fn cancellation(event: &ExecutionEvent) -> ExecutionEvent {
        let ExecutionUpdate::PermissionRequested {
            id,
            tool_id,
            input,
            options,
            ..
        } = event.update()
        else {
            panic!("request fixture")
        };
        let mut session = ExecutionSession::new(context());
        session.begin_execution(execution()).unwrap();
        session
            .observe_tool(
                &execution(),
                ToolCallUpdate::new(tool_id.clone(), None, None, None, None, None),
            )
            .unwrap();
        session
            .request_permission(PermissionRequest::new(
                id.clone(),
                execution(),
                tool_id.clone(),
                options.clone(),
            ))
            .unwrap();
        let cancelled = session
            .cancel_permission(
                &execution(),
                id,
                PermissionCancellationReason::provider_withdrawal(),
            )
            .unwrap()
            .unwrap();
        ExecutionEvent::new(
            execution(),
            ExecutionUpdate::PermissionCancelled(
                PermissionCancellation::from_record(
                    context(),
                    cancelled,
                    input.clone(),
                    CancellationOrigin::Provider,
                )
                .unwrap(),
            ),
        )
    }
    #[test]
    fn controller_weights_preserve_retroactive_cancellation_and_answered_witness() {
        let earlier = request("earlier", 17 * 1024 * 1024);
        let later = request("later", 17 * 1024 * 1024);
        let close_earlier = cancellation(&earlier);
        let close_later = cancellation(&later);
        let mut witness = Witness::new(context(), execution()).unwrap();
        witness.observe(&earlier).unwrap();
        witness.observe(&later).unwrap();
        let undo = witness.observe(&close_later).unwrap();
        assert!(witness.observe(&close_earlier).is_err());
        // Refusal leaves the earlier hypothetical answer feasible; a dropped
        // transaction can also restore the accepted later cancellation.
        witness.restore(undo);
        witness.observe(&close_later).unwrap();
        let mut nonoverlapping = Witness::new(context(), execution()).unwrap();
        nonoverlapping.observe(&earlier).unwrap();
        nonoverlapping.observe(&close_earlier).unwrap();
        nonoverlapping.observe(&later).unwrap();
        nonoverlapping.observe(&close_later).unwrap();
        let mut earlier_first = Witness::new(context(), execution()).unwrap();
        earlier_first.observe(&earlier).unwrap();
        earlier_first.observe(&later).unwrap();
        assert!(earlier_first.observe(&close_earlier).is_err());
        let a = request("a", 15 * 1024 * 1024);
        let b = request("b", 15 * 1024 * 1024);
        let mut fitting = Witness::new(context(), execution()).unwrap();
        fitting.observe(&a).unwrap();
        fitting.observe(&b).unwrap();
        fitting.observe(&cancellation(&b)).unwrap();
        fitting.observe(&cancellation(&a)).unwrap();
    }
}
