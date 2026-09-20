//! Desktop composition: relocatable application resources plus private user data.
//!
//! The credentials this app needs are created by
//! [`super::provisioning::ensure_local_credentials`], which the developer loop
//! asks for by the same name; nothing here provisions separately.
use super::{
    agent::{AgentRuntime, AgentsConfig},
    runtime_config::RuntimeConfig,
};
use crate::{agents::domain::AgentId, core::RunError, desktop_runtime::domain::RunningRuntime};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The agent the desktop starts with when nothing else has been chosen.
///
/// Which agent a caller that names none runs on: Claude, because that is the
/// agent Nessa shipped with and the one every conversation already on disk
/// belongs to.
const DEFAULT_AGENT: AgentId = AgentId::Claude;

/// How the desktop launches one bundled agent, relative to the bundle root, or
/// nothing for an agent the desktop does not ship.
///
/// A command and its arguments, so an agent that speaks ACP through the bundled
/// Node runtime and one that ships as its own executable are both sayable here.
/// Both agents Nessa bundles today are the first kind.
///
/// Opencode is not bundled. It is a whole runtime of its own — nearly two
/// hundred megabytes, against a few for a Node adapter — and shipping it would
/// put that in every download for the people who already have an agent. It is
/// meant to be fetched onto the machine that wants it instead, which is why
/// this says nothing about where it lives.
///
/// Nothing writes that yet, and this comment does not pretend otherwise. The
/// installer is on another branch and, even there, it unpacks a binary and
/// prints a report — it does not record a runtime, and no other code turns the
/// unpacked path into one. So `None` here is the whole truth at this commit:
/// the desktop does not ship Opencode and nothing else supplies it either.
/// What is missing between the two is a step that records the installed launch,
/// and model catalog entries for OpenCode Zen to select against.
fn bundled_launch(agent: AgentId) -> Option<(&'static str, &'static str)> {
    match agent {
        AgentId::Claude => Some((
            "node",
            "claude-acp/node_modules/@agentclientprotocol/claude-agent-acp/dist/index.js",
        )),
        AgentId::Codex => Some((
            "node",
            "codex-acp/node_modules/@agentclientprotocol/codex-acp/dist/index.js",
        )),
        AgentId::Opencode => None,
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
        // Unreachable today: this is read for bundled agents, and Opencode is
        // not one. Named rather than wildcarded so that a new agent has to say
        // what it starts on. It is also not yet selectable — the shipped
        // catalog has no OpenCode Zen entries — so whatever configures Opencode
        // has to add those before this string means anything.
        AgentId::Opencode => "opencode/big-pickle",
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
    // Only the agents this desktop ships. A bundle checked for files it was
    // never meant to contain would refuse to start, so an agent with no bundled
    // launch is skipped here rather than looked for. Skipped is all it is: no
    // launch is written for it anywhere else yet either, so an unbundled agent
    // stays unconfigured and the picker says exactly that.
    let launches: Vec<(AgentId, PathBuf, PathBuf)> = AgentId::ALL
        .iter()
        .filter_map(|agent| {
            let (command, entry) = bundled_launch(*agent)?;
            Some((*agent, bundle.join(command), bundle.join(entry)))
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
