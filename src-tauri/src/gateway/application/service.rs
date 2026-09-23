use super::{
    GatewayError, GatewayHost, GatewayReconciliationAttempt, GatewayReconciliationAudit,
    GatewayReconciliationEffect, GatewayReconciliationIds, GatewayReconciliationIntent,
    GatewayReconciliationOutcome, GatewayReconciliationProgress, GatewayReconciliationRequest,
    GatewayStartup, GatewayStartupEvents, GatewayStartupPhase, LoginShellPath, ReconciledGateway,
    ReconciliationHistoryFact,
};
use crate::gateway::domain::value_objects::{
    BundledSurface, PendingReconciliation, ReconciliationCause, ReconciliationEvidence,
    ReconciliationInitiator, ReconciliationRejectedReport, SearchPath,
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
    running: Option<Arc<AttemptReceipt>>,
    pending: Option<Arc<AttemptReceipt>>,
}

struct AttemptReceipt {
    request: GatewayReconciliationAttempt,
    authority: PendingReconciliation,
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
        }
    }

    fn wait(&self) -> Result<(), GatewayError> {
        self.wait_settlement()?.reported
    }

    fn wait_physical(&self) -> Result<ReconciledGateway, GatewayError> {
        self.wait_settlement()?.physical
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
    #[cfg_attr(
        all(not(target_os = "macos"), not(test)),
        expect(dead_code, reason = "native reconciliation is supported only on macOS")
    )]
    lifecycle: Arc<Mutex<Lifecycle>>,
    #[cfg_attr(
        all(not(target_os = "macos"), not(test)),
        expect(dead_code, reason = "native reconciliation is supported only on macOS")
    )]
    attempt: Arc<AttemptReceipt>,
    #[cfg_attr(
        all(not(target_os = "macos"), not(test)),
        expect(dead_code, reason = "native reconciliation is supported only on macOS")
    )]
    events: Arc<dyn GatewayStartupEvents>,
    #[cfg_attr(
        all(not(target_os = "macos"), not(test)),
        expect(dead_code, reason = "native reconciliation is supported only on macOS")
    )]
    audit: Arc<dyn GatewayReconciliationAudit>,
    intent: Arc<Mutex<Option<GatewayReconciliationIntent>>>,
    history: Arc<Mutex<Vec<ReconciliationHistoryFact>>>,
}

impl GatewayReconciliationProgress for StartupProgress {
    fn readiness_invalidated(&self) {
        let starting = self.lifecycle.lock().ok().and_then(|mut current| {
            let owns_current = current
                .running
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

    fn history_observed(&self, fact: ReconciliationHistoryFact) {
        if let Ok(mut history) = self.history.lock() {
            history.push(fact);
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
    admission: Arc<Mutex<()>>,
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
}

struct AdmittedReceipt {
    receipt: Arc<AttemptReceipt>,
    start_owner: bool,
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
                running: None,
                pending: None,
            })),
            admission: Arc::new(Mutex::new(())),
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

    #[cfg(test)]
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
        let lifecycle = self.lifecycle.clone();
        let admission = self.admission.clone();
        let reconciliation_ids = self.reconciliation_ids.clone();
        let reconciliation_audit = self.reconciliation_audit.clone();
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
        };
        let admitted = tauri::async_runtime::spawn_blocking(move || {
            let admitted = admit_request(
                &lifecycle,
                &admission,
                &reconciliation_ids,
                &reconciliation_audit,
                &startup_events,
                request,
            )?;
            if admitted.start_owner {
                let owner = admitted.receipt.clone();
                let _owner =
                    tauri::async_runtime::spawn_blocking(move || execute_chain(&executor, owner));
            }
            Ok::<_, GatewayError>(admitted)
        })
        .await
        .map_err(|error| GatewayError::Registration(error.to_string()))??;

        let receipt = admitted.receipt;
        tauri::async_runtime::spawn_blocking(move || receipt.wait())
            .await
            .map_err(|error| GatewayError::Registration(error.to_string()))?
    }
    pub fn stop_agents(&self) -> Result<(), GatewayError> {
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
            let _ = attempt.wait_physical();
        };
        self.host.stop_agents(&gateway)
    }
}

