use crate::gateway::domain::value_objects::{
    ReconciliationAttemptRecord, ReconciliationCorrelation, ReconciliationEvidence,
    ReconciliationIncarnation, ReconciliationIntentRecord, ReconciliationNativeDecision,
    ReconciliationNativeEffect, ReconciliationOutcomeRecord, ReconciliationPhysicalRecord,
    ReconciliationRequestRecord, ReconciliationTarget, SearchPath, SearchPathError,
};
use std::{error::Error, fmt, path::Path};

/// Exact native runtime incarnation established by successful reconciliation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconciledGateway {
    service: String,
    runtime_fingerprint: String,
    runtime_instance: String,
    service_generation: String,
    process_id: u32,
    /// Loopback port this service answers on, decided by its stage. Carried so
    /// a later health probe asks the socket that was registered rather than
    /// re-deriving one that could disagree.
    port: u16,
}
// The identity itself is portable evidence carried by the `GatewayHost`
// contract on every target. Reading its parts is what one native adapter does,
// and macOS is the only host that manages a background service today.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
impl ReconciledGateway {
    pub fn new(
        service: String,
        runtime_fingerprint: String,
        runtime_instance: String,
        service_generation: String,
        process_id: u32,
        port: u16,
    ) -> Self {
        Self {
            service,
            runtime_fingerprint,
            runtime_instance,
            service_generation,
            process_id,
            port,
        }
    }
    pub fn service(&self) -> &str {
        &self.service
    }
    pub fn runtime_fingerprint(&self) -> &str {
        &self.runtime_fingerprint
    }
    pub fn runtime_instance(&self) -> &str {
        &self.runtime_instance
    }
    pub fn service_generation(&self) -> &str {
        &self.service_generation
    }
    pub fn process_id(&self) -> u32 {
        self.process_id
    }
    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn audit_identity(&self) -> Result<ReconciliationIncarnation, GatewayError> {
        let target = ReconciliationTarget::new(
            self.service.clone(),
            self.runtime_fingerprint.clone(),
            self.service_generation.clone(),
        )
        .map_err(|error| GatewayError::Registration(error.to_string()))?;
        ReconciliationIncarnation::new(
            target,
            self.runtime_instance.clone(),
            self.process_id,
            self.port,
        )
        .map_err(|error| GatewayError::Registration(error.to_string()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatewayError {
    Registration(String),
    Audit {
        audit: String,
        physical: Option<Box<GatewayError>>,
    },
    NotReconciled,
    Stop(String),
}
impl fmt::Display for GatewayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Registration(message) | Self::Stop(message) => f.write_str(message),
            Self::Audit { audit, physical } => {
                write!(f, "gateway reconciliation audit failed: {audit}")?;
                if let Some(physical) = physical {
                    write!(f, "; physical reconciliation also failed: {physical}")?;
                }
                Ok(())
            }
            Self::NotReconciled => f.write_str("gateway service has not reconciled"),
        }
    }
}
impl Error for GatewayError {}

/// The managed gateway startup fact currently owned by the desktop host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatewayStartupPhase {
    /// Reconciliation is underway, including a retry after stale live evidence.
    Starting,
    /// The expected runtime identity is currently owned by the registered service.
    Ready,
    /// Reconciliation stopped and why. A later credential load or explicit UI
    /// retry starts one new serialized attempt.
    Failed(GatewayError),
}

/// A revisioned projection of managed gateway startup.
///
/// The revision lets a consumer subscribe before taking a snapshot without an
/// older command response overwriting a newer event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayStartup {
    revision: u64,
    phase: GatewayStartupPhase,
}

impl GatewayStartup {
    pub(crate) fn starting() -> Self {
        Self {
            revision: 0,
            phase: GatewayStartupPhase::Starting,
        }
    }

