#![deny(missing_docs)]

use crate::domain::agent_execution::{
    executions::ExecutionId,
    permissions::{
        PermissionCancellationReason, PermissionCancellationReasonView, PermissionDecision,
        PermissionId, PermissionOptionId, PermissionOptions,
    },
    tools::ToolCallId,
    ExecutionError,
};

/// One reviewable action, bound to an execution and tool. The owning execution
/// aggregate admits and resolves live reviews once; this entity exposes inspection.
/// Constructing another request with the same IDs does not change an admitted review
/// or authorize a provider response. The host authorizes callers through its controller.
/// Borrow requests for inspection and transfer pending requests into their session.
/// Terminal transitions are internal; detached evidence is not live decision authority.
/// ```compile_fail
/// use nessa_sdk::domain::agent_execution::{permissions::{PermissionRequest, PermissionOptionId}, executions::ExecutionId};
/// fn answer(request: &mut PermissionRequest, execution: &ExecutionId, option: &PermissionOptionId) {
///     request.answer(execution, option).unwrap();
/// }
/// ```
/// ```compile_fail
/// use nessa_sdk::domain::agent_execution::permissions::{PermissionRequest, PermissionCancellationReason};
/// fn cancel(request: &mut PermissionRequest) {
///     request.cancel(PermissionCancellationReason::provider_withdrawal()).unwrap();
/// }
/// ```
/// ```compile_fail
/// use nessa_sdk::domain::agent_execution::permissions::PermissionRequest;
/// fn duplicate(request: &PermissionRequest) -> PermissionRequest {
///     request.clone()
/// }
/// ```
#[derive(Debug, PartialEq, Eq)]
pub struct PermissionRequest {
    id: PermissionId,
    execution_id: ExecutionId,
    tool_id: ToolCallId,
    options: PermissionOptions,
    state: PermissionState,
}
// Resolution is owned only by its request; public inspection borrows its evidence.
#[derive(Debug, PartialEq, Eq)]
enum PermissionState {
    /// The review can still accept one answer or cancellation.
    Pending,
    /// A choice was accepted locally; this does not confirm tool execution.
    Answered {
        /// Identity of the exact offered choice selected.
        option_id: PermissionOptionId,
        /// Original effect and scope of the selected choice.
        decision: PermissionDecision,
    },
    /// The review ended without an answer; no rollback is implied.
    Cancelled {
        /// First lifecycle cause that cancelled the request.
        reason: PermissionCancellationReason,
    },
}

/// Borrowed resolution of one permission request; retained identities and decisions are immutable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionStateView<'a> {
    /// The review can still accept one answer or cancellation.
    Pending,
    /// A choice was accepted locally; this does not confirm tool execution.
    Answered {
        /// Identity of the exact offered choice selected.
        option_id: &'a PermissionOptionId,
        /// Original effect and scope of the selected choice.
        decision: &'a PermissionDecision,
    },
    /// The review ended without an answer; no rollback is implied.
    Cancelled {
        /// First lifecycle cause that cancelled the request.
        reason: &'a PermissionCancellationReason,
    },
}

