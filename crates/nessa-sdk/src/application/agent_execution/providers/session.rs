#![deny(missing_docs)]

use super::{
    CleanupFuture, ImageInputRefusal, OperationCapabilities, ProviderExecutionFuture,
    ProviderExecutionReply, ProviderOperationFailure, ProviderOperationFuture,
    ProviderSessionBackend, ProviderSessionState, SessionCloseRequest, SteeringOutcome,
};
use crate::application::agent_execution::agents::AgentError;
use crate::application::agent_execution::{
    executions::ExecutionRequest,
    permissions::{
        CancellationOrigin, PermissionAnswer, PermissionCancellation,
        PermissionCancellationRequest, PermissionResolution, PermissionSelectionState,
        QuestionAnswer,
    },
};
use crate::domain::{
    agent_execution::{
        executions::ExecutionId, permissions::PermissionStateView, sessions::ExecutionSessionId,
    },
    effective_capabilities::value_objects::{
        CapabilityError, CapabilityRequirement, EffectiveCapabilities, Modality,
    },
};
use std::sync::Arc;

/// Low-level adapter handle; SDK consumers should invoke the public Agent.
/// Adapters construct this handle; runtime controls are crate-private so callers
/// go through Agent storage, hooks, and scheduling.
/// A provider context client. Clones share its execution, shutdown, and restoration lifecycle.
/// Admission uses the same immutable capabilities selected at session creation,
/// the connected agent's negotiated answer on images when that is known, and the
/// backend's own `validate_input`.
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
    /// The provider backend retains the audit sink used for provider effects; the
    /// owning [`Agent`](crate::application::agent_execution::agents::Agent) receives
    /// the application audit sink when it is prepared.
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
        OperationCapabilities::resolve(self.backend.operation_capabilities())
    }
    /// Decide whether `input` may be accepted at all. Every way in runs this
    /// before anything is saved, queued, or sent: an immediate invocation, a
    /// queued one, queued steering, and native steering, and again when the
    /// backend is finally called.
    ///
    /// In order: the text's byte limit; the modalities and token budget of the
    /// selected model; each image against that model's recorded limits; the
    /// connected agent's own answer on images; then whatever the backend
    /// already knows it could never deliver.
    ///
    /// The agent's answer is refused only when it is a known "no". While a
    /// context is being opened or restored the answer is not known, and that is
    /// not a refusal: the message is admitted, and the adapter answers the same
    /// typed [`ImageInputRefusal::AgentDoesNotAccept`] at dispatch if the
    /// restored agent takes no images. A closed context keeps its last
    /// negotiation, because the same agent is what a restoration would start.
    pub(crate) fn validate(&self, input: &ExecutionRequest) -> Result<(), AgentError> {
        let images = input.user_message.images();
        validate_configured_input(&self.capabilities, input)?;
        if !images.is_empty() {
            let agent = self.operation_capabilities();
            if agent.negotiated() && !agent.image_input() {
                return Err(AgentError::ImageInputRefused(
                    ImageInputRefusal::AgentDoesNotAccept,
                ));
            }
        }
        self.backend.validate_input(input)
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
    /// Answer one question the agent asked, through the backend that holds it.
    ///
    /// There is no returned evidence to check: an answer is content the agent
    /// asked for, not a decision it reports back, so what this confirms is that
    /// the answer was written.
    pub(crate) fn answer_question(
        &self,
        answer: QuestionAnswer,
    ) -> ProviderOperationFuture<'_, ()> {
        Box::pin(async move { self.backend.answer_question(answer).await })
    }
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
                return Err(ProviderOperationFailure::permission_answer(
                    AgentError::Protocol(
                        "permission resolution contradicts the submitted answer".into(),
                    ),
                    ProviderSessionState::CleanupRequired,
                    PermissionSelectionState::Consumed,
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

pub(crate) fn validate_configured_input(
    capabilities: &EffectiveCapabilities,
    input: &ExecutionRequest,
) -> Result<(), AgentError> {
    input.validate_message_size()?;
    let images = input.user_message.images();
    let mut requirements = Vec::with_capacity(2);
    if input.user_message.text().is_some() {
        requirements.push(CapabilityRequirement::Input(Modality::Text));
    }
    if !images.is_empty() {
        requirements.push(CapabilityRequirement::Input(Modality::Image));
    }
    capabilities
        .validate(
            &requirements,
            input.estimated_input_tokens,
            input.reserved_output_tokens,
        )
        .map_err(offered_image_input_or)?;
    if !images.is_empty() {
        let Some(limits) = capabilities.image_input() else {
            return Err(AgentError::ImageInputRefused(ImageInputRefusal::NotOffered));
        };
        for image in images {
            limits
                .check(image.media_type(), image.size())
                .map_err(|violation| AgentError::ImageInputRefused(violation.into()))?;
        }
    }
    Ok(())
}

/// One unmet requirement as the caller's typed answer: an image the attachment
/// never offered to carry is the same fact as an image the agent refuses, and
/// is reported as such rather than as a sentence inside `InvalidInput`.
fn offered_image_input_or(error: CapabilityError) -> AgentError {
    match error {
        CapabilityError::Unsupported(CapabilityRequirement::Input(Modality::Image)) => {
            AgentError::ImageInputRefused(ImageInputRefusal::NotOffered)
        }
        other => AgentError::InvalidInput(other.to_string()),
    }
}
