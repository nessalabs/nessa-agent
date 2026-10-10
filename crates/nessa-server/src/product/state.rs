use super::operational_limits::OperationalLimits;
use super::{change_watch::WatchOwners, WatchTaskFault};
use crate::agent_install::application::AgentInstallations;
use crate::agents::application::{AgentProbe, SharedAgentReadiness};
use crate::attachments::{application::AttachmentService, entrypoint::http::UploadRoute};
use crate::conversation::application::{
    CatalogueReadSource, ConversationRepository, ConversationService, McpAppAudit, ReadGrants,
    ReceiverAuthority, RecordReadSource, WatchCatalogue, WatchNamespaces, WatchRecords,
};
use crate::conversation::infrastructure::UuidWatchNamespaces;
pub(crate) use crate::core::limit_log::note_limit;
use crate::device_pairing::infrastructure::PairingOwnerCommands;
use crate::mcp_servers::{
    application::McpServerSettings, entrypoint::http::ResourceRoute,
    infrastructure::ResourceTicketStore,
};
use axum::extract::FromRef;
use nessa_auth::{
    application::{
        credential_admin::CredentialAdmin,
        ports::{AccessReader, Clock, CredentialVerifier, PolicyEvaluator},
    },
    domain::{AudienceId, OrganizationId, Resource, ResourceId},
};
use nessa_protocol::clock::Clock as UptimeClock;
use nessa_protocol::product::generated::{
    AgentsListResult, MAX_GLOBAL_CHANGE_WATCHES, MAX_PRINCIPAL_CHANGE_WATCHES,
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;

/// The receiver bindings, ownership and read grants a paired device's reads
/// are admitted by.
pub(crate) type PassiveReadAuthorities = (
    Arc<dyn ReceiverAuthority>,
    Arc<dyn ConversationRepository>,
    Arc<dyn ReadGrants>,
);

/// Dependencies and trusted gateway selectors for the product route.
///
/// This state is constructed only in composition.
#[derive(Clone)]
pub struct ProductRouteState {
    pub(crate) browser_sessions: Option<Arc<dyn crate::browser_session::application::SessionStore>>,
    pub(crate) browser_http_allowed: bool,
    pub(crate) browser_session_id: Option<String>,
    pub(crate) browser_session_origin: Option<String>,
    pub(crate) requests: Arc<Semaphore>,
    pub(crate) controls: Arc<Semaphore>,
    /// Physical record reads or pending record replies, kept separate from
    /// command and control admission. How many is [`OperationalLimits`].
    pub(crate) record_reads: Arc<Semaphore>,
    pub(super) change_watches: Arc<WatchOwners>,
    pub(crate) record_watches: Option<Arc<dyn WatchRecords>>,
    pub(crate) catalogue_watches: Option<Arc<dyn WatchCatalogue>>,
    pub(crate) watch_namespaces: Arc<dyn WatchNamespaces>,
    /// Beginning an upload has capacity of its own. It opens no provider, so it
    /// does not belong behind reads and opens; and it sweeps tickets, reads
    /// holds, and writes audit records, so it must never be what keeps a
    /// permission answer or a close from being admitted.
    pub(crate) upload_begins: Arc<Semaphore>,
    /// Deleting a conversation has capacity of its own, apart from the
    /// controls. A delete can take minutes — it may launch the agent to ask it
    /// about its own record of the session — and a burst of them must never be
    /// what keeps a permission answer, a cancel, or a close from being
    /// admitted (`deletes_and_controls_never_take_each_others_place`). How
    /// many agents are asked at once is bounded again, and lower, by the
    /// conversation service.
    pub(crate) deletions: Arc<Semaphore>,
    pub(crate) settings: SessionSettings,
    /// Admission counts and the cold-read budget. The semaphores above are
    /// built from it; a socket reads its slot sizes from it too.
    pub(crate) limits: OperationalLimits,
    pub(crate) gateway: Resource,
    pub(crate) audience: AudienceId,
    pub(crate) verifier: Arc<dyn CredentialVerifier>,
    pub(crate) access: Arc<dyn AccessReader>,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) policy: Arc<dyn PolicyEvaluator>,
    pub(crate) conversations: Option<Arc<ConversationService>>,
    /// Passive reads use these independent authorities without opening an Agent.
    /// The read grants are also what every socket read asks for a paired
    /// device (`read_access`).
    pub(crate) passive_read: Option<PassiveReadAuthorities>,
    pub(crate) catalogue_source: Option<Arc<dyn CatalogueReadSource>>,
    pub(crate) record_source: Option<Arc<dyn RecordReadSource>>,
    pub(crate) agent_installations: Option<Arc<dyn AgentInstallations>>,
    pub(crate) installs: Arc<Semaphore>,
    pub(crate) agents_catalog: Option<Arc<AgentsListResult>>,
    pub(crate) attachments: Option<AttachmentService>,
    /// The MCP App resources held behind tickets: issued by the conversation
    /// service, redeemed at `GET /mcp-resources`, which records each
    /// redemption in the audit beside it before serving. `None` when no MCP server is
    /// composed, and then that route answers every ticket `404`.
    pub(crate) resource_tickets: Option<(Arc<ResourceTicketStore>, Arc<dyn McpAppAudit>)>,
    pub(crate) admin: Option<Arc<dyn CredentialAdmin>>,
    /// What `mcpServers.list`, `.save`, `.remove` and `.inspect` manage;
    /// `None` where this gateway holds no live MCP server set — not Unix, no
    /// agents configured, or MCP off this run because its relay socket could
    /// not be bound, a path was not UTF-8 or no key for its digests could be
    /// drawn — which they answer `mcp_servers_not_configured`.
    pub(crate) mcp_server_settings: Option<Arc<McpServerSettings>>,
    /// Remote MCP authorization. `None` when this gateway is not managing
    /// stored servers. `mcpServers.authorize` and `mcpServers.revoke` use it.
    pub(crate) mcp_authorization:
        Option<Arc<crate::mcp_authorization::application::AuthorizationOwner>>,
    /// Owner pairing commands, composed only when `config.json` names a native
    /// listen address. `None` answers every pairing method
    /// `pairing_not_configured`.
    pub(crate) pairing: Option<Arc<PairingOwnerCommands>>,
    pub(crate) uptime_clock: Arc<dyn UptimeClock>,
    pub(crate) agent_readiness: Arc<SharedAgentReadiness>,
}

