//! Local product dependency factory. Provider choices stay outside route handlers.
use super::agent::AgentsConfig;
#[cfg(unix)]
use super::current_agent::{CurrentAgentResolver, CurrentAgentResolverInput};
#[cfg(unix)]
use super::opencode_profile::{EffectiveOpenCodeProfile, OpenCodeProfile};
#[cfg(unix)]
use super::warm_up::{CurrentOpenCodeWarmUp, PreparedRuntime};
#[cfg(unix)]
use crate::conversation::infrastructure::NessaRecordWatches;
use crate::product::generated::AgentsListResult;
#[cfg(unix)]
use crate::product::generated::{
    AgentModelOption, AgentOption, ApprovalMode as WireApprovalMode,
    ApprovalModeChoice as WireApprovalModeChoice,
};
#[cfg(unix)]
use crate::{
    agent_warm_up::application::{AgentWarmUp, WarmUpSessionPorts},
    agent_warm_up::{
        domain::RuntimeFingerprint,
        infrastructure::{DurableWarmUpAudit, FileWarmUpRecords},
    },
    agents::{domain::AgentId, infrastructure::AgentLaunchFiles},
    attachments::infrastructure::ModelImageNormalizer,
    conversation::application::{
        ConversationAgents, ConversationDependencies, ConversationLimits, McpAppPorts, McpToolUis,
        NoMcpToolUis,
    },
    conversation::infrastructure::{
        DurableConversationCreationAudit, DurableConversationDeletionAudit,
        DurableConversationFileLinkAudit, DurableConversationModeAudit, DurableMcpAppAudit,
        LocalConversationStore,
    },
};
use crate::{
    agents::{
        application::{AgentCredentialSource, AgentProbe},
        infrastructure::{LocalAgentCredentials, LocalAgentProbe},
    },
    app::ports::Clock as ServerClock,
    attachments::application::AttachmentService,
    browser_session::adapters::PersistentSessions,
    conversation::application::{
        ConversationRepository, ConversationService, McpAppAudit, ReceiverAuthority,
        WatchCatalogue, WatchRecords,
    },
    conversation::infrastructure::{
        LocalReceiverAuthority, NessaCatalogueReadSource, NessaRecordReadSource,
    },
    core::RunError,
    env::Environment,
    mcp_servers::infrastructure::ResourceTicketStore,
    product::{ProductDependencies, ProductRouteState},
};
use nessa_agent_credentials::CredentialNamespace;
use nessa_auth::{
    adapters::{cedar::CedarPolicyEvaluator, local::LocalCredentialStore},
    application::{
        credential_admin::{
            CredentialAdmin, CredentialAdminError, IssueCredentialOutcome, IssueCredentialRequest,
            ListCredentialsRequest, RevokeCredentialOutcome, RevokeCredentialRequest,
        },
        dto::CredentialMetadataDto,
        ports::{Clock, PortFuture},
    },
    domain::{AudienceId, OrganizationId, Resource, ResourceId},
};
#[cfg(unix)]
use nessa_sdk::infrastructure::session_storage::{InMemoryStorage, RecordStorage};
#[cfg(unix)]
use nessa_sdk::{
    application::agent_execution::providers::{ApprovalMode, ApprovalModeChoice},
    infrastructure::{
        claude_acp::sessions::ClaudeAcpProvider, codex_acp::sessions::CodexAcpProvider,
        model_metadata_json::load_catalog, opencode_acp::sessions::OpencodeAcpProvider,
    },
};
use nessa_sync::replication::domain::Id as RecordId;
#[cfg(unix)]
use std::collections::HashSet;
use std::{
    collections::HashMap,
    path::Path,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
#[cfg(unix)]
use tokio::runtime::Handle;

/// Wall time for authentication. Health's monotonic uptime remains a separate port.
pub(super) struct SystemClock;
impl Clock for SystemClock {
    fn unix_milliseconds(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| {
                u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
            })
    }
}

