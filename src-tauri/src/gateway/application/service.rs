use super::{
    GatewayError, GatewayHost, GatewayReconciliationAttempt, GatewayReconciliationAudit,
    GatewayReconciliationEffect, GatewayReconciliationIds, GatewayReconciliationIntent,
    GatewayReconciliationOutcome, GatewayReconciliationProgress, GatewayReconciliationRequest,
    GatewayStartup, GatewayStartupEvents, GatewayStartupPhase, LoginShellPath, ReconciledGateway,
    ReconciliationNativeDecision, ReconciliationNativeEffect,
};
use crate::gateway::domain::value_objects::{
    BundledSurface, PendingReconciliation, ReconciliationCause, ReconciliationEvidence,
    ReconciliationInitiator, SearchPath,
};
#[cfg(test)]
use std::time::Duration;
use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    path::PathBuf,
    sync::{Arc, Condvar, Mutex, OnceLock},
};

struct Lifecycle {
    startup: GatewayStartup,
    retained_gateway: Option<ReconciledGateway>,
    attempt: Option<Arc<ReconciliationAttempt>>,
    pending_origin: Option<PendingOrigin>,
}

#[derive(Clone)]
struct PendingOrigin {
    request: GatewayReconciliationRequest,
    authority: PendingReconciliation,
}

impl PendingOrigin {
    fn new(request: GatewayReconciliationRequest) -> Self {
        Self {
            authority: PendingReconciliation::new(request.record().clone()),
            request,
        }
    }

    fn request(&self) -> &GatewayReconciliationRequest {
        &self.request
    }
    fn is_owned_by(&self, attempt: &GatewayReconciliationAttempt) -> bool {
        self.authority.is_owned_by(attempt.record())
    }
}

struct ReconciliationAttempt {
    request: GatewayReconciliationAttempt,
    state: Mutex<AttemptState>,
    settled: Condvar,
}

struct AttemptState {
    settlement: Option<AttemptSettlement>,
    waiters: usize,
}

#[derive(Clone)]
struct AttemptSettlement {
    physical: Result<ReconciledGateway, GatewayError>,
    reported: Result<(), GatewayError>,
}

impl ReconciliationAttempt {
    fn pending(request: GatewayReconciliationAttempt) -> Self {
        Self {
            request,
            state: Mutex::new(AttemptState {
                settlement: None,
                waiters: 0,
            }),
            settled: Condvar::new(),
        }
    }

    fn wait(&self) -> Result<(), GatewayError> {
        self.wait_settlement()
            .map(|settlement| settlement.reported)?
    }

    fn wait_physical(&self) -> Result<ReconciledGateway, GatewayError> {
        self.wait_settlement()
            .map(|settlement| settlement.physical)?
    }

