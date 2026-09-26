#![cfg_attr(
    not(any(target_os = "macos", target_os = "linux")),
    allow(
        dead_code,
        reason = "native gateway lifecycle state is exercised only by the macOS adapter"
    )
)]

#[cfg(test)]
use super::SystemMonotonicClock;
use super::{
    GatewayError, GatewayHost, GatewayPhysicalResult, GatewayReconciliationAttempt,
    GatewayReconciliationAudit, GatewayReconciliationEffect, GatewayReconciliationEffectTiming,
    GatewayReconciliationIds, GatewayReconciliationIntent, GatewayReconciliationIntentDelivery,
    GatewayReconciliationJournalSession, GatewayReconciliationOutcome,
    GatewayReconciliationOutcomeError, GatewayReconciliationProgress, GatewayReconciliationRequest,
    GatewayStartup, GatewayStartupEvents, GatewayStartupPhase, GatewayStopRequest,
    GatewayStopSession, LoginShellPath, MonotonicClock, ReconciledGateway,
    ReconciliationHistoryFact, StartupStep,
};
use crate::gateway::domain::value_objects::{
    AuditDeliveryReceipt, BundledSurface, LifecycleCommandResult, LifecycleEffect,
    LifecycleEffectPredicate, LifecycleFailedPhase, LifecycleObservation,
    LifecycleObservationSource, LifecyclePhysicalOutcome, LifecyclePlanStep, PendingReconciliation,
    ReconciliationCause, ReconciliationCleanupDecision, ReconciliationCorrelation,
    ReconciliationEvidence, ReconciliationIncarnation, ReconciliationInitiator, SearchPath,
};
#[cfg(test)]
use std::time::Duration;
use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    path::PathBuf,
    sync::{mpsc, Arc, Condvar, Mutex, OnceLock},
    thread,
    time::Instant,
};

struct Lifecycle {
    startup: GatewayStartup,
    retained_gateway: Option<ReconciledGateway>,
    ready_gateway: Option<ReconciledGateway>,
    running: Option<Arc<AttemptReceipt>>,
    pending: Option<Arc<AttemptReceipt>>,
}

struct AttemptReceipt {
    request: GatewayReconciliationAttempt,
    authority: PendingReconciliation,
    state: Mutex<AttemptState>,
    settled: Condvar,
    journal: Mutex<JournalState>,
    journal_ready: Condvar,
}

#[derive(Clone)]
enum JournalState {
    Pending,
    Ready(Arc<dyn GatewayReconciliationJournalSession>),
    Failed(GatewayError),
}

struct AttemptState {
    settlement: Option<AttemptSettlement>,
    waiters: usize,
}

#[cfg(test)]
#[derive(Default)]
struct LifecycleProbe {
    generation: Mutex<u64>,
    changed: Condvar,
}

#[cfg(test)]
impl LifecycleProbe {
    fn notify(&self) {
        if let Ok(mut generation) = self.generation.lock() {
            *generation = generation.saturating_add(1);
            self.changed.notify_all();
        }
    }

    fn wait_for_pending(&self, lifecycle: &Mutex<Lifecycle>) -> Option<Arc<AttemptReceipt>> {
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut generation = self.generation.lock().ok()?;
        loop {
            if let Some(pending) = lifecycle.lock().ok()?.pending.clone() {
                return Some(pending);
            }
            let remaining = deadline.checked_duration_since(Instant::now())?;
            let (next, timeout) = self.changed.wait_timeout(generation, remaining).ok()?;
            generation = next;
            if timeout.timed_out() {
                return lifecycle.lock().ok()?.pending.clone();
            }
        }
    }
}

#[cfg(test)]
#[derive(Default)]
struct OwnerLaunchGate {
    state: Mutex<OwnerLaunchGateState>,
    changed: Condvar,
}

#[cfg(test)]
#[derive(Default)]
struct OwnerLaunchGateState {
    armed: bool,
    arrived: bool,
}

#[cfg(test)]
#[derive(Default)]
struct CallerResumeGate {
    state: Mutex<CallerResumeGateState>,
    changed: Condvar,
}

#[cfg(test)]
#[derive(Default)]
struct CallerResumeGateState {
    armed: bool,
    arrived: bool,
    released: bool,
}

#[cfg(test)]
impl CallerResumeGate {
    fn arm(&self) {
        let mut state = self.state.lock().expect("caller resume gate");
        state.armed = true;
        state.arrived = false;
        state.released = false;
    }

    fn claim(&self) -> bool {
        let mut state = self.state.lock().expect("caller resume gate");
        let claimed = state.armed;
        state.armed = false;
        claimed
    }

    fn pause(&self) {
        let mut state = self.state.lock().expect("caller resume gate");
        state.arrived = true;
        self.changed.notify_all();
        while !state.released {
            state = self.changed.wait(state).expect("caller resume gate");
        }
    }

    fn wait_until_arrived(&self) -> bool {
        self.changed
            .wait_timeout_while(
                self.state.lock().expect("caller resume gate"),
                Duration::from_secs(2),
                |state| !state.arrived,
            )
            .map(|(state, timeout)| state.arrived && !timeout.timed_out())
            .unwrap_or(false)
    }

    fn release(&self) {
        let mut state = self.state.lock().expect("caller resume gate");
        state.released = true;
        self.changed.notify_all();
    }
}

#[cfg(test)]
impl OwnerLaunchGate {
    fn arm(&self) {
        let mut state = self.state.lock().expect("owner launch gate");
        state.armed = true;
        state.arrived = false;
    }

    fn pause_if_armed(&self) {
        let mut state = self.state.lock().expect("owner launch gate");
        if !state.armed {
            return;
        }
        state.arrived = true;
        self.changed.notify_all();
        while state.armed {
            state = self.changed.wait(state).expect("owner launch gate");
        }
    }

    fn wait_until_arrived(&self) -> bool {
        self.changed
            .wait_timeout_while(
                self.state.lock().expect("owner launch gate"),
                Duration::from_secs(2),
                |state| !state.arrived,
            )
            .map(|(state, timeout)| state.arrived && !timeout.timed_out())
            .unwrap_or(false)
    }

    fn release(&self) {
        let mut state = self.state.lock().expect("owner launch gate");
        state.armed = false;
        self.changed.notify_all();
    }
}

#[derive(Clone)]
struct AttemptSettlement {
    physical: Result<ReconciledGateway, GatewayError>,
    reported: Result<(), GatewayError>,
}

impl AttemptReceipt {
    fn pending(request: GatewayReconciliationAttempt) -> Self {
        Self {
            authority: PendingReconciliation::new(request.record().clone()),
            request,
            state: Mutex::new(AttemptState {
                settlement: None,
                waiters: 0,
            }),
            settled: Condvar::new(),
            journal: Mutex::new(JournalState::Pending),
            journal_ready: Condvar::new(),
        }
    }

    fn publish_journal(
        &self,
        journal: Result<Arc<dyn GatewayReconciliationJournalSession>, GatewayError>,
    ) -> Result<(), GatewayError> {
        let mut state = self.journal.lock().map_err(|_| state_unavailable())?;
        *state = match journal {
            Ok(journal) => JournalState::Ready(journal),
            Err(error) => JournalState::Failed(error),
        };
        self.journal_ready.notify_all();
        Ok(())
    }