/// Everything composition built that the server lifecycle, rather than a route,
/// has to own. The warm-up is started once the gateway is listening, so it
/// cannot be started here.
pub(super) struct LocalProduct {
    pub(super) routes: ProductRouteState,
    pub(super) record_reader: Option<Arc<NessaRecordReadSource>>,
    pub(super) catalogue_reader: Option<Arc<NessaCatalogueReadSource>>,
    /// Startup preparations for fixed providers plus the current OpenCode
    /// resolver when configured. Empty when no agent is configured.
    pub(super) warm_ups: Vec<StartupWarmUp>,
    /// The gateway's connection to each configured MCP server, and its relay:
    /// started once the gateway is listening, stopped after conversations.
    pub(super) mcp: McpParts,
    /// Native pairing with its key restored and enrollments settled, bound by
    /// the root after the browser listener; `None` unless `config.json` names
    /// a native listen address (design row S1).
    pub(super) native: Option<super::native_pairing::PreparedNative>,
}

#[cfg(unix)]
pub(super) type McpParts = Option<super::mcp_servers::McpComposition>;
/// No MCP servers where no agent can run.
#[cfg(not(unix))]
pub(super) type McpParts = Option<std::convert::Infallible>;

pub(super) enum StartupWarmUp {
    #[cfg(unix)]
    Fixed(AgentWarmUp),
    #[cfg(unix)]
    Current(Arc<CurrentAgentResolver>),
}

impl StartupWarmUp {
    #[cfg(unix)]
    pub(super) fn start(&self) {
        match self {
            Self::Fixed(warm_up) => warm_up.start(),
            Self::Current(resolver) => resolver.start_warm_up(),
        }
    }

    #[cfg(not(unix))]
    pub(super) fn start(&self) {}

    /// Whether this warm-up may still hold a provider process or its use.
    #[cfg(unix)]
    pub(super) fn may_hold_resources(&self) -> bool {
        match self {
            Self::Fixed(warm_up) => warm_up.may_hold_resources(),
            Self::Current(resolver) => resolver.warm_up_may_hold_resources(),
        }
    }
}

