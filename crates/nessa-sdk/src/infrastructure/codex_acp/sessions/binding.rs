#![deny(missing_docs)]

use super::profile::{CodexProfile, MODE};
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
use serde_json::json;
use std::sync::Arc;
use tokio::process::Command;

/// Immutable composition factory; opening twice creates independent process scopes.
#[derive(Clone)]
pub struct CodexAcpProvider {
    config: AcpConfig,
    capabilities: EffectiveCapabilities,
    system_prompt: Option<SystemPrompt>,
    audit: Arc<dyn ExecutionAudit>,
}
impl CodexAcpProvider {
    /// Configure a Codex ACP execution factory without starting a process.
    ///
    /// `config` supplies the executable, isolated environment, workspace, permission
    /// offers, and runtime bounds. `model` must be validated OpenAI metadata;
    /// `limits` selects context/output token ceilings within model and binding
    /// support. `audit` receives mandatory permission evidence independently of
    /// event consumers; its implementation defines durability and must remain live.
    /// Each subsequent provider open owns a separate process scope. This factory
    /// retains its configuration, capability snapshot, and shared audit adapter.
    ///
    /// Codex runs its own shell and patch tools inside a sandbox of its own and
    /// offers no way for a client to remove them. This binding therefore opens
    /// every session in Codex's least permissive preset, in which anything
    /// leaving the workspace sandbox — a command outside it, a file outside it,
    /// the network — is asked for and answered through Nessa's permission owner.
    /// Work Codex does inside that sandbox is observed and audited, not reviewed
    /// beforehand. Configuring a binding with tools disabled is refused rather
    /// than accepted as a text-only Codex that does not exist.
    ///
    /// # Errors
    /// Returns [`AgentError::Configuration`] for invalid process settings, a
    /// non-OpenAI model, an oversized provider model identity, or incompatible
    /// token limits.
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
            return Err(AgentError::Unsupported("Codex permissions currently support exact-request scope only; session/application rule enforcement is not implemented".into()));
        }
        if !config.tools_enabled {
            return Err(AgentError::Unsupported(
                "Codex always runs its own tools; a text-only Codex binding cannot be configured"
                    .into(),
            ));
        }
        if model.key().provider() != ModelProvider::OpenAi {
            return Err(AgentError::Configuration(
                "Codex requires an OpenAI model".into(),
            ));
        }
        let text = Modalities::new(true, false, false).expect("text modality is nonempty");
        let restrictions = BindingRestrictions::new(
            ModelFeatures::new(text, text, true, false),
            // Binding ceilings for this first profile, not model or Codex facts.
            // Codex owns its own context window and compacts it without telling
            // Nessa, and takes no per-turn output limit through ACP, so these
            // bound what this binding admits rather than what the provider does.
            TokenLimits::new(200_000, 64_000).expect("valid native profile ceilings"),
        );
        let capabilities = EffectiveCapabilities::new(model, restrictions, limits)
            .map_err(|e| AgentError::Configuration(e.to_string()))?;
        if !capabilities.features().tool_use() {
            return Err(AgentError::Configuration(
                "selected model does not support tools".into(),
            ));
        }
        ProviderIdentity::new("codex-acp", capabilities.model().model_id(), "")?;
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
    /// The Codex session configuration this binding launches with.
    ///
    /// Codex reads its session configuration from its own config file and this
    /// variable, not from the ACP session request, so the model and the
    /// instructions are supplied here. Both are then read back and checked over
    /// the protocol: what is set at launch is a request, and what the provider
    /// reports is the answer.
    fn session_config(&self) -> String {
        let mut config = json!({"model": self.capabilities.model().model_id()});
        if let Some(prompt) = &self.system_prompt {
            config["instructions"] = json!(prompt.text().as_str());
        }
        config.to_string()
    }
    fn launch_command(&self) -> Command {
        let mut command = Command::new(&self.config.executable);
        command
            .args(&self.config.arguments)
            .current_dir(&self.config.workspace)
            .env_clear()
            .envs(&self.config.environment)
            .envs(&self.config.credential_environment)
            .env("CODEX_CONFIG", self.session_config())
            // The preset every session is then explicitly selected into. Setting
            // it here as well means the session is never briefly open in a more
            // permissive one.
            .env("INITIAL_AGENT_MODE", MODE)
            // A gateway process has no browser and nobody watching it. Signing in
            // is the desktop's business, done before an agent is offered at all.
            .env("NO_BROWSER", "1");
        command
    }
}
impl AgentProvider for CodexAcpProvider {
    fn identity(&self) -> ProviderIdentity {
        ProviderIdentity::new(
            "codex-acp",
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
    fn open(&self, restore: Option<ExecutionSessionId>) -> ProviderOpenFuture<'_> {
        Box::pin(async move {
            let factory = self.clone();
            acp_binding::open(
                Arc::new(move || ProcessScope::spawn(factory.launch_command())),
                self.config.clone(),
                self.capabilities.clone(),
                CodexProfile::new(self.capabilities.model().model_id()),
                self.audit.clone(),
                restore,
            )
            .await
        })
    }
}
