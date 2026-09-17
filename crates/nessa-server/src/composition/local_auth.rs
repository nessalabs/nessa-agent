//! Local product dependency factory. Provider choices stay outside route handlers.
use crate::{
    agents::infrastructure::LocalAgentProbe,
    app::ports::Clock as ServerClock,
    browser_session::adapters::PersistentSessions,
    conversation::{
        application::{ConversationLimits, ConversationService},
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
            ListCredentialsRequest, RevokeCredentialRequest,
        },
        dto::CredentialMetadataDto,
        ports::{Clock, PortFuture},
    },
    domain::{AudienceId, OrganizationId, ResourceId},
};
use nessa_sdk::infrastructure::session_storage::LocalFileStorage;
use std::{
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
    bundle: Option<&std::path::Path>,
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
            .map_err(setup_error)?,
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
            agent_probe: Arc::new(LocalAgentProbe::from_environment()),
        },
    )
    .with_admin(admin)
    .with_settings(settings.session()?)
    .with_browser_sessions(Arc::new(
        PersistentSessions::open(&directory.join("browser-sessions.jsonl")).map_err(setup_error)?,
    ));
    product.browser_http_allowed = config.browser_http_allowed();
    if let Some(agent) = &settings.agent {
        let root = directory
            .parent()
            .ok_or_else(|| RunError::Agent("invalid namespace directory".into()))?
            .join("conversations");
        nessa_local_storage::create_directory(&root)
            .map_err(|error| RunError::Agent(error.to_string()))?;
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        let provider = super::agent::provider(agent, &root, clock.clone())?;
        let storage = Arc::new(
            LocalFileStorage::new(root.join("sessions"))
                .map_err(|error| RunError::Agent(error.to_string()))?,
        );
        let metadata = Arc::new(
            LocalConversationRepository::new(root.join("metadata"))
                .map_err(|error| RunError::Agent(error.to_string()))?,
        );
        let creation_audit = Arc::new(
            DurableConversationCreationAudit::new(root.join("audit").join("creation"))
                .map_err(|error| RunError::Agent(error.to_string()))?,
        );
        let service = ConversationService::new(
            provider,
            storage,
            metadata,
            creation_audit,
            clock,
            ConversationLimits {
                reserved_output_tokens: agent.output_tokens,
                ..ConversationLimits::default()
            },
            Some(agent.workspace.to_string_lossy().into_owned()),
        )
        .map_err(|error| RunError::Agent(error.to_string()))?;
        product = product.with_conversations(Arc::new(service));
    }
    Ok(product)
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
    ) -> PortFuture<'a, u64, CredentialAdminError> {
        let store = self.store.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || store.revoke_sync(request))
                .await
                .map_err(|_| CredentialAdminError::Unavailable)?
                .map_err(CredentialAdminError::from)
        })
    }
}