fn admit_request(
    lifecycle: &Arc<Mutex<Lifecycle>>,
    admission: &Arc<Mutex<()>>,
    ids: &Arc<dyn GatewayReconciliationIds>,
    audit: &Arc<dyn GatewayReconciliationAudit>,
    events: &Arc<dyn GatewayStartupEvents>,
    request: GatewayReconciliationRequest,
) -> Result<AdmittedReceipt, GatewayError> {
    let guard = admission.lock().map_err(|_| state_unavailable())?;
    let configuration_change =
        request.evidence().cause() == ReconciliationCause::ClaudeConfigurationChanged;
    let existing = {
        let current = lifecycle.lock().map_err(|_| state_unavailable())?;
        current.pending.clone().or_else(|| {
            (!configuration_change)
                .then(|| current.running.clone())
                .flatten()
        })
    };
    if let Some(receipt) = existing {
        drop(guard);
        audit_attached(audit, &receipt.request, &request)?;
        return Ok(AdmittedReceipt {
            receipt,
            start_owner: false,
        });
    }

    let correlation = match ids.next() {
        Ok(correlation) => correlation,
        Err(error) => {
            drop(guard);
            admission_failed(lifecycle, events, &error);
            return Err(error);
        }
    };
    let attempt = match GatewayReconciliationAttempt::new(correlation, request) {
        Ok(attempt) => attempt,
        Err(error) => {
            drop(guard);
            admission_failed(lifecycle, events, &error);
            return Err(error);
        }
    };
    let receipt = Arc::new(AttemptReceipt::pending(attempt));
    let (selected, start_owner, starting) = {
        let mut current = lifecycle.lock().map_err(|_| state_unavailable())?;
        if let Some(pending) = current.pending.clone() {
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
                    current.startup = current.startup.next(GatewayStartupPhase::Starting);
                    current.startup.clone()
                });
            (receipt.clone(), true, starting)
        }
    };
    let joined = !Arc::ptr_eq(&selected, &receipt);
    drop(guard);
    if let Some(starting) = &starting {
        publish(events, starting);
    }
    if joined {
        audit_attached(audit, &selected.request, receipt.request.origin())?;
    }
    Ok(AdmittedReceipt {
        receipt: selected,
        start_owner,
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

enum CleanupUpdate {
    Keep,
    Replace(ReconciledGateway),
    Clear,
}

struct AttemptExecution {
    physical: Result<ReconciledGateway, GatewayError>,
    reported: Result<(), GatewayError>,
    cleanup: CleanupUpdate,
    ready: bool,
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
                ready: false,
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
    let progress = StartupProgress {
        lifecycle: executor.lifecycle.clone(),
        attempt: receipt.clone(),
        events: executor.startup_events.clone(),
        audit: executor.reconciliation_audit.clone(),
        intent: Arc::new(Mutex::new(None)),
        history: Arc::new(Mutex::new(Vec::new())),
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
    let intent = progress
        .intent
        .lock()
        .ok()
        .and_then(|intent| intent.clone());
    let facts = progress
        .history
        .lock()
        .map(|history| history.clone())
        .unwrap_or_default();
    if physical.is_ok() && intent.is_none() {
        physical = Err(GatewayError::Registration(
            "gateway host completed without an admitted audit intent".into(),
        ));
    }
    let Some(intent) = intent else {
        let error = physical.clone().err().unwrap_or_else(|| {
            GatewayError::Registration("gateway audit intent is missing".into())
        });
        return AttemptExecution {
            physical,
            reported: Err(error),
            cleanup: CleanupUpdate::Keep,
            ready: false,
        };
    };
    let identity = physical
        .as_ref()
        .map_err(Clone::clone)
        .and_then(ReconciledGateway::audit_identity);
    let audit_outcome = GatewayReconciliationOutcome::assess(intent, facts, identity);
    let (base_report, cleanup, ready) = match audit_outcome.effect() {
        GatewayReconciliationEffect::Confirmed { .. } => (
            Ok(()),
            physical
                .clone()
                .map(CleanupUpdate::Replace)
                .unwrap_or(CleanupUpdate::Keep),
            true,
        ),
        GatewayReconciliationEffect::Failed { history, error } => (
            Err(error.clone()),
            if history.proves_unloaded() {
                CleanupUpdate::Clear
            } else {
                CleanupUpdate::Keep
            },
            false,
        ),
        GatewayReconciliationEffect::RejectedReport {
            trusted_history,
            report,
            error,
        } => {
            let cleanup = match report {
                ReconciliationRejectedReport::ConfirmedTargetMismatch(_)
                | ReconciliationRejectedReport::HealthyReuseMismatch(_)
                | ReconciliationRejectedReport::ReusedRuntimeInstance(_) => {
                    if trusted_history.proves_unloaded() {
                        CleanupUpdate::Clear
                    } else {
                        CleanupUpdate::Keep
                    }
                }
                _ => physical
                    .clone()
                    .map(CleanupUpdate::Replace)
                    .unwrap_or_else(|_| {
                        if trusted_history.proves_unloaded() {
                            CleanupUpdate::Clear
                        } else {
                            CleanupUpdate::Keep
                        }
                    }),
            };
            (Err(error.clone()), cleanup, false)
        }
    };
    let audit_result = catch_unwind(AssertUnwindSafe(|| {
        executor.reconciliation_audit.outcome(&audit_outcome)
    }));
    let reported = match audit_result {
        Ok(Ok(())) => base_report,
        Ok(Err(error)) => Err(GatewayError::Audit {
            audit: error.to_string(),
            physical: base_report.err().map(Box::new),
        }),
        Err(_) => Err(GatewayError::Audit {
            audit: "gateway reconciliation audit adapter panicked".into(),
            physical: base_report.err().map(Box::new),
        }),
    };
    let ready = ready && reported.is_ok();
    AttemptExecution {
        physical,
        reported,
        cleanup,
        ready,
    }
}

fn settle_and_promote(
    executor: &ReconciliationExecutor,
    receipt: &Arc<AttemptReceipt>,
    execution: AttemptExecution,
) -> Option<Arc<AttemptReceipt>> {
    let answer = execution.reported.clone();
    let settlement = AttemptSettlement {
        physical: execution.physical,
        reported: answer.clone(),
    };
    let (next, startup) = executor
        .lifecycle
        .lock()
        .ok()
        .and_then(|mut current| {
            let owns_running = current
                .running
                .as_ref()
                .is_some_and(|running| Arc::ptr_eq(running, receipt));
            if !owns_running {
                return None;
            }
            match execution.cleanup {
                CleanupUpdate::Keep => {}
                CleanupUpdate::Replace(gateway) => current.retained_gateway = Some(gateway),
                CleanupUpdate::Clear => current.retained_gateway = None,
            }
            let phase = if execution.ready {
                GatewayStartupPhase::Ready
            } else {
                GatewayStartupPhase::Failed(answer.clone().err().unwrap_or_else(|| {
                    GatewayError::Registration("gateway reconciliation was rejected".into())
                }))
            };
            let unchanged_ready =
                execution.ready && matches!(current.startup.phase(), GatewayStartupPhase::Ready);
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
            Some((current.running.clone(), startup))
        })
        .unwrap_or((None, None));
    if let Some(startup) = &startup {
        publish(&executor.startup_events, startup);
    }
    let _ = receipt.settle(settlement);
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
