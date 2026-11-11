use super::admission::Admission;
use super::operational_limits::OperationalLimits;
use super::turn_timing::TurnTimings;
use crate::agents::application::{AgentProbe, SharedAgentReadiness};
use crate::agents::infrastructure::NativeHooksPreferences;
use crate::attachments::{
    application::{AttachmentService, HeldReads},
    entrypoint::{http::UploadRoute, socket::SocketUploads},
};
use crate::conversation::application::{
    CatalogueReadSource, ConversationAccessPolicyEvaluator, ConversationMetadataReader,
    ConversationRepository, ConversationService, FenceCapture, McpAppAudit,
    OwnerPhonePolicyEvaluator, OwnerPhoneReceiverAuthority, ReceiverAuthority, RecordReadSource,
    ReviewNoticeAuditor, ShareGrants, WatchCatalogue, WatchRecords,
};
pub(crate) use crate::core::limit_log::note_limit;
use crate::device_pairing::application::NativeLinking;
use crate::device_pairing::infrastructure::PairingOwnerCommands;
use crate::mcp_servers::{
    application::McpServerSettings, entrypoint::http::ResourceRoute,
    infrastructure::ResourceTicketStore,
};
use crate::peer_gateways::infrastructure::PeerCommands;
use crate::timer::clock;
use crate::webhooks::infrastructure::WebhookRuntime;
use axum::extract::FromRef;
use nessa_agent_install::application::AgentInstallations;
use nessa_auth::{
    application::{
        credential_admin::CredentialAdmin,
        pairing::OwnerPhonePurposeSource,
        ports::{AccessReader, Clock, CredentialVerifier, PolicyEvaluator},
    },
    domain::{AudienceId, OrganizationId, Resource, ResourceId},
};
use nessa_protocol::clock::Clock as UptimeClock;
use nessa_protocol::product::generated::AgentsListResult;
use serde_json::Value;
use std::{sync::Arc, time::Duration};
use tokio::sync::watch;

/// The receiver bindings, ownership and read grants a paired device's reads
/// are admitted by.
pub(crate) type PassiveReadAuthorities = (
    Arc<dyn ReceiverAuthority>,
    Arc<dyn ConversationRepository>,
    Arc<dyn ShareGrants>,
);

/// The paired environments connected now, as `PeerEnvironments` answers
/// for them: their names, read when asked, and a wake-up each time which of
/// them are connected may have changed.
#[derive(Clone)]
pub(crate) struct ConnectedEnvironments {
    /// The names of those connected now.
    pub(crate) names: Arc<dyn Fn() -> Vec<String> + Send + Sync>,
    /// Moves when one connects, when one's connection ends, and when one is
    /// revoked; the agent catalogue's subscriptions read the catalogue again
    /// when it does.
    pub(crate) changes: watch::Receiver<u64>,
}

/// The process's one wall timeline, the one every agent's `dueAtMs` is
/// promised on (`nessa_sdk::infrastructure::clock::process_unix_ms`).
struct ProcessTimeline;
impl Clock for ProcessTimeline {
    fn unix_milliseconds(&self) -> u64 {
        nessa_sdk::infrastructure::clock::process_unix_ms()
    }
}