    fn wait_journal(&self) -> Result<Arc<dyn GatewayReconciliationJournalSession>, GatewayError> {
        let state = self
            .journal_ready
            .wait_while(
                self.journal.lock().map_err(|_| state_unavailable())?,
                |state| matches!(state, JournalState::Pending),
            )
            .map_err(|_| state_unavailable())?;
        match &*state {
            JournalState::Ready(journal) => Ok(journal.clone()),
            JournalState::Failed(error) => Err(error.clone()),
            JournalState::Pending => Err(state_unavailable()),
        }
    }

    fn wait(&self) -> Result<(), GatewayError> {
        self.wait_settlement()?.reported
    }

    fn wait_physical_until(
        &self,
        deadline: Instant,
        clock: &dyn MonotonicClock,
    ) -> Result<ReconciledGateway, GatewayError> {
        let state = self.state.lock().map_err(|_| state_unavailable())?;
        let Some(remaining) = deadline.checked_duration_since(clock.now()) else {
            return Err(GatewayError::Stop(
                "Gateway stop deadline passed while reconciliation was active".into(),
            ));
        };
        let (state, timeout) = self
            .settled
            .wait_timeout_while(state, remaining, |state| state.settlement.is_none())
            .map_err(|_| state_unavailable())?;
        if timeout.timed_out() {
            return Err(GatewayError::Stop(
                "Gateway stop deadline passed while reconciliation was active".into(),
            ));
        }
        state
            .settlement
            .as_ref()
            .ok_or_else(state_unavailable)?
            .physical
            .clone()
    }

    fn wait_settlement(&self) -> Result<AttemptSettlement, GatewayError> {
        let mut state = self.state.lock().map_err(|_| state_unavailable())?;
        state.waiters += 1;
        self.settled.notify_all();
        let settlement = self
            .settled
            .wait_while(state, |state| state.settlement.is_none())
            .map_err(|_| state_unavailable())?;
        settlement
            .settlement
            .as_ref()
            .cloned()
            .ok_or_else(state_unavailable)
    }

    fn settle(&self, result: AttemptSettlement) -> Result<(), GatewayError> {
        self.state
            .lock()
            .map_err(|_| state_unavailable())?
            .settlement = Some(result);
        self.settled.notify_all();
        Ok(())
    }

    #[cfg(test)]
    fn wait_for_waiters(&self, expected: usize) -> bool {
        self.settled
            .wait_timeout_while(
                self.state.lock().expect("attempt state"),
                Duration::from_secs(2),
                |state| state.waiters < expected,
            )
            .map(|(state, timeout)| state.waiters >= expected && !timeout.timed_out())
            .unwrap_or(false)
    }
}

struct StartupProgress {
    #[cfg_attr(
        all(not(any(target_os = "macos", target_os = "linux")), not(test)),
        expect(
            dead_code,
            reason = "native reconciliation is supported only on macOS and Linux"
        )
    )]
    lifecycle: Arc<Mutex<Lifecycle>>,
    #[cfg_attr(
        all(not(any(target_os = "macos", target_os = "linux")), not(test)),
        expect(
            dead_code,
            reason = "native reconciliation is supported only on macOS and Linux"
        )
    )]
    attempt: Arc<AttemptReceipt>,
    #[cfg_attr(
        all(not(any(target_os = "macos", target_os = "linux")), not(test)),
        expect(
            dead_code,
            reason = "native reconciliation is supported only on macOS and Linux"
        )
    )]
    events: Arc<dyn GatewayStartupEvents>,
    #[cfg_attr(
        all(not(any(target_os = "macos", target_os = "linux")), not(test)),
        expect(
            dead_code,
            reason = "native reconciliation is supported only on macOS and Linux"
        )
    )]
    journal: Arc<dyn GatewayReconciliationJournalSession>,
    state: Arc<Mutex<ProgressState>>,
}

#[derive(Clone)]
enum IntentAdmission {
    Vacant,
    Reserved(GatewayReconciliationIntent),
    Acknowledged(GatewayReconciliationIntent),
    Failed {
        intent: GatewayReconciliationIntent,
        error: GatewayError,
    },
}

#[derive(Clone)]
struct ProgressState {
    admission: IntentAdmission,
    history: Vec<ReconciliationHistoryFact>,
    effect_timing: GatewayReconciliationEffectTiming,
    observation_version: u64,
    latest_observation: Option<LifecycleObservation>,
    pending_observation: Option<(LifecycleObservationSource, LifecycleObservation)>,
    failed_phase: Option<LifecycleFailedPhase>,
    /// The step the adapter last reported, shown whenever this attempt's
    /// projection is starting.
    step: StartupStep,
}

