#![cfg_attr(
    not(any(target_os = "macos", target_os = "linux")),
    allow(
        dead_code,
        reason = "native gateway lifecycle authority is exercised only by the macOS and Linux adapters"
    )
)]

use crate::gateway::domain::value_objects::{
    AuditDeliveryReceipt, LifecycleCommandResult, LifecycleFailedPhase, LifecycleObservation,
    LifecycleObservationSource, LifecyclePhysicalOutcome, LifecyclePlanStep,
    ReconciliationAttemptRecord, ReconciliationCause, ReconciliationCleanupDecision,
    ReconciliationCorrelation, ReconciliationEffectTimingRecord, ReconciliationEvidence,
    ReconciliationHistory, ReconciliationHistoryFact, ReconciliationIncarnation,
    ReconciliationIntentDeliveryRecord, ReconciliationIntentRecord,
    ReconciliationOutcomeDisposition, ReconciliationOutcomeRecord, ReconciliationPhysicalRecord,
    ReconciliationRejectedReport, ReconciliationRequestRecord, ReconciliationTarget, SearchPath,
    SearchPathError, SystemdJobAttempt, SystemdRuntimeObservation,
};
use std::{
    error::Error,
    fmt,
    path::Path,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

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
// contract on every target. Each native adapter reads only the parts its own
// reconciliation protocol needs.
#[cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code))]
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
    #[cfg_attr(
        target_os = "linux",
        allow(dead_code, reason = "the launchd adapter reads this portable identity")
    )]
    pub fn runtime_fingerprint(&self) -> &str {
        &self.runtime_fingerprint
    }
    #[cfg_attr(
        target_os = "linux",
        allow(dead_code, reason = "the launchd adapter reads this portable identity")
    )]
    pub fn runtime_instance(&self) -> &str {
        &self.runtime_instance
    }
    #[cfg_attr(
        target_os = "linux",
        allow(dead_code, reason = "the launchd adapter reads this portable identity")
    )]
    pub fn service_generation(&self) -> &str {
        &self.service_generation
    }
    pub fn process_id(&self) -> u32 {
        self.process_id
    }
    #[cfg_attr(
        target_os = "linux",
        allow(dead_code, reason = "the launchd adapter reads this portable identity")
    )]
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

/// Audited automatic-quit request with one absolute monotonic deadline.
#[derive(Clone, Debug)]
pub struct GatewayStopRequest {
    attempt: GatewayReconciliationAttempt,
    intended: ReconciledGateway,
    deadline: Instant,
}

impl GatewayStopRequest {
    pub(crate) fn new(
        attempt: GatewayReconciliationAttempt,
        intended: ReconciledGateway,
        deadline: Instant,
    ) -> Self {
        Self {
            attempt,
            intended,
            deadline,
        }
    }

    pub fn intended(&self) -> &ReconciledGateway {
        &self.intended
    }

    pub fn deadline(&self) -> Instant {
        self.deadline
    }
}

#[derive(Clone, Debug)]
enum StopDispatchAuthority {
    AvailableUnproved,
    Proving {
        token: u64,
    },
    Proved {
        token: u64,
        candidate: ReconciliationIncarnation,
        observation_version: u64,
    },
    Claimed,
    CommandResult(LifecycleCommandResult),
    FreshObservation {
        command: LifecycleCommandResult,
        observation: LifecycleObservation,
    },
    OutcomeDelivery {
        command: LifecycleCommandResult,
        observation: LifecycleObservation,
    },
    Revoked,
}

/// Unforgeable, consume-once authority joining one proof to one dispatch.
///
/// The application mints this after checking the session deadline. Native
/// adapters may move it into a platform proof guard, but cannot clone or
/// construct another token for a different witness.
pub struct GatewayStopProofToken {
    session: Arc<()>,
    token: u64,
}

/// One application-owned proof-to-dispatch authority for automatic quit.
pub struct GatewayStopSession {
    session: Arc<()>,
    request: GatewayStopRequest,
    state: Mutex<StopDispatchAuthority>,
    latest_observation_version: AtomicU64,
    clock: Arc<dyn MonotonicClock>,
}

impl GatewayStopSession {
    pub(crate) fn new(request: GatewayStopRequest, clock: Arc<dyn MonotonicClock>) -> Self {
        Self {
            session: Arc::new(()),
            request,
            state: Mutex::new(StopDispatchAuthority::AvailableUnproved),
            latest_observation_version: AtomicU64::new(0),
            clock,
        }
    }

    pub fn request(&self) -> &GatewayStopRequest {
        &self.request
    }

    pub fn deadline_passed(&self) -> bool {
        self.clock.now() >= self.request.deadline
    }