/// Dependencies and trusted gateway selectors for the product route.
///
/// This state is constructed only in composition.
#[derive(Clone)]
pub struct ProductRouteState {
    /// One capacity pool per lane; which lane a method waits in is the
    /// manifest's, how many permits it holds [`OperationalLimits`]'s.
    pub(crate) admission: Admission,
    /// Becomes true once host cleanup begins. Every authenticated socket
    /// then closes `temporary_unavailable`, so a client reconnects after the
    /// restart instead of reading from a gateway that is stopping.
    pub(crate) closing: Arc<tokio::sync::watch::Sender<bool>>,
    pub(crate) settings: SessionSettings,
    /// Admission counts and the cold-read budget. `admission` is built from
    /// it; a socket reads its slot sizes from it too.
    pub(crate) limits: OperationalLimits,
    pub(crate) gateway: Resource,
    pub(crate) audience: AudienceId,
    pub(crate) verifier: Arc<dyn CredentialVerifier>,
    pub(crate) access: Arc<dyn AccessReader>,
    pub(crate) clock: Arc<dyn Clock>,
    /// Send-to-first-token samples written under the log directory. Quiet
    /// until composition points it at that file. Nothing leaves the machine.
    pub(crate) local_timings: Arc<TurnTimings>,
    pub(crate) policy: Arc<dyn PolicyEvaluator>,
    pub(crate) uptime_clock: Arc<dyn UptimeClock>,
    /// The time a conversation view tells a client, which it measures every
    /// deadline's `dueAtMs` against. It reads the timeline the agents' review
    /// deadlines are promised on, not [`Self::clock`]: the two answer the
    /// same question only until the operating system corrects its wall clock.
    pub(crate) view_clock: Arc<dyn Clock>,
    pub(crate) agent_readiness: Arc<SharedAgentReadiness>,
    // Each feature's dependencies, grouped by the methods that use them. A
    // field is `None` where composition did not set that feature up, and its
    // methods then answer `*_not_configured` or `*_unavailable`.
    pub(crate) browser: BrowserRoutes,
    pub(crate) owner_phone: OwnerPhoneRoutes,
    pub(crate) conversation: ConversationRoutes,
    pub(crate) reads: ReadRoutes,
    pub(crate) agents: AgentRoutes,
    pub(crate) files: FileRoutes,
    pub(crate) mcp: McpRoutes,
    pub(crate) linking: LinkingRoutes,
    pub(crate) machines: MachineRoutes,
    pub(crate) accounts: AccountRoutes,
}

/// The browser sign-in a socket or HTTP request arrived with, and the
/// store that remembers browser sessions.
#[derive(Clone, Default)]
pub(crate) struct BrowserRoutes {
    pub(crate) sessions: Option<Arc<dyn crate::browser_session::application::SessionStore>>,
    pub(crate) http_allowed: bool,
    pub(crate) session_id: Option<String>,
    pub(crate) session_origin: Option<String>,
}

/// How the owner's own phone is told apart and what it may reach.
#[derive(Clone)]
pub(crate) struct OwnerPhoneRoutes {
    pub(crate) purpose: Arc<dyn OwnerPhonePurposeSource>,
    pub(crate) policy: Arc<dyn OwnerPhonePolicyEvaluator>,
    pub(crate) receivers: Option<Arc<dyn OwnerPhoneReceiverAuthority>>,
}

/// Conversations and what a turn answers to: the service, remembered
/// permission rules, and the deletion inventory an unshare reconciles.
#[derive(Clone, Default)]
pub(crate) struct ConversationRoutes {
    pub(crate) service: Option<Arc<ConversationService>>,
    /// Remembered permission rules, and the board a waiting review notifies.
    pub(crate) permission_rules: Option<Arc<crate::conversation::infrastructure::PermissionMemory>>,
    /// Reconciles the deletion inventory after `conversation.unshare`.
    /// Absent, the grant is still revoked.
    pub(crate) fence_capture: Option<Arc<dyn FenceCapture>>,
}

/// What a passive read or a subscription reads without opening an Agent,
/// and what decides a paired party's access to it.
#[derive(Clone, Default)]
pub(crate) struct ReadRoutes {
    /// What wakes a subscription: committed record changes and changes to
    /// an owner's catalogue.
    pub(crate) record_watches: Option<Arc<dyn WatchRecords>>,
    pub(crate) catalogue_watches: Option<Arc<dyn WatchCatalogue>>,
    /// Passive reads use these independent authorities without opening an Agent.
    /// The read grants are also what every socket read asks for a paired
    /// device (`read_access`).
    pub(crate) passive_read: Option<PassiveReadAuthorities>,
    /// Checked metadata dependency for review notices; composed with passive reads.
    pub(crate) review_metadata: Option<ConversationMetadataReader>,
    /// Admitted policy evidence outlives its publisher and precedes UI disclosure.
    /// Host cleanup fences this shared owner and observes its admitted selections.
    pub(crate) review_auditor: Option<Arc<ReviewNoticeAuditor>>,
    /// What decides a paired party's access to a conversation or a
    /// catalogue; set with `passive_read`.
    pub(crate) conversation_access: Option<Arc<dyn ConversationAccessPolicyEvaluator>>,
    pub(crate) catalogue_source: Option<Arc<dyn CatalogueReadSource>>,
    pub(crate) record_source: Option<Arc<dyn RecordReadSource>>,
}

