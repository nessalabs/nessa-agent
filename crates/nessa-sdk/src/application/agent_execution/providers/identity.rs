#![deny(missing_docs)]

use crate::application::agent_execution::agents::AgentError;

/// Stable, bounded resume identity, compared exactly before attaching saved history.
/// Context describes provider-specific workspace/settings. Never include credentials.
/// Owned strings are compact: caller spare capacity is discarded on construction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderIdentity {
    name: Box<str>,
    model_id: Box<str>,
    context: Box<str>,
}
impl ProviderIdentity {
    /// Maximum UTF-8 bytes in the provider implementation discriminator.
    pub const MAX_NAME_BYTES: usize = 256;
    /// Maximum UTF-8 bytes in the exact model selection.
    pub const MAX_MODEL_ID_BYTES: usize = 256;
    /// Maximum UTF-8 bytes in the credential-free context description/fingerprint.
    pub const MAX_CONTEXT_BYTES: usize = 4096;

    /// Own and validate the three exact resume identity components without I/O.
    ///
    /// `name` identifies the provider implementation; `model_id` is the exact
    /// selected model. Both must be nonempty and contain no control characters.
    /// `context` is a stable, credential-free fingerprint or description of
    /// workspace/settings that affect resume semantics; it may be empty when
    /// the provider has no additional settings, but cannot contain NUL.
    /// No component is trimmed, normalized, hashed, or truncated here.
    ///
    /// Returns [`AgentError::Configuration`] when a component violates these
    /// constraints or its respective `MAX_*_BYTES` limit. On success, retains
    /// at most 4608 string payload bytes, independent of input spare capacity.
    ///
    /// ```
    /// use nessa_sdk::application::agent_execution::providers::ProviderIdentity;
    /// let identity = ProviderIdentity::new("fixture", "exact-model", "workspace:local")?;
    /// assert_eq!(identity.model_id(), "exact-model");
    /// # Ok::<(), nessa_sdk::application::agent_execution::agents::AgentError>(())
    /// ```
    pub fn new(
        name: impl Into<String>,
        model_id: impl Into<String>,
        context: impl Into<String>,
    ) -> Result<Self, AgentError> {
        let name = name.into();
        let model_id = model_id.into();
        let context = context.into();
        for (label, value, limit) in [
            ("name", &name, Self::MAX_NAME_BYTES),
            ("model_id", &model_id, Self::MAX_MODEL_ID_BYTES),
            ("context", &context, Self::MAX_CONTEXT_BYTES),
        ] {
            if value.len() > limit
                || value.contains('\0')
                || (label != "context" && (value.is_empty() || value.chars().any(char::is_control)))
            {
                return Err(AgentError::Configuration(format!(
                    "invalid provider identity {label}"
                )));
            }
        }
        Ok(Self {
            name: name.into_boxed_str(),
            model_id: model_id.into_boxed_str(),
            context: context.into_boxed_str(),
        })
    }
    /// Borrow the exact provider implementation discriminator.
    pub fn name(&self) -> &str {
        &self.name
    }
    /// Borrow the exact selected model identifier.
    pub fn model_id(&self) -> &str {
        &self.model_id
    }
    /// Borrow the exact credential-free configuration fingerprint or description.
    pub fn context(&self) -> &str {
        &self.context
    }
}

#[cfg(test)]
#[path = "../../../../tests/application/agent_execution/providers/identity.rs"]
mod tests;
