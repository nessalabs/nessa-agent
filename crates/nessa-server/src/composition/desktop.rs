//! Desktop composition: relocatable application resources plus private user data.
//!
//! The credentials this app needs are created by
//! [`super::provisioning::ensure_local_credentials`], which the developer loop
//! asks for by the same name; nothing here provisions separately.
use super::{agent::AgentConfig, runtime_config::RuntimeConfig};
use crate::{core::RunError, desktop_runtime::domain::RunningRuntime};
use std::path::Path;

pub(super) fn configure(
    settings: &mut RuntimeConfig,
    bundle: &Path,
    data: &Path,
) -> Result<(), RunError> {
    if !bundle.is_absolute() {
        return Err(failure("runtime directory must be absolute"));
    }
    let node = bundle.join("node");
    let entry =
        bundle.join("claude-acp/node_modules/@agentclientprotocol/claude-agent-acp/dist/index.js");
    let catalog = bundle.join("models.json");
    let mcp = bundle.join("nessa-mcp");
    for path in [&node, &entry, &catalog, &mcp] {
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
            acp_entry: entry.clone(),
            workspace: data.join(relative_workspace),
            model: "claude-sonnet-5".into(),
            tools_enabled: true,
            mcp_servers: vec![],
            context_tokens: 100_000,
            output_tokens: 4096,
        });
    }
    let agent = settings.agent.as_mut().expect("agent configured above");
    agent.catalog = catalog;
    agent.node = node;
    agent.acp_entry = entry;
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