    #[cfg_attr(
        target_os = "linux",
        allow(dead_code, reason = "the launchd adapter owns this bounded wait")
    )]
    pub fn wait(&self, duration: Duration) {
        self.clock.wait(duration);
    }

    pub fn begin_proof(&self) -> Result<GatewayStopProofToken, GatewayError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| GatewayError::Stop("Gateway stop authority is unavailable".into()))?;
        if self.deadline_passed() {
            *state = StopDispatchAuthority::Revoked;
            return Err(GatewayError::Stop(
                "Gateway stop deadline passed before recipient proof".into(),
            ));
        }
        if !matches!(&*state, StopDispatchAuthority::AvailableUnproved) {
            return Err(GatewayError::Stop(
                "Gateway stop recipient proof was already started".into(),
            ));
        }
        let token = 1;
        *state = StopDispatchAuthority::Proving { token };
        Ok(GatewayStopProofToken {
            session: self.session.clone(),
            token,
        })
    }

    pub fn prove(
        &self,
        token: &GatewayStopProofToken,
        candidate: ReconciliationIncarnation,
        observation_version: u64,
    ) -> Result<(), GatewayError> {
        let intended = self.request.intended.audit_identity()?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| GatewayError::Stop("Gateway stop authority is unavailable".into()))?;
        if self.deadline_passed() {
            *state = StopDispatchAuthority::Revoked;
            return Err(GatewayError::Stop(
                "Gateway stop recipient proof arrived after its deadline".into(),
            ));
        }
        if !Arc::ptr_eq(&token.session, &self.session)
            || !matches!(&*state, StopDispatchAuthority::Proving { token: expected } if *expected == token.token)
            || candidate != intended
        {
            return Err(GatewayError::Stop(
                "Gateway stop recipient differs from the intended incarnation".into(),
            ));
        }
        let latest = self.latest_observation_version.load(Ordering::SeqCst);
        if observation_version < latest {
            return Err(GatewayError::Stop(
                "Gateway stop proof used an obsolete observation version".into(),
            ));
        }
        self.latest_observation_version
            .store(observation_version, Ordering::SeqCst);
        *state = StopDispatchAuthority::Proved {
            token: token.token,
            candidate,
            observation_version,
        };
        Ok(())
    }

    /// Atomically consume the proof immediately before the native call.
    pub fn claim(
        &self,
        token: GatewayStopProofToken,
        plan: &AuditDeliveryReceipt,
        candidate: &ReconciliationIncarnation,
        observation_version: u64,
    ) -> Result<(), GatewayError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| GatewayError::Stop("Gateway stop authority is unavailable".into()))?;
        if self.deadline_passed() {
            *state = StopDispatchAuthority::Revoked;
            return Err(GatewayError::Stop(
                "Gateway stop deadline revoked dispatch authority".into(),
            ));
        }
        let valid_plan = plan.attempt_correlation() == self.request.attempt.correlation()
            && plan.sequence() == 1
            && plan.record_kind()
                == crate::gateway::domain::value_objects::LifecycleRecordKind::EffectPlan;
        let matches_proof = Arc::ptr_eq(&token.session, &self.session)
            && matches!(
                &*state,
                StopDispatchAuthority::Proved {
                    token: expected_token,
                    candidate: proved,
                    observation_version: proved_version,
                } if *expected_token == token.token
                    && proved == candidate
                    && *proved_version == observation_version
            )
            && self.latest_observation_version.load(Ordering::SeqCst) == observation_version;
        if !valid_plan || !matches_proof {
            return Err(GatewayError::Stop(
                "Gateway stop proof, plan, or observation version changed before dispatch".into(),
            ));
        }
        *state = StopDispatchAuthority::Claimed;
        Ok(())
    }

    /// Advance the application observation version while a proof is still
    /// revocable. A later claim for an older proof is refused.
    #[cfg(test)]
    pub fn observation_changed(&self, observation_version: u64) {
        self.latest_observation_version
            .fetch_max(observation_version, Ordering::SeqCst);
    }

    pub fn command_result(&self, result: LifecycleCommandResult) -> Result<(), GatewayError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| GatewayError::Stop("Gateway stop authority is unavailable".into()))?;
        if !matches!(&*state, StopDispatchAuthority::Claimed) {
            return Err(GatewayError::Stop(
                "Gateway stop command returned without dispatch authority".into(),
            ));
        }
        *state = StopDispatchAuthority::CommandResult(result);
        Ok(())
    }

    pub fn fresh_observation(&self, observation: LifecycleObservation) -> Result<(), GatewayError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| GatewayError::Stop("Gateway stop authority is unavailable".into()))?;
        let StopDispatchAuthority::CommandResult(command) = &*state else {
            return Err(GatewayError::Stop(
                "Gateway stop observation has no command result".into(),
            ));
        };
        let command = command.clone();
        *state = StopDispatchAuthority::FreshObservation {
            command,
            observation,
        };
        Ok(())
    }

    pub fn settlement(
        &self,
    ) -> Result<(LifecycleCommandResult, LifecycleObservation), GatewayError> {
        let state = self
            .state
            .lock()
            .map_err(|_| GatewayError::Stop("Gateway stop authority is unavailable".into()))?;
        match &*state {
            StopDispatchAuthority::FreshObservation {
                command,
                observation,
            }
            | StopDispatchAuthority::OutcomeDelivery {
                command,
                observation,
            } => Ok((command.clone(), observation.clone())),
            _ => Err(GatewayError::Stop(
                "Gateway stop did not reach a fresh observation".into(),
            )),
        }
    }

    /// Authorize one immediate terminal-outcome delivery from settled facts.
    ///
    /// Callers invoke this again before an exact retry so the same monotonic
    /// deadline guards every audit-port call, including a retry that starts
    /// after the first call consumed the remaining budget.
    pub(crate) fn begin_outcome_delivery(&self) -> Result<(), GatewayError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| GatewayError::Stop("Gateway stop authority is unavailable".into()))?;
        if self.clock.now() >= self.request.deadline {
            return Err(GatewayError::Stop(
                "Gateway stop deadline passed before outcome delivery".into(),
            ));
        }
        let (command, observation) = match &*state {
            StopDispatchAuthority::FreshObservation {
                command,
                observation,
            }
            | StopDispatchAuthority::OutcomeDelivery {
                command,
                observation,
            } => (command.clone(), observation.clone()),
            _ => {
                return Err(GatewayError::Stop(
                    "Gateway stop outcome has no settled physical facts".into(),
                ))
            }
        };
        *state = StopDispatchAuthority::OutcomeDelivery {
            command,
            observation,
        };
        Ok(())
    }

    pub fn expire_at_deadline(&self) {
        if let Ok(mut state) = self.state.lock() {
            match &*state {
                StopDispatchAuthority::AvailableUnproved
                | StopDispatchAuthority::Proving { .. }
                | StopDispatchAuthority::Proved { .. } => {
                    *state = StopDispatchAuthority::Revoked;
                }
                StopDispatchAuthority::Claimed => {
                    *state = StopDispatchAuthority::CommandResult(
                        LifecycleCommandResult::Indeterminate(
                            "gateway stop dispatch remained in flight at the quit deadline".into(),
                        ),
                    );
                }
                StopDispatchAuthority::CommandResult(_)
                | StopDispatchAuthority::FreshObservation { .. }
                | StopDispatchAuthority::OutcomeDelivery { .. }
                | StopDispatchAuthority::Revoked => {}
            }
        }
    }
}