/// The agent catalogue, installs and native hooks (`agents.*`).
#[derive(Clone, Default)]
pub(crate) struct AgentRoutes {
    pub(crate) installations: Option<Arc<dyn AgentInstallations>>,
    pub(crate) catalog: Option<Arc<AgentsListResult>>,
    /// Claude and Codex native-hook choices. None when this route has no
    /// conversation directory. OpenCode is not in the store.
    pub(crate) native_hooks: Option<Arc<NativeHooksPreferences>>,
    /// The registry's principal kinds, for the native-hooks audit. None in a
    /// route that has no registry; a change then records `unknown`.
    pub(crate) caller_kinds: Option<Arc<dyn crate::agents::application::CallerKindSource>>,
    /// The paired environments connected now, by name: added to the
    /// catalog's places each time it is read, since they come and go.
    pub(crate) connected_environments: Option<ConnectedEnvironments>,
}

/// Attachments: uploads in pieces, and held files read back.
#[derive(Clone, Default)]
pub(crate) struct FileRoutes {
    pub(crate) attachments: Option<AttachmentService>,
    /// Where `artifact.read` reads a held file back, over the same store.
    pub(crate) held_reads: Option<HeldReads>,
    /// Uploads `attachment.write` is receiving in pieces, gateway-wide, so a
    /// piece is taken whichever connection it arrives on.
    pub(crate) socket_uploads: SocketUploads,
}

/// Stored MCP servers, their remote authorization, and the MCP App
/// resources held behind tickets.
#[derive(Clone, Default)]
pub(crate) struct McpRoutes {
    /// The MCP App resources held behind tickets: issued by the conversation
    /// service, redeemed at `GET /mcp-resources`, which records each
    /// redemption in the audit beside it before serving. `None` when no MCP server is
    /// composed, and then that route answers every ticket `404`.
    pub(crate) resource_tickets: Option<(Arc<ResourceTicketStore>, Arc<dyn McpAppAudit>)>,
    /// What `mcpServers.list`, `.save`, `.remove` and `.inspect` manage;
    /// `None` where this gateway holds no live MCP server set — not Unix, no
    /// agents configured, or MCP off this run because its relay socket could
    /// not be bound, a path was not UTF-8 or no key for its digests could be
    /// drawn — which they answer `mcp_servers_not_configured`.
    pub(crate) settings: Option<Arc<McpServerSettings>>,
    /// Remote MCP authorization. `None` when this gateway is not managing
    /// stored servers. `mcpServers.authorize` and `mcpServers.revoke` use it.
    pub(crate) authorization:
        Option<Arc<crate::mcp_authorization::application::AuthorizationOwner>>,
}

/// Pairing devices and peers, native linking, and the web tunnel.
#[derive(Clone, Default)]
pub(crate) struct LinkingRoutes {
    /// Owner pairing commands, composed only when `config.json` names a native
    /// listen address. `None` answers every pairing method
    /// `pairing_not_configured`.
    pub(crate) commands: Option<Arc<PairingOwnerCommands>>,
    /// Turns native linking on or off (`pairing.setLinking`), composed
    /// whether or not it is on now. `None` — not Unix — answers
    /// `pairing_unavailable`.
    pub(crate) native_linking: Option<Arc<dyn NativeLinking>>,
    /// The web tunnel this run serves (`pairing.tunnel`, `server.reach`).
    /// `None` when it runs none.
    pub(crate) tunnel: Option<crate::tunnel::application::TunnelView>,
    /// Paired devices' tunnel secrets. `None` when this gateway does not pair devices.
    pub(crate) tunnel_secrets:
        Option<std::sync::Arc<dyn crate::tunnel::application::DeviceTunnelSecrets>>,
    /// This gateway's peer commands, composed with native pairing: a peer is
    /// enrolled with the native key. `None` answers every peer method
    /// `peer_not_configured`.
    pub(crate) peers: Option<Arc<PeerCommands>>,
}