impl GatewayReconciliationProgress for StartupProgress {
    fn step_started(&self, step: StartupStep) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        state.step = step;
        drop(state);
        let starting = self.lifecycle.lock().ok().and_then(|mut current| {
            let owns_current = current
                .running
                .as_ref()
                .is_some_and(|attempt| Arc::ptr_eq(attempt, &self.attempt));
            match current.startup.phase() {
                GatewayStartupPhase::Starting(shown) if owns_current && *shown != step => {
                    current.startup = current.startup.next(GatewayStartupPhase::Starting(step));
                    Some(current.startup.clone())
                }
                _ => None,
            }
        });
        if let Some(starting) = &starting {
            publish(&self.events, starting);
        }
    }

    fn readiness_invalidated(&self) {
        let step = self
            .state
            .lock()
            .map(|state| state.step)
            .unwrap_or(StartupStep::Preparing);
        let starting = self.lifecycle.lock().ok().and_then(|mut current| {
            let owns_current = current
                .running
                .as_ref()
                .is_some_and(|attempt| Arc::ptr_eq(attempt, &self.attempt));
            if !owns_current || !matches!(current.startup.phase(), GatewayStartupPhase::Ready) {
                return None;
            }
            current.startup = current.startup.next(GatewayStartupPhase::Starting(step));
            Some(current.startup.clone())
        });
        if let Some(starting) = &starting {
            publish(&self.events, starting);
        }
    }

    fn intent_admitted(&self, intent: GatewayReconciliationIntent) -> Result<(), GatewayError> {
        if intent.attempt() != &self.attempt.request {
            return Err(GatewayError::Registration(
                "gateway host admitted intent for a different attempt".into(),
            ));
        }
        {
            let mut state = self.state.lock().map_err(|_| state_unavailable())?;
            if !matches!(&state.admission, IntentAdmission::Vacant) {
                return Err(GatewayError::Registration(
                    "gateway host admitted more than one intent for an attempt".into(),
                ));
            }
            state.admission = IntentAdmission::Reserved(intent.clone());
        }
        let delivery = match catch_unwind(AssertUnwindSafe(|| {
            retry_delivery(|| self.journal.intent(&intent))
        })) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(GatewayError::Audit {
                audit: error.to_string(),
                physical: None,
            }),
            Err(_) => Err(GatewayError::Audit {
                audit: "gateway reconciliation audit adapter panicked".into(),
                physical: None,
            }),
        };
        let mut state = self.state.lock().map_err(|_| state_unavailable())?;
        state.admission = match &delivery {
            Ok(()) => IntentAdmission::Acknowledged(intent),
            Err(error) => IntentAdmission::Failed {
                intent,
                error: error.clone(),
            },
        };
        delivery
    }

    fn history_observed(&self, fact: ReconciliationHistoryFact) {
        if let Ok(mut state) = self.state.lock() {
            if matches!(
                state.effect_timing,
                GatewayReconciliationEffectTiming::NoEffectsObserved
            ) {
                state.effect_timing = match &state.admission {
                    IntentAdmission::Vacant => {
                        GatewayReconciliationEffectTiming::BeforeIntentReservation
                    }
                    IntentAdmission::Reserved(_) | IntentAdmission::Failed { .. } => {
                        GatewayReconciliationEffectTiming::BeforeIntentAcknowledgement
                    }
                    IntentAdmission::Acknowledged(_) => {
                        GatewayReconciliationEffectTiming::AfterIntentAcknowledgement
                    }
                };
            }
            state.history.push(fact);
        }
    }

    fn effect_planned(
        &self,
        plan_id: &str,
        primary: &LifecyclePlanStep,
        cleanup: &[LifecyclePlanStep],
    ) -> Result<AuditDeliveryReceipt, GatewayError> {
        let (before, target) = {
            let state = self.state.lock().map_err(|_| state_unavailable())?;
            let intent = match &state.admission {
                IntentAdmission::Acknowledged(intent) => intent,
                _ => {
                    return Err(GatewayError::Registration(
                        "Gateway effect was planned before intent acknowledgement".into(),
                    ))
                }
            };
            let before = match &state.latest_observation {
                Some(observation) => observation.incarnation().cloned(),
                None => intent.before().cloned(),
            };
            (before, intent.target().clone())
        };
        let result = retry_delivery(|| {
            self.journal
                .effect_plan(plan_id, before.as_ref(), &target, primary, cleanup)
        });
        if result.is_err() {
            self.state
                .lock()
                .map_err(|_| state_unavailable())?
                .failed_phase = Some(LifecycleFailedPhase::Planning);
        }
        result
    }

    fn effect_completed(
        &self,
        plan_id: &str,
        step_id: &str,
        result: &LifecycleCommandResult,
    ) -> Result<AuditDeliveryReceipt, GatewayError> {
        let delivery = retry_delivery(|| self.journal.effect_completion(plan_id, step_id, result));
        if delivery.is_err() {
            self.state
                .lock()
                .map_err(|_| state_unavailable())?
                .failed_phase = Some(LifecycleFailedPhase::NativeCompletionDelivery);
        }
        delivery
    }

    fn native_attempt_recorded(
        &self,
        plan_id: &str,
        step_id: &str,
        attempt: &crate::gateway::domain::value_objects::SystemdJobAttempt,
    ) -> Result<AuditDeliveryReceipt, GatewayError> {
        let delivery = retry_delivery(|| self.journal.native_attempt(plan_id, step_id, attempt));
        if delivery.is_err() {
            self.state
                .lock()
                .map_err(|_| state_unavailable())?
                .failed_phase = Some(LifecycleFailedPhase::NativeCompletionDelivery);
        }
        delivery
    }

    fn physical_observed(
        &self,
        source: &LifecycleObservationSource,
        incarnation: Option<ReconciliationIncarnation>,
        target_artifact_present: bool,
    ) -> Result<LifecycleObservation, GatewayError> {
        let observation = {
            let mut state = self.state.lock().map_err(|_| state_unavailable())?;
            if state.pending_observation.is_some() {
                return Err(GatewayError::Registration(
                    "A gateway lifecycle observation still awaits exact acknowledgement".into(),
                ));
            }
            state.observation_version = state.observation_version.saturating_add(1);
            let observation = LifecycleObservation::new(
                state.observation_version,
                incarnation,
                target_artifact_present,
            );
            state.pending_observation = Some((source.clone(), observation.clone()));
            observation
        };
        if let Err(error) = retry_delivery(|| self.journal.observation(source, &observation)) {
            self.state
                .lock()
                .map_err(|_| state_unavailable())?
                .failed_phase = Some(LifecycleFailedPhase::Observation);
            return Err(error);
        }
        let mut state = self.state.lock().map_err(|_| state_unavailable())?;
        state.pending_observation = None;
        state.latest_observation = Some(observation.clone());
        Ok(observation)
    }

    fn systemd_observed(
        &self,
        source: &LifecycleObservationSource,
        incarnation: ReconciliationIncarnation,
        target_artifact_present: bool,
        native: crate::gateway::domain::value_objects::SystemdRuntimeObservation,
    ) -> Result<LifecycleObservation, GatewayError> {
        let observation = {
            let mut state = self.state.lock().map_err(|_| state_unavailable())?;
            if state.pending_observation.is_some() {
                return Err(GatewayError::Registration(
                    "A gateway lifecycle observation still awaits exact acknowledgement".into(),
                ));
            }
            state.observation_version = state.observation_version.saturating_add(1);
            let observation = LifecycleObservation::with_systemd(
                state.observation_version,
                incarnation,
                target_artifact_present,
                native,
            )
            .map_err(|error| GatewayError::Registration(error.to_string()))?;
            state.pending_observation = Some((source.clone(), observation.clone()));
            observation
        };
        if let Err(error) = retry_delivery(|| self.journal.observation(source, &observation)) {
            self.state
                .lock()
                .map_err(|_| state_unavailable())?
                .failed_phase = Some(LifecycleFailedPhase::Observation);
            return Err(error);
        }
        let mut state = self.state.lock().map_err(|_| state_unavailable())?;
        state.pending_observation = None;
        state.latest_observation = Some(observation.clone());
        Ok(observation)
    }

    fn systemd_state_observed(
        &self,
        source: &LifecycleObservationSource,
        target_artifact_present: bool,
        systemd_state: crate::gateway::domain::value_objects::SystemdUnitState,
    ) -> Result<LifecycleObservation, GatewayError> {
        let observation = {
            let mut state = self.state.lock().map_err(|_| state_unavailable())?;
            if state.pending_observation.is_some() {
                return Err(GatewayError::Registration(
                    "A gateway lifecycle observation still awaits exact acknowledgement".into(),
                ));
            }
            state.observation_version = state.observation_version.saturating_add(1);
            let observation = LifecycleObservation::with_systemd_state(
                state.observation_version,
                target_artifact_present,
                systemd_state,
            )
            .map_err(|error| GatewayError::Registration(error.to_string()))?;
            state.pending_observation = Some((source.clone(), observation.clone()));
            observation
        };
        if let Err(error) = retry_delivery(|| self.journal.observation(source, &observation)) {
            self.state
                .lock()
                .map_err(|_| state_unavailable())?
                .failed_phase = Some(LifecycleFailedPhase::Observation);
            return Err(error);
        }
        let mut state = self.state.lock().map_err(|_| state_unavailable())?;
        state.pending_observation = None;
        state.latest_observation = Some(observation.clone());
        Ok(observation)
    }
}

