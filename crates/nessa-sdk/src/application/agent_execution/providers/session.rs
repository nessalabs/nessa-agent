#![deny(missing_docs)]

use crate::domain::model_metadata::value_objects::EffortLevel;

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
use crate::domain::agent_execution::permissions::{
    PermissionAuthority, PermissionAuthorityError, PermissionId,
};
use crate::domain::{
    agent_execution::{
        executions::ExecutionId, permissions::PermissionStateView, sessions::ExecutionSessionId,
    },
    effective_capabilities::value_objects::{
        CapabilityError, CapabilityRequirement, EffectiveCapabilities, Modality,
    },
};
use std::sync::{Arc, RwLock};

/// Low-level adapter handle; SDK consumers should invoke the public Agent.
/// Adapters construct this handle; runtime controls are crate-private so callers
/// go through Agent storage, hooks, and scheduling.
/// A provider context client. Clones share its execution, shutdown, and restoration lifecycle.
/// Admission uses the operation-selected model capability snapshot,
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
    capabilities: Arc<RwLock<EffectiveCapabilities>>,
    input_files: Option<Arc<dyn super::InputFileResolver>>,
}
impl ProviderSession {
    pub(crate) fn permission_authority(
        &self,
    ) -> Result<Option<PermissionAuthority>, PermissionAuthorityError> {
        let authority = self.backend.permission_authority()?;
        if authority
            .as_ref()
            .is_some_and(|value| value.session_id() != self.id())
        {
            return Err(PermissionAuthorityError::IdentityMismatch);
        }
        Ok(authority)
    }
    pub(crate) fn supports_turn_cancel(&self) -> bool {
        self.backend.supports_turn_cancel()
    }
    pub(crate) fn cancel_turn(&self, turn: ExecutionId) -> ProviderOperationFuture<'_, ()> {
        Box::pin(async move { self.backend.cancel_turn(turn).await })
    }
    pub(crate) fn set_approval_mode(
        &self,
        mode: super::ApprovalMode,
    ) -> super::ProviderOperationFuture<'_, ()> {
        self.backend.set_approval_mode(mode)
    }
    pub(crate) fn set_effort_level(
        &self,
        level: EffortLevel,
    ) -> super::ProviderOperationFuture<'_, ()> {
        self.backend.set_effort_level(level)
    }
    /// Wrap `backend` with its provider context `id` and initial model admission
    /// `capabilities`, replaced by an SDK-owned acknowledged model change.
    /// Construction performs no I/O and opens no provider context.
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
            capabilities: Arc::new(RwLock::new(capabilities)),
            input_files: None,
        }
    }
    /// Bind stored-file preparation to this session's execution environment.
    /// Clones share this resolver. Accepted input and snapshots retain original
    /// references; only the provider delivery copy receives confirmed paths.
    #[must_use]
    pub fn with_input_files(self, resolver: Arc<dyn super::InputFileResolver>) -> Self {
        Self {
            input_files: Some(resolver),
            ..self
        }
    }
    async fn prepare_input(
        &self,
        mut input: ExecutionRequest,
    ) -> Result<ExecutionRequest, AgentError> {
        let references = input.user_message.uploads();
        if references.is_empty() {
            return Ok(input);
        }
        let source = self
            .input_files
            .as_ref()
            .ok_or(AgentError::UserFile(super::UserFileError::NotOffered))?;
        let prepared = source
            .resolve(input.execution_id.clone(), references.to_vec())
            .await?;
        if prepared.len() != references.len()
            || prepared
                .iter()
                .zip(references)
                .any(|(prepared, expected)| prepared.reference() != expected)
        {
            return Err(AgentError::UserFile(super::UserFileError::Mismatch));
        }
        let files = prepared
            .into_iter()
            .map(super::PreparedInputFile::into_file)
            .collect();
        input.user_message = input.user_message.delivered_uploads(files);
        Ok(input)
    }
    pub(crate) fn prepare_invocation(&self) -> ProviderOperationFuture<'_, ()> {
        Box::pin(async move { self.backend.prepare_invocation().await })
    }
    /// Provider context identity, distinct from the local storage session key.
    pub fn id(&self) -> &ExecutionSessionId {
        &self.id
    }
    /// Owned snapshot of the model admission limits currently acknowledged on this attachment.
    pub fn capabilities(&self) -> EffectiveCapabilities {
        self.capabilities
            .read()
            .expect("provider model capabilities")
            .clone()
    }
    pub(crate) async fn set_model(
        &self,
        change: super::ModelChange,
        capabilities: EffectiveCapabilities,
    ) -> Result<super::ModelChangeEffect, ProviderOperationFailure> {
        let effect = self.backend.set_model(change, capabilities).await?;
        *self
            .capabilities
            .write()
            .expect("provider model capabilities") = effect.capabilities().clone();
        Ok(effect)
    }
    /// Read the backend's current operation support without reconnecting.
    /// Unlike model capabilities, this snapshot may change after restoration.
    pub fn operation_capabilities(&self) -> OperationCapabilities {
        OperationCapabilities::resolve(self.backend.operation_capabilities())
    }
    /// Decide whether `input` may be accepted at all. The configured limits
    /// run before anything is saved, queued, or sent, at an immediate
    /// invocation, a queued one, queued steering, and native steering. The
    /// connected agent's image answer and the backend's own refusal run
    /// here, when the backend is called.
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
        self.validate_with_capabilities(&self.capabilities(), input)
    }
    fn validate_with_capabilities(
        &self,
        capabilities: &EffectiveCapabilities,
        input: &ExecutionRequest,
    ) -> Result<(), AgentError> {
        let images = input.user_message.images();
        validate_configured_input(capabilities, input)?;
        if !images.is_empty() {
            let agent = self.operation_capabilities();
            if agent.negotiated() && !agent.image_input() {
                return Err(AgentError::ImageInputRefused(
                    ImageInputRefusal::AgentDoesNotAccept,
                ));
            }
        }
        if !input.user_message.uploads().is_empty() && self.input_files.is_none() {
            return Err(AgentError::UserFile(super::UserFileError::NotOffered));
        }
        self.backend.validate_input(input, capabilities)
    }
    /// Dispatch `input` after checking actual message bytes and caller-estimated tokens.
    /// This adapter operation does not persist evidence or run Agent hooks.
    /// Poll the paired event reader concurrently and await backend settlement.
    pub(crate) fn execute(
        &self,
        input: ExecutionRequest,
        handoff: super::NativeExecutionHandoff,
    ) -> ProviderExecutionFuture<'_> {
        Box::pin(async move {
            if let Err(error) = self.validate(&input) {
                return ProviderExecutionReply::Rejected(error);
            }
            let session = self.clone();
            let input = Box::pin(async move {
                let input = session.prepare_input(input).await?;
                session.validate(&input)?;
                Ok(input)
            });
            match self.backend.execute(input, handoff).await {
                ProviderExecutionReply::Rejected(error) => {
                    ProviderExecutionReply::Rejected(error.bounded())
                }
                report => report,
            }
        })
    }
    /// Steer an identified active invocation using its selected admission binding.
    /// The caller retains that immutable binding across admission and delivery;
    /// the attachment's current selection may govern a later native turn.
    /// Provider acknowledgements distinguish injection from unconsumed idle input.
    /// `started` is the host's handoff, forwarded to the backend: see
    /// [`ProviderSessionBackend::steer`](super::ports::ProviderSessionBackend::steer).
    pub(crate) fn steer<'a>(
        &'a self,
        target: ExecutionId,
        input: ExecutionRequest,
        capabilities: &'a EffectiveCapabilities,
        started: &'a (dyn Fn() + Send + Sync),
    ) -> ProviderOperationFuture<'a, SteeringOutcome> {
        Box::pin(async move {
            if let Err(error) = self.validate_with_capabilities(capabilities, &input) {
                started();
                return Err(ProviderOperationFailure::new(
                    error,
                    ProviderSessionState::Usable,
                ));
            }
            let session = self.clone();
            let binding = capabilities.clone();
            let input = Box::pin(async move {
                let input = session.prepare_input(input).await?;
                session.validate_with_capabilities(&binding, &input)?;
                Ok(input)
            });
            self.backend
                .steer(target, input, capabilities, started)
                .await
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
    /// Ask the backend to hold remembered allows for permission-review hooks.
    ///
    /// See [`ProviderSessionBackend::hold_remembered_allows_for_review_hooks`].
    pub(crate) fn hold_remembered_allows_for_review_hooks(&self, hold: bool) {
        self.backend.hold_remembered_allows_for_review_hooks(hold);
    }

    /// See [`ProviderSessionBackend::consult_remembered_allow`].
    pub(crate) fn consult_remembered_allow(
        &self,
        id: &PermissionId,
    ) -> ProviderOperationFuture<'_, Option<PermissionAnswer>> {
        self.backend.consult_remembered_allow(id)
    }

    /// See [`ProviderSessionBackend::release_remembered_review`].
    pub(crate) fn release_remembered_review(
        &self,
        id: &PermissionId,
    ) -> ProviderOperationFuture<'_, ()> {
        self.backend.release_remembered_review(id)
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
    // App model contexts reach the agent as a leading text block, so a
    // message carrying them is text input even with no text of its own.
    if input.user_message.text().is_some()
        || !input.user_message.app_model_context().is_empty()
        || !input.user_message.files().is_empty()
        || !input.user_message.uploads().is_empty()
    {
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

#[cfg(test)]
#[path = "../../../../tests/application/agent_execution/providers/session.rs"]
mod tests;
