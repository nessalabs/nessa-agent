//! Trusted local agent configuration.
//!
//! A request may name which configured agent it wants, and nothing more: the
//! executable, its arguments, the workspace and the credentials are this file's
//! to decide. So a request chooses between processes this machine already
//! trusts; it never supplies one, and never selects a workspace.
//!
//! ```text
//!   config.json "agents"
//!     ├── shared:   catalog, workspace, mcpServers
//!     ├── selected: which agent a caller that names none runs on
//!     └── runtimes: { "<agent>": { command, args, model,
//!                                   contextTokens, outputTokens,
//!                                   toolsEnabled } , ... }
//! ```
//!
//! What every agent on this machine shares is stated once: they work in the
//! same workspace, against the same model catalog, and are offered the same
//! Nessa MCP servers, because that is what makes them alternatives rather than
//! separate installations.
//!
//! What differs is how the agent is started, which model it runs, the budget it
//! runs in, and whether its own tools are on. Started is a command and its arguments rather than a
//! runtime and an entry script: an agent that speaks ACP through a Node harness
//! is `(node, [entry.js])` and one that speaks it natively is
//! `(its own binary, ["acp"])`, and the second cannot be said at all in the
//! narrower shape. `AcpConfig` has always taken a command and arguments; this
//! is the configuration catching up with it.
//!
//! The agents are a map keyed by name rather than a field per agent, so a new
//! agent is a new [`AgentId`] and nothing here. A name no adapter exists for is
//! reported by name rather than ignored.
//!
//! An agent absent from the configuration is one this server cannot start.
//! That is reported where it is asked about — setup says the agent is not set
//! up here, which is a fact about this installation and not about the machine —
//! rather than substituted for at startup.
use crate::agents::domain::AgentId;
use crate::conversation::application::ConversationAgent;
use crate::core::RunError;
use nessa_auth::application::ports::Clock;
use nessa_sdk::infrastructure::acp::sessions::StdioMcpServer;
use serde::Deserialize;
use std::{
    collections::{BTreeMap, HashMap},
    ffi::OsString,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct AgentsConfig {
    pub catalog: PathBuf,
    pub workspace: PathBuf,
    #[serde(default)]
    pub mcp_servers: Vec<StdioMcpServer>,
    /// The agent a conversation runs on when nothing else names one.
    ///
    /// Left out where only one agent is configured, because there is nothing to
    /// choose between; required where more than one is, because guessing which
    /// of several configured agents the operator meant is not a default, it is
    /// a coin toss with someone else's work on it.
    #[serde(default)]
    pub selected: Option<String>,
    /// What this machine can start, by the name each agent is known by.
    ///
    /// Kept as written rather than as [`AgentId`] so an unfamiliar name
    /// survives parsing and can be reported as the name the operator typed.
    #[serde(default)]
    pub runtimes: HashMap<String, AgentRuntime>,
}

/// How one agent is started, within the shared configuration above.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct AgentRuntime {
    /// The executable this server runs for this agent.
    pub command: PathBuf,
    /// What that executable is handed. A Node harness takes its entry script; an
    /// agent that speaks ACP itself takes its own subcommand.
    #[serde(default)]
    pub args: Vec<String>,
    pub model: String,
    #[serde(default = "context_tokens")]
    pub context_tokens: u32,
    #[serde(default = "output_tokens")]
    pub output_tokens: u32,
    /// Whether this agent runs its own tools.
    ///
    /// Per agent rather than shared because it is not a preference every agent
    /// can be asked about the same way: Codex has no text-only mode and refuses
    /// to be built without it, so one shared `false` — set by someone thinking
    /// about Claude — would take the whole server down over an agent they were
    /// not configuring.
    ///
    /// Stated rather than defaulted, for the same reason. `false` is the one
    /// value Codex cannot start on, so a default would have reproduced exactly
    /// that failure for anyone who simply left the field out — and reproduced it
    /// as the whole gateway refusing to start, since every configured agent is
    /// built. Omitting it now fails to parse, naming the field.
    ///
    /// Read only where a binding is built, which is Unix alone: the whole point
    /// of the field is what it tells an agent's binding, and on a platform that
    /// builds none there is no one to tell. It is still parsed and still
    /// required there, because a configuration is either well-formed or it is
    /// not, and that does not vary by host.
    #[cfg_attr(not(unix), allow(dead_code))]
    pub tools_enabled: bool,
}