    pub(super) fn next(&self, phase: GatewayStartupPhase) -> Self {
        Self {
            revision: self.revision.saturating_add(1),
            phase,
        }
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn phase(&self) -> &GatewayStartupPhase {
        &self.phase
    }
}

/// Publishes the host-owned startup projection to interested surfaces.
///
/// This application port is framework-free. A failed UI publication is
/// reported by its adapter and never changes the authoritative startup state.
pub trait GatewayStartupEvents: Send + Sync {
    fn publish(&self, startup: &GatewayStartup);
}

/// Reports that native reconciliation is about to invalidate current readiness.
///
/// Healthy verification never calls this port. A native adapter calls it before
/// it fences, retires, unloads, or replaces the ready process.
pub trait GatewayReconciliationProgress: Send + Sync {
    fn readiness_invalidated(&self);
    fn intent_admitted(&self, intent: GatewayReconciliationIntent) -> Result<(), GatewayError>;
    fn decision_observed(&self, decision: ReconciliationNativeDecision);
    fn effect_observed(&self, effect: ReconciliationNativeEffect);
}

/// One caller's immutable request to the serialized reconciliation owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GatewayReconciliationRequest {
    record: ReconciliationRequestRecord,
}

impl GatewayReconciliationRequest {
    pub fn new(correlation: ReconciliationCorrelation, evidence: ReconciliationEvidence) -> Self {
        Self {
            record: ReconciliationRequestRecord::new(correlation, evidence),
        }
    }

    pub fn correlation(&self) -> &ReconciliationCorrelation {
        self.record.correlation()
    }

    pub fn evidence(&self) -> &ReconciliationEvidence {
        self.record.evidence()
    }

    pub(crate) fn record(&self) -> &ReconciliationRequestRecord {
        &self.record
    }
}

/// One serialized native attempt and the original request that caused it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GatewayReconciliationAttempt {
    record: ReconciliationAttemptRecord,
    origin: GatewayReconciliationRequest,
}

impl GatewayReconciliationAttempt {
    pub fn new(
        correlation: ReconciliationCorrelation,
        origin: GatewayReconciliationRequest,
    ) -> Result<Self, GatewayError> {
        let record = ReconciliationAttemptRecord::new(correlation, origin.record().clone())
            .map_err(|error| GatewayError::Registration(error.to_string()))?;
        Ok(Self { record, origin })
    }

    pub fn correlation(&self) -> &ReconciliationCorrelation {
        self.record.correlation()
    }

    pub fn origin(&self) -> &GatewayReconciliationRequest {
        &self.origin
    }

    pub(crate) fn record(&self) -> &ReconciliationAttemptRecord {
        &self.record
    }
}

/// Allocates correlations without putting randomness in domain or application.
pub trait GatewayReconciliationIds: Send + Sync {
    fn next(&self) -> Result<ReconciliationCorrelation, GatewayError>;
}

/// Durable intent written before a reconciliation effect is admitted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GatewayReconciliationIntent {
    record: ReconciliationIntentRecord,
    attempt: GatewayReconciliationAttempt,
}

impl GatewayReconciliationIntent {
    pub fn new(
        attempt: GatewayReconciliationAttempt,
        target: ReconciliationTarget,
        before: Option<ReconciliationIncarnation>,
    ) -> Result<Self, GatewayError> {
        let record = ReconciliationIntentRecord::new(
            attempt.record().clone(),
            target.clone(),
            before.clone(),
        )
        .map_err(|error| GatewayError::Registration(error.to_string()))?;
        Ok(Self { record, attempt })
    }

    pub fn attempt(&self) -> &GatewayReconciliationAttempt {
        &self.attempt
    }

    pub fn target(&self) -> &ReconciliationTarget {
        self.record.target()
    }

    pub fn before(&self) -> Option<&ReconciliationIncarnation> {
        self.record.before()
    }

    pub(crate) fn record(&self) -> &ReconciliationIntentRecord {
        &self.record
    }
}

/// Physical reconciliation result, kept separate from audit delivery.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GatewayReconciliationEffect {
    Confirmed(ReconciliationIncarnation),
    Refused {
        decisions: Vec<ReconciliationNativeDecision>,
        error: GatewayError,
    },
    Partial {
        decisions: Vec<ReconciliationNativeDecision>,
        effects: Vec<ReconciliationNativeEffect>,
        error: GatewayError,
    },
}

