//! Desktop composition: relocatable application resources plus private user data.
use super::{
    agent::{AgentRuntime, AgentsConfig},
    runtime_config::RuntimeConfig,
};
use crate::{
    agents::domain::AgentId, core::RunError, desktop_runtime::domain::RunningRuntime,
    env::Environment,
};
use std::collections::HashMap;
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

/// How the desktop launches each bundled agent, relative to the bundle root.
///
/// A command and its arguments, so an agent that speaks ACP through the bundled
/// Node runtime and one that would ship as its own executable are both sayable
/// here. Both agents Nessa bundles today are the first kind.
fn bundled_launch(agent: AgentId) -> (&'static str, &'static str) {
    match agent {
        AgentId::Claude => (
            "node",
            "claude-acp/node_modules/@agentclientprotocol/claude-agent-acp/dist/index.js",
        ),
        AgentId::Codex => (
            "node",
            "codex-acp/node_modules/@agentclientprotocol/codex-acp/dist/index.js",
        ),
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
    let catalog = bundle.join("models.json");
    let mcp = bundle.join("nessa-mcp");
    let launches: Vec<(AgentId, PathBuf, PathBuf)> = AgentId::ALL
        .iter()
        .map(|agent| {
            let (command, entry) = bundled_launch(*agent);
            (*agent, bundle.join(command), bundle.join(entry))
        })
        .collect();
    for path in [&catalog, &mcp].into_iter().chain(
        launches
            .iter()
            .flat_map(|(_, command, entry)| [command, entry]),
    ) {
        if !path.is_file() {
            return Err(failure(format!(
                "missing bundled runtime file: {}",
                path.display()
            )));
        }
    }
    if settings.agents.is_none() {
        let relative_workspace = Path::new("workspaces/default");
        nessa_local_storage::create_directory_beneath(data, relative_workspace).map_err(failure)?;
        settings.agents = Some(AgentsConfig {
            catalog: catalog.clone(),
            workspace: data.join(relative_workspace),
            mcp_servers: vec![],
            selected: None,
            runtimes: HashMap::new(),
        });
    }
    let agents = settings.agents.as_mut().expect("agents configured above");
    // Whichever agent the existing configuration already answered for stays the
    // default. Filling in the agent it did not mention must not silently move a
    // running installation onto a different agent, so the one agent an existing
    // configuration named becomes the stated choice before the rest are added.
    //
    // Nessa's own default is written only into a configuration that names no
    // agent at all — a first run. Where someone has configured several agents
    // and left `selected` out, that is the same unanswered question the gateway
    // refuses to guess at, and it is refused here too: picking for them would
    // send every conversation to a vendor they never chose, and the desktop is
    // where almost nobody would ever be told.
    let selected =
        agents
            .selected
            .take()
            .or_else(|| match (agents.agents().as_slice(), agents.unknown()) {
                ([], None) => Some(DEFAULT_AGENT.name().into()),
                ([(only, _)], None) => Some(only.name().into()),
                _ => None,
            });
    agents.selected = selected;
    agents.catalog = catalog;
    for (id, command, entry) in launches {
        let args = vec![entry.to_string_lossy().into_owned()];
        agents
            .runtimes
            .entry(id.name().into())
            // Only the bundled launch is replaced. A configured model and its
            // budgets are the user's and survive every upgrade.
            .and_modify(|runtime| {
                runtime.command = command.clone();
                runtime.args = args.clone();
            })
            .or_insert_with(|| AgentRuntime {
                command,
                args,
                model: default_model(id).into(),
                tools_enabled: true,
                context_tokens: 100_000,
                output_tokens: 4096,
            });
    }
    // Only the Nessa-owned server is replaced. User-configured MCP servers retain their settings.
    agents.mcp_servers.retain(|server| server.name != "nessa");
    agents
        .mcp_servers
        .push(nessa_sdk::infrastructure::acp::sessions::StdioMcpServer {
            name: "nessa".into(),
            command: mcp,
            args: vec![
                "--workspace".into(),
                agents.workspace.to_string_lossy().into_owned(),
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