/// The hub's devices, the default device, and channel workspaces.
#[derive(Clone, Default)]
pub(crate) struct MachineRoutes {
    /// The hub's devices (`devices.list`). `None` answers
    /// `devices_unavailable`.
    pub(crate) devices: Option<Arc<crate::devices::application::Devices>>,
    /// Suggesting and connecting SSH hosts and choosing the default device
    /// (`devices.sshHosts`, `devices.connectSsh`, `devices.setDefault`), and
    /// the default the catalog names. `None` — not Unix — answers
    /// `devices_unavailable`.
    pub(crate) device_setup: Option<Arc<crate::devices::application::DeviceSetup>>,
    /// Moves each time the default device is saved: the catalog's
    /// subscribers read it again.
    pub(crate) default_device: Option<watch::Receiver<Option<String>>>,
    #[cfg(unix)]
    pub(crate) workspaces: Option<Arc<crate::workspaces::application::Workspaces>>,
}

/// Credentials and accounts: credential admin, webhooks, GitHub, saved
/// logins, voice and the vault.
#[derive(Clone, Default)]
pub(crate) struct AccountRoutes {
    pub(crate) admin: Option<Arc<dyn CredentialAdmin>>,
    pub(crate) webhooks: Option<Arc<WebhookRuntime>>,
    /// The GitHub sign-in (`github.*`). `None` — no agents run here —
    /// answers `github_not_configured`.
    pub(crate) github: Option<Arc<super::GithubRoutes>>,
    /// The saved logins (`logins.*`). `None` answers
    /// `logins_not_configured`.
    pub(crate) logins: Option<Arc<crate::logins::application::SavedLoginsService>>,
    /// The voice key and calls (`voice.*`). `None` answers
    /// `voice_not_configured`.
    pub(crate) voice: Option<Arc<crate::voice::application::VoiceCalls>>,
    /// Shared vault; each socket attaches a distinct non-owning lease.
    pub(crate) vault: Option<Arc<super::VaultRoutes>>,
    pub(crate) vault_lease: Option<nessa_vault::application::Lease>,
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
        UploadRoute::new(state.files.attachments.clone())
    }
}

/// The resource route is given the ticket store and nothing else, for the
/// same reason as the upload route: a ticket is all it acts on.
impl FromRef<ProductRouteState> for ResourceRoute {
    fn from_ref(state: &ProductRouteState) -> Self {
        ResourceRoute::new(state.mcp.resource_tickets.clone())
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
    /// Canonical current proof classification, independent of native listener configuration.
    /// Unavailable evidence is a refusal; it cannot become an ordinary owner.
    pub owner_phone_purpose: Arc<dyn OwnerPhonePurposeSource>,
    /// Additional first-party phone method/target policy, constructed before serving.
    pub owner_phone_policy: Arc<dyn OwnerPhonePolicyEvaluator>,
    /// Original receiver holding, independently composed from optional read services.
    /// None is unavailable for a genuine phone purpose, never ordinary owner access.
    pub owner_phone_receivers: Option<Arc<dyn OwnerPhoneReceiverAuthority>>,
    /// Existing server clock used only to report health uptime.
    pub uptime_clock: Arc<dyn UptimeClock>,
    /// Asks this host which agents could start here. Chosen in composition so
    /// no route handler constructs a machine probe of its own. How often it may
    /// be asked is this state's to decide, not composition's — see
    /// [`SharedAgentReadiness`].
    pub agent_probe: Arc<dyn AgentProbe>,
}

impl ProductRouteState {
    /// Begin host cleanup: every authenticated socket closes
    /// `temporary_unavailable` ([`Self::closed`]).
    pub(crate) fn close_sockets(&self) {
        self.closing.send_replace(true);
    }

