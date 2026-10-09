//! Owned publication: durable preferences, next command and installed command settle together.
use super::*;
use std::pin::Pin;

struct SettlingDirectoryHost {
    inner: Arc<GatedDirectoryHost>,
    installed: Mutex<Option<PathBuf>>,
    panic_publication: bool,
    panic_registration: bool,
    panic_restore: bool,
    fail_after_install: bool,
}

impl GatewayHost for SettlingDirectoryHost {
    fn replace_claude_config_directory(
        &self,
        directory: Option<PathBuf>,
    ) -> Result<ClaudeDirectoryReplacement, GatewayError> {
        assert!(!self.panic_publication, "configuration publication panic");
        self.inner.replace_claude_config_directory(directory)
    }
    fn restore_claude_config_directory(
        &self,
        expected: &Option<PathBuf>,
        previous: Option<PathBuf>,
    ) -> Result<bool, GatewayError> {
        assert!(!self.panic_restore, "configuration rollback panic");
        self.inner
            .restore_claude_config_directory(expected, previous)
    }
    fn register(
        &self,
        _: &Path,
        _: &str,
        _: Option<&SearchPath>,
        attempt: &GatewayReconciliationAttempt,
        progress: &dyn GatewayReconciliationProgress,
    ) -> Result<ReconciledGateway, GatewayError> {
        // Native adapters capture configuration at entry and use it after manager IO.
        let captured = self.inner.directory.lock().unwrap().clone();
        admit(attempt, progress, "claude-directory");
        let first = self.inner.attempts.fetch_add(1, Ordering::SeqCst) == 0;
        if first {
            *self.inner.entered.lock().unwrap() = true;
            self.inner.entered_changed.notify_all();
            let mut release = self.inner.release.lock().unwrap();
            while !*release {
                release = self.inner.release_changed.wait(release).unwrap();
            }
            assert!(!self.panic_registration, "native registration panic");
            if self.inner.fail_first {
                return Err(GatewayError::Registration("command retained".into()));
            }
        }
        *self.installed.lock().unwrap() = captured;
        if first && self.fail_after_install {
            return Err(GatewayError::Registration(
                "native effect was not confirmed".into(),
            ));
        }
        Ok(reconciled("claude-directory"))
    }
    fn stop_agents(
        &self,
        session: &GatewayStopSession,
        journal: &dyn GatewayReconciliationJournalSession,
        plan: &AuditDeliveryReceipt,
    ) -> Result<LifecycleObservation, GatewayError> {
        self.inner.stop_agents(session, journal, plan)
    }
}

fn publication_gateway(
    host: Arc<SettlingDirectoryHost>,
    audit: Arc<RecordingAudit>,
) -> Arc<Gateway> {
    Arc::new(Gateway::bootstrap(
        host,
        login_shell("/usr/bin"),
        testing::discard_startup_events(),
        testing::sequential_reconciliation_ids(),
        audit,
        "/runtime".into(),
        "ci".into(),
    ))
}

fn settling_host(fail: bool) -> Arc<SettlingDirectoryHost> {
    Arc::new(SettlingDirectoryHost {
        inner: GatedDirectoryHost::new(fail),
        installed: Mutex::new(None),
        panic_publication: false,
        panic_registration: false,
        panic_restore: false,
        fail_after_install: false,
    })
}

fn poll_pending(future: Pin<&mut impl Future>) {
    assert!(matches!(
        future.poll(&mut Context::from_waker(Waker::noop())),
        Poll::Pending
    ));
}

fn assert_configuration_agrees(
    host: &SettlingDirectoryHost,
    settings: &DirectorySettings,
    expected: Option<PathBuf>,
) {
    assert_eq!(
        *host.installed.lock().unwrap(),
        expected,
        "installed command"
    );
    assert_eq!(
        *host.inner.directory.lock().unwrap(),
        expected,
        "next native command"
    );
    assert_eq!(*settings.0.lock().unwrap(), expected, "durable preference");
}