fn retry_delivery<T, E>(mut deliver: impl FnMut() -> Result<T, E>) -> Result<T, E> {
    match deliver() {
        Ok(receipt) => Ok(receipt),
        Err(_) => deliver(),
    }
}

fn retry_outcome_delivery(
    mut deliver: impl FnMut() -> Result<(), GatewayReconciliationOutcomeError>,
) -> Result<(), GatewayReconciliationOutcomeError> {
    match deliver() {
        Err(error @ GatewayReconciliationOutcomeError::Rejected(_)) => Err(error),
        Ok(()) => Ok(()),
        Err(GatewayReconciliationOutcomeError::Delivery(_)) => deliver(),
    }
}

fn deliver_before_stop_effect<T>(
    deliver: impl FnMut() -> Result<T, GatewayError>,
) -> Result<T, GatewayError> {
    match catch_unwind(AssertUnwindSafe(|| retry_delivery(deliver))) {
        Ok(Ok(receipt)) => Ok(receipt),
        Ok(Err(error)) => Err(GatewayError::Audit {
            audit: error.to_string(),
            physical: None,
        }),
        Err(_) => Err(GatewayError::Audit {
            audit: "gateway stop audit adapter panicked".into(),
            physical: None,
        }),
    }
}

enum StopOutcomeAttemptFailure {
    Returned(GatewayError),
    Panicked,
}

impl StopOutcomeAttemptFailure {
    fn description(&self) -> String {
        match self {
            Self::Returned(error) => error.to_string(),
            Self::Panicked => "gateway stop outcome audit adapter panicked".into(),
        }
    }
}

enum StopOutcomeRetryFailure {
    Denied(GatewayError),
    Attempt(StopOutcomeAttemptFailure),
}

struct StopOutcomeDeliveryFailure {
    first: StopOutcomeAttemptFailure,
    retry: StopOutcomeRetryFailure,
    physical: GatewayPhysicalResult,
}

impl StopOutcomeDeliveryFailure {
    fn into_gateway_error(self) -> GatewayError {
        let retry = match self.retry {
            StopOutcomeRetryFailure::Denied(error) => {
                format!("exact retry was denied: {error}")
            }
            StopOutcomeRetryFailure::Attempt(error) => {
                format!("exact retry failed: {}", error.description())
            }
        };
        GatewayError::Audit {
            audit: format!(
                "gateway stop outcome delivery failed: first attempt failed: {}; {retry}",
                self.first.description()
            ),
            physical: Some(self.physical),
        }
    }
}

fn invoke_stop_outcome(
    deliver: &mut impl FnMut() -> Result<AuditDeliveryReceipt, GatewayError>,
) -> Result<AuditDeliveryReceipt, StopOutcomeAttemptFailure> {
    match catch_unwind(AssertUnwindSafe(deliver)) {
        Ok(Ok(receipt)) => Ok(receipt),
        Ok(Err(error)) => Err(StopOutcomeAttemptFailure::Returned(error)),
        Err(_) => Err(StopOutcomeAttemptFailure::Panicked),
    }
}

fn deliver_stop_outcome(
    session: &GatewayStopSession,
    physical: GatewayPhysicalResult,
    mut deliver: impl FnMut() -> Result<AuditDeliveryReceipt, GatewayError>,
) -> Result<AuditDeliveryReceipt, GatewayError> {
    if let Err(authority) = session.begin_outcome_delivery() {
        return Err(match physical {
            GatewayPhysicalResult::Succeeded => authority,
            GatewayPhysicalResult::Failed(error) => *error,
        });
    }
    match invoke_stop_outcome(&mut deliver) {
        Ok(receipt) => Ok(receipt),
        Err(first) => {
            let retry = match session.begin_outcome_delivery() {
                Ok(()) => match invoke_stop_outcome(&mut deliver) {
                    Ok(receipt) => return Ok(receipt),
                    Err(error) => StopOutcomeRetryFailure::Attempt(error),
                },
                Err(error) => StopOutcomeRetryFailure::Denied(error),
            };
            Err(StopOutcomeDeliveryFailure {
                first,
                retry,
                physical,
            }
            .into_gateway_error())
        }
    }
}

pub struct Gateway {
    host: Arc<dyn GatewayHost>,
    login_shell: Arc<dyn LoginShellPath>,
    startup_events: Arc<dyn GatewayStartupEvents>,
    reconciliation_ids: Arc<dyn GatewayReconciliationIds>,
    reconciliation_audit: Arc<dyn GatewayReconciliationAudit>,
    clock: Arc<dyn MonotonicClock>,
    /// The agent's search path, resolved at most once for the life of this
    /// host process — the outcome, so a shell that could not be read is
    /// remembered as such rather than asked again.
    ///
    /// `wait_ready` runs on every webview load, not once per launch, and both
    /// consequences of resolving there matter. A login shell would be spawned,
    /// with its whole deadline available to it, in front of every panel. Worse,
    /// a profile edited while Nessa is open would resolve differently on the
    /// next load, and a service definition that differs is one that retires the
    /// running gateway and stops its agents — mid-session, with nobody having
    /// asked. The path is fixed when it is first needed; a change to it takes
    /// effect on the next launch of the app, which is when the re-registration
    /// it causes is something the user is expecting.
    agent_path: Arc<OnceLock<Option<SearchPath>>>,
    runtime: PathBuf,
    stage: String,
    lifecycle: Arc<Mutex<Lifecycle>>,
    #[cfg(test)]
    lifecycle_probe: Arc<LifecycleProbe>,
    #[cfg(test)]
    owner_launch_gate: Arc<OwnerLaunchGate>,
    #[cfg(test)]
    caller_resume_gate: Arc<CallerResumeGate>,
}

pub struct GatewayRuntimeDependencies {
    host: Arc<dyn GatewayHost>,
    login_shell: Arc<dyn LoginShellPath>,
    startup_events: Arc<dyn GatewayStartupEvents>,
    reconciliation_ids: Arc<dyn GatewayReconciliationIds>,
    reconciliation_audit: Arc<dyn GatewayReconciliationAudit>,
    clock: Arc<dyn MonotonicClock>,
}

impl GatewayRuntimeDependencies {
    pub fn new(
        host: Arc<dyn GatewayHost>,
        login_shell: Arc<dyn LoginShellPath>,
        startup_events: Arc<dyn GatewayStartupEvents>,
        reconciliation_ids: Arc<dyn GatewayReconciliationIds>,
        reconciliation_audit: Arc<dyn GatewayReconciliationAudit>,
        clock: Arc<dyn MonotonicClock>,
    ) -> Self {
        Self {
            host,
            login_shell,
            startup_events,
            reconciliation_ids,
            reconciliation_audit,
            clock,
        }
    }
}

