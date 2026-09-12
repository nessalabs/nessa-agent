use super::{
    AgentSession, BindingError, BindingFuture, PermissionAnswer, Prompt, PromptOutcome, StopOutcome,
};
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
    pub fn prompt(&self, input: Prompt) -> BindingFuture<'_, PromptOutcome> {
        Box::pin(async move {
            validate_prompt(&input, &self.capabilities)?;
            self.session.prompt(input).await
        })
    }
    pub fn answer_permission(&self, answer: PermissionAnswer) -> BindingFuture<'_, ()> {
        self.session.answer_permission(answer)
    }
    pub fn stop(&self) -> BindingFuture<'_, StopOutcome> {
        self.session.stop()
    }
}

/// Boundary structure is checked here; capability rules stay in the domain.
pub(crate) fn validate_prompt(
    input: &Prompt,
    capabilities: &EffectiveCapabilities,
) -> Result<(), BindingError> {
    if input.text.trim().is_empty()
        || input.execution_id.trim().is_empty()
        || input.execution_id.len() > 256
    {
        return Err(BindingError::InvalidInput(
            "text and a bounded execution ID are required".into(),
        ));
    }
    capabilities
        .validate(
            &[CapabilityRequirement::Input(Modality::Text)],
            input.input_tokens,
            input.reserved_output_tokens,
        )
        .map_err(|error| BindingError::InvalidInput(error.to_string()))
}
