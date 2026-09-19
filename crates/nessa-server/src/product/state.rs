use crate::agents::application::{AgentProbe, SharedAgentReadiness};
use crate::conversation::application::ConversationService;
use axum::extract::FromRef;
use nessa_auth::{
    application::{
        credential_admin::CredentialAdmin,
        ports::{AccessReader, Clock, CredentialVerifier, PolicyEvaluator},
    },
    domain::{AudienceId, OrganizationId, Resource, ResourceId},
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
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
        gateway_organization_id: OrganizationId,
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
