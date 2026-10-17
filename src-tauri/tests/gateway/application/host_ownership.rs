//! Competing desktop identities through the actual application entry points.
use super::*;

struct VirtualOwnershipClock(Mutex<Instant>);
impl MonotonicClock for VirtualOwnershipClock {
    fn now(&self) -> Instant {
        *self.0.lock().unwrap()
    }
    fn wait(&self, duration: Duration) {
        *self.0.lock().unwrap() += duration;
    }
}

struct PoolLease(Arc<AtomicBool>);
impl Drop for PoolLease {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}
struct OwnedHost {
    pool: Arc<AtomicBool>,
    claims: AtomicUsize,
    fail_registration: AtomicBool,
    stop_gate: Mutex<Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>>,
    inner: FakeHost,
}
impl OwnedHost {
    fn new(pool: Arc<AtomicBool>, name: &str) -> Arc<Self> {
        Arc::new(Self {
            pool,
            claims: AtomicUsize::new(0),
            fail_registration: AtomicBool::new(false),
            stop_gate: Mutex::new(None),
            inner: FakeHost {
                registration: Ok(reconciled(name)),
                stop_result: Ok(()),
                calls: Mutex::new(Vec::new()),
            },
        })
    }
}
impl GatewayHost for OwnedHost {
    fn claim_desktop_ownership(&self) -> Result<DesktopHostClaim, GatewayError> {
        self.claims.fetch_add(1, Ordering::SeqCst);
        Ok(
            if self
                .pool
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                DesktopHostClaim::Acquired(DesktopHostLease::new(PoolLease(self.pool.clone())))
            } else {
                DesktopHostClaim::Busy
            },
        )
    }
    fn replace_claude_config_directory(
        &self,
        _: Option<PathBuf>,
    ) -> Result<ClaudeDirectoryReplacement, GatewayError> {
        self.inner
            .calls
            .lock()
            .unwrap()
            .push("configuration".into());
        Ok(ClaudeDirectoryReplacement::Changed { previous: None })
    }
    fn register(
        &self,
        runtime: &Path,
        stage: &str,
        path: Option<&SearchPath>,
        attempt: &GatewayReconciliationAttempt,
        progress: &dyn GatewayReconciliationProgress,
    ) -> Result<ReconciledGateway, GatewayError> {
        if self.fail_registration.load(Ordering::SeqCst) {
            admit(attempt, progress, "failed-owner");
            return Err(GatewayError::Registration(
                "controlled registration failure".into(),
            ));
        }
        self.inner.register(runtime, stage, path, attempt, progress)
    }
    fn recover(
        &self,
        _: &GatewayLifecycleRecovery,
        _: &dyn GatewayReconciliationJournalSession,
    ) -> Result<(), GatewayError> {
        self.inner.calls.lock().unwrap().push("recovery".into());
        Ok(())
    }
    fn stop_agents(
        &self,
        session: &GatewayStopSession,
        journal: &dyn GatewayReconciliationJournalSession,
        plan: &AuditDeliveryReceipt,
    ) -> Result<LifecycleObservation, GatewayError> {
        if let Some((entered, release)) = self.stop_gate.lock().unwrap().take() {
            entered.send(()).unwrap();
            release.recv_timeout(Duration::from_secs(2)).unwrap();
        }
        self.inner.stop_agents(session, journal, plan)
    }
}
struct CountJournalOpens(AtomicUsize);
impl GatewayReconciliationAudit for CountJournalOpens {
    fn open(
        self: Arc<Self>,
        attempt: &GatewayReconciliationAttempt,
        deadline: Option<Instant>,
    ) -> Result<Arc<dyn GatewayReconciliationJournalSession>, GatewayError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        testing::discard_reconciliation_audit().open(attempt, deadline)
    }
}
fn owned_gateway(
    host: Arc<OwnedHost>,
    audit: Arc<CountJournalOpens>,
    bundle: &str,
) -> Arc<Gateway> {
    Arc::new(Gateway::bootstrap_with_dependencies(
        GatewayRuntimeDependencies::new(
            host,
            login_shell("/usr/bin"),
            testing::discard_startup_events(),
            testing::sequential_reconciliation_ids(),
            audit,
            Arc::new(VirtualOwnershipClock(Mutex::new(Instant::now()))),
        ),
        PathBuf::from(bundle),
        "ci".into(),
    ))
}