/// Final audit record for one admitted attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GatewayReconciliationOutcome {
    _record: ReconciliationOutcomeRecord,
    intent: GatewayReconciliationIntent,
    effect: GatewayReconciliationEffect,
}

impl GatewayReconciliationOutcome {
    pub fn new(
        intent: GatewayReconciliationIntent,
        effect: GatewayReconciliationEffect,
    ) -> Result<Self, GatewayError> {
        let physical = match &effect {
            GatewayReconciliationEffect::Confirmed(after) => {
                ReconciliationPhysicalRecord::Confirmed(after.clone())
            }
            GatewayReconciliationEffect::Refused { .. } => ReconciliationPhysicalRecord::Refused,
            GatewayReconciliationEffect::Partial { effects, .. } => {
                ReconciliationPhysicalRecord::Partial(effects.clone())
            }
        };
        let decisions = match &effect {
            GatewayReconciliationEffect::Confirmed(_) => Vec::new(),
            GatewayReconciliationEffect::Refused { decisions, .. }
            | GatewayReconciliationEffect::Partial { decisions, .. } => decisions.clone(),
        };
        let record = ReconciliationOutcomeRecord::new(intent.record().clone(), decisions, physical)
            .map_err(|error| GatewayError::Registration(error.to_string()))?;
        Ok(Self {
            _record: record,
            intent,
            effect,
        })
    }

    pub fn attempt(&self) -> &GatewayReconciliationAttempt {
        self.intent.attempt()
    }

    pub fn intent(&self) -> &GatewayReconciliationIntent {
        &self.intent
    }

    pub fn effect(&self) -> &GatewayReconciliationEffect {
        &self.effect
    }
}

/// Durable audit boundary for caller coalescing and native reconciliation.
pub trait GatewayReconciliationAudit: Send + Sync {
    fn intent(&self, intent: &GatewayReconciliationIntent) -> Result<(), GatewayError>;

    fn outcome(&self, outcome: &GatewayReconciliationOutcome) -> Result<(), GatewayError>;

    /// Records a caller whose request joined an already-running attempt.
    fn joined(
        &self,
        attempt: &GatewayReconciliationAttempt,
        joined: &GatewayReconciliationRequest,
    ) -> Result<(), GatewayError>;
}
/// Why the user's login shell did not produce a search path.
///
/// Kept apart from [`GatewayError`]: none of these stop a registration. They
/// are what the fallback to the system path is reported as.
// A host with no login shell to run reports only `Unavailable`; the other two
// are what running one can end in, and the port reads the same on every target.
#[cfg_attr(not(unix), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginShellError {
    /// The shell could not be started, or did not exit successfully.
    Unavailable(String),
    /// The shell was still running when its deadline passed and was stopped. A
    /// login file that waits for input or never returns lands here.
    TimedOut,
    /// The shell answered with something that is not a search path.
    Rejected(SearchPathError),
}
impl fmt::Display for LoginShellError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(message) => write!(f, "login shell unavailable: {message}"),
            Self::TimedOut => f.write_str("login shell did not answer before its deadline"),
            Self::Rejected(error) => {
                write!(f, "login shell answered with an unusable path: {error}")
            }
        }
    }
}
impl Error for LoginShellError {}

/// The `PATH` the user's own login shell would give them.
///
/// The agent is meant to reach the tools the user installed, and the desktop
/// host is the part of Nessa that runs inside that user's session — so it is the
/// part that can ask. A login shell is user-controlled code: the implementation
/// runs it with a bounded deadline and a clean environment, and the value it
/// returns has been through [`SearchPath::parse`].
///
/// Asked at most once per host process, not once per registration: reconciling
/// again is routine — every webview load does it — and a path that changed in
/// between would retire a healthy gateway and stop its agents mid-session. A
/// changed profile takes effect the next time the app is launched.
pub trait LoginShellPath: Send + Sync {
    fn resolve(&self) -> Result<SearchPath, LoginShellError>;
}

