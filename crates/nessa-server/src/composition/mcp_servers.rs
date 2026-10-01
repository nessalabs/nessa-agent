//! The gateway's connection to each configured MCP server for each harness
//! session (ADR 344), composed before any agent is built.
//!
//! ```text
//! AgentsConfig.mcpServers ──▶ McpServers (each started by the gateway)
//!            │
//!            └──replaced by──▶ stand-ins: <this executable> mcp-relay <socket> <name> <digest>
//!                              ──▶ every agent's session/new
//! ```
//!
//! Arrows are what each is built from. A relay socket that cannot be bound
//! leaves MCP servers off for this run, logged: an agent is never handed a
//! server directly instead, because then its calls and an app's would reach
//! different sessions.
use super::agent::{agent_search_path, AgentsConfig};
use crate::core::RunError;
use crate::mcp_servers::{
    domain::{configuration_digest, relay_arguments},
    infrastructure::{bind, Relay},
};
use nessa_sdk::infrastructure::{
    acp::sessions::StdioMcpServer,
    clock::RuntimeClock,
    mcp::{McpServerLaunch, McpServers},
};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::net::UnixListener;

/// What composition hands the server lifecycle: the servers to start once it
/// is listening and stop once conversations have, and the relay to serve.
pub(super) struct McpComposition {
    pub(super) servers: McpServers,
    pub(super) relay: Arc<Relay>,
    pub(super) listener: UnixListener,
}

/// Where the relay socket of the namespace at `namespace` is, for the user
/// `uid`: `/tmp/nessa-mcp-<uid>/<16 hex of the namespace's digest>.sock`.
///
/// Short, because a socket's path has a platform limit (104 bytes on macOS)
/// that a namespace under a long data directory passes; in a directory of the
/// user's own, created private and refused when it is not (`relay::bind`); and
/// the same on every run of one namespace, so a stand-in's arguments, and the
/// context fingerprint that reads them, are too.
pub(super) fn relay_socket(namespace: &Path, uid: u32) -> PathBuf {
    let digest = Sha256::digest(namespace.as_os_str().as_encoded_bytes());
    let name: String = digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    PathBuf::from(format!("/tmp/nessa-mcp-{uid}")).join(format!("{name}.sock"))
}

/// What a configured server is started with, read through `lookup`: the
/// locale and the user's own variables, and the agents' search path
/// (`agent_search_path`) — the one the Nessa MCP shell tool had when an agent
/// started it.
pub(super) fn server_environment(
    lookup: impl Fn(&str) -> Option<OsString>,
) -> BTreeMap<OsString, OsString> {
    let mut environment: BTreeMap<OsString, OsString> = [
        "HOME", "USER", "LOGNAME", "TMPDIR", "LANG", "LC_ALL", "LC_CTYPE", "TZ",
    ]
    .into_iter()
    .filter_map(|key| lookup(key).map(|value| (key.into(), value)))
    .collect();
    if let Some(path) = agent_search_path(lookup("NESSA_AGENT_PATH"), lookup("PATH")) {
        environment.insert("PATH".into(), path);
    }
    environment
}

/// The stand-in for each server in `servers`, run by `gateway` over `socket`;
/// `None` when the socket's path is not UTF-8, which an argument must be.
pub(super) fn stand_ins(
    servers: &[StdioMcpServer],
    gateway: &Path,
    socket: &Path,
) -> Option<Vec<StdioMcpServer>> {
    let socket = socket.to_str()?;
    Some(
        servers
            .iter()
            .map(|server| StdioMcpServer {
                name: server.name.clone(),
                command: gateway.to_owned(),
                args: relay_arguments(
                    socket,
                    &server.name,
                    &configuration_digest(&server.command, &server.args),
                ),
            })
            .collect(),
    )
}

/// Take over `agents`' MCP servers: start nothing yet, bind the relay socket
/// at `socket` ([`relay_socket`]), and replace each server with its stand-in run by `gateway`.
/// `None` when no server is configured, or when the socket cannot be bound —
/// then `agents` is left with no MCP servers, and why is logged.
///
/// # Errors
///
/// [`RunError::Agent`] when the configured servers could not all be launched
/// as configured.
pub(super) fn compose(
    agents: &mut AgentsConfig,
    socket: &Path,
    gateway: &Path,
    environment: BTreeMap<OsString, OsString>,
) -> Result<Option<McpComposition>, RunError> {
    if agents.mcp_servers.is_empty() {
        return Ok(None);
    }
    let configured = std::mem::take(&mut agents.mcp_servers);
    let launches = configured
        .iter()
        .map(|server| McpServerLaunch {
            server: server.clone(),
            working_directory: agents.workspace.clone(),
            environment: environment.clone(),
        })
        .collect();
    let servers = McpServers::new(launches, Arc::new(RuntimeClock::new()))
        .map_err(|error| RunError::Agent(error.to_string()))?;
    let Some(stand_ins) = stand_ins(&configured, gateway, socket) else {
        tracing::error!(socket = %socket.display(), "MCP servers are off this run: the relay socket's path is not UTF-8");
        return Ok(None);
    };
    let listener = match bind(socket) {
        Ok(listener) => listener,
        Err(error) => {
            tracing::error!(socket = %socket.display(), %error, "MCP servers are off this run: the relay socket could not be bound");
            return Ok(None);
        }
    };
    let digests = configured
        .iter()
        .map(|server| {
            (
                server.name.clone(),
                configuration_digest(&server.command, &server.args),
            )
        })
        .collect();
    agents.mcp_servers = stand_ins;
    Ok(Some(McpComposition {
        relay: Arc::new(Relay::new(servers.clone(), digests)),
        servers,
        listener,
    }))
}

#[cfg(test)]
#[path = "../../tests/composition/mcp_servers.rs"]
mod tests;