#[derive(Clone)]
struct ReconciliationExecutor {
    host: Arc<dyn GatewayHost>,
    login_shell: Arc<dyn LoginShellPath>,
    startup_events: Arc<dyn GatewayStartupEvents>,
    reconciliation_audit: Arc<dyn GatewayReconciliationAudit>,
    agent_path: Arc<OnceLock<Option<SearchPath>>>,
    runtime: PathBuf,
    stage: String,
    lifecycle: Arc<Mutex<Lifecycle>>,
    #[cfg(test)]
    lifecycle_probe: Arc<LifecycleProbe>,
    #[cfg(test)]
    owner_launch_gate: Arc<OwnerLaunchGate>,
    #[cfg(test)]
    caller_resume_gate: Arc<CallerResumeGate>,
}

struct AdmittedReceipt {
    receipt: Arc<AttemptReceipt>,
    start_owner: bool,
    starting: Option<GatewayStartup>,
    joined: Option<GatewayReconciliationRequest>,
    #[cfg(test)]
    pause_caller_resume: bool,
}
impl Gateway {
    #[cfg(test)]
    pub fn bootstrap(
        host: Arc<dyn GatewayHost>,
        login_shell: Arc<dyn LoginShellPath>,
        startup_events: Arc<dyn GatewayStartupEvents>,
        reconciliation_ids: Arc<dyn GatewayReconciliationIds>,
        reconciliation_audit: Arc<dyn GatewayReconciliationAudit>,
        runtime: PathBuf,
        stage: String,
    ) -> Self {
        Self::bootstrap_with_dependencies(
            GatewayRuntimeDependencies::new(
                host,
                login_shell,
                startup_events,
                reconciliation_ids,
                reconciliation_audit,
                Arc::new(SystemMonotonicClock),
            ),
            runtime,
            stage,
        )
    }

    pub fn bootstrap_with_dependencies(
        dependencies: GatewayRuntimeDependencies,
        runtime: PathBuf,
        stage: String,
    ) -> Self {
        let GatewayRuntimeDependencies {
            host,
            login_shell,
            startup_events,
            reconciliation_ids,
            reconciliation_audit,
            clock,
        } = dependencies;
        Self {
            host,
            login_shell,
            startup_events,
            reconciliation_ids,
            reconciliation_audit,
            clock,
            agent_path: Arc::new(OnceLock::new()),
            runtime,
            stage,
            lifecycle: Arc::new(Mutex::new(Lifecycle {
                startup: GatewayStartup::starting(),
                retained_gateway: None,
                ready_gateway: None,
                running: None,
                pending: None,
            })),
            #[cfg(test)]
            lifecycle_probe: Arc::new(LifecycleProbe::default()),
            #[cfg(test)]
            owner_launch_gate: Arc::new(OwnerLaunchGate::default()),
            #[cfg(test)]
            caller_resume_gate: Arc::new(CallerResumeGate::default()),
        }
    }

    /// The latest host-owned startup projection.
    pub fn startup(&self) -> Result<GatewayStartup, GatewayError> {
        self.lifecycle
            .lock()
            .map(|lifecycle| lifecycle.startup.clone())
            .map_err(|_| GatewayError::Registration("gateway startup state is unavailable".into()))
    }

    pub async fn start(&self) -> Result<(), GatewayError> {
        let cause = catch_unwind(AssertUnwindSafe(|| self.host.startup_cause())).map_err(|_| {
            GatewayError::Registration("gateway host startup classification panicked".into())
        })?;
        self.reconcile(evidence(cause, ReconciliationInitiator::DesktopHost)?)
            .await
    }

    pub async fn wait_ready(&self, surface: BundledSurface) -> Result<(), GatewayError> {
        self.reconcile(evidence(
            ReconciliationCause::CredentialLoad,
            ReconciliationInitiator::BundledSurface(surface),
        )?)
        .await
    }

    /// Explicitly retries a failed startup. A retry that arrives while another
    /// reconciliation is running joins that work and never starts a second one.
    pub async fn retry(&self, surface: BundledSurface) -> Result<(), GatewayError> {
        self.reconcile(evidence(
            ReconciliationCause::ExplicitRetry,
            ReconciliationInitiator::BundledSurface(surface),
        )?)
        .await
    }

    #[cfg(test)]
    pub async fn configuration_changed(&self, surface: BundledSurface) -> Result<(), GatewayError> {
        self.reconcile(evidence(
            ReconciliationCause::ClaudeConfigurationChanged,
            ReconciliationInitiator::BundledSurface(surface),
        )?)
        .await
    }

    async fn reconcile(&self, evidence: ReconciliationEvidence) -> Result<(), GatewayError> {
        let request_correlation = match allocate_id(self.reconciliation_ids.as_ref()) {
            Ok(correlation) => correlation,
            Err(error) => {
                admission_failed(&self.lifecycle, &self.startup_events, &error);
                return Err(error);
            }
        };
        let request = GatewayReconciliationRequest::new(request_correlation, evidence);
        let attempt_correlation = match allocate_id(self.reconciliation_ids.as_ref()) {
            Ok(correlation) => correlation,
            Err(error) => {
                admission_failed(&self.lifecycle, &self.startup_events, &error);
                return Err(error);
            }
        };
        let attempt = match GatewayReconciliationAttempt::new(attempt_correlation, request) {
            Ok(attempt) => attempt,
            Err(error) => {
                admission_failed(&self.lifecycle, &self.startup_events, &error);
                return Err(error);
            }
        };
        let receipt = Arc::new(AttemptReceipt::pending(attempt));
        let lifecycle = self.lifecycle.clone();
        let startup_events = self.startup_events.clone();
        let executor = ReconciliationExecutor {
            host: self.host.clone(),
            login_shell: self.login_shell.clone(),
            startup_events: self.startup_events.clone(),
            reconciliation_audit: self.reconciliation_audit.clone(),
            agent_path: self.agent_path.clone(),
            runtime: self.runtime.clone(),
            stage: self.stage.clone(),
            lifecycle: self.lifecycle.clone(),
            #[cfg(test)]
            lifecycle_probe: self.lifecycle_probe.clone(),
            #[cfg(test)]
            owner_launch_gate: self.owner_launch_gate.clone(),
            #[cfg(test)]
            caller_resume_gate: self.caller_resume_gate.clone(),
        };
        let admitted = tauri::async_runtime::spawn_blocking(move || {
            let admitted = admit_request(&lifecycle, receipt)?;
            #[cfg(test)]
            let admitted = {
                let mut admitted = admitted;
                admitted.pause_caller_resume = executor.caller_resume_gate.claim();
                admitted
            };
            #[cfg(test)]
            executor.lifecycle_probe.notify();
            if admitted.start_owner {
                #[cfg(test)]
                executor.owner_launch_gate.pause_if_armed();
                let owner = admitted.receipt.clone();
                let _owner =
                    tauri::async_runtime::spawn_blocking(move || execute_chain(&executor, owner));
            }
            if let Some(starting) = &admitted.starting {
                publish(&startup_events, starting);
            }
            if let Some(joined) = &admitted.joined {
                audit_attached(&admitted.receipt, joined)?;
            }
            Ok::<_, GatewayError>(admitted)
        })
        .await
        .map_err(|error| GatewayError::Registration(error.to_string()))??;

        #[cfg(test)]
        let pause_caller_resume = admitted.pause_caller_resume;
        let receipt = admitted.receipt;
        let result = tauri::async_runtime::spawn_blocking(move || receipt.wait())
            .await
            .map_err(|error| GatewayError::Registration(error.to_string()))?;
        #[cfg(test)]
        if pause_caller_resume {
            self.caller_resume_gate.pause();
        }
        result
    }
    pub fn stop_agents(&self, deadline: Instant) -> Result<(), GatewayError> {
        let gateway = loop {
            let lifecycle = self
                .lifecycle
                .lock()
                .map_err(|_| GatewayError::NotReconciled)?;
            let Some(attempt) = lifecycle.running.clone() else {
                break lifecycle
                    .retained_gateway
                    .clone()
                    .ok_or(GatewayError::NotReconciled)?;
            };
            drop(lifecycle);
            attempt.wait_physical_until(deadline, self.clock.as_ref())?;
        };
        let origin = GatewayReconciliationRequest::new(
            allocate_id(self.reconciliation_ids.as_ref())?,
            evidence(
                ReconciliationCause::DesktopQuitPolicy,
                ReconciliationInitiator::DesktopHost,
            )?,
        );
        let attempt = GatewayReconciliationAttempt::new(
            allocate_id(self.reconciliation_ids.as_ref())?,
            origin,
        )?;
        let intended = gateway.audit_identity()?;
        let request = GatewayStopRequest::new(attempt.clone(), gateway, deadline);
        let session = Arc::new(GatewayStopSession::new(request, self.clock.clone()));
        let (receiver, _worker) = spawn_stop_request(
            self.host.clone(),
            self.reconciliation_audit.clone(),
            session.clone(),
            attempt,
            intended,
        );
        let Some(remaining) = deadline.checked_duration_since(self.clock.now()) else {
            session.expire_at_deadline();
            return Err(GatewayError::Stop(
                "Gateway stop deadline passed before dispatch".into(),
            ));
        };
        match receiver.recv_timeout(remaining) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                session.expire_at_deadline();
                Err(GatewayError::Stop(
                    "Gateway stop did not settle before the desktop quit deadline".into(),
                ))
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(GatewayError::Stop(
                "Gateway stop worker ended without settlement".into(),
            )),
        }
    }

    #[cfg(test)]
    fn wait_for_pending_receipt(&self) -> Option<Arc<AttemptReceipt>> {
        self.lifecycle_probe.wait_for_pending(&self.lifecycle)
    }
}