/// The pre-authentication onboarding route is given one way to ask this machine
/// about its agents, and nothing else. It runs before there is a session, so it
/// has no use for the rest of this state and must not be able to reach it.
///
/// It is handed the shared reader rather than the probe itself because the route
/// is unauthenticated: whoever is calling chooses how many requests arrive, and
/// the reader is what keeps that from choosing how many probes this server runs.
impl FromRef<ProductRouteState> for Arc<SharedAgentReadiness> {
    fn from_ref(state: &ProductRouteState) -> Self {
        state.agent_readiness.clone()
    }
}

/// The upload route is given the attachment service and nothing else. It
/// authenticates nobody, so it must not be able to reach a verifier, a policy,
/// or a conversation: a ticket the socket issued is all it acts on.
impl FromRef<ProductRouteState> for UploadRoute {
    fn from_ref(state: &ProductRouteState) -> Self {
        UploadRoute::new(state.attachments.clone())
    }
}

/// The resource route is given the ticket store and nothing else, for the
/// same reason as the upload route: a ticket is all it acts on.
impl FromRef<ProductRouteState> for ResourceRoute {
    fn from_ref(state: &ProductRouteState) -> Self {
        ResourceRoute::new(state.resource_tickets.clone())
    }
}

/// Typed dependencies selected once by the server composition root.
pub struct ProductDependencies {
    /// Verifies opaque proof and resolves a Nessa credential binding.
    pub verifier: Arc<dyn CredentialVerifier>,
    /// Reads a coherent current credential and membership snapshot.
    pub access: Arc<dyn AccessReader>,
    /// Absolute time for expiry checks; never replaced by the uptime fixture.
    pub clock: Arc<dyn Clock>,
    /// Embedded policy engine constructed and validated once at startup.
    pub policy: Arc<dyn PolicyEvaluator>,
    /// Existing server clock used only to report health uptime.
    pub uptime_clock: Arc<dyn UptimeClock>,
    /// Asks this host which agents could start here. Chosen in composition so
    /// no route handler constructs a machine probe of its own. How often it may
    /// be asked is this state's to decide, not composition's — see
    /// [`SharedAgentReadiness`].
    pub agent_probe: Arc<dyn AgentProbe>,
}

impl ProductRouteState {
    /// Close only watch admission; original admitted authority tasks keep their permits.
    /// Normal host cleanup consumes this before polling reader or watch drain.
    pub(crate) fn close_watch_admission(&self) {
        self.change_watches.close();
    }

