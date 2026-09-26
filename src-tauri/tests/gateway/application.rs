use super::*;
use crate::gateway::application::ports::GatewayLifecycleRecovery;
use crate::gateway::{
    application::{
        testing::{self, FixedLoginShell},
        GatewayStartup, GatewayStartupEvents, GatewayStartupPhase, LoginShellError,
    },
    domain::value_objects::{
        LifecycleRecordKind, ReconciliationCause, ReconciliationCorrelation,
        ReconciliationEvidence, ReconciliationIncarnation, ReconciliationInitiator,
        ReconciliationTarget, SearchPathError,
    },
};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Condvar, Mutex, Weak,
    },
    thread,
    time::{Duration, Instant},
};

fn login_shell(path: &str) -> Arc<FixedLoginShell> {
    Arc::new(FixedLoginShell(Ok(SearchPath::parse(path).unwrap())))
}

struct ControlledOutcomeClock(Mutex<Instant>);

impl ControlledOutcomeClock {
    fn new(now: Instant) -> Self {
        Self(Mutex::new(now))
    }

    fn set(&self, now: Instant) {
        *self.0.lock().unwrap() = now;
    }
}

impl MonotonicClock for ControlledOutcomeClock {
    fn now(&self) -> Instant {
        *self.0.lock().unwrap()
    }
}

fn admit(
    attempt: &GatewayReconciliationAttempt,
    progress: &dyn GatewayReconciliationProgress,
    service: &str,
) {
    let target = ReconciliationTarget::new(service.into(), "a".repeat(64), "b".repeat(64)).unwrap();
    let before = reconciled(service).audit_identity().unwrap();
    let intent = GatewayReconciliationIntent::new(attempt.clone(), target, Some(before)).unwrap();
    progress.intent_admitted(intent).unwrap();
}

fn complete_stop(
    session: &GatewayStopSession,
    journal: &dyn GatewayReconciliationJournalSession,
    plan: &AuditDeliveryReceipt,
    run: impl FnOnce() -> Result<(), GatewayError>,
) -> Result<LifecycleObservation, GatewayError> {
    let proof_token = session.begin_proof()?;
    let candidate = session.request().intended().audit_identity()?;
    session.prove(&proof_token, candidate.clone(), 1)?;
    session.claim(proof_token, plan, &candidate, 1)?;
    let physical = run();
    let command = match &physical {
        Ok(()) => LifecycleCommandResult::Accepted,
        Err(error) => LifecycleCommandResult::Failed(error.to_string()),
    };
    session.command_result(command.clone())?;
    journal.effect_completion("stop-agents-on-desktop-quit", "signal-agents", &command)?;
    let observation = LifecycleObservation::new(2, Some(candidate), true);
    journal.observation(
        &LifecycleObservationSource::Effect {
            plan_id: "stop-agents-on-desktop-quit".into(),
            step_id: "signal-agents".into(),
        },
        &observation,
    )?;
    session.fresh_observation(observation.clone())?;
    physical?;
    Ok(observation)
}

fn admit_fresh(
    attempt: &GatewayReconciliationAttempt,
    progress: &dyn GatewayReconciliationProgress,
    service: &str,
) {
    let target = ReconciliationTarget::new(service.into(), "a".repeat(64), "b".repeat(64)).unwrap();
    let intent = GatewayReconciliationIntent::new(attempt.clone(), target, None).unwrap();
    progress.intent_admitted(intent).unwrap();
}

/// A login shell that answers whatever it is told to, and counts being asked.
struct CountingLoginShell {
    answers: Mutex<Vec<Result<SearchPath, LoginShellError>>>,
    resolutions: Mutex<usize>,
}
impl CountingLoginShell {
    fn new(answers: Vec<Result<SearchPath, LoginShellError>>) -> Arc<Self> {
        Arc::new(Self {
            answers: Mutex::new(answers),
            resolutions: Mutex::new(0),
        })
    }
    fn resolutions(&self) -> usize {
        *self.resolutions.lock().unwrap()
    }
}
impl LoginShellPath for CountingLoginShell {
    fn resolve(&self) -> Result<SearchPath, LoginShellError> {
        *self.resolutions.lock().unwrap() += 1;
        let mut answers = self.answers.lock().unwrap();
        if answers.is_empty() {
            Err(LoginShellError::TimedOut)
        } else {
            answers.remove(0)
        }
    }
}

struct FakeHost {
    registration: Result<ReconciledGateway, GatewayError>,
    stop_result: Result<(), GatewayError>,
    calls: Mutex<Vec<String>>,
}
impl GatewayHost for FakeHost {
    fn register(
        &self,
        _: &Path,
        stage: &str,
        agent_path: Option<&SearchPath>,
        attempt: &GatewayReconciliationAttempt,
        progress: &dyn GatewayReconciliationProgress,
    ) -> Result<ReconciledGateway, GatewayError> {
        self.calls.lock().unwrap().push(format!(
            "register:{stage}:{}",
            agent_path.map_or("-", SearchPath::as_str)
        ));
        let registration = self.registration.clone();
        let service = registration
            .as_ref()
            .map_or("unconfirmed", |gateway| gateway.service());
        admit(attempt, progress, service);
        registration
    }
    fn stop_agents(
        &self,
        session: &GatewayStopSession,
        journal: &dyn GatewayReconciliationJournalSession,
        plan: &AuditDeliveryReceipt,
    ) -> Result<LifecycleObservation, GatewayError> {
        let gateway = session.request().intended();
        complete_stop(session, journal, plan, || {
            self.calls
                .lock()
                .unwrap()
                .push(format!("stop:{}", gateway.service()));
            self.stop_result.clone()
        })
    }
}
#[test]
fn gateway_instances_use_only_their_injected_service_and_surface_stop_failure() {
    for identity in ["first", "second"] {
        let host = Arc::new(FakeHost {
            registration: Ok(reconciled(identity)),
            stop_result: Err(GatewayError::Stop("delivery failed".into())),
            calls: Mutex::new(vec![]),
        });
        let gateway = Gateway::bootstrap(
            host.clone(),
            login_shell("/opt/homebrew/bin:/usr/bin"),
            testing::discard_startup_events(),
            testing::sequential_reconciliation_ids(),
            testing::discard_reconciliation_audit(),
            "/runtime".into(),
            "ci".into(),
        );
        tauri::async_runtime::block_on(gateway.wait_ready(BundledSurface::Main)).unwrap();
        assert_eq!(
            gateway.stop_agents(Instant::now() + Duration::from_secs(30)),
            Err(GatewayError::Stop("delivery failed".into()))
        );
        assert_eq!(
            *host.calls.lock().unwrap(),
            [
                "register:ci:/opt/homebrew/bin:/usr/bin".to_string(),
                format!("stop:{identity}")
            ]
        );
    }
}

#[test]
fn claimed_blocked_stop_becomes_indeterminate_and_starts_no_later_cleanup() {
    struct BlockedDispatchHost {
        entered: Mutex<Option<mpsc::Sender<()>>>,
        release: Mutex<mpsc::Receiver<()>>,
        returned: mpsc::Sender<Result<(), GatewayError>>,
        later_cleanup: AtomicUsize,
    }

    impl GatewayHost for BlockedDispatchHost {
        fn register(
            &self,
            _: &Path,
            _: &str,
            _: Option<&SearchPath>,
            attempt: &GatewayReconciliationAttempt,
            progress: &dyn GatewayReconciliationProgress,
        ) -> Result<ReconciledGateway, GatewayError> {
            admit(attempt, progress, "blocked-stop");
            Ok(reconciled("blocked-stop"))
        }

        fn stop_agents(
            &self,
            session: &GatewayStopSession,
            _: &dyn GatewayReconciliationJournalSession,
            plan: &AuditDeliveryReceipt,
        ) -> Result<LifecycleObservation, GatewayError> {
            let proof_token = session.begin_proof()?;
            let candidate = session.request().intended().audit_identity()?;
            session.prove(&proof_token, candidate.clone(), 1)?;
            session.claim(proof_token, plan, &candidate, 1)?;
            self.entered
                .lock()
                .unwrap()
                .take()
                .unwrap()
                .send(())
                .unwrap();
            self.release.lock().unwrap().recv().unwrap();
            let command = session.command_result(LifecycleCommandResult::Accepted);
            self.returned.send(command.clone()).unwrap();
            command?;
            self.later_cleanup.fetch_add(1, Ordering::SeqCst);
            Err(GatewayError::Stop(
                "cleanup should not start after indeterminate dispatch".into(),
            ))
        }
    }

    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (returned_tx, returned_rx) = mpsc::channel();
    let host = Arc::new(BlockedDispatchHost {
        entered: Mutex::new(Some(entered_tx)),
        release: Mutex::new(release_rx),
        returned: returned_tx,
        later_cleanup: AtomicUsize::new(0),
    });
    let audit = Arc::new(RecordingAudit::default());
    let gateway = Arc::new(Gateway::bootstrap(
        host.clone(),
        login_shell("/usr/bin"),
        testing::discard_startup_events(),
        testing::sequential_reconciliation_ids(),
        audit.clone(),
        "/runtime".into(),
        "ci".into(),
    ));
    tauri::async_runtime::block_on(gateway.start()).unwrap();
    let (stop_tx, stop_rx) = mpsc::channel();
    let stop = {
        let gateway = gateway.clone();
        thread::spawn(move || {
            stop_tx
                .send(gateway.stop_agents(Instant::now() + Duration::from_millis(100)))
                .unwrap();
        })
    };
    entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(matches!(
        stop_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        Err(GatewayError::Stop(message)) if message.contains("did not settle")
    ));
    release_tx.send(()).unwrap();
    assert!(returned_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .is_err());
    stop.join().unwrap();
    assert_eq!(host.later_cleanup.load(Ordering::SeqCst), 0);
    assert_eq!(audit.effect_completions.load(Ordering::SeqCst), 0);
    assert_eq!(audit.observations.load(Ordering::SeqCst), 0);
    assert_eq!(audit.physical_outcomes.load(Ordering::SeqCst), 0);
}

#[test]
fn proof_blocked_across_quit_deadline_never_claims_or_dispatches() {
    struct BlockedProofHost {
        entered: Mutex<Option<mpsc::Sender<()>>>,
        release: Mutex<mpsc::Receiver<()>>,
        proof_returned: mpsc::Sender<Result<(), GatewayError>>,
        dispatches: AtomicUsize,
    }

    impl GatewayHost for BlockedProofHost {
        fn register(
            &self,
            _: &Path,
            _: &str,
            _: Option<&SearchPath>,
            attempt: &GatewayReconciliationAttempt,
            progress: &dyn GatewayReconciliationProgress,
        ) -> Result<ReconciledGateway, GatewayError> {
            admit(attempt, progress, "blocked-proof");
            Ok(reconciled("blocked-proof"))
        }

        fn stop_agents(
            &self,
            session: &GatewayStopSession,
            _: &dyn GatewayReconciliationJournalSession,
            plan: &AuditDeliveryReceipt,
        ) -> Result<LifecycleObservation, GatewayError> {
            let proof_token = session.begin_proof()?;
            self.entered
                .lock()
                .unwrap()
                .take()
                .unwrap()
                .send(())
                .unwrap();
            self.release.lock().unwrap().recv().unwrap();
            let candidate = session.request().intended().audit_identity()?;
            let proof = session.prove(&proof_token, candidate.clone(), 1);
            self.proof_returned.send(proof.clone()).unwrap();
            proof?;
            session.claim(proof_token, plan, &candidate, 1)?;
            self.dispatches.fetch_add(1, Ordering::SeqCst);
            Err(GatewayError::Stop(
                "dispatch should not start after deadline revocation".into(),
            ))
        }
    }

    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (proof_tx, proof_rx) = mpsc::channel();
    let host = Arc::new(BlockedProofHost {
        entered: Mutex::new(Some(entered_tx)),
        release: Mutex::new(release_rx),
        proof_returned: proof_tx,
        dispatches: AtomicUsize::new(0),
    });
    let gateway = Arc::new(Gateway::bootstrap(
        host.clone(),
        login_shell("/usr/bin"),
        testing::discard_startup_events(),
        testing::sequential_reconciliation_ids(),
        testing::discard_reconciliation_audit(),
        "/runtime".into(),
        "ci".into(),
    ));
    tauri::async_runtime::block_on(gateway.start()).unwrap();
    let (stop_tx, stop_rx) = mpsc::channel();
    let stop = {
        let gateway = gateway.clone();
        thread::spawn(move || {
            stop_tx
                .send(gateway.stop_agents(Instant::now() + Duration::from_millis(100)))
                .unwrap();
        })
    };
    entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(matches!(
        stop_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        Err(GatewayError::Stop(message)) if message.contains("did not settle")
    ));
    release_tx.send(()).unwrap();
    assert!(proof_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .is_err());
    stop.join().unwrap();
    assert_eq!(host.dispatches.load(Ordering::SeqCst), 0);
}
#[test]
fn failed_registration_is_retried_and_never_derives_a_service_to_stop() {
    let host = Arc::new(FakeHost {
        registration: Err(GatewayError::Registration("not installed".into())),
        stop_result: Ok(()),
        calls: Mutex::new(vec![]),
    });
    let gateway = Gateway::bootstrap(
        host.clone(),
        login_shell("/opt/homebrew/bin:/usr/bin"),
        testing::discard_startup_events(),
        testing::sequential_reconciliation_ids(),
        testing::discard_reconciliation_audit(),
        "/runtime".into(),
        "ci".into(),
    );
    assert!(matches!(
        tauri::async_runtime::block_on(gateway.wait_ready(BundledSurface::Main)),
        Err(GatewayError::Registration(_))
    ));
    assert!(tauri::async_runtime::block_on(gateway.wait_ready(BundledSurface::Main)).is_err());
    assert!(tauri::async_runtime::block_on(gateway.retry(BundledSurface::Main)).is_err());
    assert_eq!(
        gateway.stop_agents(Instant::now() + Duration::from_secs(30)),
        Err(GatewayError::NotReconciled)
    );
    assert_eq!(
        *host.calls.lock().unwrap(),
        ["register:ci:/opt/homebrew/bin:/usr/bin"; 3]
    );
}

#[test]
fn later_failed_reconciliation_preserves_candidate_for_native_revalidation() {
    struct ChangingHost {
        registrations: Mutex<Vec<Result<ReconciledGateway, GatewayError>>>,
        calls: Mutex<Vec<String>>,
    }
    impl GatewayHost for ChangingHost {
        fn register(
            &self,
            _: &Path,
            _: &str,
            _: Option<&SearchPath>,
            attempt: &GatewayReconciliationAttempt,
            progress: &dyn GatewayReconciliationProgress,
        ) -> Result<ReconciledGateway, GatewayError> {
            let registration = self.registrations.lock().unwrap().remove(0);
            let service = registration
                .as_ref()
                .map_or("unconfirmed", |gateway| gateway.service());
            admit(attempt, progress, service);
            registration
        }
        fn stop_agents(
            &self,
            session: &GatewayStopSession,
            journal: &dyn GatewayReconciliationJournalSession,
            plan: &AuditDeliveryReceipt,
        ) -> Result<LifecycleObservation, GatewayError> {
            let gateway = session.request().intended();
            complete_stop(session, journal, plan, || {
                self.calls
                    .lock()
                    .unwrap()
                    .push(gateway.service().to_owned());
                Ok(())
            })
        }
    }
    let host = Arc::new(ChangingHost {
        registrations: Mutex::new(vec![
            Ok(reconciled("gui/501/exact-reconciled")),
            Err(GatewayError::Registration("foreign service".into())),
        ]),
        calls: Mutex::new(vec![]),
    });
    let gateway = Gateway::bootstrap(
        host.clone(),
        login_shell("/opt/homebrew/bin:/usr/bin"),
        testing::discard_startup_events(),
        testing::sequential_reconciliation_ids(),
        testing::discard_reconciliation_audit(),
        "/runtime".into(),
        "ci".into(),
    );
    tauri::async_runtime::block_on(gateway.wait_ready(BundledSurface::Main)).unwrap();
    assert!(tauri::async_runtime::block_on(gateway.wait_ready(BundledSurface::Main)).is_err());
    gateway
        .stop_agents(Instant::now() + Duration::from_secs(30))
        .unwrap();
    assert_eq!(*host.calls.lock().unwrap(), ["gui/501/exact-reconciled"]);
}

/// A login shell that hangs, fails or answers with nonsense is not a reason to
/// leave the user without a gateway. The registration still happens; the host is
/// told there is no resolved path this launch, which is a different fact from
/// "the user has nothing on their path".
#[test]
fn a_login_shell_that_cannot_be_read_still_registers_the_service() {
    for failure in [
        LoginShellError::TimedOut,
        LoginShellError::Unavailable("No such file or directory".into()),
        LoginShellError::Rejected(SearchPathError::NoAbsoluteEntry),
    ] {
        let host = Arc::new(FakeHost {
            registration: Ok(reconciled("gui/501/so.nessa.gateway.ci")),
            stop_result: Ok(()),
            calls: Mutex::new(vec![]),
        });
        let gateway = Gateway::bootstrap(
            host.clone(),
            Arc::new(FixedLoginShell(Err(failure.clone()))),
            testing::discard_startup_events(),
            testing::sequential_reconciliation_ids(),
            testing::discard_reconciliation_audit(),
            "/runtime".into(),
            "ci".into(),
        );
        tauri::async_runtime::block_on(gateway.wait_ready(BundledSurface::Main)).unwrap();
        assert_eq!(*host.calls.lock().unwrap(), ["register:ci:-"], "{failure}");
    }
}