fn spawn_stop_request(
    host: Arc<dyn GatewayHost>,
    audit: Arc<dyn GatewayReconciliationAudit>,
    session: Arc<GatewayStopSession>,
    attempt: GatewayReconciliationAttempt,
    intended: ReconciliationIncarnation,
) -> (
    mpsc::Receiver<Result<(), GatewayError>>,
    thread::JoinHandle<()>,
) {
    let (answer, receiver) = mpsc::sync_channel(1);
    let worker = thread::spawn(move || {
        let result = execute_stop_request(host, audit, session, attempt, intended);
        let _ = answer.send(result);
    });
    (receiver, worker)
}

fn execute_stop_request(
    host: Arc<dyn GatewayHost>,
    audit: Arc<dyn GatewayReconciliationAudit>,
    session: Arc<GatewayStopSession>,
    attempt: GatewayReconciliationAttempt,
    intended: ReconciliationIncarnation,
) -> Result<(), GatewayError> {
    let target = intended.target().clone();
    let journal = audit
        .clone()
        .open(&attempt, Some(session.request().deadline()))?;
    let intent = GatewayReconciliationIntent::new(attempt, target.clone(), Some(intended.clone()))?;
    deliver_before_stop_effect(|| journal.intent(&intent))?;
    let primary = LifecyclePlanStep::new(
        "signal-agents".into(),
        LifecycleEffect::StopAgents {
            incarnation: intended.clone(),
        },
        LifecycleEffectPredicate::Always,
    )
    .map_err(|error| GatewayError::Stop(error.to_string()))?;
    let plan = deliver_before_stop_effect(|| {
        journal.effect_plan(
            "stop-agents-on-desktop-quit",
            Some(&intended),
            &target,
            &primary,
            &[],
        )
    })?;
    let result = host.stop_agents(&session, journal.as_ref(), &plan);
    match result {
        Ok(observation) => {
            let (command, observed) = session.settlement()?;
            debug_assert_eq!(observation, observed);
            let physical = LifecyclePhysicalOutcome::StopAgentsSettled {
                intended,
                command,
                observed: observation,
            };
            deliver_stop_outcome(session.as_ref(), GatewayPhysicalResult::Succeeded, || {
                journal.physical_outcome(
                    &physical,
                    Some(&observed),
                    ReconciliationCleanupDecision::RetainPrior,
                )
            })?;
            Ok(())
        }
        Err(error) => {
            session.expire_at_deadline();
            let Ok((_, last_confirmed)) = session.settlement() else {
                return Err(error);
            };
            let physical = LifecyclePhysicalOutcome::Failed {
                phase: LifecycleFailedPhase::NativeDispatch,
                message: error.to_string(),
            };
            let outcome = deliver_stop_outcome(
                session.as_ref(),
                GatewayPhysicalResult::Failed(Box::new(error.clone())),
                || {
                    journal.physical_outcome(
                        &physical,
                        Some(&last_confirmed),
                        ReconciliationCleanupDecision::RetainPrior,
                    )
                },
            );
            outcome.and(Err(error))
        }
    }
}

fn admit_request(
    lifecycle: &Arc<Mutex<Lifecycle>>,
    receipt: Arc<AttemptReceipt>,
) -> Result<AdmittedReceipt, GatewayError> {
    let configuration_change = receipt.request.origin().evidence().cause()
        == ReconciliationCause::ClaudeConfigurationChanged;
    let mut current = lifecycle.lock().map_err(|_| state_unavailable())?;
    let (selected, start_owner, starting) = if let Some(pending) = current.pending.clone() {
        (pending, false, None)
    } else if current.running.is_some() {
        if configuration_change {
            current.pending = Some(receipt.clone());
            (receipt.clone(), false, None)
        } else {
            (
                current.running.clone().ok_or_else(state_unavailable)?,
                false,
                None,
            )
        }
    } else {
        current.running = Some(receipt.clone());
        let starting =
            matches!(current.startup.phase(), GatewayStartupPhase::Failed(_)).then(|| {
                current.startup = current
                    .startup
                    .next(GatewayStartupPhase::Starting(StartupStep::Preparing));
                current.startup.clone()
            });
        (receipt.clone(), true, starting)
    };
    let joined = !Arc::ptr_eq(&selected, &receipt);
    let joined_request = joined.then(|| receipt.request.origin().clone());
    Ok(AdmittedReceipt {
        receipt: selected,
        start_owner,
        starting,
        joined: joined_request,
        #[cfg(test)]
        pause_caller_resume: false,
    })
}

fn allocate_id(
    ids: &dyn GatewayReconciliationIds,
) -> Result<ReconciliationCorrelation, GatewayError> {
    catch_unwind(AssertUnwindSafe(|| ids.next())).unwrap_or_else(|_| {
        Err(GatewayError::Registration(
            "gateway reconciliation id adapter panicked".into(),
        ))
    })
}