    /// Resolves once [`Self::close_sockets`] has been called, at once if it
    /// already was.
    pub(crate) async fn closed(&self) {
        let mut closing = self.closing.subscribe();
        let _ = closing.wait_for(|closing| *closing).await;
    }

    /// Admit no more `mcpServers.save`, `.remove` or `.inspect` — each later
    /// one answers `mcp_servers_stopping` — and stop the inspections under
    /// way now, as cleanup begins rather than after the conversations drain
    /// ([`McpServerSettings::close`]). The changes admitted run on until the
    /// MCP stop drains them before the servers stop
    /// ([`McpServerSettings::shutdown`]).
    pub(crate) fn close_mcp_server_admission(&self) {
        if let Some(settings) = &self.mcp.settings {
            settings.close();
        }
    }

    /// Bind trusted gateway ownership and audience to an isolated dependency scope.
    pub fn new(
        gateway_id: ResourceId,
        gateway_organization_id: OrganizationId,
        audience: AudienceId,
        dependencies: ProductDependencies,
    ) -> Self {
        let limits = OperationalLimits::small();
        Self {
            settings: SessionSettings::default(),
            limits,
            admission: Admission::new(limits),
            closing: Arc::new(tokio::sync::watch::Sender::new(false)),
            gateway: Resource::new(gateway_organization_id, gateway_id),
            audience,
            verifier: dependencies.verifier,
            access: dependencies.access,
            clock: dependencies.clock,
            local_timings: Arc::new(TurnTimings::quiet()),
            policy: dependencies.policy,
            owner_phone: OwnerPhoneRoutes {
                purpose: dependencies.owner_phone_purpose,
                policy: dependencies.owner_phone_policy,
                receivers: dependencies.owner_phone_receivers,
            },
            browser: BrowserRoutes::default(),
            conversation: ConversationRoutes::default(),
            reads: ReadRoutes::default(),
            agents: AgentRoutes::default(),
            files: FileRoutes::default(),
            mcp: McpRoutes::default(),
            linking: LinkingRoutes::default(),
            machines: MachineRoutes::default(),
            accounts: AccountRoutes::default(),
            uptime_clock: dependencies.uptime_clock,
            view_clock: Arc::new(ProcessTimeline),
            agent_readiness: Arc::new(SharedAgentReadiness::new(dependencies.agent_probe)),
        }
    }

    pub fn with_browser_sessions(
        mut self,
        store: Arc<dyn crate::browser_session::application::SessionStore>,
    ) -> Self {
        self.browser.sessions = Some(store);
        self
    }

    pub(crate) fn with_local_timings(
        mut self,
        store: Arc<dyn super::turn_timing::FirstTokenStore>,
    ) -> Self {
        self.local_timings = Arc::new(TurnTimings::recording(store));
        self
    }

    pub fn with_settings(mut self, settings: SessionSettings) -> Self {
        self.settings = settings;
        self
    }

    /// Replace the gateway admission and the limits a socket reads.
    /// Called once, from composition, before anything is served.
    pub fn with_limits(mut self, limits: OperationalLimits) -> Self {
        self.admission = Admission::new(limits);
        self.limits = limits;
        self
    }

    /// Bind the concrete retained webhook/ordinary credential command owner.
    pub fn with_webhooks(mut self, runtime: Arc<WebhookRuntime>) -> Self {
        self.accounts.admin = Some(runtime.clone());
        self.accounts.webhooks = Some(runtime);
        self
    }

    /// Register credential lifecycle operations; missing administration fails closed.
    pub fn with_admin(mut self, admin: Arc<dyn CredentialAdmin>) -> Self {
        self.accounts.admin = Some(admin);
        self
    }

