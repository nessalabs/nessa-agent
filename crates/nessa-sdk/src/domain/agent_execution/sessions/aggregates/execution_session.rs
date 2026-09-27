#![deny(missing_docs)]

use crate::domain::agent_execution::{
    executions::{ExecutionId, ExecutionOutcome},
    permissions::{
        PermissionCancellationReason, PermissionCancellationReasonView, PermissionId,
        PermissionOptionId, PermissionRequest, PermissionStateView,
    },
    sessions::{ExecutionFinish, ExecutionSessionId, SessionClosure},
    tools::{ToolCall, ToolCallId, ToolCallUpdate},
    ExecutionError,
};
use std::collections::{HashMap, HashSet};

/// State for one live attachment to an agent context, with sequential executions.
/// Restoring a closed provider context creates a fresh aggregate; it never revives this one.
/// This is ephemeral execution state, not a conversation transcript or auth session.
#[derive(Debug)]
pub struct ExecutionSession {
    id: ExecutionSessionId,
    // The first local closure survives settlement and repeated close calls. It
    // remains separate from final execution evidence; it is not cleanup confirmation.
    closure: Option<SessionClosure>,
    // One execution lifetime, retained through close until finish_execution.
    active_execution: Option<ActiveExecution>,
    // Every admitted execution identity for this live attachment. Never cleared by
    // finish/close: reuse would let delayed callbacks acquire authority over a new run.
    // Linear lifetime history; adapters bound individual IDs, not conversation length.
    seen_execution_ids: HashSet<ExecutionId>,
}