/// Reconciliation returns the exact native runtime incarnation only after matching readiness. A stop
/// acknowledges request delivery, not the eventual physical cleanup of each agent.
pub trait GatewayHost: Send + Sync {
    /// Registers the service for `stage`, running the staged `runtime`.
    ///
    /// `agent_path` is the search path resolved for the agent this launch, or
    /// `None` when the login shell could not be read. `None` is not "use the
    /// system path": an adapter that already registered a service keeps the
    /// path that service was registered with, so one slow login shell does not
    /// rewrite the service definition and retire a healthy gateway.
    fn register(
        &self,
        runtime: &Path,
        stage: &str,
        agent_path: Option<&SearchPath>,
        attempt: &GatewayReconciliationAttempt,
        progress: &dyn GatewayReconciliationProgress,
    ) -> Result<ReconciledGateway, GatewayError>;
    fn stop_agents(&self, gateway: &ReconciledGateway) -> Result<(), GatewayError>;
}

/// Substitutes for these ports, beside the ports themselves, so every module
/// that bootstraps a [`Gateway`](super::Gateway) in a test uses the same ones.
///
/// Inline rather than in a file of its own, like `settings::testing`: an item
/// at the top level of a `#[cfg(test)]` file reads to
/// `scripts/desktop/platform-gates.mjs` as something every platform compiles
/// and only macOS reaches, and on Windows — where that script's module walk
/// finds no children to carry the gate to — it says so.
#[cfg(test)]
pub(crate) mod testing {
    use super::{
        GatewayError, GatewayReconciliationAttempt, GatewayReconciliationAudit,
        GatewayReconciliationIds, GatewayReconciliationIntent, GatewayReconciliationOutcome,
        GatewayReconciliationRequest, GatewayStartup, GatewayStartupEvents, LoginShellError,
        LoginShellPath, ReconciliationCorrelation, SearchPath,
    };
    use std::sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    };

    /// A login shell with a fixed answer — the path it reports, or the reason
    /// it reported none.
    pub(crate) struct FixedLoginShell(pub(crate) Result<SearchPath, LoginShellError>);
    impl LoginShellPath for FixedLoginShell {
        fn resolve(&self) -> Result<SearchPath, LoginShellError> {
            self.0.clone()
        }
    }

    /// A login shell that answers with the system path: enough for a test whose
    /// subject is something else.
    pub(crate) fn system_login_shell() -> Arc<dyn LoginShellPath> {
        Arc::new(FixedLoginShell(Ok(SearchPath::system())))
    }

    pub(crate) struct DiscardStartupEvents;

    impl GatewayStartupEvents for DiscardStartupEvents {
        fn publish(&self, _startup: &GatewayStartup) {}
    }

    pub(crate) fn discard_startup_events() -> Arc<dyn GatewayStartupEvents> {
        Arc::new(DiscardStartupEvents)
    }

    #[derive(Default)]
    pub(crate) struct SequentialReconciliationIds(AtomicU64);

    impl GatewayReconciliationIds for SequentialReconciliationIds {
        fn next(&self) -> Result<ReconciliationCorrelation, GatewayError> {
            let next = self.0.fetch_add(1, Ordering::Relaxed) + 1;
            ReconciliationCorrelation::parse(format!("00000000-0000-4000-8000-{next:012x}"))
                .map_err(|error| GatewayError::Registration(error.to_string()))
        }
    }

    pub(crate) fn sequential_reconciliation_ids() -> Arc<dyn GatewayReconciliationIds> {
        Arc::new(SequentialReconciliationIds::default())
    }

    pub(crate) struct DiscardReconciliationAudit;

    impl GatewayReconciliationAudit for DiscardReconciliationAudit {
        fn intent(&self, _: &GatewayReconciliationIntent) -> Result<(), GatewayError> {
            Ok(())
        }

        fn outcome(&self, _: &GatewayReconciliationOutcome) -> Result<(), GatewayError> {
            Ok(())
        }

        fn joined(
            &self,
            _: &GatewayReconciliationAttempt,
            _: &GatewayReconciliationRequest,
        ) -> Result<(), GatewayError> {
            Ok(())
        }
    }

    pub(crate) fn discard_reconciliation_audit() -> Arc<dyn GatewayReconciliationAudit> {
        Arc::new(DiscardReconciliationAudit)
    }
}
