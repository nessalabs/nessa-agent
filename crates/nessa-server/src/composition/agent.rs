//! Trusted local agent configuration. Requests never select processes or workspaces.
//!
//! ```text
//!   config.json "agent"
//!     ├── shared: catalog, node, workspace, toolsEnabled, mcpServers
//!     ├── claude: { acpEntry, model, contextTokens, outputTokens }
//!     ├── codex:  { acpEntry, model, contextTokens, outputTokens }
//!     └── selected: which of them a caller that names none runs on
//! ```
//!
//! What every agent on this machine shares is stated once: they run the same
//! Node runtime against the same workspace and are offered the same Nessa MCP
//! servers, because that is what makes them alternatives rather than separate
//! installations. What differs is the harness entry point they are started
//! with, the model they run, and the budget they run it in.
//!
//! An agent absent from the configuration is one this server cannot start.
//! That is reported where it is asked about — setup says the agent is not
//! installed — rather than substituted for at startup.
use crate::agents::domain::AgentId;
use crate::conversation::application::ConversationAgent;
use crate::core::RunError;
use nessa_auth::application::ports::Clock;
use nessa_sdk::infrastructure::acp::sessions::StdioMcpServer;
use serde::Deserialize;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct AgentConfig {
    pub catalog: PathBuf,
    pub node: PathBuf,
    pub workspace: PathBuf,
    #[serde(default)]
    pub tools_enabled: bool,
    #[serde(default)]
    pub mcp_servers: Vec<StdioMcpServer>,
    /// The agent a conversation runs on when nothing else names one.
    ///
    /// Left out where only one agent is configured, because there is nothing to
    /// choose between; required where more than one is, because guessing which
    /// of two configured agents the operator meant is not a default, it is a
    /// coin toss with someone else's work on it.
    #[serde(default)]
    pub selected: Option<String>,
    #[serde(default)]
    pub claude: Option<AgentRuntime>,
    #[serde(default)]
    pub codex: Option<AgentRuntime>,
}

/// What one agent is started as, within the shared configuration above.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct AgentRuntime {
    pub acp_entry: PathBuf,
    pub model: String,
    #[serde(default = "context_tokens")]
    pub context_tokens: u32,
    #[serde(default = "output_tokens")]
    pub output_tokens: u32,
}

impl AgentConfig {
    /// Each configured agent, in the order [`AgentId::ALL`] lists them.
    pub fn agents(&self) -> Vec<(AgentId, &AgentRuntime)> {
        AgentId::ALL
            .iter()
            .filter_map(|agent| self.runtime(*agent).map(|runtime| (*agent, runtime)))
            .collect()
    }

    /// What this configuration says about one agent, or nothing when it says
    /// nothing about it.
    pub fn runtime(&self, agent: AgentId) -> Option<&AgentRuntime> {
        match agent {
            AgentId::Claude => self.claude.as_ref(),
            AgentId::Codex => self.codex.as_ref(),
        }
    }

    /// The agent a caller that names none runs on.
    ///
    /// # Errors
    /// Returns [`RunError::Agent`] for a name no adapter exists for, a name with
    /// no configuration under it, no agents at all, or several agents with no
    /// choice stated between them.
    pub fn selected(&self) -> Result<AgentId, RunError> {
        let configured = self.agents();
        if let Some(name) = &self.selected {
            let agent = AgentId::parse(name).ok_or_else(|| {
                RunError::Agent(format!("selected agent \"{name}\" has no adapter in Nessa"))
            })?;
            if self.runtime(agent).is_none() {
                return Err(RunError::Agent(format!(
                    "selected agent \"{name}\" has no configuration under \"agent\""
                )));
            }
            return Ok(agent);
        }
        match configured.as_slice() {
            [(agent, _)] => Ok(*agent),
            [] => Err(RunError::Agent(
                "configure at least one agent under \"agent\"".into(),
            )),
            _ => Err(RunError::Agent(
                "several agents are configured; name one in \"selected\"".into(),
            )),
        }
    }