/// Reconciling again is routine — every webview load does it — so the login
/// shell is asked once and the answer is reused, whatever it was. A profile
/// edited while Nessa is open must not produce a second, different path: that
/// would be a changed service definition, which retires the running gateway and
/// stops its agents in the middle of a session nobody asked to interrupt.
#[test]
fn reconciling_again_reuses_the_path_rather_than_asking_the_login_shell_again() {
    let login_shell = CountingLoginShell::new(vec![
        Ok(SearchPath::parse("/opt/homebrew/bin:/usr/bin").unwrap()),
        Ok(SearchPath::parse("/somewhere/else").unwrap()),
    ]);
    let host = Arc::new(FakeHost {
        registration: Ok(reconciled("gui/501/so.nessa.gateway.ci")),
        stop_result: Ok(()),
        calls: Mutex::new(vec![]),
    });
    let gateway = Gateway::bootstrap(
        host.clone(),
        login_shell.clone(),
        testing::discard_startup_events(),
        testing::sequential_reconciliation_ids(),
        testing::discard_reconciliation_audit(),
        "/runtime".into(),
        "ci".into(),
    );
    for _ in 0..3 {
        tauri::async_runtime::block_on(gateway.wait_ready(BundledSurface::Main)).unwrap();
    }
    assert_eq!(login_shell.resolutions(), 1);
    assert_eq!(
        *host.calls.lock().unwrap(),
        ["register:ci:/opt/homebrew/bin:/usr/bin"; 3]
    );
}

/// The same holds for a shell that could not be read: the failure is the
/// outcome, remembered as such. Otherwise every panel load would spend the
/// whole deadline again on a profile already known to hang.
#[test]
fn a_login_shell_that_failed_once_is_not_asked_again_this_session() {
    let login_shell = CountingLoginShell::new(vec![
        Err(LoginShellError::TimedOut),
        Ok(SearchPath::parse("/opt/homebrew/bin:/usr/bin").unwrap()),
    ]);
    let host = Arc::new(FakeHost {
        registration: Ok(reconciled("gui/501/so.nessa.gateway.ci")),
        stop_result: Ok(()),
        calls: Mutex::new(vec![]),
    });
    let gateway = Gateway::bootstrap(
        host.clone(),
        login_shell.clone(),
        testing::discard_startup_events(),
        testing::sequential_reconciliation_ids(),
        testing::discard_reconciliation_audit(),
        "/runtime".into(),
        "ci".into(),
    );
    for _ in 0..2 {
        tauri::async_runtime::block_on(gateway.wait_ready(BundledSurface::Main)).unwrap();
    }
    assert_eq!(login_shell.resolutions(), 1);
    assert_eq!(*host.calls.lock().unwrap(), ["register:ci:-"; 2]);
}

/// Every failure names itself, because the fallback is only discoverable if it
/// is reported: the symptom otherwise is a tool that works in every terminal
/// and not in Nessa.
#[test]
fn every_login_shell_failure_can_be_reported() {
    for (failure, wording) in [
        (LoginShellError::TimedOut, "deadline"),
        (
            LoginShellError::Unavailable("no such shell".into()),
            "no such shell",
        ),
        (
            LoginShellError::Rejected(SearchPathError::Control),
            "control characters",
        ),
    ] {
        assert!(failure.to_string().contains(wording), "{failure:?}");
    }
}

fn reconciled(service: &str) -> ReconciledGateway {
    ReconciledGateway::new(
        service.into(),
        "a".repeat(64),
        "550e8400-e29b-41d4-a716-446655440000".into(),
        "b".repeat(64),
        42,
        7420,
    )
}

trait TestAuditBehavior: Send + Sync {
    fn intent(&self, intent: &GatewayReconciliationIntent) -> Result<(), GatewayError>;
    fn outcome(&self, outcome: &GatewayReconciliationOutcome) -> Result<(), GatewayError>;
    fn outcome_rejected(&self, _: &GatewayReconciliationOutcome) -> bool {
        false
    }
    fn joined(
        &self,
        attempt: &GatewayReconciliationAttempt,
        joined: &GatewayReconciliationRequest,
    ) -> Result<(), GatewayError>;

    fn recovery(&self) -> Option<GatewayLifecycleRecovery> {
        None
    }

    fn observation(
        &self,
        _: &LifecycleObservationSource,
        _: &LifecycleObservation,
    ) -> Result<(), GatewayError> {
        Ok(())
    }

    fn effect_completion(
        &self,
        _: &str,
        _: &str,
        _: &LifecycleCommandResult,
    ) -> Result<(), GatewayError> {
        Ok(())
    }

    fn physical_outcome(
        &self,
        _: &LifecyclePhysicalOutcome,
        _: Option<&LifecycleObservation>,
        _: ReconciliationCleanupDecision,
    ) -> Result<(), GatewayError> {
        Ok(())
    }
}

struct TestJournal<T> {
    audit: Arc<T>,
    attempt: GatewayReconciliationAttempt,
    sequence: AtomicUsize,
}

impl<T: TestAuditBehavior> TestJournal<T> {
    fn receipt(&self, kind: LifecycleRecordKind) -> AuditDeliveryReceipt {
        AuditDeliveryReceipt::new(
            self.attempt.correlation().clone(),
            self.sequence.fetch_add(1, Ordering::SeqCst) as u64,
            kind,
        )
    }
}

impl<T: TestAuditBehavior> GatewayReconciliationJournalSession for TestJournal<T> {
    fn recovery(&self) -> Option<GatewayLifecycleRecovery> {
        self.audit.recovery()
    }

    fn intent(&self, intent: &GatewayReconciliationIntent) -> Result<(), GatewayError> {
        self.audit.intent(intent)
    }

    fn outcome(
        &self,
        outcome: &GatewayReconciliationOutcome,
    ) -> Result<(), GatewayReconciliationOutcomeError> {
        self.audit.outcome(outcome).map_err(|error| {
            if self.audit.outcome_rejected(outcome) {
                GatewayReconciliationOutcomeError::Rejected(error)
            } else {
                GatewayReconciliationOutcomeError::Delivery(error)
            }
        })
    }

    fn joined(&self, joined: &GatewayReconciliationRequest) -> Result<(), GatewayError> {
        self.audit.joined(&self.attempt, joined)
    }

    fn effect_plan(
        &self,
        _: &str,
        _: Option<&ReconciliationIncarnation>,
        _: &ReconciliationTarget,
        _: &LifecyclePlanStep,
        _: &[LifecyclePlanStep],
    ) -> Result<AuditDeliveryReceipt, GatewayError> {
        Ok(self.receipt(LifecycleRecordKind::EffectPlan))
    }

    fn effect_completion(
        &self,
        plan_id: &str,
        step_id: &str,
        result: &LifecycleCommandResult,
    ) -> Result<AuditDeliveryReceipt, GatewayError> {
        self.audit.effect_completion(plan_id, step_id, result)?;
        Ok(self.receipt(LifecycleRecordKind::EffectCompletion))
    }

    fn observation(
        &self,
        source: &LifecycleObservationSource,
        observation: &LifecycleObservation,
    ) -> Result<AuditDeliveryReceipt, GatewayError> {
        self.audit.observation(source, observation)?;
        Ok(self.receipt(LifecycleRecordKind::Observation))
    }

    fn physical_outcome(
        &self,
        physical: &LifecyclePhysicalOutcome,
        last_confirmed: Option<&LifecycleObservation>,
        cleanup: ReconciliationCleanupDecision,
    ) -> Result<AuditDeliveryReceipt, GatewayError> {
        self.audit
            .physical_outcome(physical, last_confirmed, cleanup)?;
        Ok(self.receipt(LifecycleRecordKind::Outcome))
    }
}

#[test]
fn a_restored_no_effect_attempt_settles_before_the_current_attempt_opens() {
    struct RecoveryAudit {
        pending: AtomicBool,
    }

    impl TestAuditBehavior for RecoveryAudit {
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

        fn recovery(&self) -> Option<GatewayLifecycleRecovery> {
            if !self.pending.swap(false, Ordering::SeqCst) {
                return None;
            }
            let request = GatewayReconciliationRequest::new(
                correlation(90),
                ReconciliationEvidence::new(
                    ReconciliationCause::Startup,
                    ReconciliationInitiator::DesktopHost,
                )
                .unwrap(),
            );
            let attempt = GatewayReconciliationAttempt::new(correlation(91), request).unwrap();
            Some(GatewayLifecycleRecovery::new(
                attempt,
                ReconciliationTarget::new(
                    "gui/501/so.nessa.gateway.old".into(),
                    "c".repeat(64),
                    "d".repeat(64),
                )
                .unwrap(),
                None,
                false,
                None,
                None,
                Vec::new(),
            ))
        }
    }

    struct RecoveryHost(Mutex<Vec<String>>);

    impl GatewayHost for RecoveryHost {
        fn register(
            &self,
            _: &Path,
            _: &str,
            _: Option<&SearchPath>,
            attempt: &GatewayReconciliationAttempt,
            progress: &dyn GatewayReconciliationProgress,
        ) -> Result<ReconciledGateway, GatewayError> {
            self.0.lock().unwrap().push("register-current".into());
            admit(attempt, progress, "gui/501/so.nessa.gateway.current");
            Ok(reconciled("gui/501/so.nessa.gateway.current"))
        }

        fn recover(
            &self,
            recovery: &GatewayLifecycleRecovery,
            journal: &dyn GatewayReconciliationJournalSession,
        ) -> Result<(), GatewayError> {
            self.0
                .lock()
                .unwrap()
                .push(format!("recover:{}", recovery.target().service()));
            let observation = LifecycleObservation::new(1, None, false);
            journal.observation(&LifecycleObservationSource::Intent, &observation)?;
            journal.physical_outcome(
                &LifecyclePhysicalOutcome::Failed {
                    phase: LifecycleFailedPhase::NativeDispatch,
                    message: "no effect".into(),
                },
                Some(&observation),
                ReconciliationCleanupDecision::RetainPrior,
            )?;
            Ok(())
        }

        fn stop_agents(
            &self,
            session: &GatewayStopSession,
            journal: &dyn GatewayReconciliationJournalSession,
            plan: &AuditDeliveryReceipt,
        ) -> Result<LifecycleObservation, GatewayError> {
            complete_stop(session, journal, plan, || Ok(()))
        }
    }

    let audit = Arc::new(RecoveryAudit {
        pending: AtomicBool::new(true),
    });
    let host = Arc::new(RecoveryHost(Mutex::new(Vec::new())));
    let gateway = Gateway::bootstrap(
        host.clone(),
        login_shell("/usr/bin"),
        testing::discard_startup_events(),
        testing::sequential_reconciliation_ids(),
        audit,
        "/runtime".into(),
        "ci".into(),
    );

    tauri::async_runtime::block_on(gateway.start()).unwrap();
    assert_eq!(
        *host.0.lock().unwrap(),
        ["recover:gui/501/so.nessa.gateway.old", "register-current"]
    );
}

impl<T: TestAuditBehavior + 'static> GatewayReconciliationAudit for T {
    fn open(
        self: Arc<Self>,
        attempt: &GatewayReconciliationAttempt,
        _: Option<Instant>,
    ) -> Result<Arc<dyn GatewayReconciliationJournalSession>, GatewayError> {
        Ok(Arc::new(TestJournal {
            audit: self,
            attempt: attempt.clone(),
            sequence: AtomicUsize::new(1),
        }))
    }
}

#[derive(Default)]
struct RecordingAudit {
    intents: Mutex<Vec<GatewayReconciliationIntent>>,
    outcomes: Mutex<Vec<GatewayReconciliationOutcome>>,
    joined: Mutex<Vec<(GatewayReconciliationAttempt, GatewayReconciliationRequest)>>,
    joined_changed: Condvar,
    fail_intent: bool,
    panic_intent: bool,
    fail_outcome: bool,
    fail_joined: bool,
    panic_outcome: bool,
    effect_completions: AtomicUsize,
    observations: AtomicUsize,
    physical_outcomes: AtomicUsize,
}

impl RecordingAudit {
    fn wait_for_joined(&self, count: usize) -> bool {
        self.joined_changed
            .wait_timeout_while(
                self.joined.lock().unwrap(),
                Duration::from_secs(2),
                |joined| joined.len() < count,
            )
            .map(|(joined, timeout)| joined.len() >= count && !timeout.timed_out())
            .unwrap_or(false)
    }
}

struct QueuedReconciliationIds(Mutex<Vec<Result<ReconciliationCorrelation, GatewayError>>>);

impl GatewayReconciliationIds for QueuedReconciliationIds {
    fn next(&self) -> Result<ReconciliationCorrelation, GatewayError> {
        self.0.lock().unwrap().remove(0)
    }
}

fn correlation(number: u64) -> ReconciliationCorrelation {
    ReconciliationCorrelation::parse(format!("00000000-0000-4000-8000-{number:012x}")).unwrap()
}

#[test]
fn attempt_and_outcome_constructors_reject_contradictory_correlations_and_targets() {
    let request_correlation = correlation(1);
    let request = GatewayReconciliationRequest::new(
        request_correlation.clone(),
        ReconciliationEvidence::new(
            ReconciliationCause::Startup,
            ReconciliationInitiator::DesktopHost,
        )
        .unwrap(),
    );
    assert!(GatewayReconciliationAttempt::new(request_correlation, request.clone()).is_err());

    let attempt = GatewayReconciliationAttempt::new(correlation(2), request).unwrap();
    let admitted_target =
        ReconciliationTarget::new("admitted".into(), "a".repeat(64), "b".repeat(64)).unwrap();
    let intent = GatewayReconciliationIntent::new(attempt, admitted_target.clone(), None).unwrap();
    let contradictory_target =
        ReconciliationTarget::new("other".into(), "a".repeat(64), "b".repeat(64)).unwrap();
    let after = ReconciliationIncarnation::new(
        contradictory_target,
        "550e8400-e29b-41d4-a716-446655440000".into(),
        42,
        7420,
    )
    .unwrap();

    let outcome = GatewayReconciliationOutcome::assess(
        intent,
        GatewayReconciliationIntentDelivery::Acknowledged,
        GatewayReconciliationEffectTiming::NoEffectsObserved,
        Vec::new(),
        LifecycleFailedPhase::NativeDispatch,
        Ok(after),
    );
    let GatewayReconciliationEffect::RejectedReport { report, .. } = outcome.effect() else {
        panic!("contradictory target was accepted")
    };
    assert!(!report.validation().target_matches());
    assert_eq!(report.cleanup(), ReconciliationCleanupDecision::RetainPrior);
}

impl TestAuditBehavior for RecordingAudit {
    fn intent(&self, intent: &GatewayReconciliationIntent) -> Result<(), GatewayError> {
        self.intents.lock().unwrap().push(intent.clone());
        assert!(!self.panic_intent, "intent audit panic");
        if self.fail_intent {
            Err(GatewayError::Registration(
                "intent audit unavailable".into(),
            ))
        } else {
            Ok(())
        }
    }

    fn outcome(&self, outcome: &GatewayReconciliationOutcome) -> Result<(), GatewayError> {
        self.outcomes.lock().unwrap().push(outcome.clone());
        assert!(!self.panic_outcome, "outcome audit panic");
        if self.fail_outcome {
            Err(GatewayError::Registration(
                "outcome audit unavailable".into(),
            ))
        } else {
            Ok(())
        }
    }

    fn joined(
        &self,
        attempt: &GatewayReconciliationAttempt,
        joined: &GatewayReconciliationRequest,
    ) -> Result<(), GatewayError> {
        self.joined
            .lock()
            .unwrap()
            .push((attempt.clone(), joined.clone()));
        self.joined_changed.notify_all();
        if self.fail_joined {
            Err(GatewayError::Registration(
                "joined audit unavailable".into(),
            ))
        } else {
            Ok(())
        }
    }