/// Construct the guarded product route from a previously initialized local registry.
pub(super) async fn product_state(
    config: &Environment,
    uptime: Arc<dyn ServerClock>,
    bundle: Option<&Path>,
) -> Result<LocalProduct, RunError> {
    let directory = config
        .auth_directory
        .as_ref()
        .ok_or_else(|| RunError::Authentication("set NESSA_DATA_DIR or HOME".into()))?;
    let mut settings = super::runtime_config::RuntimeConfig::load(directory)?;
    if let Some(bundle) = bundle {
        super::desktop::configure(
            &mut settings,
            bundle,
            directory
                .parent()
                .ok_or_else(|| setup_error("invalid data root"))?,
        )?;
    }
    let store = Arc::new(super::credential_registry::open(
        directory,
        settings.registry,
        super::credential_registry::RegistryOpenContext::GatewayStartup,
    )?);
    let identity = store.identity().map_err(|_| {
        RunError::Authentication(
            "initialize local access with `nessa auth init --local --owner-token-file <new-path>`"
                .into(),
        )
    })?;
    if identity.organization_ids.len() != 1 {
        return Err(RunError::Authentication(
            "local gateway requires exactly one personal organization".into(),
        ));
    }
    let record_origin = RecordId::new(identity.gateway_id.clone())
        .map_err(|error| RunError::Authentication(format!("invalid record origin: {error:?}")))?;
    let gateway = ResourceId::new(identity.gateway_id.clone()).map_err(setup_error)?;
    let audience = AudienceId::new(identity.gateway_id).map_err(setup_error)?;
    let organization =
        OrganizationId::new(identity.organization_ids[0].clone()).map_err(setup_error)?;
    let policy = Arc::new(CedarPolicyEvaluator::new().map_err(setup_error)?);
    let namespace = directory
        .parent()
        .ok_or_else(|| setup_error("invalid data root"))?;
    // One receiver authority, shared by conversations' passive reads and by
    // native pairing, which pairs and fences receivers (design row S7).
    let receivers = if settings.agents.is_some() || settings.native.is_some() {
        Some(receiver_access(
            namespace,
            &CedarPolicyEvaluator::profile_digest(),
        )?)
    } else {
        None
    };
    // Before any socket is bound: the key is restored or first published,
    // this gateway's unfinished enrollments are settled, and ended ones have
    // their receivers settled (design rows S3–S8).
    let native = match (&settings.native, &receivers) {
        (Some(native), Some(receivers)) => Some(
            super::native_pairing::prepare(
                native,
                super::native_pairing::NativeInputs {
                    namespace: namespace.to_path_buf(),
                    registry: store.clone(),
                    policy: policy.clone(),
                    receivers: receivers.clone(),
                    clock: Arc::new(SystemClock),
                    gateway: Resource::new(organization.clone(), gateway.clone()),
                },
            )
            .await?,
        ),
        _ => None,
    };
    let credential_namespace = CredentialNamespace::new(
        config.stage.as_str().to_owned(),
        config.instance().map(str::to_owned),
    )
    .map_err(setup_error)?;
    let agent_credentials: Arc<dyn AgentCredentialSource> = Arc::new(
        LocalAgentCredentials::from_environment(credential_namespace),
    );
    // Built here, before the launch files below, because building it is how
    // this server finds out which configured agents cannot be started at all,
    // and that answer belongs in what setup is told. Nothing else between here
    // and its use depends on the order.
    let packaged_agents = bundle.is_some();
    let (conversations, agent_probe, warm_ups, mcp) = match (&settings.agents, receivers) {
        (Some(agents), Some(receivers)) => {
            let mut built = conversations(
                agents,
                directory,
                receivers,
                agent_credentials.clone(),
                packaged_agents,
                record_origin.clone(),
            )
            .await?;
            (
                Some((
                    built.service,
                    built.attachments,
                    built.agents_catalog,
                    built.receivers,
                    built.metadata,
                    built.record_reader,
                    built.catalogue_reader,
                    built.resource_route,
                    built.record_watches,
                    built.catalogue_watches,
                )),
                built.agent_probe,
                built.warm_ups,
                built.mcp.take(),
            )
        }
        _ => (
            None,
            Arc::new(LocalAgentProbe::from_environment(
                HashMap::new(),
                agent_credentials.clone(),
            )) as Arc<dyn AgentProbe>,
            Vec::new(),
            None,
        ),
    };
    let admin = Arc::new(LocalAdmin {
        store: store.clone(),
    });
    let mut product = ProductRouteState::new(
        gateway,
        organization,
        audience,
        ProductDependencies {
            verifier: store.clone(),
            access: store,
            clock: Arc::new(SystemClock),
            policy,
            uptime_clock: uptime,
            agent_probe,
        },
    )
    .with_admin(admin)
    .with_settings(settings.session()?)
    .with_browser_sessions(Arc::new({
        let path = directory.join("browser-sessions.jsonl");
        PersistentSessions::open(&path, SystemClock.unix_seconds())
            .map_err(|cause| RunError::opening_browser_sessions(&path, cause))?
    }));
    if bundle.is_some() {
        product =
            product.with_agent_installations(Arc::new(super::install_command::GatewayInstaller {
                account_id: super::install_command::local_account_id(),
                root: directory
                    .parent()
                    .ok_or_else(|| RunError::Agent("invalid namespace directory".into()))?
                    .join("agents"),
            }));
    }
    product.browser_http_allowed = config.browser_http_allowed();
    let native = match native {
        Some((prepared, commands)) => {
            product = product.with_pairing(Arc::new(commands));
            Some(prepared)
        }
        None => None,
    };
    let mut record_reader = None;
    let mut catalogue_reader = None;
    if let Some((
        service,
        attachments,
        agents_catalog,
        receivers,
        metadata,
        reader,
        catalogue,
        resource_route,
        record_watches,
        catalogue_watches,
    )) = conversations
    {
        record_reader = Some(reader.clone());
        catalogue_reader = Some(catalogue.clone());
        product = product
            .with_conversations(Arc::new(service))
            .with_passive_read(receivers, metadata)
            .with_change_watches(record_watches, catalogue_watches)
            .with_record_source(reader)
            .with_catalogue_source(catalogue)
            .with_attachments(attachments)
            .with_agents_catalog(agents_catalog);
        // The route redeems on the store the conversation service issues on.
        if let Some((tickets, audit)) = resource_route {
            product = product.with_resource_tickets(tickets, audit);
        }
    }
    Ok(LocalProduct {
        routes: product,
        record_reader,
        catalogue_reader,
        warm_ups,
        mcp,
        native,
    })
}