/// Private state owned by the session aggregate, never an independent aggregate.
/// Dropping it ends the tool, pending-review, and reserved-review-ID lifetimes together.
#[derive(Debug)]
struct ActiveExecution {
    id: ExecutionId,
    tools: HashMap<ToolCallId, ToolCall>,
    // Resolved reviews leave this map and become application evidence.
    pending_permissions: HashMap<PermissionId, PermissionRequest>,
    // Reserves every admitted review ID until finish, preventing replay after resolution.
    seen_permission_ids: HashSet<PermissionId>,
}
impl ActiveExecution {
    fn new(id: ExecutionId) -> Self {
        Self {
            id,
            tools: HashMap::new(),
            pending_permissions: HashMap::new(),
            seen_permission_ids: HashSet::new(),
        }
    }
    fn cancel_permissions(
        &mut self,
        reason: PermissionCancellationReason,
    ) -> Vec<PermissionRequest> {
        std::mem::take(&mut self.pending_permissions)
            .into_values()
            .map(|mut request| {
                request
                    .cancel(reason.clone())
                    .expect("session retains only pending permissions");
                request
            })
            .collect()
    }
}
impl ExecutionSession {
    /// Create an open live attachment with provider identity id and no active execution. This does not create a provider process or persisted history.
    pub fn new(id: ExecutionSessionId) -> Self {
        Self {
            id,
            closure: None,
            active_execution: None,
            seen_execution_ids: HashSet::new(),
        }
    }
    /// Provider context identity attached to this live aggregate.
    pub fn id(&self) -> &ExecutionSessionId {
        &self.id
    }
    /// Identity of the execution retained through cleanup, or None between executions.
    pub fn active_execution(&self) -> Option<&ExecutionId> {
        self.active_execution.as_ref().map(|active| &active.id)
    }
    /// Whether close has been requested, even if execution cleanup is still in progress.
    pub fn is_closed(&self) -> bool {
        self.closure.is_some()
    }
    /// First local closure evidence, retained through execution settlement and
    /// repeated close until this aggregate is dropped. None while still open.
    pub fn closure(&self) -> Option<&SessionClosure> {
        self.closure.as_ref()
    }
    /// Begin work in an already-live attachment. The application backend resumes
    /// closed provider contexts and constructs a fresh aggregate before calling this.
    /// `id` must never have been admitted by this aggregate; `DuplicateExecution`
    /// rejects reuse even after completion. Rejected attempts do not reserve their ID.
    /// Accepted IDs remain until aggregate drop, with linear per-attachment history;
    /// this ephemeral set does not claim to restore provider or persisted history.
    pub fn begin_execution(&mut self, id: ExecutionId) -> Result<(), ExecutionError> {
        if self.is_closed() {
            return Err(ExecutionError::SessionClosed);
        }
        if self.active_execution.is_some() {
            return Err(ExecutionError::SessionBusy);
        }
        if !self.seen_execution_ids.insert(id.clone()) {
            return Err(ExecutionError::DuplicateExecution);
        }
        self.active_execution = Some(ActiveExecution::new(id));
        Ok(())
    }
    /// Complete only `id` with its explicit terminal `result`, returning once-only
    /// execution evidence even without pending reviews. Success retains its outcome.
    /// Failure accepts execution failure, session closure/failure, deadline, or lost consumer/
    /// handles; permission withdrawal, normal completion, and custom permission causes
    /// are not execution failures. Session closure/failure or lost consumer/handles require
    /// a preceding `close` transition retaining the active execution identity.
    /// Execution failure may settle an open attachment. A deadline requires prior
    /// `close`, so a timeout alone cannot release execution ownership or admit new work.
    /// The first local closure and later settlement retain separate facts: every
    /// explicit provider outcome remains valid after closure, including late success
    /// after a deadline or failure. Settlement releases execution resources but never
    /// reopens this aggregate or claims that provider process cleanup was confirmed.
    /// The application owns external cleanup and admission of a fresh attachment.
    /// `DifferentExecution` and `InvalidExecutionFailureReason`
    /// leave all state unchanged. Success releases tools and reserved review IDs together.
    pub fn finish_execution(
        &mut self,
        id: &ExecutionId,
        result: Result<ExecutionOutcome, PermissionCancellationReason>,
    ) -> Result<(ExecutionFinish, Vec<PermissionRequest>), ExecutionError> {
        self.check_execution(id)?;
        match result.as_ref().map_err(PermissionCancellationReason::view) {
            Err(
                PermissionCancellationReasonView::ProviderWithdrawal
                | PermissionCancellationReasonView::ExecutionFinished
                | PermissionCancellationReasonView::Custom(_),
            ) => {
                return Err(ExecutionError::InvalidExecutionFailureReason);
            }
            Err(
                PermissionCancellationReasonView::SessionClosed
                | PermissionCancellationReasonView::SessionFailed
                | PermissionCancellationReasonView::EventConsumerDropped
                | PermissionCancellationReasonView::SessionHandlesDropped
                | PermissionCancellationReasonView::DeadlineExceeded,
            ) => {
                if !self.is_closed() {
                    return Err(ExecutionError::InvalidExecutionFailureReason);
                }
            }
            Ok(_) | Err(PermissionCancellationReasonView::ExecutionFailed) => {}
        }
        let mut active = self
            .active_execution
            .take()
            .expect("checked active execution");
        let reason = result
            .as_ref()
            .err()
            .cloned()
            .unwrap_or(PermissionCancellationReason::execution_finished());
        Ok((
            ExecutionFinish::new(self.id.clone(), active.id.clone(), result),
            active.cancel_permissions(reason),
        ))
    }
    /// Permanently end this live session, rejecting new executions and permission answers.
    /// This does not delete stored history. Process cleanup and result delivery are external.
    /// Pending permissions are cancelled even while an active execution is being stopped.
    /// Returns an open-to-closed transition once, even when idle, alongside cancelled
    /// reviews. Repeated valid calls return no new transition and cannot replace its cause.
    /// Execution-scoped callbacks must use `close_execution` with their captured ID.
    /// `ExecutionFailed` is rejected here; idle attachment failures use `SessionFailed`.
    /// `reason` must describe session closure/failure, execution failure, deadline, or lost
    /// consumer/handles. Permission-only causes return `InvalidSessionClosureReason`
    /// before any mutation, including on an already-closed attachment.
    #[must_use = "retain cancellation evidence when closing the session"]
    pub fn close(
        &mut self,
        reason: PermissionCancellationReason,
    ) -> Result<SessionClosureResult, ExecutionError> {
        if reason == PermissionCancellationReason::execution_failed() {
            return Err(ExecutionError::InvalidSessionClosureReason);
        }
        self.close_session(reason)
    }

