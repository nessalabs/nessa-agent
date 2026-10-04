//! The gateway's connection to each configured MCP server for each harness
//! session (ADR 344), composed before any agent is built.
//!
//! ```text
//! AgentsConfig.mcpServers ──▶ McpServers: the live set (each started by the gateway)
//!                                 │ configured(), read at each provider open and each hello
//!                                 ├──▶ StandIns: <this executable> mcp-relay <socket> <name> <digest>
//!                                 │             ──▶ every agent's session/new (AcpConfig.mcp_servers)
//!                                 └──▶ Relay: admits a stand-in against the digests now
//! ```
//!
//! Arrows are what each is built from. On Unix the relay is composed even
//! with no server configured, so a set replaced later reaches the next open.
//! A relay socket that cannot be bound leaves MCP servers off for this run,
//! logged: an agent is never handed a server directly instead, because then
//! its calls and an app's would reach different sessions.
use super::agent::{agent_search_path, AgentsConfig};
use crate::core::RunError;
use crate::mcp_servers::{
    domain::{configuration_digest, relay_arguments},
    infrastructure::{
        bind, BoundRelay, ConversationGrants, OsTokens, Relay, ResourceTicketStore, TicketEvent,
    },
};
use nessa_sdk::infrastructure::{
    acp::sessions::{McpServerList, McpServerSource, StandInSessions, StdioMcpServer},
    clock::RuntimeClock,
    mcp::{McpServerLaunch, McpServers},
};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

/// How often the resource ticket store looks for tickets past their
/// deadline between the calls it answers: an unredeemed ticket's bytes are
/// let go of, and its expiry reported, at most this long after it.
pub(super) const RESOURCE_TICKET_SWEEP: Duration = Duration::from_secs(5);

/// What composition hands the server lifecycle: the servers to start once it
/// is listening and stop once conversations have, and the relay to serve.
pub(super) struct McpComposition {
    pub(super) servers: McpServers,
    pub(super) relay: Arc<Relay>,
    pub(super) listener: BoundRelay,
    /// The MCP App resources held behind tickets: the conversation service
    /// issues and releases on it, `GET /mcp-resources` redeems on it
    /// (`ProductRouteState::with_resource_tickets`), and the server lifecycle
    /// sweeps it (`ResourceTicketStore::sweep_periodically`). One instance,
    /// shared by `Arc`.
    pub(super) resource_tickets: Arc<ResourceTicketStore>,
    /// Each ticket's unredeemed end — expired, released, or dropped — as the
    /// store reports it. Composition takes it once, for `audit_ticket_ends`,
    /// beside the conversation service that audits an app's calls.
    pub(super) ticket_events: Option<UnboundedReceiver<TicketEvent>>,
    /// The task recording those ends, once started: the server lifecycle
    /// stops it last, after the conversations whose ends it records.
    pub(super) ticket_recorder: Option<TicketRecorder>,
}

/// The task recording each ticket's unredeemed end.
pub(super) struct TicketRecorder {
    pub(super) stop: tokio::sync::oneshot::Sender<()>,
    pub(super) task: tokio::task::JoinHandle<()>,
}
impl TicketRecorder {
    /// Record every end already reported, then stop: called once the
    /// conversations, and their apps, have ended.
    pub(super) async fn finish(self) {
        let _ = self.stop.send(());
        // Bounded, as the app calls' own records are: a record that hangs
        // must not hold the gateway's exit.
        match tokio::time::timeout(std::time::Duration::from_secs(10), self.task).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => tracing::error!(%error, "the MCP App ticket recorder failed"),
            Err(_) => tracing::warn!(
                "MCP App ticket ends were still being recorded when the gateway stopped"
            ),
        }
    }
}

/// Where the relay socket of the namespace at `namespace` is, for the user
/// `uid`: `/tmp/nessa-mcp-<uid>/<16 hex of the namespace's digest>.sock`.
///
/// Short, because a socket's path has a platform limit (104 bytes on macOS)
/// that a namespace under a long data directory passes; in a directory of the
/// user's own, created private and refused when it is not (`relay::bind`); and
/// the same on every run of one namespace, so a stand-in's arguments are too.
/// Those arguments are not part of any restoration identity (ADR 344).
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