    /// Admit no more `mcpServers.save`, `.remove` or `.inspect` — each later
    /// one answers `mcp_servers_stopping` — and stop the inspections under
    /// way now, as cleanup begins rather than after the conversations drain
    /// ([`McpServerSettings::close`]). The changes admitted run on until the
    /// MCP stop drains them before the servers stop
    /// ([`McpServerSettings::shutdown`]).
    pub(crate) fn close_mcp_server_admission(&self) {
        if let Some(settings) = &self.mcp_server_settings {
            settings.close();
        }
    }

    /// Join the distinct actual watch resource after closing admission/connection interest.
    /// The normal host report retains its returned first fault alongside reader outcomes.
    pub(crate) async fn drain_watches(&self) -> Result<(), WatchTaskFault> {
        self.close_watch_admission();
        self.change_watches.drain().await
    }

    /// Bind trusted gateway ownership and audience to an isolated dependency scope.
    pub fn new(
        gateway_id: ResourceId,
        gateway_organization_id: OrganizationId,
        audience: AudienceId,
        dependencies: ProductDependencies,
    ) -> Self {
        let limits = OperationalLimits::default();
        Self {
            browser_sessions: None,
            browser_session_id: None,
            browser_session_origin: None,
            browser_http_allowed: false,
            settings: SessionSettings::default(),
            limits,
            requests: Arc::new(Semaphore::new(limits.requests())),
            controls: Arc::new(Semaphore::new(limits.controls())),
            record_reads: Arc::new(Semaphore::new(limits.record_reads())),
            change_watches: Arc::new(WatchOwners::new(
                MAX_GLOBAL_CHANGE_WATCHES,
                MAX_PRINCIPAL_CHANGE_WATCHES,
            )),
            record_watches: None,
            catalogue_watches: None,
            watch_namespaces: Arc::new(UuidWatchNamespaces),
            upload_begins: Arc::new(Semaphore::new(limits.upload_begins())),
            deletions: Arc::new(Semaphore::new(limits.deletions())),
            gateway: Resource::new(gateway_organization_id, gateway_id),
            audience,
            verifier: dependencies.verifier,
            access: dependencies.access,
            clock: dependencies.clock,
            policy: dependencies.policy,
            admin: None,
            mcp_server_settings: None,
            mcp_authorization: None,
            pairing: None,
            conversations: None,
            passive_read: None,
            record_source: None,
            catalogue_source: None,
            agents_catalog: None,
            agent_installations: None,
            installs: Arc::new(Semaphore::new(1)),
            attachments: None,
            resource_tickets: None,
            uptime_clock: dependencies.uptime_clock,
            agent_readiness: Arc::new(SharedAgentReadiness::new(dependencies.agent_probe)),
        }
    }

    pub fn with_browser_sessions(
        mut self,
        store: Arc<dyn crate::browser_session::application::SessionStore>,
    ) -> Self {
        self.browser_sessions = Some(store);
        self
    }

    pub fn with_settings(mut self, settings: SessionSettings) -> Self {
        self.settings = settings;
        self
    }

    /// Replace the gateway admission semaphores and the limits a socket reads.
    /// Called once, from composition, before anything is served.
    pub fn with_limits(mut self, limits: OperationalLimits) -> Self {
        self.requests = Arc::new(Semaphore::new(limits.requests()));
        self.controls = Arc::new(Semaphore::new(limits.controls()));
        self.record_reads = Arc::new(Semaphore::new(limits.record_reads()));
        self.upload_begins = Arc::new(Semaphore::new(limits.upload_begins()));
        self.deletions = Arc::new(Semaphore::new(limits.deletions()));
        self.limits = limits;
        self
    }

    /// Register credential lifecycle operations; missing administration fails closed.
    pub fn with_admin(mut self, admin: Arc<dyn CredentialAdmin>) -> Self {
        self.admin = Some(admin);
        self
    }

    /// Register what manages this gateway's stored MCP servers. Without it
    /// the `mcpServers.*` methods answer `mcp_servers_not_configured`.
    pub fn with_mcp_server_settings(mut self, settings: Arc<McpServerSettings>) -> Self {
        self.mcp_server_settings = Some(settings);
        self
    }

    /// Register the remote authorization owner. Without it, authorize and
    /// revoke answer `mcp_servers_not_configured`.
    pub fn with_mcp_authorization(
        mut self,
        authorization: Arc<crate::mcp_authorization::application::AuthorizationOwner>,
    ) -> Self {
        self.mcp_authorization = Some(authorization);
        self
    }

    /// Register the owner pairing commands of this gateway's native enrollment
    /// runtime. Without them the pairing methods answer `pairing_not_configured`.
    pub fn with_pairing(mut self, pairing: Arc<PairingOwnerCommands>) -> Self {
        self.pairing = Some(pairing);
        self
    }