    fn wait_settlement(&self) -> Result<AttemptSettlement, GatewayError> {
        let mut state = self.state.lock().map_err(|_| state_unavailable())?;
        state.waiters += 1;
        self.settled.notify_all();
        let settlement = self
            .settled
            .wait_while(state, |state| state.settlement.is_none())
            .map_err(|_| state_unavailable())?;
        Ok(settlement
            .settlement
            .as_ref()
            .cloned()
            .ok_or_else(state_unavailable)?)
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
    fn wait_for_waiter(&self) -> bool {
        self.settled
            .wait_timeout_while(
                self.state.lock().expect("attempt state"),
                Duration::from_secs(2),
                |state| state.waiters == 0,
            )
            .map(|(state, timeout)| state.waiters > 0 && !timeout.timed_out())
            .unwrap_or(false)
    }
}

struct StartupProgress {
    lifecycle: Arc<Mutex<Lifecycle>>,
    attempt: Arc<ReconciliationAttempt>,
    events: Arc<dyn GatewayStartupEvents>,
    audit: Arc<dyn GatewayReconciliationAudit>,
    intent: Arc<Mutex<Option<GatewayReconciliationIntent>>>,
    decisions: Arc<Mutex<Vec<ReconciliationNativeDecision>>>,
    effects: Arc<Mutex<Vec<ReconciliationNativeEffect>>>,
}

impl GatewayReconciliationProgress for StartupProgress {
    fn readiness_invalidated(&self) {
        let starting = self.lifecycle.lock().ok().and_then(|mut current| {
            let owns_current = current
                .attempt
                .as_ref()
                .is_some_and(|attempt| Arc::ptr_eq(attempt, &self.attempt));
            if !owns_current || !matches!(current.startup.phase(), GatewayStartupPhase::Ready) {
                return None;
            }
            current.startup = current.startup.next(GatewayStartupPhase::Starting);
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
        match catch_unwind(AssertUnwindSafe(|| self.audit.intent(&intent))) {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                return Err(GatewayError::Audit {
                    audit: error.to_string(),
                    physical: None,
                })
            }
            Err(_) => {
                return Err(GatewayError::Audit {
                    audit: "gateway reconciliation audit adapter panicked".into(),
                    physical: None,
                })
            }
        }
        let mut admitted = self.intent.lock().map_err(|_| state_unavailable())?;
        if admitted.replace(intent).is_some() {
            return Err(GatewayError::Registration(
                "gateway host admitted more than one intent for an attempt".into(),
            ));
        }
        Ok(())
    }

    fn effect_observed(&self, effect: ReconciliationNativeEffect) {
        if let Ok(mut effects) = self.effects.lock() {
            effects.push(effect);
        }
    }

    fn decision_observed(&self, decision: ReconciliationNativeDecision) {
        if let Ok(mut decisions) = self.decisions.lock() {
            decisions.push(decision);
        }
    }
}

pub struct Gateway {
    host: Arc<dyn GatewayHost>,
    login_shell: Arc<dyn LoginShellPath>,
    startup_events: Arc<dyn GatewayStartupEvents>,
    reconciliation_ids: Arc<dyn GatewayReconciliationIds>,
    reconciliation_audit: Arc<dyn GatewayReconciliationAudit>,
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
}
impl Gateway {
    pub fn bootstrap(
        host: Arc<dyn GatewayHost>,
        login_shell: Arc<dyn LoginShellPath>,
        startup_events: Arc<dyn GatewayStartupEvents>,
        reconciliation_ids: Arc<dyn GatewayReconciliationIds>,
        reconciliation_audit: Arc<dyn GatewayReconciliationAudit>,
        runtime: PathBuf,
        stage: String,
    ) -> Self {
        Self {
            host,
            login_shell,
            startup_events,
            reconciliation_ids,
            reconciliation_audit,
            agent_path: Arc::new(OnceLock::new()),
            runtime,
            stage,
            lifecycle: Arc::new(Mutex::new(Lifecycle {
                startup: GatewayStartup::starting(),
                retained_gateway: None,
                attempt: None,
                pending_origin: None,
            })),
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
        self.reconcile(evidence(
            ReconciliationCause::Startup,
            ReconciliationInitiator::DesktopHost,
        )?)
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

    pub async fn configuration_changed(&self, surface: BundledSurface) -> Result<(), GatewayError> {
        self.reconcile(evidence(
            ReconciliationCause::ClaudeConfigurationChanged,
            ReconciliationInitiator::BundledSurface(surface),
        )?)
        .await
    }

    async fn reconcile(&self, evidence: ReconciliationEvidence) -> Result<(), GatewayError> {
        let request_correlation = match self.reconciliation_ids.next() {
            Ok(correlation) => correlation,
            Err(error) => {
                admission_failed(&self.lifecycle, &self.startup_events, &error);
                return Err(error);
            }
        };
        let request = GatewayReconciliationRequest::new(request_correlation, evidence);
        let host = self.host.clone();
        let login_shell = self.login_shell.clone();
        let startup_events = self.startup_events.clone();
        let reconciliation_audit = self.reconciliation_audit.clone();
        let reconciliation_ids = self.reconciliation_ids.clone();
        let agent_path = self.agent_path.clone();
        let runtime = self.runtime.clone();
        let stage = self.stage.clone();
        let lifecycle = self.lifecycle.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let mut queued_configuration_change = false;
            let mut current = loop {
                let mut current = lifecycle.lock().map_err(|_| state_unavailable())?;
                let Some(attempt) = current.attempt.clone() else {
                    break current;
                };
                let configuration_change =
                    request.evidence().cause() == ReconciliationCause::ClaudeConfigurationChanged;
                if configuration_change && !queued_configuration_change {
                    current.pending_origin = Some(PendingOrigin::new(request.clone()));
                    queued_configuration_change = true;
                    drop(current);
                    let _ = attempt.wait();
                    continue;
                }
                if attempt.request.origin() == &request {
                    drop(current);
                    return attempt.wait();
                }
                if !configuration_change {
                    if let Some(pending) = current.pending_origin.as_ref() {
                        if attempt.request.origin() == pending.request() {
                            drop(current);
                            audit_attached(&reconciliation_audit, &attempt.request, &request)?;
                            return attempt.wait();
                        }
                        drop(current);
                        let _ = attempt.wait();
                        continue;
                    }
                }
                drop(current);
                audit_attached(&reconciliation_audit, &attempt.request, &request)?;
                return attempt.wait();
            };
            if request.evidence().cause() == ReconciliationCause::ClaudeConfigurationChanged
                && !queued_configuration_change
            {
                current.pending_origin = Some(PendingOrigin::new(request.clone()));
            }
            let origin = current
                .pending_origin
                .as_ref()
                .map(|pending| pending.request().clone())
                .unwrap_or_else(|| request.clone());
            let attempt_correlation = match reconciliation_ids.next() {
                Ok(correlation) => correlation,
                Err(error) => {
                    current.startup = current
                        .startup
                        .next(GatewayStartupPhase::Failed(error.clone()));
                    let failed = current.startup.clone();
                    drop(current);
                    publish(&startup_events, &failed);
                    return Err(error);
                }
            };
            let admitted =
                match GatewayReconciliationAttempt::new(attempt_correlation, origin.clone()) {
                    Ok(attempt) => attempt,
                    Err(error) => {
                        current.startup = current
                            .startup
                            .next(GatewayStartupPhase::Failed(error.clone()));
                        let failed = current.startup.clone();
                        drop(current);
                        publish(&startup_events, &failed);
                        return Err(error);
                    }
                };
            if origin != request {
                audit_attached(&reconciliation_audit, &admitted, &request)?;
            }
            let attempt = Arc::new(ReconciliationAttempt::pending(admitted));
            current.attempt = Some(attempt.clone());
            let announce_starting =
                matches!(current.startup.phase(), GatewayStartupPhase::Failed(_));
            let starting = announce_starting.then(|| {
                current.startup = current.startup.next(GatewayStartupPhase::Starting);
                current.startup.clone()
            });
            drop(current);
            if let Some(starting) = &starting {
                publish(&startup_events, starting);
            }

            let progress = StartupProgress {
                lifecycle: lifecycle.clone(),
                attempt: attempt.clone(),
                events: startup_events.clone(),
                audit: reconciliation_audit.clone(),
                intent: Arc::new(Mutex::new(None)),
                decisions: Arc::new(Mutex::new(Vec::new())),
                effects: Arc::new(Mutex::new(Vec::new())),
            };

            let outcome = catch_unwind(AssertUnwindSafe(|| {
                let agent_path =
                    agent_path.get_or_init(|| resolve_agent_path(login_shell.as_ref()));
                host.register(
                    &runtime,
                    &stage,
                    agent_path.as_ref(),
                    &attempt.request,
                    &progress,
                )
            }));
            let mut physical = outcome.unwrap_or_else(|_| {
                Err(GatewayError::Registration(
                    "gateway host panicked during reconciliation".into(),
                ))
            });
            let intent = progress
                .intent
                .lock()
                .ok()
                .and_then(|intent| intent.clone());
            let effects = progress
                .effects
                .lock()
                .map(|effects| effects.clone())
                .unwrap_or_default();
            let decisions = progress
                .decisions
                .lock()
                .map(|decisions| decisions.clone())
                .unwrap_or_default();
            if physical.is_ok() && intent.is_none() {
                physical = Err(GatewayError::Registration(
                    "gateway host completed without an admitted audit intent".into(),
                ));
            }
            let mut outcome_error = None;
            let audit_outcome = intent.and_then(|intent| {
                let effect = match &physical {
                    Ok(gateway) => match gateway.audit_identity() {
                        Ok(identity) => GatewayReconciliationEffect::Confirmed(identity),
                        Err(error) => {
                            outcome_error = Some(error);
                            return None;
                        }
                    },
                    Err(error) if !effects.is_empty() => GatewayReconciliationEffect::Partial {
                        decisions: decisions.clone(),
                        effects: effects.clone(),
                        error: error.clone(),
                    },
                    Err(error) => GatewayReconciliationEffect::Refused {
                        decisions: decisions.clone(),
                        error: error.clone(),
                    },
                };
                match GatewayReconciliationOutcome::new(intent.clone(), effect) {
                    Ok(outcome) => Some(outcome),
                    Err(error) => {
                        outcome_error = Some(error);
                        None
                    }
                }
            });
            let audit_result = audit_outcome.as_ref().map(|audit_outcome| {
                catch_unwind(AssertUnwindSafe(|| {
                    reconciliation_audit.outcome(audit_outcome)
                }))
            });
            let reported = match (outcome_error, audit_result) {
                (Some(error), _) => Err(GatewayError::Audit {
                    audit: error.to_string(),
                    physical: physical.clone().err().map(Box::new),
                }),
                (None, None) => physical.clone().map(|_| ()),
                (None, Some(Ok(Ok(())))) => physical.clone().map(|_| ()),
                (None, Some(Ok(Err(error)))) => Err(GatewayError::Audit {
                    audit: error.to_string(),
                    physical: physical.clone().err().map(Box::new),
                }),
                (None, Some(Err(_))) => Err(GatewayError::Audit {
                    audit: "gateway reconciliation audit adapter panicked".into(),
                    physical: physical.clone().err().map(Box::new),
                }),
            };
            finish(
                &lifecycle,
                &attempt,
                &startup_events,
                physical,
                reported,
                &effects,
            )
        })
        .await
        .map_err(|error| GatewayError::Registration(error.to_string()))?
    }
    pub fn stop_agents(&self) -> Result<(), GatewayError> {
        let gateway = loop {
            let lifecycle = self
                .lifecycle
                .lock()
                .map_err(|_| GatewayError::NotReconciled)?;
            let Some(attempt) = lifecycle.attempt.clone() else {
                break lifecycle
                    .retained_gateway
                    .clone()
                    .ok_or(GatewayError::NotReconciled)?;
            };
            drop(lifecycle);
            let _ = attempt.wait_physical();
        };
        self.host.stop_agents(&gateway)
    }
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
        if current.attempt.is_some() {
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
    audit: &Arc<dyn GatewayReconciliationAudit>,
    attempt: &GatewayReconciliationAttempt,
    joined: &GatewayReconciliationRequest,
) -> Result<(), GatewayError> {
    match catch_unwind(AssertUnwindSafe(|| audit.joined(attempt, joined))) {
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

fn state_unavailable() -> GatewayError {
    GatewayError::Registration("gateway startup state is unavailable".into())
}

fn finish(
    lifecycle: &Arc<Mutex<Lifecycle>>,
    attempt: &Arc<ReconciliationAttempt>,
    events: &Arc<dyn GatewayStartupEvents>,
    result: Result<ReconciledGateway, GatewayError>,
    reported: Result<(), GatewayError>,
    effects: &[ReconciliationNativeEffect],
) -> Result<(), GatewayError> {
    let mut current = lifecycle.lock().map_err(|_| state_unavailable())?;
    let owns_current = current
        .attempt
        .as_ref()
        .is_some_and(|current| Arc::ptr_eq(current, attempt));
    if !owns_current {
        return Err(state_unavailable());
    }
    let physical_settlement = result.clone();
    let (answer, publish_startup) = match (result, reported) {
        (Ok(gateway), Ok(())) => {
            let changed_identity = current
                .retained_gateway
                .as_ref()
                .is_some_and(|previous| previous != &gateway);
            current.retained_gateway = Some(gateway);
            if current
                .pending_origin
                .as_ref()
                .is_some_and(|pending| pending.is_owned_by(&attempt.request))
            {
                current.pending_origin = None;
            }
            if matches!(current.startup.phase(), GatewayStartupPhase::Ready) && !changed_identity {
                (Ok(()), None)
            } else {
                current.startup = current.startup.next(GatewayStartupPhase::Ready);
                (Ok(()), Some(current.startup.clone()))
            }
        }
        (Ok(gateway), Err(error)) => {
            current.retained_gateway = Some(gateway);
            current.startup = current
                .startup
                .next(GatewayStartupPhase::Failed(error.clone()));
            (Err(error), Some(current.startup.clone()))
        }
        (Err(physical_error), reported) => {
            let error = reported.err().unwrap_or(physical_error);
            if effects.contains(&ReconciliationNativeEffect::OldServiceUnloaded) {
                current.retained_gateway = None;
            }
            current.startup = current
                .startup
                .next(GatewayStartupPhase::Failed(error.clone()));
            (Err(error), Some(current.startup.clone()))
        }
    };
    let settled = attempt.settle(AttemptSettlement {
        physical: physical_settlement,
        reported: answer.clone(),
    });
    current.attempt = None;
    drop(current);
    if let Some(startup) = &publish_startup {
        publish(events, startup);
    }
    settled?;
    answer
}
/// The search path the agent should be given, if it can be had. Called once per
/// host process; see [`Gateway::agent_path`].
///
/// A login shell that hangs or fails is not a registration failure: the user
/// still gets their gateway, the agent still gets a working path, and the one
/// consequence — that the tools they installed are not on it — is said out loud
/// rather than left to be discovered as `command not found`.
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
