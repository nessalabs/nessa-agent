use crate::gateway::domain::value_objects::{
    ReconciliationAttemptRecord, ReconciliationCleanupDecision, ReconciliationCorrelation,
    ReconciliationEffectTimingRecord, ReconciliationEvidence, ReconciliationHistory,
    ReconciliationHistoryFact, ReconciliationIncarnation, ReconciliationIntentDeliveryRecord,
    ReconciliationIntentRecord, ReconciliationOutcomeDisposition, ReconciliationOutcomeRecord,
    ReconciliationPhysicalRecord, ReconciliationRejectedReport, ReconciliationRequestRecord,
    ReconciliationTarget, ReconciliationValidationFacts, SearchPath, SearchPathError,
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

/// Reports readiness invalidation and the ordered native history for one attempt.
///
/// Healthy verification never calls this port. A native adapter calls it before
/// it fences, retires, unloads, or replaces the ready process.
pub trait GatewayReconciliationProgress: Send + Sync {
    #[cfg_attr(
        all(not(target_os = "macos"), not(test)),
        expect(dead_code, reason = "native reconciliation is supported only on macOS")
    )]
    fn readiness_invalidated(&self);
    #[cfg_attr(
        all(not(target_os = "macos"), not(test)),
        expect(dead_code, reason = "native reconciliation is supported only on macOS")
    )]
    fn intent_admitted(&self, intent: GatewayReconciliationIntent) -> Result<(), GatewayError>;
    #[cfg_attr(
        all(not(target_os = "macos"), not(test)),
        expect(dead_code, reason = "native reconciliation is supported only on macOS")
    )]
    fn history_observed(&self, fact: ReconciliationHistoryFact);
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

    #[cfg_attr(
        all(not(target_os = "macos"), not(test)),
        expect(dead_code, reason = "native reconciliation is supported only on macOS")
    )]
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

    #[cfg_attr(
        all(not(target_os = "macos"), not(test)),
        expect(dead_code, reason = "native reconciliation is supported only on macOS")
    )]
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

/// Immutable intent reserved before delivery decides whether effects may proceed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GatewayReconciliationIntent {
    record: ReconciliationIntentRecord,
    attempt: GatewayReconciliationAttempt,
}

impl GatewayReconciliationIntent {
    #[cfg_attr(
        all(not(target_os = "macos"), not(test)),
        expect(dead_code, reason = "native reconciliation is supported only on macOS")
    )]
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

    #[cfg_attr(
        all(not(target_os = "macos"), not(test)),
        expect(dead_code, reason = "native reconciliation is supported only on macOS")
    )]
    pub fn attempt(&self) -> &GatewayReconciliationAttempt {
        &self.attempt
    }

    #[cfg_attr(
        all(not(target_os = "macos"), not(test)),
        expect(dead_code, reason = "native reconciliation is supported only on macOS")
    )]
    pub fn target(&self) -> &ReconciliationTarget {
        self.record.target()
    }

    #[cfg_attr(
        not(target_os = "macos"),
        expect(dead_code, reason = "native reconciliation is supported only on macOS")
    )]
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
    Confirmed {
        after: ReconciliationIncarnation,
        history: ReconciliationHistory,
    },
    Failed {
        history: ReconciliationHistory,
        error: GatewayError,
    },
    RejectedReport {
        trusted_history: ReconciliationHistory,
        report: ReconciliationRejectedReport,
        error: GatewayError,
    },
}

/// Delivery state of the immutable intent record reserved before native effects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GatewayReconciliationIntentDelivery {
    Reserved,
    Acknowledged,
    Failed(GatewayError),
}

impl GatewayReconciliationIntentDelivery {
    fn domain_record(&self) -> ReconciliationIntentDeliveryRecord {
        match self {
            Self::Reserved => ReconciliationIntentDeliveryRecord::Reserved,
            Self::Acknowledged => ReconciliationIntentDeliveryRecord::Acknowledged,
            Self::Failed(_) => ReconciliationIntentDeliveryRecord::Failed,
        }
    }
}

/// Earliest native effect timing relative to immutable intent acknowledgement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GatewayReconciliationEffectTiming {
    NoEffectsObserved,
    BeforeIntentReservation,
    BeforeIntentAcknowledgement,
    AfterIntentAcknowledgement,
}

impl GatewayReconciliationEffectTiming {
    fn domain_record(self) -> ReconciliationEffectTimingRecord {
        match self {
            Self::NoEffectsObserved => ReconciliationEffectTimingRecord::NoEffectsObserved,
            Self::BeforeIntentReservation => {
                ReconciliationEffectTimingRecord::BeforeIntentReservation
            }
            Self::BeforeIntentAcknowledgement => {
                ReconciliationEffectTimingRecord::BeforeIntentAcknowledgement
            }
            Self::AfterIntentAcknowledgement => {
                ReconciliationEffectTimingRecord::AfterIntentAcknowledgement
            }
        }
    }
}

