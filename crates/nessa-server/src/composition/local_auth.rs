//! Local product dependency factory. Provider choices stay outside route handlers.
use super::agent::AgentsConfig;
#[cfg(unix)]
use super::current_agent::{CurrentAgentResolver, CurrentAgentResolverInput};
#[cfg(unix)]
use super::opencode_profile::EffectiveOpenCodeProfile;
#[cfg(unix)]
use super::warm_up::{CurrentOpenCodeWarmUp, PreparedRuntime};
#[cfg(unix)]
use crate::{
    agent_warm_up::application::AgentWarmUp,
    agent_warm_up::{
        domain::RuntimeFingerprint,
        infrastructure::{DurableWarmUpAudit, FileWarmUpRecords},
    },
    agents::{domain::AgentId, infrastructure::AgentLaunchFiles},
    attachments::infrastructure::ModelImageNormalizer,
    conversation::application::{ConversationAgents, ConversationDependencies, ConversationLimits},
    conversation::infrastructure::{
        DurableConversationCreationAudit, DurableConversationDeletionAudit,
        DurableConversationFileLinkAudit, LocalConversationStore,
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
    conversation::application::ConversationService,
    core::RunError,
    env::Environment,
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
    domain::{AudienceId, OrganizationId, ResourceId},
};
#[cfg(unix)]
use nessa_sdk::infrastructure::session_storage::{InMemoryStorage, LocalFileStorage};
#[cfg(unix)]
use std::collections::HashSet;
use std::{
    collections::HashMap,
    path::Path,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

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
    /// Startup preparations for fixed providers plus the current OpenCode
    /// resolver when configured. Empty when no agent is configured.
    pub(super) warm_ups: Vec<StartupWarmUp>,
}

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
}