fn publish(events: &Arc<dyn GatewayStartupEvents>, startup: &GatewayStartup) {
    if catch_unwind(AssertUnwindSafe(|| events.publish(startup))).is_err() {
        eprintln!("[nessa] gateway startup event adapter panicked");
    }
}

fn admission_failed(
    lifecycle: &Arc<Mutex<Lifecycle>>,
    events: &Arc<dyn GatewayStartupEvents>,
    error: &GatewayError,
) {
    let failed = lifecycle.lock().ok().and_then(|mut current| {
        if current.running.is_some() {
            return None;
        }
        current.startup = current
            .startup
            .next(GatewayStartupPhase::Failed(error.clone()));
        Some(current.startup.clone())
    });
    if let Some(failed) = &failed {
        publish(events, failed);
    }
}

fn evidence(
    cause: ReconciliationCause,
    initiator: ReconciliationInitiator,
) -> Result<ReconciliationEvidence, GatewayError> {
    ReconciliationEvidence::new(cause, initiator)
        .map_err(|error| GatewayError::Registration(error.to_string()))
}

fn audit_attached(
    receipt: &Arc<AttemptReceipt>,
    joined: &GatewayReconciliationRequest,
) -> Result<(), GatewayError> {
    let journal = receipt.wait_journal()?;
    match catch_unwind(AssertUnwindSafe(|| journal.joined(joined))) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(GatewayError::Audit {
            audit: error.to_string(),
            physical: None,
        }),
        Err(_) => Err(GatewayError::Audit {
            audit: "gateway reconciliation audit adapter panicked".into(),
            physical: None,
        }),
    }
}

fn intent_delivery_report(
    delivery: &GatewayReconciliationIntentDelivery,
    physical: Result<(), GatewayError>,
) -> Result<(), GatewayError> {
    let GatewayReconciliationIntentDelivery::Failed(delivery_error) = delivery else {
        return physical;
    };
    let physical_error = physical.err();
    if physical_error.as_ref() == Some(delivery_error) {
        return Err(delivery_error.clone());
    }
    let audit = match delivery_error {
        GatewayError::Audit { audit, .. } => audit.clone(),
        error => error.to_string(),
    };
    Err(GatewayError::Audit {
        audit,
        physical: physical_error.map(|error| GatewayPhysicalResult::Failed(Box::new(error))),
    })
}

fn state_unavailable() -> GatewayError {
    GatewayError::Registration("gateway startup state is unavailable".into())
}

enum CleanupUpdate {
    Keep,
    Replace(ReconciledGateway),
    Clear,
}

struct AttemptExecution {
    physical: Result<ReconciledGateway, GatewayError>,
    reported: Result<(), GatewayError>,
    cleanup: CleanupUpdate,
    ready_gateway: Option<ReconciledGateway>,
}

fn execute_chain(executor: &ReconciliationExecutor, mut receipt: Arc<AttemptReceipt>) {
    loop {
        let execution = catch_unwind(AssertUnwindSafe(|| execute_attempt(executor, &receipt)))
            .unwrap_or_else(|_| AttemptExecution {
                physical: Err(GatewayError::Registration(
                    "gateway reconciliation owner panicked".into(),
                )),
                reported: Err(GatewayError::Registration(
                    "gateway reconciliation owner panicked".into(),
                )),
                cleanup: CleanupUpdate::Keep,
                ready_gateway: None,
            });
        match settle_and_promote(executor, &receipt, execution) {
            Some(next) => receipt = next,
            None => return,
        }
    }
}