    /// Share server-owned Agents across authenticated sockets. No socket owns cleanup.
    pub fn with_conversations(mut self, service: Arc<ConversationService>) -> Self {
        self.conversations = Some(service);
        self
    }

    /// Bind the server-owned receiver state and conversation ownership source.
    pub fn with_passive_read(
        mut self,
        receivers: Arc<dyn ReceiverAuthority>,
        conversations: Arc<dyn ConversationRepository>,
        read_grants: Arc<dyn ReadGrants>,
    ) -> Self {
        self.passive_read = Some((receivers, conversations, read_grants));
        self
    }

    /// Compose the actual local producers alongside independently admitted readers.
    pub fn with_change_watches(
        mut self,
        records: Arc<dyn WatchRecords>,
        catalogue: Arc<dyn WatchCatalogue>,
    ) -> Self {
        self.record_watches = Some(records);
        self.catalogue_watches = Some(catalogue);
        self
    }

    /// Inject connection identity minting for host substitution and deterministic tests.
    pub fn with_watch_namespaces(mut self, namespaces: Arc<dyn WatchNamespaces>) -> Self {
        self.watch_namespaces = namespaces;
        self
    }

    /// Inject current catalogue metadata reads behind passive admission.
    pub fn with_catalogue_source(mut self, source: Arc<dyn CatalogueReadSource>) -> Self {
        self.catalogue_source = Some(source);
        self
    }

    /// Inject the bounded SDK record adapter without opening a conversation Agent.
    pub fn with_record_source(mut self, source: Arc<dyn RecordReadSource>) -> Self {
        self.record_source = Some(source);
        self
    }

    /// Install only composition-selected pinned releases through the authenticated gateway.
    pub fn with_agent_installations(mut self, installations: Arc<dyn AgentInstallations>) -> Self {
        self.agent_installations = Some(installations);
        self
    }

    /// Publish the validated choices composed for this gateway.
    pub fn with_agents_catalog(mut self, catalog: AgentsListResult) -> Self {
        self.agents_catalog = Some(Arc::new(catalog));
        self
    }

    /// Share one attachment service between the socket that issues tickets and
    /// the route that redeems them. Composed only alongside conversations.
    pub fn with_attachments(mut self, service: AttachmentService) -> Self {
        self.attachments = Some(service);
        self
    }

    /// Share one resource ticket store between the conversation service that
    /// issues tickets and the route that redeems them, and the audit the
    /// service records an app's calls in, which the route records each
    /// redemption in. Composed only with MCP servers.
    pub fn with_resource_tickets(
        mut self,
        tickets: Arc<ResourceTicketStore>,
        audit: Arc<dyn McpAppAudit>,
    ) -> Self {
        self.resource_tickets = Some((tickets, audit));
        self
    }

    pub fn gateway_id(&self) -> &ResourceId {
        self.gateway.id()
    }

    pub fn audience(&self) -> &AudienceId {
        &self.audience
    }
}

/// Validated per-route connection deadlines; no process-wide mutable settings.
///
/// Every deadline here becomes a timer on an authenticated socket, and a zero
/// interval is not a fast timer but a panic inside `tokio::time::interval`. The
/// fields are private and [`SessionSettings::new`] is fallible so that a caller
/// embedding this server reaches the same rejection the configuration file does,
/// rather than a panicking socket task after authentication has already
/// succeeded. A test builds deadlines a server must never be given through a
/// `#[cfg(test)]` seam, which is what keeps that out of the running server —
/// the privacy here is what keeps it out of every other caller's reach.
#[derive(Clone, Copy, Debug)]
pub struct SessionSettings {
    handshake_timeout: Duration,
    write_timeout: Duration,
    current_state_interval: Duration,
}

/// Which deadline could not be turned into a timer.
///
/// A variant rather than a field name, so a caller decides on the deadline
/// rather than on the wording of a message.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InvalidSessionSettings {
    /// How long the challenge, authentication, and its reply may take together.
    HandshakeTimeout,
    /// How long one write to a socket may take.
    WriteTimeout,
    /// How often an authenticated session rechecks its own current authority.
    CurrentStateInterval,
}
impl InvalidSessionSettings {
    /// The rejected deadline's name, for a message.
    pub fn field(self) -> &'static str {
        match self {
            Self::HandshakeTimeout => "handshake_timeout",
            Self::WriteTimeout => "write_timeout",
            Self::CurrentStateInterval => "current_state_interval",
        }
    }
}
impl std::fmt::Display for InvalidSessionSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} must be positive and representable as a deadline",
            self.field()
        )
    }
}
impl std::error::Error for InvalidSessionSettings {}