    /// Record share revocations in the deletion inventory. Without it, an
    /// unshare still revokes the grant.
    pub fn with_fence_capture(mut self, capture: Arc<dyn FenceCapture>) -> Self {
        self.conversation.fence_capture = Some(capture);
        self
    }

    /// Register what manages this gateway's stored MCP servers. Without it
    /// the `mcpServers.*` methods answer `mcp_servers_not_configured`.
    pub fn with_mcp_server_settings(mut self, settings: Arc<McpServerSettings>) -> Self {
        self.mcp.settings = Some(settings);
        self
    }

    /// Register the remote authorization owner. Without it, authorize and
    /// revoke answer `mcp_servers_not_configured`.
    pub fn with_mcp_authorization(
        mut self,
        authorization: Arc<crate::mcp_authorization::application::AuthorizationOwner>,
    ) -> Self {
        self.mcp.authorization = Some(authorization);
        self
    }

    /// Register the owner pairing commands of this gateway's native enrollment
    /// runtime. Without them the pairing methods answer `pairing_not_configured`.
    pub fn with_pairing(mut self, pairing: Arc<PairingOwnerCommands>) -> Self {
        self.linking.commands = Some(pairing);
        self
    }

    /// Register what turns native linking on or off from Settings.
    pub fn with_native_linking(mut self, linking: Arc<dyn NativeLinking>) -> Self {
        self.linking.native_linking = Some(linking);
        self
    }

    /// Register channel workspace allocation.
    #[cfg(unix)]
    pub(crate) fn with_workspaces(
        mut self,
        workspaces: Arc<crate::workspaces::application::Workspaces>,
    ) -> Self {
        self.machines.workspaces = Some(workspaces);
        self
    }
    /// Register the reusable vault independently of provider configuration.
    pub fn with_vault(mut self, vault: Arc<super::VaultRoutes>) -> Self {
        self.accounts.vault = Some(vault);
        self
    }

    /// Register the web tunnel this run serves (Unix only).
    #[cfg_attr(not(unix), allow(dead_code))]
    pub(crate) fn with_tunnel(mut self, tunnel: crate::tunnel::application::TunnelView) -> Self {
        self.linking.tunnel = Some(tunnel);
        self
    }

    /// Register the tunnel secrets issued at pairing.
    pub(crate) fn with_tunnel_secrets(
        mut self,
        secrets: std::sync::Arc<dyn crate::tunnel::application::DeviceTunnelSecrets>,
    ) -> Self {
        self.linking.tunnel_secrets = Some(secrets);
        self
    }

    /// Set the hub's machines up from Devices, and offer the default device
    /// `default` holds in the catalog.
    #[cfg_attr(not(unix), allow(dead_code))]
    pub(crate) fn with_device_setup(
        mut self,
        setup: Arc<crate::devices::application::DeviceSetup>,
        default: watch::Receiver<Option<String>>,
    ) -> Self {
        self.machines.device_setup = Some(setup);
        self.machines.default_device = Some(default);
        self
    }

    pub(crate) fn with_devices(
        mut self,
        devices: Arc<crate::devices::application::Devices>,
    ) -> Self {
        self.machines.devices = Some(devices);
        self
    }

    /// Register the GitHub sign-in the github methods answer for.
    pub fn with_github(mut self, github: Arc<super::GithubRoutes>) -> Self {
        self.accounts.github = Some(github);
        self
    }

    /// Register the saved logins the logins methods answer for.
    pub fn with_logins(
        mut self,
        logins: Arc<crate::logins::application::SavedLoginsService>,
    ) -> Self {
        self.accounts.logins = Some(logins);
        self
    }

    /// Register the voice key and calls the voice methods answer for.
    pub fn with_voice(mut self, voice: Arc<crate::voice::application::VoiceCalls>) -> Self {
        self.accounts.voice = Some(voice);
        self
    }

