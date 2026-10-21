#![deny(missing_docs)]

use super::profile::{native_mode, CodexProfile};
use crate::application::agent_execution::agents::AgentError;
use crate::application::agent_execution::executions::ExecutionAudit;
use crate::application::agent_execution::providers::{
    AgentProvider, ApprovalMode, ApprovalModeChoice, HarnessHost, HarnessHostIdentity,
    HarnessLaunch, NativeHooksSwitch, ProviderIdentity, ProviderOpenFuture, ProviderOpenRequest,
};
use crate::application::agent_execution::traces::LocalTraceStore;
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
use std::{
    collections::BTreeMap,
    ffi::OsString,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use tokio::process::Command;

/// Immutable composition factory; opening twice creates independent process scopes.
#[derive(Clone)]
pub struct CodexAcpProvider {
    config: AcpConfig,
    capabilities: EffectiveCapabilities,
    system_prompt: Option<SystemPrompt>,
    approval_mode: ApprovalMode,
    effort_level: Option<EffortLevel>,
    /// Read when `CODEX_CONFIG` is built for a launch. Off sends today's
    /// suppression keys.
    native_hooks: NativeHooksSwitch,
    audit: Arc<dyn ExecutionAudit>,
    /// Deletions this binding started that are still stopping their process.
    deletions: DeletionCleanups,
    /// Where its harness is started when that is not this machine
    /// ([`AgentProvider::on_host`]).
    host: Option<Arc<dyn HarnessHost>>,
    /// Spans for every session this factory opens.
    traces: Arc<LocalTraceStore>,
}
impl CodexAcpProvider {
    /// The variables this binding sets for one launch of its harness, and
    /// the only ones a host that starts the harness for it accepts from it
    /// (`HarnessLaunch`).
    pub const LAUNCH_VARIABLES: &'static [&'static str] =
        &["CODEX_CONFIG", "INITIAL_AGENT_MODE", "NO_BROWSER"];
    /// The sandbox profiles this binding can set up: only the harness's own
    /// default today, since it configures no sandbox of its own. A lease that
    /// asks for any other is refused, never run under a weaker one.
    pub const SANDBOX_PROFILES: crate::domain::agent_execution::leases::SandboxProfiles =
        crate::domain::agent_execution::leases::SandboxProfiles::HARNESS_DEFAULT;
    /// Presets this binding has verified for an exact catalog model ID.
    pub fn approval_modes(model_id: &str) -> &'static [ApprovalModeChoice] {
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
    /// By default every launch also requests Codex's user-configured hooks and
    /// its legacy notify command off (`features.hooks` false, `notify` empty).
    /// A launch whose [`Self::with_native_hooks`] switch is on omits both keys
    /// and so does not make that request. The request
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
        let input = Modalities::new(true, config.images.is_some(), false)
            .expect("text input modality is nonempty");
        let restrictions = BindingRestrictions::new(
            // An effort level is sent (`with_effort_level`); fast mode is not.
            ModelFeatures::new(input, text, true, true, false),
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
            native_hooks: NativeHooksSwitch::off(),
            audit,
            deletions: DeletionCleanups::default(),
            host: None,
            traces: LocalTraceStore::new(),
        })
    }
    /// Record this factory's turns into `traces`.
    ///
    /// The default store is private to the factory. A host that rebuilds the
    /// factory for the same conversation passes the store it already reads.
    pub fn with_traces(mut self, traces: Arc<LocalTraceStore>) -> Self {
        self.traces = traces;
        self
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
    /// Let Codex run its own hooks when a process this factory starts reads the switch on.
    ///
    /// Off keeps `features.hooks: false` and `notify: []` in `CODEX_CONFIG`.
    /// On omits both keys, so the user's configuration is not overridden.
    /// The model and instructions stay. Each process start reads the switch
    /// once and uses that bool for both the profile and `CODEX_CONFIG`,
    /// including the next start of a conversation already open. The negotiated
    /// suppression fact stays unsupported either way.
    pub fn with_native_hooks(mut self, native_hooks: NativeHooksSwitch) -> Self {
        self.native_hooks = native_hooks;
        self
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
    /// the protocol. By default `features.hooks` is false and `notify` is
    /// empty. Those keys are not read back: legacy managed configuration and
    /// MDM are applied after the session layer, and ACP exposes no effective
    /// value, so the profile reports native hook suppression as unsupported.
    /// When the caller has allowed native hooks, both keys are omitted and
    /// the fact stays unsupported.
    fn session_config(&self, allowed: bool) -> String {
        let mut config = json!({
            "model": self.capabilities.model().model_id(),
        });
        if !allowed {
            // Ordinary configured handlers stop when the feature is false.
            // Provider builtin cleanup is a separate lifecycle path and is
            // not disabled here. Legacy notify is constructed independently
            // of the feature bit, so it is cleared on its own.
            config["features"] = json!({"hooks": false});
            config["notify"] = json!([]);
        }
        if let Some(prompt) = &self.system_prompt {
            config["instructions"] = json!(prompt.text().as_str());
        }
        config.to_string()
    }
    /// What this binding sets for every launch of its harness, wherever it
    /// runs: its session configuration, the preset it starts in, and no
    /// browser sign-in. Never a credential or a path.
    fn launch_environment(&self, allowed: bool) -> BTreeMap<OsString, OsString> {
        [
            ("CODEX_CONFIG", self.session_config(allowed)),
            // The preset every session is then explicitly selected into. Setting
            // it here as well means the session is never briefly open in a more
            // permissive one.
            (
                "INITIAL_AGENT_MODE",
                native_mode(self.approval_mode).to_owned(),
            ),
            // A gateway process has no browser and nobody watching it. Signing in
            // is the desktop's business, done before an agent is offered at all.
            ("NO_BROWSER", "1".to_owned()),
        ]
        .into_iter()
        .inspect(|(key, _)| debug_assert!(Self::LAUNCH_VARIABLES.contains(key)))
        .map(|(key, value)| (key.into(), value.into()))
        .collect()
    }
    fn launch_command(&self, allowed: bool) -> Command {
        let mut command = Command::new(self.config.executable.executable());
        command
            .args(self.config.command_arguments())
            .current_dir(&self.config.workspace)
            .env_clear()
            .envs(&self.config.environment)
            .envs(&self.config.runtime_environment)
            .envs(&self.config.credential_environment)
            .envs(self.launch_environment(allowed));
        command
    }
}
impl AgentProvider for CodexAcpProvider {
    fn approval_mode(&self) -> Option<ApprovalMode> {
        Some(self.approval_mode)
    }
    fn native_hooks_allowed(&self) -> bool {
        self.native_hooks.allowed()
    }
    fn effort_level(&self) -> Option<EffortLevel> {
        self.effort_level.clone()
    }
    fn harness_host_identity(&self) -> HarnessHostIdentity<'_> {
        self.host
            .as_ref()
            .map_or_else(HarnessHostIdentity::local, HarnessHostIdentity::hosted)
    }
    fn model_change_context(&self) -> Option<String> {
        Some(identity::model_change_context(
            &self.config,
            self.system_prompt.as_ref(),
        ))
    }
    fn with_model_configuration(
        &self,
        mode: Option<ApprovalMode>,
        effort: Option<EffortLevel>,
        traces: Option<Arc<LocalTraceStore>>,
    ) -> Result<Arc<dyn AgentProvider>, AgentError> {
        let approval_mode = mode.ok_or_else(|| {
            AgentError::Configuration("model change omitted approval mode".into())
        })?;
        let mut normalized = self.clone().with_approval_mode(approval_mode)?;
        normalized.effort_level = None;
        if let Some(level) = effort {
            normalized = normalized.with_effort_level(level)?;
        }
        normalized.traces = traces.unwrap_or_else(|| self.traces.clone());
        Ok(Arc::new(normalized))
    }
    fn launch_fingerprint(&self) -> String {
        identity::launch_fingerprint(&self.config)
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
        // The process start reads the switch once into `launch`. The profile
        // and the process factory share that cell, so they cannot diverge.
        let launch = Arc::new(AtomicBool::new(false));
        let profile = self.launch_profile(launch.clone());
        let process = self.process_factory_following(launch);
        let traces = self.traces.clone();
        Box::pin(async move {
            acp_binding::open(
                process,
                self.config.clone(),
                self.capabilities.clone(),
                profile,
                self.audit.clone(),
                request,
                traces,
            )
            .await
        })
    }
    fn traces(&self) -> Option<Arc<LocalTraceStore>> {
        Some(self.traces.clone())
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
    /// The profile a process start captures from the live switch.
    fn launch_profile(&self, launch: Arc<AtomicBool>) -> CodexProfile {
        CodexProfile::new(
            self.capabilities.model().model_id(),
            self.approval_mode,
            self.effort_level.clone(),
        )
        .following(self.native_hooks.clone(), launch)
    }

    /// The profile a launch speaks when native hooks are `allowed`.
    ///
    /// Deletion passes `false` here and to
    /// [`process_factory_allowing`](Self::process_factory_allowing). A
    /// conversation's process start uses the profile `open` builds, which
    /// reads the switch once at that start.
    pub(super) fn profile_allowing(&self, allowed: bool) -> CodexProfile {
        CodexProfile::new(
            self.capabilities.model().model_id(),
            self.approval_mode,
            self.effort_level.clone(),
        )
        .allowing_native_hooks(allowed)
    }

    /// How a deletion process is launched with hooks `allowed` or suppressed.
    ///
    /// The closure keeps `allowed`. A conversation's process start does not
    /// use this: it reads the shared cell the profile writes at that start.
    pub(super) fn process_factory_allowing(&self, allowed: bool) -> acp_binding::ProcessFactory {
        let factory = self.clone();
        match &self.host {
            Some(host) => {
                let host = host.clone();
                Arc::new(move || {
                    let launch = HarnessLaunch {
                        environment: factory.launch_environment(allowed),
                    };
                    Ok(ProcessScope::remote(host.start(launch)?))
                })
            }
            None => Arc::new(move || {
                ProcessScope::spawn(factory.launch_command(allowed)).map_err(Into::into)
            }),
        }
    }

    /// How a conversation's process is launched.
    ///
    /// The closure reads `launch`, which the profile writes from the one
    /// switch read at the start of that process. It does not read the switch.
    fn process_factory_following(&self, launch: Arc<AtomicBool>) -> acp_binding::ProcessFactory {
        let factory = self.clone();
        match &self.host {
            Some(host) => {
                let host = host.clone();
                Arc::new(move || {
                    let launch = HarnessLaunch {
                        environment: factory.launch_environment(launch.load(Ordering::Relaxed)),
                    };
                    Ok(ProcessScope::remote(host.start(launch)?))
                })
            }
            None => Arc::new(move || {
                ProcessScope::spawn(factory.launch_command(launch.load(Ordering::Relaxed)))
                    .map_err(Into::into)
            }),
        }
    }
}
