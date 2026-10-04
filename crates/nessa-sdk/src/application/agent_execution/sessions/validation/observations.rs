//! Incremental relationship checks shared by full admission and semantic suffixes.
use super::{corrupt, validate_observation_context};
use crate::application::agent_execution::{
    executions::{limits::validate_observation_id, ExecutionEvent, ExecutionUpdate},
    sessions::{app_sources, InvocationRecord, ProviderContext, StorageError},
};
use crate::domain::agent_execution::{
    permissions::{
        PermissionId, PermissionRequest, PermissionStateView, ReviewDeclineId, ReviewDeclineStage,
    },
    questions::QuestionId,
    tools::ToolCallId,
};
use std::{
    collections::{HashMap, HashSet},
    mem::size_of,
};

#[derive(Default)]
pub(super) struct Observations {
    reviews: HashMap<PermissionId, usize>,
    review_ids: HashSet<PermissionId>,
    declines: HashMap<ReviewDeclineId, usize>,
    asked_ids: HashSet<QuestionId>,
    open_questions: HashSet<QuestionId>,
    // Each MCP tool call's first observation naming its MCP identity, so an
    // app a later message names is found without scanning the invocation.
    mcp_tool_calls: HashMap<ToolCallId, usize>,
    identity_bytes: usize,
}
pub(super) enum ObservationUndo {
    None,
    Requested(PermissionId),
    Cancelled(PermissionId, usize),
    Declined(ReviewDeclineId, Option<usize>),
    Asked(QuestionId),
    Closed(QuestionId),
    McpToolCall(ToolCallId),
}
impl Observations {
    pub(super) fn observe(
        &mut self,
        context: &ProviderContext,
        record: &InvocationRecord,
        index: usize,
        event: &ExecutionEvent,
    ) -> Result<ObservationUndo, StorageError> {
        validate_observation_context(context, event)?;
        let undo = match event.update() {
            ExecutionUpdate::Tool(tool) => {
                validate_observation_id(tool.id().as_str()).map_err(corrupt)?;
                match app_sources::mcp_tool_call(event.update()) {
                    Some((id, _)) if !self.mcp_tool_calls.contains_key(id) => {
                        self.mcp_tool_calls.insert(id.clone(), index);
                        self.identity_bytes = self.identity_bytes.saturating_add(id.as_str().len());
                        ObservationUndo::McpToolCall(id.clone())
                    }
                    _ => ObservationUndo::None,
                }
            }
            ExecutionUpdate::PermissionRequested { id, tool_id, .. } => {
                validate_observation_id(id.as_str()).map_err(corrupt)?;
                validate_observation_id(tool_id.as_str()).map_err(corrupt)?;
                if self.review_ids.contains(id) {
                    return Err(corrupt(
                        "permission identity is repeated within an invocation",
                    ));
                }
                self.review_ids.insert(id.clone());
                self.reviews.insert(id.clone(), index);
                self.identity_bytes = self.identity_bytes.saturating_add(2 * id.as_str().len());
                ObservationUndo::Requested(id.clone())
            }
            ExecutionUpdate::PermissionCancelled(cancellation) => {
                let id = cancellation.request().id();
                let &previous = self
                    .reviews
                    .get(id)
                    .ok_or_else(|| corrupt("cancellation has no preceding pending request"))?;
                let ExecutionUpdate::PermissionRequested {
                    id,
                    tool_id,
                    options,
                    input,
                    ..
                } = record.events[previous].update()
                else {
                    unreachable!("review correlation index")
                };
                let mut pending = PermissionRequest::new(
                    id.clone(),
                    event.execution_id().clone(),
                    tool_id.clone(),
                    options.clone(),
                );
                let PermissionStateView::Cancelled { reason } = cancellation.request().state()
                else {
                    return Err(corrupt("cancellation request is not cancelled"));
                };
                pending.cancel(reason.clone()).map_err(corrupt)?;
                if &pending != cancellation.request() || input != cancellation.input() {
                    return Err(corrupt(
                        "cancellation differs from the original permission request",
                    ));
                }
                let (id, previous) = self
                    .reviews
                    .remove_entry(id)
                    .expect("validated prior request");
                self.identity_bytes -= id.as_str().len();
                ObservationUndo::Cancelled(id, previous)
            }
            ExecutionUpdate::ReviewDeclined(observation) => {
                let previous = self.declines.get(observation.id()).copied();
                match previous {
                    None if observation.stage() == ReviewDeclineStage::Selected => {}
                    Some(previous) => {
                        let ExecutionUpdate::ReviewDeclined(selected) =
                            record.events[previous].update()
                        else {
                            unreachable!("decline correlation index")
                        };
                        if selected.advance(observation.stage()).as_ref() != Ok(observation) {
                            return Err(corrupt(
                                "declined review identity is repeated or changes its decision",
                            ));
                        }
                    }
                    None => {
                        return Err(corrupt(
                            "declined review delivery has no preceding local selection",
                        ))
                    }
                }
                self.declines.insert(observation.id().clone(), index);
                if previous.is_none() {
                    self.identity_bytes = self
                        .identity_bytes
                        .saturating_add(observation.id().as_str().len());
                }
                ObservationUndo::Declined(observation.id().clone(), previous)
            }
            ExecutionUpdate::QuestionAsked { id, .. } => {
                validate_observation_id(id.as_str()).map_err(corrupt)?;
                if self.asked_ids.contains(id) {
                    return Err(corrupt(
                        "question identity is repeated within an invocation",
                    ));
                }
                self.asked_ids.insert(id.clone());
                self.open_questions.insert(id.clone());
                self.identity_bytes = self.identity_bytes.saturating_add(2 * id.as_str().len());
                ObservationUndo::Asked(id.clone())
            }
            ExecutionUpdate::QuestionClosed { id } => {
                validate_observation_id(id.as_str()).map_err(corrupt)?;
                let id = self
                    .open_questions
                    .take(id)
                    .ok_or_else(|| corrupt("question closure has no preceding open question"))?;
                self.identity_bytes -= id.as_str().len();
                ObservationUndo::Closed(id)
            }
            _ => ObservationUndo::None,
        };
        Ok(undo)
    }
    pub(super) fn restore(&mut self, undo: ObservationUndo) {
        match undo {
            ObservationUndo::None => {}
            ObservationUndo::Requested(id) => {
                self.reviews.remove(&id);
                self.review_ids.remove(&id);
                self.identity_bytes -= 2 * id.as_str().len();
            }
            ObservationUndo::Cancelled(id, index) => {
                self.identity_bytes += id.as_str().len();
                self.reviews.insert(id, index);
            }
            ObservationUndo::Declined(id, previous) => {
                if let Some(index) = previous {
                    self.declines.insert(id, index);
                } else {
                    self.declines.remove(&id);
                    self.identity_bytes -= id.as_str().len();
                }
            }
            ObservationUndo::Asked(id) => {
                self.asked_ids.remove(&id);
                self.open_questions.remove(&id);
                self.identity_bytes -= 2 * id.as_str().len();
            }
            ObservationUndo::Closed(id) => {
                self.identity_bytes += id.as_str().len();
                self.open_questions.insert(id);
            }
            ObservationUndo::McpToolCall(id) => {
                self.mcp_tool_calls.remove(&id);
                self.identity_bytes -= id.as_str().len();
            }
        }
    }
    /// The event of `record` (the invocation these observations are of) that
    /// first named `tool_id`'s MCP identity.
    pub(super) fn mcp_tool_call(&self, tool_id: &ToolCallId) -> Option<usize> {
        self.mcp_tool_calls.get(tool_id).copied()
    }
    pub(super) fn retained_bytes(&self) -> usize {
        self.identity_bytes
            .saturating_add(
                self.reviews
                    .capacity()
                    .saturating_mul(size_of::<(PermissionId, usize)>() + 16),
            )
            .saturating_add(
                self.review_ids
                    .capacity()
                    .saturating_mul(size_of::<PermissionId>() + 16),
            )
            .saturating_add(
                self.declines
                    .capacity()
                    .saturating_mul(size_of::<(ReviewDeclineId, usize)>() + 16),
            )
            .saturating_add(
                self.asked_ids
                    .capacity()
                    .saturating_mul(size_of::<QuestionId>() + 16),
            )
            .saturating_add(
                self.open_questions
                    .capacity()
                    .saturating_mul(size_of::<QuestionId>() + 16),
            )
            .saturating_add(
                self.mcp_tool_calls
                    .capacity()
                    .saturating_mul(size_of::<(ToolCallId, usize)>() + 16),
            )
    }
}