/// Monotonic time used to arbitrate lifecycle deadlines.
pub trait MonotonicClock: Send + Sync {
    fn now(&self) -> Instant;

    /// Wait between bounded retries. Test clocks may advance or coordinate
    /// deterministically instead of sleeping the process thread.
    fn wait(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// Process monotonic clock used by desktop composition.
pub struct SystemMonotonicClock;

impl MonotonicClock for SystemMonotonicClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatewayPhysicalResult {
    Succeeded,
    Failed(Box<GatewayError>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatewayError {
    Registration(String),
    Audit {
        audit: String,
        physical: Option<GatewayPhysicalResult>,
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
                    match physical {
                        GatewayPhysicalResult::Succeeded => {
                            f.write_str("; physical reconciliation succeeded")?
                        }
                        GatewayPhysicalResult::Failed(error) => {
                            write!(f, "; physical reconciliation also failed: {error}")?
                        }
                    }
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
        all(not(any(target_os = "macos", target_os = "linux")), not(test)),
        allow(
            dead_code,
            reason = "native reconciliation is supported only on macOS and Linux"
        )
    )]
    fn readiness_invalidated(&self);
    #[cfg_attr(
        all(not(any(target_os = "macos", target_os = "linux")), not(test)),
        allow(
            dead_code,
            reason = "native reconciliation is supported only on macOS and Linux"
        )
    )]
    fn intent_admitted(&self, intent: GatewayReconciliationIntent) -> Result<(), GatewayError>;
    #[cfg_attr(
        all(not(any(target_os = "macos", target_os = "linux")), not(test)),
        allow(
            dead_code,
            reason = "native reconciliation is supported only on macOS and Linux"
        )
    )]
    fn history_observed(&self, fact: ReconciliationHistoryFact);

    fn effect_planned(
        &self,
        plan_id: &str,
        primary: &LifecyclePlanStep,
        cleanup: &[LifecyclePlanStep],
    ) -> Result<AuditDeliveryReceipt, GatewayError>;

    fn effect_completed(
        &self,
        plan_id: &str,
        step_id: &str,
        result: &LifecycleCommandResult,
    ) -> Result<AuditDeliveryReceipt, GatewayError>;

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    fn native_attempt_recorded(
        &self,
        plan_id: &str,
        step_id: &str,
        attempt: &SystemdJobAttempt,
    ) -> Result<AuditDeliveryReceipt, GatewayError> {
        let _ = (plan_id, step_id, attempt);
        Err(GatewayError::Audit {
            audit: "The reconciliation progress cannot record a native systemd attempt".into(),
            physical: None,
        })
    }

    fn physical_observed(
        &self,
        source: &LifecycleObservationSource,
        incarnation: Option<ReconciliationIncarnation>,
        target_artifact_present: bool,
    ) -> Result<LifecycleObservation, GatewayError>;

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    fn systemd_observed(
        &self,
        source: &LifecycleObservationSource,
        incarnation: ReconciliationIncarnation,
        target_artifact_present: bool,
        native: SystemdRuntimeObservation,
    ) -> Result<LifecycleObservation, GatewayError> {
        let _ = (source, incarnation, target_artifact_present, native);
        Err(GatewayError::Registration(
            "This gateway progress port cannot record systemd evidence".into(),
        ))
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    fn systemd_state_observed(
        &self,
        source: &LifecycleObservationSource,
        target_artifact_present: bool,
        state: crate::gateway::domain::value_objects::SystemdUnitState,
    ) -> Result<LifecycleObservation, GatewayError> {
        let _ = (source, target_artifact_present, state);
        Err(GatewayError::Registration(
            "This gateway progress port cannot record systemd state evidence".into(),
        ))
    }

    /// Retry the exact observation whose publication may have succeeded before
    /// acknowledgement. Returns `None` when no observation is pending.
    #[cfg_attr(
        target_os = "linux",
        allow(
            dead_code,
            reason = "the launchd progress substitutes use the portable no-pending default"
        )
    )]
    fn retry_pending_observation(&self) -> Result<Option<LifecycleObservation>, GatewayError> {
        Ok(None)
    }
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
        all(not(any(target_os = "macos", target_os = "linux")), not(test)),
        allow(
            dead_code,
            reason = "native reconciliation is supported only on macOS and Linux"
        )
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
        all(not(any(target_os = "macos", target_os = "linux")), not(test)),
        allow(
            dead_code,
            reason = "native reconciliation is supported only on macOS and Linux"
        )
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
        all(not(any(target_os = "macos", target_os = "linux")), not(test)),
        allow(
            dead_code,
            reason = "native reconciliation is supported only on macOS and Linux"
        )
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
        all(not(any(target_os = "macos", target_os = "linux")), not(test)),
        allow(
            dead_code,
            reason = "native reconciliation is supported only on macOS and Linux"
        )
    )]
    pub fn attempt(&self) -> &GatewayReconciliationAttempt {
        &self.attempt
    }

    #[cfg_attr(
        all(not(any(target_os = "macos", target_os = "linux")), not(test)),
        allow(
            dead_code,
            reason = "native reconciliation is supported only on macOS and Linux"
        )
    )]
    pub fn target(&self) -> &ReconciliationTarget {
        self.record.target()
    }

    #[cfg_attr(
        not(any(target_os = "macos", target_os = "linux")),
        allow(
            dead_code,
            reason = "native reconciliation is supported only on macOS and Linux"
        )
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
    failed_phase: LifecycleFailedPhase,
}