fn execute_attempt(
    executor: &ReconciliationExecutor,
    receipt: &Arc<AttemptReceipt>,
) -> AttemptExecution {
    let open_journal = || match catch_unwind(AssertUnwindSafe(|| {
        executor
            .reconciliation_audit
            .clone()
            .open(&receipt.request, None)
    })) {
        Ok(result) => result.map_err(|error| GatewayError::Audit {
            audit: error.to_string(),
            physical: None,
        }),
        Err(_) => Err(GatewayError::Audit {
            audit: "gateway reconciliation journal adapter panicked".into(),
            physical: None,
        }),
    };
    let mut journal = open_journal();
    let restored = journal.as_ref().ok().cloned();
    if let Some(recovery) = restored.as_ref().and_then(|session| session.recovery()) {
        if let Some(restored) = restored {
            let recovery_result = catch_unwind(AssertUnwindSafe(|| {
                executor.host.recover(&recovery, restored.as_ref())
            }))
            .unwrap_or_else(|_| {
                Err(GatewayError::Registration(
                    "gateway host panicked during lifecycle recovery".into(),
                ))
            });
            drop(restored);
            drop(journal);
            journal = match recovery_result {
                Ok(()) => open_journal(),
                Err(error) => Err(error),
            };
            if journal
                .as_ref()
                .ok()
                .and_then(|session| session.recovery())
                .is_some()
            {
                journal = Err(GatewayError::Registration(
                    "Gateway lifecycle recovery returned without terminal settlement".into(),
                ));
            }
        }
    }
    if let Err(error) = receipt.publish_journal(journal.clone()) {
        return AttemptExecution {
            physical: Err(error.clone()),
            reported: Err(error),
            cleanup: CleanupUpdate::Keep,
            ready_gateway: None,
        };
    }
    let journal = match journal {
        Ok(journal) => journal,
        Err(error) => {
            return AttemptExecution {
                physical: Err(error.clone()),
                reported: Err(error),
                cleanup: CleanupUpdate::Keep,
                ready_gateway: None,
            }
        }
    };
    let progress = StartupProgress {
        lifecycle: executor.lifecycle.clone(),
        attempt: receipt.clone(),
        events: executor.startup_events.clone(),
        journal: journal.clone(),
        state: Arc::new(Mutex::new(ProgressState {
            admission: IntentAdmission::Vacant,
            history: Vec::new(),
            effect_timing: GatewayReconciliationEffectTiming::NoEffectsObserved,
            observation_version: 0,
            latest_observation: None,
            pending_observation: None,
            failed_phase: None,
            step: StartupStep::Preparing,
        })),
    };
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let agent_path = executor
            .agent_path
            .get_or_init(|| resolve_agent_path(executor.login_shell.as_ref()));
        executor.host.register(
            &executor.runtime,
            &executor.stage,
            agent_path.as_ref(),
            &receipt.request,
            &progress,
        )
    }));
    let mut physical = outcome.unwrap_or_else(|_| {
        Err(GatewayError::Registration(
            "gateway host panicked during reconciliation".into(),
        ))
    });
    let state = progress.state.lock().ok().map(|state| state.clone());
    let admission = state.as_ref().map(|state| state.admission.clone());
    let facts = state
        .as_ref()
        .map(|state| state.history.clone())
        .unwrap_or_default();
    let effect_timing = state
        .as_ref()
        .map(|state| state.effect_timing)
        .unwrap_or(GatewayReconciliationEffectTiming::NoEffectsObserved);
    let mut failed_phase = state
        .as_ref()
        .and_then(|state| state.failed_phase)
        .unwrap_or(LifecycleFailedPhase::NativeDispatch);
    if physical.is_ok() && matches!(&admission, None | Some(IntentAdmission::Vacant)) {
        physical = Err(GatewayError::Registration(
            "gateway host completed without an admitted audit intent".into(),
        ));
    }
    let Some(admission) = admission else {
        let error = physical.clone().err().unwrap_or_else(|| {
            GatewayError::Registration("gateway audit intent is missing".into())
        });
        return AttemptExecution {
            physical,
            reported: Err(error),
            cleanup: CleanupUpdate::Keep,
            ready_gateway: None,
        };
    };
    let (intent, delivery) = match admission {
        IntentAdmission::Vacant => {
            let error = physical.clone().err().unwrap_or_else(|| {
                GatewayError::Registration("gateway audit intent is missing".into())
            });
            return AttemptExecution {
                physical,
                reported: Err(error),
                cleanup: CleanupUpdate::Keep,
                ready_gateway: None,
            };
        }
        IntentAdmission::Reserved(intent) => {
            (intent, GatewayReconciliationIntentDelivery::Reserved)
        }
        IntentAdmission::Acknowledged(intent) => {
            (intent, GatewayReconciliationIntentDelivery::Acknowledged)
        }
        IntentAdmission::Failed { intent, error } => {
            failed_phase = LifecycleFailedPhase::IntentDelivery;
            (intent, GatewayReconciliationIntentDelivery::Failed(error))
        }
    };
    if matches!(
        failed_phase,
        LifecycleFailedPhase::Planning
            | LifecycleFailedPhase::NativeCompletionDelivery
            | LifecycleFailedPhase::Observation
    ) {
        let error = physical
            .clone()
            .err()
            .unwrap_or_else(|| GatewayError::Audit {
                audit: "gateway lifecycle delivery failed with an unresolved effect".into(),
                physical: Some(GatewayPhysicalResult::Succeeded),
            });
        return AttemptExecution {
            physical,
            reported: Err(error),
            cleanup: CleanupUpdate::Keep,
            ready_gateway: None,
        };
    }
    let identity = physical
        .as_ref()
        .map_err(Clone::clone)
        .and_then(ReconciledGateway::audit_identity);
    let audit_outcome = GatewayReconciliationOutcome::assess(
        intent,
        delivery.clone(),
        effect_timing,
        facts,
        failed_phase,
        identity,
    );
    let mut cleanup = match audit_outcome.cleanup() {
        ReconciliationCleanupDecision::RetainPrior => CleanupUpdate::Keep,
        ReconciliationCleanupDecision::ClearPrior => CleanupUpdate::Clear,
        ReconciliationCleanupDecision::AdoptClaimed => physical
            .clone()
            .map(CleanupUpdate::Replace)
            .unwrap_or(CleanupUpdate::Keep),
    };
    let (base_report, ready) = match audit_outcome.effect() {
        GatewayReconciliationEffect::Confirmed { .. } => (Ok(()), true),
        GatewayReconciliationEffect::Failed { error, .. } => (Err(error.clone()), false),
        GatewayReconciliationEffect::RejectedReport { error, .. } => (Err(error.clone()), false),
    };
    let base_report = intent_delivery_report(&delivery, base_report);
    let audit_result = catch_unwind(AssertUnwindSafe(|| {
        retry_outcome_delivery(|| journal.outcome(&audit_outcome))
    }));
    let reported = match audit_result {
        Ok(Ok(())) => base_report,
        Ok(Err(error)) => {
            if matches!(&error, GatewayReconciliationOutcomeError::Rejected(_)) {
                cleanup = CleanupUpdate::Keep;
            }
            Err(GatewayError::Audit {
                audit: error.error().to_string(),
                physical: Some(match base_report {
                    Ok(()) => GatewayPhysicalResult::Succeeded,
                    Err(error) => GatewayPhysicalResult::Failed(Box::new(error)),
                }),
            })
        }
        Err(_) => Err(GatewayError::Audit {
            audit: "gateway reconciliation audit adapter panicked".into(),
            physical: Some(match base_report {
                Ok(()) => GatewayPhysicalResult::Succeeded,
                Err(error) => GatewayPhysicalResult::Failed(Box::new(error)),
            }),
        }),
    };
    let ready_gateway = if ready && reported.is_ok() {
        physical.clone().ok()
    } else {
        None
    };
    AttemptExecution {
        physical,
        reported,
        cleanup,
        ready_gateway,
    }
}

fn settle_and_promote(
    executor: &ReconciliationExecutor,
    receipt: &Arc<AttemptReceipt>,
    execution: AttemptExecution,
) -> Option<Arc<AttemptReceipt>> {
    let AttemptExecution {
        physical,
        reported: answer,
        cleanup,
        ready_gateway,
    } = execution;
    let settlement = AttemptSettlement {
        physical,
        reported: answer.clone(),
    };
    let Ok(mut current) = executor.lifecycle.lock() else {
        let _ = receipt.settle(settlement);
        return None;
    };
    let owns_running = current
        .running
        .as_ref()
        .is_some_and(|running| Arc::ptr_eq(running, receipt));
    if !owns_running {
        let _ = receipt.settle(settlement);
        return None;
    }
    match cleanup {
        CleanupUpdate::Keep => {}
        CleanupUpdate::Replace(gateway) => current.retained_gateway = Some(gateway),
        CleanupUpdate::Clear => current.retained_gateway = None,
    }
    let phase = if ready_gateway.is_some() {
        GatewayStartupPhase::Ready
    } else {
        GatewayStartupPhase::Failed(answer.clone().err().unwrap_or_else(|| {
            GatewayError::Registration("gateway reconciliation was rejected".into())
        }))
    };
    let unchanged_ready = ready_gateway.as_ref().is_some_and(|gateway| {
        matches!(current.startup.phase(), GatewayStartupPhase::Ready)
            && current.ready_gateway.as_ref() == Some(gateway)
    });
    if let Some(gateway) = ready_gateway {
        current.ready_gateway = Some(gateway);
    }
    let startup = (!unchanged_ready).then(|| {
        current.startup = current.startup.next(phase);
        current.startup.clone()
    });
    current.running = None;
    let next = current.pending.take();
    if let Some(next) = &next {
        if next.authority.is_owned_by(next.request.record()) {
            current.running = Some(next.clone());
        }
    }
    let next = current.running.clone();
    drop(current);
    let _ = receipt.settle(settlement);
    #[cfg(test)]
    executor.lifecycle_probe.notify();
    if let Some(startup) = &startup {
        publish(&executor.startup_events, startup);
    }
    next
}

fn resolve_agent_path(login_shell: &dyn LoginShellPath) -> Option<SearchPath> {
    match login_shell.resolve() {
        Ok(path) => Some(path),
        Err(error) => {
            eprintln!(
                "[nessa] Could not read the login shell's PATH ({error}); the agent keeps the path already registered for its service, or the system path if there is none, until Nessa is launched again"
            );
            None
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/gateway/application.rs"]
mod tests;
