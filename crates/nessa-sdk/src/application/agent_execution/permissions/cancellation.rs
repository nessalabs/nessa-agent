use super::ActionContext;
use crate::application::agent_execution::agents::AgentError;
use crate::application::agent_execution::tools::ToolReviewInput;
use crate::domain::agent_execution::{
    executions::ExecutionId,
    permissions::{
        PermissionCancellationReason, PermissionCancellationReasonView, PermissionId,
        PermissionRequest, PermissionStateView,
    },
    sessions::ExecutionSessionId,
};
use std::sync::Arc;

/// The observed initiator. Provider/runtime actions never imply a human identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CancellationOrigin {
    /// Explicit host-authorized action, including a guard acting as its own principal.
    Client(ActionContext),
    /// Withdrawal observed from the provider; no human attribution is inferred.
    Provider,
    /// Automatic SDK lifecycle cleanup, deadline, or failure.
    Runtime,
}

impl CancellationOrigin {
    /// Reject a lifecycle cause that contradicts its recorded initiator.
    /// Runtime release may retain SessionClosed from an earlier explicit close.
    pub(crate) fn validate_reason(
        &self,
        reason: &PermissionCancellationReason,
    ) -> Result<(), AgentError> {
        let valid = match self {
            Self::Provider => matches!(
                reason.view(),
                PermissionCancellationReasonView::ProviderWithdrawal
            ),
            Self::Client(_) => matches!(
                reason.view(),
                PermissionCancellationReasonView::Custom(_)
                    | PermissionCancellationReasonView::SessionClosed
            ),
            Self::Runtime => !matches!(
                reason.view(),
                PermissionCancellationReasonView::ProviderWithdrawal
                    | PermissionCancellationReasonView::Custom(_)
            ),
        };
        if valid {
            Ok(())
        } else {
            Err(AgentError::InvalidInput(
                "cancellation cause contradicts its initiator".into(),
            ))
        }
    }
}

/// A cancelled review with its original input and lifecycle cause retained in request.state().
/// This records local permission cancellation, not provider acknowledgement or tool termination.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "cancellation evidence must be recorded or its delivery failure reported"]
pub struct PermissionCancellation {
    session_id: ExecutionSessionId,
    // Only terminal evidence is shared. The private Arc and borrowed accessor
    // never grant another mutable permission authority to an audit consumer.
    request: Arc<PermissionRequest>,
    input: ToolReviewInput,
    origin: CancellationOrigin,
}
impl PermissionCancellation {
    /// Restore persisted application evidence through validated domain values.
    /// Non-cancelled requests or contradictory cause/origin pairs return InvalidInput.
    /// Live permission decisions must continue through the execution controller.
    pub fn from_record(
        session_id: ExecutionSessionId,
        request: PermissionRequest,
        input: ToolReviewInput,
        origin: CancellationOrigin,
    ) -> Result<Self, AgentError> {
        let PermissionStateView::Cancelled { reason } = request.state() else {
            return Err(AgentError::InvalidInput(
                "cancellation evidence requires a cancelled request".into(),
            ));
        };
        origin.validate_reason(reason)?;
        Ok(Self {
            session_id,
            request: Arc::new(request),
            input,
            origin,
        })
    }
    pub(in crate::application::agent_execution) fn new(
        session_id: ExecutionSessionId,
        request: PermissionRequest,
        input: ToolReviewInput,
        origin: CancellationOrigin,
    ) -> Self {
        Self {
            session_id,
            request: Arc::new(request),
            input,
            origin,
        }
    }
    /// Provider context that owned the cancelled review.
    pub fn session_id(&self) -> &ExecutionSessionId {
        &self.session_id
    }
    /// Immutable cancelled review carrying exact identities, options, and cause.
    pub fn request(&self) -> &PermissionRequest {
        &self.request
    }
    /// Original tool input captured when the review was requested.
    pub fn input(&self) -> &ToolReviewInput {
        &self.input
    }
    /// Verified explicit actor or observed provider/runtime initiator.
    pub fn origin(&self) -> &CancellationOrigin {
        &self.origin
    }
}

/// An explicit, host-authorized withdrawal of one pending review.
/// Automatic guards use their own principal and a custom reason with a stable code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionCancellationRequest {
    /// Execution that owns the pending review; never inferred from current activity.
    pub execution_id: ExecutionId,
    /// Exact pending permission identity within that execution.
    pub id: PermissionId,
    /// Caller-owned Custom cause. Runtime/provider lifecycle causes are rejected.
    pub reason: PermissionCancellationReason,
    /// Host-verified caller or guard attribution; construction does not authorize it.
    pub actor: ActionContext,
}
