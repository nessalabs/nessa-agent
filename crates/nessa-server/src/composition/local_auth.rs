//! Local product dependency factory. Provider choices stay outside route handlers.
use crate::{
    app::ports::Clock as ServerClock,
    core::RunError,
    env::Environment,
    product::{ProductDependencies, ProductRouteState},
};
use nessa_auth::{
    adapters::{cedar::CedarPolicyEvaluator, local::LocalCredentialStore},
    application::{
        credential_admin::{
            CredentialAdmin, IssueCredentialOutcome, IssueCredentialRequest,
            ListCredentialsRequest, RevokeCredentialRequest,
        },
        dto::CredentialMetadataDto,
        ports::{AccessError, Clock, PortFuture},
    },
    domain::{AudienceId, OrganizationId, ResourceId},
};
use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

/// Wall time for authentication. Health's monotonic uptime remains a separate port.
pub(super) struct SystemClock;
impl Clock for SystemClock {
    fn unix_seconds(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs())
    }
}

/// Construct the guarded product route from a previously initialized local registry.
pub(super) fn product_state(
    config: &Environment,
    uptime: Arc<dyn ServerClock>,
) -> Result<ProductRouteState, RunError> {
    let directory = config
        .auth_directory
        .as_ref()
        .ok_or_else(|| RunError::Authentication("set NESSA_DATA_DIR or HOME".into()))?;
    let settings = super::runtime_config::RuntimeConfig::load(directory)?;
    let store = Arc::new(
        LocalCredentialStore::open_with_config(
            directory.join("credentials.v1.json"),
            settings.registry,
        )
        .map_err(setup_error)?,
    );
    let identity = store.identity().map_err(|_| {
        RunError::Authentication(
            "initialize local access with `nessa-server auth init --owner-token-file <new-path>`"
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
    Ok(ProductRouteState::new(
        gateway,
        organization,
        audience,
        ProductDependencies {
            verifier: store.clone(),
            access: store,
            clock: Arc::new(SystemClock),
            policy,
            uptime_clock: uptime,
        },
    )
    .with_admin(admin)
    .with_settings(settings.session()?))
}

fn setup_error(error: impl std::fmt::Display) -> RunError {
    RunError::Authentication(error.to_string())
}

/// Move durable lifecycle writes off Tokio's socket workers. The product admission
/// gate bounds concurrent calls; cancellation never rolls back a committed write.
struct LocalAdmin {
    store: Arc<LocalCredentialStore>,
}
impl CredentialAdmin for LocalAdmin {
    fn issue<'a>(
        &'a self,
        request: IssueCredentialRequest,
    ) -> PortFuture<'a, IssueCredentialOutcome> {
        let store = self.store.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || store.issue_sync(request))
                .await
                .map_err(|_| AccessError::Unavailable)?
                .map_err(|_| AccessError::Unavailable)
        })
    }
    fn list<'a>(
        &'a self,
        request: ListCredentialsRequest,
    ) -> PortFuture<'a, Vec<CredentialMetadataDto>> {
        let store = self.store.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || store.list_sync(&request))
                .await
                .map_err(|_| AccessError::Unavailable)?
                .map_err(|_| AccessError::Unavailable)
        })
    }
    fn revoke<'a>(&'a self, request: RevokeCredentialRequest) -> PortFuture<'a, u64> {
        let store = self.store.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || store.revoke_sync(request))
                .await
                .map_err(|_| AccessError::Unavailable)?
                .map_err(|_| AccessError::Unavailable)
        })
    }
}