impl AgentRuntime {
    /// Every absolute path this server would hand the command.
    ///
    /// Exact rather than a guess about arguments: a path this server would pass
    /// has to be on this machine for the launch to work, and an argument that is
    /// not a path is the agent's own vocabulary — a subcommand or a flag — which
    /// is nothing for this machine to be asked about.
    pub fn paths(&self) -> Vec<PathBuf> {
        self.args
            .iter()
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .collect()
    }
}

impl AgentsConfig {
    /// Each configured agent, in the order [`AgentId::ALL`] lists them.
    ///
    /// Silent about a name no adapter exists for; [`Self::unknown`] is what
    /// reports one, so that reading the configuration and refusing it stay
    /// separate concerns.
    pub fn agents(&self) -> Vec<(AgentId, &AgentRuntime)> {
        AgentId::ALL
            .iter()
            .filter_map(|agent| self.runtime(*agent).map(|runtime| (*agent, runtime)))
            .collect()
    }

    /// What this configuration says about one agent, or nothing when it says
    /// nothing about it.
    pub fn runtime(&self, agent: AgentId) -> Option<&AgentRuntime> {
        self.runtimes.get(agent.name())
    }

    /// The first configured name this build has no adapter for, if any.
    ///
    /// Reported rather than skipped: a typo under `runtimes` would otherwise be
    /// an agent silently missing from setup, which reads as an agent that is not
    /// installed on a machine where it is.
    pub fn unknown(&self) -> Option<&str> {
        let mut unknown: Vec<&str> = self
            .runtimes
            .keys()
            .map(String::as_str)
            .filter(|name| AgentId::parse(name).is_none())
            .collect();
        // The map has no order of its own, so the same configuration must not
        // name a different one of its mistakes on each startup.
        unknown.sort_unstable();
        unknown.into_iter().next()
    }

    /// The agent a caller that names none runs on.
    ///
    /// # Errors
    /// Returns [`RunError::Agent`] for a name no adapter exists for, a name with
    /// no configuration under it, no agents at all, or several agents with no
    /// choice stated between them.
    pub fn selected(&self) -> Result<AgentId, RunError> {
        if let Some(name) = self.unknown() {
            return Err(RunError::Agent(format!(
                "configured agent \"{name}\" has no adapter in Nessa"
            )));
        }
        let configured = self.agents();
        if let Some(name) = &self.selected {
            let agent = AgentId::parse(name).ok_or_else(|| {
                RunError::Agent(format!("selected agent \"{name}\" has no adapter in Nessa"))
            })?;
            if self.runtime(agent).is_none() {
                return Err(RunError::Agent(format!(
                    "selected agent \"{name}\" has no configuration under \"runtimes\""
                )));
            }
            return Ok(agent);
        }
        match configured.as_slice() {
            [(agent, _)] => Ok(*agent),
            [] => Err(RunError::Agent(
                "configure at least one agent under \"agents.runtimes\"".into(),
            )),
            _ => Err(RunError::Agent(
                "several agents are configured; name one in \"selected\"".into(),
            )),
        }
    }