    /// Close this attachment because of the matching `execution` and retain `reason`.
    /// The caller supplies the execution ID captured when its work began. A stale
    /// or absent execution returns `DifferentExecution` before changing any state.
    /// Valid closure causes and audit evidence match [`Self::close`], including
    /// execution failure. This performs no provider cleanup or history deletion.
    #[must_use = "retain cancellation evidence when closing the session"]
    pub fn close_execution(
        &mut self,
        execution: &ExecutionId,
        reason: PermissionCancellationReason,
    ) -> Result<SessionClosureResult, ExecutionError> {
        self.check_execution(execution)?;
        self.close_session(reason)
    }

    fn close_session(
        &mut self,
        reason: PermissionCancellationReason,
    ) -> Result<SessionClosureResult, ExecutionError> {
        let closure = if self.is_closed() {
            SessionClosure::validate_reason(&reason)?;
            None
        } else {
            Some(SessionClosure::new(
                self.id.clone(),
                self.active_execution().cloned(),
                reason.clone(),
            )?)
        };
        if let Some(closure) = &closure {
            self.closure = Some(closure.clone());
        }
        // Closing owns the entire attachment, including its current execution.
        // Execution-scoped callbacks must use the identity-checked public method.
        let permissions = self
            .active_execution
            .as_mut()
            .map_or_else(Vec::new, |active| active.cancel_permissions(reason));
        Ok(SessionClosureResult::new(closure, permissions))
    }
    /// Apply `update` within the matching active execution, creating a first-seen tool.
    /// `SessionClosed` or `DifferentExecution` leaves observations unchanged; closing
    /// freezes retained tools until finish releases them. No provider effect occurs here.
    pub fn observe_tool(
        &mut self,
        execution: &ExecutionId,
        update: ToolCallUpdate,
    ) -> Result<(), ExecutionError> {
        if self.is_closed() {
            return Err(ExecutionError::SessionClosed);
        }
        self.check_execution(execution)?;
        let active = self
            .active_execution
            .as_mut()
            .expect("checked active execution");
        if let Some(tool) = active.tools.get_mut(update.id()) {
            tool.apply(execution, update)
                .expect("session selects the tool by identity within its execution");
        } else {
            active.tools.insert(
                update.id().clone(),
                ToolCall::new(execution.clone(), update),
            );
        }
        Ok(())
    }
    /// Find the observed tool by id in the active execution; returns None outside that lifetime.
    pub fn tool(&self, id: &ToolCallId) -> Option<&ToolCall> {
        self.active_execution.as_ref()?.tools.get(id)
    }
    /// Variable bytes retained for this tool, including the aggregate's separate lookup key.
    /// Returns zero when the tool is absent. Fixed map storage is bounded by tool count.
    pub fn tool_payload_bytes(&self, id: &ToolCallId) -> usize {
        self.tool(id).map_or(0, |tool| {
            tool.payload_bytes()
                .saturating_add(tool.id().as_str().len())
        })
    }
    /// Predict retained tool bytes after this update without cloning its payload.
    /// Includes the separate lookup key for both new and existing tools. A mismatched
    /// execution returns DifferentExecution; a closed session returns SessionClosed.
    /// Rejection leaves state unchanged.
    pub fn tool_payload_bytes_after(
        &self,
        execution: &ExecutionId,
        update: &ToolCallUpdate,
    ) -> Result<usize, ExecutionError> {
        if self.is_closed() {
            return Err(ExecutionError::SessionClosed);
        }
        self.check_execution(execution)?;
        let bytes = self.tool(update.id()).map_or_else(
            || ToolCall::initial_payload_bytes(execution, update),
            |tool| tool.payload_bytes_after(update),
        );
        Ok(bytes.saturating_add(update.id().as_str().len()))
    }
    /// Number of observed tools retained in the active execution, or zero when idle.
    pub fn tool_count(&self) -> usize {
        self.active_execution
            .as_ref()
            .map_or(0, |active| active.tools.len())
    }
    /// All requests admitted in the active execution, including resolved requests.
    pub fn permission_count(&self) -> usize {
        self.active_execution
            .as_ref()
            .map_or(0, |active| active.seen_permission_ids.len())
    }
    /// Variable bytes retained by pending reviews and reserved review identities.
    /// Includes each request payload and separate pending-map and seen-set keys.
    /// Resolved requests release payload and pending keys; their seen keys remain
    /// until execution finish. Fixed map/set storage is budgeted by permission count.
    /// Tools and external audit/input evidence are separate ownership budgets.
    pub fn permission_payload_bytes(&self) -> usize {
        self.active_execution.as_ref().map_or(0, |active| {
            let seen = active
                .seen_permission_ids
                .iter()
                .fold(0usize, |total, id| total.saturating_add(id.as_str().len()));
            active
                .pending_permissions
                .iter()
                .fold(seen, |total, (id, request)| {
                    total
                        .saturating_add(id.as_str().len())
                        .saturating_add(request.payload_bytes())
                })
        })
    }
    /// Predict total retained review bytes after admitting `request`, without mutation
    /// or payload cloning. Validates pending state, execution, session, and duplicate
    /// identity before measurement; the tool may still await its initial observation.
    /// Callers apply their own byte/count budgets before `request_permission`, which
    /// additionally requires a known tool. No policy limit is imposed by measurement.
    pub fn permission_payload_bytes_after(
        &self,
        request: &PermissionRequest,
    ) -> Result<usize, ExecutionError> {
        self.validate_permission_admission(request)?;
        Ok(self
            .permission_payload_bytes()
            .saturating_add(request.payload_bytes())
            .saturating_add(request.id().as_str().len().saturating_mul(2)))
    }
    /// Check whether a review can be admitted before its tool observation is applied.
    pub fn validate_permission_admission(
        &self,
        request: &PermissionRequest,
    ) -> Result<(), ExecutionError> {
        if self.is_closed() {
            return Err(ExecutionError::SessionClosed);
        }
        self.check_execution(request.execution_id())?;
        if request.state() != PermissionStateView::Pending {
            return Err(ExecutionError::PermissionResolved);
        }
        let active = self
            .active_execution
            .as_ref()
            .expect("checked active execution");
        if active.seen_permission_ids.contains(request.id()) {
            return Err(ExecutionError::DuplicatePermission);
        }
        Ok(())
    }
    /// Admit a pending review only for this active execution and a known tool. Rejects closed sessions, wrong execution, resolved or duplicate reviews, and unknown tools without reserving the rejected identity.
    pub fn request_permission(&mut self, request: PermissionRequest) -> Result<(), ExecutionError> {
        self.validate_permission_admission(&request)?;
        let active = self
            .active_execution
            .as_mut()
            .expect("checked active execution");
        if !active.tools.contains_key(request.tool_id()) {
            return Err(ExecutionError::UnknownTool);
        }
        active.seen_permission_ids.insert(request.id().clone());
        active
            .pending_permissions
            .insert(request.id().clone(), request);
        Ok(())
    }
    /// Resolve the exact execution, review ID, and offered option. Rejects closed or mismatched contexts and stale/unknown choices; returns the answered entity as evidence and retains its ID against replay.
    pub fn answer_permission(
        &mut self,
        execution: &ExecutionId,
        id: &PermissionId,
        option: &PermissionOptionId,
    ) -> Result<PermissionRequest, ExecutionError> {
        if self.is_closed() {
            return Err(ExecutionError::SessionClosed);
        }
        self.check_execution(execution)?;
        let active = self
            .active_execution
            .as_mut()
            .expect("checked active execution");
        let request = active
            .pending_permissions
            .get_mut(id)
            .ok_or(ExecutionError::UnknownPermission)?;
        request.answer(execution, option)?;
        Ok(active
            .pending_permissions
            .remove(id)
            .expect("answered permission"))
    }
    /// Cancel one pending review in `execution` without affecting other requests or reopening its identity.
    /// Execution-terminal causes require `finish_execution`, or correlated `close`
    /// for execution failure; standalone cancellation rejects them.
    /// Session-terminal causes require a preceding `close`; otherwise
    /// `InvalidPermissionCancellationReason` preserves every pending review.
    /// A different execution returns `DifferentExecution` without changing state; an unknown or resolved ID returns `Ok(None)`.
    #[must_use = "retain the cancelled request and its cause"]
    pub fn cancel_permission(
        &mut self,
        execution: &ExecutionId,
        id: &PermissionId,
        reason: PermissionCancellationReason,
    ) -> Result<Option<PermissionRequest>, ExecutionError> {
        self.check_execution(execution)?;
        self.check_permission_cancellation_reason(&reason)?;
        let Some(mut request) = self
            .active_execution
            .as_mut()
            .expect("checked active execution")
            .pending_permissions
            .remove(id)
        else {
            return Ok(None);
        };
        request
            .cancel(reason)
            .expect("session retains only pending permissions");
        Ok(Some(request))
    }
    #[must_use = "retain every cancelled request and its cause"]
    /// Cancel pending reviews only in `execution`, retaining `reason` in every result.
    /// Execution-terminal causes are reserved for `finish_execution` or correlated
    /// failure closure. Session-terminal causes require `close` first, returning
    /// `InvalidPermissionCancellationReason` without draining reviews on an open session.
    /// A different or inactive execution returns `DifferentExecution` before mutation.
    /// Resolved reviews keep their original decisions and IDs remain reserved until finish.
    pub fn cancel_permissions(
        &mut self,
        execution: &ExecutionId,
        reason: PermissionCancellationReason,
    ) -> Result<Vec<PermissionRequest>, ExecutionError> {
        self.check_execution(execution)?;
        self.check_permission_cancellation_reason(&reason)?;
        Ok(self
            .active_execution
            .as_mut()
            .expect("checked active execution")
            .cancel_permissions(reason))
    }
    fn check_permission_cancellation_reason(
        &self,
        reason: &PermissionCancellationReason,
    ) -> Result<(), ExecutionError> {
        match reason.view() {
            PermissionCancellationReasonView::ExecutionFinished
            | PermissionCancellationReasonView::ExecutionFailed => {
                Err(ExecutionError::InvalidPermissionCancellationReason)
            }
            PermissionCancellationReasonView::SessionClosed
            | PermissionCancellationReasonView::SessionFailed
            | PermissionCancellationReasonView::EventConsumerDropped
            | PermissionCancellationReasonView::SessionHandlesDropped
                if !self.is_closed() =>
            {
                Err(ExecutionError::InvalidPermissionCancellationReason)
            }
            _ => Ok(()),
        }
    }
    fn check_execution(&self, id: &ExecutionId) -> Result<(), ExecutionError> {
        if self.active_execution() != Some(id) {
            return Err(ExecutionError::DifferentExecution);
        }
        Ok(())
    }
}

/// Once-only session transition evidence and pending permissions cancelled by close.
/// A repeated close has no transition evidence and does not rewrite the original cause.
#[derive(Debug)]
pub struct SessionClosureResult {
    closure: Option<SessionClosure>,
    permissions: Vec<PermissionRequest>,
}
impl SessionClosureResult {
    fn new(closure: Option<SessionClosure>, permissions: Vec<PermissionRequest>) -> Self {
        Self {
            closure,
            permissions,
        }
    }
    /// Consume the result to deliver the optional session transition and every cancelled review.
    pub fn into_parts(self) -> (Option<SessionClosure>, Vec<PermissionRequest>) {
        (self.closure, self.permissions)
    }
}
