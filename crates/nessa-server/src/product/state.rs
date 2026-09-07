use nessa_auth::{
    application::credential_admin::CredentialAdmin,
    application::ports::{AccessReader, Clock, CredentialVerifier, PolicyEvaluator},
    domain::{AudienceId, Resource, ResourceId},
};
use std::sync::Arc;

/// Dependencies and trusted gateway selectors for the product route.
///
/// This state is constructed only in composition.
#[derive(Clone)]
pub struct ProductRouteState {
    pub(crate) settings: SessionSettings,
    pub(crate) gateway: Resource,
    pub(crate) audience: AudienceId,
    pub(crate) verifier: Arc<dyn CredentialVerifier>,
    pub(crate) access: Arc<dyn AccessReader>,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) policy: Arc<dyn PolicyEvaluator>,
    pub(crate) admin: Option<Arc<dyn CredentialAdmin>>,
    pub(crate) uptime_clock: Arc<dyn crate::app::ports::Clock>,
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
    pub uptime_clock: Arc<dyn crate::app::ports::Clock>,
}

impl ProductRouteState {
    /// Bind trusted gateway ownership and audience to an isolated dependency scope.
    pub fn new(
        gateway_id: ResourceId,
        gateway_organization_id: nessa_auth::domain::OrganizationId,
        audience: AudienceId,
        dependencies: ProductDependencies,
    ) -> Self {
        Self {
            settings: SessionSettings::default(),
            gateway: Resource::new(gateway_organization_id, gateway_id),
            audience,
            verifier: dependencies.verifier,
            access: dependencies.access,
            clock: dependencies.clock,
            policy: dependencies.policy,
            admin: None,
            uptime_clock: dependencies.uptime_clock,
        }
    }

    pub fn with_settings(mut self, settings: SessionSettings) -> Self {
        self.settings = settings;
        self
    }

    /// Register credential lifecycle operations; missing administration fails closed.
    pub fn with_admin(mut self, admin: Arc<dyn CredentialAdmin>) -> Self {
        self.admin = Some(admin);
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
#[derive(Clone, Copy, Debug)]
pub struct SessionSettings {
    pub handshake_timeout: std::time::Duration,
    pub write_timeout: std::time::Duration,
    pub current_state_interval: std::time::Duration,
}
impl Default for SessionSettings {
    fn default() -> Self {
        Self {
            handshake_timeout: std::time::Duration::from_secs(10),
            write_timeout: std::time::Duration::from_secs(5),
            current_state_interval: std::time::Duration::from_secs(1),
        }
    }
}