    fn validate(&self) -> Result<(), RunError> {
        self.selected()?;
        if [&self.catalog, &self.workspace]
            .iter()
            .any(|path| !path.is_absolute())
        {
            return Err(RunError::Agent("agent paths must be absolute".into()));
        }
        for (agent, runtime) in self.agents() {
            // Only the command is required to be absolute. A relative argument
            // is not a path this server resolves at all — it is the agent's own
            // vocabulary, passed through untouched — so there is nothing here to
            // reject it for.
            if !runtime.command.is_absolute()
                || runtime.model.trim().is_empty()
                || runtime.output_tokens == 0
                || runtime.context_tokens <= runtime.output_tokens
            {
                return Err(RunError::Agent(format!(
                    "{}: command must be absolute, model nonempty, and token limits positive with room for input",
                    agent.name()
                )));
            }
        }
        Ok(())
    }
}
fn context_tokens() -> u32 {
    100_000
}
fn output_tokens() -> u32 {
    4096
}

/// The environment every agent process inherits, beyond its credentials.
///
/// `env_clear` is what the bindings launch with, so anything an agent needs
/// has to be named. The vendor-specific entries are each agent's own
/// directory variable: naming every one of them for every agent would be
/// shorter and would also hand each agent a pointer into the others'
/// configuration. Opencode has none to name — it reads its configuration from
/// under `HOME`, which every agent is given anyway — and an agent with nothing
/// of its own is not given somebody else's.
fn process_environment(agent: AgentId) -> BTreeMap<OsString, OsString> {
    let mut environment = BTreeMap::new();
    let vendor = match agent {
        AgentId::Claude => Some("CLAUDE_CONFIG_DIR"),
        AgentId::Codex => Some("CODEX_HOME"),
        AgentId::Opencode => None,
    };
    let shared = ["PATH", "HOME", "USER", "LOGNAME", "TMPDIR"];
    for key in shared.into_iter().chain(vendor) {
        if let Some(value) = std::env::var_os(key) {
            environment.insert(key.into(), value);
        }
    }
    if let Some((key, value)) = sign_in_instruction(agent, &credential_environment(agent)) {
        environment.insert(key.into(), value.into());
    }
    environment
}

