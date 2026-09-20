#![deny(missing_docs)]

use super::profile::OpencodeProfile;
use crate::application::agent_execution::agents::AgentError;
use crate::application::agent_execution::executions::ExecutionAudit;
use crate::application::agent_execution::providers::{
    AgentProvider, ProviderIdentity, ProviderOpenFuture,
};
use crate::domain::agent_execution::permissions::PermissionScope;
use crate::domain::agent_execution::prompts::SystemPrompt;
use crate::domain::agent_execution::sessions::ExecutionSessionId;
use crate::domain::common::value_objects::TokenLimits;
use crate::domain::effective_capabilities::value_objects::{
    BindingRestrictions, EffectiveCapabilities,
};
use crate::domain::model_metadata::entities::ModelMetadata;
use crate::domain::model_metadata::value_objects::{Modalities, ModelFeatures, ModelProvider};
use crate::infrastructure::acp::sessions::{binding as acp_binding, identity, AcpConfig};
use crate::infrastructure::process::ProcessScope;
use std::sync::Arc;
use tokio::process::Command;

/// Immutable composition factory; opening twice creates independent process scopes.
#[derive(Clone)]
pub struct OpencodeAcpProvider {
    config: AcpConfig,
    capabilities: EffectiveCapabilities,
    system_prompt: Option<SystemPrompt>,
    audit: Arc<dyn ExecutionAudit>,
}

impl OpencodeAcpProvider {
    /// Configure an Opencode ACP execution factory without starting a process.
    ///
    /// `config` supplies the executable, isolated environment, workspace,
    /// permission offers, and runtime bounds. `model` must be validated
    /// OpenCode Zen metadata; `limits` selects context/output token ceilings
    /// within model and binding support. `audit` receives mandatory permission
    /// evidence independently of event consumers; its implementation defines
    /// durability and must remain live. Each subsequent provider open owns a
    /// separate process scope. This factory retains its configuration,
    /// capability snapshot, and shared audit adapter.
    ///
    /// Opencode brings its own tools and offers its session policy as a config
    /// option, so every session is opened in the less permissive of the two it
    /// has — one that reads and plans without editing the workspace or running
    /// commands in it. Configuring a binding with tools disabled is refused
    /// rather than accepted as a text-only Opencode that does not exist.
    ///
    /// # Errors
    /// Returns [`AgentError::Configuration`] for invalid process settings, a
    /// model served by anything but OpenCode Zen, an oversized provider model
    /// identity, or incompatible token limits.
    /// Returns [`AgentError::Unsupported`] for reusable permission scopes, which
    /// this profile cannot enforce, and for a binding with tools disabled. No
    /// provider or model request occurs here.
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
            return Err(AgentError::Unsupported("Opencode permissions currently support exact-request scope only; session/application rule enforcement is not implemented".into()));
        }
        if !config.tools_enabled {
            return Err(AgentError::Unsupported(
                "Opencode always runs its own tools; a text-only Opencode binding cannot be configured"
                    .into(),
            ));
        }
        if model.key().provider() != ModelProvider::Opencode {
            return Err(AgentError::Configuration(
                "Opencode requires a model served through OpenCode Zen".into(),
            ));
        }
        let text = Modalities::new(true, false, false).expect("text modality is nonempty");
        // Opencode's `initialize` advertises image prompts, so a picture can be
        // sent; nothing it serves sends one back.
        let prompt = Modalities::new(true, true, false).expect("text modality is nonempty");
        let restrictions = BindingRestrictions::new(
            ModelFeatures::new(prompt, text, true, false),
            // Binding ceilings, not model or Opencode facts. OpenCode Zen
            // reports nothing about the windows of the models it serves — and
            // rotates which ones it serves — so these bound what this binding
            // admits rather than what the provider does. Lower than the Codex
            // binding's on purpose: a ceiling this binding cannot check is one
            // to be conservative with.
            TokenLimits::new(128_000, 32_000).expect("valid native profile ceilings"),
        );
        let capabilities = EffectiveCapabilities::new(model, restrictions, limits)
            .map_err(|e| AgentError::Configuration(e.to_string()))?;
        if !capabilities.features().tool_use() {
            return Err(AgentError::Configuration(
                "selected model does not support tools".into(),
            ));
        }
        ProviderIdentity::new("opencode-acp", capabilities.model().model_id(), "")?;
        Ok(Self {
            config,
            capabilities,
            system_prompt: None,
            audit,
        })
    }

    /// Replace the harness's default instructions with these attributed ones.
    /// The metadata stays local; only composed text reaches the launched process.
    pub fn with_system_prompt(mut self, prompt: SystemPrompt) -> Self {
        self.system_prompt = Some(prompt);
        self
    }

    /// Borrow the configured override and its contribution provenance, or None
    /// when opening should retain the harness default instructions.
    pub fn system_prompt(&self) -> Option<&SystemPrompt> {
        self.system_prompt.as_ref()
    }

    fn launch_command(&self) -> Command {
        let mut command = Command::new(&self.config.executable);
        command
            .args(&self.config.arguments)
            .current_dir(&self.config.workspace)
            .env_clear()
            .envs(&self.config.environment)
            .envs(&self.config.credential_environment)
            // A gateway process has no browser and nobody watching it. Signing
            // in is the desktop's business, done before an agent is offered at
            // all — and Opencode needs none of it to reach the free models this
            // binding is here for.
            .env("NO_BROWSER", "1");
        command
    }
}

impl AgentProvider for OpencodeAcpProvider {
    fn identity(&self) -> ProviderIdentity {
        ProviderIdentity::new(
            "opencode-acp",
            self.capabilities.model().model_id(),
            identity::fingerprint(
                &self.config,
                self.capabilities.limits(),
                self.system_prompt.as_ref(),
            ),
        )
        .expect("validated model and fixed-size context fingerprint")
    }

    fn open(&self, restore: Option<ExecutionSessionId>) -> ProviderOpenFuture<'_> {
        Box::pin(async move {
            let factory = self.clone();
            acp_binding::open(
                Arc::new(move || ProcessScope::spawn(factory.launch_command())),
                self.config.clone(),
                self.capabilities.clone(),
                OpencodeProfile::new(self.capabilities.model().model_id()),
                self.audit.clone(),
                restore,
            )
            .await
        })
    }
}
