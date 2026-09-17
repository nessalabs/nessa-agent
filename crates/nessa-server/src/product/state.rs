use crate::agents::application::{AgentProbe, SharedAgentReadiness};
use crate::conversation::application::ConversationService;
use axum::extract::FromRef;
use nessa_auth::{
    application::{
        credential_admin::CredentialAdmin,
        ports::{AccessReader, Clock, CredentialVerifier, PolicyEvaluator},
    },
    domain::{AudienceId, Resource, ResourceId},
};
use std::sync::Arc;
use tokio::sync::Semaphore;

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
    pub(crate) settings: SessionSettings,
    pub(crate) gateway: Resource,
    pub(crate) audience: AudienceId,
    pub(crate) verifier: Arc<dyn CredentialVerifier>,
    pub(crate) access: Arc<dyn AccessReader>,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) policy: Arc<dyn PolicyEvaluator>,
    pub(crate) conversations: Option<Arc<ConversationService>>,
    pub(crate) admin: Option<Arc<dyn CredentialAdmin>>,
    pub(crate) uptime_clock: Arc<dyn crate::app::ports::Clock>,
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
    /// Asks this host which agents could start here. Chosen in composition so
    /// no route handler constructs a machine probe of its own. How often it may
    /// be asked is this state's to decide, not composition's — see
    /// [`SharedAgentReadiness`].
    pub agent_probe: Arc<dyn AgentProbe>,
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
            browser_sessions: None,
            browser_session_id: None,
            browser_session_origin: None,
            browser_http_allowed: false,
            settings: SessionSettings::default(),
            requests: Arc::new(Semaphore::new(128)),
            controls: Arc::new(Semaphore::new(32)),
            gateway: Resource::new(gateway_organization_id, gateway_id),
            audience,
            verifier: dependencies.verifier,
            access: dependencies.access,
            clock: dependencies.clock,
            policy: dependencies.policy,
            admin: None,
            conversations: None,
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

    /// Register credential lifecycle operations; missing administration fails closed.
    pub fn with_admin(mut self, admin: Arc<dyn CredentialAdmin>) -> Self {
        self.admin = Some(admin);
        self
    }

    /// Share server-owned Agents across authenticated sockets. No socket owns cleanup.
    pub fn with_conversations(mut self, service: Arc<ConversationService>) -> Self {
        self.conversations = Some(service);
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