/// Final audit record for one attempt that reserved immutable intent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GatewayReconciliationOutcome {
    record: ReconciliationOutcomeRecord,
    intent: GatewayReconciliationIntent,
    intent_delivery: GatewayReconciliationIntentDelivery,
    effect_timing: GatewayReconciliationEffectTiming,
    effect: GatewayReconciliationEffect,
}

impl GatewayReconciliationOutcome {
    pub fn assess(
        intent: GatewayReconciliationIntent,
        intent_delivery: GatewayReconciliationIntentDelivery,
        effect_timing: GatewayReconciliationEffectTiming,
        facts: Vec<ReconciliationHistoryFact>,
        physical: Result<ReconciliationIncarnation, GatewayError>,
    ) -> Self {
        let physical_record = match &physical {
            Ok(after) => ReconciliationPhysicalRecord::Confirmed(after.clone()),
            Err(_) => ReconciliationPhysicalRecord::Failed,
        };
        let record = ReconciliationOutcomeRecord::assess(
            intent.record().clone(),
            intent_delivery.domain_record(),
            effect_timing.domain_record(),
            facts,
            physical_record,
        );
        let effect = match (record.disposition(), physical) {
            (
                ReconciliationOutcomeDisposition::Accepted(
                    ReconciliationPhysicalRecord::Confirmed(after),
                ),
                Ok(_),
            ) => GatewayReconciliationEffect::Confirmed {
                after: after.clone(),
                history: record.history().clone(),
            },
            (
                ReconciliationOutcomeDisposition::Accepted(ReconciliationPhysicalRecord::Failed),
                Err(error),
            ) => GatewayReconciliationEffect::Failed {
                history: record.history().clone(),
                error,
            },
            (ReconciliationOutcomeDisposition::Rejected(report), physical) => {
                GatewayReconciliationEffect::RejectedReport {
                    trusted_history: record.history().clone(),
                    report: report.clone(),
                    error: physical.err().unwrap_or_else(|| match &intent_delivery {
                        GatewayReconciliationIntentDelivery::Failed(error) => error.clone(),
                        GatewayReconciliationIntentDelivery::Reserved => {
                            GatewayError::Registration(
                                "gateway host reported success before intent delivery completed"
                                    .into(),
                            )
                        }
                        GatewayReconciliationIntentDelivery::Acknowledged => {
                            GatewayError::Registration(
                                "gateway host returned a contradictory success report".into(),
                            )
                        }
                    }),
                }
            }
            _ => unreachable!("domain assessment preserves the supplied physical report"),
        };
        Self {
            record,
            intent,
            intent_delivery,
            effect_timing,
            effect,
        }
    }

    #[cfg_attr(
        not(target_os = "macos"),
        expect(dead_code, reason = "native reconciliation is supported only on macOS")
    )]
    pub fn attempt(&self) -> &GatewayReconciliationAttempt {
        self.intent.attempt()
    }

    #[cfg_attr(
        all(not(target_os = "macos"), not(test)),
        expect(dead_code, reason = "native reconciliation is supported only on macOS")
    )]
    pub fn intent(&self) -> &GatewayReconciliationIntent {
        &self.intent
    }

    #[cfg_attr(
        all(not(target_os = "macos"), not(test)),
        expect(dead_code, reason = "native reconciliation is supported only on macOS")
    )]
    pub fn intent_delivery(&self) -> &GatewayReconciliationIntentDelivery {
        &self.intent_delivery
    }

    #[cfg_attr(
        all(not(target_os = "macos"), not(test)),
        expect(dead_code, reason = "native reconciliation is supported only on macOS")
    )]
    pub fn effect_timing(&self) -> GatewayReconciliationEffectTiming {
        self.effect_timing
    }

    pub fn cleanup(&self) -> ReconciliationCleanupDecision {
        self.record.cleanup()
    }

    #[cfg_attr(
        all(not(target_os = "macos"), not(test)),
        expect(dead_code, reason = "native reconciliation is supported only on macOS")
    )]
    pub fn physical(&self) -> &ReconciliationPhysicalRecord {
        self.record.physical()
    }

    #[cfg_attr(
        all(not(target_os = "macos"), not(test)),
        expect(dead_code, reason = "native reconciliation is supported only on macOS")
    )]
    pub fn reported_history(&self) -> &[ReconciliationHistoryFact] {
        self.record.reported_history()
    }

    #[cfg_attr(
        all(not(target_os = "macos"), not(test)),
        expect(dead_code, reason = "native reconciliation is supported only on macOS")
    )]
    pub fn validation(&self) -> &ReconciliationValidationFacts {
        self.record.validation()
    }

    pub fn effect(&self) -> &GatewayReconciliationEffect {
        &self.effect
    }
}

