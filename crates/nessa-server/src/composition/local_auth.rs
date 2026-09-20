//! Local product dependency factory. Provider choices stay outside route handlers.
use super::agent::AgentsConfig;
use crate::{
    agents::{
        domain::AgentId,
        infrastructure::{AgentLaunchFiles, LocalAgentProbe},
    },
    app::ports::Clock as ServerClock,
    attachments::{application::AttachmentService, infrastructure::ModelImageNormalizer},
    browser_session::adapters::PersistentSessions,
    conversation::{
        application::{
            ConversationAgents, ConversationDependencies, ConversationLimits, ConversationService,
        },
        infrastructure::{DurableConversationCreationAudit, LocalConversationRepository},
    },
    core::RunError,
    env::Environment,
    product::{ProductDependencies, ProductRouteState},
};
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
use nessa_sdk::infrastructure::session_storage::LocalFileStorage;
use std::{
    collections::{HashMap, HashSet},
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

/// Construct the guarded product route from a previously initialized local registry.
pub(super) fn product_state(
    config: &Environment,
    uptime: Arc<dyn ServerClock>,
    bundle: Option<&Path>,
) -> Result<ProductRouteState, RunError> {
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
    let store = Arc::new(
        LocalCredentialStore::open_with_config(directory, "credentials.v1.json", settings.registry)
            .map_err(RunError::Registry)?,
    );
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
    // Built here, before the launch files below, because building it is how
    // this server finds out which configured agents cannot be started at all,
    // and that answer belongs in what setup is told. Nothing else between here
    // and its use depends on the order.
    let (conversations, unavailable) = match &settings.agents {
        Some(agents) => {
            let (service, attachments, unavailable) = conversations(agents, directory)?;
            (Some((service, attachments)), unavailable)
        }
        None => (None, HashSet::new()),
    };
    let agent_launch_files = launch_files(settings.agents.as_ref(), &unavailable);
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
            agent_probe: Arc::new(LocalAgentProbe::from_environment(agent_launch_files)),
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
    Ok(product)
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
                            command: runtime.command.clone(),
                            paths: runtime.paths(),
                            environment: super::agent::launch_environment(id),
                        },
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The conversation service and the attachment service beside it, and which
/// configured agents this run cannot start even though they are installed.
///
/// That last part is the reason this is a function rather than the tail of
/// [`product_state`]: it has to be known before the readiness probe is built,
/// and it is only known once every provider has been built. See
/// [`super::agent::ConfiguredAgents`].
fn conversations(
    agents: &AgentsConfig,
    directory: &Path,
) -> Result<(ConversationService, AttachmentService, HashSet<AgentId>), RunError> {
    let namespace = directory
        .parent()
        .ok_or_else(|| RunError::Agent("invalid namespace directory".into()))?;
    let root = namespace.join("conversations");
    nessa_local_storage::create_directory(&root)
        .map_err(|error| RunError::Agent(error.to_string()))?;
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let selected = agents.selected()?;
    // Ownership records come first. A binding needs somewhere to read image
    // bytes, reading them needs the attachment store, and beginning an upload
    // needs to ask who owns a conversation: so the repository is built, then
    // attachments over it, and only then the providers.
    let metadata = Arc::new(
        LocalConversationRepository::new(root.join("metadata"))
            .map_err(|error| RunError::Agent(error.to_string()))?,
    );
    let attachments = super::attachments::attachments(
        &namespace.join("attachments"),
        metadata.clone(),
        // The image limits from the catalog, which is the one place they are
        // recorded, taken across every configured agent's model rather than the
        // selected one's: the store is shared by conversations that each run on
        // their own agent. Every uploaded image is fitted to them, using the
        // running system's decoder for the encodings the image library does not
        // read itself.
        Arc::new(
            ModelImageNormalizer::new(
                super::agent::image_limits(agents)?.as_ref(),
                nessa_images::platform_decoder(),
            )
            .map_err(|error| RunError::Agent(format!("model image limits: {error}")))?,
        ),
        clock.clone(),
    )?;
    let built = super::agent::providers(agents, &root, clock.clone(), attachments.images.clone())?;
    let storage = Arc::new(
        LocalFileStorage::new(root.join("sessions"))
            .map_err(|error| RunError::Agent(error.to_string()))?,
    );
    let creation_audit = Arc::new(
        DurableConversationCreationAudit::new(root.join("audit").join("creation"))
            .map_err(|error| RunError::Agent(error.to_string()))?,
    );
    let service = ConversationService::new(
        ConversationDependencies {
            agents: ConversationAgents::new(built.providers, selected)
                .map_err(|error| RunError::Agent(error.to_string()))?,
            storage,
            metadata,
            creation_audit,
            attachments: Some(attachments.conversations),
            clock,
        },
        ConversationLimits::default(),
        Some(agents.workspace.to_string_lossy().into_owned()),
    )
    .map_err(|error| RunError::Agent(error.to_string()))?;
    Ok((service, attachments.service, built.unavailable))
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

#[cfg(test)]
#[path = "../../tests/composition/local_auth.rs"]
mod tests;
