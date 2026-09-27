//! Adapter coordination for validated execution state and attributed evidence.
#![deny(missing_docs)]

use super::{
    limits::{validate_message_chunk, validate_observation_id},
    ExecutionAuditRecord, ExecutionEvent, ExecutionUpdate, SessionClosureRecord,
};
use crate::application::agent_execution::agents::AgentError;
use crate::application::agent_execution::permissions::{
    CancellationOrigin, PermissionAnswer, PermissionCancellation, PermissionCancellationRequest,
    PermissionResolution,
};
use crate::application::agent_execution::tools::ToolReviewInput;
use crate::domain::agent_execution::questions::{AgentQuestion, QuestionId};
use crate::domain::agent_execution::{
    executions::*,
    permissions::*,
    sessions::{ExecutionSession, ExecutionSessionId},
    tools::*,
    ExecutionError,
};
use std::collections::HashMap;

const MAX_TOOLS: usize = 4096;
const MAX_PERMISSIONS: usize = 128;
const MAX_EXECUTION_PERMISSIONS: usize = 4096;
const MAX_RETAINED_BYTES: usize = 32 * 1024 * 1024;

/// Ephemeral execution coordination shared by session adapters. The aggregate
/// owns lifecycle decisions; this controller pairs review input with permissions
/// and projects accepted observations. It owns no transport or durable history.
#[derive(Debug)]
pub struct ExecutionController {
    session: ExecutionSession,
    review_inputs: HashMap<PermissionId, (ToolReviewInput, usize)>,
    retained_tool_bytes: usize,
    retained_review_bytes: usize,
}
impl ExecutionController {
    /// Creates an idle live aggregate for provider context `id`, with empty tool
    /// and permission observations. This does not open a provider or load history.
    pub fn new(id: ExecutionSessionId) -> Self {
        Self {
            session: ExecutionSession::new(id),
            review_inputs: HashMap::new(),
            retained_tool_bytes: 0,
            retained_review_bytes: 0,
        }
    }
    /// Whether this live attachment has already recorded its permanent closure.
    /// Adapters may skip repeated teardown recording; this does not confirm resource cleanup.
    pub fn is_closed(&self) -> bool {
        self.session.is_closed()
    }

