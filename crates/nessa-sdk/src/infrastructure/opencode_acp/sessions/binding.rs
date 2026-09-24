#![deny(missing_docs)]

use super::{effective_data_home, profile::OpencodeProfile};
use crate::application::agent_execution::agents::AgentError;
use crate::application::agent_execution::executions::ExecutionAudit;
use crate::application::agent_execution::providers::{
    AgentProvider, ProviderIdentity, ProviderOpenFuture, ProviderOpenRequest,
};
use crate::domain::agent_execution::permissions::PermissionScope;
use crate::domain::common::value_objects::TokenLimits;
use crate::domain::effective_capabilities::value_objects::{
    BindingRestrictions, EffectiveCapabilities,
};
use crate::domain::model_metadata::entities::ModelMetadata;
use crate::domain::model_metadata::value_objects::{Modalities, ModelFeatures, ModelProvider};
use crate::infrastructure::acp::sessions::{binding as acp_binding, identity, AcpConfig};
use crate::infrastructure::process::ProcessScope;
use std::{ffi::OsStr, path::Path, sync::Arc};
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
/// whichever agent is selected, so it outranks both the defaults and the mode.
///
/// Deny-first rather than a list of denied tools, because a deny list cannot
/// name the MCP tools: those are whatever the servers handed over at
/// `session/new` expose, so only `"*"` reaches them. Reading and searching are
/// allowed back — what a read-and-plan session needs and nothing that acts. The
/// shape is Opencode's own: its built-in `explore` agent is written the same
/// way.
///
/// This is not `ask`. An `ask` is a bound a person can lift one call at a time,
/// and the profile's claim is that these sessions do not act at all, so there
/// is nothing here for a host to approve.
///
/// # What this does not cover
///
/// Stated rather than implied, because a bound believed to be wider than it is
/// is worse than a narrow one.
///
/// **The environment-file denial is not a secrets boundary.** `read` is asked
/// for permission with the *path* it is about, so denying `*.env` there works.
/// `grep` is asked with the *regular expression* instead, and Opencode runs
/// ripgrep with `--hidden` unconditionally, so an allowed `grep` returns
/// matches from the very files `read` refuses to open. The `read` rules are
/// kept because they still stop the direct path, and they are not claimed to
/// be more than that. `lsp` is allowed on the same footing and has the same
/// shape — its permission carries no path either — and it is inert today only
/// because the tool is registered behind an experimental flag this launch does
/// not set. It is allowed because a read-and-plan session wants it if it ever
/// ships, not because it is bounded.
///
/// **Custom tools are not subject to permissions at all.** Opencode loads
/// `{tool,tools}/*.{js,ts}` from every config directory and calls their
/// `execute` with no permission evaluation of any kind. `"*"` does not reach
/// them because nothing asks. There is no pinned switch for this scan. This
/// binding instead gives each process a fresh private `HOME` and
/// `XDG_CONFIG_HOME`, removes every alternate config input, and disables project
/// config, so neither the checkout nor the person's global config can supply a
/// custom tool.
///
/// Plugin-supplied tools took the same route and no longer do: `OPENCODE_PURE`
/// makes the external plugin list empty, so none is loaded, none of its tools
/// is registered, and none of its `tool.execute.before`, `auth` or `shell.env`
/// hooks runs — the last of which could otherwise rewrite the arguments of a
/// tool this policy did allow. Upstream's own word for it is "skip external
/// plugins entirely". Opencode's *default* plugins are left alone: those ship
/// inside the binary Nessa pinned and are part of the agent rather than
/// something a machine brings to it.
///
/// **An MCP server in config is started, not merely offered.** A top-level or
/// `agent.plan.permission` block can also outrank this policy. The same private
/// config boundary closes both paths: there is no global config to contribute
/// an MCP entry or a later permission rule. The caller's resolved
/// `XDG_DATA_HOME` is preserved separately because that is where Opencode keeps
/// `auth.json`; code-loading configuration does not share that directory.
///
/// **A launch writes to the machine before any tool runs.** Two effects sit
/// outside permission evaluation entirely, so no policy reaches them and
/// nothing here records them. Every directory the config search returns is
/// created and given a `.gitignore`. Opencode then starts an asynchronous
/// in-process Arborist installation of `@opencode-ai/plugin` there, which may
/// write `node_modules`, a `package.json` and a lockfile and make registry
/// requests. It is an Effect fiber, not an OS-detached child, so process-group
/// cleanup bounds it. These writes land in the private process directory,
/// which is removed only after the process group is confirmed gone and is
/// retained when cleanup is uncertain. While the gateway is alive, shared ACP
/// cleanup owns and retries that release. A hard gateway-process exit can orphan
/// the private directory because Nessa has no durable process identity with
/// which a later gateway could prove every prior descendant gone; new launches
/// use fresh roots and never reclaim an uncertain one. Separately and earlier,
/// the binary creates seven directories of its own under the XDG roots and the
/// system temporary directory as its global module loads, before any
/// configuration is read at all. Lifecycle scripts are off upstream and a directory that cannot be
/// written is a no-op there, but neither of those is a switch and there is
/// none to set: `OPENCODE_PURE` empties the plugin list and does not touch
/// this path. Named because this section is where a reader finds out what a
/// launch costs, not because anything here can prevent it.
///
/// One rule is appended after this policy: Opencode gives every agent
/// `external_directory` access to its own `tool-output` directory unless the
/// agent already denies that exact path. Narrow and its own scratch space, so
/// it is left alone — but it does mean this policy is the last word among
/// the permission rules rather than the last rule merged.
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
        let config = isolated_config(config)?;
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
        // Image input is offered only when composition supplied the bytes'
        // source, the same rule the Claude binding states. The model's own
        // metadata and Opencode's advertised prompt capabilities narrow it
        // further; neither can widen it.
        //
        // This binding declared text-only for a while, and the reason has since
        // expired: the shared worker built every `session/prompt` as a single
        // text block, so an image declared here had no way to reach the
        // process. It now builds text and image blocks and gates them on
        // `promptCapabilities.image` together with a byte source, so the
        // capability is deliverable and withholding it is what would be the
        // false statement. Opencode's `initialize` does advertise image
        // prompts, measured against 1.18.31.
        //
        // No model a person can select today gets one, and the reason is the
        // catalogue rather than this line. `opencode/mimo-v2.5-free` declares
        // `input.image` and records no `imageInput` limits, and
        // `EffectiveCapabilities` offers image input only where the limits
        // are — without them nothing can prepare an image the model would
        // accept — so the effective input modality comes out text for all
        // three Opencode entries. This declaration is still the right one:
        // the layer that cannot deliver an image is the one that should
        // withhold it, and a binding that lied about the transport would hide
        // the catalogue's gap instead of leaving it visible.
        //
        // That gap has a cost beyond Opencode, which is the base branch's to
        // fix rather than this binding's: `composition::agent::image_limits`
        // keeps no images at all once any configured model records none, and
        // that value is the one attachment store every conversation shares. So
        // configuring Opencode turns image attachments off gateway-wide,
        // Claude conversations included, exactly as configuring Codex already
        // does — the four `openai` entries record no limits either.
        // `no_opencode_model_nessa_ships_can_be_sent_an_image` is where that
        // is said out loud; it goes red the day somebody records limits, which
        // is the day this comment needs reading again.
        let input = Modalities::new(true, config.images.is_some(), false)
            .expect("text modality is nonempty");
        let restrictions = BindingRestrictions::new(
            ModelFeatures::new(input, text, true, false),
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

    fn launch_command(&self, private_home: &Path) -> Command {
        let mut command = Command::new(self.config.executable.executable());
        command
            .args(&self.config.arguments)
            .current_dir(&self.config.workspace)
            .env_clear()
            .envs(&self.config.environment)
            .envs(&self.config.credential_environment)
            .env_remove("OPENCODE_CONFIG")
            .env_remove("OPENCODE_CONFIG_CONTENT")
            .env_remove("OPENCODE_CONFIG_DIR")
            .env("HOME", private_home)
            .env("XDG_CONFIG_HOME", private_home.join("config"))
            .env("XDG_CACHE_HOME", private_home.join("cache"))
            .env("XDG_STATE_HOME", private_home.join("state"))
            .env("OPENCODE_PERMISSION", SESSION_POLICY)
            // The workspace Opencode is pointed at is a checkout, and a
            // checkout is somebody else's text. Without this, an
            // `opencode.json` committed to it is read as configuration and
            // merges into the agent it is about to run.
            .env("OPENCODE_DISABLE_PROJECT_CONFIG", "1")
            // A plugin's tools run without any permission being asked for, and
            // its hooks can rewrite the arguments of a tool that was. Neither
            // is reachable by a policy, so the loading is what has to go: this
            // empties the external plugin list. Not the default plugins, which
            // are part of the binary Nessa pinned.
            .env("OPENCODE_PURE", "1")
            // Opencode refreshes the models.dev catalogue over the network at
            // startup and every hour after, carrying its version and channel.
            // Nessa picks the model from its own pinned catalogue and the
            // binary answers offline from the snapshot built into it, so the
            // request buys a read-and-plan session nothing and is one more
            // thing a launch does that nobody asked for.
            //
            // What this pins along with the binary is the list `session/new`
            // offers: the catalogue is read from the machine's cache first,
            // then from the snapshot, and suppressing the hourly refresh means
            // neither is ever renewed. `configuration::offers` requires an
            // exact match, so a model id retired or renamed upstream after the
            // pinned build stops being offered and every session is refused
            // rather than healing itself on the next refresh. That is
            // fail-closed and the refusal names the cause, and `data/models.json`
            // would need the same edit regardless. The opt-in pinned-binary
            // contract asks a live Opencode for selected IDs in each auth tier;
            // the synthetic handler remains the deterministic protocol fixture.
            .env("OPENCODE_DISABLE_MODELS_FETCH", "1")
            // The pinned release is the tested one, so a copy that moves on
            // its own is a version this profile's `initialize` check would
            // start refusing with nothing saying why. Only Opencode's TUI
            // reaches the upgrade path and `acp` never does, so this makes a
            // sentence that is true of the entry point true of the binary.
            .env("OPENCODE_DISABLE_AUTOUPDATE", "1");
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

    fn capabilities(&self) -> &EffectiveCapabilities {
        &self.capabilities
    }

    fn open(&self, request: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        Box::pin(async move {
            let (restore, control) = request.into_parts();
            let factory = self.clone();
            acp_binding::open(
                Arc::new(move || {
                    ProcessScope::spawn_with_private_directory(|private_home| {
                        factory.launch_command(private_home)
                    })
                    .map(|(scope, _)| scope)
                }),
                self.config.clone(),
                self.capabilities.clone(),
                OpencodeProfile::new(self.capabilities.model().model_id()),
                self.audit.clone(),
                restore,
                control,
            )
            .await
        })
    }
}

fn isolated_config(mut config: AcpConfig) -> Result<AcpConfig, AgentError> {
    let data_home = effective_data_home(&config.environment, &config.credential_environment)?;
    for key in [
        "HOME",
        "XDG_CONFIG_HOME",
        "XDG_CACHE_HOME",
        "XDG_STATE_HOME",
        "XDG_DATA_HOME",
        "OPENCODE_CONFIG",
        "OPENCODE_CONFIG_CONTENT",
        "OPENCODE_CONFIG_DIR",
    ] {
        config.environment.remove(OsStr::new(key));
        config.credential_environment.remove(OsStr::new(key));
    }
    config
        .environment
        .insert("XDG_DATA_HOME".into(), data_home.into_os_string());
    Ok(config)
}