/// What an agent has to be *told* about the credential it was handed, beyond
/// being handed it.
///
/// Codex only reaches for a key in its environment when it is asked to sign in
/// that way. Its adapter takes that request from the client's ACP
/// `authenticate` or from this variable at startup, and Nessa's shared worker
/// sends no `authenticate` — the protocol layer is one runtime for every vendor
/// and signing in is not something it does. So without this, a machine whose
/// only Codex credential is `OPENAI_API_KEY` starts the agent, is offered as
/// signed in by setup, and then refuses every `session/new` with
/// "Authentication required": the readiness answer and the launch describing
/// different machines.
///
/// `api-key` is the only method that works unattended. The others open a
/// browser or print a device code, which is not something a gateway starting an
/// agent can complete.
///
/// No key travels in this value. The adapter reads the key itself out of the
/// environment it was started with, which is the one this is added to.
fn sign_in_instruction(
    agent: AgentId,
    credentials: &BTreeMap<OsString, OsString>,
) -> Option<(&'static str, &'static str)> {
    match agent {
        AgentId::Codex if !credentials.is_empty() => {
            Some(("DEFAULT_AUTH_REQUEST", r#"{"methodId":"api-key"}"#))
        }
        // Claude's harness reads its own credential without being asked, and an
        // agent handed no credential at all has nothing to be told. Opencode is
        // always the second of those: it is handed none, because the models
        // this binding runs it on need no account.
        AgentId::Claude | AgentId::Codex | AgentId::Opencode => None,
    }
}

/// Exactly what a launched agent's environment is, for anything that has to
/// ask about the agent rather than start it.
///
/// The readiness probe runs the agent's own tool to ask whether it is
/// signed in, and an answer from a different environment is an answer about
/// a different installation: `CODEX_HOME` decides which account it reads,
/// and the session-bus variables decide whether a keyring can be opened at
/// all. Inheriting this server's whole environment would let the probe find
/// a sign-in the launch then cannot use.
pub(super) fn launch_environment(agent: AgentId) -> BTreeMap<OsString, OsString> {
    let mut environment = process_environment(agent);
    environment.extend(credential_environment(agent));
    environment
}

/// The sign-in this agent is started with, read from this server's own
/// environment. Named per agent so none is handed another's key.
///
/// Opencode names none. It reaches the models this binding runs it on without
/// an account at all, and a key for its gateway is something a person gives
/// Opencode itself, under `HOME` — so there is no variable here that would
/// start a signed-in Opencode, and inventing one would put somebody else's key
/// into its environment.
fn credential_environment(agent: AgentId) -> BTreeMap<OsString, OsString> {
    let keys: &[&str] = match agent {
        AgentId::Claude => &["ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN"],
        AgentId::Codex => &["CODEX_API_KEY", "OPENAI_API_KEY"],
        AgentId::Opencode => &[],
    };
    let mut environment = BTreeMap::new();
    for key in keys {
        if let Some(value) = std::env::var_os(key) {
            environment.insert((*key).into(), value);
        }
    }
    environment
}

/// Build a provider for every configured agent.
///
/// All of them, not only the selected one: a conversation records the agent it
/// was created on and is reopened on that same agent afterwards, so a server
/// that had only built the selected one could not reopen the conversations
/// already on disk.
#[cfg(unix)]
pub(super) fn providers(
    config: &AgentsConfig,
    directory: &Path,
    clock: Arc<dyn Clock>,
) -> Result<HashMap<AgentId, ConversationAgent>, RunError> {
    config.validate()?;
    let mut agents = HashMap::new();
    for (agent, runtime) in config.agents() {
        agents.insert(
            agent,
            ConversationAgent {
                provider: build::provider(agent, config, runtime, directory, clock.clone())?,
                reserved_output_tokens: runtime.output_tokens,
            },
        );
    }
    Ok(agents)
}
#[cfg(not(unix))]
pub(super) fn providers(
    config: &AgentsConfig,
    _: &Path,
    _: Arc<dyn Clock>,
) -> Result<HashMap<AgentId, ConversationAgent>, RunError> {
    config.validate()?;
    Err(RunError::Agent(
        "ACP agents require Unix process supervision".into(),
    ))
}
#[cfg(unix)]
mod build {
    use super::{AgentId, AgentRuntime, AgentsConfig, RunError};
    use crate::conversation::infrastructure::DurableExecutionAudit;
    use nessa_auth::application::ports::Clock;
    use nessa_sdk::{
        application::agent_execution::{agents::AgentError, providers::AgentProvider},
        domain::{
            agent_execution::{
                permissions::PermissionOfferPolicy,
                prompts::{PromptSource, PromptSourceKind, SystemPrompt, SystemPromptBuilder},
            },
            common::value_objects::TokenLimits,
            model_metadata::entities::ModelMetadata,
        },
        infrastructure::{
            acp::sessions::AcpConfig, claude_acp::sessions::ClaudeAcpProvider,
            codex_acp::sessions::CodexAcpProvider, model_metadata_json::load_catalog,
            opencode_acp::sessions::OpencodeAcpProvider,
        },
    };
    use std::{fs::File, path::Path, sync::Arc, time::Duration};

    /// Which model catalog entries an agent's harness is allowed to run.
    ///
    /// Not a preference: each harness speaks to one vendor's API and is signed
    /// in to it, so a catalog entry from another vendor is a model that agent
    /// cannot reach, and saying so at startup beats a provider refusing every
    /// prompt.
    fn catalog_provider(agent: AgentId) -> &'static str {
        match agent {
            AgentId::Claude => "anthropic",
            AgentId::Codex => "openai",
            AgentId::Opencode => "opencode",
        }
    }

    /// Nessa's own instructions, attributed to Nessa rather than to the agent.
    ///
    /// The same text for either agent: it describes what Nessa is and how its
    /// tools are reached, and none of that changes with the harness underneath.
    fn system_prompt() -> Result<SystemPrompt, RunError> {
        SystemPromptBuilder::new()
            .text(
                PromptSource::new(PromptSourceKind::Core, "nessa/gateway")
                    .map_err(|e| RunError::Agent(e.to_string()))?,
                "You are Nessa, a helpful assistant. Follow the user's request. Use your native tools for file operations and web research. All Nessa tools are provided through MCP. For shell commands use the Nessa MCP shell tool, which tracks execution through Shepherd; never substitute an unmanaged shell path. Report tool failures accurately.",
            )
            .build()
            .map_err(|e| RunError::Agent(e.to_string()))
    }

    pub(super) fn provider(
        agent: AgentId,
        config: &AgentsConfig,
        runtime: &AgentRuntime,
        directory: &Path,
        clock: Arc<dyn Clock>,
    ) -> Result<Arc<dyn AgentProvider>, RunError> {
        let invalid = |error| RunError::Agent(format!("{error}"));
        let model = ModelMetadata::try_from(
            load_catalog(
                File::open(&config.catalog)
                    .map_err(|_| RunError::Agent("cannot read model catalog".into()))?,
            )
            .map_err(|e| RunError::Agent(e.to_string()))?
            .select(catalog_provider(agent), &runtime.model)
            .map_err(|e| RunError::Agent(e.to_string()))?,
        )
        .map_err(|e| RunError::Agent(e.to_string()))?;
        let workspace = config
            .workspace
            .canonicalize()
            .map_err(|_| RunError::Agent("workspace must exist".into()))?;
        if !workspace.is_dir()
            || !runtime.command.is_file()
            || runtime.paths().iter().any(|path| !path.exists())
        {
            return Err(RunError::Agent(format!(
                "{}: its command must be an existing file and every absolute path it is given must exist; workspace must be a directory",
                agent.name()
            )));
        }
        let limits = TokenLimits::new(runtime.context_tokens, runtime.output_tokens)
            .map_err(|e| RunError::Agent(e.to_string()))?;
        let audit =
            Arc::new(DurableExecutionAudit::new(directory.join("audit"), clock).map_err(invalid)?);
        let acp = AcpConfig {
            executable: runtime.command.clone(),
            arguments: runtime.args.iter().map(Into::into).collect(),
            environment: super::process_environment(agent),
            credential_environment: super::credential_environment(agent),
            workspace,
            tools_enabled: runtime.tools_enabled,
            mcp_servers: config.mcp_servers.clone(),
            permissions: PermissionOfferPolicy::once_only(),
            startup_timeout: Duration::from_secs(45),
            execution_timeout: None,
            shutdown_grace: Duration::from_secs(3),
            kill_timeout: Duration::from_secs(2),
            event_capacity: 256,
            max_frame_bytes: 1024 * 1024,
        };
        let prompt = system_prompt()?;
        let failed = |e: AgentError| RunError::Agent(format!("{}: {e}", agent.name()));
        let provider: Arc<dyn AgentProvider> = match agent {
            AgentId::Claude => Arc::new(
                ClaudeAcpProvider::new(acp, &model, limits, audit)
                    .map_err(failed)?
                    .with_system_prompt(prompt),
            ),
            AgentId::Codex => Arc::new(
                CodexAcpProvider::new(acp, &model, limits, audit)
                    .map_err(failed)?
                    .with_system_prompt(prompt),
            ),
            // No prompt, because there is nowhere to put one that Opencode can
            // be shown to read: its binding offers no `with_system_prompt` for
            // exactly that reason, and this arm not calling one is the compiler
            // enforcing it rather than a convention someone has to remember.
            // Opencode therefore runs under its own instructions. It is opened
            // in a mode that runs nothing, which is what keeps that difference
            // from mattering yet.
            AgentId::Opencode => {
                Arc::new(OpencodeAcpProvider::new(acp, &model, limits, audit).map_err(failed)?)
            }
        };
        Ok(provider)
    }
}

#[cfg(test)]
#[path = "../../tests/conversation/configuration.rs"]
mod tests;