    /// Register this gateway's peer commands. Without them the peer methods
    /// answer `peer_not_configured`.
    pub fn with_peers(mut self, peers: Arc<PeerCommands>) -> Self {
        self.linking.peers = Some(peers);
        self
    }

    /// Share server-owned Agents across authenticated sockets. No socket owns cleanup.
    pub fn with_conversations(mut self, service: Arc<ConversationService>) -> Self {
        self.conversation.service = Some(service);
        self
    }

    /// Bind the remembered permission rules and the waiting-review notices.
    pub fn with_permission_rules(
        mut self,
        rules: Arc<crate::conversation::infrastructure::PermissionMemory>,
    ) -> Self {
        self.conversation.permission_rules = Some(rules);
        self
    }

    /// Bind the server-owned receiver state and conversation ownership
    /// source, and the access policy a paired party's reads are decided by.
    pub fn with_passive_read(
        mut self,
        receivers: Arc<dyn ReceiverAuthority>,
        conversations: Arc<dyn ConversationRepository>,
        share_grants: Arc<dyn ShareGrants>,
        access: Arc<dyn ConversationAccessPolicyEvaluator>,
    ) -> Self {
        self.reads.review_metadata = Some(ConversationMetadataReader::new(conversations.clone()));
        self.reads.passive_read = Some((receivers, conversations, share_grants));
        self.reads.conversation_access = Some(access);
        self
    }

    /// Bind the shared application owner of unpaired review-policy audit attempts.
    /// A route with metadata but without this dependency refuses review publication.
    pub fn with_review_notice_auditor(mut self, auditor: Arc<ReviewNoticeAuditor>) -> Self {
        self.reads.review_auditor = Some(auditor);
        self
    }

    /// Compose the local producers of committed record and catalogue
    /// changes that wake subscriptions.
    pub fn with_change_watches(
        mut self,
        records: Arc<dyn WatchRecords>,
        catalogue: Arc<dyn WatchCatalogue>,
    ) -> Self {
        self.reads.record_watches = Some(records);
        self.reads.catalogue_watches = Some(catalogue);
        self
    }

    /// Inject current catalogue metadata reads behind passive admission.
    pub fn with_catalogue_source(mut self, source: Arc<dyn CatalogueReadSource>) -> Self {
        self.reads.catalogue_source = Some(source);
        self
    }

    /// Inject the bounded SDK record adapter without opening a conversation Agent.
    pub fn with_record_source(mut self, source: Arc<dyn RecordReadSource>) -> Self {
        self.reads.record_source = Some(source);
        self
    }

    /// Install only composition-selected pinned releases through the authenticated gateway.
    pub fn with_agent_installations(mut self, installations: Arc<dyn AgentInstallations>) -> Self {
        self.agents.installations = Some(installations);
        self
    }

    /// Publish the validated choices composed for this gateway.
    /// Replace the clock a conversation view's `serverNowMs` is read from.
    /// Composition keeps the default, the agents' own timeline; a test
    /// injects a clock it controls.
    pub fn with_view_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.view_clock = clock;
        self
    }

    pub fn with_agents_catalog(mut self, catalog: AgentsListResult) -> Self {
        self.agents.catalog = Some(Arc::new(catalog));
        self
    }

    /// The native-hook choices Claude and Codex launches read.
    pub fn with_native_hooks(mut self, native_hooks: Arc<NativeHooksPreferences>) -> Self {
        self.agents.native_hooks = Some(native_hooks);
        self
    }

    /// Where a native-hooks change reads the caller's kind.
    pub fn with_caller_kinds(
        mut self,
        caller_kinds: Arc<dyn crate::agents::application::CallerKindSource>,
    ) -> Self {
        self.agents.caller_kinds = Some(caller_kinds);
        self
    }

    /// Offer the paired environments connected when the catalog is read as
    /// places a new conversation may run, after its SSH hosts, and read it
    /// again for its subscribers when they change.
    #[cfg_attr(not(any(unix, test)), allow(dead_code))]
    pub(crate) fn with_connected_environments(mut self, connected: ConnectedEnvironments) -> Self {
        self.agents.connected_environments = Some(connected);
        self
    }