    /// Identity of the provider context owned by this live aggregate.
    pub fn id(&self) -> &ExecutionSessionId {
        self.session.id()
    }
    /// Execution still owned by the aggregate, absent after settlement or while idle.
    /// Adapters use this authoritative correlation when selecting lifecycle causes.
    pub fn active_execution_id(&self) -> Option<&ExecutionId> {
        self.session.active_execution()
    }
    /// Admits `id` as the sole active execution, retaining it until settlement.
    /// Returns Busy for an active run, Closed for a retired aggregate, or
    /// InvalidInput when the identity was already admitted by this live attachment. Identity history grows linearly until aggregate drop;
    /// normal settlement releases observations but never makes an ID reusable.
    pub fn begin_execution(&mut self, id: ExecutionId) -> Result<(), AgentError> {
        self.session.begin_execution(id).map_err(domain_error)?;
        self.retained_tool_bytes = 0;
        Ok(())
    }
    fn event(
        &self,
        execution: &ExecutionId,
        update: ExecutionUpdate,
    ) -> Result<ExecutionEvent, AgentError> {
        self.check_execution(execution)?;
        Ok(ExecutionEvent::new(execution.clone(), update))
    }
    /// Correlates observed `message` with the current execution without I/O.
    /// Supply the captured `execution` ID; a stale or inactive target returns an error.
    /// Returns Closed after session closure, or a protocol error without an active execution
    /// or when the chunk exceeds [`ExecutionEvent::MAX_MESSAGE_CHUNK_BYTES`].
    pub fn message_event(
        &self,
        execution: &ExecutionId,
        message: MessageChunk,
    ) -> Result<ExecutionEvent, AgentError> {
        validate_message_chunk(&message)?;
        if self.session.is_closed() {
            return Err(AgentError::Closed);
        }
        self.event(execution, ExecutionUpdate::Message(message))
    }
    /// Correlates the adapter-established `outcome` with the active execution.
    /// Supply the captured `execution` ID; a stale or inactive target returns an error.
    /// This constructs a projection without settling the aggregate or publishing
    /// evidence. Returns a protocol error when no execution is active.
    pub fn finished_event(
        &self,
        execution: &ExecutionId,
        outcome: ExecutionOutcome,
    ) -> Result<ExecutionEvent, AgentError> {
        self.event(execution, ExecutionUpdate::Finished(outcome))
    }
    /// Applies sparse `update` to the active execution's tool and creates its event.
    /// Supply the captured `execution` ID; a stale or inactive target returns an error.
    /// Returns a protocol error for an invalid domain transition or retention limit,
    /// and InvalidInput for an oversized tool identity. No event is published here.
    pub fn tool_event(
        &mut self,
        execution: &ExecutionId,
        update: ToolCallUpdate,
    ) -> Result<ExecutionEvent, AgentError> {
        self.validate_tool_retention(execution, &update)?;
        self.observe_tool(execution, update.clone())?;
        self.event(execution, ExecutionUpdate::Tool(update))
    }
    /// Emit one question the agent is asking, correlated to `execution`.
    ///
    /// Not a review, so nothing is admitted to the session aggregate: an ask
    /// authorises nothing and holds no lifetime there. What it does share with
    /// a review is its retention budget, which is checked before the event is
    /// built rather than after.
    pub fn ask_question(
        &mut self,
        execution: &ExecutionId,
        id: QuestionId,
        question: AgentQuestion,
    ) -> Result<ExecutionEvent, AgentError> {
        let event = ExecutionEvent::new(
            execution.clone(),
            ExecutionUpdate::QuestionAsked { id, question },
        );
        event.validate_payload_size()?;
        Ok(event)
    }
    /// Emit that one ask has stopped waiting, answered or withdrawn.
    pub fn close_question(
        &self,
        execution: &ExecutionId,
        id: QuestionId,
    ) -> Result<ExecutionEvent, AgentError> {
        let event = ExecutionEvent::new(execution.clone(), ExecutionUpdate::QuestionClosed { id });
        event.validate_payload_size()?;
        Ok(event)
    }
    /// Admits review `id` for the observed `tool`, retaining its exact `input` and
    /// offered `options` until resolution. Produces the correlated review event.
    /// Supply the captured `execution` ID; a stale or inactive target returns an error.
    /// Rejects stale/duplicate identity, missing execution, or retention limits
    /// before admitting the review. Performs no provider response or audit I/O.
    pub fn request_permission(
        &mut self,
        execution: &ExecutionId,
        id: PermissionId,
        tool: ToolCallUpdate,
        input: ToolReviewInput,
        options: PermissionOptions,
    ) -> Result<ExecutionEvent, AgentError> {
        let (charged_bytes, review_bytes) =
            self.validate_review_retention(execution, &id, tool.id(), &input, &options)?;
        let tool_id = tool.id().clone();
        let request = PermissionRequest::new(
            id.clone(),
            execution.clone(),
            tool_id.clone(),
            options.clone(),
        );
        self.session
            .validate_permission_admission(&request)
            .map_err(domain_error)?;
        self.observe_tool(execution, tool)?;
        self.session
            .request_permission(request)
            .map_err(domain_error)?;
        let observation = self
            .session
            .tool(&tool_id)
            .expect("accepted observed tool")
            .observation()
            .clone();
        self.review_inputs
            .insert(id.clone(), (input.clone(), charged_bytes));
        self.retained_review_bytes = review_bytes;
        self.event(
            execution,
            ExecutionUpdate::PermissionRequested {
                id,
                tool_id,
                observation,
                input,
                options,
            },
        )
    }
    pub(crate) fn validate_review_retention(
        &self,
        execution: &ExecutionId,
        id: &PermissionId,
        tool: &ToolCallId,
        input: &ToolReviewInput,
        options: &PermissionOptions,
    ) -> Result<(usize, usize), AgentError> {
        self.check_execution(execution)?;
        validate_observation_id(id.as_str())?;
        if self.session.permission_count() >= MAX_EXECUTION_PERMISSIONS {
            return Err(AgentError::Protocol(
                "execution permission limit exceeded".into(),
            ));
        }
        if self.review_inputs.len() >= MAX_PERMISSIONS {
            return Err(AgentError::Protocol(
                "pending permission limit exceeded".into(),
            ));
        }
        let charged_bytes = Self::validate_review_payload(execution, id, tool, input, options)?;
        let review_bytes = self.retained_review_bytes.saturating_add(charged_bytes);
        Self::validate_review_bytes(review_bytes)?;
        Ok((charged_bytes, review_bytes))
    }
    pub(crate) fn validate_review_payload(
        execution: &ExecutionId,
        id: &PermissionId,
        tool: &ToolCallId,
        input: &ToolReviewInput,
        options: &PermissionOptions,
    ) -> Result<usize, AgentError> {
        validate_observation_id(id.as_str())?;
        validate_observation_id(tool.as_str())?;
        let bytes = input_bytes(input)
            .saturating_add(options.payload_bytes())
            .saturating_add(review_identity_bytes(execution, id, tool));
        Self::validate_review_bytes(bytes)?;
        Ok(bytes)
    }
    fn validate_review_bytes(bytes: usize) -> Result<(), AgentError> {
        if bytes > MAX_RETAINED_BYTES {
            return Err(AgentError::Protocol(
                "permission review retention limit exceeded".into(),
            ));
        }
        Ok(())
    }
    pub(crate) fn validate_tool_payload(bytes: usize) -> Result<(), AgentError> {
        if bytes > MAX_RETAINED_BYTES {
            return Err(AgentError::Protocol("tool retention limit exceeded".into()));
        }
        Ok(())
    }
    /// Resolves `answer` once against its execution, review, and offered option.
    /// Returns StalePermission if it cannot match a pending review. The resolution
    /// retains this controller’s session, original input and attribution; the adapter must audit selection
    /// before attempting delivery and audit that delivery independently of waiters.
    pub fn answer_permission(
        &mut self,
        answer: PermissionAnswer,
    ) -> Result<PermissionResolution, AgentError> {
        let request = self
            .session
            .answer_permission(&answer.execution_id, &answer.id, &answer.option_id)
            .map_err(|_| AgentError::StalePermission)?;
        let (input, charged_bytes) = self
            .review_inputs
            .remove(&answer.id)
            .expect("controller retains the accepted review input");
        self.retained_review_bytes -= charged_bytes;
        PermissionResolution::new(self.id().clone(), request, input, answer.attribution)
    }
    // Only the snapshot retention validator uses this on a disposable controller.
    // Answers are absent from observations: release the hypothetical pending
    // payload without manufacturing attribution or durable resolution evidence.
    pub(crate) fn release_review_for_retention_validation(
        &mut self,
        execution: &ExecutionId,
        id: &PermissionId,
        option: &PermissionOptionId,
    ) -> Result<(), AgentError> {
        self.session
            .answer_permission(execution, id, option)
            .map_err(domain_error)?;
        let (_, charged_bytes) = self
            .review_inputs
            .remove(id)
            .expect("accepted review input");
        self.retained_review_bytes -= charged_bytes;
        Ok(())
    }
    /// Resolves explicit cancellation `input`, retaining its actor and exact reason.
    /// Returns InvalidInput for a non-Custom caller cause, or Closed/StalePermission
    /// when the live review cannot be matched. Rejection leaves pending state intact.
    /// The caller must deliver returned evidence to the mandatory audit port.
    pub fn cancel_review(
        &mut self,
        input: PermissionCancellationRequest,
    ) -> Result<PermissionCancellation, AgentError> {
        if !matches!(
            input.reason.view(),
            PermissionCancellationReasonView::Custom(_)
        ) {
            return Err(AgentError::InvalidInput(
                "explicit permission cancellation requires a custom caller reason".into(),
            ));
        }
        if self.session.is_closed() {
            return Err(AgentError::Closed);
        }
        if self.session.active_execution() != Some(&input.execution_id) {
            return Err(AgentError::StalePermission);
        }
        self.cancel_permission(
            &input.execution_id,
            &input.id,
            input.reason,
            CancellationOrigin::Client(input.actor),
        )?
        .ok_or(AgentError::StalePermission)
    }
    /// Cancels pending review `id` in the active execution for `reason` and `origin`.
    /// Supply the captured `execution` ID; a stale or inactive target returns an error.
    /// Returns InvalidInput for a contradictory cause/origin without changing state.
    /// Returns None if the live review is absent; the returned evidence includes
    /// the original review input. The adapter owns audit and provider effects.
    #[must_use = "record cancellation evidence before discarding it"]
    pub fn cancel_permission(
        &mut self,
        execution: &ExecutionId,
        id: &PermissionId,
        reason: PermissionCancellationReason,
        origin: CancellationOrigin,
    ) -> Result<Option<PermissionCancellation>, AgentError> {
        origin.validate_reason(&reason)?;
        let request = self
            .session
            .cancel_permission(execution, id, reason)
            .map_err(domain_error)?;
        Ok(request.map(|request| self.cancellation(request, origin)))
    }
    fn cancellation(
        &mut self,
        request: PermissionRequest,
        origin: CancellationOrigin,
    ) -> PermissionCancellation {
        let (input, charged_bytes) = self
            .review_inputs
            .remove(request.id())
            .expect("accepted review input");
        self.retained_review_bytes -= charged_bytes;
        PermissionCancellation::new(self.id().clone(), request, input, origin)
    }
    /// Cancels pending reviews in `execution` with the lifecycle `reason` and `origin`.
    /// A contradictory cause/origin returns InvalidInput. A different or inactive
    /// execution also returns an error. Rejection leaves retained state unchanged.
    /// Retains resolved identities until execution settlement to prevent replay.
    /// Returns all mandatory evidence; this performs no audit or provider I/O.
    #[must_use = "record every cancellation, including lifecycle cleanup"]
    pub fn cancel_permissions(
        &mut self,
        execution: &ExecutionId,
        reason: PermissionCancellationReason,
        origin: CancellationOrigin,
    ) -> Result<Vec<PermissionCancellation>, AgentError> {
        origin.validate_reason(&reason)?;
        Ok(self
            .session
            .cancel_permissions(execution, reason)
            .map_err(domain_error)?
            .into_iter()
            .map(|request| self.cancellation(request, origin.clone()))
            .collect())
    }
    /// Closes this live aggregate and returns mandatory lifecycle audit evidence.
    /// `reason` identifies the lifecycle cause; `origin` retains verified caller
    /// attribution or the automatic initiator. The first close returns a session
    /// record even when idle, followed by every pending permission cancellation.
    /// Execution-owned callbacks use `close_execution` with their captured ID;
    /// this session-wide method rejects `ExecutionFailed`.
    /// Repeated close returns no records. Invalid cause/origin pairs return InvalidInput
    /// without changing state. This performs no provider/process I/O.
    #[must_use = "record every closure and cancellation before reporting shutdown"]
    pub fn close(
        &mut self,
        reason: PermissionCancellationReason,
        origin: CancellationOrigin,
    ) -> Result<Vec<ExecutionAuditRecord>, AgentError> {
        self.close_target(None, reason, origin)
    }

