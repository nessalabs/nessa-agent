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
#[cfg(unix)]
use crate::agents::infrastructure::CredentialedClaudeProvider;
use crate::agents::{application::AgentCredentialSource, domain::AgentId};
use crate::conversation::application::ConversationAgent;
use crate::core::RunError;
use nessa_auth::application::ports::Clock;
use nessa_sdk::{
    application::agent_execution::providers::{ExecutableUseSnapshot, UserImageSource},
    domain::model_metadata::{entities::ModelMetadata, value_objects::ImageInputLimits},
    infrastructure::{acp::sessions::StdioMcpServer, model_metadata_json::load_catalog},
};
use serde::{Deserialize, Deserializer};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    ffi::OsString,
    fs::File,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone, Debug, Deserialize)]
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
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct AgentRuntime {
    /// The executable this server runs for this agent.
    #[serde(deserialize_with = "unmanaged_executable")]
    pub command: ExecutableUseSnapshot,
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

fn unmanaged_executable<'de, D>(deserializer: D) -> Result<ExecutableUseSnapshot, D::Error>
where
    D: Deserializer<'de>,
{
    PathBuf::deserialize(deserializer).map(ExecutableUseSnapshot::unmanaged)
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
    #[cfg(test)]
    pub fn selected(&self) -> Result<AgentId, RunError> {
        self.selected_from(&self.agents().into_iter().map(|(agent, _)| agent).collect())
    }

    /// The default agent among the identities composition can resolve.
    ///
    /// Deferred agents have no startup runtime entry, but are still concrete
    /// configured choices because their launch is resolved at cold open.
    pub fn selected_from(&self, configured: &HashSet<AgentId>) -> Result<AgentId, RunError> {
        if let Some(name) = self.unknown() {
            return Err(RunError::Agent(format!(
                "configured agent \"{name}\" has no adapter in Nessa"
            )));
        }
        if let Some(name) = &self.selected {
            let agent = AgentId::parse(name).ok_or_else(|| {
                RunError::Agent(format!("selected agent \"{name}\" has no adapter in Nessa"))
            })?;
            if !configured.contains(&agent) {
                return Err(RunError::Agent(format!(
                    "selected agent \"{name}\" is not configured"
                )));
            }
            return Ok(agent);
        }
        match configured.len() {
            1 => Ok(*configured.iter().next().expect("one configured agent")),
            0 => Err(RunError::Agent(
                "configure at least one agent under \"agents.runtimes\"".into(),
            )),
            _ => Err(RunError::Agent(
                "several agents are configured; name one in \"selected\"".into(),
            )),
        }
    }

    fn validate_for(&self, configured: &HashSet<AgentId>) -> Result<(), RunError> {
        self.selected_from(configured)?;
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
            if !runtime.command.executable().is_absolute()
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
/// The `PATH` the agent's process tree gets: Claude Code, its Bash tool, and
/// the Nessa MCP shell tool all inherit this one.
///
/// It is deliberately not this process's own. A packaged gateway is a launchd
/// service, and the `PATH` launchd gives it is the system one — nothing the user
/// installed is on it, which is how "run the tests" became `command not found:
/// pnpm` on a machine where every terminal has `pnpm`. The desktop host resolves
/// the user's login-shell path once, when it registers the service, and hands it
/// over as `NESSA_AGENT_PATH`: a variable of Nessa's own, so widening what the
/// agent can reach never widens what the service itself can.
///
/// A developer loop has no host and no such variable, and there the process
/// `PATH` *is* the developer's own shell path, which is the right answer.
fn agent_search_path(resolved: Option<OsString>, inherited: Option<OsString>) -> Option<OsString> {
    resolved
        .filter(|path| !path.is_empty())
        .or(inherited)
        .filter(|path| !path.is_empty())
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
/// configuration.
///
/// Opencode's are the XDG ones, because that is what it resolves its own
/// directories from — config, data, cache and state, and with them its
/// providers, its plugins and whatever account the person signed in on. Under
/// `env_clear` an unnamed `XDG_CONFIG_HOME` does not mean "unset", it means
/// Opencode falls back to `$HOME/.config` and reads a different installation
/// than the one the readiness probe answered about. They are general-purpose
/// variables rather than Opencode's own, but they are the person's own paths
/// and every agent here is already given `HOME`, so nothing is handed over that
/// was not already reachable.
///
/// Nothing here tells an agent *how* to sign in. Codex's adapter will take a
/// `DEFAULT_AUTH_REQUEST` and sign itself in from the environment key at
/// startup, which makes an environment-only key work — and does it by writing
/// that key, in plaintext, into the user's own `auth.json` under `CODEX_HOME`,
/// where it then outlives the variable and is used in preference to it. A
/// gateway starting an agent must not move the operator's credential onto the
/// user's disk, so it is not asked for. Setting
/// `cli_auth_credentials_store = "ephemeral"` does not avoid the write, which
/// was checked against the pinned adapter rather than assumed.
fn process_environment(agent: AgentId) -> BTreeMap<OsString, OsString> {
    // `PATH` is the one entry that is not simply inherited: under launchd this
    // process's own `PATH` is launchd's, not the user's, and an agent given it
    // cannot find the tools every terminal on that machine can.
    // `agent_search_path` decides which of the two is handed over; this reads
    // the two it decides between, and nothing else here knows the rule.
    inherited_environment(
        agent,
        agent_search_path(
            std::env::var_os("NESSA_AGENT_PATH"),
            std::env::var_os("PATH"),
        ),
        |key: &str| std::env::var_os(key),
    )
}

/// `process_environment` with the search path already decided, and the rest
/// read through `lookup` rather than from this process.
///
/// Both for the same reason: what the agent is actually launched with has to be
/// readable back without a test writing to the environment every other test is
/// reading. `path` is the entry `agent_search_path` already settled; `lookup`
/// is every other key.
fn inherited_environment(
    agent: AgentId,
    path: Option<OsString>,
    lookup: impl Fn(&str) -> Option<OsString>,
) -> BTreeMap<OsString, OsString> {
    let mut environment = BTreeMap::new();
    let vendor: &[&str] = match agent {
        AgentId::Claude => &["CLAUDE_CONFIG_DIR"],
        AgentId::Codex => &["CODEX_HOME"],
        AgentId::Opencode => &[
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
            "XDG_STATE_HOME",
        ],
    };
    // `PATH` is not in this list: it is decided above rather than inherited.
    let shared = ["HOME", "USER", "LOGNAME", "TMPDIR"];
    for key in shared.into_iter().chain(vendor.iter().copied()) {
        if let Some(value) = lookup(key) {
            environment.insert(key.into(), value);
        }
    }
    if let Some(path) = path {
        environment.insert("PATH".into(), path);
    }
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
        // OpenCode credentials have one owner: the injected stage-scoped
        // source used by the current-agent resolver.
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

/// Exactly what a launched agent's environment is, for anything that has to
/// ask about the agent rather than start it.
///
/// The readiness probe runs the agent's own tool to ask whether it is signed
/// in, and an answer from a different environment is an answer about a
/// different installation: `CODEX_HOME` decides which account it reads, and the
/// session-bus variables decide whether a keyring can be opened at all.
/// Inheriting this server's whole environment would let the probe find a
/// sign-in the launch then cannot use.
pub(super) fn launch_environment(agent: AgentId) -> BTreeMap<OsString, OsString> {
    let mut environment = process_environment(agent);
    environment.extend(credential_environment(agent));
    environment
}

/// Which model catalog entries an agent's harness is allowed to run.
///
/// Not a preference: each harness speaks to one vendor's API and is signed in
/// to it, so a catalog entry from another vendor is a model that agent cannot
/// reach, and saying so at startup beats a provider refusing every prompt.
fn catalog_provider(agent: AgentId) -> &'static str {
    match agent {
        AgentId::Claude => "anthropic",
        AgentId::Codex => "openai",
        AgentId::Opencode => "opencode",
    }
}

/// The model this agent runs, as the catalog records it.
///
/// Read here rather than inside the provider because two things need it: the
/// provider is built for it, and uploaded images are fitted to the image
/// limits it publishes.
pub(super) fn model(
    agent: AgentId,
    config: &AgentsConfig,
    runtime: &AgentRuntime,
) -> Result<ModelMetadata, RunError> {
    model_by_id(agent, config, &runtime.model)
}

fn model_by_id(
    agent: AgentId,
    config: &AgentsConfig,
    model_id: &str,
) -> Result<ModelMetadata, RunError> {
    ModelMetadata::try_from(
        load_catalog(
            File::open(&config.catalog)
                .map_err(|_| RunError::Agent("cannot read model catalog".into()))?,
        )
        .map_err(|e| RunError::Agent(e.to_string()))?
        .select(catalog_provider(agent), model_id)
        .map_err(|e| RunError::Agent(e.to_string()))?,
    )
    .map_err(|e| RunError::Agent(e.to_string()))
}

/// The image limits every configured agent can meet.
///
/// Uploads are kept once and shared: one attachment store, holding images for
/// conversations that each run on their own agent. So an image is fitted to
/// what the strictest configured model accepts rather than to the selected
/// one's — fitting it to one agent's model and sending it to another's is how
/// an image that uploaded cleanly comes back refused at the moment it is sent,
/// which is the one place there is nothing left to do about it.
///
/// `None` is a gateway that keeps no images: either no configured model
/// publishes image input, or the models between them share no encoding, and in
/// both cases there is no image this gateway could store and then send.
pub(super) fn image_limits(
    config: &AgentsConfig,
    deferred_models: &[(AgentId, &str)],
) -> Result<Option<ImageInputLimits>, RunError> {
    let mut strictest: Option<ImageInputLimits> = None;
    let configured = config
        .agents()
        .into_iter()
        .map(|(agent, runtime)| (agent, runtime.model.as_str()));
    for (agent, model_id) in configured.chain(deferred_models.iter().copied()) {
        let Some(limits) = model_by_id(agent, config, model_id)?.image_input().cloned() else {
            // A model that takes no images cannot be met by any image at all.
            return Ok(None);
        };
        strictest = Some(match strictest {
            None => limits,
            Some(held) => match narrower(&held, &limits) {
                Some(both) => both,
                // No encoding both accept, so nothing is storable for both.
                None => return Ok(None),
            },
        });
    }
    Ok(strictest)
}

/// The limits an image has to meet to satisfy both models.
///
/// Every figure is the smaller of the two, and the encodings are those both
/// accept, in the first's published order. `None` when that leaves no encoding.
/// Taking the smaller of each edge cannot break the rule that neither smaller
/// edge exceeds the maximum: each model already satisfies it, so the smallest
/// maximum is at least its own model's smaller edges, and so at least the
/// smallest of them.
fn narrower(held: &ImageInputLimits, other: &ImageInputLimits) -> Option<ImageInputLimits> {
    let media_types: Vec<_> = held
        .media_types()
        .iter()
        .filter(|media_type| other.media_types().contains(media_type))
        .copied()
        .collect();
    if media_types.is_empty() {
        return None;
    }
    ImageInputLimits::new(
        media_types,
        held.max_encoded_bytes().min(other.max_encoded_bytes()),
        held.max_edge_px().min(other.max_edge_px()),
        held.many_images_max_edge_px()
            .min(other.many_images_max_edge_px()),
        held.native_long_edge_px().min(other.native_long_edge_px()),
    )
    .ok()
}

/// Build a provider for every configured agent that can be built.
///
/// All of them, not only the selected one: a conversation records the agent it
/// was created on and is reopened on that same agent afterwards, so a server
/// that had only built the selected one could not reopen the conversations
/// already on disk.
///
/// Every one that can be, and not every one: an agent whose provider cannot be
/// built is left out of the map rather than ending the build of the rest. What
/// [`build::provider`] refuses on is ordinary and local to one agent — a model
/// the catalog does not serve under that agent's vendor, a command that is not
/// an existing file, token limits the pair will not take — and none of that is
/// a statement about the other agents. Failing all of them together meant a
/// person with Claude installed and Codex merely configured got a gateway that
/// would not start, with a message about Codex and no way to reach the setup
/// page that would have fixed it.
///
/// Left out is a real answer downstream rather than a silence: the conversation
/// service refuses an agent it has no provider for with
/// `ConversationError::AgentNotConfigured`, which reaches a client as
/// `agent_not_configured` on the one conversation that asked for it. What is
/// *not* downgraded is the agent the installation is set to use. That one is
/// still fatal, because a server that cannot start a conversation on the agent
/// it is set to is not a degraded server.
///
/// Most of readiness is answered somewhere else and deliberately stays that
/// way: the current-agent resolver reads the managed store on every ask, so an
/// agent missing here because it is not installed yet still reports
/// `not-installed` now and `ready` after a later verified install. Narrowing
/// the probe to what was built at startup would freeze that answer to what was
/// true once.
///
/// That holds for everything an install can change and for nothing else, which
/// is why the agents left out come back in two groups rather than one. See
/// [`ConfiguredAgents::unavailable`].
#[cfg(unix)]
pub(super) fn providers(
    config: &AgentsConfig,
    directory: &Path,
    clock: Arc<dyn Clock>,
    images: Arc<dyn UserImageSource>,
    credentials: Arc<dyn AgentCredentialSource>,
    deferred: &HashSet<AgentId>,
) -> Result<ConfiguredAgents, RunError> {
    let configured: HashSet<_> = config
        .agents()
        .into_iter()
        .map(|(agent, _)| agent)
        .chain(deferred.iter().copied())
        .collect();
    config.validate_for(&configured)?;
    let selected = config.selected_from(&configured)?;
    let mut providers = HashMap::new();
    let mut unavailable = HashSet::new();
    let dependencies = ProviderDependencies {
        directory: directory.to_owned(),
        clock,
        images,
        credentials,
    };
    for (agent, runtime) in config.agents() {
        if deferred.contains(&agent) {
            continue;
        }
        // Every agent is given the source, not only the one whose profile is
        // known to use it: the runtime sends an image only to an agent that
        // advertised `promptCapabilities.image`, so an agent that takes none is
        // offered none without this having to know which those are.
        let build::ProviderComposition {
            provider,
            execution_audit,
        } = match build::provider(
            agent,
            config,
            runtime,
            &dependencies,
            credential_environment(agent),
        ) {
            Ok(provider) => provider,
            Err(failure) if agent == selected => return Err(failure),
            Err(failure) => {
                // Loud, because it is the only place the reason is said. A
                // person who never opens a conversation on this agent will see
                // nothing else, and the refusal downstream knows only that
                // there is no provider.
                tracing::error!(
                    agent = agent.name(),
                    %failure,
                    "configured agent is unavailable this run; the others are unaffected"
                );
                // Asked after the failure rather than before it, because it is
                // not a second opinion on the failure. It is the one question
                // about it readiness needs answered: is this something
                // installing the agent would fix?
                if build::present(config, runtime) {
                    unavailable.insert(agent);
                }
                continue;
            }
        };
        providers.insert(
            agent,
            ConversationAgent {
                provider,
                execution_audit,
                reserved_output_tokens: runtime.output_tokens,
                // Filled in by whoever has somewhere to keep warm-up records.
                // This builds providers and knows nothing about the durable
                // directories a warm-up writes to.
                readiness: None,
            },
        );
    }
    Ok(ConfiguredAgents {
        providers,
        unavailable,
    })
}

/// Build one provider from a launch and credential observation owned by composition.
#[cfg(unix)]
pub(super) fn provider_for(
    agent: AgentId,
    config: &AgentsConfig,
    runtime: &AgentRuntime,
    dependencies: &ProviderDependencies,
    credential_environment: BTreeMap<OsString, OsString>,
) -> Result<ConversationAgent, RunError> {
    let build::ProviderComposition {
        provider,
        execution_audit,
    } = build::provider(agent, config, runtime, dependencies, credential_environment)?;
    Ok(ConversationAgent {
        provider,
        execution_audit,
        reserved_output_tokens: runtime.output_tokens,
        readiness: None,
    })
}
#[cfg(not(unix))]
pub(super) fn providers(
    config: &AgentsConfig,
    _: &Path,
    _: Arc<dyn Clock>,
    _: Arc<dyn UserImageSource>,
    _: Arc<dyn AgentCredentialSource>,
    deferred: &HashSet<AgentId>,
) -> Result<ConfiguredAgents, RunError> {
    let configured: HashSet<_> = config
        .agents()
        .into_iter()
        .map(|(agent, _)| agent)
        .chain(deferred.iter().copied())
        .collect();
    config.validate_for(&configured)?;
    Err(RunError::Agent(
        "ACP agents require Unix process supervision".into(),
    ))
}

/// What building every configured agent settled.
pub(super) struct ConfiguredAgents {
    /// Every agent a conversation can be created on or reopened on this run.
    pub providers: HashMap<AgentId, ConversationAgent>,
    /// The configured agents that could not be built although everything they
    /// launch was already on the machine.
    ///
    /// A narrower set than "absent from [`Self::providers`]", and the
    /// difference is the whole reason it is carried separately. An agent
    /// missing only because its command is not installed yet is one an install
    /// fixes, and readiness has to go on saying `not-installed` for it, or
    /// setup stops offering the install button for the one agent it would help.
    /// An agent whose command is right there and which still could not be built
    /// failed on something no amount of installing re-asks: a model its vendor
    /// does not serve, token limits the pair will not take, a catalog that will
    /// not parse. Reporting that one `ready` offers a person a conversation
    /// that cannot be opened, so composition drops it from the launch files the
    /// probe reads and readiness answers `not-configured` — not set up on this
    /// installation, which is what happened.
    ///
    /// Empty in the ordinary case, including the one this whole path exists
    /// for: an agent in the configuration that nobody has installed.
    pub unavailable: HashSet<AgentId>,
}

/// Effects shared by every provider generation built for one conversation root.
#[derive(Clone)]
pub(super) struct ProviderDependencies {
    pub(super) directory: PathBuf,
    pub(super) clock: Arc<dyn Clock>,
    pub(super) images: Arc<dyn UserImageSource>,
    pub(super) credentials: Arc<dyn AgentCredentialSource>,
}
#[cfg(unix)]
mod build {
    use super::super::agent_budgets as budgets;
    use super::{
        AgentId, AgentRuntime, AgentsConfig, CredentialedClaudeProvider, ProviderDependencies,
        RunError,
    };
    use crate::conversation::infrastructure::DurableExecutionAudit;
    use nessa_sdk::{
        application::agent_execution::{
            agents::AgentError,
            executions::ExecutionAudit,
            providers::{AgentProvider, UserImageSource},
        },
        domain::{
            agent_execution::{
                permissions::PermissionOfferPolicy,
                prompts::{
                    PromptSource, PromptSourceKind, SystemPrompt, SystemPromptBuilder, UserMessage,
                },
            },
            common::value_objects::TokenLimits,
        },
        infrastructure::{
            acp::sessions::AcpConfig, codex_acp::sessions::CodexAcpProvider,
            opencode_acp::sessions::OpencodeAcpProvider,
        },
    };
    use std::{collections::BTreeMap, ffi::OsString, path::PathBuf, sync::Arc};

    pub(super) struct ProviderComposition {
        pub(super) provider: Arc<dyn AgentProvider>,
        pub(super) execution_audit: Arc<dyn ExecutionAudit>,
    }

    /// The largest ACP frame, derived from the largest message rather than
    /// chosen beside it. One `session/prompt` carries every image of a message
    /// as base64, which grows bytes by a third: `UserMessage::MAX_IMAGE_BYTES`
    /// (10 MiB) becomes 13⅓ MiB. With 8 KiB of text and the JSON around each
    /// block, that fits 16 MiB and nothing smaller that is a round number. 16
    /// MiB is also the most `AcpConfig` accepts, so the image budget cannot
    /// grow without the SDK's ceiling growing first.
    const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
    const _: () = assert!(
        UserMessage::MAX_IMAGE_BYTES as usize / 3 * 4 + 1024 * 1024 <= MAX_FRAME_BYTES,
        "one message's images, encoded, must fit one ACP frame"
    );
    /// What an agent may send us, which is the buffer this host can be made to
    /// allocate for one frame and has nothing to do with the prompts it writes.
    /// An agent answers in text, so it keeps the mebibyte it had before images.
    const MAX_INCOMING_FRAME_BYTES: usize = 1024 * 1024;

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

    /// Whether everything this agent would launch is on the machine now.
    ///
    /// Asked in two places and written once, because the two have to agree.
    /// [`provider`] refuses when the answer is no, and [`super::providers`] uses
    /// it to tell that refusal apart from every other one: a command that is not
    /// there yet becomes a command that is there the moment somebody installs
    /// it, and nothing else [`provider`] refuses on works that way.
    ///
    /// Re-asked of the filesystem each time rather than remembered. That is the
    /// whole point of it: the answer is allowed to change while the gateway
    /// runs.
    pub(super) fn present(config: &AgentsConfig, runtime: &AgentRuntime) -> bool {
        config
            .workspace
            .canonicalize()
            .is_ok_and(|workspace| workspace.is_dir())
            && runtime.command.executable().is_file()
            && runtime.paths().iter().all(|path| path.exists())
    }

    /// The launch configuration composition injects, separated from resolving
    /// what goes into it so a test can read back the values actually used.
    /// Everything here is a decision; nothing here reads the filesystem or this
    /// process's own environment.
    ///
    /// `images` is the one dependency rather than a decision: the source a
    /// binding reads a message's uploads from, and `None` is a binding that
    /// sends none.
    pub(super) fn launch_configuration(
        config: &AgentsConfig,
        runtime: &AgentRuntime,
        workspace: PathBuf,
        environment: BTreeMap<OsString, OsString>,
        credential_environment: BTreeMap<OsString, OsString>,
        images: Option<Arc<dyn UserImageSource>>,
    ) -> AcpConfig {
        AcpConfig {
            executable: runtime.command.clone(),
            arguments: runtime.args.iter().map(Into::into).collect(),
            environment,
            credential_environment,
            workspace,
            tools_enabled: runtime.tools_enabled,
            mcp_servers: config.mcp_servers.clone(),
            permissions: PermissionOfferPolicy::once_only(),
            // All four from protocol/defaults/agent-startup-budgets.json,
            // which the client compiles in too: a client that gives up before
            // the gateway has finished failing never sees the typed answer.
            // Spawning is the operating system's work — a runtime staged by a
            // fresh install is scanned on its first execution — so it has its
            // own, far larger budget than protocol work.
            //
            // One table for every agent, because a budget here is the user's
            // patience with a cold runtime rather than anything a vendor
            // decides.
            launch_timeout: budgets::launch_timeout(),
            startup_timeout: budgets::startup_timeout(),
            execution_timeout: None,
            shutdown_grace: budgets::shutdown_grace(),
            kill_timeout: budgets::kill_timeout(),
            event_capacity: 256,
            max_frame_bytes: MAX_FRAME_BYTES,
            max_incoming_frame_bytes: MAX_INCOMING_FRAME_BYTES,
            images,
        }
    }

    pub(super) fn provider(
        agent: AgentId,
        config: &AgentsConfig,
        runtime: &AgentRuntime,
        dependencies: &ProviderDependencies,
        credential_environment: BTreeMap<OsString, OsString>,
    ) -> Result<ProviderComposition, RunError> {
        let invalid = |error| RunError::Agent(format!("{error}"));
        let model = super::model(agent, config, runtime)?;
        let workspace = config
            .workspace
            .canonicalize()
            .map_err(|_| RunError::Agent("workspace must exist".into()))?;
        if !present(config, runtime) {
            return Err(RunError::Agent(format!(
                "{}: its command must be an existing file and every absolute path it is given must exist; workspace must be a directory",
                agent.name()
            )));
        }
        let limits = TokenLimits::new(runtime.context_tokens, runtime.output_tokens)
            .map_err(|e| RunError::Agent(e.to_string()))?;
        let audit = Arc::new(
            DurableExecutionAudit::new(
                dependencies.directory.join("audit"),
                dependencies.clock.clone(),
            )
            .map_err(invalid)?,
        );
        let acp = launch_configuration(
            config,
            runtime,
            workspace,
            super::process_environment(agent),
            credential_environment,
            Some(dependencies.images.clone()),
        );
        let prompt = system_prompt()?;
        let failed = |e: AgentError| RunError::Agent(format!("{}: {e}", agent.name()));
        let provider: Arc<dyn AgentProvider> = match agent {
            AgentId::Claude => Arc::new(
                CredentialedClaudeProvider::new(
                    acp,
                    model,
                    limits,
                    audit.clone(),
                    prompt,
                    dependencies.credentials.clone(),
                )
                .map_err(failed)?,
            ),
            AgentId::Codex => Arc::new(
                CodexAcpProvider::new(acp, &model, limits, audit.clone())
                    .map_err(failed)?
                    .with_system_prompt(prompt),
            ),
            // No prompt, because there is nowhere to put one that Opencode can
            // be shown to read: its binding offers no `with_system_prompt` for
            // exactly that reason, and this arm not calling one is the compiler
            // enforcing it rather than a convention someone has to remember.
            // Opencode therefore runs under its own instructions. What keeps
            // that difference from mattering yet is not the session mode, which
            // only denies edits, but the permission policy its binding launches
            // it with: reading and searching allowed, everything else denied,
            // including this server's own MCP shell tool.
            AgentId::Opencode => Arc::new(
                OpencodeAcpProvider::new(acp, &model, limits, audit.clone()).map_err(failed)?,
            ),
        };
        Ok(ProviderComposition {
            provider,
            execution_audit: audit,
        })
    }
}

#[cfg(test)]
#[path = "../../tests/conversation/configuration.rs"]
mod tests;

#[cfg(all(test, unix))]
#[path = "../../tests/conversation/launch_configuration.rs"]
mod launch_configuration_tests;