#[test]
fn an_unpolled_directory_request_has_no_effects() {
    let host = settling_host(false);
    let audit = Arc::new(RecordingAudit::default());
    let gateway = publication_gateway(host.clone(), audit.clone());
    let settings = Arc::new(DirectorySettings::default());
    drop(gateway.change_claude_configuration(
        BundledSurface::Main,
        Some(absolute_claude_directory(".claude-unpolled")),
        settings.clone(),
    ));
    assert_configuration_agrees(&host, &settings, None);
    assert_eq!(host.inner.attempts.load(Ordering::SeqCst), 0);
    assert!(audit.intents.lock().unwrap().is_empty());
}

#[test]
fn a_dropped_directory_caller_retains_ownership_through_late_success_and_failure() {
    for fail in [false, true] {
        let host = settling_host(fail);
        let audit = Arc::new(RecordingAudit::default());
        let gateway = publication_gateway(host.clone(), audit.clone());
        let settings = Arc::new(DirectorySettings::default());
        let requested = Some(absolute_claude_directory(".claude-owned"));
        let mut caller = Box::pin(gateway.change_claude_configuration(
            BundledSurface::Main,
            requested.clone(),
            settings.clone(),
        ));
        poll_pending(caller.as_mut());
        host.inner.wait_until_registered();
        drop(caller);
        assert_eq!(*settings.0.lock().unwrap(), requested);
        assert_eq!(*host.inner.directory.lock().unwrap(), requested);
        assert_eq!(*host.installed.lock().unwrap(), None);
        // Another caller joins this exact publication even after the first disappears.
        let mut joined = Box::pin(gateway.change_claude_configuration(
            BundledSurface::Setup,
            requested.clone(),
            settings.clone(),
        ));
        poll_pending(joined.as_mut());
        assert_eq!(host.inner.attempts.load(Ordering::SeqCst), 1);
        host.inner.release();
        let outcome = tauri::async_runtime::block_on(joined);
        assert_eq!(outcome.is_err(), fail);
        let settled = if fail { None } else { requested.clone() };
        assert_configuration_agrees(&host, &settings, settled.clone());
        assert_eq!(host.inner.attempts.load(Ordering::SeqCst), 1);
        let intents = audit.intents.lock().unwrap();
        assert_eq!(intents.len(), 1);
        assert_eq!(
            intents[0].attempt().origin().evidence().cause(),
            ReconciliationCause::ClaudeConfigurationChanged
        );
        assert_eq!(
            intents[0].attempt().origin().evidence().initiator(),
            ReconciliationInitiator::BundledSurface(BundledSurface::Main)
        );
        let outcomes = audit.outcomes.lock().unwrap();
        assert_eq!(outcomes.len(), 1);
        assert_eq!(
            matches!(
                outcomes[0].effect(),
                GatewayReconciliationEffect::Confirmed { .. }
            ),
            !fail
        );
        drop(outcomes);
        drop(intents);
        // Equal retry has no native duplicate; different retry must update the installed command.
        let equal = tauri::async_runtime::block_on(gateway.change_claude_configuration(
            BundledSurface::Main,
            settled,
            settings.clone(),
        ));
        assert_eq!(
            equal.is_err(),
            fail,
            "equal save must preserve failed startup"
        );
        assert_eq!(host.inner.attempts.load(Ordering::SeqCst), 1);
        let different = if fail { requested } else { None };
        tauri::async_runtime::block_on(gateway.change_claude_configuration(
            BundledSurface::Setup,
            different.clone(),
            settings.clone(),
        ))
        .unwrap();
        assert_configuration_agrees(&host, &settings, different);
        assert_eq!(host.inner.attempts.load(Ordering::SeqCst), 2);
        assert_eq!(audit.outcomes.lock().unwrap().len(), 2);
    }
}

