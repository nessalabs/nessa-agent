#![deny(missing_docs)]

use super::{
    CleanupFuture, OperationCapabilities, ProviderExecutionFuture, ProviderExecutionReply,
    ProviderOperationFailure, ProviderOperationFuture, ProviderSessionBackend,
    ProviderSessionState, SessionCloseRequest, SteeringOutcome,
};
use crate::application::agent_execution::agents::AgentError;
use crate::application::agent_execution::{
    executions::ExecutionRequest,
    permissions::{
        CancellationOrigin, PermissionAnswer, PermissionCancellation,
        PermissionCancellationRequest, PermissionResolution,
    },
};
use crate::domain::{
    agent_execution::{
        executions::ExecutionId, permissions::PermissionStateView, sessions::ExecutionSessionId,
    },
    effective_capabilities::value_objects::{
        CapabilityRequirement, EffectiveCapabilities, Modality,
    },
};
use std::sync::Arc;

/// Low-level adapter handle; SDK consumers should invoke the public Agent.
/// Adapters construct this handle; runtime controls are crate-private so callers
/// go through Agent storage, hooks, and scheduling.
/// A provider context client. Clones share its execution, shutdown, and restoration lifecycle.
/// Admission uses the same immutable capabilities selected at session creation.
/// The backend owns execution concurrency and cleanup; the host authorizes answers.
///
/// Consumer code cannot dispatch this handle directly:
/// ```compile_fail
/// use nessa_sdk::application::agent_execution::{
///     executions::ExecutionRequest, providers::ProviderSession,
/// };
/// fn bypass_agent(session: ProviderSession, request: ExecutionRequest) {
///     let _ = session.execute(request);
/// }
/// ```
#[derive(Clone)]
pub struct ProviderSession {
    id: ExecutionSessionId,
    backend: Arc<dyn ProviderSessionBackend>,
    capabilities: EffectiveCapabilities,
}
impl ProviderSession {
    /// Wrap `backend` with its provider context `id` and immutable model admission
    /// `capabilities`. Construction performs no I/O and opens no provider context.
    pub fn new(
        id: ExecutionSessionId,
        backend: Arc<dyn ProviderSessionBackend>,
        capabilities: EffectiveCapabilities,
    ) -> Self {
        Self {
            id,
            backend,
            capabilities,
        }
    }
    pub(crate) fn prepare_invocation(&self) -> ProviderOperationFuture<'_, ()> {
        Box::pin(async move { self.backend.prepare_invocation().await })
    }
    /// Provider context identity, distinct from the local storage session key.
    pub fn id(&self) -> &ExecutionSessionId {
        &self.id
    }
    /// Model admission limits selected for this attachment.
    pub fn capabilities(&self) -> &EffectiveCapabilities {
        &self.capabilities
    }
    /// Read the backend's current operation support without reconnecting.
    /// Unlike model capabilities, this snapshot may change after restoration.
    pub fn operation_capabilities(&self) -> OperationCapabilities {
        self.backend.operation_capabilities()
    }
    /// Send one new user message. The ACP backend automatically restores a closed
    /// context before execution; missing history or unsupported resume is an error.
    /// The provider retains conversation history.
    pub(crate) fn validate(&self, input: &ExecutionRequest) -> Result<(), AgentError> {
        input.validate_message_size()?;
        self.capabilities
            .validate(
                &[CapabilityRequirement::Input(Modality::Text)],
                input.estimated_input_tokens,
                input.reserved_output_tokens,
            )
            .map_err(|error| AgentError::InvalidInput(error.to_string()))
    }
    /// Dispatch `input` after checking actual message bytes and caller-estimated tokens.
    /// This adapter operation does not persist evidence or run Agent hooks.
    /// Poll the paired event reader concurrently and await backend settlement.
    pub(crate) fn execute(&self, input: ExecutionRequest) -> ProviderExecutionFuture<'_> {
        Box::pin(async move {
            if let Err(error) = self.validate(&input) {
                return ProviderExecutionReply::Rejected(error);
            }
            match self.backend.execute(input).await {
                ProviderExecutionReply::Rejected(error) => {
                    ProviderExecutionReply::Rejected(error.bounded())
                }
                report => report,
            }
        })
    }
    /// Steer an identified active invocation after the usual input admission checks.
    /// Provider acknowledgements distinguish injection from unconsumed idle input.
    pub(crate) fn steer(
        &self,
        target: ExecutionId,
        input: ExecutionRequest,
    ) -> ProviderOperationFuture<'_, SteeringOutcome> {
        Box::pin(async move {
            self.validate(&input).map_err(|error| {
                ProviderOperationFailure::new(error, ProviderSessionState::Usable)
            })?;
            self.backend.steer(target, input).await
        })
    }
    /// Delegate the exact reviewed choice and actor to the backend's audit and
    /// delivery path; reports correlation, policy, audit, and transport failures.
    /// Rejects returned evidence that disagrees with any submitted identity, choice,
    /// or attribution. This check cannot undo effects already performed by an adapter.
    pub(crate) fn answer_permission(
        &self,
        answer: PermissionAnswer,
    ) -> ProviderOperationFuture<'_, PermissionResolution> {
        Box::pin(async move {
            let resolution = self.backend.answer_permission(answer.clone()).await?;
            if resolution.session_id() != self.id()
                || resolution.request().execution_id() != &answer.execution_id
                || resolution.request().id() != &answer.id
                || resolution.attribution() != &answer.attribution
                || !matches!(resolution.request().state(), PermissionStateView::Answered { option_id, .. } if option_id == &answer.option_id)
            {
                return Err(ProviderOperationFailure::new(
                    AgentError::Protocol(
                        "permission resolution contradicts the submitted answer".into(),
                    ),
                    ProviderSessionState::CleanupRequired,
                ));
            }
            Ok(resolution)
        })
    }
    /// Withdraw one pending review with an explicit actor and reason.
    /// Returned evidence must match the submitted target, full reason, and actor.
    /// This does not claim that a running tool was stopped or rolled back.
    pub(crate) fn cancel_permission(
        &self,
        input: PermissionCancellationRequest,
    ) -> ProviderOperationFuture<'_, PermissionCancellation> {
        Box::pin(async move {
            let cancellation = self.backend.cancel_permission(input.clone()).await?;
            if cancellation.session_id() != self.id()
                || cancellation.request().execution_id() != &input.execution_id
                || cancellation.request().id() != &input.id
                || cancellation.origin() != &CancellationOrigin::Client(input.actor)
                || !matches!(cancellation.request().state(), PermissionStateView::Cancelled { reason } if reason == &input.reason)
            {
                return Err(ProviderOperationFailure::new(
                    AgentError::Protocol(
                        "permission cancellation contradicts the submitted withdrawal".into(),
                    ),
                    ProviderSessionState::CleanupRequired,
                ));
            }
            Ok(cancellation)
        })
    }
    /// Shut down the current live connection, including any active execution.
    /// The report independently records physical cleanup and audit acknowledgement.
    /// This does not delete saved provider history or a Nessa conversation.
    /// A later execute restores this context after verified cleanup; it does not
    /// silently create a replacement conversation.
    pub(crate) fn shutdown(&self, request: SessionCloseRequest) -> CleanupFuture<'_> {
        self.backend.close(request)
    }
}
