use super::*;
use crate::gateway::{
    application::{
        testing::{self, FixedLoginShell},
        GatewayStartup, GatewayStartupEvents, GatewayStartupPhase, LoginShellError,
    },
    domain::value_objects::{
        ReconciliationCause, ReconciliationCorrelation, ReconciliationEvidence,
        ReconciliationIncarnation, ReconciliationInitiator, ReconciliationTarget, SearchPathError,
    },
};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, Condvar, Mutex,
    },
    thread,
    time::Duration,
};

fn login_shell(path: &str) -> Arc<FixedLoginShell> {
    Arc::new(FixedLoginShell(Ok(SearchPath::parse(path).unwrap())))
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
    fn stop_agents(&self, gateway: &ReconciledGateway) -> Result<(), GatewayError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("stop:{}", gateway.service()));
        self.stop_result.clone()
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
            gateway.stop_agents(),
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
    assert_eq!(gateway.stop_agents(), Err(GatewayError::NotReconciled));
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
        fn stop_agents(&self, gateway: &ReconciledGateway) -> Result<(), GatewayError> {
            self.calls
                .lock()
                .unwrap()
                .push(gateway.service().to_owned());
            Ok(())
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
    gateway.stop_agents().unwrap();
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

#[derive(Default)]
struct RecordingAudit {
    intents: Mutex<Vec<GatewayReconciliationIntent>>,
    outcomes: Mutex<Vec<GatewayReconciliationOutcome>>,
    joined: Mutex<Vec<(GatewayReconciliationAttempt, GatewayReconciliationRequest)>>,
    joined_changed: Condvar,
    fail_intent: bool,
    fail_outcome: bool,
    fail_joined: bool,
    panic_outcome: bool,
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

    let outcome = GatewayReconciliationOutcome::assess(intent, Vec::new(), Ok(after));
    assert!(matches!(
        outcome.effect(),
        GatewayReconciliationEffect::RejectedReport {
            report: ReconciliationRejectedReport::ConfirmedTargetMismatch(_),
            ..
        }
    ));
}

impl GatewayReconciliationAudit for RecordingAudit {
    fn intent(&self, intent: &GatewayReconciliationIntent) -> Result<(), GatewayError> {
        self.intents.lock().unwrap().push(intent.clone());
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
        admit(attempt, progress, service);
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

    fn stop_agents(&self, gateway: &ReconciledGateway) -> Result<(), GatewayError> {
        self.stopped.lock().unwrap().push(gateway.service().into());
        Ok(())
    }
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
                    assert!(physical.is_none());
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
            assert_eq!(outcomes.len(), 1);
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
                gateway.stop_agents().unwrap();
                assert_eq!(*host.stopped.lock().unwrap(), ["confirmed"]);
            } else {
                assert_eq!(gateway.stop_agents(), Err(GatewayError::NotReconciled));
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

        fn stop_agents(&self, gateway: &ReconciledGateway) -> Result<(), GatewayError> {
            self.stopped.lock().unwrap().push(gateway.service().into());
            Ok(())
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
    gateway.stop_agents().unwrap();
    assert_eq!(*host.stopped.lock().unwrap(), ["active-owner"]);
    assert_eq!(audit.intents.lock().unwrap().len(), 1);
    assert_eq!(audit.outcomes.lock().unwrap().len(), 1);
    assert!(audit.joined.lock().unwrap().is_empty());
}

#[test]
fn failed_intent_audit_prevents_admission_and_has_no_outcome() {
    let audit = Arc::new(RecordingAudit {
        fail_intent: true,
        ..RecordingAudit::default()
    });
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

    assert!(matches!(
        tauri::async_runtime::block_on(gateway.start()),
        Err(GatewayError::Audit { physical: None, .. })
    ));
    assert_eq!(audit.intents.lock().unwrap().len(), 1);
    assert!(audit.outcomes.lock().unwrap().is_empty());
    assert_eq!(gateway.stop_agents(), Err(GatewayError::NotReconciled));
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
        Err(GatewayError::Audit { physical: None, .. })
    ));
    gateway.stop_agents().unwrap();
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

        fn stop_agents(&self, _: &ReconciledGateway) -> Result<(), GatewayError> {
            Ok(())
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

impl GatewayReconciliationAudit for DelayedJoinedAudit {
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

    fn stop_agents(&self, _: &ReconciledGateway) -> Result<(), GatewayError> {
        Ok(())
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

        fn stop_agents(&self, _: &ReconciledGateway) -> Result<(), GatewayError> {
            Ok(())
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
    let first = gateway.lifecycle.lock().unwrap().pending.clone().unwrap();
    assert!(
        first.wait_for_waiter(),
        "configuration change did not wait for A"
    );

    let credential = {
        let gateway = gateway.clone();
        thread::spawn(move || {
            tauri::async_runtime::block_on(gateway.wait_ready(BundledSurface::Setup))
        })
    };
    assert!(
        audit.wait_for_joined(1),
        "credential did not join the unstarted successor"
    );

    a_release_tx.send(()).unwrap();
    a_result_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    a.join().unwrap();
    successor_entered_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap();
    assert!(matches!(
        successor_result_rx.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));

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
    let later_receipt = gateway.lifecycle.lock().unwrap().pending.clone().unwrap();
    assert!(later_receipt.wait_for_waiter());
    assert_eq!(host.attempts.lock().unwrap().len(), 2);

    successor_release_tx.send(()).unwrap();
    successor_result_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    successor.join().unwrap();
    credential.join().unwrap().unwrap();
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
struct RecordingEvents(Mutex<Vec<GatewayStartup>>);

impl GatewayStartupEvents for RecordingEvents {
    fn publish(&self, startup: &GatewayStartup) {
        self.0.lock().unwrap().push(startup.clone());
    }
}

#[test]
fn a_later_credential_load_retries_failed_startup_with_monotonic_events() {
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

        fn stop_agents(&self, _: &ReconciledGateway) -> Result<(), GatewayError> {
            Ok(())
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

    let revisions = events
        .0
        .lock()
        .unwrap()
        .iter()
        .map(GatewayStartup::revision)
        .collect::<Vec<_>>();
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

        fn stop_agents(&self, gateway: &ReconciledGateway) -> Result<(), GatewayError> {
            self.stopped
                .lock()
                .unwrap()
                .push(gateway.service().to_owned());
            Ok(())
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
    gateway.stop_agents().unwrap();
    assert_eq!(*host.stopped.lock().unwrap(), ["replacement"]);
    let observations = events.0.lock().unwrap().clone();
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

        fn stop_agents(&self, _: &ReconciledGateway) -> Result<(), GatewayError> {
            Ok(())
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

    let observations = events.0.lock().unwrap().clone();
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

        fn stop_agents(&self, _: &ReconciledGateway) -> Result<(), GatewayError> {
            Ok(())
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
        fn stop_agents(&self, _: &ReconciledGateway) -> Result<(), GatewayError> {
            Ok(())
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
    let pending = gateway.lifecycle.lock().unwrap().pending.clone().unwrap();
    assert!(pending.wait_for_waiter());
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

        fn stop_agents(&self, _: &ReconciledGateway) -> Result<(), GatewayError> {
            Ok(())
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
    assert!(attempt.wait_for_waiter(), "follower did not join in time");
    release_tx.send(()).unwrap();

    first.join().unwrap().unwrap();
    second.join().unwrap().unwrap();
    assert_eq!(*host.registrations.lock().unwrap(), 1);
}

#[test]
fn cancelled_caller_does_not_cancel_or_duplicate_shared_reconciliation() {
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

        fn stop_agents(&self, _: &ReconciledGateway) -> Result<(), GatewayError> {
            Ok(())
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

    let cancelled = {
        let gateway = gateway.clone();
        tauri::async_runtime::spawn(async move { gateway.wait_ready(BundledSurface::Main).await })
    };
    entered_rx.recv().unwrap();
    cancelled.abort();
    assert!(tauri::async_runtime::block_on(cancelled).is_err());

    let joined = {
        let gateway = gateway.clone();
        tauri::async_runtime::spawn(async move { gateway.wait_ready(BundledSurface::Main).await })
    };
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

impl GatewayReconciliationAudit for FailSecondAudit {
    fn intent(&self, _: &GatewayReconciliationIntent) -> Result<(), GatewayError> {
        let call = self.intent_calls.fetch_add(1, Ordering::SeqCst) + 1;
        if self.fail_intent && call == 2 {
            Err(GatewayError::Registration(
                "second intent audit failed".into(),
            ))
        } else {
            Ok(())
        }
    }

    fn outcome(&self, _: &GatewayReconciliationOutcome) -> Result<(), GatewayError> {
        let call = self.outcome_calls.fetch_add(1, Ordering::SeqCst) + 1;
        if !self.fail_intent && call == 2 {
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

    fn stop_agents(&self, gateway: &ReconciledGateway) -> Result<(), GatewayError> {
        self.stopped.lock().unwrap().push(gateway.service().into());
        Ok(())
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
    gateway.stop_agents().unwrap();
    assert_eq!(*host.stopped.lock().unwrap(), ["old"]);

    let (gateway, _) = retained_identity_gateway(
        SecondReconciliation::OldServiceUnloaded,
        Arc::new(RecordingAudit::default()),
    );
    tauri::async_runtime::block_on(gateway.start()).unwrap();
    assert!(tauri::async_runtime::block_on(gateway.wait_ready(BundledSurface::Main)).is_err());
    assert_eq!(gateway.stop_agents(), Err(GatewayError::NotReconciled));

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
        Err(GatewayError::Audit { physical: None, .. })
    ));
    gateway.stop_agents().unwrap();
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
            admit(attempt, progress, "target");
            for fact in &self.0 {
                progress.history_observed(*fact);
            }
            Err(GatewayError::Registration("boundary failure".into()))
        }
        fn stop_agents(&self, _: &ReconciledGateway) -> Result<(), GatewayError> {
            Ok(())
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
        fn stop_agents(&self, gateway: &ReconciledGateway) -> Result<(), GatewayError> {
            self.0.lock().unwrap().push(gateway.service().into());
            Ok(())
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
    assert!(matches!(
        audit.outcomes.lock().unwrap()[0].effect(),
        GatewayReconciliationEffect::RejectedReport {
            trusted_history,
            report: ReconciliationRejectedReport::InvalidHistory(
                ReconciliationHistoryFact::BootstrapCommandSucceeded
            ),
            ..
        } if trusted_history.facts().is_empty()
    ));
    gateway.stop_agents().unwrap();
    assert_eq!(*host.0.lock().unwrap(), ["malformed"]);
}