    /// Close only the attachment still running the captured `execution`.
    /// `reason` describes why that execution requires closure; `origin` retains
    /// caller attribution or its automatic initiator. A stale target or invalid
    /// cause returns an error before mutation. Retain all returned audit records.
    #[must_use = "record every closure and cancellation before reporting shutdown"]
    pub fn close_execution(
        &mut self,
        execution: &ExecutionId,
        reason: PermissionCancellationReason,
        origin: CancellationOrigin,
    ) -> Result<Vec<ExecutionAuditRecord>, AgentError> {
        self.close_target(Some(execution), reason, origin)
    }

    fn close_target(
        &mut self,
        execution: Option<&ExecutionId>,
        reason: PermissionCancellationReason,
        origin: CancellationOrigin,
    ) -> Result<Vec<ExecutionAuditRecord>, AgentError> {
        origin.validate_reason(&reason)?;
        let result = match execution {
            Some(execution) => self.session.close_execution(execution, reason),
            None => self.session.close(reason),
        };
        let (closure, permissions) = result.map_err(domain_error)?.into_parts();
        let mut records = Vec::new();
        if let Some(closure) = closure {
            records.push(ExecutionAuditRecord::SessionClosed(
                SessionClosureRecord::new(closure, origin.clone())?,
            ));
        }
        records.extend(permissions.into_iter().map(|request| {
            ExecutionAuditRecord::Cancelled(self.cancellation(request, origin.clone()))
        }));
        Ok(records)
    }
    /// Settles the matching `execution`, releasing retained tools and review identities.
    /// The terminal `result` is retained even without reviews. Any remaining permission
    /// is cancelled with its failure cause or ExecutionFinished and a runtime origin;
    /// every returned record must be audited. A stale or idle target returns an error
    /// before mutation. Supply the ID captured when the provider invocation began.
    /// This changes local state only; the adapter establishes external settlement.
    pub fn finish_execution(
        &mut self,
        execution: &ExecutionId,
        result: Result<ExecutionOutcome, PermissionCancellationReason>,
    ) -> Result<Vec<ExecutionAuditRecord>, AgentError> {
        let (finished, cancelled) = self
            .session
            .finish_execution(execution, result)
            .map_err(domain_error)?;
        let mut records = vec![ExecutionAuditRecord::Finished(finished)];
        records.extend(cancelled.into_iter().map(|request| {
            ExecutionAuditRecord::Cancelled(self.cancellation(request, CancellationOrigin::Runtime))
        }));
        self.retained_tool_bytes = 0;
        Ok(records)
    }
    fn check_execution(&self, execution: &ExecutionId) -> Result<(), AgentError> {
        if self.session.active_execution() != Some(execution) {
            return Err(AgentError::InvalidInput(
                "event targets a different or inactive execution".into(),
            ));
        }
        Ok(())
    }
    pub(crate) fn validate_tool_retention(
        &self,
        execution: &ExecutionId,
        update: &ToolCallUpdate,
    ) -> Result<usize, AgentError> {
        validate_observation_id(update.id().as_str())?;
        self.check_execution(execution)?;
        let previous = self.session.tool(update.id());
        if previous.is_none() && self.session.tool_count() >= MAX_TOOLS {
            return Err(AgentError::Protocol("tool count limit exceeded".into()));
        }
        let old_bytes = self.session.tool_payload_bytes(update.id());
        let new_bytes = self
            .session
            .tool_payload_bytes_after(execution, update)
            .map_err(domain_error)?;
        let total = self
            .retained_tool_bytes
            .saturating_sub(old_bytes)
            .saturating_add(new_bytes);
        Self::validate_tool_payload(total)?;
        Ok(total)
    }
    fn observe_tool(
        &mut self,
        execution: &ExecutionId,
        update: ToolCallUpdate,
    ) -> Result<(), AgentError> {
        let total = self.validate_tool_retention(execution, &update)?;
        self.session
            .observe_tool(execution, update)
            .map_err(domain_error)?;
        self.retained_tool_bytes = total;
        Ok(())
    }
}
// A pending request owns execution/permission/tool IDs. Both the aggregate's
// pending map and this controller's input map retain a separate permission key.
// The seen-ID history is retained after resolution and bounded separately.
fn review_identity_bytes(
    execution: &ExecutionId,
    permission: &PermissionId,
    tool: &ToolCallId,
) -> usize {
    execution
        .as_str()
        .len()
        .saturating_add(permission.as_str().len().saturating_mul(3))
        .saturating_add(tool.as_str().len())
}
fn input_bytes(input: &ToolReviewInput) -> usize {
    input
        .name
        .capacity()
        .saturating_add(input.arguments_json.capacity())
}
fn domain_error(error: ExecutionError) -> AgentError {
    match error {
        ExecutionError::SessionBusy => AgentError::Busy,
        ExecutionError::SessionClosed => AgentError::Closed,
        error @ (ExecutionError::InvalidExecutionFailureReason
        | ExecutionError::InvalidPermissionCancellationReason
        | ExecutionError::InvalidSessionClosureReason
        | ExecutionError::DuplicateExecution) => AgentError::InvalidInput(error.to_string()),
        error => AgentError::Protocol(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::agent_execution::permissions::{
        ActionContext, ApprovalAttribution, ApprovalBasis,
    };

    use crate::domain::agent_execution::permissions::CustomPermissionCancellationReason;

    #[test]
    fn cause_origin_validation_preserves_pending_state_and_retained_budgets() {
        let reasons = [
            (
                PermissionCancellationReason::provider_withdrawal(),
                [false, true, false],
            ),
            (
                PermissionCancellationReason::session_closed(),
                [true, false, true],
            ),
            (
                PermissionCancellationReason::session_failed(),
                [false, false, true],
            ),
            (
                PermissionCancellationReason::execution_finished(),
                [false, false, true],
            ),
            (
                PermissionCancellationReason::execution_failed(),
                [false, false, true],
            ),
            (
                PermissionCancellationReason::deadline_exceeded(),
                [false, false, true],
            ),
            (
                PermissionCancellationReason::event_consumer_dropped(),
                [false, false, true],
            ),
            (
                PermissionCancellationReason::session_handles_dropped(),
                [false, false, true],
            ),
            (
                PermissionCancellationReason::custom(
                    CustomPermissionCancellationReason::new("guard", "guard withdrew review")
                        .unwrap(),
                ),
                [true, false, false],
            ),
        ];
        let origins = [
            CancellationOrigin::Client(ActionContext::new("guard", "host", "withdraw").unwrap()),
            CancellationOrigin::Provider,
            CancellationOrigin::Runtime,
        ];
        for (reason, allowed) in reasons {
            for (origin, allowed) in origins.iter().zip(allowed) {
                for operation in 0..3 {
                    let mut controller = controller();
                    request(&mut controller, "permission", "original").unwrap();
                    let bytes = controller.retained_review_bytes;
                    let result = match operation {
                        0 => controller
                            .cancel_permission(
                                &ExecutionId::new("execution").unwrap(),
                                &PermissionId::new("permission").unwrap(),
                                reason.clone(),
                                origin.clone(),
                            )
                            .map(|record| assert!(record.is_some())),
                        1 => controller
                            .cancel_permissions(
                                &ExecutionId::new("execution").unwrap(),
                                reason.clone(),
                                origin.clone(),
                            )
                            .map(|records| assert_eq!(records.len(), 1)),
                        _ => controller
                            .close_execution(
                                &ExecutionId::new("execution").unwrap(),
                                reason.clone(),
                                origin.clone(),
                            )
                            .map(|records| assert_eq!(records.len(), 2)),
                    };
                    let closure_cause = !matches!(
                        reason.view(),
                        PermissionCancellationReasonView::ProviderWithdrawal
                            | PermissionCancellationReasonView::ExecutionFinished
                            | PermissionCancellationReasonView::Custom(_)
                    );
                    let terminal_cancellation = matches!(
                        reason.view(),
                        PermissionCancellationReasonView::ExecutionFinished
                            | PermissionCancellationReasonView::ExecutionFailed
                            | PermissionCancellationReasonView::SessionFailed
                            | PermissionCancellationReasonView::SessionClosed
                            | PermissionCancellationReasonView::EventConsumerDropped
                            | PermissionCancellationReasonView::SessionHandlesDropped
                    );
                    let valid_lifecycle = if operation == 2 {
                        closure_cause
                    } else {
                        !terminal_cancellation
                    };
                    if allowed && valid_lifecycle {
                        assert!(result.is_ok(), "{reason:?} {origin:?}");
                        assert_eq!(controller.retained_review_bytes, 0);
                    } else {
                        assert!(matches!(result, Err(AgentError::InvalidInput(_))));
                        assert!(!controller.session.is_closed());
                        assert_eq!(controller.retained_review_bytes, bytes);
                        let resolution =
                            controller.answer_permission(answer("permission")).unwrap();
                        assert_eq!(resolution.input(), &input());
                    }
                }
                let mut request = PermissionRequest::new(
                    PermissionId::new("permission").unwrap(),
                    ExecutionId::new("execution").unwrap(),
                    ToolCallId::new("tool").unwrap(),
                    options("Allow".into()),
                );
                request.cancel(reason.clone()).unwrap();
                assert_eq!(
                    PermissionCancellation::from_record(
                        ExecutionSessionId::new("session").unwrap(),
                        request,
                        input(),
                        origin.clone()
                    )
                    .is_ok(),
                    allowed
                );
                let mut controller = controller();
                if let Ok(result) = controller
                    .session
                    .close_execution(&ExecutionId::new("execution").unwrap(), reason.clone())
                {
                    let (closure, _) = result.into_parts();
                    assert_eq!(
                        SessionClosureRecord::new(closure.unwrap(), origin.clone()).is_ok(),
                        allowed
                    );
                }
            }
        }
    }

    fn controller() -> ExecutionController {
        let mut controller = ExecutionController::new(ExecutionSessionId::new("session").unwrap());
        controller
            .begin_execution(ExecutionId::new("execution").unwrap())
            .unwrap();
        controller
    }
    fn tool(title: Option<String>) -> ToolCallUpdate {
        ToolCallUpdate::new(
            ToolCallId::new("tool").unwrap(),
            title,
            None,
            None,
            None,
            None,
        )
    }
    fn options(label: String) -> PermissionOptions {
        PermissionOptions::new(
            vec![PermissionOption::new(
                PermissionOptionId::new("allow").unwrap(),
                label,
                PermissionDecision::new(PermissionEffect::Allow, PermissionScope::request()),
            )
            .unwrap()],
            &PermissionOfferPolicy::once_only(),
        )
        .unwrap()
    }
    fn input() -> ToolReviewInput {
        ToolReviewInput {
            name: "Write".into(),
            arguments_json: "reviewed input".into(),
        }
    }
    fn request(
        controller: &mut ExecutionController,
        id: &str,
        title: &str,
    ) -> Result<ExecutionEvent, AgentError> {
        let execution = controller.active_execution_id().unwrap().clone();
        controller.request_permission(
            &execution,
            PermissionId::new(id).unwrap(),
            tool(Some(title.into())),
            input(),
            options("Allow".into()),
        )
    }
    fn answer(id: &str) -> PermissionAnswer {
        PermissionAnswer {
            execution_id: ExecutionId::new("execution").unwrap(),
            id: PermissionId::new(id).unwrap(),
            option_id: PermissionOptionId::new("allow").unwrap(),
            attribution: ApprovalAttribution::new(
                ActionContext::new("actor", "surface", "answer").unwrap(),
                ApprovalBasis::Explicit,
            ),
        }
    }
    #[test]
    fn rejected_reviews_preserve_the_observation_and_exact_pending_input() {
        let mut controller = controller();
        request(&mut controller, "permission", "original").unwrap();
        assert!(request(&mut controller, "permission", "unaccepted").is_err());
        assert_eq!(
            controller
                .session
                .tool(&ToolCallId::new("tool").unwrap())
                .unwrap()
                .observation()
                .title()
                .as_deref(),
            Some("original")
        );
        let resolution = controller.answer_permission(answer("permission")).unwrap();
        assert_eq!(resolution.input(), &input());
        assert_eq!(controller.retained_review_bytes, 0);
        assert!(request(&mut controller, "permission", "replayed").is_err());
        assert_eq!(
            controller
                .close(
                    PermissionCancellationReason::session_closed(),
                    CancellationOrigin::Runtime
                )
                .unwrap()
                .len(),
            1
        );
        assert!(request(&mut controller, "other", "after close").is_err());
        assert_eq!(
            controller
                .session
                .tool(&ToolCallId::new("tool").unwrap())
                .unwrap()
                .observation()
                .title()
                .as_deref(),
            Some("original")
        );
    }
    #[test]
    fn permission_event_retains_snapshot_after_later_tool_updates() {
        let mut controller = controller();
        let event = request(&mut controller, "permission", "reviewed title").unwrap();
        controller
            .tool_event(
                &ExecutionId::new("execution").unwrap(),
                tool(Some("later title".into())),
            )
            .unwrap();
        let ExecutionUpdate::PermissionRequested {
            id,
            tool_id,
            observation,
            input: reviewed_input,
            ..
        } = event.update()
        else {
            panic!("expected permission review");
        };
        assert_eq!(event.execution_id().as_str(), "execution");
        assert_eq!(id.as_str(), "permission");
        assert_eq!(tool_id.as_str(), "tool");
        assert_eq!(observation.title().as_deref(), Some("reviewed title"));
        assert_eq!(reviewed_input, &input());
        assert_eq!(
            controller
                .session
                .tool(tool_id)
                .unwrap()
                .observation()
                .title()
                .as_deref(),
            Some("later title")
        );
        let resolution = controller.answer_permission(answer("permission")).unwrap();
        assert_eq!(resolution.request().tool_id(), tool_id);
        assert_eq!(resolution.input(), reviewed_input);
    }
    #[test]
    fn long_tool_identity_copies_count_at_admission_replacement_and_review_release() {
        let id = ToolCallId::new("t".repeat(256)).unwrap();
        let update = |title| ToolCallUpdate::new(id.clone(), title, None, None, None, None);
        let mut controller = controller();
        let identity_bytes = "execution".len() + 2 * id.as_str().len();
        assert!(controller
            .tool_event(
                &ExecutionId::new("execution").unwrap(),
                update(Some("x".repeat(MAX_RETAINED_BYTES - identity_bytes + 1)))
            )
            .is_err());
        assert_eq!(controller.session.tool_count(), 0);
        controller
            .tool_event(
                &ExecutionId::new("execution").unwrap(),
                update(Some("x".repeat(MAX_RETAINED_BYTES - identity_bytes))),
            )
            .unwrap();
        assert_eq!(controller.retained_tool_bytes, MAX_RETAINED_BYTES);
        controller
            .tool_event(&ExecutionId::new("execution").unwrap(), update(None))
            .unwrap();
        assert_eq!(controller.retained_tool_bytes, MAX_RETAINED_BYTES);
        assert!(controller
            .tool_event(
                &ExecutionId::new("execution").unwrap(),
                update(Some("x".repeat(MAX_RETAINED_BYTES - identity_bytes + 1)))
            )
            .is_err());
        assert_eq!(controller.retained_tool_bytes, MAX_RETAINED_BYTES);
        controller
            .tool_event(
                &ExecutionId::new("execution").unwrap(),
                update(Some(String::new())),
            )
            .unwrap();
        assert_eq!(controller.retained_tool_bytes, identity_bytes);
        for release in 0..5 {
            let target = controller.active_execution_id().unwrap().clone();
            let choices = options("Allow".into());
            let permission = PermissionId::new(format!("review-{release}")).unwrap();
            let overhead = id.as_str().len()
                + 3 * permission.as_str().len()
                + controller
                    .session
                    .active_execution()
                    .unwrap()
                    .as_str()
                    .len()
                + choices.payload_bytes()
                + "Read".len();
            let review = |extra| ToolReviewInput {
                name: "Read".into(),
                arguments_json: "x".repeat(MAX_RETAINED_BYTES - overhead + extra),
            };
            assert!(controller
                .request_permission(
                    &target,
                    permission.clone(),
                    update(None),
                    review(1),
                    choices.clone()
                )
                .is_err());
            assert_eq!(controller.retained_review_bytes, 0);
            controller
                .request_permission(
                    &target,
                    permission.clone(),
                    update(None),
                    review(0),
                    choices,
                )
                .unwrap();
            assert_eq!(controller.retained_review_bytes, MAX_RETAINED_BYTES);
            match release {
                0 => {
                    controller
                        .answer_permission(answer(permission.as_str()))
                        .unwrap();
                }
                1 => {
                    assert!(controller
                        .cancel_permission(
                            &target,
                            &permission,
                            PermissionCancellationReason::provider_withdrawal(),
                            CancellationOrigin::Provider
                        )
                        .unwrap()
                        .is_some());
                }
                2 => {
                    assert_eq!(
                        controller
                            .cancel_permissions(
                                &target,
                                PermissionCancellationReason::deadline_exceeded(),
                                CancellationOrigin::Runtime
                            )
                            .unwrap()
                            .len(),
                        1
                    );
                }
                3 => {
                    let _evidence = controller
                        .finish_execution(&target, Ok(ExecutionOutcome::Completed))
                        .unwrap();
                    assert_eq!(controller.retained_tool_bytes, 0);
                    controller
                        .begin_execution(ExecutionId::new("next-execution").unwrap())
                        .unwrap();
                }
                _ => {
                    let records = controller
                        .close(
                            PermissionCancellationReason::session_closed(),
                            CancellationOrigin::Runtime,
                        )
                        .unwrap();
                    assert_eq!(records.len(), 2);
                }
            }
            assert_eq!(controller.retained_review_bytes, 0);
        }
    }

    #[test]
    fn tool_budget_retains_sparse_fields_and_releases_replaced_or_finished_content() {
        let mut controller = controller();
        let identity_bytes = controller
            .session
            .tool_payload_bytes_after(controller.session.active_execution().unwrap(), &tool(None))
            .unwrap();
        controller
            .tool_event(
                &ExecutionId::new("execution").unwrap(),
                tool(Some("x".repeat(MAX_RETAINED_BYTES - identity_bytes))),
            )
            .unwrap();
        controller
            .tool_event(&ExecutionId::new("execution").unwrap(), tool(None))
            .unwrap();
        assert_eq!(controller.retained_tool_bytes, MAX_RETAINED_BYTES);
        let other = ToolCallUpdate::new(
            ToolCallId::new("other").unwrap(),
            Some("y".into()),
            None,
            None,
            None,
            None,
        );
        assert!(controller
            .tool_event(&ExecutionId::new("execution").unwrap(), other.clone())
            .is_err());
        assert_eq!(controller.session.tool_count(), 1);
        controller
            .tool_event(
                &ExecutionId::new("execution").unwrap(),
                tool(Some(String::new())),
            )
            .unwrap();
        controller
            .tool_event(&ExecutionId::new("execution").unwrap(), other)
            .unwrap();
        controller
            .finish_execution(
                &ExecutionId::new("execution").unwrap(),
                Ok(ExecutionOutcome::Completed),
            )
            .unwrap();
        assert_eq!(controller.retained_tool_bytes, 0);
        assert_eq!(controller.session.tool_count(), 0);
    }
    #[test]
    fn oversized_options_are_rejected_before_tool_or_permission_mutation() {
        let mut controller = controller();
        assert!(controller
            .request_permission(
                &ExecutionId::new("execution").unwrap(),
                PermissionId::new("p").unwrap(),
                tool(Some("unaccepted".into())),
                input(),
                options("x".repeat(MAX_RETAINED_BYTES))
            )
            .is_err());
        assert_eq!(controller.session.tool_count(), 0);
        assert_eq!(controller.session.permission_count(), 0);
        assert_eq!(controller.retained_review_bytes, 0);
    }
    #[test]
    fn cancellation_releases_only_its_review_and_execution_caps_include_resolved_ids() {
        let mut controller = controller();
        request(&mut controller, "first", "tool").unwrap();
        request(&mut controller, "second", "tool").unwrap();
        let both = controller.retained_review_bytes;
        assert!(matches!(controller
                .cancel_permission(
                    &ExecutionId::new("execution").unwrap(),
                    &PermissionId::new("first").unwrap(),
                    PermissionCancellationReason::provider_withdrawal(),
                    CancellationOrigin::Provider
                )
                .unwrap()
                .unwrap()
                .request()
                .state(),
            PermissionStateView::Cancelled { reason } if reason.view() == PermissionCancellationReasonView::ProviderWithdrawal
        ));
        assert!(controller.retained_review_bytes < both);
        assert_eq!(
            controller.answer_permission(answer("first")),
            Err(AgentError::StalePermission)
        );
        controller.answer_permission(answer("second")).unwrap();
        assert_eq!(controller.retained_review_bytes, 0);
        for index in 2..MAX_EXECUTION_PERMISSIONS {
            let id = index.to_string();
            request(&mut controller, &id, "tool").unwrap();
            controller.answer_permission(answer(&id)).unwrap();
        }
        assert!(request(&mut controller, "over-limit", "tool").is_err());
        controller
            .finish_execution(
                &ExecutionId::new("execution").unwrap(),
                Ok(ExecutionOutcome::Completed),
            )
            .unwrap();
        controller
            .begin_execution(ExecutionId::new("next-execution").unwrap())
            .unwrap();
        request(&mut controller, "first", "tool").unwrap();
        assert_eq!(
            controller
                .cancel_permissions(
                    &ExecutionId::new("next-execution").unwrap(),
                    PermissionCancellationReason::deadline_exceeded(),
                    CancellationOrigin::Runtime
                )
                .unwrap()
                .len(),
            1
        );
        assert_eq!(controller.retained_review_bytes, 0);
    }
    #[test]
    fn controller_rejects_reused_execution_identity_without_releasing_new_run() {
        let mut controller = controller();
        controller
            .finish_execution(
                &ExecutionId::new("execution").unwrap(),
                Ok(ExecutionOutcome::Completed),
            )
            .unwrap();
        assert!(matches!(
            controller.begin_execution(ExecutionId::new("execution").unwrap()),
            Err(AgentError::InvalidInput(_))
        ));
        assert_eq!(controller.session.active_execution(), None);
        controller
            .begin_execution(ExecutionId::new("next-execution").unwrap())
            .unwrap();
        request(&mut controller, "review", "tool").unwrap();
        let bytes = controller.retained_review_bytes;
        assert!(matches!(
            controller.finish_execution(
                &ExecutionId::new("next-execution").unwrap(),
                Err(PermissionCancellationReason::session_closed())
            ),
            Err(AgentError::InvalidInput(_))
        ));
        assert_eq!(
            controller.session.active_execution().unwrap().as_str(),
            "next-execution"
        );
        assert!(!controller.session.is_closed());
        assert_eq!(controller.retained_review_bytes, bytes);
        assert_eq!(
            controller
                .close(
                    PermissionCancellationReason::session_closed(),
                    CancellationOrigin::Runtime
                )
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            controller
                .finish_execution(
                    &ExecutionId::new("next-execution").unwrap(),
                    Err(PermissionCancellationReason::session_closed())
                )
                .unwrap()
                .len(),
            1
        );
    }
    #[test]
    fn stale_execution_close_preserves_later_run_and_review_accounting() {
        let mut controller = controller();
        controller
            .finish_execution(
                &ExecutionId::new("execution").unwrap(),
                Ok(ExecutionOutcome::Completed),
            )
            .unwrap();
        let next = ExecutionId::new("next").unwrap();
        controller.begin_execution(next.clone()).unwrap();
        request(&mut controller, "permission", "original").unwrap();
        let bytes = controller.retained_review_bytes;
        assert!(controller
            .finish_execution(
                &ExecutionId::new("execution").unwrap(),
                Ok(ExecutionOutcome::Completed)
            )
            .is_err());
        assert_eq!(controller.active_execution_id(), Some(&next));
        assert_eq!(controller.retained_review_bytes, bytes);
        assert!(controller
            .close_execution(
                &ExecutionId::new("execution").unwrap(),
                PermissionCancellationReason::execution_failed(),
                CancellationOrigin::Runtime,
            )
            .is_err());
        assert_eq!(controller.active_execution_id(), Some(&next));
        assert!(!controller.session.is_closed());
        assert_eq!(controller.retained_review_bytes, bytes);
        assert!(controller
            .close(
                PermissionCancellationReason::execution_failed(),
                CancellationOrigin::Runtime
            )
            .is_err());
        let records = controller
            .close_execution(
                &next,
                PermissionCancellationReason::execution_failed(),
                CancellationOrigin::Runtime,
            )
            .unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(controller.retained_review_bytes, 0);
    }
}