/// Durable audit boundary for caller coalescing and native reconciliation.
pub trait GatewayReconciliationAudit: Send + Sync {
    #[cfg_attr(
        all(not(target_os = "macos"), not(test)),
        expect(dead_code, reason = "native reconciliation is supported only on macOS")
    )]
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::domain::value_objects::{
        BundledSurface, ReconciliationCause, ReconciliationInitiator, ReconciliationRuntimeIdentity,
    };

    fn correlation(serial: u64) -> ReconciliationCorrelation {
        ReconciliationCorrelation::parse(format!("00000000-0000-4000-8000-{serial:012x}"))
            .expect("correlation")
    }

    fn target(service: &str) -> ReconciliationTarget {
        ReconciliationTarget::new(service.into(), "a".repeat(64), "b".repeat(64)).expect("target")
    }

    fn incarnation(
        target: ReconciliationTarget,
        serial: u64,
        process_id: u32,
    ) -> ReconciliationIncarnation {
        ReconciliationIncarnation::new(
            target,
            format!("00000000-0000-4000-8000-{serial:012x}"),
            process_id,
            7420,
        )
        .expect("incarnation")
    }

    fn intent(
        target: ReconciliationTarget,
        before: Option<ReconciliationIncarnation>,
    ) -> GatewayReconciliationIntent {
        let evidence = ReconciliationEvidence::new(
            ReconciliationCause::ExplicitRetry,
            ReconciliationInitiator::BundledSurface(BundledSurface::Main),
        )
        .expect("evidence");
        let request = GatewayReconciliationRequest::new(correlation(1), evidence);
        let attempt = GatewayReconciliationAttempt::new(correlation(2), request).expect("attempt");
        GatewayReconciliationIntent::new(attempt, target, before).expect("intent")
    }

    fn replacement_history() -> Vec<ReconciliationHistoryFact> {
        vec![
            ReconciliationHistoryFact::RetirementAcknowledged,
            ReconciliationHistoryFact::OldServiceUnloaded,
            ReconciliationHistoryFact::ServiceDefinitionPublished,
            ReconciliationHistoryFact::ServiceDefinitionDurable,
            ReconciliationHistoryFact::BootstrapCommandRequested,
            ReconciliationHistoryFact::BootstrapCommandCompleted,
            ReconciliationHistoryFact::BootstrapCommandSucceeded,
        ]
    }

    #[test]
    fn application_mapping_cannot_accept_success_after_failed_intent_delivery() {
        let expected = target("service");
        let before = incarnation(expected.clone(), 10, 42);
        let after = incarnation(expected.clone(), 11, 43);
        let outcome = GatewayReconciliationOutcome::assess(
            intent(expected, Some(before)),
            GatewayReconciliationIntentDelivery::Failed(GatewayError::Registration(
                "intent audit failed".into(),
            )),
            GatewayReconciliationEffectTiming::BeforeIntentAcknowledgement,
            replacement_history(),
            Ok(after.clone()),
        );
        let GatewayReconciliationEffect::RejectedReport { report, error, .. } = outcome.effect()
        else {
            panic!("application mapping bypassed the domain assessment");
        };
        assert_eq!(
            error,
            &GatewayError::Registration("intent audit failed".into())
        );
        assert_eq!(report.claimed_identity(), Some(&after));
        assert!(report.validation().candidate_eligible());
        assert_eq!(
            outcome.cleanup(),
            ReconciliationCleanupDecision::AdoptClaimed
        );
    }

    #[test]
    fn application_mapping_preserves_independent_rejection_facts() {
        let expected = target("service");
        let before = incarnation(expected.clone(), 10, 42);
        let claimed = incarnation(target("other"), 10, 99);
        let reported = vec![ReconciliationHistoryFact::BootstrapCommandCompleted];
        let outcome = GatewayReconciliationOutcome::assess(
            intent(expected, Some(before)),
            GatewayReconciliationIntentDelivery::Acknowledged,
            GatewayReconciliationEffectTiming::BeforeIntentReservation,
            reported.clone(),
            Ok(claimed.clone()),
        );
        let GatewayReconciliationEffect::RejectedReport { report, .. } = outcome.effect() else {
            panic!("application mapping bypassed the domain assessment");
        };
        assert_eq!(outcome.reported_history(), reported);
        assert_eq!(report.claimed_identity(), Some(&claimed));
        assert!(!report.validation().target_matches());
        assert_eq!(
            report.validation().runtime_identity(),
            ReconciliationRuntimeIdentity::ChangedProcessForRuntimeInstance
        );
        assert_eq!(report.cleanup(), ReconciliationCleanupDecision::RetainPrior);
    }

    #[test]
    fn post_bootstrap_readiness_failure_remains_an_honest_physical_failure() {
        let expected = target("service");
        let before = incarnation(expected.clone(), 10, 42);
        let outcome = GatewayReconciliationOutcome::assess(
            intent(expected, Some(before)),
            GatewayReconciliationIntentDelivery::Acknowledged,
            GatewayReconciliationEffectTiming::AfterIntentAcknowledgement,
            replacement_history(),
            Err(GatewayError::Registration("readiness failed".into())),
        );
        assert!(matches!(
            outcome.effect(),
            GatewayReconciliationEffect::Failed { error, .. }
                if error == &GatewayError::Registration("readiness failed".into())
        ));
        assert_eq!(outcome.cleanup(), ReconciliationCleanupDecision::ClearPrior);
    }
}