    fn physical_outcome(
        &self,
        _: &LifecyclePhysicalOutcome,
        _: Option<&LifecycleObservation>,
        _: ReconciliationCleanupDecision,
    ) -> Result<(), GatewayError> {
        self.physical_outcomes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn effect_completion(
        &self,
        _: &str,
        _: &str,
        _: &LifecycleCommandResult,
    ) -> Result<(), GatewayError> {
        self.effect_completions.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn observation(
        &self,
        _: &LifecycleObservationSource,
        _: &LifecycleObservation,
    ) -> Result<(), GatewayError> {
        self.observations.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[test]
fn settled_stop_at_deadline_starts_no_success_or_rejection_outcome_delivery() {
    struct DeadlineAtSettlementHost {
        clock: Arc<ControlledOutcomeClock>,
        deadline: Instant,
        rejected: bool,
        indeterminate: bool,
    }

    impl GatewayHost for DeadlineAtSettlementHost {
        fn register(
            &self,
            _: &Path,
            _: &str,
            _: Option<&SearchPath>,
            _: &GatewayReconciliationAttempt,
            _: &dyn GatewayReconciliationProgress,
        ) -> Result<ReconciledGateway, GatewayError> {
            unreachable!("the focused stop test does not register")
        }

        fn stop_agents(
            &self,
            session: &GatewayStopSession,
            journal: &dyn GatewayReconciliationJournalSession,
            plan: &AuditDeliveryReceipt,
        ) -> Result<LifecycleObservation, GatewayError> {
            let result = if self.indeterminate {
                let proof_token = session.begin_proof()?;
                let candidate = session.request().intended().audit_identity()?;
                session.prove(&proof_token, candidate.clone(), 1)?;
                session.claim(proof_token, plan, &candidate, 1)?;
                let command = LifecycleCommandResult::Indeterminate(
                    "native stop result remained unknown".into(),
                );
                session.command_result(command.clone())?;
                journal.effect_completion(
                    "stop-agents-on-desktop-quit",
                    "signal-agents",
                    &command,
                )?;
                let observation = LifecycleObservation::new(2, Some(candidate), true);
                journal.observation(
                    &LifecycleObservationSource::Effect {
                        plan_id: "stop-agents-on-desktop-quit".into(),
                        step_id: "signal-agents".into(),
                    },
                    &observation,
                )?;
                session.fresh_observation(observation)?;
                Err(GatewayError::Stop("native stop indeterminate".into()))
            } else {
                complete_stop(session, journal, plan, || {
                    if self.rejected {
                        Err(GatewayError::Stop("native stop rejected".into()))
                    } else {
                        Ok(())
                    }
                })
            };
            self.clock.set(self.deadline);
            result
        }
    }

    for (rejected, indeterminate) in [(false, false), (true, false), (false, true)] {
        let now = Instant::now();
        let deadline = now + Duration::from_secs(1);
        let clock = Arc::new(ControlledOutcomeClock::new(now));
        let request = GatewayReconciliationRequest::new(
            correlation(if indeterminate {
                401
            } else if rejected {
                301
            } else {
                201
            }),
            ReconciliationEvidence::new(
                ReconciliationCause::DesktopQuitPolicy,
                ReconciliationInitiator::DesktopHost,
            )
            .unwrap(),
        );
        let attempt = GatewayReconciliationAttempt::new(
            correlation(if indeterminate {
                402
            } else if rejected {
                302
            } else {
                202
            }),
            request,
        )
        .unwrap();
        let gateway = reconciled(if indeterminate {
            "deadline-indeterminate"
        } else if rejected {
            "deadline-rejected"
        } else {
            "deadline-accepted"
        });
        let intended = gateway.audit_identity().unwrap();
        let session = Arc::new(GatewayStopSession::new(
            GatewayStopRequest::new(attempt.clone(), gateway, deadline),
            clock.clone(),
        ));
        let audit = Arc::new(RecordingAudit::default());
        let result = execute_stop_request(
            Arc::new(DeadlineAtSettlementHost {
                clock,
                deadline,
                rejected,
                indeterminate,
            }),
            audit.clone(),
            session.clone(),
            attempt,
            intended,
        );

        if indeterminate {
            assert_eq!(
                result,
                Err(GatewayError::Stop("native stop indeterminate".into()))
            );
        } else if rejected {
            assert_eq!(
                result,
                Err(GatewayError::Stop("native stop rejected".into()))
            );
        } else {
            assert!(matches!(
                result,
                Err(GatewayError::Stop(message))
                    if message.contains("deadline passed before outcome delivery")
            ));
        }
        assert_eq!(audit.physical_outcomes.load(Ordering::SeqCst), 0);
        assert!(session.settlement().is_ok());
    }
}

#[derive(Clone, Copy)]
enum StopOutcomeAuditBehavior {
    ErrorThenDeadline,
    PanicThenDeadline,
    InFlightSucceedsAtDeadline,
    RetrySucceeds,
    RetryFails,
    RetryPanics,
}

#[derive(Clone, Copy)]
enum StopCommandBehavior {
    Accepted,
    Rejected,
    Indeterminate,
}

#[test]
fn stop_outcome_delivery_preserves_attempt_retry_and_physical_facts() {
    struct OutcomeAudit {
        behavior: StopOutcomeAuditBehavior,
        clock: Arc<ControlledOutcomeClock>,
        deadline: Instant,
        physical_outcomes: AtomicUsize,
    }

    impl TestAuditBehavior for OutcomeAudit {
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

        fn physical_outcome(
            &self,
            _: &LifecyclePhysicalOutcome,
            _: Option<&LifecycleObservation>,
            _: ReconciliationCleanupDecision,
        ) -> Result<(), GatewayError> {
            let call = self.physical_outcomes.fetch_add(1, Ordering::SeqCst) + 1;
            match (self.behavior, call) {
                (StopOutcomeAuditBehavior::ErrorThenDeadline, 1) => {
                    self.clock.set(self.deadline);
                    Err(GatewayError::Registration("first sink error".into()))
                }
                (StopOutcomeAuditBehavior::PanicThenDeadline, 1) => {
                    self.clock.set(self.deadline);
                    panic!("first sink panic")
                }
                (StopOutcomeAuditBehavior::InFlightSucceedsAtDeadline, 1) => {
                    self.clock.set(self.deadline);
                    Ok(())
                }
                (StopOutcomeAuditBehavior::RetrySucceeds, 1)
                | (StopOutcomeAuditBehavior::RetryFails, 1)
                | (StopOutcomeAuditBehavior::RetryPanics, 1) => {
                    Err(GatewayError::Registration("first sink error".into()))
                }
                (StopOutcomeAuditBehavior::RetrySucceeds, 2) => Ok(()),
                (StopOutcomeAuditBehavior::RetryFails, 2) => {
                    Err(GatewayError::Registration("second sink error".into()))
                }
                (StopOutcomeAuditBehavior::RetryPanics, 2) => panic!("second sink panic"),
                _ => panic!("unexpected outcome call {call}"),
            }
        }
    }

    struct SettledHost(StopCommandBehavior);

    impl GatewayHost for SettledHost {
        fn register(
            &self,
            _: &Path,
            _: &str,
            _: Option<&SearchPath>,
            _: &GatewayReconciliationAttempt,
            _: &dyn GatewayReconciliationProgress,
        ) -> Result<ReconciledGateway, GatewayError> {
            unreachable!("the focused stop test does not register")
        }

        fn stop_agents(
            &self,
            session: &GatewayStopSession,
            journal: &dyn GatewayReconciliationJournalSession,
            plan: &AuditDeliveryReceipt,
        ) -> Result<LifecycleObservation, GatewayError> {
            match self.0 {
                StopCommandBehavior::Accepted => complete_stop(session, journal, plan, || Ok(())),
                StopCommandBehavior::Rejected => complete_stop(session, journal, plan, || {
                    Err(GatewayError::Stop("native stop rejected".into()))
                }),
                StopCommandBehavior::Indeterminate => {
                    let proof_token = session.begin_proof()?;
                    let candidate = session.request().intended().audit_identity()?;
                    session.prove(&proof_token, candidate.clone(), 1)?;
                    session.claim(proof_token, plan, &candidate, 1)?;
                    let command = LifecycleCommandResult::Indeterminate(
                        "native stop result remained unknown".into(),
                    );
                    session.command_result(command.clone())?;
                    journal.effect_completion(
                        "stop-agents-on-desktop-quit",
                        "signal-agents",
                        &command,
                    )?;
                    let observation = LifecycleObservation::new(2, Some(candidate), true);
                    journal.observation(
                        &LifecycleObservationSource::Effect {
                            plan_id: "stop-agents-on-desktop-quit".into(),
                            step_id: "signal-agents".into(),
                        },
                        &observation,
                    )?;
                    session.fresh_observation(observation)?;
                    Err(GatewayError::Stop("native stop indeterminate".into()))
                }
            }
        }
    }

    for command in [
        StopCommandBehavior::Accepted,
        StopCommandBehavior::Rejected,
        StopCommandBehavior::Indeterminate,
    ] {
        for behavior in [
            StopOutcomeAuditBehavior::ErrorThenDeadline,
            StopOutcomeAuditBehavior::PanicThenDeadline,
            StopOutcomeAuditBehavior::InFlightSucceedsAtDeadline,
            StopOutcomeAuditBehavior::RetrySucceeds,
            StopOutcomeAuditBehavior::RetryFails,
            StopOutcomeAuditBehavior::RetryPanics,
        ] {
            let now = Instant::now();
            let deadline = now + Duration::from_secs(1);
            let clock = Arc::new(ControlledOutcomeClock::new(now));
            let serial = match behavior {
                StopOutcomeAuditBehavior::ErrorThenDeadline => 501,
                StopOutcomeAuditBehavior::PanicThenDeadline => 511,
                StopOutcomeAuditBehavior::InFlightSucceedsAtDeadline => 521,
                StopOutcomeAuditBehavior::RetrySucceeds => 531,
                StopOutcomeAuditBehavior::RetryFails => 541,
                StopOutcomeAuditBehavior::RetryPanics => 551,
            } + match command {
                StopCommandBehavior::Accepted => 0,
                StopCommandBehavior::Rejected => 100,
                StopCommandBehavior::Indeterminate => 200,
            };
            let request = GatewayReconciliationRequest::new(
                correlation(serial),
                ReconciliationEvidence::new(
                    ReconciliationCause::DesktopQuitPolicy,
                    ReconciliationInitiator::DesktopHost,
                )
                .unwrap(),
            );
            let attempt =
                GatewayReconciliationAttempt::new(correlation(serial + 1), request).unwrap();
            let gateway = reconciled(match command {
                StopCommandBehavior::Accepted => "outcome-accepted",
                StopCommandBehavior::Rejected => "outcome-rejected",
                StopCommandBehavior::Indeterminate => "outcome-indeterminate",
            });
            let intended = gateway.audit_identity().unwrap();
            let session = Arc::new(GatewayStopSession::new(
                GatewayStopRequest::new(attempt.clone(), gateway, deadline),
                clock.clone(),
            ));
            let audit = Arc::new(OutcomeAudit {
                behavior,
                clock,
                deadline,
                physical_outcomes: AtomicUsize::new(0),
            });

            let result = execute_stop_request(
                Arc::new(SettledHost(command)),
                audit.clone(),
                session.clone(),
                attempt,
                intended,
            );

            let expected_calls = match behavior {
                StopOutcomeAuditBehavior::ErrorThenDeadline
                | StopOutcomeAuditBehavior::PanicThenDeadline
                | StopOutcomeAuditBehavior::InFlightSucceedsAtDeadline => 1,
                StopOutcomeAuditBehavior::RetrySucceeds
                | StopOutcomeAuditBehavior::RetryFails
                | StopOutcomeAuditBehavior::RetryPanics => 2,
            };
            assert_eq!(
                audit.physical_outcomes.load(Ordering::SeqCst),
                expected_calls
            );
            assert!(session.settlement().is_ok());

            if matches!(
                behavior,
                StopOutcomeAuditBehavior::RetrySucceeds
                    | StopOutcomeAuditBehavior::InFlightSucceedsAtDeadline
            ) {
                assert_eq!(
                    result,
                    match command {
                        StopCommandBehavior::Accepted => Ok(()),
                        StopCommandBehavior::Rejected => {
                            Err(GatewayError::Stop("native stop rejected".into()))
                        }
                        StopCommandBehavior::Indeterminate => {
                            Err(GatewayError::Stop("native stop indeterminate".into()))
                        }
                    }
                );
                continue;
            }

            let Err(GatewayError::Audit { audit, physical }) = result else {
                panic!("outcome failure lost audit or physical facts: {result:?}")
            };
            assert!(audit.contains("first attempt failed"));
            match behavior {
                StopOutcomeAuditBehavior::ErrorThenDeadline => {
                    assert!(audit.contains("first sink error"));
                    assert!(audit.contains("exact retry was denied"));
                    assert!(audit.contains("deadline passed before outcome delivery"));
                }
                StopOutcomeAuditBehavior::PanicThenDeadline => {
                    assert!(audit.contains("outcome audit adapter panicked"));
                    assert!(audit.contains("exact retry was denied"));
                }
                StopOutcomeAuditBehavior::RetryFails => {
                    assert!(audit.contains("first sink error"));
                    assert!(audit.contains("exact retry failed: second sink error"));
                }
                StopOutcomeAuditBehavior::RetryPanics => {
                    assert!(audit.contains("first sink error"));
                    assert!(audit.contains("exact retry failed"));
                    assert!(audit.contains("outcome audit adapter panicked"));
                }
                StopOutcomeAuditBehavior::RetrySucceeds
                | StopOutcomeAuditBehavior::InFlightSucceedsAtDeadline => unreachable!(),
            }
            match (command, physical) {
                (StopCommandBehavior::Accepted, Some(GatewayPhysicalResult::Succeeded)) => {}
                (StopCommandBehavior::Rejected, Some(GatewayPhysicalResult::Failed(error))) => {
                    assert_eq!(*error, GatewayError::Stop("native stop rejected".into()));
                }
                (
                    StopCommandBehavior::Indeterminate,
                    Some(GatewayPhysicalResult::Failed(error)),
                ) => {
                    assert_eq!(
                        *error,
                        GatewayError::Stop("native stop indeterminate".into())
                    );
                }
                (_, physical) => panic!("wrong settled physical result: {physical:?}"),
            }
        }
    }
}

#[test]
fn stop_outcome_is_audited_after_the_response_receiver_is_dropped() {
    struct CallerLossAudit {
        delivered: Mutex<bool>,
    }

    impl TestAuditBehavior for CallerLossAudit {
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

        fn physical_outcome(
            &self,
            _: &LifecyclePhysicalOutcome,
            _: Option<&LifecycleObservation>,
            _: ReconciliationCleanupDecision,
        ) -> Result<(), GatewayError> {
            *self.delivered.lock().unwrap() = true;
            Ok(())
        }
    }

    struct SettledHost;

    impl GatewayHost for SettledHost {
        fn register(
            &self,
            _: &Path,
            _: &str,
            _: Option<&SearchPath>,
            _: &GatewayReconciliationAttempt,
            _: &dyn GatewayReconciliationProgress,
        ) -> Result<ReconciledGateway, GatewayError> {
            unreachable!("the focused stop test does not register")
        }

        fn stop_agents(
            &self,
            session: &GatewayStopSession,
            journal: &dyn GatewayReconciliationJournalSession,
            plan: &AuditDeliveryReceipt,
        ) -> Result<LifecycleObservation, GatewayError> {
            complete_stop(session, journal, plan, || Ok(()))
        }
    }

    let deadline = Instant::now() + Duration::from_secs(60);
    let request = GatewayReconciliationRequest::new(
        correlation(801),
        ReconciliationEvidence::new(
            ReconciliationCause::DesktopQuitPolicy,
            ReconciliationInitiator::DesktopHost,
        )
        .unwrap(),
    );
    let attempt = GatewayReconciliationAttempt::new(correlation(802), request).unwrap();
    let gateway = reconciled("caller-loss");
    let intended = gateway.audit_identity().unwrap();
    let session = Arc::new(GatewayStopSession::new(
        GatewayStopRequest::new(attempt.clone(), gateway, deadline),
        Arc::new(SystemMonotonicClock),
    ));
    let audit = Arc::new(CallerLossAudit {
        delivered: Mutex::new(false),
    });

    let (receiver, worker) = spawn_stop_request(
        Arc::new(SettledHost),
        audit.clone(),
        session.clone(),
        attempt,
        intended,
    );
    drop(receiver);
    worker.join().unwrap();

    assert!(*audit.delivered.lock().unwrap());
    assert!(session.settlement().is_ok());
}

#[test]
fn stop_outcome_retry_returns_only_the_acknowledged_receipt() {
    let deadline = Instant::now() + Duration::from_secs(60);
    let request = GatewayReconciliationRequest::new(
        correlation(901),
        ReconciliationEvidence::new(
            ReconciliationCause::DesktopQuitPolicy,
            ReconciliationInitiator::DesktopHost,
        )
        .unwrap(),
    );
    let attempt = GatewayReconciliationAttempt::new(correlation(902), request).unwrap();
    let gateway = reconciled("receipt-retry");
    let intended = gateway.audit_identity().unwrap();
    let session = GatewayStopSession::new(
        GatewayStopRequest::new(attempt.clone(), gateway, deadline),
        Arc::new(SystemMonotonicClock),
    );
    let plan = AuditDeliveryReceipt::new(
        attempt.correlation().clone(),
        1,
        LifecycleRecordKind::EffectPlan,
    );
    let proof_token = session.begin_proof().unwrap();
    session.prove(&proof_token, intended.clone(), 1).unwrap();
    session.claim(proof_token, &plan, &intended, 1).unwrap();
    session
        .command_result(LifecycleCommandResult::Accepted)
        .unwrap();
    session
        .fresh_observation(LifecycleObservation::new(2, Some(intended), true))
        .unwrap();
    let expected = AuditDeliveryReceipt::new(
        attempt.correlation().clone(),
        77,
        LifecycleRecordKind::Outcome,
    );
    let mut calls = 0;

    let receipt = deliver_stop_outcome(&session, GatewayPhysicalResult::Succeeded, || {
        calls += 1;
        if calls == 1 {
            Err(GatewayError::Registration(
                "published record receipt was uncertain".into(),
            ))
        } else {
            Ok(expected.clone())
        }
    })
    .unwrap();

    assert_eq!(calls, 2);
    assert_eq!(receipt, expected);
}

#[derive(Clone, Copy, Debug)]
enum AuditedPhysicalResult {
    Confirmed,
    Refused,
    Partial,
}

struct AuditedHost {
    result: AuditedPhysicalResult,
    stopped: Mutex<Vec<String>>,
}

struct ConfigurationChangedHost;

impl GatewayHost for ConfigurationChangedHost {
    fn startup_cause(&self) -> ReconciliationCause {
        ReconciliationCause::ClaudeConfigurationChanged
    }

    fn register(
        &self,
        _: &Path,
        _: &str,
        _: Option<&SearchPath>,
        attempt: &GatewayReconciliationAttempt,
        progress: &dyn GatewayReconciliationProgress,
    ) -> Result<ReconciledGateway, GatewayError> {
        admit(attempt, progress, "configured");
        Ok(reconciled("configured"))
    }

    fn stop_agents(
        &self,
        session: &GatewayStopSession,
        journal: &dyn GatewayReconciliationJournalSession,
        plan: &AuditDeliveryReceipt,
    ) -> Result<LifecycleObservation, GatewayError> {
        complete_stop(session, journal, plan, || Ok(()))
    }
}

impl GatewayHost for AuditedHost {
    fn register(
        &self,
        _: &Path,
        _: &str,
        _: Option<&SearchPath>,
        attempt: &GatewayReconciliationAttempt,
        progress: &dyn GatewayReconciliationProgress,
    ) -> Result<ReconciledGateway, GatewayError> {
        let service = match self.result {
            AuditedPhysicalResult::Confirmed => "confirmed",
            AuditedPhysicalResult::Refused => "refused",
            AuditedPhysicalResult::Partial => "partial",
        };
        if matches!(self.result, AuditedPhysicalResult::Partial) {
            admit_fresh(attempt, progress, service);
        } else {
            admit(attempt, progress, service);
        }
        match self.result {
            AuditedPhysicalResult::Confirmed => Ok(reconciled(service)),
            AuditedPhysicalResult::Refused => {
                Err(GatewayError::Registration("registration refused".into()))
            }
            AuditedPhysicalResult::Partial => {
                progress.history_observed(ReconciliationHistoryFact::ServiceDefinitionPublished);
                progress.history_observed(ReconciliationHistoryFact::ServiceDefinitionDurable);
                progress.history_observed(ReconciliationHistoryFact::BootstrapCommandRequested);
                progress.history_observed(ReconciliationHistoryFact::BootstrapCommandCompleted);
                Err(GatewayError::Registration(
                    "bootstrap result unavailable".into(),
                ))
            }
        }
    }

    fn stop_agents(
        &self,
        session: &GatewayStopSession,
        journal: &dyn GatewayReconciliationJournalSession,
        plan: &AuditDeliveryReceipt,
    ) -> Result<LifecycleObservation, GatewayError> {
        let gateway = session.request().intended();
        complete_stop(session, journal, plan, || {
            self.stopped.lock().unwrap().push(gateway.service().into());
            Ok(())
        })
    }
}

#[derive(Default)]
struct RetryOnceAudit {
    intent_failed: AtomicBool,
    outcome_failed: AtomicBool,
    intent_calls: AtomicUsize,
    outcome_calls: AtomicUsize,
}

impl TestAuditBehavior for RetryOnceAudit {
    fn intent(&self, _: &GatewayReconciliationIntent) -> Result<(), GatewayError> {
        self.intent_calls.fetch_add(1, Ordering::SeqCst);
        if !self.intent_failed.swap(true, Ordering::SeqCst) {
            Err(GatewayError::Registration(
                "transient intent sync failure".into(),
            ))
        } else {
            Ok(())
        }
    }

    fn outcome(&self, _: &GatewayReconciliationOutcome) -> Result<(), GatewayError> {
        self.outcome_calls.fetch_add(1, Ordering::SeqCst);
        if !self.outcome_failed.swap(true, Ordering::SeqCst) {
            Err(GatewayError::Registration(
                "transient outcome sync failure".into(),
            ))
        } else {
            Ok(())
        }
    }

    fn joined(
        &self,
        _: &GatewayReconciliationAttempt,
        _: &GatewayReconciliationRequest,
    ) -> Result<(), GatewayError> {
        Ok(())
    }
}

#[test]
fn exact_delivery_retry_recovers_published_intent_and_outcome() {
    let audit = Arc::new(RetryOnceAudit::default());
    let gateway = Gateway::bootstrap(
        Arc::new(AuditedHost {
            result: AuditedPhysicalResult::Confirmed,
            stopped: Mutex::new(Vec::new()),
        }),
        login_shell("/usr/bin"),
        testing::discard_startup_events(),
        testing::sequential_reconciliation_ids(),
        audit.clone(),
        "/runtime".into(),
        "ci".into(),
    );

    tauri::async_runtime::block_on(gateway.start()).unwrap();
    assert_eq!(audit.intent_calls.load(Ordering::SeqCst), 2);
    assert_eq!(audit.outcome_calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        gateway.startup().unwrap().phase(),
        &GatewayStartupPhase::Ready
    );
}

#[derive(Default)]
struct ObservationRetryAudit {
    attempts: Mutex<Vec<(LifecycleObservationSource, LifecycleObservation)>>,
    failures_remaining: AtomicUsize,
    outcomes: AtomicUsize,
}

impl TestAuditBehavior for ObservationRetryAudit {
    fn intent(&self, _: &GatewayReconciliationIntent) -> Result<(), GatewayError> {
        Ok(())
    }

    fn outcome(&self, _: &GatewayReconciliationOutcome) -> Result<(), GatewayError> {
        self.outcomes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn joined(
        &self,
        _: &GatewayReconciliationAttempt,
        _: &GatewayReconciliationRequest,
    ) -> Result<(), GatewayError> {
        Ok(())
    }

    fn observation(
        &self,
        source: &LifecycleObservationSource,
        observation: &LifecycleObservation,
    ) -> Result<(), GatewayError> {
        self.attempts
            .lock()
            .unwrap()
            .push((source.clone(), observation.clone()));
        if self
            .failures_remaining
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
        {
            Err(GatewayError::Registration(
                "observation acknowledgement unavailable".into(),
            ))
        } else {
            Ok(())
        }
    }
}

struct ObservationRetryHost;

impl GatewayHost for ObservationRetryHost {
    fn register(
        &self,
        _: &Path,
        _: &str,
        _: Option<&SearchPath>,
        attempt: &GatewayReconciliationAttempt,
        progress: &dyn GatewayReconciliationProgress,
    ) -> Result<ReconciledGateway, GatewayError> {
        admit_fresh(
            attempt,
            progress,
            "gui/501/so.nessa.gateway.observation-retry",
        );
        let first = progress.physical_observed(&LifecycleObservationSource::Intent, None, false);
        assert!(matches!(first, Err(GatewayError::Registration(_))));
        let retried = progress.retry_pending_observation()?.unwrap();
        assert_eq!(retried.version(), 1);
        Err(GatewayError::Registration("test finished".into()))
    }

    fn stop_agents(
        &self,
        _: &GatewayStopSession,
        _: &dyn GatewayReconciliationJournalSession,
        _: &AuditDeliveryReceipt,
    ) -> Result<LifecycleObservation, GatewayError> {
        unreachable!()
    }
}

#[test]
fn an_unacknowledged_observation_retries_the_identical_record_before_progressing() {
    let audit = Arc::new(ObservationRetryAudit {
        attempts: Mutex::new(Vec::new()),
        failures_remaining: AtomicUsize::new(2),
        outcomes: AtomicUsize::new(0),
    });
    let gateway = Gateway::bootstrap(
        Arc::new(ObservationRetryHost),
        login_shell("/usr/bin"),
        testing::discard_startup_events(),
        testing::sequential_reconciliation_ids(),
        audit.clone(),
        "/runtime".into(),
        "ci".into(),
    );

    assert_eq!(
        tauri::async_runtime::block_on(gateway.start()),
        Err(GatewayError::Registration("test finished".into()))
    );
    let attempts = audit.attempts.lock().unwrap();
    assert_eq!(attempts.len(), 3);
    assert!(attempts.windows(2).all(|pair| pair[0] == pair[1]));
    drop(attempts);
    assert_eq!(audit.outcomes.load(Ordering::SeqCst), 1);
}

#[derive(Default)]
struct PersistentCompletionFailureAudit {
    completions: AtomicUsize,
    outcomes: AtomicUsize,
}

impl TestAuditBehavior for PersistentCompletionFailureAudit {
    fn intent(&self, _: &GatewayReconciliationIntent) -> Result<(), GatewayError> {
        Ok(())
    }

    fn outcome(&self, _: &GatewayReconciliationOutcome) -> Result<(), GatewayError> {
        self.outcomes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn joined(
        &self,
        _: &GatewayReconciliationAttempt,
        _: &GatewayReconciliationRequest,
    ) -> Result<(), GatewayError> {
        Ok(())
    }

    fn effect_completion(
        &self,
        _: &str,
        _: &str,
        _: &LifecycleCommandResult,
    ) -> Result<(), GatewayError> {
        self.completions.fetch_add(1, Ordering::SeqCst);
        Err(GatewayError::Registration(
            "completion acknowledgement unavailable".into(),
        ))
    }
}

struct PersistentCompletionFailureHost;

impl GatewayHost for PersistentCompletionFailureHost {
    fn register(
        &self,
        _: &Path,
        _: &str,
        _: Option<&SearchPath>,
        attempt: &GatewayReconciliationAttempt,
        progress: &dyn GatewayReconciliationProgress,
    ) -> Result<ReconciledGateway, GatewayError> {
        let gateway = reconciled("completion-failure");
        let target = gateway.audit_identity()?.target().clone();
        let intent = GatewayReconciliationIntent::new(attempt.clone(), target.clone(), None)?;
        progress.intent_admitted(intent)?;
        let step = LifecyclePlanStep::new(
            "primary".into(),
            LifecycleEffect::AdoptReadyIncarnation { target },
            LifecycleEffectPredicate::Always,
        )
        .unwrap();
        progress.effect_planned("adopt", &step, &[])?;
        progress.history_observed(ReconciliationHistoryFact::ServiceDefinitionPublished);
        progress.effect_completed("adopt", "primary", &LifecycleCommandResult::Accepted)?;
        Ok(gateway)
    }

    fn stop_agents(
        &self,
        _: &GatewayStopSession,
        _: &dyn GatewayReconciliationJournalSession,
        _: &AuditDeliveryReceipt,
    ) -> Result<LifecycleObservation, GatewayError> {
        unreachable!()
    }
}

#[test]
fn persistent_completion_failure_never_attempts_a_terminal_outcome() {
    let audit = Arc::new(PersistentCompletionFailureAudit::default());
    let gateway = Gateway::bootstrap(
        Arc::new(PersistentCompletionFailureHost),
        login_shell("/usr/bin"),
        testing::discard_startup_events(),
        testing::sequential_reconciliation_ids(),
        audit.clone(),
        "/runtime".into(),
        "ci".into(),
    );

    assert!(matches!(
        tauri::async_runtime::block_on(gateway.start()),
        Err(GatewayError::Registration(message))
            if message.contains("completion acknowledgement unavailable")
    ));
    assert_eq!(audit.completions.load(Ordering::SeqCst), 2);
    assert_eq!(audit.outcomes.load(Ordering::SeqCst), 0);
}

#[test]
fn confirmed_refused_and_partial_effects_are_audited_with_delivery_failures_separate() {
    for physical in [
        AuditedPhysicalResult::Confirmed,
        AuditedPhysicalResult::Refused,
        AuditedPhysicalResult::Partial,
    ] {
        for fail_outcome in [false, true] {
            let audit = Arc::new(RecordingAudit {
                fail_outcome,
                ..RecordingAudit::default()
            });
            let host = Arc::new(AuditedHost {
                result: physical,
                stopped: Mutex::new(Vec::new()),
            });
            let gateway = Gateway::bootstrap(
                host.clone(),
                login_shell("/usr/bin"),
                testing::discard_startup_events(),
                testing::sequential_reconciliation_ids(),
                audit.clone(),
                "/runtime".into(),
                "ci".into(),
            );

            let result = tauri::async_runtime::block_on(gateway.start());
            match (physical, fail_outcome, &result) {
                (AuditedPhysicalResult::Confirmed, false, Ok(())) => {}
                (
                    AuditedPhysicalResult::Confirmed,
                    true,
                    Err(GatewayError::Audit { physical, .. }),
                ) => {
                    assert_eq!(physical, &Some(GatewayPhysicalResult::Succeeded));
                }
                (
                    AuditedPhysicalResult::Refused | AuditedPhysicalResult::Partial,
                    false,
                    Err(GatewayError::Registration(_)),
                ) => {}
                (
                    AuditedPhysicalResult::Refused | AuditedPhysicalResult::Partial,
                    true,
                    Err(GatewayError::Audit {
                        physical: Some(_), ..
                    }),
                ) => {}
                _ => panic!(
                    "unexpected result for {physical:?}, audit failure {fail_outcome}: {result:?}"
                ),
            }
            match (physical, fail_outcome, gateway.startup().unwrap().phase()) {
                (AuditedPhysicalResult::Confirmed, false, GatewayStartupPhase::Ready) => {}
                (_, true, GatewayStartupPhase::Failed(GatewayError::Audit { .. })) => {}
                (
                    AuditedPhysicalResult::Refused | AuditedPhysicalResult::Partial,
                    false,
                    GatewayStartupPhase::Failed(GatewayError::Registration(_)),
                ) => {}
                (_, _, startup) => panic!(
                    "unexpected startup projection for {physical:?}, audit failure {fail_outcome}: {startup:?}"
                ),
            }

            let intents = audit.intents.lock().unwrap();
            let outcomes = audit.outcomes.lock().unwrap();
            assert_eq!(intents.len(), 1);
            assert_eq!(outcomes.len(), if fail_outcome { 2 } else { 1 });
            assert_eq!(outcomes[0].intent(), &intents[0]);
            let attempt = intents[0].attempt();
            assert_ne!(attempt.correlation(), attempt.origin().correlation());
            assert_eq!(
                attempt.origin().evidence().cause(),
                ReconciliationCause::Startup
            );
            assert_eq!(
                attempt.origin().evidence().initiator(),
                ReconciliationInitiator::DesktopHost
            );
            match (physical, outcomes[0].effect()) {
                (
                    AuditedPhysicalResult::Confirmed,
                    GatewayReconciliationEffect::Confirmed { after, .. },
                ) => assert_eq!(after.target(), intents[0].target()),
                (
                    AuditedPhysicalResult::Refused,
                    GatewayReconciliationEffect::Failed { history, .. },
                ) => assert!(history.facts().is_empty()),
                (
                    AuditedPhysicalResult::Partial,
                    GatewayReconciliationEffect::Failed { history, .. },
                ) => {
                    assert_eq!(
                        history.facts(),
                        &[
                            ReconciliationHistoryFact::ServiceDefinitionPublished,
                            ReconciliationHistoryFact::ServiceDefinitionDurable,
                            ReconciliationHistoryFact::BootstrapCommandRequested,
                            ReconciliationHistoryFact::BootstrapCommandCompleted,
                        ]
                    );
                }
                _ => panic!("audit effect disagreed with {physical:?}"),
            }
            drop(outcomes);
            drop(intents);

            if matches!(physical, AuditedPhysicalResult::Confirmed) {
                gateway
                    .stop_agents(Instant::now() + Duration::from_secs(30))
                    .unwrap();
                assert_eq!(*host.stopped.lock().unwrap(), ["confirmed"]);
            } else {
                assert_eq!(
                    gateway.stop_agents(Instant::now() + Duration::from_secs(30)),
                    Err(GatewayError::NotReconciled)
                );
            }
        }
    }
}

#[test]
fn every_reconciliation_entry_point_preserves_its_cause_and_verified_initiator() {
    let audit = Arc::new(RecordingAudit::default());
    let gateway = Gateway::bootstrap(
        Arc::new(AuditedHost {
            result: AuditedPhysicalResult::Confirmed,
            stopped: Mutex::new(Vec::new()),
        }),
        login_shell("/usr/bin"),
        testing::discard_startup_events(),
        testing::sequential_reconciliation_ids(),
        audit.clone(),
        "/runtime".into(),
        "ci".into(),
    );

    tauri::async_runtime::block_on(gateway.start()).unwrap();
    tauri::async_runtime::block_on(gateway.wait_ready(BundledSurface::Main)).unwrap();
    tauri::async_runtime::block_on(gateway.retry(BundledSurface::Setup)).unwrap();
    tauri::async_runtime::block_on(gateway.configuration_changed(BundledSurface::Main)).unwrap();

    let intents = audit.intents.lock().unwrap();
    let evidence = intents
        .iter()
        .map(|intent| {
            let attempt = intent.attempt();
            assert_ne!(attempt.correlation(), attempt.origin().correlation());
            (
                attempt.origin().evidence().cause(),
                attempt.origin().evidence().initiator(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        evidence,
        [
            (
                ReconciliationCause::Startup,
                ReconciliationInitiator::DesktopHost,
            ),
            (
                ReconciliationCause::CredentialLoad,
                ReconciliationInitiator::BundledSurface(BundledSurface::Main),
            ),
            (
                ReconciliationCause::ExplicitRetry,
                ReconciliationInitiator::BundledSurface(BundledSurface::Setup),
            ),
            (
                ReconciliationCause::ClaudeConfigurationChanged,
                ReconciliationInitiator::BundledSurface(BundledSurface::Main),
            ),
        ]
    );
    assert_eq!(audit.outcomes.lock().unwrap().len(), 4);
}

#[test]
fn desktop_start_audits_a_durable_provider_configuration_change() {
    let audit = Arc::new(RecordingAudit::default());
    let gateway = Gateway::bootstrap(
        Arc::new(ConfigurationChangedHost),
        login_shell("/usr/bin"),
        testing::discard_startup_events(),
        testing::sequential_reconciliation_ids(),
        audit.clone(),
        "/runtime".into(),
        "ci".into(),
    );

    tauri::async_runtime::block_on(gateway.start()).unwrap();

    let intents = audit.intents.lock().unwrap();
    assert_eq!(intents.len(), 1);
    assert_eq!(
        intents[0].attempt().origin().evidence().cause(),
        ReconciliationCause::ClaudeConfigurationChanged
    );
    assert_eq!(
        intents[0].attempt().origin().evidence().initiator(),
        ReconciliationInitiator::DesktopHost
    );
    assert_eq!(audit.outcomes.lock().unwrap().len(), 1);
}

#[test]
fn initial_request_id_failure_advances_startup_without_calling_the_host() {
    let host = Arc::new(FakeHost {
        registration: Ok(reconciled("must-not-run")),
        stop_result: Ok(()),
        calls: Mutex::new(Vec::new()),
    });
    let ids = Arc::new(QueuedReconciliationIds(Mutex::new(vec![Err(
        GatewayError::Registration("request id unavailable".into()),
    )])));
    let gateway = Gateway::bootstrap(
        host.clone(),
        login_shell("/usr/bin"),
        testing::discard_startup_events(),
        ids,
        testing::discard_reconciliation_audit(),
        "/runtime".into(),
        "ci".into(),
    );

    assert_eq!(
        tauri::async_runtime::block_on(gateway.start()),
        Err(GatewayError::Registration("request id unavailable".into()))
    );
    let startup = gateway.startup().unwrap();
    assert_eq!(startup.revision(), 1);
    assert_eq!(
        startup.phase(),
        &GatewayStartupPhase::Failed(GatewayError::Registration("request id unavailable".into()))
    );
    assert!(host.calls.lock().unwrap().is_empty());
}

#[test]
fn initial_attempt_id_failure_advances_startup_without_installing_a_receipt() {
    let host = Arc::new(FakeHost {
        registration: Ok(reconciled("must-not-run")),
        stop_result: Ok(()),
        calls: Mutex::new(Vec::new()),
    });
    let ids = Arc::new(QueuedReconciliationIds(Mutex::new(vec![
        Ok(correlation(1)),
        Err(GatewayError::Registration("attempt id unavailable".into())),
    ])));
    let gateway = Gateway::bootstrap(
        host.clone(),
        login_shell("/usr/bin"),
        testing::discard_startup_events(),
        ids,
        testing::discard_reconciliation_audit(),
        "/runtime".into(),
        "ci".into(),
    );

    assert_eq!(
        tauri::async_runtime::block_on(gateway.start()),
        Err(GatewayError::Registration("attempt id unavailable".into()))
    );
    assert!(matches!(
        gateway.startup().unwrap().phase(),
        GatewayStartupPhase::Failed(_)
    ));
    assert!(gateway.lifecycle.lock().unwrap().running.is_none());
    assert!(host.calls.lock().unwrap().is_empty());
}

#[test]
fn allocator_panic_is_typed_and_a_later_retry_can_own_a_fresh_receipt() {
    struct PanickingOnceIds(AtomicUsize);

    impl GatewayReconciliationIds for PanickingOnceIds {
        fn next(&self) -> Result<ReconciliationCorrelation, GatewayError> {
            let call = self.0.fetch_add(1, Ordering::SeqCst);
            assert_ne!(call, 0, "allocator panic");
            Ok(correlation(call as u64))
        }
    }

    let host = Arc::new(FakeHost {
        registration: Ok(reconciled("retry-after-id-panic")),
        stop_result: Ok(()),
        calls: Mutex::new(Vec::new()),
    });
    let gateway = Gateway::bootstrap(
        host.clone(),
        login_shell("/usr/bin"),
        testing::discard_startup_events(),
        Arc::new(PanickingOnceIds(AtomicUsize::new(0))),
        testing::discard_reconciliation_audit(),
        "/runtime".into(),
        "ci".into(),
    );

    assert_eq!(
        tauri::async_runtime::block_on(gateway.start()),
        Err(GatewayError::Registration(
            "gateway reconciliation id adapter panicked".into()
        ))
    );
    assert!(matches!(
        gateway.startup().unwrap().phase(),
        GatewayStartupPhase::Failed(_)
    ));
    tauri::async_runtime::block_on(gateway.retry(BundledSurface::Main)).unwrap();
    assert_eq!(
        gateway.startup().unwrap().phase(),
        &GatewayStartupPhase::Ready
    );
    assert_eq!(host.calls.lock().unwrap().len(), 1);
}

#[test]
fn id_allocation_can_inspect_the_startup_snapshot_without_holding_lifecycle_state() {
    #[derive(Default)]
    struct SnapshotIds {
        calls: AtomicUsize,
        gateway: Mutex<Option<Weak<Gateway>>>,
    }

    impl GatewayReconciliationIds for SnapshotIds {
        fn next(&self) -> Result<ReconciliationCorrelation, GatewayError> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            assert_eq!(
                self.gateway
                    .lock()
                    .unwrap()
                    .as_ref()
                    .and_then(Weak::upgrade)
                    .unwrap()
                    .startup()
                    .unwrap()
                    .phase(),
                &GatewayStartupPhase::Starting
            );
            Ok(correlation(call as u64))
        }
    }

    let ids = Arc::new(SnapshotIds::default());
    let gateway = Arc::new(Gateway::bootstrap(
        Arc::new(FakeHost {
            registration: Ok(reconciled("allocator-snapshot")),
            stop_result: Ok(()),
            calls: Mutex::new(Vec::new()),
        }),
        login_shell("/usr/bin"),
        testing::discard_startup_events(),
        ids.clone(),
        testing::discard_reconciliation_audit(),
        "/runtime".into(),
        "ci".into(),
    ));
    *ids.gateway.lock().unwrap() = Some(Arc::downgrade(&gateway));

    tauri::async_runtime::block_on(gateway.start()).unwrap();
    assert_eq!(ids.calls.load(Ordering::SeqCst), 2);
}

#[test]
fn joining_request_id_failure_does_not_replace_the_active_owner_projection() {
    struct BlockingOwnerHost {
        entered: Mutex<Option<mpsc::Sender<()>>>,
        release: Mutex<mpsc::Receiver<()>>,
        stopped: Mutex<Vec<String>>,
    }

    impl GatewayHost for BlockingOwnerHost {
        fn register(
            &self,
            _: &Path,
            _: &str,
            _: Option<&SearchPath>,
            attempt: &GatewayReconciliationAttempt,
            progress: &dyn GatewayReconciliationProgress,
        ) -> Result<ReconciledGateway, GatewayError> {
            let gateway = reconciled("active-owner");
            admit(attempt, progress, gateway.service());
            self.entered
                .lock()
                .unwrap()
                .take()
                .unwrap()
                .send(())
                .unwrap();
            self.release.lock().unwrap().recv().unwrap();
            Ok(gateway)
        }

        fn stop_agents(
            &self,
            session: &GatewayStopSession,
            journal: &dyn GatewayReconciliationJournalSession,
            plan: &AuditDeliveryReceipt,
        ) -> Result<LifecycleObservation, GatewayError> {
            let gateway = session.request().intended();
            complete_stop(session, journal, plan, || {
                self.stopped.lock().unwrap().push(gateway.service().into());
                Ok(())
            })
        }
    }

    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let host = Arc::new(BlockingOwnerHost {
        entered: Mutex::new(Some(entered_tx)),
        release: Mutex::new(release_rx),
        stopped: Mutex::new(Vec::new()),
    });
    let audit = Arc::new(RecordingAudit::default());
    let gateway = Arc::new(Gateway::bootstrap(
        host.clone(),
        login_shell("/usr/bin"),
        testing::discard_startup_events(),
        Arc::new(QueuedReconciliationIds(Mutex::new(vec![
            Ok(correlation(1)),
            Ok(correlation(2)),
            Err(GatewayError::Registration("join id unavailable".into())),
            Ok(correlation(4)),
            Ok(correlation(5)),
        ]))),
        audit.clone(),
        "/runtime".into(),
        "ci".into(),
    ));
    let (owner_result_tx, owner_result_rx) = mpsc::channel();
    let owner = {
        let gateway = gateway.clone();
        thread::spawn(move || {
            owner_result_tx
                .send(tauri::async_runtime::block_on(gateway.start()))
                .unwrap();
        })
    };
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();

    assert_eq!(
        tauri::async_runtime::block_on(gateway.retry(BundledSurface::Main)),
        Err(GatewayError::Registration("join id unavailable".into()))
    );
    assert_eq!(gateway.startup().unwrap().revision(), 0);
    assert_eq!(
        gateway.startup().unwrap().phase(),
        &GatewayStartupPhase::Starting
    );

    release_tx.send(()).unwrap();
    owner_result_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    owner.join().unwrap();
    assert_eq!(
        gateway.startup().unwrap().phase(),
        &GatewayStartupPhase::Ready
    );
    assert_eq!(audit.intents.lock().unwrap().len(), 1);
    assert!(audit.joined.lock().unwrap().is_empty());
    gateway
        .stop_agents(Instant::now() + Duration::from_secs(30))
        .unwrap();
    assert_eq!(*host.stopped.lock().unwrap(), ["active-owner"]);
    assert_eq!(audit.intents.lock().unwrap().len(), 2);
    assert_eq!(audit.outcomes.lock().unwrap().len(), 1);
}

#[test]
fn failed_intent_delivery_is_terminally_audited_and_cannot_be_replaced() {
    struct IgnoringIntentFailureHost(Mutex<Vec<String>>);

    impl GatewayHost for IgnoringIntentFailureHost {
        fn register(
            &self,
            _: &Path,
            _: &str,
            _: Option<&SearchPath>,
            attempt: &GatewayReconciliationAttempt,
            progress: &dyn GatewayReconciliationProgress,
        ) -> Result<ReconciledGateway, GatewayError> {
            let gateway = reconciled("ignored-intent-failure");
            let target = gateway.audit_identity().unwrap().target().clone();
            let before = gateway.audit_identity().unwrap();
            let intent =
                GatewayReconciliationIntent::new(attempt.clone(), target, Some(before)).unwrap();
            assert!(progress.intent_admitted(intent.clone()).is_err());
            assert!(matches!(
                progress.intent_admitted(intent),
                Err(GatewayError::Registration(_))
            ));
            Ok(gateway)
        }

        fn stop_agents(
            &self,
            session: &GatewayStopSession,
            journal: &dyn GatewayReconciliationJournalSession,
            plan: &AuditDeliveryReceipt,
        ) -> Result<LifecycleObservation, GatewayError> {
            let gateway = session.request().intended();
            complete_stop(session, journal, plan, || {
                self.0.lock().unwrap().push(gateway.service().into());
                Ok(())
            })
        }
    }

    for (panic_intent, fail_outcome) in [(false, false), (false, true), (true, false), (true, true)]
    {
        let audit = Arc::new(RecordingAudit {
            fail_intent: !panic_intent,
            panic_intent,
            fail_outcome,
            ..RecordingAudit::default()
        });
        let host = Arc::new(IgnoringIntentFailureHost(Mutex::new(Vec::new())));
        let gateway = Gateway::bootstrap(
            host.clone(),
            login_shell("/usr/bin"),
            testing::discard_startup_events(),
            testing::sequential_reconciliation_ids(),
            audit.clone(),
            "/runtime".into(),
            "ci".into(),
        );

        assert!(matches!(
            tauri::async_runtime::block_on(gateway.start()),
            Err(GatewayError::Audit { .. })
        ));
        assert_eq!(
            audit.intents.lock().unwrap().len(),
            if panic_intent { 1 } else { 2 }
        );
        let outcomes = audit.outcomes.lock().unwrap();
        assert_eq!(outcomes.len(), if fail_outcome { 2 } else { 1 });
        assert!(matches!(
            outcomes[0].intent_delivery(),
            GatewayReconciliationIntentDelivery::Failed(GatewayError::Audit { .. })
        ));
        assert!(matches!(
            outcomes[0].effect(),
            GatewayReconciliationEffect::RejectedReport { report, .. }
                if report.cleanup() == ReconciliationCleanupDecision::AdoptClaimed
        ));
        drop(outcomes);
        assert!(matches!(
            gateway.startup().unwrap().phase(),
            GatewayStartupPhase::Failed(_)
        ));
        assert!(matches!(
            gateway.stop_agents(Instant::now() + Duration::from_secs(30)),
            Err(GatewayError::Audit { physical: None, .. })
        ));
        assert!(host.0.lock().unwrap().is_empty());
    }
}

#[test]
fn concurrent_duplicate_intents_are_refused_before_a_second_sink_delivery() {
    #[derive(Default)]
    struct BlockingIntentAudit {
        calls: AtomicUsize,
        entered: Mutex<bool>,
        released: Mutex<bool>,
        entered_changed: Condvar,
        released_changed: Condvar,
        gateway: Mutex<Option<Weak<Gateway>>>,
        outcomes: Mutex<Vec<GatewayReconciliationOutcome>>,
    }

    impl BlockingIntentAudit {
        fn wait_until_entered(&self) {
            let (entered, timeout) = self
                .entered_changed
                .wait_timeout_while(
                    self.entered.lock().unwrap(),
                    Duration::from_secs(2),
                    |entered| !*entered,
                )
                .unwrap();
            assert!(
                *entered && !timeout.timed_out(),
                "intent audit did not start"
            );
        }

        fn release(&self) {
            *self.released.lock().unwrap() = true;
            self.released_changed.notify_all();
        }
    }

    impl TestAuditBehavior for BlockingIntentAudit {
        fn intent(&self, _: &GatewayReconciliationIntent) -> Result<(), GatewayError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(
                self.gateway
                    .lock()
                    .unwrap()
                    .as_ref()
                    .and_then(Weak::upgrade)
                    .unwrap()
                    .startup()
                    .unwrap()
                    .phase(),
                &GatewayStartupPhase::Starting
            );
            *self.entered.lock().unwrap() = true;
            self.entered_changed.notify_all();
            let mut released = self.released.lock().unwrap();
            while !*released {
                released = self.released_changed.wait(released).unwrap();
            }
            Ok(())
        }

        fn outcome(&self, outcome: &GatewayReconciliationOutcome) -> Result<(), GatewayError> {
            self.outcomes.lock().unwrap().push(outcome.clone());
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

    struct DuplicateIntentHost {
        audit: Arc<BlockingIntentAudit>,
        conflicting_target: bool,
    }

    impl GatewayHost for DuplicateIntentHost {
        fn register(
            &self,
            _: &Path,
            _: &str,
            _: Option<&SearchPath>,
            attempt: &GatewayReconciliationAttempt,
            progress: &dyn GatewayReconciliationProgress,
        ) -> Result<ReconciledGateway, GatewayError> {
            let gateway = reconciled("first-intent");
            let before = gateway.audit_identity().unwrap();
            let first = GatewayReconciliationIntent::new(
                attempt.clone(),
                before.target().clone(),
                Some(before),
            )
            .unwrap();
            let duplicate_target = if self.conflicting_target {
                ReconciliationTarget::new(
                    "conflicting-intent".into(),
                    "a".repeat(64),
                    "b".repeat(64),
                )
                .unwrap()
            } else {
                first.target().clone()
            };
            let duplicate =
                GatewayReconciliationIntent::new(attempt.clone(), duplicate_target, None).unwrap();
            thread::scope(|scope| {
                let first_delivery = scope.spawn(|| progress.intent_admitted(first));
                self.audit.wait_until_entered();
                assert!(matches!(
                    progress.intent_admitted(duplicate),
                    Err(GatewayError::Registration(_))
                ));
                self.audit.release();
                first_delivery.join().unwrap().unwrap();
            });
            Ok(gateway)
        }

        fn stop_agents(
            &self,
            session: &GatewayStopSession,
            journal: &dyn GatewayReconciliationJournalSession,
            plan: &AuditDeliveryReceipt,
        ) -> Result<LifecycleObservation, GatewayError> {
            complete_stop(session, journal, plan, || Ok(()))
        }
    }

    for conflicting_target in [false, true] {
        let audit = Arc::new(BlockingIntentAudit::default());
        let gateway = Arc::new(Gateway::bootstrap(
            Arc::new(DuplicateIntentHost {
                audit: audit.clone(),
                conflicting_target,
            }),
            login_shell("/usr/bin"),
            testing::discard_startup_events(),
            testing::sequential_reconciliation_ids(),
            audit.clone(),
            "/runtime".into(),
            "ci".into(),
        ));
        *audit.gateway.lock().unwrap() = Some(Arc::downgrade(&gateway));

        tauri::async_runtime::block_on(gateway.start()).unwrap();
        assert_eq!(audit.calls.load(Ordering::SeqCst), 1);
        assert_eq!(audit.outcomes.lock().unwrap().len(), 1);
        assert_eq!(
            gateway.startup().unwrap().phase(),
            &GatewayStartupPhase::Ready
        );
    }
}

#[test]
fn effects_before_intent_acknowledgement_remain_rejected_after_late_acknowledgement() {
    #[derive(Clone, Copy)]
    enum EarlyEffects {
        BeforeReservation,
        BeforeAcknowledgement,
    }

    #[derive(Default)]
    struct TimingAudit {
        block_intent: bool,
        entered: Mutex<bool>,
        released: Mutex<bool>,
        entered_changed: Condvar,
        released_changed: Condvar,
        outcomes: Mutex<Vec<GatewayReconciliationOutcome>>,
    }

    impl TimingAudit {
        fn wait_until_entered(&self) {
            let (entered, timeout) = self
                .entered_changed
                .wait_timeout_while(
                    self.entered.lock().unwrap(),
                    Duration::from_secs(2),
                    |entered| !*entered,
                )
                .unwrap();
            assert!(
                *entered && !timeout.timed_out(),
                "intent sink did not block"
            );
        }

        fn release(&self) {
            *self.released.lock().unwrap() = true;
            self.released_changed.notify_all();
        }
    }

    impl TestAuditBehavior for TimingAudit {
        fn intent(&self, _: &GatewayReconciliationIntent) -> Result<(), GatewayError> {
            if self.block_intent {
                *self.entered.lock().unwrap() = true;
                self.entered_changed.notify_all();
                let mut released = self.released.lock().unwrap();
                while !*released {
                    released = self.released_changed.wait(released).unwrap();
                }
            }
            Ok(())
        }

        fn outcome(&self, outcome: &GatewayReconciliationOutcome) -> Result<(), GatewayError> {
            self.outcomes.lock().unwrap().push(outcome.clone());
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

    struct EarlyEffectsHost {
        timing: EarlyEffects,
        audit: Arc<TimingAudit>,
        stopped: Mutex<Vec<String>>,
    }

    impl EarlyEffectsHost {
        fn report_effects(progress: &dyn GatewayReconciliationProgress) {
            for fact in [
                ReconciliationHistoryFact::ServiceDefinitionPublished,
                ReconciliationHistoryFact::ServiceDefinitionDurable,
                ReconciliationHistoryFact::BootstrapCommandRequested,
                ReconciliationHistoryFact::BootstrapCommandCompleted,
                ReconciliationHistoryFact::BootstrapCommandSucceeded,
            ] {
                progress.history_observed(fact);
            }
        }
    }

    impl GatewayHost for EarlyEffectsHost {
        fn register(
            &self,
            _: &Path,
            _: &str,
            _: Option<&SearchPath>,
            attempt: &GatewayReconciliationAttempt,
            progress: &dyn GatewayReconciliationProgress,
        ) -> Result<ReconciledGateway, GatewayError> {
            let gateway = reconciled("early-effects");
            let intent = GatewayReconciliationIntent::new(
                attempt.clone(),
                gateway.audit_identity().unwrap().target().clone(),
                None,
            )
            .unwrap();
            match self.timing {
                EarlyEffects::BeforeReservation => {
                    Self::report_effects(progress);
                    progress.intent_admitted(intent)?;
                }
                EarlyEffects::BeforeAcknowledgement => thread::scope(|scope| {
                    let delivery = scope.spawn(|| progress.intent_admitted(intent));
                    self.audit.wait_until_entered();
                    Self::report_effects(progress);
                    self.audit.release();
                    delivery.join().unwrap().unwrap();
                }),
            }
            Ok(gateway)
        }

        fn stop_agents(
            &self,
            session: &GatewayStopSession,
            journal: &dyn GatewayReconciliationJournalSession,
            plan: &AuditDeliveryReceipt,
        ) -> Result<LifecycleObservation, GatewayError> {
            let gateway = session.request().intended();
            complete_stop(session, journal, plan, || {
                self.stopped.lock().unwrap().push(gateway.service().into());
                Ok(())
            })
        }
    }

    for timing in [
        EarlyEffects::BeforeReservation,
        EarlyEffects::BeforeAcknowledgement,
    ] {
        let audit = Arc::new(TimingAudit {
            block_intent: matches!(timing, EarlyEffects::BeforeAcknowledgement),
            ..TimingAudit::default()
        });
        let host = Arc::new(EarlyEffectsHost {
            timing,
            audit: audit.clone(),
            stopped: Mutex::new(Vec::new()),
        });
        let gateway = Gateway::bootstrap(
            host.clone(),
            login_shell("/usr/bin"),
            testing::discard_startup_events(),
            testing::sequential_reconciliation_ids(),
            audit.clone(),
            "/runtime".into(),
            "ci".into(),
        );

        assert!(tauri::async_runtime::block_on(gateway.start()).is_err());
        let outcomes = audit.outcomes.lock().unwrap();
        assert_eq!(outcomes.len(), 1);
        assert_eq!(
            outcomes[0].intent_delivery(),
            &GatewayReconciliationIntentDelivery::Acknowledged
        );
        let expected_timing = match timing {
            EarlyEffects::BeforeReservation => {
                GatewayReconciliationEffectTiming::BeforeIntentReservation
            }
            EarlyEffects::BeforeAcknowledgement => {
                GatewayReconciliationEffectTiming::BeforeIntentAcknowledgement
            }
        };
        assert_eq!(outcomes[0].effect_timing(), expected_timing);
        let GatewayReconciliationEffect::RejectedReport { report, .. } = outcomes[0].effect()
        else {
            panic!("effects before intent acknowledgement were accepted")
        };
        assert!(!report.validation().effects_followed_intent());
        assert!(report.validation().effect_timing_matches_history());
        assert_eq!(
            report.cleanup(),
            ReconciliationCleanupDecision::AdoptClaimed
        );
        drop(outcomes);
        assert!(matches!(
            gateway.startup().unwrap().phase(),
            GatewayStartupPhase::Failed(_)
        ));
        gateway
            .stop_agents(Instant::now() + Duration::from_secs(30))
            .unwrap();
        assert_eq!(*host.stopped.lock().unwrap(), ["early-effects"]);
    }
}

#[test]
fn panicking_outcome_audit_is_reported_without_losing_confirmed_stop_identity() {
    let audit = Arc::new(RecordingAudit {
        panic_outcome: true,
        ..RecordingAudit::default()
    });
    let host = Arc::new(AuditedHost {
        result: AuditedPhysicalResult::Confirmed,
        stopped: Mutex::new(Vec::new()),
    });
    let gateway = Gateway::bootstrap(
        host.clone(),
        login_shell("/usr/bin"),
        testing::discard_startup_events(),
        testing::sequential_reconciliation_ids(),
        audit,
        "/runtime".into(),
        "ci".into(),
    );

    assert!(matches!(
        tauri::async_runtime::block_on(gateway.start()),
        Err(GatewayError::Audit {
            physical: Some(GatewayPhysicalResult::Succeeded),
            ..
        })
    ));
    gateway
        .stop_agents(Instant::now() + Duration::from_secs(30))
        .unwrap();
    assert_eq!(*host.stopped.lock().unwrap(), ["confirmed"]);
}

#[test]
fn joined_audit_failure_rejects_only_the_joiner_and_does_not_cancel_the_owner() {
    struct BlockingAuditedHost {
        entered: Mutex<Option<mpsc::Sender<()>>>,
        release: Mutex<mpsc::Receiver<()>>,
    }

    impl GatewayHost for BlockingAuditedHost {
        fn register(
            &self,
            _: &Path,
            _: &str,
            _: Option<&SearchPath>,
            attempt: &GatewayReconciliationAttempt,
            progress: &dyn GatewayReconciliationProgress,
        ) -> Result<ReconciledGateway, GatewayError> {
            let gateway = reconciled("joined-audit-owner");
            admit(attempt, progress, gateway.service());
            self.entered
                .lock()
                .unwrap()
                .take()
                .unwrap()
                .send(())
                .unwrap();
            self.release.lock().unwrap().recv().unwrap();
            Ok(gateway)
        }

        fn stop_agents(
            &self,
            session: &GatewayStopSession,
            journal: &dyn GatewayReconciliationJournalSession,
            plan: &AuditDeliveryReceipt,
        ) -> Result<LifecycleObservation, GatewayError> {
            complete_stop(session, journal, plan, || Ok(()))
        }
    }

    let audit = Arc::new(RecordingAudit {
        fail_joined: true,
        ..RecordingAudit::default()
    });
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let gateway = Arc::new(Gateway::bootstrap(
        Arc::new(BlockingAuditedHost {
            entered: Mutex::new(Some(entered_tx)),
            release: Mutex::new(release_rx),
        }),
        login_shell("/usr/bin"),
        testing::discard_startup_events(),
        testing::sequential_reconciliation_ids(),
        audit.clone(),
        "/runtime".into(),
        "ci".into(),
    ));
    let (owner_result_tx, owner_result_rx) = mpsc::channel();
    let owner = {
        let gateway = gateway.clone();
        thread::spawn(move || {
            owner_result_tx
                .send(tauri::async_runtime::block_on(gateway.start()))
                .unwrap();
        })
    };
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();

    assert!(matches!(
        tauri::async_runtime::block_on(gateway.retry(BundledSurface::Main)),
        Err(GatewayError::Audit { physical: None, .. })
    ));
    release_tx.send(()).unwrap();
    owner_result_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    owner.join().unwrap();

    assert_eq!(audit.joined.lock().unwrap().len(), 1);
    assert_eq!(audit.outcomes.lock().unwrap().len(), 1);
    assert_eq!(
        gateway.startup().unwrap().phase(),
        &GatewayStartupPhase::Ready
    );
}

struct DelayedJoinedAudit {
    intents: Mutex<Vec<GatewayReconciliationIntent>>,
    outcomes: Mutex<Vec<GatewayReconciliationOutcome>>,
    joined: Mutex<Vec<(GatewayReconciliationAttempt, GatewayReconciliationRequest)>>,
    joined_entered: mpsc::Sender<()>,
    joined_release: Mutex<mpsc::Receiver<()>>,
}

impl TestAuditBehavior for DelayedJoinedAudit {
    fn intent(&self, intent: &GatewayReconciliationIntent) -> Result<(), GatewayError> {
        self.intents.lock().unwrap().push(intent.clone());
        Ok(())
    }

    fn outcome(&self, outcome: &GatewayReconciliationOutcome) -> Result<(), GatewayError> {
        self.outcomes.lock().unwrap().push(outcome.clone());
        Ok(())
    }

    fn joined(
        &self,
        attempt: &GatewayReconciliationAttempt,
        joined: &GatewayReconciliationRequest,
    ) -> Result<(), GatewayError> {
        self.joined
            .lock()
            .unwrap()
            .push((attempt.clone(), joined.clone()));
        self.joined_entered.send(()).unwrap();
        self.joined_release.lock().unwrap().recv().unwrap();
        Ok(())
    }
}

struct InterleavedHost {
    calls: Mutex<usize>,
    attempts: Mutex<Vec<GatewayReconciliationAttempt>>,
    a_entered: mpsc::Sender<()>,
    a_release: Mutex<mpsc::Receiver<()>>,
    c_entered: mpsc::Sender<()>,
    c_release: Mutex<mpsc::Receiver<()>>,
}

impl GatewayHost for InterleavedHost {
    fn register(
        &self,
        _: &Path,
        _: &str,
        _: Option<&SearchPath>,
        attempt: &GatewayReconciliationAttempt,
        progress: &dyn GatewayReconciliationProgress,
    ) -> Result<ReconciledGateway, GatewayError> {
        let call = {
            let mut calls = self.calls.lock().unwrap();
            let call = *calls;
            *calls += 1;
            call
        };
        self.attempts.lock().unwrap().push(attempt.clone());
        let service = match call {
            0 => "attempt-a",
            1 => "attempt-c",
            _ => panic!("unexpected native attempt"),
        };
        admit(attempt, progress, service);
        match call {
            0 => {
                self.a_entered.send(()).unwrap();
                self.a_release.lock().unwrap().recv().unwrap();
                Ok(reconciled(service))
            }
            1 => {
                self.c_entered.send(()).unwrap();
                self.c_release.lock().unwrap().recv().unwrap();
                Err(GatewayError::Registration("attempt C refused".into()))
            }
            _ => unreachable!(),
        }
    }

    fn stop_agents(
        &self,
        session: &GatewayStopSession,
        journal: &dyn GatewayReconciliationJournalSession,
        plan: &AuditDeliveryReceipt,
    ) -> Result<LifecycleObservation, GatewayError> {
        complete_stop(session, journal, plan, || Ok(()))
    }
}

#[test]
fn delayed_joiner_returns_exact_attempt_a_after_attempt_c_has_started() {
    let (joined_entered_tx, joined_entered_rx) = mpsc::channel();
    let (joined_release_tx, joined_release_rx) = mpsc::channel();
    let audit = Arc::new(DelayedJoinedAudit {
        intents: Mutex::new(Vec::new()),
        outcomes: Mutex::new(Vec::new()),
        joined: Mutex::new(Vec::new()),
        joined_entered: joined_entered_tx,
        joined_release: Mutex::new(joined_release_rx),
    });
    let (a_entered_tx, a_entered_rx) = mpsc::channel();
    let (a_release_tx, a_release_rx) = mpsc::channel();
    let (c_entered_tx, c_entered_rx) = mpsc::channel();
    let (c_release_tx, c_release_rx) = mpsc::channel();
    let host = Arc::new(InterleavedHost {
        calls: Mutex::new(0),
        attempts: Mutex::new(Vec::new()),
        a_entered: a_entered_tx,
        a_release: Mutex::new(a_release_rx),
        c_entered: c_entered_tx,
        c_release: Mutex::new(c_release_rx),
    });
    let gateway = Arc::new(Gateway::bootstrap(
        host.clone(),
        login_shell("/usr/bin"),
        testing::discard_startup_events(),
        testing::sequential_reconciliation_ids(),
        audit.clone(),
        "/runtime".into(),
        "ci".into(),
    ));
    let (a_result_tx, a_result_rx) = mpsc::channel();
    let a = {
        let gateway = gateway.clone();
        thread::spawn(move || {
            a_result_tx
                .send(tauri::async_runtime::block_on(gateway.start()))
                .unwrap();
        })
    };
    a_entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();

    let (b_result_tx, b_result_rx) = mpsc::channel();
    let b = {
        let gateway = gateway.clone();
        thread::spawn(move || {
            b_result_tx
                .send(tauri::async_runtime::block_on(
                    gateway.retry(BundledSurface::Main),
                ))
                .unwrap();
        })
    };
    joined_entered_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap();

    a_release_tx.send(()).unwrap();
    a_result_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    a.join().unwrap();

    let (c_result_tx, c_result_rx) = mpsc::channel();
    let c = {
        let gateway = gateway.clone();
        thread::spawn(move || {
            c_result_tx
                .send(tauri::async_runtime::block_on(
                    gateway.configuration_changed(BundledSurface::Setup),
                ))
                .unwrap();
        })
    };
    c_entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();

    joined_release_tx.send(()).unwrap();
    b_result_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    b.join().unwrap();

    c_release_tx.send(()).unwrap();
    assert_eq!(
        c_result_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        Err(GatewayError::Registration("attempt C refused".into()))
    );
    c.join().unwrap();

    let attempts = host.attempts.lock().unwrap();
    let joined = audit.joined.lock().unwrap();
    assert_eq!(attempts.len(), 2);
    assert_eq!(joined.len(), 1);
    assert_eq!(joined[0].0, attempts[0]);
    assert_ne!(
        joined[0].0.correlation(),
        joined[0].0.origin().correlation()
    );
    assert_ne!(
        joined[0].1.correlation(),
        joined[0].0.origin().correlation()
    );
    assert_eq!(
        joined[0].0.origin().evidence().cause(),
        ReconciliationCause::Startup
    );
    assert_eq!(
        joined[0].0.origin().evidence().initiator(),
        ReconciliationInitiator::DesktopHost
    );
    assert_eq!(
        joined[0].1.evidence().cause(),
        ReconciliationCause::ExplicitRetry
    );
    assert_eq!(
        joined[0].1.evidence().initiator(),
        ReconciliationInitiator::BundledSurface(BundledSurface::Main)
    );
    assert_eq!(
        attempts[1].origin().evidence().cause(),
        ReconciliationCause::ClaudeConfigurationChanged
    );
    assert_eq!(
        attempts[1].origin().evidence().initiator(),
        ReconciliationInitiator::BundledSurface(BundledSurface::Setup)
    );
    assert_ne!(attempts[0].correlation(), attempts[1].correlation());
    assert_eq!(audit.intents.lock().unwrap().len(), 2);
    assert_eq!(audit.outcomes.lock().unwrap().len(), 2);
}

#[test]
fn configuration_change_during_an_attempt_owns_exactly_one_successor() {
    struct SuccessorHost {
        attempts: Mutex<Vec<GatewayReconciliationAttempt>>,
        entered: [mpsc::Sender<()>; 3],
        release: [Mutex<mpsc::Receiver<()>>; 3],
    }

    impl GatewayHost for SuccessorHost {
        fn register(
            &self,
            _: &Path,
            _: &str,
            _: Option<&SearchPath>,
            attempt: &GatewayReconciliationAttempt,
            progress: &dyn GatewayReconciliationProgress,
        ) -> Result<ReconciledGateway, GatewayError> {
            let call = {
                let mut attempts = self.attempts.lock().unwrap();
                attempts.push(attempt.clone());
                attempts.len() - 1
            };
            let service = match call {
                0 => "configuration-predecessor",
                1 => "configuration-successor",
                2 => "later-configuration-successor",
                _ => panic!("configuration change started more than one successor"),
            };
            let gateway = reconciled(service);
            admit(attempt, progress, gateway.service());
            self.entered[call].send(()).unwrap();
            self.release[call].lock().unwrap().recv().unwrap();
            Ok(gateway)
        }

        fn stop_agents(
            &self,
            session: &GatewayStopSession,
            journal: &dyn GatewayReconciliationJournalSession,
            plan: &AuditDeliveryReceipt,
        ) -> Result<LifecycleObservation, GatewayError> {
            complete_stop(session, journal, plan, || Ok(()))
        }
    }

    let (a_entered_tx, a_entered_rx) = mpsc::channel();
    let (a_release_tx, a_release_rx) = mpsc::channel();
    let (successor_entered_tx, successor_entered_rx) = mpsc::channel();
    let (successor_release_tx, successor_release_rx) = mpsc::channel();
    let (later_entered_tx, later_entered_rx) = mpsc::channel();
    let (later_release_tx, later_release_rx) = mpsc::channel();
    let host = Arc::new(SuccessorHost {
        attempts: Mutex::new(Vec::new()),
        entered: [a_entered_tx, successor_entered_tx, later_entered_tx],
        release: [
            Mutex::new(a_release_rx),
            Mutex::new(successor_release_rx),
            Mutex::new(later_release_rx),
        ],
    });
    let audit = Arc::new(RecordingAudit::default());
    let gateway = Arc::new(Gateway::bootstrap(
        host.clone(),
        login_shell("/usr/bin"),
        testing::discard_startup_events(),
        testing::sequential_reconciliation_ids(),
        audit.clone(),
        "/runtime".into(),
        "ci".into(),
    ));
    let (a_result_tx, a_result_rx) = mpsc::channel();
    let a = {
        let gateway = gateway.clone();
        thread::spawn(move || {
            a_result_tx
                .send(tauri::async_runtime::block_on(gateway.start()))
                .unwrap();
        })
    };
    a_entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    gateway.caller_resume_gate.arm();

    let (successor_result_tx, successor_result_rx) = mpsc::channel();
    let successor = {
        let gateway = gateway.clone();
        thread::spawn(move || {
            successor_result_tx
                .send(tauri::async_runtime::block_on(
                    gateway.configuration_changed(BundledSurface::Main),
                ))
                .unwrap();
        })
    };
    let first = gateway
        .wait_for_pending_receipt()
        .expect("configuration successor was not admitted");
    assert!(
        first.wait_for_waiters(1),
        "configuration change did not wait for A"
    );

    let (credential_result_tx, credential_result_rx) = mpsc::channel();
    let credential = {
        let gateway = gateway.clone();
        thread::spawn(move || {
            credential_result_tx
                .send(tauri::async_runtime::block_on(
                    gateway.wait_ready(BundledSurface::Setup),
                ))
                .unwrap();
        })
    };
    assert!(matches!(
        credential_result_rx.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));

    a_release_tx.send(()).unwrap();
    a_result_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    a.join().unwrap();
    successor_entered_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap();
    assert!(
        audit.wait_for_joined(1),
        "credential join was not audited when the successor journal opened"
    );
    assert!(matches!(
        successor_result_rx.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));

    successor_release_tx.send(()).unwrap();
    assert!(
        gateway.caller_resume_gate.wait_until_arrived(),
        "successor caller resumed before its receipt settled"
    );
    credential_result_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    credential.join().unwrap();
    assert!(matches!(
        successor_result_rx.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    gateway.caller_resume_gate.release();
    successor_result_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    successor.join().unwrap();

    let (later_result_tx, later_result_rx) = mpsc::channel();
    let later = {
        let gateway = gateway.clone();
        thread::spawn(move || {
            later_result_tx
                .send(tauri::async_runtime::block_on(
                    gateway.configuration_changed(BundledSurface::Setup),
                ))
                .unwrap();
        })
    };
    later_entered_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap();
    assert_eq!(host.attempts.lock().unwrap().len(), 3);
    later_release_tx.send(()).unwrap();
    later_result_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    later.join().unwrap();

    let attempts = host.attempts.lock().unwrap();
    assert_eq!(attempts.len(), 3);
    assert_eq!(
        attempts[0].origin().evidence().cause(),
        ReconciliationCause::Startup
    );
    assert_eq!(
        attempts[1].origin().evidence().cause(),
        ReconciliationCause::ClaudeConfigurationChanged
    );
    assert_eq!(
        attempts[1].origin().evidence().initiator(),
        ReconciliationInitiator::BundledSurface(BundledSurface::Main)
    );
    assert_eq!(
        attempts[2].origin().evidence().cause(),
        ReconciliationCause::ClaudeConfigurationChanged
    );
}

#[derive(Default)]
struct RecordingEvents {
    observations: Mutex<Vec<GatewayStartup>>,
    changed: Condvar,
}

impl RecordingEvents {
    fn wait_for(&self, expected: usize) -> Vec<GatewayStartup> {
        let (observations, timeout) = self
            .changed
            .wait_timeout_while(
                self.observations.lock().unwrap(),
                Duration::from_secs(2),
                |observations| observations.len() < expected,
            )
            .unwrap();
        assert!(
            observations.len() >= expected && !timeout.timed_out(),
            "startup events did not reach the caller-owned port"
        );
        observations.clone()
    }
}

impl GatewayStartupEvents for RecordingEvents {
    fn publish(&self, startup: &GatewayStartup) {
        self.observations.lock().unwrap().push(startup.clone());
        self.changed.notify_all();
    }
}

#[test]
fn a_later_credential_load_retries_failed_startup_with_revisioned_events() {
    struct RecoveringHost(Mutex<Vec<Result<ReconciledGateway, GatewayError>>>);

    impl GatewayHost for RecoveringHost {
        fn register(
            &self,
            _: &Path,
            _: &str,
            _: Option<&SearchPath>,
            attempt: &GatewayReconciliationAttempt,
            progress: &dyn GatewayReconciliationProgress,
        ) -> Result<ReconciledGateway, GatewayError> {
            let registration = self.0.lock().unwrap().remove(0);
            let service = registration
                .as_ref()
                .map_or("unconfirmed", |gateway| gateway.service());
            admit(attempt, progress, service);
            registration
        }

        fn stop_agents(
            &self,
            session: &GatewayStopSession,
            journal: &dyn GatewayReconciliationJournalSession,
            plan: &AuditDeliveryReceipt,
        ) -> Result<LifecycleObservation, GatewayError> {
            complete_stop(session, journal, plan, || Ok(()))
        }
    }

    let events = Arc::new(RecordingEvents::default());
    let gateway = Gateway::bootstrap(
        Arc::new(RecoveringHost(Mutex::new(vec![
            Err(GatewayError::Registration("not yet".into())),
            Ok(reconciled("recovered")),
        ]))),
        login_shell("/usr/bin"),
        events.clone(),
        testing::sequential_reconciliation_ids(),
        testing::discard_reconciliation_audit(),
        "/runtime".into(),
        "ci".into(),
    );

    assert!(tauri::async_runtime::block_on(gateway.wait_ready(BundledSurface::Main)).is_err());
    let failed = gateway.startup().unwrap();
    assert_eq!(failed.revision(), 1);
    assert!(matches!(failed.phase(), GatewayStartupPhase::Failed(_)));

    tauri::async_runtime::block_on(gateway.wait_ready(BundledSurface::Main)).unwrap();
    let ready = gateway.startup().unwrap();
    assert_eq!(ready.revision(), 3);
    assert_eq!(ready.phase(), &GatewayStartupPhase::Ready);

    let mut revisions = events
        .wait_for(3)
        .iter()
        .map(GatewayStartup::revision)
        .collect::<Vec<_>>();
    revisions.sort_unstable();
    assert_eq!(revisions, [1, 2, 3]);
}

#[test]
fn a_changed_identity_advances_ready_even_if_native_progress_was_missed() {
    struct ReplacingHost {
        registrations: Mutex<Vec<ReconciledGateway>>,
        stopped: Mutex<Vec<String>>,
    }

    impl GatewayHost for ReplacingHost {
        fn register(
            &self,
            _: &Path,
            _: &str,
            _: Option<&SearchPath>,
            attempt: &GatewayReconciliationAttempt,
            progress: &dyn GatewayReconciliationProgress,
        ) -> Result<ReconciledGateway, GatewayError> {
            let registration = self.registrations.lock().unwrap().remove(0);
            admit(attempt, progress, registration.service());
            Ok(registration)
        }

        fn stop_agents(
            &self,
            session: &GatewayStopSession,
            journal: &dyn GatewayReconciliationJournalSession,
            plan: &AuditDeliveryReceipt,
        ) -> Result<LifecycleObservation, GatewayError> {
            let gateway = session.request().intended();
            complete_stop(session, journal, plan, || {
                self.stopped
                    .lock()
                    .unwrap()
                    .push(gateway.service().to_owned());
                Ok(())
            })
        }
    }

    let events = Arc::new(RecordingEvents::default());
    let host = Arc::new(ReplacingHost {
        registrations: Mutex::new(vec![
            reconciled("first"),
            reconciled("replacement"),
            reconciled("replacement"),
        ]),
        stopped: Mutex::new(vec![]),
    });
    let gateway = Gateway::bootstrap(
        host.clone(),
        login_shell("/usr/bin"),
        events.clone(),
        testing::sequential_reconciliation_ids(),
        testing::discard_reconciliation_audit(),
        "/runtime".into(),
        "ci".into(),
    );

    tauri::async_runtime::block_on(gateway.wait_ready(BundledSurface::Main)).unwrap();
    tauri::async_runtime::block_on(gateway.wait_ready(BundledSurface::Main)).unwrap();
    tauri::async_runtime::block_on(gateway.wait_ready(BundledSurface::Main)).unwrap();

    assert!(host.registrations.lock().unwrap().is_empty());
    gateway
        .stop_agents(Instant::now() + Duration::from_secs(30))
        .unwrap();
    assert_eq!(*host.stopped.lock().unwrap(), ["replacement"]);
    let mut observations = events.wait_for(2);
    observations.sort_by_key(GatewayStartup::revision);
    assert_eq!(observations.len(), 2);
    assert_eq!(observations[0].phase(), &GatewayStartupPhase::Ready);
    assert_eq!(observations[0].revision(), 1);
    assert_eq!(observations[1].phase(), &GatewayStartupPhase::Ready);
    assert_eq!(observations[1].revision(), 2);
}

#[test]
fn native_restart_progress_transitions_ready_through_starting() {
    struct RestartingHost(Mutex<u8>);

    impl GatewayHost for RestartingHost {
        fn register(
            &self,
            _: &Path,
            _: &str,
            _: Option<&SearchPath>,
            attempt: &GatewayReconciliationAttempt,
            progress: &dyn GatewayReconciliationProgress,
        ) -> Result<ReconciledGateway, GatewayError> {
            let mut calls = self.0.lock().unwrap();
            *calls += 1;
            if *calls == 2 {
                progress.readiness_invalidated();
                let gateway = reconciled("restarted");
                admit(attempt, progress, gateway.service());
                Ok(gateway)
            } else {
                let gateway = reconciled("first");
                admit(attempt, progress, gateway.service());
                Ok(gateway)
            }
        }

        fn stop_agents(
            &self,
            session: &GatewayStopSession,
            journal: &dyn GatewayReconciliationJournalSession,
            plan: &AuditDeliveryReceipt,
        ) -> Result<LifecycleObservation, GatewayError> {
            complete_stop(session, journal, plan, || Ok(()))
        }
    }

    let events = Arc::new(RecordingEvents::default());
    let gateway = Gateway::bootstrap(
        Arc::new(RestartingHost(Mutex::new(0))),
        login_shell("/usr/bin"),
        events.clone(),
        testing::sequential_reconciliation_ids(),
        testing::discard_reconciliation_audit(),
        "/runtime".into(),
        "ci".into(),
    );
    tauri::async_runtime::block_on(gateway.wait_ready(BundledSurface::Main)).unwrap();
    tauri::async_runtime::block_on(gateway.wait_ready(BundledSurface::Main)).unwrap();

    let mut observations = events.wait_for(3);
    observations.sort_by_key(GatewayStartup::revision);
    assert_eq!(observations.len(), 3);
    assert_eq!(observations[0].phase(), &GatewayStartupPhase::Ready);
    assert_eq!(observations[1].phase(), &GatewayStartupPhase::Starting);
    assert_eq!(observations[2].phase(), &GatewayStartupPhase::Ready);
    assert_eq!(
        observations
            .iter()
            .map(GatewayStartup::revision)
            .collect::<Vec<_>>(),
        [1, 2, 3]
    );
}

#[test]
fn receipt_settlement_does_not_wait_for_startup_event_delivery() {
    struct BlockingEvents {
        entered: Mutex<Option<mpsc::Sender<GatewayStartup>>>,
        release: Mutex<mpsc::Receiver<()>>,
    }

    impl GatewayStartupEvents for BlockingEvents {
        fn publish(&self, startup: &GatewayStartup) {
            if let Some(entered) = self.entered.lock().unwrap().take() {
                entered.send(startup.clone()).unwrap();
            }
            self.release.lock().unwrap().recv().unwrap();
        }
    }

    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let events = Arc::new(BlockingEvents {
        entered: Mutex::new(Some(entered_tx)),
        release: Mutex::new(release_rx),
    });
    let gateway = Arc::new(Gateway::bootstrap(
        Arc::new(FakeHost {
            registration: Ok(reconciled("settled")),
            stop_result: Ok(()),
            calls: Mutex::new(vec![]),
        }),
        login_shell("/usr/bin"),
        events,
        testing::sequential_reconciliation_ids(),
        testing::discard_reconciliation_audit(),
        "/runtime".into(),
        "ci".into(),
    ));
    let (result_tx, result_rx) = mpsc::channel();
    let caller = {
        let gateway = gateway.clone();
        thread::spawn(move || {
            result_tx
                .send(tauri::async_runtime::block_on(
                    gateway.wait_ready(BundledSurface::Main),
                ))
                .unwrap();
        })
    };

    let delivered = entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(delivered.phase(), &GatewayStartupPhase::Ready);
    assert_eq!(delivered.revision(), 1);
    result_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("receipt did not settle while event delivery was blocked")
        .unwrap();
    release_tx.send(()).unwrap();
    caller.join().unwrap();
}

#[test]
fn panicking_host_and_event_adapters_cannot_strand_startup() {
    struct PanickingHost;
    impl GatewayHost for PanickingHost {
        fn register(
            &self,
            _: &Path,
            _: &str,
            _: Option<&SearchPath>,
            _: &GatewayReconciliationAttempt,
            _: &dyn GatewayReconciliationProgress,
        ) -> Result<ReconciledGateway, GatewayError> {
            panic!("adapter panic")
        }

        fn stop_agents(
            &self,
            session: &GatewayStopSession,
            journal: &dyn GatewayReconciliationJournalSession,
            plan: &AuditDeliveryReceipt,
        ) -> Result<LifecycleObservation, GatewayError> {
            complete_stop(session, journal, plan, || Ok(()))
        }
    }
    struct PanickingEvents;
    impl GatewayStartupEvents for PanickingEvents {
        fn publish(&self, _: &GatewayStartup) {
            panic!("event panic")
        }
    }

    let gateway = Gateway::bootstrap(
        Arc::new(PanickingHost),
        login_shell("/usr/bin"),
        Arc::new(PanickingEvents),
        testing::sequential_reconciliation_ids(),
        testing::discard_reconciliation_audit(),
        "/runtime".into(),
        "ci".into(),
    );
    for _ in 0..2 {
        assert!(tauri::async_runtime::block_on(gateway.wait_ready(BundledSurface::Main)).is_err());
        assert!(matches!(
            gateway.startup().unwrap().phase(),
            GatewayStartupPhase::Failed(_)
        ));
    }
}

#[test]
fn panicking_predecessor_settles_and_promotes_the_exact_pending_receipt() {
    struct PanickingPredecessor {
        calls: Mutex<usize>,
        entered: Mutex<Option<mpsc::Sender<()>>>,
        release: Mutex<mpsc::Receiver<()>>,
    }
    impl GatewayHost for PanickingPredecessor {
        fn register(
            &self,
            _: &Path,
            _: &str,
            _: Option<&SearchPath>,
            attempt: &GatewayReconciliationAttempt,
            progress: &dyn GatewayReconciliationProgress,
        ) -> Result<ReconciledGateway, GatewayError> {
            let call = {
                let mut calls = self.calls.lock().unwrap();
                *calls += 1;
                *calls
            };
            if call == 1 {
                self.entered
                    .lock()
                    .unwrap()
                    .take()
                    .unwrap()
                    .send(())
                    .unwrap();
                self.release.lock().unwrap().recv().unwrap();
                panic!("predecessor panic");
            }
            let gateway = reconciled("promoted-after-panic");
            admit(attempt, progress, gateway.service());
            Ok(gateway)
        }
        fn stop_agents(
            &self,
            session: &GatewayStopSession,
            journal: &dyn GatewayReconciliationJournalSession,
            plan: &AuditDeliveryReceipt,
        ) -> Result<LifecycleObservation, GatewayError> {
            complete_stop(session, journal, plan, || Ok(()))
        }
    }
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let host = Arc::new(PanickingPredecessor {
        calls: Mutex::new(0),
        entered: Mutex::new(Some(entered_tx)),
        release: Mutex::new(release_rx),
    });
    let gateway = Arc::new(Gateway::bootstrap(
        host.clone(),
        login_shell("/usr/bin"),
        testing::discard_startup_events(),
        testing::sequential_reconciliation_ids(),
        testing::discard_reconciliation_audit(),
        "/runtime".into(),
        "ci".into(),
    ));
    let predecessor = {
        let gateway = gateway.clone();
        thread::spawn(move || tauri::async_runtime::block_on(gateway.start()))
    };
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let successor = {
        let gateway = gateway.clone();
        thread::spawn(move || {
            tauri::async_runtime::block_on(gateway.configuration_changed(BundledSurface::Main))
        })
    };
    let pending = gateway
        .wait_for_pending_receipt()
        .expect("successor was not admitted");
    assert!(pending.wait_for_waiters(1));
    release_tx.send(()).unwrap();
    assert!(predecessor.join().unwrap().is_err());
    successor.join().unwrap().unwrap();
    assert_eq!(*host.calls.lock().unwrap(), 2);
}

#[test]
fn concurrent_waiters_run_one_registration_and_share_its_ready_identity() {
    struct BlockingHost {
        entered: Mutex<Option<mpsc::Sender<()>>>,
        release: Mutex<mpsc::Receiver<()>>,
        registrations: Mutex<u32>,
    }

    impl GatewayHost for BlockingHost {
        fn register(
            &self,
            _: &Path,
            _: &str,
            _: Option<&SearchPath>,
            attempt: &GatewayReconciliationAttempt,
            progress: &dyn GatewayReconciliationProgress,
        ) -> Result<ReconciledGateway, GatewayError> {
            *self.registrations.lock().unwrap() += 1;
            if let Some(entered) = self.entered.lock().unwrap().take() {
                entered.send(()).unwrap();
            }
            self.release.lock().unwrap().recv().unwrap();
            let gateway = reconciled("shared");
            admit(attempt, progress, gateway.service());
            Ok(gateway)
        }

        fn stop_agents(
            &self,
            session: &GatewayStopSession,
            journal: &dyn GatewayReconciliationJournalSession,
            plan: &AuditDeliveryReceipt,
        ) -> Result<LifecycleObservation, GatewayError> {
            complete_stop(session, journal, plan, || Ok(()))
        }
    }

    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let host = Arc::new(BlockingHost {
        entered: Mutex::new(Some(entered_tx)),
        release: Mutex::new(release_rx),
        registrations: Mutex::new(0),
    });
    let gateway = Arc::new(Gateway::bootstrap(
        host.clone(),
        login_shell("/usr/bin"),
        testing::discard_startup_events(),
        testing::sequential_reconciliation_ids(),
        testing::discard_reconciliation_audit(),
        "/runtime".into(),
        "ci".into(),
    ));

    let first = {
        let gateway = gateway.clone();
        thread::spawn(move || {
            tauri::async_runtime::block_on(gateway.wait_ready(BundledSurface::Main))
        })
    };
    entered_rx.recv().unwrap();
    let second = {
        let gateway = gateway.clone();
        thread::spawn(move || {
            tauri::async_runtime::block_on(gateway.wait_ready(BundledSurface::Main))
        })
    };
    let attempt = gateway.lifecycle.lock().unwrap().running.clone().unwrap();
    assert!(
        attempt.wait_for_waiters(2),
        "both callers did not join the same receipt in time"
    );
    release_tx.send(()).unwrap();

    first.join().unwrap().unwrap();
    second.join().unwrap().unwrap();
    assert_eq!(*host.registrations.lock().unwrap(), 1);
}

#[test]
fn caller_cancelled_after_admission_but_before_owner_launch_cannot_strand_the_receipt() {
    struct BlockingHost {
        entered: Mutex<Option<mpsc::Sender<()>>>,
        release: Mutex<mpsc::Receiver<()>>,
        registrations: Mutex<u32>,
    }

    impl GatewayHost for BlockingHost {
        fn register(
            &self,
            _: &Path,
            _: &str,
            _: Option<&SearchPath>,
            attempt: &GatewayReconciliationAttempt,
            progress: &dyn GatewayReconciliationProgress,
        ) -> Result<ReconciledGateway, GatewayError> {
            *self.registrations.lock().unwrap() += 1;
            if let Some(entered) = self.entered.lock().unwrap().take() {
                entered.send(()).unwrap();
            }
            self.release.lock().unwrap().recv().unwrap();
            let gateway = reconciled("shared-after-cancellation");
            admit(attempt, progress, gateway.service());
            Ok(gateway)
        }

        fn stop_agents(
            &self,
            session: &GatewayStopSession,
            journal: &dyn GatewayReconciliationJournalSession,
            plan: &AuditDeliveryReceipt,
        ) -> Result<LifecycleObservation, GatewayError> {
            complete_stop(session, journal, plan, || Ok(()))
        }
    }

    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let host = Arc::new(BlockingHost {
        entered: Mutex::new(Some(entered_tx)),
        release: Mutex::new(release_rx),
        registrations: Mutex::new(0),
    });
    let audit = Arc::new(RecordingAudit::default());
    let gateway = Arc::new(Gateway::bootstrap(
        host.clone(),
        login_shell("/usr/bin"),
        testing::discard_startup_events(),
        testing::sequential_reconciliation_ids(),
        audit.clone(),
        "/runtime".into(),
        "ci".into(),
    ));
    gateway.owner_launch_gate.arm();

    let cancelled = {
        let gateway = gateway.clone();
        tauri::async_runtime::spawn(async move { gateway.wait_ready(BundledSurface::Main).await })
    };
    assert!(
        gateway.owner_launch_gate.wait_until_arrived(),
        "receipt was not installed before owner launch"
    );
    cancelled.abort();
    gateway.owner_launch_gate.release();
    assert!(tauri::async_runtime::block_on(cancelled).is_err());
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();

    let joined = {
        let gateway = gateway.clone();
        tauri::async_runtime::spawn(async move { gateway.wait_ready(BundledSurface::Main).await })
    };
    if !audit.wait_for_joined(1) {
        joined.abort();
        let _ = release_tx.send(());
        panic!("follower did not join the installed receipt before its owner settled");
    }
    release_tx.send(()).unwrap();
    tauri::async_runtime::block_on(joined).unwrap().unwrap();

    assert_eq!(*host.registrations.lock().unwrap(), 1);
    assert_eq!(
        gateway.startup().unwrap().phase(),
        &GatewayStartupPhase::Ready
    );
}

struct FailSecondAudit {
    intent_calls: AtomicUsize,
    outcome_calls: AtomicUsize,
    fail_intent: bool,
}

impl TestAuditBehavior for FailSecondAudit {
    fn intent(&self, _: &GatewayReconciliationIntent) -> Result<(), GatewayError> {
        let call = self.intent_calls.fetch_add(1, Ordering::SeqCst) + 1;
        if self.fail_intent && matches!(call, 2 | 3) {
            Err(GatewayError::Registration(
                "second intent audit failed".into(),
            ))
        } else {
            Ok(())
        }
    }

    fn outcome(&self, _: &GatewayReconciliationOutcome) -> Result<(), GatewayError> {
        let call = self.outcome_calls.fetch_add(1, Ordering::SeqCst) + 1;
        if !self.fail_intent && matches!(call, 2 | 3) {
            Err(GatewayError::Registration(
                "second outcome audit failed".into(),
            ))
        } else {
            Ok(())
        }
    }

    fn joined(
        &self,
        _: &GatewayReconciliationAttempt,
        _: &GatewayReconciliationRequest,
    ) -> Result<(), GatewayError> {
        Ok(())
    }
}

struct RejectSecondClearPriorOutcome {
    outcome_calls: AtomicUsize,
}

impl TestAuditBehavior for RejectSecondClearPriorOutcome {
    fn intent(&self, _: &GatewayReconciliationIntent) -> Result<(), GatewayError> {
        Ok(())
    }

    fn outcome(&self, _: &GatewayReconciliationOutcome) -> Result<(), GatewayError> {
        let call = self.outcome_calls.fetch_add(1, Ordering::SeqCst) + 1;
        if call == 2 {
            Err(GatewayError::Registration(
                "terminal facts contradicted the latest observation".into(),
            ))
        } else {
            Ok(())
        }
    }

    fn outcome_rejected(&self, outcome: &GatewayReconciliationOutcome) -> bool {
        outcome.cleanup() == ReconciliationCleanupDecision::ClearPrior
    }

    fn joined(
        &self,
        _: &GatewayReconciliationAttempt,
        _: &GatewayReconciliationRequest,
    ) -> Result<(), GatewayError> {
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum SecondReconciliation {
    IntentOnlyFailure,
    OldServiceUnloaded,
    ConfirmedReplacement,
}

struct RetainedIdentityHost {
    calls: Mutex<usize>,
    second: SecondReconciliation,
    stopped: Mutex<Vec<String>>,
}

impl GatewayHost for RetainedIdentityHost {
    fn register(
        &self,
        _: &Path,
        _: &str,
        _: Option<&SearchPath>,
        attempt: &GatewayReconciliationAttempt,
        progress: &dyn GatewayReconciliationProgress,
    ) -> Result<ReconciledGateway, GatewayError> {
        let call = {
            let mut calls = self.calls.lock().unwrap();
            *calls += 1;
            *calls
        };
        if call == 1 {
            let gateway = reconciled("old");
            admit(attempt, progress, gateway.service());
            return Ok(gateway);
        }
        match self.second {
            SecondReconciliation::IntentOnlyFailure => {
                let target =
                    ReconciliationTarget::new("old".into(), "a".repeat(64), "b".repeat(64))
                        .unwrap();
                progress.intent_admitted(
                    GatewayReconciliationIntent::new(attempt.clone(), target, None).unwrap(),
                )?;
                Err(GatewayError::Registration(
                    "unreachable after intent failure".into(),
                ))
            }
            SecondReconciliation::OldServiceUnloaded => {
                admit(attempt, progress, "replacement");
                progress.history_observed(ReconciliationHistoryFact::RetirementAcknowledged);
                progress.history_observed(ReconciliationHistoryFact::OldServiceUnloaded);
                Err(GatewayError::Registration(
                    "bootstrap failed after unload".into(),
                ))
            }
            SecondReconciliation::ConfirmedReplacement => {
                let gateway = reconciled("replacement");
                admit(attempt, progress, gateway.service());
                Ok(gateway)
            }
        }
    }

    fn stop_agents(
        &self,
        session: &GatewayStopSession,
        journal: &dyn GatewayReconciliationJournalSession,
        plan: &AuditDeliveryReceipt,
    ) -> Result<LifecycleObservation, GatewayError> {
        let gateway = session.request().intended();
        complete_stop(session, journal, plan, || {
            self.stopped.lock().unwrap().push(gateway.service().into());
            Ok(())
        })
    }
}

fn retained_identity_gateway(
    second: SecondReconciliation,
    audit: Arc<dyn GatewayReconciliationAudit>,
) -> (Gateway, Arc<RetainedIdentityHost>) {
    let host = Arc::new(RetainedIdentityHost {
        calls: Mutex::new(0),
        second,
        stopped: Mutex::new(Vec::new()),
    });
    (
        Gateway::bootstrap(
            host.clone(),
            login_shell("/usr/bin"),
            testing::discard_startup_events(),
            testing::sequential_reconciliation_ids(),
            audit,
            "/runtime".into(),
            "ci".into(),
        ),
        host,
    )
}

#[test]
fn retained_cleanup_identity_tracks_native_and_audit_facts_separately() {
    let audit = Arc::new(FailSecondAudit {
        intent_calls: AtomicUsize::new(0),
        outcome_calls: AtomicUsize::new(0),
        fail_intent: true,
    });
    let (gateway, host) = retained_identity_gateway(SecondReconciliation::IntentOnlyFailure, audit);
    tauri::async_runtime::block_on(gateway.start()).unwrap();
    assert!(tauri::async_runtime::block_on(gateway.wait_ready(BundledSurface::Main)).is_err());
    gateway
        .stop_agents(Instant::now() + Duration::from_secs(30))
        .unwrap();
    assert_eq!(*host.stopped.lock().unwrap(), ["old"]);

    let (gateway, _) = retained_identity_gateway(
        SecondReconciliation::OldServiceUnloaded,
        Arc::new(RecordingAudit::default()),
    );
    tauri::async_runtime::block_on(gateway.start()).unwrap();
    assert!(tauri::async_runtime::block_on(gateway.wait_ready(BundledSurface::Main)).is_err());
    assert_eq!(
        gateway.stop_agents(Instant::now() + Duration::from_secs(30)),
        Err(GatewayError::NotReconciled)
    );

    let audit = Arc::new(FailSecondAudit {
        intent_calls: AtomicUsize::new(0),
        outcome_calls: AtomicUsize::new(0),
        fail_intent: false,
    });
    let (gateway, _) =
        retained_identity_gateway(SecondReconciliation::OldServiceUnloaded, audit.clone());
    tauri::async_runtime::block_on(gateway.start()).unwrap();
    assert!(matches!(
        tauri::async_runtime::block_on(gateway.wait_ready(BundledSurface::Main)),
        Err(GatewayError::Audit { .. })
    ));
    assert_eq!(audit.outcome_calls.load(Ordering::SeqCst), 3);
    assert_eq!(
        gateway.stop_agents(Instant::now() + Duration::from_secs(30)),
        Err(GatewayError::NotReconciled)
    );

    let audit = Arc::new(RejectSecondClearPriorOutcome {
        outcome_calls: AtomicUsize::new(0),
    });
    let (gateway, host) =
        retained_identity_gateway(SecondReconciliation::OldServiceUnloaded, audit.clone());
    tauri::async_runtime::block_on(gateway.start()).unwrap();
    assert!(matches!(
        tauri::async_runtime::block_on(gateway.wait_ready(BundledSurface::Main)),
        Err(GatewayError::Audit { .. })
    ));
    assert_eq!(audit.outcome_calls.load(Ordering::SeqCst), 2);
    gateway
        .stop_agents(Instant::now() + Duration::from_secs(30))
        .unwrap();
    assert_eq!(*host.stopped.lock().unwrap(), ["old"]);

    let audit = Arc::new(FailSecondAudit {
        intent_calls: AtomicUsize::new(0),
        outcome_calls: AtomicUsize::new(0),
        fail_intent: false,
    });
    let (gateway, host) =
        retained_identity_gateway(SecondReconciliation::ConfirmedReplacement, audit);
    tauri::async_runtime::block_on(gateway.start()).unwrap();
    assert!(matches!(
        tauri::async_runtime::block_on(gateway.wait_ready(BundledSurface::Main)),
        Err(GatewayError::Audit {
            physical: Some(GatewayPhysicalResult::Succeeded),
            ..
        })
    ));
    gateway
        .stop_agents(Instant::now() + Duration::from_secs(30))
        .unwrap();
    assert_eq!(*host.stopped.lock().unwrap(), ["replacement"]);
}

#[test]
fn partial_outcomes_preserve_each_confirmed_native_boundary() {
    struct BoundaryHost(Vec<ReconciliationHistoryFact>);
    impl GatewayHost for BoundaryHost {
        fn register(
            &self,
            _: &Path,
            _: &str,
            _: Option<&SearchPath>,
            attempt: &GatewayReconciliationAttempt,
            progress: &dyn GatewayReconciliationProgress,
        ) -> Result<ReconciledGateway, GatewayError> {
            if self.0.first() == Some(&ReconciliationHistoryFact::RetirementAcknowledged) {
                admit(attempt, progress, "target");
            } else {
                admit_fresh(attempt, progress, "target");
            }
            for fact in &self.0 {
                progress.history_observed(*fact);
            }
            Err(GatewayError::Registration("boundary failure".into()))
        }
        fn stop_agents(
            &self,
            session: &GatewayStopSession,
            journal: &dyn GatewayReconciliationJournalSession,
            plan: &AuditDeliveryReceipt,
        ) -> Result<LifecycleObservation, GatewayError> {
            complete_stop(session, journal, plan, || Ok(()))
        }
    }

    let cases = [
        vec![ReconciliationHistoryFact::RetirementAcknowledged],
        vec![ReconciliationHistoryFact::OldServiceUnloaded],
        vec![ReconciliationHistoryFact::ServiceDefinitionPublished],
        vec![
            ReconciliationHistoryFact::ServiceDefinitionPublished,
            ReconciliationHistoryFact::ServiceDefinitionDurable,
            ReconciliationHistoryFact::BootstrapCommandRequested,
        ],
        vec![
            ReconciliationHistoryFact::ServiceDefinitionPublished,
            ReconciliationHistoryFact::ServiceDefinitionDurable,
            ReconciliationHistoryFact::BootstrapCommandRequested,
            ReconciliationHistoryFact::BootstrapCommandCompleted,
        ],
        vec![
            ReconciliationHistoryFact::ServiceDefinitionPublished,
            ReconciliationHistoryFact::ServiceDefinitionDurable,
            ReconciliationHistoryFact::BootstrapCommandRequested,
            ReconciliationHistoryFact::BootstrapCommandCompleted,
            ReconciliationHistoryFact::BootstrapCommandSucceeded,
        ],
    ];
    for expected in cases {
        let audit = Arc::new(RecordingAudit::default());
        let gateway = Gateway::bootstrap(
            Arc::new(BoundaryHost(expected.clone())),
            login_shell("/usr/bin"),
            testing::discard_startup_events(),
            testing::sequential_reconciliation_ids(),
            audit.clone(),
            "/runtime".into(),
            "ci".into(),
        );
        assert!(tauri::async_runtime::block_on(gateway.start()).is_err());
        let outcomes = audit.outcomes.lock().unwrap();
        assert!(matches!(
            outcomes[0].effect(),
            GatewayReconciliationEffect::Failed { history, .. }
                if history.facts() == expected
        ));
    }
}

#[test]
fn malformed_success_is_audited_as_rejected_and_never_projects_ready() {
    struct MalformedSuccessHost(Mutex<Vec<String>>);
    impl GatewayHost for MalformedSuccessHost {
        fn register(
            &self,
            _: &Path,
            _: &str,
            _: Option<&SearchPath>,
            attempt: &GatewayReconciliationAttempt,
            progress: &dyn GatewayReconciliationProgress,
        ) -> Result<ReconciledGateway, GatewayError> {
            let gateway = reconciled("malformed");
            admit(attempt, progress, gateway.service());
            progress.history_observed(ReconciliationHistoryFact::BootstrapCommandSucceeded);
            Ok(gateway)
        }
        fn stop_agents(
            &self,
            session: &GatewayStopSession,
            journal: &dyn GatewayReconciliationJournalSession,
            plan: &AuditDeliveryReceipt,
        ) -> Result<LifecycleObservation, GatewayError> {
            let gateway = session.request().intended();
            complete_stop(session, journal, plan, || {
                self.0.lock().unwrap().push(gateway.service().into());
                Ok(())
            })
        }
    }
    let audit = Arc::new(RecordingAudit {
        fail_outcome: true,
        ..RecordingAudit::default()
    });
    let host = Arc::new(MalformedSuccessHost(Mutex::new(Vec::new())));
    let gateway = Gateway::bootstrap(
        host.clone(),
        login_shell("/usr/bin"),
        testing::discard_startup_events(),
        testing::sequential_reconciliation_ids(),
        audit.clone(),
        "/runtime".into(),
        "ci".into(),
    );

    assert!(matches!(
        tauri::async_runtime::block_on(gateway.start()),
        Err(GatewayError::Audit { .. })
    ));
    assert!(matches!(
        gateway.startup().unwrap().phase(),
        GatewayStartupPhase::Failed(_)
    ));
    let outcomes = audit.outcomes.lock().unwrap();
    let GatewayReconciliationEffect::RejectedReport {
        trusted_history,
        report,
        ..
    } = outcomes[0].effect()
    else {
        panic!("malformed success was accepted")
    };
    assert!(trusted_history.facts().is_empty());
    assert_eq!(
        report.validation().rejected_history_fact(),
        Some(ReconciliationHistoryFact::BootstrapCommandSucceeded)
    );
    assert_eq!(report.cleanup(), ReconciliationCleanupDecision::RetainPrior);
    drop(outcomes);
    assert_eq!(
        gateway.stop_agents(Instant::now() + Duration::from_secs(30)),
        Err(GatewayError::NotReconciled)
    );
    assert!(host.0.lock().unwrap().is_empty());
}

#[test]
fn invalid_history_cannot_hide_an_ineligible_claimed_identity() {
    struct ContradictoryHost;

    impl GatewayHost for ContradictoryHost {
        fn register(
            &self,
            _: &Path,
            _: &str,
            _: Option<&SearchPath>,
            attempt: &GatewayReconciliationAttempt,
            progress: &dyn GatewayReconciliationProgress,
        ) -> Result<ReconciledGateway, GatewayError> {
            let admitted = reconciled("admitted");
            let intent = GatewayReconciliationIntent::new(
                attempt.clone(),
                admitted.audit_identity().unwrap().target().clone(),
                None,
            )
            .unwrap();
            progress.intent_admitted(intent)?;
            progress.history_observed(ReconciliationHistoryFact::BootstrapCommandSucceeded);
            Ok(reconciled("wrong-target"))
        }

        fn stop_agents(
            &self,
            session: &GatewayStopSession,
            journal: &dyn GatewayReconciliationJournalSession,
            plan: &AuditDeliveryReceipt,
        ) -> Result<LifecycleObservation, GatewayError> {
            complete_stop(session, journal, plan, || Ok(()))
        }
    }

    let audit = Arc::new(RecordingAudit::default());
    let gateway = Gateway::bootstrap(
        Arc::new(ContradictoryHost),
        login_shell("/usr/bin"),
        testing::discard_startup_events(),
        testing::sequential_reconciliation_ids(),
        audit.clone(),
        "/runtime".into(),
        "ci".into(),
    );

    assert!(tauri::async_runtime::block_on(gateway.start()).is_err());
    let outcomes = audit.outcomes.lock().unwrap();
    let GatewayReconciliationEffect::RejectedReport { report, .. } = outcomes[0].effect() else {
        panic!("contradictory report was accepted")
    };
    assert_eq!(
        report.validation().rejected_history_fact(),
        Some(ReconciliationHistoryFact::BootstrapCommandSucceeded)
    );
    assert!(!report.validation().target_matches());
    assert!(!report.validation().candidate_eligible());
    assert_eq!(report.cleanup(), ReconciliationCleanupDecision::RetainPrior);
    drop(outcomes);
    assert_eq!(
        gateway.stop_agents(Instant::now() + Duration::from_secs(30)),
        Err(GatewayError::NotReconciled)
    );
}
