use super::support::{pending, sockets, Fixture, WAIT};
use nessa_auth::{
    adapters::{
        local::LocalCredentialStore,
        pairing::{
            rand::Error as EntropyError, CryptoRng, FilePairingState, GatewayTrust, ManualCode,
            NativeIdentity, NativeTransport, OsEntropy, RngCore,
        },
    },
    application::pairing::{
        ClientPendingStore, PairingStore, PairingWorkerFault, PendingEnrollment,
        PrivateKeyMaterial, PrivateStateError,
    },
    domain::pairing::{InvitationId, PublicIntent},
};
use nessa_server::{
    app::dependencies::RuntimeDependencies,
    device_pairing::infrastructure::{
        wire::NativePairingStatus, NativeClientError, NativeConnectionFailure,
        NativeEnrollmentClient, NativeEnrollmentConnections, NativeWakeCause, PairingRuntimeError,
        RegisteredInvitation, RegistrationError, RegistrationWorker,
    },
};
use std::{
    net::TcpStream,
    path::Path,
    sync::{
        mpsc::{self, SyncSender},
        Arc, Condvar, Mutex,
    },
};

/// Run one synchronous registration on a blocking worker, as create does.
async fn register<R: RngCore + CryptoRng + Send + 'static>(
    worker: &Arc<RegistrationWorker>,
    fixture: &Fixture,
    entropy: R,
    id: u8,
) -> Result<Result<RegisteredInvitation, RegistrationError>, tokio::task::JoinError> {
    let worker = worker.clone();
    let gateway = fixture.gateway.clone();
    tokio::time::timeout(
        WAIT,
        tokio::task::spawn_blocking(move || {
            worker.register(entropy, InvitationId::new([id; 16]), gateway.identity())
        }),
    )
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_registration_output_retains_original_capacity() {
    let fixture = Fixture::new().await;
    let worker = Arc::new(RegistrationWorker::new());
    let output = register(&worker, &fixture, OsEntropy, 1)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        register(&worker, &fixture, OsEntropy, 2).await.unwrap(),
        Err(RegistrationError::Busy)
    ));
    drop(output);
    let neighbor = register(&worker, &fixture, OsEntropy, 2)
        .await
        .unwrap()
        .unwrap();
    drop(neighbor);
    worker.shutdown().await;
    fixture.gateway.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_shutdown_keeps_original_physical_capacity() {
    let fixture = Fixture::new().await;
    let connections = Arc::new(NativeEnrollmentConnections::new(
        fixture.gateway.clone(),
        RuntimeDependencies::default().clock,
    ));
    let mut peers = Vec::new();
    let mut observers = Vec::new();
    let mut targets = Vec::new();
    for _ in 0..8 {
        let (server, client) = sockets();
        targets.push(client.local_addr().unwrap());
        let owner = connections.clone();
        observers.push(tokio::spawn(
            async move { owner.serve(server, OsEntropy).await },
        ));
        let identity = NativeIdentity::generate(&mut OsEntropy).unwrap();
        let pin = fixture.gateway.identity().public_spki();
        // Completing real TLS proves this original physical socket was admitted;
        // its enrollment read remains blocked without a Hello frame.
        peers.push(
            tokio::task::spawn_blocking(move || {
                NativeTransport::connect(client, &identity, GatewayTrust::Pinned(pin))
            })
            .await
            .unwrap()
            .unwrap(),
        );
    }
    for observer in observers {
        observer.abort();
        assert!(matches!(observer.await, Err(error) if error.is_cancelled()));
    }
    let (ninth, other) = sockets();
    assert_eq!(
        connections
            .serve(ninth, OsEntropy)
            .await
            .unwrap_err()
            .failure,
        NativeConnectionFailure::Capacity
    );
    drop(other);
    // The eight workers are blocked reading their sockets, with 30 s left on
    // their enrollment deadline; the wake must end them within its tick on
    // every OS, so the drain finishes long before the deadline could.
    tokio::time::timeout(std::time::Duration::from_secs(5), connections.shutdown())
        .await
        .expect("shutdown wakes blocked socket reads");
    let first = connections.wake_report().unwrap();
    assert!(first
        .iter()
        .all(|entry| entry.cause() == NativeWakeCause::AdmissionRetirement));
    let mut actual: Vec<_> = first.iter().map(|entry| entry.target()).collect();
    actual.sort();
    targets.sort();
    assert_eq!(actual, targets);
    tokio::time::timeout(WAIT, connections.shutdown())
        .await
        .unwrap();
    assert_eq!(connections.wake_report(), Some(first));
    let (closed, other) = sockets();
    assert_eq!(
        connections
            .serve(closed, OsEntropy)
            .await
            .unwrap_err()
            .failure,
        NativeConnectionFailure::Capacity
    );
    drop(other);
    drop(peers);
    fixture.gateway.shutdown().await;
}

