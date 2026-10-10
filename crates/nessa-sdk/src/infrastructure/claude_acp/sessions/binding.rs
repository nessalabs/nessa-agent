#![deny(missing_docs)]

use super::profile::ClaudeProfile;
use crate::application::agent_execution::agents::AgentError;
use crate::application::agent_execution::executions::ExecutionAudit;
use crate::application::agent_execution::providers::{
    AgentProvider, ApprovalMode, HarnessHost, HarnessLaunch, ProviderIdentity, ProviderOpenFuture,
    ProviderOpenRequest,
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
use std::{collections::BTreeMap, ffi::OsString, sync::Arc};
use tokio::process::Command;

/// Immutable composition factory; opening twice creates independent process scopes.
#[derive(Clone)]
pub struct ClaudeAcpProvider {
    config: AcpConfig,
    capabilities: EffectiveCapabilities,
    system_prompt: Option<SystemPrompt>,
    approval_mode: ApprovalMode,
    effort_level: Option<EffortLevel>,
    audit: Arc<dyn ExecutionAudit>,
    /// Deletions this binding started that are still stopping their process.
    deletions: DeletionCleanups,
    /// Where its harness is started when that is not this machine
    /// ([`AgentProvider::on_host`]).
    host: Option<Arc<dyn HarnessHost>>,
    #[cfg(test)]
    process: Option<acp_binding::ProcessFactory>,
}
impl ClaudeAcpProvider {
    /// The variables this binding sets for one launch of its harness, and
    /// the only ones a host that starts the harness for it accepts from it
    /// (`HarnessLaunch`).
    pub const LAUNCH_VARIABLES: &'static [&'static str] = &[
        "ANTHROPIC_MODEL",
        "ANTHROPIC_CUSTOM_MODEL_OPTION",
        "CLAUDE_CODE_MAX_OUTPUT_TOKENS",
        "DISABLE_AUTOUPDATER",
        "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC",
    ];
    /// The sandbox profiles this binding can set up: only the harness's own
    /// default today, since it configures no sandbox of its own. A lease that
    /// asks for any other is refused, never run under a weaker one.
    pub const SANDBOX_PROFILES: crate::domain::agent_execution::leases::SandboxProfiles =
        crate::domain::agent_execution::leases::SandboxProfiles::HARNESS_DEFAULT;
    /// Presets this binding has verified for an exact catalog model ID.
    pub fn approval_modes(
        model_id: &str,
    ) -> &'static [crate::application::agent_execution::providers::ApprovalModeChoice] {
        super::profile::approval_modes(model_id)
    }
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
            // An effort level is sent (`with_effort_level`); fast mode is not.
            ModelFeatures::new(input, text, config.tools_enabled, true, false),
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
            approval_mode: ApprovalMode::Ask,
            effort_level: None,
            audit,
            deletions: DeletionCleanups::default(),
            host: None,
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
                "Claude approval preset is unavailable for this model".into(),
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
    /// when opening should retain the harness default prompt.
    pub fn system_prompt(&self) -> Option<&SystemPrompt> {
        self.system_prompt.as_ref()
    }
    /// What this binding sets for every launch of its harness, wherever it
    /// runs: the model, its output budget, and no self-update or optional
    /// traffic. Never a credential or a path.
    fn launch_environment(&self) -> BTreeMap<OsString, OsString> {
        let model = self.capabilities.model().model_id();
        [
            ("ANTHROPIC_MODEL", model.to_owned()),
            ("ANTHROPIC_CUSTOM_MODEL_OPTION", model.to_owned()),
            (
                "CLAUDE_CODE_MAX_OUTPUT_TOKENS",
                self.capabilities.limits().max_output().to_string(),
            ),
            ("DISABLE_AUTOUPDATER", "1".to_owned()),
            ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1".to_owned()),
        ]
        .into_iter()
        .inspect(|(key, _)| debug_assert!(Self::LAUNCH_VARIABLES.contains(key)))
        .map(|(key, value)| (key.into(), value.into()))
        .collect()
    }
    fn launch_command(&self) -> Command {
        let mut command = Command::new(self.config.executable.executable());
        command
            .args(&self.config.arguments)
            .current_dir(&self.config.workspace)
            .env_clear()
            .envs(&self.config.environment)
            .envs(&self.config.credential_environment)
            .envs(self.launch_environment());
        command
    }
}
impl AgentProvider for ClaudeAcpProvider {
    fn approval_mode(&self) -> Option<ApprovalMode> {
        Some(self.approval_mode)
    }
    fn effort_level(&self) -> Option<EffortLevel> {
        self.effort_level.clone()
    }
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
    fn on_host(&self, host: Arc<dyn HarnessHost>) -> Result<Arc<dyn AgentProvider>, AgentError> {
        let config = self.config.on_host(host.workspace().to_path_buf());
        config.validate()?;
        Ok(Arc::new(Self {
            config,
            host: Some(host),
            ..self.clone()
        }))
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
        match &self.host {
            Some(host) => {
                let host = host.clone();
                Arc::new(move || {
                    let launch = HarnessLaunch {
                        environment: factory.launch_environment(),
                    };
                    Ok(ProcessScope::remote(host.start(launch)?))
                })
            }
            None => {
                Arc::new(move || ProcessScope::spawn(factory.launch_command()).map_err(Into::into))
            }
        }
    }
    pub(super) fn profile(&self) -> ClaudeProfile {
        ClaudeProfile::new(
            self.system_prompt.clone(),
            self.approval_mode,
            self.effort_level.clone(),
        )
    }
}