/// Why a terminal lifecycle outcome was not durably acknowledged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GatewayReconciliationOutcomeError {
    /// The journal state machine rejected contradictory terminal facts.
    Rejected(GatewayError),
    /// Valid terminal facts could not be delivered to durable storage.
    Delivery(GatewayError),
}

impl GatewayReconciliationOutcomeError {
    pub fn error(&self) -> &GatewayError {
        match self {
            Self::Rejected(error) | Self::Delivery(error) => error,
        }
    }
}

impl GatewayReconciliationOutcome {
    pub fn assess(
        intent: GatewayReconciliationIntent,
        intent_delivery: GatewayReconciliationIntentDelivery,
        effect_timing: GatewayReconciliationEffectTiming,
        facts: Vec<ReconciliationHistoryFact>,
        failed_phase: LifecycleFailedPhase,
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
            failed_phase,
        }
    }

    #[cfg(test)]
    pub fn intent(&self) -> &GatewayReconciliationIntent {
        &self.intent
    }

    #[cfg(test)]
    pub fn intent_delivery(&self) -> &GatewayReconciliationIntentDelivery {
        &self.intent_delivery
    }

    #[cfg(test)]
    pub fn effect_timing(&self) -> GatewayReconciliationEffectTiming {
        self.effect_timing
    }

    pub fn cleanup(&self) -> ReconciliationCleanupDecision {
        self.record.cleanup()
    }

    #[cfg(test)]
    pub fn reported_history(&self) -> &[ReconciliationHistoryFact] {
        self.record.reported_history()
    }

    pub fn effect(&self) -> &GatewayReconciliationEffect {
        &self.effect
    }

    pub fn failed_phase(&self) -> LifecycleFailedPhase {
        self.failed_phase
    }
}