#[test]
fn a_different_directory_waits_after_its_predecessor_caller_disappears() {
    let host = settling_host(false);
    let audit = Arc::new(RecordingAudit::default());
    let gateway = publication_gateway(host.clone(), audit.clone());
    let settings = Arc::new(DirectorySettings::default());
    let desired = Some(absolute_claude_directory(".claude-first"));
    let mut first = Box::pin(gateway.change_claude_configuration(
        BundledSurface::Main,
        desired.clone(),
        settings.clone(),
    ));
    poll_pending(first.as_mut());
    host.inner.wait_until_registered();
    drop(first);
    let later = Some(absolute_claude_directory(".claude-later"));
    let mut next = Box::pin(gateway.change_claude_configuration(
        BundledSurface::Setup,
        later.clone(),
        settings.clone(),
    ));
    poll_pending(next.as_mut());
    assert_eq!(*settings.0.lock().unwrap(), desired);
    assert_eq!(host.inner.attempts.load(Ordering::SeqCst), 1);
    host.inner.release();
    tauri::async_runtime::block_on(next).unwrap();
    assert_configuration_agrees(&host, &settings, later);
    assert_eq!(host.inner.attempts.load(Ordering::SeqCst), 2);
    assert_eq!(audit.outcomes.lock().unwrap().len(), 2);
}

