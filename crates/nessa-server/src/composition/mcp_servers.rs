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
use crate::conversation::application::{DroppedContexts, McpAppAudit};
use crate::core::RunError;
use crate::mcp_servers::{
    domain::{configuration_digest, relay_arguments},
    infrastructure::{
        bind, BoundRelay, ConversationGrants, OsTokens, Relay, ResourceTicketStore, TicketEvent,
    },
};
use nessa_sdk::infrastructure::{
    acp::sessions::{StandInSessions, StdioMcpServer},
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
/// How long the gateway's exit waits for its MCP App recorders, together,
/// to record what was already reported: bounded, as the app calls' own
/// records are, so a record that hangs does not hold the exit.
pub(super) const RECORDERS_FINISH: Duration = Duration::from_secs(10);

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
    pub(super) ticket_recorder: Option<AuditRecorder>,
    /// The task recording each MCP App context the conversations' apps drop
    /// unsent (`audit_context_drops`), once started: stopped beside the
    /// ticket recorder, after the conversations whose drops it records.
    pub(super) context_drop_recorder: Option<AuditRecorder>,
}

impl McpComposition {
    /// Start the recorders of what outlives an app's call, each writing to
    /// `audit`: every ticket's unredeemed end (`audit_ticket_ends`, on the
    /// store's events, taken once), and every held context's drop
    /// (`audit_context_drops`). The sink the conversations' apps report
    /// each drop to is returned, for the conversation service; both are
    /// finished by [`finish_recorders`] after the conversations.
    pub(super) fn start_recorders(
        &mut self,
        audit: Arc<dyn McpAppAudit>,
    ) -> Arc<dyn DroppedContexts> {
        if let Some(events) = self.ticket_events.take() {
            let (stop, stopping) = tokio::sync::oneshot::channel();
            self.ticket_recorder = Some(AuditRecorder {
                what: "ticket ends",
                stop,
                task: tokio::spawn(crate::mcp_servers::infrastructure::audit_ticket_ends(
                    events,
                    audit.clone(),
                    stopping,
                )),
            });
        }
        let (dropped, drops) = unbounded_channel();
        let (stop, stopping) = tokio::sync::oneshot::channel();
        self.context_drop_recorder = Some(AuditRecorder {
            what: "context drops",
            stop,
            task: tokio::spawn(crate::conversation::infrastructure::audit_context_drops(
                drops, audit, stopping,
            )),
        });
        Arc::new(dropped)
    }
}

/// A task recording what the conversations report after the command that
/// caused it — a ticket's unredeemed end, or a held context's drop — named
/// by `what` in its logs.
pub(super) struct AuditRecorder {
    pub(super) what: &'static str,
    pub(super) stop: tokio::sync::oneshot::Sender<()>,
    pub(super) task: tokio::task::JoinHandle<()>,
}

/// Have every recorder in `recorders` record everything already reported,
/// then stop — all at once, under the one `bound` — called once the
/// conversations, and their apps, have ended. A recorder still recording at
/// the bound is let go of, and said so
/// (`recorders_finish_together_under_one_bound`).
pub(super) async fn finish_recorders(
    recorders: impl IntoIterator<Item = AuditRecorder>,
    bound: Duration,
) {
    let deadline = tokio::time::Instant::now() + bound;
    futures_util::future::join_all(recorders.into_iter().map(|recorder| async move {
        let what = recorder.what;
        let _ = recorder.stop.send(());
        match tokio::time::timeout_at(deadline, recorder.task).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => tracing::error!(%error, what, "an MCP App recorder failed"),
            Err(_) => tracing::warn!(
                what,
                "an MCP App recorder was still recording when the gateway stopped"
            ),
        }
    }))
    .await;
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
/// `None` when the gateway's or the socket's path is not UTF-8: a stand-in's
/// command and arguments must be (`StdioMcpServer::all_valid`).
pub(super) fn stand_ins(
    servers: &[StdioMcpServer],
    gateway: &Path,
    socket: &Path,
) -> Option<Vec<StdioMcpServer>> {
    gateway.to_str()?;
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
/// at `socket` ([`relay_socket`]), replace each server with its stand-in run by `gateway`,
/// and give each provider open a grant whose token its stand-ins carry.
/// `None` when no server is configured, or when the socket cannot be bound —
/// then `agents` is left with no MCP servers, and why is logged.
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
    let digests = configured
        .iter()
        .map(|server| {
            (
                server.name.clone(),
                configuration_digest(&server.command, &server.args),
            )
        })
        .collect();
    let grants = ConversationGrants::new(servers.clone(), Arc::new(OsTokens));
    agents.mcp_servers = stand_ins;
    agents.stand_ins = StandInSessions::granted_by(Arc::new(grants.clone()));
    let (ticket_ends, ticket_events) = unbounded_channel();
    Ok(Some(McpComposition {
        relay: Arc::new(Relay::new(servers.clone(), digests, grants)),
        servers,
        listener,
        resource_tickets: Arc::new(ResourceTicketStore::new(
            Arc::new(super::local_auth::SystemClock),
            Arc::new(OsTokens),
            Arc::new(ticket_ends),
        )),
        ticket_events: Some(ticket_events),
        ticket_recorder: None,
        context_drop_recorder: None,
    }))
}

#[cfg(all(test, unix))]
#[path = "../../tests/composition/mcp_servers.rs"]
mod tests;