/// One locked lifecycle-journal session for a serialized native attempt.
///
/// The session owns stage-wide ordering from restore through terminal outcome.
/// Dropping it releases the stage lock even when audit delivery or native work
/// fails.
pub trait GatewayReconciliationJournalSession: Send + Sync {
    /// Return a sole validated unresolved lifecycle restored while acquiring
    /// this stage-wide session. A fresh session returns `None`.
    fn recovery(&self) -> Option<GatewayLifecycleRecovery> {
        None
    }

    fn intent(&self, intent: &GatewayReconciliationIntent) -> Result<(), GatewayError>;

    fn outcome(
        &self,
        outcome: &GatewayReconciliationOutcome,
    ) -> Result<(), GatewayReconciliationOutcomeError>;

    /// Records a caller whose request joined this admitted attempt.
    fn joined(&self, joined: &GatewayReconciliationRequest) -> Result<(), GatewayError>;

    fn effect_plan(
        &self,
        plan_id: &str,
        expected_before: Option<&ReconciliationIncarnation>,
        target: &ReconciliationTarget,
        primary: &LifecyclePlanStep,
        cleanup: &[LifecyclePlanStep],
    ) -> Result<AuditDeliveryReceipt, GatewayError>;

    fn effect_completion(
        &self,
        plan_id: &str,
        step_id: &str,
        result: &LifecycleCommandResult,
    ) -> Result<AuditDeliveryReceipt, GatewayError>;

    /// Records the exact native job identity returned by a systemd enqueue.
    /// A missing record means the call may still have taken effect and cannot
    /// be replayed as though no native attempt existed.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    fn native_attempt(
        &self,
        plan_id: &str,
        step_id: &str,
        attempt: &SystemdJobAttempt,
    ) -> Result<AuditDeliveryReceipt, GatewayError> {
        let _ = (plan_id, step_id, attempt);
        Err(GatewayError::Audit {
            audit: "The reconciliation journal cannot record a native systemd attempt".into(),
            physical: None,
        })
    }

    fn observation(
        &self,
        source: &LifecycleObservationSource,
        state: &LifecycleObservation,
    ) -> Result<AuditDeliveryReceipt, GatewayError>;

    fn physical_outcome(
        &self,
        physical: &LifecyclePhysicalOutcome,
        last_confirmed: Option<&LifecycleObservation>,
        cleanup: ReconciliationCleanupDecision,
    ) -> Result<AuditDeliveryReceipt, GatewayError>;
}

/// Exact persisted authority that must settle before a fresh attempt starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GatewayLifecycleRecovery {
    attempt: GatewayReconciliationAttempt,
    target: ReconciliationTarget,
    before: Option<ReconciliationIncarnation>,
    has_effect_plan: bool,
    latest_observation: Option<LifecycleObservation>,
    pending_step: Option<GatewayLifecycleRecoveryStep>,
    pending_observation_source: Option<LifecycleObservationSource>,
}

/// Exact persisted effect boundary awaiting completion or observation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GatewayLifecycleRecoveryStep {
    plan_id: String,
    step: LifecyclePlanStep,
    contingencies: Vec<LifecyclePlanStep>,
    completion: Option<LifecycleCommandResult>,
    native_attempt: Option<SystemdJobAttempt>,
}

impl GatewayLifecycleRecoveryStep {
    pub(crate) fn new(
        plan_id: String,
        step: LifecyclePlanStep,
        contingencies: Vec<LifecyclePlanStep>,
        completion: Option<LifecycleCommandResult>,
        native_attempt: Option<SystemdJobAttempt>,
    ) -> Self {
        Self {
            plan_id,
            step,
            contingencies,
            completion,
            native_attempt,
        }
    }

    pub fn plan_id(&self) -> &str {
        &self.plan_id
    }

    pub fn step(&self) -> &LifecyclePlanStep {
        &self.step
    }