#[test]
fn competing_host_is_refused_before_journal_settings_recovery_and_stop() {
    let pool = Arc::new(AtomicBool::new(false));
    let first_host = OwnedHost::new(pool.clone(), "first-bundle");
    let second_host = OwnedHost::new(pool, "second-bundle");
    let first = owned_gateway(
        first_host.clone(),
        Arc::new(CountJournalOpens(AtomicUsize::new(0))),
        "/first/runtime",
    );
    let second_audit = Arc::new(CountJournalOpens(AtomicUsize::new(0)));
    let second = owned_gateway(second_host.clone(), second_audit.clone(), "/second/runtime");
    tauri::async_runtime::block_on(first.start()).unwrap();
    assert_eq!(
        tauri::async_runtime::block_on(second.start()),
        Err(GatewayError::DesktopHostBusy)
    );
    assert_eq!(
        tauri::async_runtime::block_on(second.wait_ready(BundledSurface::Main)),
        Err(GatewayError::DesktopHostBusy)
    );
    let settings = Arc::new(DirectorySettings(Mutex::new(None)));
    assert_eq!(
        tauri::async_runtime::block_on(second.change_claude_configuration(
            BundledSurface::Main,
            Some(absolute_claude_directory("competing")),
            settings.clone()
        )),
        Err(ClaudeConfigurationChangeError::Gateway(
            GatewayError::DesktopHostBusy
        ))
    );
    assert_eq!(*settings.0.lock().unwrap(), None);
    assert_eq!(
        second.stop_agents(Instant::now() + Duration::from_secs(30)),
        Err(GatewayError::NotReconciled)
    );
    assert_eq!(second_audit.0.load(Ordering::SeqCst), 0);
    assert!(second_host.inner.calls.lock().unwrap().is_empty());
    tauri::async_runtime::block_on(first.retry(BundledSurface::Main)).unwrap();
    assert_eq!(first_host.claims.load(Ordering::SeqCst), 1);
    drop(first);
    let release_deadline = Instant::now() + Duration::from_secs(2);
    while second_host.pool.load(Ordering::SeqCst) {
        assert!(
            Instant::now() < release_deadline,
            "finished first host retained its worker lease"
        );
        thread::yield_now();
    }
    tauri::async_runtime::block_on(second.retry(BundledSurface::Main)).unwrap();
    assert_eq!(second_audit.0.load(Ordering::SeqCst), 1);
    assert_eq!(second_host.inner.calls.lock().unwrap().len(), 1);
}

#[test]
fn failed_registration_keeps_ownership_for_its_original_host_retry() {
    let pool = Arc::new(AtomicBool::new(false));
    let first_host = OwnedHost::new(pool.clone(), "first-bundle");
    first_host.fail_registration.store(true, Ordering::SeqCst);
    let first = owned_gateway(
        first_host.clone(),
        Arc::new(CountJournalOpens(AtomicUsize::new(0))),
        "/first/runtime",
    );
    assert!(tauri::async_runtime::block_on(first.start()).is_err());
    let second_host = OwnedHost::new(pool, "second-bundle");
    let second = owned_gateway(
        second_host,
        Arc::new(CountJournalOpens(AtomicUsize::new(0))),
        "/second/runtime",
    );
    assert_eq!(
        tauri::async_runtime::block_on(second.start()),
        Err(GatewayError::DesktopHostBusy)
    );
    first_host.fail_registration.store(false, Ordering::SeqCst);
    tauri::async_runtime::block_on(first.retry(BundledSurface::Main)).unwrap();
    assert_eq!(first_host.claims.load(Ordering::SeqCst), 1);
}

struct GatedOwnershipClock {
    now: Mutex<Instant>,
    first_wait: Mutex<Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>>,
}
impl MonotonicClock for GatedOwnershipClock {
    fn now(&self) -> Instant {
        *self.now.lock().unwrap()
    }
    fn wait(&self, duration: Duration) {
        if let Some((entered, release)) = self.first_wait.lock().unwrap().take() {
            entered.send(()).unwrap();
            release.recv_timeout(Duration::from_secs(2)).unwrap();
        }
        *self.now.lock().unwrap() += duration;
    }
}