    fn validate(&self) -> Result<(), RunError> {
        self.selected()?;
        if [&self.catalog, &self.node, &self.workspace]
            .iter()
            .any(|path| !path.is_absolute())
        {
            return Err(RunError::Agent("agent paths must be absolute".into()));
        }
        for (agent, runtime) in self.agents() {
            if !runtime.acp_entry.is_absolute()
                || runtime.model.trim().is_empty()
                || runtime.output_tokens == 0
                || runtime.context_tokens <= runtime.output_tokens
            {
                return Err(RunError::Agent(format!(
                    "{}: acpEntry must be absolute, model nonempty, and token limits positive with room for input",
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

/// Which model catalog entries an agent's harness is allowed to run.
///
/// Not a preference: each harness speaks to one vendor's API and is signed in
/// to it, so a catalog entry from another vendor is a model that agent cannot
/// reach, and saying so at startup beats a provider refusing every prompt.
fn catalog_provider(agent: AgentId) -> &'static str {
    match agent {
        AgentId::Claude => "anthropic",
        AgentId::Codex => "openai",
    }
}

/// Build a provider for every configured agent.
///
/// All of them, not only the selected one: a conversation records the agent it
/// was created on and is reopened on that same agent afterwards, so a server
/// that had only built the selected one could not reopen the conversations
/// already on disk.
#[cfg(unix)]
pub(super) fn providers(
    config: &AgentConfig,
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
    config: &AgentConfig,
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
    use super::{catalog_provider, AgentConfig, AgentId, AgentRuntime, RunError};
    use crate::conversation::infrastructure::DurableExecutionAudit;
    use nessa_auth::application::ports::Clock;
    use nessa_sdk::{
        application::agent_execution::providers::AgentProvider,
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
        },
    };
    use std::{collections::BTreeMap, fs::File, path::Path, sync::Arc, time::Duration};

    /// The environment every agent process inherits, beyond its credentials.
    ///
    /// `env_clear` is what the bindings launch with, so anything an agent needs
    /// has to be named. The vendor-specific entries are each agent's own
    /// directory variable: naming both for both agents would be shorter and
    /// would also hand each agent a pointer into the other's configuration.
    fn process_environment(agent: AgentId) -> BTreeMap<std::ffi::OsString, std::ffi::OsString> {
        let mut environment = BTreeMap::new();
        let vendor = match agent {
            AgentId::Claude => "CLAUDE_CONFIG_DIR",
            AgentId::Codex => "CODEX_HOME",
        };
        for key in ["PATH", "HOME", "USER", "LOGNAME", "TMPDIR", vendor] {
            if let Some(value) = std::env::var_os(key) {
                environment.insert(key.into(), value);
            }
        }
        environment
    }

    /// The sign-in this agent is started with, read from this server's own
    /// environment. Named per agent so neither is handed the other's key.
    fn credential_environment(agent: AgentId) -> BTreeMap<std::ffi::OsString, std::ffi::OsString> {
        let keys: &[&str] = match agent {
            AgentId::Claude => &["ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN"],
            AgentId::Codex => &["CODEX_API_KEY", "OPENAI_API_KEY"],
        };
        let mut environment = BTreeMap::new();
        for key in keys {
            if let Some(value) = std::env::var_os(key) {
                environment.insert((*key).into(), value);
            }
        }
        environment
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
        config: &AgentConfig,
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
        if !workspace.is_dir() || !config.node.is_file() || !runtime.acp_entry.is_file() {
            return Err(RunError::Agent(format!(
                "{}: node and acpEntry must be existing absolute files; workspace must be a directory",
                agent.name()
            )));
        }
        let limits = TokenLimits::new(runtime.context_tokens, runtime.output_tokens)
            .map_err(|e| RunError::Agent(e.to_string()))?;
        let audit =
            Arc::new(DurableExecutionAudit::new(directory.join("audit"), clock).map_err(invalid)?);
        let acp = AcpConfig {
            executable: config.node.clone(),
            arguments: vec![runtime.acp_entry.clone().into_os_string()],
            environment: process_environment(agent),
            credential_environment: credential_environment(agent),
            workspace,
            tools_enabled: config.tools_enabled,
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
        let failed = |e: nessa_sdk::application::agent_execution::agents::AgentError| {
            RunError::Agent(format!("{}: {e}", agent.name()))
        };
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
        };
        Ok(provider)
    }
}

#[cfg(test)]
#[path = "../../tests/conversation/configuration.rs"]
mod tests;
