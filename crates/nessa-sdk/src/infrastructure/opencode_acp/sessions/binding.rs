#![deny(missing_docs)]

use super::profile::OpencodeProfile;
use crate::application::agent_execution::agents::AgentError;
use crate::application::agent_execution::executions::ExecutionAudit;
use crate::application::agent_execution::providers::{
    AgentProvider, ProviderIdentity, ProviderOpenFuture,
};
use crate::domain::agent_execution::permissions::PermissionScope;
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

/// What an Opencode session launched by this binding is allowed to do, as the
/// `OPENCODE_PERMISSION` value its process is started with.
///
/// The mode's name is not the policy. `plan` disallows *edits*; it leaves
/// `bash`, `webfetch`, `websearch`, `task` and every MCP tool at the global
/// default of `allow`. Nessa hands each session an MCP server whose only tool
/// runs a shell command, so a session bounded by nothing but the mode could run
/// anything in the workspace it was opened on. This is the bound instead, and
/// it is a launch-time one: Opencode merges `OPENCODE_PERMISSION` into the
/// top-level `permission` config, and merges *that* after the built-in rules of
/// whichever agent is selected, so it is the last word over both the defaults
/// and the mode.
///
/// Deny-first rather than a list of denied tools, because a deny list cannot
/// name the MCP tools: those are whatever the servers handed over at
/// `session/new` expose, so only `"*"` reaches them. Reading and searching are
/// allowed back, minus the environment files — what a read-and-plan session
/// needs and nothing that acts. The shape is Opencode's own: its built-in
/// `explore` agent is written the same way.
///
/// This is not `ask`. An `ask` is a bound a person can lift one call at a time,
/// and the profile's claim is that these sessions do not act at all, so there
/// is nothing here for a host to approve.
///
/// What it does not cover, stated rather than implied: Opencode merges a
/// *per-agent* `agent.plan.permission` after the top-level one this variable
/// feeds, so a config file that names the mode by name still wins. The
/// workspace cannot be that file — `OPENCODE_DISABLE_PROJECT_CONFIG` takes the
/// checkout out of the search — but the person's own global config still can,
/// and deliberately still does. Pointing `OPENCODE_CONFIG_DIR` at a directory
/// Nessa owns would not close it either: `$HOME/.opencode` is read whatever
/// that variable says. So the line this draws is at somebody else's repository,
/// not at the person running Nessa on their own machine.
const SESSION_POLICY: &str = r#"{"*":"deny","read":{"*":"allow","*.env":"deny","*.env.*":"deny","*.env.example":"allow"},"grep":"allow","glob":"allow","lsp":"allow","todowrite":"allow"}"#;

/// Immutable composition factory; opening twice creates independent process scopes.
#[derive(Clone)]
pub struct OpencodeAcpProvider {
    config: AcpConfig,
    capabilities: EffectiveCapabilities,
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
    /// Opencode brings its own tools, so what a session may do is decided by
    /// the policy it is launched under rather than by a tool set the host
    /// withholds: every session is launched under the read-only permission
    /// policy this module documents, which allows reading and searching and
    /// denies the rest. The session mode is pinned to the less permissive of
    /// the two Opencode has, but that is a preference on top of the policy, not
    /// the bound — see `sessions::profile`. Configuring a binding with tools
    /// disabled is refused rather than accepted as a text-only Opencode that
    /// does not exist.
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
        // Opencode's `initialize` advertises image prompts, but this binding
        // cannot offer one: the shared ACP worker builds every `session/prompt`
        // as a single text block, so an image declared here would be a
        // capability with no way to reach the process. Text in, text out, until
        // the worker can carry an image block.
        let text = Modalities::new(true, false, false).expect("text modality is nonempty");
        let restrictions = BindingRestrictions::new(
            ModelFeatures::new(text, text, true, false),
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
            audit,
        })
    }

    // No `with_system_prompt`, deliberately, and its absence is the contract:
    // Nessa has found no way to give Opencode instructions over ACP that it can
    // show arrives.
    //
    // The other two profiles each have one and prove it. Claude carries the
    // text in `_meta.systemPrompt`; Codex writes it into `CODEX_CONFIG` and has
    // a test that reads it back out of the launched process. Opencode offers no
    // equivalent that could be tested: `initialize` advertises no such
    // capability, and while `session/new` accepts `instructions`,
    // `systemPrompt` and `_meta.systemPrompt` without complaint, so does it
    // accept `thisFieldIsNonsense` — the server ignores unknown parameters
    // rather than rejecting them, so acceptance says nothing about delivery,
    // and the one thing that would say is a model turn.
    //
    // A builder here would compile, look like the others, and drop the text in
    // silence. It would also change the provider fingerprint, so editing a
    // prompt that never arrived would invalidate restorable Opencode sessions
    // for no effect on them. Leaving it out makes composition unable to pass
    // one by accident, which is the property worth having.
    //
    // What this costs: Opencode runs under its own default instructions, not
    // Nessa's. The MCP servers are still handed over at `session/new`, but
    // under `SESSION_POLICY` their tools are denied, so what is offered is a
    // list Opencode can see and not call. That is deliberate while the profile
    // reads and plans — Nessa's own server exposes a shell, which is exactly
    // what this policy is for — and it is the second thing to solve, with the
    // prompt, before Opencode is given a mode that acts.

    fn launch_command(&self) -> Command {
        let mut command = Command::new(&self.config.executable);
        command
            .args(&self.config.arguments)
            .current_dir(&self.config.workspace)
            .env_clear()
            .envs(&self.config.environment)
            .envs(&self.config.credential_environment)
            .env("OPENCODE_PERMISSION", SESSION_POLICY)
            // The workspace Opencode is pointed at is a checkout, and a
            // checkout is somebody else's text. Without this, an
            // `opencode.json` committed to it is read as configuration and
            // merges into the agent it is about to run.
            .env("OPENCODE_DISABLE_PROJECT_CONFIG", "1");
        command
    }
}

impl AgentProvider for OpencodeAcpProvider {
    fn identity(&self) -> ProviderIdentity {
        ProviderIdentity::new(
            "opencode-acp",
            self.capabilities.model().model_id(),
            // `None`: there is no prompt to hash, and hashing one that never
            // reached the process would tie restoration to text it never saw.
            identity::fingerprint(&self.config, self.capabilities.limits(), None),
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
