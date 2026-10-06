//! The gateway's connection to each configured MCP server for each harness
//! session (ADR 344), composed before any agent is built.
//!
//! ```text
//! AgentsConfig.mcpServers ──LaunchSettings::launch_set──▶ McpServers: the live set (each started by the gateway)
//!                                 │ configured(), read at each provider open and each hello
//!                                 ├──▶ StandIns: <this executable> mcp-relay <socket> <name> <keyed digest>
//!                                 │             ──▶ every agent's session/new (AcpConfig.mcp_servers)
//!                                 ├──▶ Relay: admits a stand-in against the digests now
//!                                 ├◀── McpServerSettings::edit replaces it after each publish (`settings`)
//!                                 └──▶ McpServerInspector: open_once for mcpServers.inspect (`settings`)
//! ```
//!
//! Arrows are what each is built from. The digest is keyed with a secret
//! minted for this process ([`ConfigurationKey`]), which the stand-ins and
//! the relay share, and the stored servers' revision with it. On Unix the relay is composed even
//! with no server configured, so a set replaced later reaches the next open.
//! A relay socket that cannot be bound leaves MCP servers off for this run,
//! logged: an agent is never handed a server directly instead, because then
//! its calls and an app's would reach different sessions.
use super::agent::{agent_search_path, AgentsConfig};
use super::runtime_config::{RuntimeConfig, MAX_CONFIG_BYTES};
use crate::conversation::application::{DroppedContexts, McpAppAudit};
use crate::core::RunError;
use crate::mcp_servers::{
    application::{McpServerSettings, Unfinished},
    domain::{relay_arguments, ConfigurationKey},
    infrastructure::{
        bind, launch_digest, BoundRelay, ConfigCheck, ConfigFiles, ConfigJsonStore,
        ConversationGrants, DurableMcpServerAudit, LaunchSettings, LiveMcpServers,
        McpServerInspector, OsConfigFiles, OsTokens, Relay, ResourceTicketStore, TicketEvent,
        TokenSource,
    },
};
use crate::product::mcp_servers::list_fits;
use nessa_sdk::infrastructure::{
    acp::sessions::{McpServerList, McpServerSource, StandInSessions, StdioMcpServer},
    clock::RuntimeClock,
    mcp::McpServers,
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
    /// How a stored server is launched, and the managed one: what each
    /// change's live set is built with ([`settings`]).
    pub(super) launches: LaunchSettings,
    pub(super) relay: Arc<Relay>,
    pub(super) listener: BoundRelay,
    /// This process's key for configuration digests: the stand-ins' and the
    /// relay's, and the stored servers' revision ([`settings`]).
    pub(super) key: ConfigurationKey,
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
/// derived from the namespace alone, so two gateways of one namespace meet at
/// one socket and the second cannot take it over (`relay::bind`).
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
/// name, and the digest of its command, arguments and environment keyed with
/// `key` ([`launch_digest`]), which the relay compares at each hello.
pub(super) fn stand_ins(
    servers: &McpServers,
    gateway: &str,
    socket: &str,
    key: &ConfigurationKey,
) -> Vec<StdioMcpServer> {
    let mut stand_ins: Vec<StdioMcpServer> = servers
        .configured()
        .iter()
        .map(|launch| StdioMcpServer {
            name: launch.server.name.clone(),
            command: gateway.into(),
            args: relay_arguments(socket, &launch.server.name, &launch_digest(key, launch)),
        })
        .collect();
    for remote in servers.configured_remotes() {
        stand_ins.push(StdioMcpServer {
            name: remote.name().to_owned(),
            command: gateway.into(),
            args: relay_arguments(
                socket,
                remote.name(),
                &crate::mcp_servers::domain::remote_configuration_digest(
                    key,
                    &remote.id().to_string(),
                    remote.url().as_str(),
                ),
            ),
        });
    }
    stand_ins
}

/// The stand-ins for the live set, which every provider open reads
/// ([`McpServerSource`]): an open gets the stand-ins for the servers
/// configured then, and keeps them for its provider session's life.
pub(super) struct StandIns {
    servers: McpServers,
    gateway: String,
    socket: String,
    key: ConfigurationKey,
}
impl StandIns {
    /// Stand-ins for `servers`, run by `gateway` over `socket`, their digests
    /// keyed with `key`; `None` when the gateway's or the socket's path is
    /// not UTF-8: a stand-in's command and arguments must be
    /// ([`StdioMcpServer::problem`]).
    pub(super) fn new(
        servers: McpServers,
        gateway: &Path,
        socket: &Path,
        key: ConfigurationKey,
    ) -> Option<Self> {
        Some(Self {
            servers,
            gateway: gateway.to_str()?.to_owned(),
            socket: socket.to_str()?.to_owned(),
            key,
        })
    }
}
impl McpServerSource for StandIns {
    fn servers(&self) -> Vec<StdioMcpServer> {
        stand_ins(&self.servers, &self.gateway, &self.socket, &self.key)
    }
}

/// A new key for this process's configuration digests, drawn from `tokens`;
/// `None` when the random source fails.
pub(super) fn configuration_key(tokens: &dyn TokenSource) -> Option<ConfigurationKey> {
    let mut bytes = [0; 32];
    tokens.fill(&mut bytes).ok()?;
    Some(ConfigurationKey::new(bytes))
}

/// Take over `agents`' MCP servers: start nothing yet, bind the relay socket
/// at `socket` ([`relay_socket`]), give each provider open the stand-ins, run
/// by `gateway`, for the servers configured then, and give it a grant whose
/// token its stand-ins carry. Composed with no server configured too, so a
/// set replaced later ([`McpServers::replace`]) reaches the next open.
/// `None` when the socket cannot be bound, a path is not UTF-8, or no key
/// for the digests could be drawn — then `agents` is left with no MCP
/// servers, and why is logged. `bundled` is whether this is the desktop
/// gateway, whose managed server `desktop::configure` put in `agents` in
/// place of any stored under its name ([`LaunchSettings::new`]).
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
    bundled: bool,
) -> Result<Option<McpComposition>, RunError> {
    let configured = std::mem::take(&mut agents.mcp_servers);
    let launches = LaunchSettings::new(&configured, bundled, agents.workspace.clone(), environment);
    let (launch_set, remotes) = launches
        .launch_set(&configured)
        .map_err(|problem| RunError::Agent(problem.to_string()))?;
    let servers = McpServers::new(Vec::new(), Arc::new(RuntimeClock::new()))
        .map_err(|error| RunError::Agent(error.to_string()))?;
    servers
        .replace_all(launch_set, remotes)
        .map_err(|error| RunError::Agent(error.to_string()))?;
    if let Some(http) = crate::mcp_servers::infrastructure::ReqwestExchange::new() {
        servers.set_remote_transport(
            Arc::new(http),
            Arc::new(nessa_sdk::infrastructure::mcp::NoAuthorization),
        );
    } else {
        tracing::error!("remote MCP is unreachable this run: the HTTP client could not be built");
    }
    let Some(key) = configuration_key(&OsTokens) else {
        tracing::error!(
            "MCP servers are off this run: no key for their configuration digests could be drawn"
        );
        return Ok(None);
    };
    let Some(stand_ins) = StandIns::new(servers.clone(), gateway, socket, key.clone()) else {
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
        relay: Arc::new(Relay::new(servers.clone(), grants, key.clone())),
        key,
        servers,
        launches,
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

/// What manages the stored servers of the namespace whose `config.json` is
/// at `config` (`mcpServers.list`, `.save`, `.remove`, `.inspect`): the file
/// and its lock, checked by the runtime configuration's own parse and bound;
/// the audit in `audit` (`<namespace>/conversations/audit/mcp-servers`);
/// `mcp`'s live set, replaced after each publish; and an inspector on the
/// same SDK client — [`stop`] stops and drains the inspections before it
/// stops that client, and a stop past the drain's bound still ends one under
/// way. A
/// file with no `agents` block gains one from `agents`' catalog and
/// workspace on its first write. Every write re-serialises the whole file
/// (`ConfigJsonStore`). `None`, logged, when the catalog or workspace path
/// is not UTF-8: that block could not be written as it is, so the gateway
/// manages no stored servers this run and the methods answer
/// `mcp_servers_not_configured`
/// (`a_fallback_path_that_is_not_utf8_composes_no_settings`).
///
/// # Errors
///
/// [`RunError::Agent`] when the audit's directory cannot be created.
pub(super) fn settings(
    mcp: &McpComposition,
    agents: &AgentsConfig,
    config: PathBuf,
    audit: PathBuf,
) -> Result<Option<McpServerSettings>, RunError> {
    settings_over(mcp, agents, Arc::new(OsConfigFiles::new(config)), audit)
}

/// [`settings`] over `files`: the real file and its lock, or — in a test —
/// something wrapped around them.
pub(super) fn settings_over(
    mcp: &McpComposition,
    agents: &AgentsConfig,
    files: Arc<dyn ConfigFiles>,
    audit: PathBuf,
) -> Result<Option<McpServerSettings>, RunError> {
    let Some(fallback) = fallback_agents(agents) else {
        tracing::error!(
            "MCP server settings are off this run: the catalog or workspace path is not UTF-8, \
             so a first write could not store it as it is"
        );
        return Ok(None);
    };
    let store = ConfigJsonStore::new(
        files,
        ConfigCheck {
            limit: MAX_CONFIG_BYTES,
            parses: Box::new(|bytes| RuntimeConfig::parse(bytes).is_ok()),
        },
        fallback,
        Arc::new(RuntimeClock::new()),
        mcp.key.clone(),
    );
    let audit = DurableMcpServerAudit::new(audit, Arc::new(super::local_auth::SystemClock))
        .map_err(|_| RunError::Agent("the MCP server audit could not be opened".into()))?;
    Ok(Some(McpServerSettings::new(
        Arc::new(store),
        Arc::new(audit),
        Arc::new(LiveMcpServers::new(
            mcp.servers.clone(),
            mcp.launches.clone(),
        )),
        Arc::new(McpServerInspector::new(
            mcp.servers.clone(),
            mcp.launches.clone(),
            Arc::new(RuntimeClock::new()),
        )),
        list_fits,
    )))
}

/// The `agents` block a first write starts from: the running catalog and
/// workspace, or `None` when either is not UTF-8 — written lossily, the file
/// would name another path, which the next start would use
/// (`a_fallback_path_that_is_not_utf8_composes_no_settings`).
fn fallback_agents(agents: &AgentsConfig) -> Option<serde_json::Map<String, serde_json::Value>> {
    Some(serde_json::Map::from_iter([
        ("catalog".to_owned(), agents.catalog.to_str()?.into()),
        ("workspace".to_owned(), agents.workspace.to_str()?.into()),
    ]))
}

/// The gateway's MCP stop, in its one order: `settings` — when this gateway
/// manages its stored servers — admits no more changes or inspections and
/// stops the inspections under way (done already as cleanup began,
/// `ProductRouteState::close_mcp_server_admission`), and drains every
/// admitted one to its outcome record ([`McpServerSettings::shutdown`]);
/// then `servers` stop, whatever
/// the drain answered. Called once, by the gateway's cleanup
/// (`root::cleanup_product`), after conversations
/// (`the_mcp_stop_drains_admitted_writes_before_the_servers_stop`).
///
/// # Errors
///
/// [`Unfinished`] when the drain ran out of time: the servers are stopped
/// all the same, and the shutdown report is not confirmed
/// (`an_unfinished_drain_is_an_unconfirmed_shutdown`).
pub(super) async fn stop(
    settings: Option<&McpServerSettings>,
    servers: &McpServers,
) -> Result<(), Unfinished> {
    let drained = match settings {
        Some(settings) => settings.shutdown().await,
        None => Ok(()),
    };
    servers.stop().await;
    drained
}

#[cfg(all(test, unix))]
#[path = "../../tests/composition/mcp_servers.rs"]
mod tests;