/// Construct the guarded product route from a previously initialized local registry.
pub(super) fn product_state(
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
    let gateway = ResourceId::new(identity.gateway_id.clone()).map_err(setup_error)?;
    let audience = AudienceId::new(identity.gateway_id).map_err(setup_error)?;
    let organization =
        OrganizationId::new(identity.organization_ids[0].clone()).map_err(setup_error)?;
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
    let managed_opencode = bundle.is_some();
    let (conversations, agent_probe, warm_ups) = match &settings.agents {
        Some(agents) => {
            let built = conversations(
                agents,
                directory,
                agent_credentials.clone(),
                managed_opencode,
            )?;
            (
                Some((built.service, built.attachments)),
                built.agent_probe,
                built.warm_ups,
            )
        }
        None => (
            None,
            Arc::new(LocalAgentProbe::from_environment(
                HashMap::new(),
                agent_credentials.clone(),
            )) as Arc<dyn AgentProbe>,
            Vec::new(),
        ),
    };
    let policy = Arc::new(CedarPolicyEvaluator::new().map_err(setup_error)?);
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
    .with_browser_sessions(Arc::new(
        PersistentSessions::open(
            &directory.join("browser-sessions.jsonl"),
            SystemClock.unix_seconds(),
        )
        .map_err(setup_error)?,
    ));
    product.browser_http_allowed = config.browser_http_allowed();
    if let Some((service, attachments)) = conversations {
        product = product
            .with_conversations(Arc::new(service))
            .with_attachments(attachments);
    }
    Ok(LocalProduct {
        routes: product,
        warm_ups,
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
    attachments: AttachmentService,
    /// Which configured agents this run cannot start even though they are
    /// installed. Known only once every provider has been built, and needed
    /// before the readiness probe is, which is why this is a function rather
    /// than the tail of [`product_state`]. See
    /// [`super::agent::ConfiguredAgents`].
    agent_probe: Arc<dyn AgentProbe>,
    /// Fixed-provider preparations plus the current OpenCode resolver. Started
    /// by the server lifecycle once the gateway is listening, not here.
    warm_ups: Vec<StartupWarmUp>,
}

/// Refuse to start conversations while metadata an earlier build kept as
/// files is still on disk: this build reads only the database, and starting
/// beside the files would show nobody their conversations
/// (`metadata_an_earlier_build_kept_as_files_is_refused_with_what_moves_it`).
#[cfg_attr(not(unix), allow(dead_code))]
fn refuse_metadata_files(root: &Path) -> Result<(), RunError> {
    for name in ["metadata", "summaries"] {
        let directory = root.join(name);
        match std::fs::symlink_metadata(&directory) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(RunError::Agent(format!(
                    "could not tell whether {} holds conversation metadata from an earlier build: {error}",
                    directory.display()
                )))
            }
            Ok(_) => {
                return Err(RunError::Agent(format!(
                    "conversation metadata from an earlier build is still at {}; stop the gateway and run `node scripts/move-conversation-metadata.mjs` to move it into the database",
                    directory.display()
                )))
            }
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn conversations(
    _agents: &AgentsConfig,
    _directory: &Path,
    _credentials: Arc<dyn AgentCredentialSource>,
    _managed_opencode: bool,
) -> Result<BuiltConversations, RunError> {
    Err(RunError::Agent(
        "ACP agents require Unix process supervision".into(),
    ))
}

#[cfg(unix)]
fn conversations(
    agents: &AgentsConfig,
    directory: &Path,
    credentials: Arc<dyn AgentCredentialSource>,
    managed_opencode: bool,
) -> Result<BuiltConversations, RunError> {
    let mut warm_ups = Vec::new();
    let root = directory
        .parent()
        .ok_or_else(|| RunError::Agent("invalid namespace directory".into()))?
        .join("conversations");
    nessa_local_storage::create_directory(&root)
        .map_err(|error| RunError::Agent(error.to_string()))?;
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let host = crate::agent_install::infrastructure::host_platform();
    let opencode = EffectiveOpenCodeProfile::decide(
        agents,
        managed_opencode,
        &host,
        (!managed_opencode)
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
    // Ownership, tombstones and summaries are one database. Metadata an
    // earlier build kept as files is refused rather than read in its old
    // shape, and the refusal names what moves it (docs/adr/todo/196-conversation-metadata-database.md).
    refuse_metadata_files(&root)?;
    let metadata = Arc::new(
        LocalConversationStore::open(&root.join("metadata.sqlite3"))
            .map_err(|error| RunError::Agent(error.to_string()))?,
    );
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
    let deferred = if opencode.configured().is_some() {
        HashSet::from([AgentId::Opencode])
    } else {
        HashSet::new()
    };
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
            Arc::new(InMemoryStorage::new()),
            records.clone(),
            warm_up_audit.clone(),
            clock.clone(),
            runtime,
        );
        agent.readiness = Some(Arc::new(PreparedRuntime(prepared.clone())));
        warm_ups.push(StartupWarmUp::Fixed(prepared));
    }
    let storage = Arc::new(
        LocalFileStorage::new(root.join("sessions"))
            .map_err(|error| RunError::Agent(error.to_string()))?,
    );
    let creation_audit = Arc::new(
        DurableConversationCreationAudit::new(root.join("audit").join("creation"))
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
    let resolver = Arc::new(CurrentAgentResolver::new(CurrentAgentResolverInput {
        fixed: built.providers,
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
    let configured = resolver.configured();
    let selected = resolver.default_agent()?;
    // The one registry of how each agent deletes its own record of a session:
    // the fixed agents' bindings, built above, and OpenCode's, whose binding
    // is built from the current generation each time it is asked, as a cold
    // conversation's provider is.
    let mut erasers = built.erasers;
    resolver.register_session_eraser(&mut erasers);
    let service = ConversationService::new(
        ConversationDependencies {
            agents: ConversationAgents::from_source(configured, selected, resolver.clone())
                .map_err(|error| RunError::Agent(error.to_string()))?,
            storage,
            metadata: metadata.clone(),
            creation_audit,
            file_link_audit,
            deletion_audit,
            attachments: Some(attachments.conversations),
            summaries: metadata.clone(),
            listing: metadata,
            provider_sessions: erasers,
            deletion_budgets: super::agent_budgets::deletion(),
            clock,
        },
        ConversationLimits::default(),
        Some(agents.workspace.to_string_lossy().into_owned()),
    )
    .map_err(|error| RunError::Agent(error.to_string()))?;
    if warm_current_opencode {
        warm_ups.push(StartupWarmUp::Current(resolver.clone()));
    }
    Ok(BuiltConversations {
        service,
        attachments: attachments.service,
        agent_probe: resolver,
        warm_ups,
    })
}

fn setup_error(error: impl std::fmt::Display) -> RunError {
    RunError::Authentication(error.to_string())
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