    pub fn completion(&self) -> Option<&LifecycleCommandResult> {
        self.completion.as_ref()
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn contingencies(&self) -> &[LifecyclePlanStep] {
        &self.contingencies
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn native_attempt(&self) -> Option<&SystemdJobAttempt> {
        self.native_attempt.as_ref()
    }

    pub fn source(&self) -> LifecycleObservationSource {
        LifecycleObservationSource::Effect {
            plan_id: self.plan_id.clone(),
            step_id: self.step.id().to_owned(),
        }
    }
}

impl GatewayLifecycleRecovery {
    pub(crate) fn new(
        attempt: GatewayReconciliationAttempt,
        target: ReconciliationTarget,
        before: Option<ReconciliationIncarnation>,
        has_effect_plan: bool,
        latest_observation: Option<LifecycleObservation>,
        pending_step: Option<GatewayLifecycleRecoveryStep>,
        pending_observation_source: Option<LifecycleObservationSource>,
    ) -> Self {
        Self {
            attempt,
            target,
            before,
            has_effect_plan,
            latest_observation,
            pending_step,
            pending_observation_source,
        }
    }

    pub fn attempt(&self) -> &GatewayReconciliationAttempt {
        &self.attempt
    }

    pub fn target(&self) -> &ReconciliationTarget {
        &self.target
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn before(&self) -> Option<&ReconciliationIncarnation> {
        self.before.as_ref()
    }

    pub fn has_effect_plan(&self) -> bool {
        self.has_effect_plan
    }

    pub fn latest_observation(&self) -> Option<&LifecycleObservation> {
        self.latest_observation.as_ref()
    }

    pub fn pending_step(&self) -> Option<&GatewayLifecycleRecoveryStep> {
        self.pending_step.as_ref()
    }

    #[cfg_attr(
        target_os = "linux",
        allow(
            dead_code,
            reason = "the launchd adapter resumes this portable observation delivery"
        )
    )]
    pub fn pending_observation_source(&self) -> Option<&LifecycleObservationSource> {
        self.pending_observation_source.as_ref()
    }
}

/// Opens the single durable lifecycle journal for one serialized attempt.
pub trait GatewayReconciliationAudit: Send + Sync {
    fn open(
        self: Arc<Self>,
        attempt: &GatewayReconciliationAttempt,
        deadline: Option<Instant>,
    ) -> Result<Arc<dyn GatewayReconciliationJournalSession>, GatewayError>;
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
    /// Classify the durable definition change that caused host startup work.
    fn startup_cause(&self) -> ReconciliationCause {
        ReconciliationCause::Startup
    }

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

    /// Reobserve and settle one validated persisted lifecycle before current
    /// configuration is allowed to allocate or mutate anything.
    fn recover(
        &self,
        recovery: &GatewayLifecycleRecovery,
        journal: &dyn GatewayReconciliationJournalSession,
    ) -> Result<(), GatewayError> {
        let _ = (recovery, journal);
        Err(GatewayError::Registration(
            "The unresolved gateway lifecycle cannot be safely settled by this host".into(),
        ))
    }

