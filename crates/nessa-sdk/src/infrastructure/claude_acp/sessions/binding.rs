#![deny(missing_docs)]

use super::profile::ClaudeProfile;
use crate::application::agent_execution::agents::AgentError;
use crate::application::agent_execution::executions::ExecutionAudit;
use crate::application::agent_execution::providers::{
    AgentProvider, ProviderIdentity, ProviderOpenFuture, ProviderOpenRequest,
};
use crate::domain::agent_execution::permissions::PermissionScope;
use crate::domain::agent_execution::prompts::SystemPrompt;
use crate::domain::common::value_objects::TokenLimits;
use crate::domain::effective_capabilities::value_objects::{
    BindingRestrictions, EffectiveCapabilities,
};
use crate::domain::model_metadata::entities::ModelMetadata;
use crate::domain::model_metadata::value_objects::{Modalities, ModelFeatures, ModelProvider};
use crate::infrastructure::acp::sessions::{
    binding as acp_binding, deletion::DeletionCleanups, identity, AcpConfig,
};
use crate::infrastructure::process::ProcessScope;
use std::sync::Arc;
use tokio::process::Command;

/// Immutable composition factory; opening twice creates independent process scopes.
#[derive(Clone)]
pub struct ClaudeAcpProvider {
    config: AcpConfig,
    capabilities: EffectiveCapabilities,
    system_prompt: Option<SystemPrompt>,
    audit: Arc<dyn ExecutionAudit>,
    /// Deletions this binding started that are still stopping their process.
    deletions: DeletionCleanups,
    #[cfg(test)]
    process: Option<acp_binding::ProcessFactory>,
}
impl ClaudeAcpProvider {
    /// Configure a Claude ACP execution factory without starting a process.
    ///
    /// `config` supplies the executable, isolated environment, workspace, permission
    /// offers, and runtime bounds. `model` must be validated Anthropic metadata;
    /// `limits` selects context/output token ceilings within model and binding
    /// support. `audit` receives mandatory permission evidence independently of
    /// event consumers; its implementation defines durability and must remain live.
    /// Each subsequent provider open owns a separate process scope. This factory
    /// retains its configuration, capability snapshot, and shared audit adapter.
    ///
    /// # Errors
    /// Returns [`AgentError::Configuration`] for invalid process settings, a
    /// non-Anthropic model, an oversized provider model identity, unsupported tools,
    /// or incompatible token limits.
    /// Returns [`AgentError::Unsupported`] for reusable permission scopes, which
    /// this profile cannot enforce. No provider or model request occurs here.
    pub fn new(
        config: AcpConfig,
        model: &ModelMetadata,
        limits: TokenLimits,
        audit: Arc<dyn ExecutionAudit>,
    ) -> Result<Self, AgentError> {
        config.validate()?;
        if config
            .permissions
            .decisions()
            .iter()
            .any(|decision| decision.scope() != &PermissionScope::request())
        {
            return Err(AgentError::Unsupported("Claude permissions currently support exact-request scope only; session/application rule enforcement is not implemented".into()));
        }
        if model.key().provider() != ModelProvider::Anthropic {
            return Err(AgentError::Configuration(
                "Claude requires an Anthropic model".into(),
            ));
        }
        let text = Modalities::new(true, false, false).expect("text modality is nonempty");
        // Image input is offered only when composition supplied the bytes'
        // source. The model's own metadata and the connected agent's advertised
        // prompt capabilities narrow it further; neither can widen it.
        let input = Modalities::new(true, config.images.is_some(), false)
            .expect("text modality is nonempty");
        let restrictions = BindingRestrictions::new(
            ModelFeatures::new(input, text, config.tools_enabled, false),
            // This first profile deliberately excludes extended context and
            // larger output modes. These are binding ceilings, not model facts.
            TokenLimits::new(200_000, 64_000).expect("valid native profile ceilings"),
        );
        let capabilities = EffectiveCapabilities::new(model, restrictions, limits)
            .map_err(|e| AgentError::Configuration(e.to_string()))?;
        if config.tools_enabled && !capabilities.features().tool_use() {
            return Err(AgentError::Configuration(
                "selected model does not support tools".into(),
            ));
        }
        ProviderIdentity::new("claude-acp", capabilities.model().model_id(), "")?;
        Ok(Self {
            config,
            capabilities,
            system_prompt: None,
            audit,
            deletions: DeletionCleanups::default(),
            #[cfg(test)]
            process: None,
        })
    }
    #[cfg(all(test, unix))]
    pub(crate) fn with_process_factory(mut self, process: acp_binding::ProcessFactory) -> Self {
        self.process = Some(process);
        self
    }
    /// Replace the harness's default system prompt with these attributed instructions.
    /// The metadata stays local; only composed text is sent when opening a session.
    pub fn with_system_prompt(mut self, prompt: SystemPrompt) -> Self {
        self.system_prompt = Some(prompt);
        self
    }
    /// Borrow the configured override and its contribution provenance, or None
    /// when opening should retain the harness default prompt.
    pub fn system_prompt(&self) -> Option<&SystemPrompt> {
        self.system_prompt.as_ref()
    }
    fn launch_command(&self) -> Command {
        let mut command = Command::new(self.config.executable.executable());
        command
            .args(&self.config.arguments)
            .current_dir(&self.config.workspace)
            .env_clear()
            .envs(&self.config.environment)
            .envs(&self.config.credential_environment)
            .env("ANTHROPIC_MODEL", self.capabilities.model().model_id())
            .env(
                "ANTHROPIC_CUSTOM_MODEL_OPTION",
                self.capabilities.model().model_id(),
            )
            .env(
                "CLAUDE_CODE_MAX_OUTPUT_TOKENS",
                self.capabilities.limits().max_output().to_string(),
            )
            .env("DISABLE_AUTOUPDATER", "1")
            .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1");
        command
    }
}
impl AgentProvider for ClaudeAcpProvider {
    fn identity(&self) -> ProviderIdentity {
        ProviderIdentity::new(
            "claude-acp",
            self.capabilities.model().model_id(),
            identity::fingerprint(
                &self.config,
                self.capabilities.limits(),
                self.system_prompt.as_ref(),
            ),
        )
        .expect("validated model and fixed-size context fingerprint")
    }
    fn capabilities(&self) -> &EffectiveCapabilities {
        &self.capabilities
    }
    fn open(&self, request: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        Box::pin(async move {
            let (restore, control) = request.into_parts();
            acp_binding::open(
                self.process_factory(),
                self.config.clone(),
                self.capabilities.clone(),
                self.profile(),
                self.audit.clone(),
                restore,
                control,
            )
            .await
        })
    }
}
impl ClaudeAcpProvider {
    /// The launch configuration every connection this provider opens uses.
    pub(super) fn config(&self) -> &AcpConfig {
        &self.config
    }
    /// Deletions this binding started that are still stopping their process.
    pub(super) fn deletions(&self) -> &DeletionCleanups {
        &self.deletions
    }
    /// How every connection this provider opens is launched.
    pub(super) fn process_factory(&self) -> acp_binding::ProcessFactory {
        let factory = self.clone();
        #[cfg(test)]
        if let Some(process) = self.process.clone() {
            return process;
        }
        Arc::new(move || ProcessScope::spawn(factory.launch_command()).map_err(Into::into))
    }
    pub(super) fn profile(&self) -> ClaudeProfile {
        ClaudeProfile::new(self.system_prompt.clone()).with_mcp_servers(&self.config)
    }
}