    /// The agent catalog as it is now: the agents and SSH hosts composed at
    /// startup, then the paired environments connected at this moment, and
    /// which of those take the owner's commands.
    /// `None` where conversations are not configured. `agents.list` answers
    /// it and every `agents.listed` frame carries it, so the two are read
    /// the same way.
    pub(crate) fn agents_catalog_now(&self) -> Option<Value> {
        let composed = self.agents.catalog.as_ref()?;
        let mut catalog =
            serde_json::to_value(composed.as_ref()).expect("generated catalog serializes");
        if let Some(connected) = &self.agents.connected_environments {
            if let Some(places) = catalog["environments"].as_array_mut() {
                places.extend((connected.names)().into_iter().map(Value::String));
            }
        }
        // Of those places, the ones the owner's turns may run commands on,
        // read with them: the conversation service asks the command policy.
        // The composed catalog's own copy is never shown.
        let places: Vec<&str> = catalog["environments"]
            .as_array()
            .map(|places| places.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        // The default device, while it is one of those places.
        let default = self
            .machines
            .default_device
            .as_ref()
            .and_then(|default| default.borrow().clone())
            .filter(|default| places.contains(&default.as_str()));
        let command_hosts = self
            .conversation
            .service
            .as_ref()
            .map(|service| service.owner_command_hosts(places))
            .unwrap_or_default();
        catalog["commandHosts"] = Value::from(command_hosts);
        if let Some(default) = default {
            catalog["defaultEnvironment"] = Value::from(default);
        }
        // The stored hooks choice, read now. The composed catalog's own copy
        // is never shown.
        let selection = self
            .agents
            .native_hooks
            .as_ref()
            .map(|hooks| hooks.selection())
            .unwrap_or_else(crate::agents::application::NativeHooksSelection::off);
        catalog["nativeHooks"] = native_hooks_value(selection);
        Some(catalog)
    }

    /// A wake-up each time a paired environment may have come or gone,
    /// seen as of now; `None` on a gateway that pairs no environments.
    pub(crate) fn agents_catalog_changes(&self) -> Option<watch::Receiver<u64>> {
        let mut changes = self.agents.connected_environments.as_ref()?.changes.clone();
        changes.mark_unchanged();
        Some(changes)
    }

    /// A wake-up each time the default device is saved, seen as of now;
    /// `None` where Devices cannot set one.
    pub(crate) fn default_device_changes(&self) -> Option<watch::Receiver<Option<String>>> {
        let mut changes = self.machines.default_device.clone()?;
        changes.mark_unchanged();
        Some(changes)
    }

    /// A wake-up after each native hooks command, seen as of now; `None`
    /// where hooks are not configured.
    pub(crate) fn native_hooks_changes(&self) -> Option<watch::Receiver<u64>> {
        Some(self.agents.native_hooks.as_ref()?.changes())
    }

    /// Share one attachment service between the socket that issues tickets and
    /// the route that redeems them, and read held files back over the same
    /// store. Composed only alongside conversations.
    pub fn with_attachments(mut self, service: AttachmentService, reads: HeldReads) -> Self {
        self.files.attachments = Some(service);
        self.files.held_reads = Some(reads);
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
        self.mcp.resource_tickets = Some((tickets, audit));
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
            if value.is_zero() || clock::now().checked_add(value).is_none() {
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

fn native_hooks_value(selection: crate::agents::application::NativeHooksSelection) -> Value {
    use nessa_protocol::product::generated::{AgentsNativeHooks, NativeHooksChoice};
    let choice = |enabled| {
        if enabled {
            NativeHooksChoice::On
        } else {
            NativeHooksChoice::Off
        }
    };
    serde_json::to_value(AgentsNativeHooks {
        claude: choice(selection.claude()),
        codex: choice(selection.codex()),
    })
    .expect("generated hooks choice serializes")
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
