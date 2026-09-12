use super::{
    AgentSession, BindingError, BindingFuture, PermissionAnswer, PromptRequest, StopOutcome,
};
use crate::domain::agent_execution::value_objects::*;
use crate::domain::effective_capabilities::value_objects::{
    CapabilityRequirement, EffectiveCapabilities, Modality,
};
use std::sync::Arc;

/// Admission and execution use the same immutable capability snapshot.
/// Permission answers must already be authorized by the embedding host.
pub struct Agent {
    session: Arc<dyn AgentSession>,
    capabilities: EffectiveCapabilities,
}
impl Agent {
    pub fn new(session: Arc<dyn AgentSession>, capabilities: EffectiveCapabilities) -> Self {
        Self {
            session,
            capabilities,
        }
    }
    pub fn capabilities(&self) -> &EffectiveCapabilities {
        &self.capabilities
    }
    pub fn prompt(&self, input: PromptRequest) -> BindingFuture<'_, PromptOutcome> {
        Box::pin(async move {
            validate_prompt(&input, &self.capabilities)?;
            self.session.prompt(input).await
        })
    }
    pub fn answer_permission(&self, answer: PermissionAnswer) -> BindingFuture<'_, ()> {
        Box::pin(async move {
            answer.to_domain()?;
            self.session.answer_permission(answer).await
        })
    }
    pub fn stop(&self) -> BindingFuture<'_, StopOutcome> {
        self.session.stop()
    }
}

/// Boundary structure is checked here; capability rules stay in the domain.
pub(crate) fn validate_prompt(
    input: &PromptRequest,
    capabilities: &EffectiveCapabilities,
) -> Result<(), BindingError> {
    input.execution_id()?;
    capabilities
        .validate(
            &[CapabilityRequirement::Input(Modality::Text)],
            input.input_tokens,
            input.reserved_output_tokens,
        )
        .map_err(|error| BindingError::InvalidInput(error.to_string()))
}