#[test]
fn ownership_refusal_releases_callers_waiting_for_the_unopened_journal() {
    let (entered, wait_entered) = mpsc::channel();
    let (release, wait_release) = mpsc::channel();
    let audit = Arc::new(CountJournalOpens(AtomicUsize::new(0)));
    let host = OwnedHost::new(Arc::new(AtomicBool::new(true)), "competing");
    let gateway = Arc::new(Gateway::bootstrap_with_dependencies(
        GatewayRuntimeDependencies::new(
            host,
            login_shell("/usr/bin"),
            testing::discard_startup_events(),
            testing::sequential_reconciliation_ids(),
            audit.clone(),
            Arc::new(GatedOwnershipClock {
                now: Mutex::new(Instant::now()),
                first_wait: Mutex::new(Some((entered, wait_release))),
            }),
        ),
        "/competing/runtime".into(),
        "ci".into(),
    ));
    let start_gateway = gateway.clone();
    let starter = thread::spawn(move || tauri::async_runtime::block_on(start_gateway.start()));
    wait_entered.recv_timeout(Duration::from_secs(2)).unwrap();
    let receipt = gateway.lifecycle.lock().unwrap().running.clone().unwrap();
    let (answered, wait_answer) = mpsc::channel();
    let journal_waiter = thread::spawn(move || {
        answered.send(receipt.wait_journal().map(|_| ())).unwrap();
    });
    release.send(()).unwrap();
    assert_eq!(starter.join().unwrap(), Err(GatewayError::DesktopHostBusy));
    assert_eq!(
        wait_answer.recv_timeout(Duration::from_secs(2)).unwrap(),
        Err(GatewayError::DesktopHostBusy)
    );
    journal_waiter.join().unwrap();
    assert_eq!(audit.0.load(Ordering::SeqCst), 0);
}

#[test]
fn timed_out_stop_worker_retains_ownership_after_the_gateway_is_dropped() {
    let pool = Arc::new(AtomicBool::new(false));
    let host = OwnedHost::new(pool.clone(), "stop-owner");
    let gateway = owned_gateway(
        host.clone(),
        Arc::new(CountJournalOpens(AtomicUsize::new(0))),
        "/stop/runtime",
    );
    tauri::async_runtime::block_on(gateway.start()).unwrap();
    let (entered, wait_entered) = mpsc::channel();
    let (release, wait_release) = mpsc::channel();
    *host.stop_gate.lock().unwrap() = Some((entered, wait_release));
    let stop_gateway = gateway.clone();
    let stopper = thread::spawn(move || {
        stop_gateway.stop_agents(Instant::now() + Duration::from_millis(100))
    });
    wait_entered.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(matches!(
        stopper.join().unwrap(),
        Err(GatewayError::Stop(_))
    ));
    drop(gateway);
    assert!(
        pool.load(Ordering::SeqCst),
        "pending physical stop lost host ownership"
    );
    release.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while pool.load(Ordering::SeqCst) {
        assert!(
            Instant::now() < deadline,
            "stop worker did not release ownership"
        );
        thread::yield_now();
    }
}

struct PanickingOwnershipClock {
    first_read: Mutex<Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>>,
}
impl MonotonicClock for PanickingOwnershipClock {
    fn now(&self) -> Instant {
        if let Some((entered, release)) = self.first_read.lock().unwrap().take() {
            entered.send(()).unwrap();
            release.recv_timeout(Duration::from_secs(2)).unwrap();
            panic!("first ownership clock read failed");
        }
        Instant::now()
    }
}

#[test]
fn first_ownership_clock_panic_settles_the_unopened_journal_waiter() {
    let (entered, wait_entered) = mpsc::channel();
    let (release, wait_release) = mpsc::channel();
    let audit = Arc::new(CountJournalOpens(AtomicUsize::new(0)));
    let host = OwnedHost::new(Arc::new(AtomicBool::new(false)), "clock-panic");
    let gateway = Arc::new(Gateway::bootstrap_with_dependencies(
        GatewayRuntimeDependencies::new(
            host,
            login_shell("/usr/bin"),
            testing::discard_startup_events(),
            testing::sequential_reconciliation_ids(),
            audit.clone(),
            Arc::new(PanickingOwnershipClock {
                first_read: Mutex::new(Some((entered, wait_release))),
            }),
        ),
        "/clock-panic/runtime".into(),
        "ci".into(),
    ));
    let start_gateway = gateway.clone();
    let starter = thread::spawn(move || tauri::async_runtime::block_on(start_gateway.start()));
    wait_entered.recv_timeout(Duration::from_secs(2)).unwrap();
    let receipt = gateway.lifecycle.lock().unwrap().running.clone().unwrap();
    let (answered, wait_answer) = mpsc::channel();
    let journal_waiter = thread::spawn(move || {
        answered.send(receipt.wait_journal().map(|_| ())).unwrap();
    });
    release.send(()).unwrap();
    assert!(matches!(
        starter.join().unwrap(),
        Err(GatewayError::Registration(_))
    ));
    assert!(matches!(
        wait_answer.recv_timeout(Duration::from_secs(2)).unwrap(),
        Err(GatewayError::Registration(_))
    ));
    journal_waiter.join().unwrap();
    assert_eq!(audit.0.load(Ordering::SeqCst), 0);
}
