#![deny(missing_docs)]

use super::profile::{native_mode, CodexProfile};
use crate::application::agent_execution::agents::AgentError;
use crate::application::agent_execution::executions::ExecutionAudit;
use crate::application::agent_execution::providers::{
    AgentProvider, ApprovalMode, ProviderIdentity, ProviderOpenFuture, ProviderOpenRequest,
};
use crate::domain::agent_execution::permissions::PermissionScope;
use crate::domain::agent_execution::prompts::SystemPrompt;
use crate::domain::common::value_objects::TokenLimits;
use crate::domain::effective_capabilities::value_objects::{
    BindingRestrictions, EffectiveCapabilities,
};
use crate::domain::model_metadata::entities::ModelMetadata;
use crate::domain::model_metadata::value_objects::{
    EffortLevel, Modalities, ModelFeatures, ModelProvider,
};
use crate::infrastructure::acp::sessions::{
    binding as acp_binding, deletion::DeletionCleanups, identity, thought_level, AcpConfig,
};
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
    approval_mode: ApprovalMode,
    effort_level: Option<EffortLevel>,
    audit: Arc<dyn ExecutionAudit>,
    /// Deletions this binding started that are still stopping their process.
    deletions: DeletionCleanups,
}
impl CodexAcpProvider {
    /// Presets this binding has verified for an exact catalog model ID.
    pub fn approval_modes(
        model_id: &str,
    ) -> &'static [crate::application::agent_execution::providers::ApprovalModeChoice] {
        super::profile::approval_modes(model_id)
    }
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
    /// Every launch also requests Codex's user-configured hooks and its legacy
    /// notify command off (`features.hooks` false, `notify` empty). That request
    /// does not outrank legacy managed configuration, and ACP cannot show the
    /// effective setting, so a negotiated session reports native hook suppression
    /// as unsupported. The open still completes, and so does restoration.
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
            // An effort level is sent (`with_effort_level`); fast mode is not.
            ModelFeatures::new(text, text, true, true, false),
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
            approval_mode: ApprovalMode::Ask,
            effort_level: None,
            audit,
            deletions: DeletionCleanups::default(),
        })
    }
    /// Replace the harness's default instructions with these attributed ones.
    /// The metadata stays local; only composed text reaches the launched process.
    pub fn with_system_prompt(mut self, prompt: SystemPrompt) -> Self {
        self.system_prompt = Some(prompt);
        self
    }
    /// Select one binding-verified native preset for every session this factory opens.
    ///
    /// # Errors
    /// Returns [`AgentError::Unsupported`] when the exact model has not been
    /// verified for this preset. The factory is unchanged on failure.
    pub fn with_approval_mode(mut self, mode: ApprovalMode) -> Result<Self, AgentError> {
        if !Self::approval_modes(self.capabilities.model().model_id())
            .iter()
            .any(|choice| choice.id == mode)
        {
            return Err(AgentError::Unsupported(
                "Codex approval preset is unavailable for this model".into(),
            ));
        }
        self.approval_mode = mode;
        Ok(self)
    }
    /// Select one reasoning effort level for every session this factory opens.
    /// Without one, no level is sent and the agent keeps its own default.
    ///
    /// The level is sent once the session exists and read back from the
    /// agent's answer; an agent that does not offer it refuses it, and the
    /// session fails to open rather than run at another level. What the
    /// connected agent offers is known only then
    /// ([`OperationCapabilities::effort_levels`](crate::application::agent_execution::providers::OperationCapabilities::effort_levels)).
    ///
    /// # Errors
    /// Returns [`AgentError::Unsupported`] when `level` is not one of the
    /// model's catalogue levels this binding offers
    /// ([`EffectiveCapabilities::effort_levels`]). The factory is unchanged on
    /// failure.
    pub fn with_effort_level(mut self, level: EffortLevel) -> Result<Self, AgentError> {
        thought_level::selectable(&self.capabilities, &level)?;
        self.effort_level = Some(level);
        Ok(self)
    }
    /// Borrow the configured override and its contribution provenance, or None
    /// when opening should retain the harness default instructions.
    pub fn system_prompt(&self) -> Option<&SystemPrompt> {
        self.system_prompt.as_ref()
    }
    /// The Codex session configuration this binding launches with.
    ///
    /// Codex reads its session configuration from its own config file and this
    /// variable, not from the ACP session request. The pinned adapter parses
    /// `CODEX_CONFIG` as JSON and sends it as the `thread/start` config
    /// override, which the pinned loader applies as a session layer. The model
    /// and the instructions are supplied here and the model is read back over
    /// the protocol. `features.hooks` and `notify` are not read back: legacy
    /// managed configuration and MDM are applied after the session layer, and
    /// ACP exposes no effective value, so the profile reports native hook
    /// suppression as unsupported.
    fn session_config(&self) -> String {
        let mut config = json!({
            "model": self.capabilities.model().model_id(),
            // Ordinary configured handlers stop when the feature is false.
            // Provider builtin cleanup is a separate lifecycle path and is
            // not disabled here. Legacy notify is constructed independently
            // of the feature bit, so it is cleared on its own.
            "features": {"hooks": false},
            "notify": [],
        });
        if let Some(prompt) = &self.system_prompt {
            config["instructions"] = json!(prompt.text().as_str());
        }
        config.to_string()
    }
    fn launch_command(&self) -> Command {
        let mut command = Command::new(self.config.executable.executable());
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
            .env("INITIAL_AGENT_MODE", native_mode(self.approval_mode))
            // A gateway process has no browser and nobody watching it. Signing in
            // is the desktop's business, done before an agent is offered at all.
            .env("NO_BROWSER", "1");
        command
    }
}
impl AgentProvider for CodexAcpProvider {
    fn approval_mode(&self) -> Option<ApprovalMode> {
        Some(self.approval_mode)
    }
    fn effort_level(&self) -> Option<EffortLevel> {
        self.effort_level.clone()
    }
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
    fn open(&self, request: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        Box::pin(async move {
            acp_binding::open(
                self.process_factory(),
                self.config.clone(),
                self.capabilities.clone(),
                self.profile(),
                self.audit.clone(),
                request,
            )
            .await
        })
    }
}
impl CodexAcpProvider {
    /// The launch configuration every connection this provider opens uses.
    pub(super) fn config(&self) -> &AcpConfig {
        &self.config
    }
    /// Deletions this binding started that are still stopping their process.
    pub(super) fn deletions(&self) -> &DeletionCleanups {
        &self.deletions
    }
    /// The profile every connection this provider opens speaks.
    pub(super) fn profile(&self) -> CodexProfile {
        CodexProfile::new(
            self.capabilities.model().model_id(),
            self.approval_mode,
            self.effort_level.clone(),
        )
    }
    /// How every connection this provider opens is launched.
    pub(super) fn process_factory(&self) -> acp_binding::ProcessFactory {
        let factory = self.clone();
        Arc::new(move || ProcessScope::spawn(factory.launch_command()).map_err(Into::into))
    }
}