impl PermissionRequest {
    /// Create a pending review bound to the supplied request, execution, tool, and validated offered choices. Construction does not authorize a caller.
    pub fn new(
        id: PermissionId,
        execution_id: ExecutionId,
        tool_id: ToolCallId,
        options: PermissionOptions,
    ) -> Self {
        Self {
            id,
            execution_id,
            tool_id,
            options,
            state: PermissionState::Pending,
        }
    }
    /// Stable review identity reserved by the owning execution through completion.
    pub fn id(&self) -> &PermissionId {
        &self.id
    }
    /// Execution in which this review can be answered.
    pub fn execution_id(&self) -> &ExecutionId {
        &self.execution_id
    }
    /// Tool whose exact input is being reviewed.
    pub fn tool_id(&self) -> &ToolCallId {
        &self.tool_id
    }
    /// Validated offered identities and decisions; answers must select one of these options.
    pub fn options(&self) -> &PermissionOptions {
        &self.options
    }
    /// Borrow the current pending, answered, or cancelled state and its retained evidence.
    /// The returned view cannot replace the selected option independently of its decision.
    ///
    /// ```compile_fail
    /// use nessa_sdk::domain::agent_execution::permissions::{PermissionRequest, PermissionOptionId, PermissionStateView};
    /// fn replace_selection(request: &mut PermissionRequest) {
    ///     if let PermissionStateView::Answered { option_id, .. } = request.state() {
    ///         *option_id = PermissionOptionId::new("another-option").unwrap();
    ///     }
    /// }
    /// ```
    ///
    /// ```compile_fail
    /// use nessa_sdk::domain::agent_execution::permissions::{PermissionRequest, PermissionDecision, PermissionEffect, PermissionScope, PermissionStateView};
    /// fn replace_decision(request: &mut PermissionRequest) {
    ///     if let PermissionStateView::Answered { decision, .. } = request.state() {
    ///         *decision = PermissionDecision::new(PermissionEffect::Deny, PermissionScope::request());
    ///     }
    /// }
    /// ```
    pub fn state(&self) -> PermissionStateView<'_> {
        match &self.state {
            PermissionState::Pending => PermissionStateView::Pending,
            PermissionState::Answered {
                option_id,
                decision,
            } => PermissionStateView::Answered {
                option_id,
                decision,
            },
            PermissionState::Cancelled { reason } => PermissionStateView::Cancelled { reason },
        }
    }
    /// Variable retained bytes for identities, options, and terminal evidence.
    /// Includes choice slots and every owned copy, but excludes this entity's fixed
    /// struct size and any external collection keys. Pending aggregate storage and
    /// returned terminal evidence have different owners and are budgeted separately.
    pub fn payload_bytes(&self) -> usize {
        let state_bytes = match &self.state {
            PermissionState::Pending => 0,
            PermissionState::Answered {
                option_id,
                decision,
            } => option_id
                .as_str()
                .len()
                .saturating_add(decision.payload_bytes()),
            PermissionState::Cancelled { reason } => match reason.view() {
                PermissionCancellationReasonView::Custom(reason) => reason
                    .code()
                    .len()
                    .saturating_add(reason.explanation().len()),
                _ => 0,
            },
        };
        self.id
            .as_str()
            .len()
            .saturating_add(self.execution_id.as_str().len())
            .saturating_add(self.tool_id.as_str().len())
            .saturating_add(self.options.payload_bytes())
            .saturating_add(state_bytes)
    }
    /// Resolve the offered option for the matching execution. Returns DifferentExecution, PermissionResolved, or UnknownPermissionOption without changing state on rejection.
    pub(crate) fn answer(
        &mut self,
        execution_id: &ExecutionId,
        option_id: &PermissionOptionId,
    ) -> Result<PermissionDecision, ExecutionError> {
        if execution_id != &self.execution_id {
            return Err(ExecutionError::DifferentExecution);
        }
        if self.state != PermissionState::Pending {
            return Err(ExecutionError::PermissionResolved);
        }
        let decision = self
            .options
            .find(option_id)
            .ok_or(ExecutionError::UnknownPermissionOption)?
            .decision()
            .clone();
        self.state = PermissionState::Answered {
            option_id: option_id.clone(),
            decision: decision.clone(),
        };
        Ok(decision)
    }
    /// Cancel a pending request with the supplied lifecycle reason. Returns PermissionResolved for answered or already-cancelled reviews, preserving their original evidence.
    pub(crate) fn cancel(
        &mut self,
        reason: PermissionCancellationReason,
    ) -> Result<(), ExecutionError> {
        if self.state != PermissionState::Pending {
            return Err(ExecutionError::PermissionResolved);
        }
        self.state = PermissionState::Cancelled { reason };
        Ok(())
    }
}