    fn stop_agents(
        &self,
        session: &GatewayStopSession,
        journal: &dyn GatewayReconciliationJournalSession,
        plan: &AuditDeliveryReceipt,
    ) -> Result<LifecycleObservation, GatewayError>;
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
        AuditDeliveryReceipt, GatewayError, GatewayReconciliationAttempt,
        GatewayReconciliationAudit, GatewayReconciliationIds, GatewayReconciliationIntent,
        GatewayReconciliationJournalSession, GatewayReconciliationOutcome,
        GatewayReconciliationOutcomeError, GatewayReconciliationRequest, GatewayStartup,
        GatewayStartupEvents, LifecycleCommandResult, LifecycleObservation,
        LifecycleObservationSource, LifecyclePhysicalOutcome, LifecyclePlanStep, LoginShellError,
        LoginShellPath, ReconciliationCleanupDecision, ReconciliationCorrelation,
        ReconciliationIncarnation, ReconciliationTarget, SearchPath,
    };
    use std::{
        sync::{
            atomic::{AtomicU64, Ordering},
            Arc,
        },
        time::Instant,
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

    struct DiscardJournalSession(ReconciliationCorrelation);

    impl GatewayReconciliationJournalSession for DiscardJournalSession {
        fn intent(&self, _: &GatewayReconciliationIntent) -> Result<(), GatewayError> {
            Ok(())
        }

        fn outcome(
            &self,
            _: &GatewayReconciliationOutcome,
        ) -> Result<(), GatewayReconciliationOutcomeError> {
            Ok(())
        }

        fn joined(&self, _: &GatewayReconciliationRequest) -> Result<(), GatewayError> {
            Ok(())
        }

        fn effect_plan(
            &self,
            _: &str,
            _: Option<&ReconciliationIncarnation>,
            _: &ReconciliationTarget,
            _: &LifecyclePlanStep,
            _: &[LifecyclePlanStep],
        ) -> Result<AuditDeliveryReceipt, GatewayError> {
            Ok(AuditDeliveryReceipt::new(
                self.0.clone(),
                1,
                crate::gateway::domain::value_objects::LifecycleRecordKind::EffectPlan,
            ))
        }

        fn effect_completion(
            &self,
            _: &str,
            _: &str,
            _: &LifecycleCommandResult,
        ) -> Result<AuditDeliveryReceipt, GatewayError> {
            Ok(AuditDeliveryReceipt::new(
                self.0.clone(),
                2,
                crate::gateway::domain::value_objects::LifecycleRecordKind::EffectCompletion,
            ))
        }

        fn observation(
            &self,
            _: &LifecycleObservationSource,
            _: &LifecycleObservation,
        ) -> Result<AuditDeliveryReceipt, GatewayError> {
            Ok(AuditDeliveryReceipt::new(
                self.0.clone(),
                3,
                crate::gateway::domain::value_objects::LifecycleRecordKind::Observation,
            ))
        }

        fn physical_outcome(
            &self,
            _: &LifecyclePhysicalOutcome,
            _: Option<&LifecycleObservation>,
            _: ReconciliationCleanupDecision,
        ) -> Result<AuditDeliveryReceipt, GatewayError> {
            Ok(AuditDeliveryReceipt::new(
                self.0.clone(),
                4,
                crate::gateway::domain::value_objects::LifecycleRecordKind::Outcome,
            ))
        }
    }

    impl GatewayReconciliationAudit for DiscardReconciliationAudit {
        fn open(
            self: Arc<Self>,
            attempt: &GatewayReconciliationAttempt,
            _: Option<Instant>,
        ) -> Result<Arc<dyn GatewayReconciliationJournalSession>, GatewayError> {
            Ok(Arc::new(DiscardJournalSession(
                attempt.correlation().clone(),
            )))
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
    use std::{
        sync::{Arc, Mutex},
        time::Duration,
    };

    struct ControlledClock(Mutex<Instant>);

    impl ControlledClock {
        fn new(now: Instant) -> Self {
            Self(Mutex::new(now))
        }

        fn set(&self, now: Instant) {
            *self.0.lock().expect("controlled clock") = now;
        }
    }

    impl MonotonicClock for ControlledClock {
        fn now(&self) -> Instant {
            *self.0.lock().expect("controlled clock")
        }
    }

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
            LifecycleFailedPhase::NativeDispatch,
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
            LifecycleFailedPhase::NativeDispatch,
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
            LifecycleFailedPhase::Observation,
            Err(GatewayError::Registration("readiness failed".into())),
        );
        assert!(matches!(
            outcome.effect(),
            GatewayReconciliationEffect::Failed { error, .. }
                if error == &GatewayError::Registration("readiness failed".into())
        ));
        assert_eq!(outcome.cleanup(), ReconciliationCleanupDecision::ClearPrior);
    }

    fn stop_session(
        deadline: Instant,
    ) -> (
        GatewayStopSession,
        ReconciliationIncarnation,
        AuditDeliveryReceipt,
    ) {
        stop_session_with_clock(deadline, Arc::new(SystemMonotonicClock))
    }

    fn stop_session_with_clock(
        deadline: Instant,
        clock: Arc<dyn MonotonicClock>,
    ) -> (
        GatewayStopSession,
        ReconciliationIncarnation,
        AuditDeliveryReceipt,
    ) {
        let evidence = ReconciliationEvidence::new(
            ReconciliationCause::DesktopQuitPolicy,
            ReconciliationInitiator::DesktopHost,
        )
        .unwrap();
        let request = GatewayReconciliationRequest::new(correlation(20), evidence);
        let attempt = GatewayReconciliationAttempt::new(correlation(21), request).unwrap();
        let intended = incarnation(target("service"), 22, 42);
        let gateway = ReconciledGateway::new(
            intended.target().service().into(),
            intended.target().runtime_fingerprint().into(),
            intended.runtime_instance().into(),
            intended.target().service_generation().into(),
            intended.process_id(),
            intended.port(),
        );
        let receipt = AuditDeliveryReceipt::new(
            attempt.correlation().clone(),
            1,
            crate::gateway::domain::value_objects::LifecycleRecordKind::EffectPlan,
        );
        (
            GatewayStopSession::new(GatewayStopRequest::new(attempt, gateway, deadline), clock),
            intended,
            receipt,
        )
    }

    #[test]
    fn stop_proof_arriving_after_deadline_cannot_claim_dispatch() {
        let (session, _, _) = stop_session(Instant::now());
        assert!(session.begin_proof().is_err());
    }

    #[test]
    fn stop_claim_requires_the_proved_identity_and_observation_version() {
        let (session, intended, receipt) = stop_session(Instant::now() + Duration::from_secs(60));
        let proof_token = session.begin_proof().unwrap();
        session.prove(&proof_token, intended.clone(), 7).unwrap();
        assert!(session.claim(proof_token, &receipt, &intended, 8).is_err());

        let (session, intended, receipt) = stop_session(Instant::now() + Duration::from_secs(60));
        let proof_token = session.begin_proof().unwrap();
        session.prove(&proof_token, intended.clone(), 7).unwrap();
        session.observation_changed(8);
        assert!(session.claim(proof_token, &receipt, &intended, 7).is_err());

        let (session, intended, receipt) = stop_session(Instant::now() + Duration::from_secs(60));
        let proof_token = session.begin_proof().unwrap();
        session.prove(&proof_token, intended.clone(), 7).unwrap();
        session.claim(proof_token, &receipt, &intended, 7).unwrap();
        assert!(session.begin_proof().is_err());
    }

    #[test]
    fn stop_proof_tokens_cannot_cross_sessions_for_the_same_incarnation() {
        let deadline = Instant::now() + Duration::from_secs(60);
        let (first, intended, _) = stop_session(deadline);
        let (second, second_intended, second_receipt) = stop_session(deadline);
        assert_eq!(intended, second_intended);
        let first_token = first.begin_proof().unwrap();
        let second_token = second.begin_proof().unwrap();
        assert!(second.prove(&first_token, intended.clone(), 1).is_err());
        second.prove(&second_token, intended.clone(), 1).unwrap();
        assert!(second
            .claim(first_token, &second_receipt, &intended, 1)
            .is_err());
    }

    #[test]
    fn deadline_revokes_a_proof_that_finishes_late_without_a_dispatch_claim() {
        let start = Instant::now();
        let clock = Arc::new(ControlledClock::new(start));
        let (session, intended, receipt) =
            stop_session_with_clock(start + Duration::from_secs(1), clock.clone());
        let proof_token = session.begin_proof().unwrap();
        clock.set(start + Duration::from_secs(1));
        assert!(session.prove(&proof_token, intended.clone(), 1).is_err());
        assert!(session.claim(proof_token, &receipt, &intended, 1).is_err());
    }

    #[test]
    fn claim_and_deadline_use_one_clock_and_lock_in_either_order() {
        let start = Instant::now();
        let deadline = start + Duration::from_secs(1);

        let clock = Arc::new(ControlledClock::new(start));
        let (claimed, intended, receipt) = stop_session_with_clock(deadline, clock.clone());
        let proof_token = claimed.begin_proof().unwrap();
        claimed.prove(&proof_token, intended.clone(), 1).unwrap();
        claimed.claim(proof_token, &receipt, &intended, 1).unwrap();
        clock.set(deadline);
        claimed.expire_at_deadline();
        assert!(claimed
            .command_result(LifecycleCommandResult::Accepted)
            .is_err());

        let clock = Arc::new(ControlledClock::new(start));
        let (revoked, intended, receipt) = stop_session_with_clock(deadline, clock.clone());
        let proof_token = revoked.begin_proof().unwrap();
        revoked.prove(&proof_token, intended.clone(), 1).unwrap();
        clock.set(deadline);
        assert!(revoked.claim(proof_token, &receipt, &intended, 1).is_err());
        assert!(revoked
            .command_result(LifecycleCommandResult::Accepted)
            .is_err());
    }

    #[test]
    fn accepted_label_command_keeps_a_replacement_observation_separate() {
        let (session, intended, receipt) = stop_session(Instant::now() + Duration::from_secs(60));
        let proof_token = session.begin_proof().unwrap();
        session.prove(&proof_token, intended.clone(), 1).unwrap();
        session.claim(proof_token, &receipt, &intended, 1).unwrap();
        session
            .command_result(LifecycleCommandResult::Accepted)
            .unwrap();
        let replacement =
            LifecycleObservation::new(2, Some(incarnation(target("service"), 23, 43)), true);
        session.fresh_observation(replacement.clone()).unwrap();

        assert_eq!(
            session.settlement().unwrap(),
            (LifecycleCommandResult::Accepted, replacement)
        );
        assert_ne!(
            session.settlement().unwrap().1.incarnation(),
            Some(&intended)
        );
    }

    #[test]
    fn outcome_start_uses_the_stop_clock_and_preserves_settled_facts_at_deadline() {
        let start = Instant::now();
        let deadline = start + Duration::from_secs(1);
        let clock = Arc::new(ControlledClock::new(start));
        let (session, intended, receipt) = stop_session_with_clock(deadline, clock.clone());
        let proof_token = session.begin_proof().unwrap();
        session.prove(&proof_token, intended.clone(), 1).unwrap();
        session.claim(proof_token, &receipt, &intended, 1).unwrap();
        session
            .command_result(LifecycleCommandResult::Accepted)
            .unwrap();
        let observation = LifecycleObservation::new(2, Some(intended), true);
        session.fresh_observation(observation.clone()).unwrap();

        clock.set(deadline);
        assert!(session.begin_outcome_delivery().is_err());
        assert_eq!(
            session.settlement().unwrap(),
            (LifecycleCommandResult::Accepted, observation)
        );
    }
}