/// What the readiness probe is given to ask about, for every agent it should
/// answer for.
///
/// What onboarding is told about an agent is the same fact the launcher acts
/// on: the configuration as resolved, and whether the files it would actually
/// execute are there. A bundled desktop run and a plain server run answer this
/// the same way, because they answer it from the same place. Composition
/// settles *which* paths those are and hands them over; it does not settle
/// whether they exist, because a person can install the agent long after this
/// runs and setup has a button that says so.
///
/// Every agent the configuration describes, not only the one a new conversation
/// would start on: setup lists them all and a person deciding between them is
/// entitled to the truth about each.
///
/// Except the ones in `unavailable`, which this run already proved it cannot
/// start with the agent sitting there installed. Those are left out entirely,
/// so the probe has nothing to stat and readiness reports the agent as not set
/// up on this installation rather than offering a conversation that would be
/// refused. See [`super::agent::ConfiguredAgents::unavailable`].
#[cfg(unix)]
fn launch_files(
    agents: Option<&AgentsConfig>,
    unavailable: &HashSet<AgentId>,
) -> HashMap<AgentId, AgentLaunchFiles> {
    agents
        .map(|agents| {
            agents
                .agents()
                .into_iter()
                .filter(|(id, _)| !unavailable.contains(id))
                .map(|(id, runtime)| {
                    (
                        id,
                        AgentLaunchFiles {
                            command: runtime.command.executable().to_owned(),
                            paths: runtime.paths(),
                            environment: super::agent::launch_environment(id),
                        },
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Everything building the conversation stack settled.
struct BuiltConversations {
    service: ConversationService,
    record_reader: Arc<NessaRecordReadSource>,
    catalogue_reader: Arc<NessaCatalogueReadSource>,
    record_watches: Arc<dyn WatchRecords>,
    catalogue_watches: Arc<dyn WatchCatalogue>,
    receivers: Arc<dyn ReceiverAuthority>,
    metadata: Arc<dyn ConversationRepository>,
    attachments: AttachmentService,
    agents_catalog: AgentsListResult,
    /// Which configured agents this run cannot start even though they are
    /// installed. Known only once every provider has been built, and needed
    /// before the readiness probe is, which is why this is a function rather
    /// than the tail of [`product_state`]. See
    /// [`super::agent::ConfiguredAgents`].
    agent_probe: Arc<dyn AgentProbe>,
    /// Fixed-provider preparations plus the current OpenCode resolver. Started
    /// by the server lifecycle once the gateway is listening, not here.
    warm_ups: Vec<StartupWarmUp>,
    /// The MCP servers this run holds, when any are configured.
    mcp: McpParts,
    /// What `GET /mcp-resources` redeems on, and records each redemption
    /// in: the store the conversation service issues on, and the audit it
    /// records an app's calls in. `None` without MCP servers.
    resource_route: Option<(Arc<ResourceTicketStore>, Arc<dyn McpAppAudit>)>,
}

#[cfg(not(unix))]
async fn conversations(
    _agents: &AgentsConfig,
    _directory: &Path,
    _receivers: Arc<LocalReceiverAuthority>,
    _credentials: Arc<dyn AgentCredentialSource>,
    _packaged_agents: bool,
    _record_origin: RecordId,
) -> Result<BuiltConversations, RunError> {
    Err(RunError::Agent(
        "ACP agents require Unix process supervision".into(),
    ))
}

/// What an app's calls take, as this gateway wires them: the conversation's
/// own sessions of its MCP servers, `audit` for every step, the ticket
/// store, and the drop recorder's sink — started here with the ticket
/// recorder, both writing to `audit` — for every held context's drop
/// (`a_composed_gateways_dropped_context_is_written_by_its_recorder`).
#[cfg(unix)]
pub(super) fn mcp_app_ports(
    mcp: &mut super::mcp_servers::McpComposition,
    audit: Arc<dyn McpAppAudit>,
) -> McpAppPorts {
    let dropped = mcp.start_recorders(audit.clone());
    McpAppPorts {
        apps: Arc::new(crate::mcp_servers::infrastructure::SessionApps(
            mcp.servers.clone(),
        )),
        audit,
        tickets: mcp.resource_tickets.clone(),
        dropped,
    }
}

/// Open the namespace's receiver-access store, creating its directory.
fn receiver_access(
    namespace: &Path,
    policy_revision: &str,
) -> Result<Arc<LocalReceiverAuthority>, RunError> {
    let root = conversation_root(namespace);
    nessa_local_storage::create_directory(&root)
        .map_err(|error| RunError::Agent(error.to_string()))?;
    let path = root.join("receiver-access.sqlite3");
    LocalReceiverAuthority::open(&path, policy_revision, Arc::new(SystemClock))
        .map(Arc::new)
        .map_err(|cause| RunError::opening(crate::core::Dataset::ReceiverAccess, &path, cause))
}

/// Where a namespace keeps its conversations. Read by composition and by the
/// retirement that reports whether they are still there.
pub(crate) fn conversation_root(namespace: &Path) -> std::path::PathBuf {
    namespace.join("conversations")
}

#[cfg(unix)]
async fn conversations(
    agents: &AgentsConfig,
    directory: &Path,
    receivers: Arc<LocalReceiverAuthority>,
    credentials: Arc<dyn AgentCredentialSource>,
    packaged_agents: bool,
    record_origin: RecordId,
) -> Result<BuiltConversations, RunError> {
    let mut warm_ups = Vec::new();
    // The gateway holds the one connection to each MCP server (ADR 344), so
    // every agent below is built with stand-ins in their place.
    let namespace = directory
        .parent()
        .ok_or_else(|| RunError::Agent("invalid namespace directory".into()))?;
    let mut agents = agents.clone();
    let mut mcp = match std::env::current_exe() {
        Ok(gateway) => {
            super::mcp_servers::compose(
                &mut agents,
                // The effective user, whom `bind` requires to own the directory.
                // SAFETY: geteuid has no preconditions and cannot fail.
                &super::mcp_servers::relay_socket(namespace, unsafe { libc::geteuid() }),
                &gateway,
                super::mcp_servers::server_environment(|key| std::env::var_os(key)),
            )
            .await?
        }
        Err(error) => {
            tracing::error!(%error, "MCP servers are off this run: this executable's path is unknown");
            agents.mcp_servers.clear();
            None
        }
    };
    let agents = &agents;
    let root = conversation_root(
        directory
            .parent()
            .ok_or_else(|| RunError::Agent("invalid namespace directory".into()))?,
    );
    nessa_local_storage::create_directory(&root)
        .map_err(|error| RunError::Agent(error.to_string()))?;
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let host = crate::agent_install::infrastructure::host_platform();
    let opencode = EffectiveOpenCodeProfile::decide(
        agents,
        packaged_agents,
        &host,
        (!packaged_agents)
            .then(|| std::env::var_os("OPENCODE_API_KEY"))
            .flatten(),
    );
    if let Some(reason) = opencode.invalid_reason() {
        tracing::error!(%reason, "configured OpenCode policy is invalid");
    }
    // Ownership records come first. A binding needs somewhere to read image
    // bytes, reading them needs the attachment store, and beginning an
    // upload needs to ask who owns a conversation: so the repository is
    // built, then attachments over it, and only then the providers.
    //
    // Ownership, tombstones and summaries are one database
    // (docs/adr/done/196-conversation-metadata-database.md).
    let metadata_path = root.join("metadata.sqlite3");
    let metadata = Arc::new(
        LocalConversationStore::open(&metadata_path).map_err(|cause| {
            RunError::opening(
                crate::core::Dataset::ConversationMetadata,
                &metadata_path,
                cause,
            )
        })?,
    );
    let receivers: Arc<dyn ReceiverAuthority> = receivers;
    let current_opencode_model = opencode
        .configured()
        .map(|profile| profile.validated().model());
    let attachments = super::attachments::attachments(
        &directory
            .parent()
            .ok_or_else(|| RunError::Agent("invalid namespace directory".into()))?
            .join("attachments"),
        metadata.clone(),
        // The image limits from the catalog, which is the one place they
        // are recorded, taken across every configured agent's model rather
        // than the selected one's: the store is shared by conversations
        // that each run on their own agent. Every uploaded image is fitted
        // to them, using the running system's decoder for the encodings the
        // image library does not read itself.
        Arc::new(
            ModelImageNormalizer::new(
                super::agent::image_limits(agents, current_opencode_model)?.as_ref(),
                nessa_images::platform_decoder(),
            )
            .map_err(|error| RunError::Agent(format!("model image limits: {error}")))?,
        ),
        clock.clone(),
    )?;
    let managed_adapters: HashSet<_> = if packaged_agents {
        HashSet::from([AgentId::Claude, AgentId::Codex])
    } else {
        HashSet::new()
    };
    let mut deferred = if opencode.configured().is_some() {
        HashSet::from([AgentId::Opencode])
    } else {
        HashSet::new()
    };
    if packaged_agents {
        deferred.extend([AgentId::Claude, AgentId::Codex]);
    }
    let mut built = super::agent::providers(
        agents,
        &root,
        clock.clone(),
        attachments.images.clone(),
        credentials.clone(),
        &deferred,
    )?;
    // One warm-up per configured agent, because each runs its own runtime
    // and the operating system scans each of them separately on its first
    // execution. One for the server would leave whichever agent it did not
    // cover paying that scan inside somebody's first message, which is the
    // failure this exists to prevent.
    //
    // They share one records directory and one audit directory: a record is
    // stored under a digest of the runtime it describes, so two runtimes
    // never collide, and a third configured later finds no record of its
    // own and warms itself.
    let records = Arc::new(
        FileWarmUpRecords::new(root.join("warm-up"))
            .map_err(|error| RunError::Agent(error.to_string()))?,
    );
    let warm_up_audit = Arc::new(
        DurableWarmUpAudit::new(root.join("audit").join("warm-up"))
            .map_err(|error| RunError::Agent(error.to_string()))?,
    );
    for agent in built.providers.values_mut() {
        // The provider's own credential-free identity, rather than a
        // hand-picked list of fields: it already covers the executable, its
        // arguments, the environment, the workspace, and every MCP server
        // binary the child will start, and it is computed from raw OS bytes
        // rather than a lossy path conversion. Anything that changes which
        // files are executed changes it, which is what a first-execution
        // scan is paid for.
        let identity = agent.provider.identity();
        let runtime =
            RuntimeFingerprint::new(identity.name(), identity.model_id(), identity.context())
                .map_err(|error| RunError::Agent(error.to_string()))?;
        let prepared = AgentWarmUp::new(
            agent.provider.clone(),
            agent.execution_audit.clone(),
            // A throwaway context: the warm-up must not leave a snapshot on
            // disk and must not take an exclusive lease on a conversation a
            // user owns.
            WarmUpSessionPorts {
                storage: Arc::new(InMemoryStorage::new()),
                message_commit_clock: Arc::new(
                    nessa_sdk::infrastructure::session_storage::RuntimeMessageCommitClock::new(),
                ),
            },
            records.clone(),
            warm_up_audit.clone(),
            clock.clone(),
            runtime,
        );
        agent.readiness = Some(Arc::new(PreparedRuntime(prepared.clone())));
        warm_ups.push(StartupWarmUp::Fixed(prepared));
    }
    let storage = Arc::new(
        RecordStorage::new(root.join("sessions"))
            .map_err(|error| RunError::Agent(error.to_string()))?,
    );
    storage
        .initialize()
        .await
        .map_err(|error| RunError::Agent(error.to_string()))?;
    let record_watches = Arc::new(NessaRecordWatches::new(storage.clone()));
    let catalogue_watches: Arc<dyn WatchCatalogue> = metadata.clone();
    let record_reader = Arc::new(NessaRecordReadSource::new(
        storage.clone(),
        record_origin.clone(),
        Handle::current(),
    ));
    let catalogue_reader = Arc::new(NessaCatalogueReadSource::new(
        metadata.clone(),
        record_origin,
    ));
    let creation_audit = Arc::new(
        DurableConversationCreationAudit::new(root.join("audit").join("creation"))
            .map_err(|error| RunError::Agent(error.to_string()))?,
    );
    let mode_audit = Arc::new(
        DurableConversationModeAudit::new(root.join("audit").join("approval-mode"), clock.clone())
            .map_err(|error| RunError::Agent(error.to_string()))?,
    );
    // Every step of an MCP App's call (#348), for the conversation service's
    // app calls.
    let mcp_app_audit: Arc<dyn McpAppAudit> = Arc::new(
        DurableMcpAppAudit::new(root.join("audit").join("mcp-apps"), clock.clone())
            .map_err(|error| RunError::Agent(error.to_string()))?,
    );
    let file_link_audit = Arc::new(
        DurableConversationFileLinkAudit::new(root.join("audit").join("file-links"))
            .map_err(|error| RunError::Agent(error.to_string()))?,
    );
    let deletion_audit = Arc::new(
        DurableConversationDeletionAudit::new(root.join("audit").join("deletion"), clock.clone())
            .map_err(|error| RunError::Agent(error.to_string()))?,
    );
    let fixed_probe = LocalAgentProbe::from_environment(
        launch_files(Some(agents), &built.unavailable)
            .into_iter()
            .filter(|(agent, _)| *agent != AgentId::Opencode)
            .collect(),
        credentials.clone(),
    );
    let warm_current_opencode = opencode.configured().is_some();
    let mut configured: HashSet<AgentId> = built.providers.keys().copied().collect();
    if warm_current_opencode {
        configured.insert(AgentId::Opencode);
    }
    configured.extend(managed_adapters.iter().copied());
    let agents_catalog = agent_catalog(agents, &configured, opencode.configured())?;
    let resolver = Arc::new(CurrentAgentResolver::new(CurrentAgentResolverInput {
        fixed: built.providers,
        managed_adapters,
        fixed_probe,
        config: agents.clone(),
        opencode,
        store: Arc::new(crate::agent_install::infrastructure::ManagedRuntimes::new(
            directory
                .parent()
                .expect("conversation root already validated")
                .join("agents"),
        )),
        host,
        credentials,
        provider_directory: root.clone(),
        clock: clock.clone(),
        images: attachments.images.clone(),
        warm_up: CurrentOpenCodeWarmUp::new(records.clone(), warm_up_audit.clone(), clock.clone()),
    }));
    let selected = resolver.default_agent()?;
    // The one registry of how each agent deletes its own record of a session:
    // the fixed agents' bindings, built above, and OpenCode's, whose binding
    // is built from the current generation each time it is asked, as a cold
    // conversation's provider is.
    let mut erasers = built.erasers;
    resolver.register_session_eraser(&mut erasers);
    // The view finds each MCP call's UI in its server's open sessions' lists.
    let tool_uis: Arc<dyn McpToolUis> = match &mcp {
        Some(mcp) => Arc::new(crate::mcp_servers::infrastructure::ListedToolUis(
            mcp.servers.clone(),
        )),
        None => Arc::new(NoMcpToolUis),
    };
    let dependencies = ConversationDependencies {
        agents: ConversationAgents::from_source(configured, selected, resolver.clone())
            .map_err(|error| RunError::Agent(error.to_string()))?,
        storage,
        metadata: metadata.clone(),
        creation_audit,
        mode_audit,
        file_link_audit,
        deletion_audit,
        attachments: Some(attachments.conversations),
        summaries: metadata.clone(),
        listing: metadata.clone(),
        provider_sessions: erasers,
        deletion_budgets: super::agent_budgets::deletion(),
        message_commit_clock: Arc::new(
            nessa_sdk::infrastructure::session_storage::RuntimeMessageCommitClock::new(),
        ),
        clock,
    };
    let workspace = Some(agents.workspace.to_string_lossy().into_owned());
    // With MCP servers, an app's calls go through the conversation's own
    // sessions of them, every step on record in `mcp_app_audit`: the issue
    // by the service, the redemption by the route, every other end of a
    // ticket by `audit_ticket_ends`, which takes the store's events, and
    // every held context's drop by `audit_context_drops`, which takes what
    // the conversations' apps report.
    let (service, resource_route) = match mcp.as_mut() {
        Some(mcp) => {
            let service = ConversationService::with_mcp_apps(
                dependencies,
                ConversationLimits::default(),
                workspace,
                tool_uis,
                mcp_app_ports(mcp, mcp_app_audit.clone()),
            );
            (service, Some((mcp.resource_tickets.clone(), mcp_app_audit)))
        }
        None => (
            ConversationService::with_tool_uis(
                dependencies,
                ConversationLimits::default(),
                workspace,
                tool_uis,
            ),
            None,
        ),
    };
    let service = service.map_err(|error| RunError::Agent(error.to_string()))?;
    if warm_current_opencode {
        warm_ups.push(StartupWarmUp::Current(resolver.clone()));
    }
    Ok(BuiltConversations {
        service,
        receivers,
        metadata,
        attachments: attachments.service,
        agents_catalog,
        record_reader,
        catalogue_reader,
        record_watches,
        catalogue_watches,
        agent_probe: resolver,
        warm_ups,
        mcp,
        resource_route,
    })
}

fn setup_error(error: impl std::fmt::Display) -> RunError {
    RunError::Authentication(error.to_string())
}

#[cfg(unix)]
fn agent_catalog(
    config: &AgentsConfig,
    configured: &HashSet<AgentId>,
    opencode: Option<&OpenCodeProfile>,
) -> Result<AgentsListResult, RunError> {
    let models = load_catalog(
        std::fs::File::open(&config.catalog)
            .map_err(|_| RunError::Agent("cannot read model catalog".into()))?,
    )
    .map_err(|error| RunError::Agent(error.to_string()))?
    .models();
    let agents = AgentId::ALL
        .iter()
        .copied()
        .filter(|agent| configured.contains(agent))
        .map(|agent| {
            let default_model = match agent {
                AgentId::Opencode => opencode
                    .map(OpenCodeProfile::model_id)
                    .ok_or_else(|| RunError::Agent("OpenCode profile missing".into()))?,
                _ => config
                    .runtime(agent)
                    .map(|runtime| runtime.model.as_str())
                    .ok_or_else(|| RunError::Agent("agent runtime missing".into()))?,
            };
            let provider = super::agent::catalog_provider(agent);
            // OpenCode still launches its one validated static profile. Do not
            // advertise catalog entries its current resolver cannot compose.
            let options = models
                .iter()
                .filter(|model| {
                    model.provider == provider
                        && (agent != AgentId::Opencode || model.model_id == default_model)
                })
                .map(|model| {
                    let modes = match agent {
                        AgentId::Claude => ClaudeAcpProvider::approval_modes(&model.model_id),
                        AgentId::Codex => CodexAcpProvider::approval_modes(&model.model_id),
                        AgentId::Opencode => OpencodeAcpProvider::approval_modes(),
                    };
                    AgentModelOption {
                        model_id: model.model_id.clone(),
                        display_name: model.display_name.clone(),
                        max_context_window_tokens: u64::from(model.max_context_window_tokens),
                        reasoning: model.reasoning.is_some(),
                        image_input: model.image_input.is_some(),
                        approval_modes: modes.iter().copied().map(wire_mode_choice).collect(),
                    }
                })
                .collect::<Vec<_>>();
            if !options.iter().any(|model| model.model_id == default_model) {
                return Err(RunError::Agent(format!(
                    "default model {default_model} missing from {provider} catalog"
                )));
            }
            Ok(AgentOption {
                agent: agent.name().into(),
                default_model: default_model.into(),
                models: options,
            })
        })
        .collect::<Result<Vec<_>, RunError>>()?;
    Ok(AgentsListResult { agents })
}

#[cfg(unix)]
fn wire_mode_choice(choice: ApprovalModeChoice) -> WireApprovalModeChoice {
    WireApprovalModeChoice {
        id: match choice.id {
            ApprovalMode::Ask => WireApprovalMode::Ask,
            ApprovalMode::Auto => WireApprovalMode::Auto,
            ApprovalMode::Full => WireApprovalMode::Full,
        },
        name: choice.name.into(),
        description: choice.description.into(),
    }
}

/// Move durable lifecycle writes off Tokio's socket workers. The store serializes
/// mutations; cancellation never rolls back a committed write.
struct LocalAdmin {
    store: Arc<LocalCredentialStore>,
}
impl CredentialAdmin for LocalAdmin {
    fn issue<'a>(
        &'a self,
        request: IssueCredentialRequest,
    ) -> PortFuture<'a, IssueCredentialOutcome, CredentialAdminError> {
        let store = self.store.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || store.issue_sync(request))
                .await
                .map_err(|_| CredentialAdminError::Unavailable)?
                .map_err(CredentialAdminError::from)
        })
    }
    fn list<'a>(
        &'a self,
        request: ListCredentialsRequest,
    ) -> PortFuture<'a, Vec<CredentialMetadataDto>, CredentialAdminError> {
        let store = self.store.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || store.list_sync(&request))
                .await
                .map_err(|_| CredentialAdminError::Unavailable)?
                .map_err(CredentialAdminError::from)
        })
    }
    fn revoke<'a>(
        &'a self,
        request: RevokeCredentialRequest,
    ) -> PortFuture<'a, RevokeCredentialOutcome, CredentialAdminError> {
        let store = self.store.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || store.revoke_sync(request))
                .await
                .map_err(|_| CredentialAdminError::Unavailable)?
                .map_err(CredentialAdminError::from)
        })
    }
}

#[cfg(all(test, unix))]
#[path = "../../tests/composition/local_auth.rs"]
mod tests;