#[test]
fn late_physical_success_retains_configuration_when_outcome_delivery_fails_or_panics() {
    for panic_outcome in [false, true] {
        let host = settling_host(false);
        let audit = Arc::new(RecordingAudit {
            fail_outcome: !panic_outcome,
            panic_outcome,
            ..Default::default()
        });
        let gateway = publication_gateway(host.clone(), audit);
        let settings = Arc::new(DirectorySettings::default());
        let directory = Some(absolute_claude_directory(".claude-receipt"));
        let mut caller = Box::pin(gateway.change_claude_configuration(
            BundledSurface::Main,
            directory.clone(),
            settings.clone(),
        ));
        poll_pending(caller.as_mut());
        host.inner.wait_until_registered();
        drop(caller);
        let mut joined = Box::pin(gateway.change_claude_configuration(
            BundledSurface::Setup,
            directory.clone(),
            settings.clone(),
        ));
        poll_pending(joined.as_mut());
        host.inner.release();
        assert!(matches!(
            tauri::async_runtime::block_on(joined),
            Err(ClaudeConfigurationChangeError::Gateway(
                GatewayError::Audit {
                    physical: Some(GatewayPhysicalResult::Succeeded),
                    ..
                }
            ))
        ));
        assert_configuration_agrees(&host, &settings, directory.clone());
        assert!(matches!(
            tauri::async_runtime::block_on(gateway.change_claude_configuration(
                BundledSurface::Main,
                directory.clone(),
                settings.clone(),
            )),
            Err(ClaudeConfigurationChangeError::Gateway(
                GatewayError::Audit {
                    physical: Some(GatewayPhysicalResult::Succeeded),
                    ..
                }
            ))
        ));
        assert!(matches!(
            gateway.startup().unwrap().phase(),
            GatewayStartupPhase::Failed(_)
        ));
        assert_configuration_agrees(&host, &settings, directory);
        assert_eq!(host.inner.attempts.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn a_panicking_directory_waiter_cannot_end_the_publication_transaction() {
    let host = settling_host(false);
    let gateway = publication_gateway(host.clone(), Arc::new(RecordingAudit::default()));
    let settings = Arc::new(DirectorySettings::default());
    let directory = Some(absolute_claude_directory(".claude-waiter"));
    let caller_gateway = gateway.clone();
    let caller_settings = settings.clone();
    let caller_directory = directory.clone();
    let (panicked, observed) = mpsc::sync_channel(1);
    let waiter = thread::spawn(move || {
        let mut caller = Box::pin(caller_gateway.change_claude_configuration(
            BundledSurface::Main,
            caller_directory,
            caller_settings,
        ));
        poll_pending(caller.as_mut());
        host_wait_for_publication(&caller_gateway);
        let result = catch_unwind(AssertUnwindSafe(|| {
            let _caller = caller;
            panic!("caller receipt consumer panicked");
        }));
        assert!(result.is_err());
        panicked.send(()).unwrap();
    });
    host.inner.wait_until_registered();
    observed.recv_timeout(Duration::from_secs(2)).unwrap();
    waiter.join().unwrap();
    assert_eq!(*settings.0.lock().unwrap(), directory);
    let mut remaining = Box::pin(gateway.change_claude_configuration(
        BundledSurface::Setup,
        directory.clone(),
        settings.clone(),
    ));
    poll_pending(remaining.as_mut());
    host.inner.release();
    tauri::async_runtime::block_on(remaining).unwrap();
    assert_configuration_agrees(&host, &settings, directory);
    assert_eq!(host.inner.attempts.load(Ordering::SeqCst), 1);
}

fn host_wait_for_publication(gateway: &Gateway) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while gateway.claude_publication_waiters() == 0 {
        assert!(Instant::now() < deadline, "caller never waited");
        thread::yield_now();
    }
}

#[test]
fn publication_panics_settle_the_slot_and_preserve_independent_rollback_failures() {
    struct PanicSettings {
        inner: DirectorySettings,
        publish: bool,
        restore: bool,
    }
    impl ClaudeDirectorySettings for PanicSettings {
        fn publish(
            &self,
            directory: Option<PathBuf>,
        ) -> Result<Option<PathBuf>, ClaudeSettingsPublishError> {
            assert!(!self.publish, "settings publish panic");
            self.inner.publish(directory)
        }
        fn restore(
            &self,
            expected: &Option<PathBuf>,
            previous: Option<PathBuf>,
        ) -> Result<(), String> {
            assert!(!self.restore, "settings restore panic");
            self.inner.restore(expected, previous)
        }
    }
    for phase in 0..5 {
        let mut host = settling_host(true);
        let config = Arc::get_mut(&mut host).unwrap();
        config.panic_publication = phase == 1;
        config.panic_registration = phase == 2;
        config.panic_restore = phase == 4;
        host.inner.release();
        let settings = Arc::new(PanicSettings {
            inner: DirectorySettings::default(),
            publish: phase == 0,
            restore: phase == 3,
        });
        let gateway = publication_gateway(host.clone(), Arc::new(RecordingAudit::default()));
        let result = tauri::async_runtime::block_on(gateway.change_claude_configuration(
            BundledSurface::Main,
            Some(absolute_claude_directory(".claude-panic")),
            settings.clone(),
        ));
        if phase >= 3 {
            assert!(
                matches!(result, Err(ClaudeConfigurationChangeError::Rollback { failure, settings, gateway })
                if *failure == ClaudeConfigurationChangeError::Gateway(GatewayError::Registration("command retained".into()))
                && settings == (phase == 3).then(|| "claude settings rollback panicked".into())
                && gateway == (phase == 4).then(|| GatewayError::Registration("claude configuration rollback panicked".into())))
            );
        } else if phase == 0 {
            assert_eq!(
                result,
                Err(ClaudeConfigurationChangeError::Settings(
                    "claude settings publication panicked".into()
                ))
            );
        } else {
            let message = if phase == 1 {
                "claude configuration publication panicked"
            } else {
                "gateway host panicked during reconciliation"
            };
            assert_eq!(
                result,
                Err(ClaudeConfigurationChangeError::Gateway(
                    GatewayError::Registration(message.into())
                ))
            );
        }
        if phase != 3 {
            assert_eq!(*settings.inner.0.lock().unwrap(), None);
        }
        if phase != 4 {
            assert_eq!(*host.inner.directory.lock().unwrap(), None);
        }
        // The error is a settled receipt, so a later request can take the slot.
        let retry = tauri::async_runtime::block_on(gateway.change_claude_configuration(
            BundledSurface::Main,
            None,
            settings,
        ));
        if phase == 4 {
            retry.unwrap();
        } else {
            assert!(retry.is_err());
        }
    }
}

#[test]
fn receipt_admission_panic_restores_published_settings_before_a_fresh_retry() {
    struct PanicFirstId(AtomicUsize);
    impl GatewayReconciliationIds for PanicFirstId {
        fn next(&self) -> Result<ReconciliationCorrelation, GatewayError> {
            let call = self.0.fetch_add(1, Ordering::SeqCst);
            assert_ne!(call, 0, "receipt allocation panic");
            Ok(correlation(call as u64))
        }
    }
    let host = settling_host(false);
    host.inner.release();
    let audit = Arc::new(RecordingAudit::default());
    let gateway = Arc::new(Gateway::bootstrap(
        host.clone(),
        login_shell("/usr/bin"),
        testing::discard_startup_events(),
        Arc::new(PanicFirstId(AtomicUsize::new(0))),
        audit.clone(),
        "/runtime".into(),
        "ci".into(),
    ));
    let settings = Arc::new(DirectorySettings::default());
    let requested = Some(absolute_claude_directory(".claude-admission-panic"));
    let result = tauri::async_runtime::block_on(gateway.change_claude_configuration(
        BundledSurface::Main,
        requested.clone(),
        settings.clone(),
    ));
    assert!(matches!(
        result,
        Err(ClaudeConfigurationChangeError::Gateway(
            GatewayError::Registration(_)
        ))
    ));
    assert_configuration_agrees(&host, &settings, None);
    assert_eq!(host.inner.attempts.load(Ordering::SeqCst), 0);
    assert!(audit.intents.lock().unwrap().is_empty());
    tauri::async_runtime::block_on(gateway.change_claude_configuration(
        BundledSurface::Setup,
        requested.clone(),
        settings.clone(),
    ))
    .unwrap();
    assert_configuration_agrees(&host, &settings, requested);
    assert_eq!(host.inner.attempts.load(Ordering::SeqCst), 1);
    assert_eq!(audit.outcomes.lock().unwrap().len(), 1);
}

#[test]
fn an_equal_save_does_not_claim_recovery_of_an_unconfirmed_native_effect() {
    let mut host = settling_host(false);
    Arc::get_mut(&mut host).unwrap().fail_after_install = true;
    let audit = Arc::new(RecordingAudit::default());
    let gateway = publication_gateway(host.clone(), audit.clone());
    let settings = Arc::new(DirectorySettings::default());
    let requested = Some(absolute_claude_directory(".claude-unknown"));
    let mut caller = Box::pin(gateway.change_claude_configuration(
        BundledSurface::Main,
        requested.clone(),
        settings.clone(),
    ));
    poll_pending(caller.as_mut());
    host.inner.wait_until_registered();
    drop(caller);
    let mut joined = Box::pin(gateway.change_claude_configuration(
        BundledSurface::Setup,
        requested.clone(),
        settings.clone(),
    ));
    poll_pending(joined.as_mut());
    host.inner.release();
    let failure = tauri::async_runtime::block_on(joined).unwrap_err();
    assert_eq!(
        failure,
        ClaudeConfigurationChangeError::Gateway(GatewayError::Registration(
            "native effect was not confirmed".into()
        ))
    );
    // Local rollback is observable; it provides no proof that the daemon command was restored.
    assert_eq!(*settings.0.lock().unwrap(), None);
    assert_eq!(*host.inner.directory.lock().unwrap(), None);
    assert_eq!(*host.installed.lock().unwrap(), requested);
    assert!(matches!(
        gateway.startup().unwrap().phase(),
        GatewayStartupPhase::Failed(_)
    ));
    assert!(matches!(
        audit.outcomes.lock().unwrap()[0].effect(),
        GatewayReconciliationEffect::Failed { .. }
    ));
    assert_eq!(
        tauri::async_runtime::block_on(gateway.change_claude_configuration(
            BundledSurface::Main,
            None,
            settings.clone()
        )),
        Err(failure)
    );
    assert_eq!(host.inner.attempts.load(Ordering::SeqCst), 1);
    assert_eq!(*host.installed.lock().unwrap(), requested);
    // Explicit retry owns reconciliation/recovery; the save never claims it did this work.
    tauri::async_runtime::block_on(gateway.retry(BundledSurface::Setup)).unwrap();
    assert_configuration_agrees(&host, &settings, None);
    assert_eq!(host.inner.attempts.load(Ordering::SeqCst), 2);
    assert_eq!(
        audit.intents.lock().unwrap()[1]
            .attempt()
            .origin()
            .evidence()
            .cause(),
        ReconciliationCause::ExplicitRetry
    );
    assert!(matches!(
        audit.outcomes.lock().unwrap()[1].effect(),
        GatewayReconciliationEffect::Confirmed { .. }
    ));
}
