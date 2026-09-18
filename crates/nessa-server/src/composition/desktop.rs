//! Desktop composition: relocatable application resources plus private user data.
use super::{
    agent::{AgentConfig, AgentRuntime},
    runtime_config::RuntimeConfig,
};
use crate::{
    agents::domain::AgentId, core::RunError, desktop_runtime::domain::RunningRuntime,
    env::Environment,
};
use std::path::{Path, PathBuf};

pub(super) fn prepare(config: &Environment) -> Result<(), RunError> {
    let auth = config
        .auth_directory
        .as_ref()
        .ok_or_else(|| failure("missing data directory"))?;
    let root = auth
        .parent()
        .ok_or_else(|| failure("invalid data directory"))?;
    nessa_local_storage::create_directory(root).map_err(failure)?;
    if !auth.join("credentials.v1.json").exists() {
        super::auth_command::execute(&[
            "auth".into(),
            "init".into(),
            "--owner-token-file".into(),
            root.join("owner.token").to_string_lossy().into_owned(),
        ])?;
    }
    // Never rotate an existing surface credential on app startup.
    if !auth.join("surfaces/nessa-panel.token").exists() {
        super::auth_command::execute(&[
            "auth".into(),
            "provision-surface".into(),
            "--surface-id".into(),
            "nessa-panel".into(),
        ])?;
    }
    Ok(())
}

/// The agent the desktop starts with when nothing else has been chosen.
///
/// Both agents are bundled, so this decides only which one a caller that names
/// none runs on. Claude, because that is the agent Nessa shipped with and the
/// one every conversation already on disk belongs to.
const DEFAULT_AGENT: AgentId = AgentId::Claude;

/// Where each agent's harness sits inside the application bundle.
fn harness_entry(agent: AgentId) -> &'static str {
    match agent {
        AgentId::Claude => {
            "claude-acp/node_modules/@agentclientprotocol/claude-agent-acp/dist/index.js"
        }
        AgentId::Codex => "codex-acp/node_modules/@agentclientprotocol/codex-acp/dist/index.js",
    }
}

/// The model each agent runs until someone configures another.
///
/// A starting point rather than a recommendation: each is a current model from
/// the bundled catalog that the agent's own harness can reach, and `config.json`
/// replaces it without rebuilding.
fn default_model(agent: AgentId) -> &'static str {
    match agent {
        AgentId::Claude => "claude-sonnet-5",
        AgentId::Codex => "gpt-5.6-terra",
    }
}

pub(super) fn configure(
    settings: &mut RuntimeConfig,
    bundle: &Path,
    data: &Path,
) -> Result<(), RunError> {
    if !bundle.is_absolute() {
        return Err(failure("runtime directory must be absolute"));
    }
    let node = bundle.join("node");
    let catalog = bundle.join("models.json");
    let mcp = bundle.join("nessa-mcp");
    let entries: Vec<(AgentId, PathBuf)> = AgentId::ALL
        .iter()
        .map(|agent| (*agent, bundle.join(harness_entry(*agent))))
        .collect();
    for path in [&node, &catalog, &mcp]
        .into_iter()
        .chain(entries.iter().map(|(_, entry)| entry))
    {
        if !path.is_file() {
            return Err(failure(format!(
                "missing bundled runtime file: {}",
                path.display()
            )));
        }
    }
    if settings.agent.is_none() {
        let relative_workspace = Path::new("workspaces/default");
        nessa_local_storage::create_directory_beneath(data, relative_workspace).map_err(failure)?;
        settings.agent = Some(AgentConfig {
            catalog: catalog.clone(),
            node: node.clone(),
            workspace: data.join(relative_workspace),
            tools_enabled: true,
            mcp_servers: vec![],
            selected: None,
            claude: None,
            codex: None,
        });
    }
    let agent = settings.agent.as_mut().expect("agent configured above");
    // Whichever agent the existing configuration already answered for stays the
    // default. Filling in the agent it did not mention must not silently move a
    // running installation onto a different agent, and leaving the choice unset
    // once both are configured is a startup failure rather than a guess.
    let selected = agent
        .selected
        .take()
        .or_else(|| match agent.agents().as_slice() {
            [(only, _)] => Some(only.name().into()),
            _ => None,
        })
        .unwrap_or_else(|| DEFAULT_AGENT.name().into());
    agent.selected = Some(selected);
    agent.catalog = catalog;
    agent.node = node;
    for (id, entry) in entries {
        let slot = match id {
            AgentId::Claude => &mut agent.claude,
            AgentId::Codex => &mut agent.codex,
        };
        match slot {
            // Only the bundled harness path is replaced. A configured model and
            // its budgets are the user's and survive every upgrade.
            Some(runtime) => runtime.acp_entry = entry,
            None => {
                *slot = Some(AgentRuntime {
                    acp_entry: entry,
                    model: default_model(id).into(),
                    context_tokens: 100_000,
                    output_tokens: 4096,
                })
            }
        }
    }
    // Only the Nessa-owned server is replaced. User-configured MCP servers retain their settings.
    agent.mcp_servers.retain(|server| server.name != "nessa");
    agent
        .mcp_servers
        .push(nessa_sdk::infrastructure::acp::sessions::StdioMcpServer {
            name: "nessa".into(),
            command: mcp,
            args: vec![
                "--workspace".into(),
                agent.workspace.to_string_lossy().into_owned(),
                "--audit-directory".into(),
                data.join("process-audit").to_string_lossy().into_owned(),
            ],
        });
    Ok(())
}
fn failure(error: impl std::fmt::Display) -> RunError {
    RunError::Agent(error.to_string())
}

#[cfg(test)]
#[path = "../../tests/conversation/desktop.rs"]
mod tests;

/// Match process configuration to the actual bundle before publishing desktop health.
pub(super) fn runtime_identity(
    bundle: &Path,
    configured: String,
    generation: String,
    instance: String,
    process_id: u32,
) -> Result<RunningRuntime, RunError> {
    let installed: serde_json::Value =
        serde_json::from_slice(&std::fs::read(bundle.join("manifest.json"))?).map_err(failure)?;
    if installed
        .get("fingerprint")
        .and_then(serde_json::Value::as_str)
        != Some(configured.as_str())
    {
        return Err(failure(
            "configured desktop fingerprint differs from installed runtime manifest",
        ));
    }
    RunningRuntime::new(configured, instance, process_id, generation).map_err(failure)
}