impl SessionSettings {
    /// Validate connection deadlines before any of them can become a timer.
    ///
    /// Each duration must be positive — a zero interval panics inside
    /// `tokio::time::interval` — and must be small enough to add to the current
    /// instant, which is a sanity bound on the configured magnitude rather than a
    /// guarantee about the clock a timer is later built on. Returns the first
    /// field that fails.
    pub fn new(
        handshake_timeout: Duration,
        write_timeout: Duration,
        current_state_interval: Duration,
    ) -> Result<Self, InvalidSessionSettings> {
        for (field, value) in [
            (InvalidSessionSettings::HandshakeTimeout, handshake_timeout),
            (InvalidSessionSettings::WriteTimeout, write_timeout),
            (
                InvalidSessionSettings::CurrentStateInterval,
                current_state_interval,
            ),
        ] {
            if value.is_zero() || Instant::now().checked_add(value).is_none() {
                return Err(field);
            }
        }
        Ok(Self {
            handshake_timeout,
            write_timeout,
            current_state_interval,
        })
    }

    /// How long the challenge, authentication, and its reply may take together.
    pub fn handshake_timeout(&self) -> Duration {
        self.handshake_timeout
    }
    /// How long one write to a socket may take before the session is abandoned.
    pub fn write_timeout(&self) -> Duration {
        self.write_timeout
    }
    /// How often an authenticated session rechecks its own current authority.
    pub fn current_state_interval(&self) -> Duration {
        self.current_state_interval
    }

    /// Deadlines a test needs but a running server must never be given, such as
    /// one that has already elapsed. Not compiled into the server itself.
    #[cfg(test)]
    pub(crate) fn with_deadlines(
        mut self,
        handshake_timeout: Option<Duration>,
        write_timeout: Option<Duration>,
    ) -> Self {
        if let Some(value) = handshake_timeout {
            self.handshake_timeout = value;
        }
        if let Some(value) = write_timeout {
            self.write_timeout = value;
        }
        self
    }
}
impl Default for SessionSettings {
    /// Through [`SessionSettings::new`], so the built-in deadlines answer to the
    /// same rule as every other set and the invariant keeps one owner.
    fn default() -> Self {
        Self::new(
            Duration::from_secs(10),
            Duration::from_secs(5),
            Duration::from_secs(1),
        )
        .expect("the built-in deadlines are positive and representable")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zero_deadline_is_rejected_at_construction_rather_than_at_its_timer() {
        for (field, settings) in [
            (
                InvalidSessionSettings::HandshakeTimeout,
                SessionSettings::new(
                    Duration::ZERO,
                    Duration::from_secs(5),
                    Duration::from_secs(1),
                ),
            ),
            (
                InvalidSessionSettings::WriteTimeout,
                SessionSettings::new(
                    Duration::from_secs(10),
                    Duration::ZERO,
                    Duration::from_secs(1),
                ),
            ),
            (
                InvalidSessionSettings::CurrentStateInterval,
                SessionSettings::new(
                    Duration::from_secs(10),
                    Duration::from_secs(5),
                    Duration::ZERO,
                ),
            ),
        ] {
            assert_eq!(settings.unwrap_err(), field);
        }
        assert_eq!(
            SessionSettings::new(
                Duration::from_secs(10),
                Duration::from_secs(5),
                Duration::MAX
            )
            .unwrap_err(),
            InvalidSessionSettings::CurrentStateInterval
        );
    }

    #[test]
    fn valid_deadlines_are_kept_exactly_and_match_the_defaults() {
        let settings = SessionSettings::new(
            Duration::from_millis(1),
            Duration::from_secs(5),
            Duration::from_millis(250),
        )
        .unwrap();
        assert_eq!(settings.handshake_timeout(), Duration::from_millis(1));
        assert_eq!(settings.write_timeout(), Duration::from_secs(5));
        assert_eq!(
            settings.current_state_interval(),
            Duration::from_millis(250)
        );

        // The built-in deadlines themselves, so changing one has to be deliberate
        // rather than silently round-tripping through the constructor.
        let default = SessionSettings::default();
        assert_eq!(default.handshake_timeout(), Duration::from_secs(10));
        assert_eq!(default.write_timeout(), Duration::from_secs(5));
        assert_eq!(default.current_state_interval(), Duration::from_secs(1));
    }
}