struct Release {
    open: Mutex<bool>,
    changed: Condvar,
}
impl Release {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            open: Mutex::new(false),
            changed: Condvar::new(),
        })
    }
    fn wait(&self) {
        let mut open = self.open.lock().unwrap();
        while !*open {
            let (next, limit) = self.changed.wait_timeout(open, WAIT).unwrap();
            open = next;
            assert!(!limit.timed_out(), "original IO gate not released");
        }
    }
    fn open(&self) {
        *self.open.lock().unwrap() = true;
        self.changed.notify_all();
    }
}
struct GatedEntropy {
    entered: Option<SyncSender<()>>,
    release: Arc<Release>,
}
impl RngCore for GatedEntropy {
    fn next_u32(&mut self) -> u32 {
        let mut bytes = [0; 4];
        self.fill_bytes(&mut bytes);
        u32::from_le_bytes(bytes)
    }
    fn next_u64(&mut self) -> u64 {
        let mut bytes = [0; 8];
        self.fill_bytes(&mut bytes);
        u64::from_le_bytes(bytes)
    }
    fn fill_bytes(&mut self, bytes: &mut [u8]) {
        self.try_fill_bytes(bytes).unwrap();
    }
    fn try_fill_bytes(&mut self, bytes: &mut [u8]) -> Result<(), EntropyError> {
        if let Some(entered) = self.entered.take() {
            entered.send(()).unwrap();
            self.release.wait();
        }
        OsEntropy.try_fill_bytes(bytes)
    }
}
impl CryptoRng for GatedEntropy {}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_create_observer_loss_keeps_original_owner_until_drain() {
    let fixture = Fixture::new().await;
    let release = Release::new();
    let (entered, received) = mpsc::sync_channel(1);
    let entropy = GatedEntropy {
        entered: Some(entered),
        release: release.clone(),
    };
    let owner = fixture.gateway.clone();
    let session = fixture.session.clone();
    let observer = tokio::spawn(async move { owner.create(session, entropy).await });
    tokio::task::spawn_blocking(move || received.recv_timeout(WAIT).unwrap())
        .await
        .unwrap();
    observer.abort();
    assert!(matches!(observer.await, Err(error) if error.is_cancelled()));
    assert!(matches!(
        fixture
            .gateway
            .create(fixture.session.clone(), OsEntropy)
            .await,
        Err(PairingRuntimeError::Busy)
    ));
    let retained = Arc::downgrade(&fixture.gateway);
    let Fixture {
        directory,
        registry,
        session,
        gateway,
        time: _,
        owner_token: _,
    } = fixture;
    drop(gateway);
    drop(registry);
    #[cfg(unix)]
    let parent = directory.path().canonicalize().unwrap();
    #[cfg(not(unix))]
    let parent = directory.path().to_path_buf();
    let private = parent.join("gateway-private");
    assert!(matches!(
        FilePairingState::open(&private, Path::new("state")),
        Err(PrivateStateError::Locked)
    ));
    let owner = retained
        .upgrade()
        .expect("original physical create retains runtime");
    {
        let shutdown = owner.shutdown();
        tokio::pin!(shutdown);
        assert!(futures_util::poll!(shutdown.as_mut()).is_pending());
        release.open();
        tokio::time::timeout(WAIT, shutdown).await.unwrap();
    }
    assert!(matches!(
        owner.create(session, OsEntropy).await,
        Err(PairingRuntimeError::Busy)
    ));
    drop(owner);
    let reopened = FilePairingState::open(&private, Path::new("state")).unwrap();
    drop(reopened);
    let registry = LocalCredentialStore::open(parent.join("registry"), "credentials.json").unwrap();
    assert!(registry.pending_pairings().unwrap().is_empty());
    drop(registry);
    let accepted = Fixture::new().await;
    assert!(accepted
        .gateway
        .create(accepted.session.clone(), OsEntropy)
        .await
        .is_ok());
    accepted.gateway.shutdown().await;
}