/// The stand-in for each server in `servers`, run by `gateway` over
/// `socket`: the gateway's executable, `mcp-relay`, the socket, the server's
/// name, and the digest of its command and arguments, which the relay
/// compares at each hello.
pub(super) fn stand_ins(
    servers: &[StdioMcpServer],
    gateway: &str,
    socket: &str,
) -> Vec<StdioMcpServer> {
    servers
        .iter()
        .map(|server| StdioMcpServer {
            name: server.name.clone(),
            command: gateway.into(),
            args: relay_arguments(
                socket,
                &server.name,
                &configuration_digest(&server.command, &server.args),
            ),
        })
        .collect()
}

/// The stand-ins for the live set, which every provider open reads
/// ([`McpServerSource`]): an open gets the stand-ins for the servers
/// configured then, and keeps them for its provider session's life.
pub(super) struct StandIns {
    servers: McpServers,
    gateway: String,
    socket: String,
}
impl StandIns {
    /// Stand-ins for `servers`, run by `gateway` over `socket`; `None` when
    /// the gateway's or the socket's path is not UTF-8: a stand-in's command
    /// and arguments must be ([`StdioMcpServer::problem`]).
    pub(super) fn new(servers: McpServers, gateway: &Path, socket: &Path) -> Option<Self> {
        Some(Self {
            servers,
            gateway: gateway.to_str()?.to_owned(),
            socket: socket.to_str()?.to_owned(),
        })
    }
}
impl McpServerSource for StandIns {
    fn servers(&self) -> Vec<StdioMcpServer> {
        stand_ins(&self.servers.configured(), &self.gateway, &self.socket)
    }
}

/// Take over `agents`' MCP servers: start nothing yet, bind the relay socket
/// at `socket` ([`relay_socket`]), give each provider open the stand-ins, run
/// by `gateway`, for the servers configured then, and give it a grant whose
/// token its stand-ins carry. Composed with no server configured too, so a
/// set replaced later ([`McpServers::replace`]) reaches the next open.
/// `None` when the socket cannot be bound, or a path is not UTF-8 — then
/// `agents` is left with no MCP servers, and why is logged.
///
/// # Errors
///
/// [`RunError::Agent`] when the configured servers could not all be launched
/// as configured.
pub(super) async fn compose(
    agents: &mut AgentsConfig,
    socket: &Path,
    gateway: &Path,
    environment: BTreeMap<OsString, OsString>,
) -> Result<Option<McpComposition>, RunError> {
    let configured = std::mem::take(&mut agents.mcp_servers);
    let launches = configured
        .into_iter()
        .map(|server| McpServerLaunch {
            server,
            working_directory: agents.workspace.clone(),
            environment: environment.clone(),
        })
        .collect();
    let servers = McpServers::new(launches, Arc::new(RuntimeClock::new()))
        .map_err(|error| RunError::Agent(error.to_string()))?;
    let Some(stand_ins) = StandIns::new(servers.clone(), gateway, socket) else {
        tracing::error!(socket = %socket.display(), gateway = %gateway.display(), "MCP servers are off this run: the gateway's or the relay socket's path is not UTF-8");
        return Ok(None);
    };
    let listener = match bind(socket).await {
        Ok(listener) => listener,
        Err(error) => {
            tracing::error!(socket = %socket.display(), %error, "MCP servers are off this run: the relay socket could not be bound");
            return Ok(None);
        }
    };
    let grants = ConversationGrants::new(servers.clone(), Arc::new(OsTokens));
    agents.mcp_stand_ins = McpServerList::read_from(Arc::new(stand_ins));
    agents.stand_ins = StandInSessions::granted_by(Arc::new(grants.clone()));
    let (ticket_ends, ticket_events) = unbounded_channel();
    Ok(Some(McpComposition {
        relay: Arc::new(Relay::new(servers.clone(), grants)),
        servers,
        listener,
        resource_tickets: Arc::new(ResourceTicketStore::new(
            Arc::new(super::local_auth::SystemClock),
            Arc::new(OsTokens),
            Arc::new(ticket_ends),
        )),
        ticket_events: Some(ticket_events),
        ticket_recorder: None,
    }))
}

#[cfg(all(test, unix))]
#[path = "../../tests/composition/mcp_servers.rs"]
mod tests;