/// Delay only the acknowledgement return of an actual public private-file save.
/// The durable record and all bytes come from the production TLS/PAKE exchange.
struct AcknowledgedSave {
    state: Arc<FilePairingState>,
    entered: Mutex<Option<SyncSender<()>>>,
    release: Arc<Release>,
}
impl ClientPendingStore for AcknowledgedSave {
    fn load_pending(&self) -> Result<Option<PendingEnrollment>, PrivateStateError> {
        self.state.load_pending()
    }
    fn save_pending(
        &self,
        key: &PrivateKeyMaterial,
        pin: &[u8; 44],
        intent: PublicIntent,
        expected: Option<PublicIntent>,
    ) -> Result<(), PrivateStateError> {
        self.state.save_pending(key, pin, intent, expected)?;
        if let Some(entered) = self.entered.lock().unwrap().take() {
            entered.send(()).unwrap();
            self.release.wait();
        }
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_client_observer_loss_keeps_pending_save_and_operation_owned() {
    let fixture = Fixture::new().await;
    let created = fixture
        .gateway
        .create(fixture.session.clone(), OsEntropy)
        .await
        .unwrap();
    let (address, stop, listener, connections) = fixture.listener().await;
    let (_, state) = pending(fixture.directory.path(), "client-private");
    let release = Release::new();
    let (entered, received) = mpsc::sync_channel(1);
    let pending = Arc::new(AcknowledgedSave {
        state: state.clone(),
        entered: Mutex::new(Some(entered)),
        release: release.clone(),
    });
    let client = Arc::new(NativeEnrollmentClient::new(
        pending,
        RuntimeDependencies::default().clock,
    ));
    let owner = client.clone();
    let code = ManualCode::parse(created.code().expose_bytes()).unwrap();
    let stream = TcpStream::connect(address).unwrap();
    let target = stream.peer_addr().unwrap();
    let observer = tokio::spawn(async move { owner.enroll(stream, code, OsEntropy).await });
    tokio::task::spawn_blocking(move || received.recv_timeout(WAIT).unwrap())
        .await
        .unwrap();
    let public = state.load_pending().unwrap().unwrap().intent();
    observer.abort();
    assert!(matches!(observer.await, Err(error) if error.is_cancelled()));
    assert_eq!(
        client
            .status(TcpStream::connect(address).unwrap(), None)
            .await
            .unwrap_err(),
        NativeClientError::Busy
    );
    let shutdown = client.shutdown();
    tokio::pin!(shutdown);
    assert!(futures_util::poll!(shutdown.as_mut()).is_pending());
    let wake = client.wake_report().unwrap();
    assert_eq!(
        wake.iter().map(|entry| entry.target()).collect::<Vec<_>>(),
        vec![target]
    );
    assert_eq!(state.load_pending().unwrap().unwrap().intent(), public);
    release.open();
    tokio::time::timeout(WAIT, shutdown).await.unwrap();
    // Collect the original gateway peer before starting a fresh status operation.
    stop.send(()).unwrap();
    tokio::time::timeout(WAIT, listener)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    connections.shutdown().await;
    let (address, stop, listener, connections) = fixture.listener().await;
    let resumed = NativeEnrollmentClient::new(state.clone(), RuntimeDependencies::default().clock);
    let status = tokio::time::timeout(
        WAIT,
        resumed.status(TcpStream::connect(address).unwrap(), None),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(status.public(), public);
    assert!(matches!(status, NativePairingStatus::Unclaimed { .. }));
    assert_eq!(state.load_pending().unwrap().unwrap().intent(), public);
    let original = status.clone();
    let code = ManualCode::parse(created.code().expose_bytes()).unwrap();
    let retried = tokio::time::timeout(
        WAIT,
        resumed.retry(
            TcpStream::connect(address).unwrap(),
            TcpStream::connect(address).unwrap(),
            code,
            OsEntropy,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(retried.original, original);
    assert!(matches!(retried.retried, NativePairingStatus::Claimed(_)));
    let renewed = retried.retried.public();
    assert_ne!(renewed.attempt(), public.attempt());
    assert_eq!(public.with_attempt(renewed.attempt()), renewed);
    assert_eq!(state.load_pending().unwrap().unwrap().intent(), renewed);
    let record = fixture.registry.read_pairing(public.invitation()).unwrap();
    let (_, key) = record.claim_binding().unwrap();
    let NativePairingStatus::Unclaimed { outcome, .. } = original else {
        panic!("terminal original attempt")
    };
    assert_eq!(
        record.attempt_status(public.attempt(), key).unwrap(),
        outcome
    );
    resumed.shutdown().await;
    stop.send(()).unwrap();
    tokio::time::timeout(WAIT, listener)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    connections.shutdown().await;
    fixture.gateway.shutdown().await;
}

/// Fills once, then panics: create's identifiers succeed, registration panics.
struct PanicAfterFirstFill(bool);
impl RngCore for PanicAfterFirstFill {
    fn next_u32(&mut self) -> u32 {
        panic!("external entropy fault")
    }
    fn next_u64(&mut self) -> u64 {
        panic!("external entropy fault")
    }
    fn fill_bytes(&mut self, bytes: &mut [u8]) {
        self.try_fill_bytes(bytes).unwrap();
    }
    fn try_fill_bytes(&mut self, bytes: &mut [u8]) -> Result<(), EntropyError> {
        assert!(!self.0, "external entropy fault");
        self.0 = true;
        OsEntropy.try_fill_bytes(bytes)
    }
}
impl CryptoRng for PanicAfterFirstFill {}

struct PanicEntropy;
impl RngCore for PanicEntropy {
    fn next_u32(&mut self) -> u32 {
        panic!("external entropy fault")
    }
    fn next_u64(&mut self) -> u64 {
        panic!("external entropy fault")
    }
    fn fill_bytes(&mut self, _: &mut [u8]) {
        panic!("external entropy fault")
    }
    fn try_fill_bytes(&mut self, _: &mut [u8]) -> Result<(), EntropyError> {
        panic!("external entropy fault")
    }
}
impl CryptoRng for PanicEntropy {}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_worker_faults_preserve_type_and_allow_new_work() {
    let fixture = Fixture::new().await;
    assert!(matches!(
        fixture
            .gateway
            .create(fixture.session.clone(), PanicEntropy)
            .await,
        Err(PairingRuntimeError::WorkerFault(PairingWorkerFault::Panic))
    ));
    assert!(fixture.registry.pending_pairings().unwrap().is_empty());
    // A panic inside create's registration stage is the create's worker fault.
    assert!(matches!(
        fixture
            .gateway
            .create(fixture.session.clone(), PanicAfterFirstFill(false))
            .await,
        Err(PairingRuntimeError::WorkerFault(PairingWorkerFault::Panic))
    ));
    assert!(fixture.registry.pending_pairings().unwrap().is_empty());
    // A panicking registration releases its slot on unwind.
    let worker = Arc::new(RegistrationWorker::new());
    assert!(matches!(
        register(&worker, &fixture, PanicEntropy, 25).await,
        Err(error) if error.is_panic()
    ));
    let output = register(&worker, &fixture, OsEntropy, 25)
        .await
        .unwrap()
        .unwrap();
    drop(output);
    worker.shutdown().await;
    let created = fixture
        .gateway
        .create(fixture.session.clone(), OsEntropy)
        .await
        .unwrap();
    let (address, stop, listener, connections) = fixture.listener().await;
    let (_, state) = pending(fixture.directory.path(), "client-private");
    let client = NativeEnrollmentClient::new(state.clone(), RuntimeDependencies::default().clock);
    let code = ManualCode::parse(created.code().expose_bytes()).unwrap();
    assert_eq!(
        tokio::time::timeout(
            WAIT,
            client.enroll(TcpStream::connect(address).unwrap(), code, PanicEntropy)
        )
        .await
        .unwrap()
        .unwrap_err(),
        NativeClientError::WorkerFault(PairingWorkerFault::Panic)
    );
    assert!(state.load_pending().unwrap().is_none());
    let code = ManualCode::parse(created.code().expose_bytes()).unwrap();
    assert!(matches!(
        tokio::time::timeout(
            WAIT,
            client.enroll(TcpStream::connect(address).unwrap(), code, OsEntropy)
        )
        .await
        .unwrap()
        .unwrap(),
        NativePairingStatus::Claimed(_)
    ));
    client.shutdown().await;
    stop.send(()).unwrap();
    tokio::time::timeout(WAIT, listener)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    connections.shutdown().await;
    fixture.gateway.shutdown().await;
}
