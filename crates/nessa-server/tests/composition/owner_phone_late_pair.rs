//! Actual signed owner approval with a late physical Pair outcome.
//! Actual signed socket request loses its observer while its real Pair is held.
use super::*;
use crate::device_pairing::{
    application::{
        PairingReceivers, PublicationGuard, ReceiverDispatch, ReceiverError, ReceiverRequest,
    },
    infrastructure::{
        restore_gateway_identity, ConversationReceivers, GatewayPairing,
        NativeEnrollmentConnections, NativeEnrollmentListener, PairingOwnerCommands,
        PairingRuntimeDependencies, TcpEnrollmentAccept,
    },
};
use nessa_auth::{
    adapters::pairing::{FilePairingState, OsEntropy},
    application::{
        pairing::{PairingStoreError, ReceiverOutcome},
        ports::{AccessError, AccessReader, Clock},
    },
    domain::{pairing::PairingPhase, ResourceId},
};
use std::{
    net::{Shutdown, TcpStream},
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        mpsc, Mutex,
    },
};
use tokio::sync::oneshot;
struct Time(AtomicU64);
impl Clock for Time {
    fn unix_milliseconds(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}
struct HeldPair {
    real: ConversationReceivers,
    entered: Mutex<Option<oneshot::Sender<ReceiverOutcome>>>,
    release: Mutex<mpsc::Receiver<()>>,
    fenced: Mutex<Option<oneshot::Sender<ReceiverOutcome>>>,
    pairs: AtomicUsize,
    fences: AtomicUsize,
}
impl PairingReceivers for HeldPair {
    fn lock_publication(&self) -> Result<PublicationGuard, ReceiverError> {
        self.real.lock_publication()
    }

    fn pair(&self, dispatch: &ReceiverDispatch<'_>) -> Result<ReceiverOutcome, ReceiverError> {
        self.pairs.fetch_add(1, Ordering::SeqCst);
        let outcome = self.real.pair(dispatch)?;
        self.entered
            .lock()
            .unwrap()
            .take()
            .unwrap()
            .send(outcome.clone())
            .unwrap();
        self.release.lock().unwrap().recv_timeout(WAIT).unwrap();
        Ok(outcome)
    }
    fn paired(&self, request: &ReceiverRequest) -> Result<Option<ReceiverOutcome>, ReceiverError> {
        self.real.paired(request)
    }
    fn holding(
        &self,
        request: &ReceiverRequest,
        receiver: &ResourceId,
        epoch: u64,
    ) -> Result<Option<u64>, ReceiverError> {
        self.real.holding(request, receiver, epoch)
    }
    fn fence(
        &self,
        dispatch: &ReceiverDispatch<'_>,
        receiver: &ResourceId,
        epoch: u64,
    ) -> Result<ReceiverOutcome, ReceiverError> {
        self.fences.fetch_add(1, Ordering::SeqCst);
        let outcome = self.real.fence(dispatch, receiver, epoch)?;
        if let Some(sender) = self.fenced.lock().unwrap().take() {
            sender.send(outcome.clone()).unwrap();
        }
        Ok(outcome)
    }
}
fn response(
    socket: &mut tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>,
) -> Value {
    loop {
        match socket.read().unwrap() {
            tungstenite::Message::Text(text) => return serde_json::from_str(&text).unwrap(),
            tungstenite::Message::Ping(_) | tungstenite::Message::Pong(_) => {}
            other => panic!("unexpected observer frame {other:?}"),
        }
    }
}
fn observer(
    address: SocketAddr,
    token: &str,
    params: Value,
) -> tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>> {
    let (mut socket, _) = tungstenite::connect(format!("ws://{address}/session")).unwrap();
    let challenge = response(&mut socket);
    socket.send(tungstenite::Message::Text(json!({"type":"req","id":"1","method":"session.authenticate","params":{"minVersion":1,"maxVersion":1,"nonce":challenge["payload"]["nonce"],"credential":token,"client":{"id":"nessa-cli"},"surface":{"kind":"cli","instance":"nessa-cli"}}}).to_string().into())).unwrap();
    assert_eq!(response(&mut socket)["ok"], true);
    socket
        .send(tungstenite::Message::Text(
            json!({"type":"req","id":"2","method":"phone.authorize","params":params})
                .to_string()
                .into(),
        ))
        .unwrap();
    socket
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn signed_owner_phone_lost_observer_retains_late_pair_and_first_expiry_until_physical_drain()
{
    let namespace = Namespace::new();
    let registry = namespace.registry();
    let token = credential(&registry);
    let clock = Arc::new(Time(AtomicU64::new(SystemClock.unix_milliseconds())));
    // A canonicalized temp path is a reparse point on Windows, which the
    // private-directory check refuses. Unix resolves the temp alias first.
    #[cfg(windows)]
    let parent = namespace.root();
    #[cfg(not(windows))]
    let parent = namespace.root().canonicalize().unwrap();
    let private = parent.join("late-private");
    nessa_local_storage::create_directory(&private).unwrap();
    nessa_local_storage::create_directory_beneath(&private, std::path::Path::new("state")).unwrap();
    let keys = Arc::new(FilePairingState::open(&private, std::path::Path::new("state")).unwrap());
    let identity =
        restore_gateway_identity(registry.clone(), keys.clone(), clock.clone(), OsEntropy)
            .await
            .unwrap();
    let receivers = namespace.receivers();
    let (entered, parked) = oneshot::channel();
    let (release, released) = mpsc::sync_channel(1);
    let (fenced, fence_observed) = oneshot::channel();
    let held = Arc::new(HeldPair {
        real: ConversationReceivers::new(receivers.clone()),
        entered: Mutex::new(Some(entered)),
        release: Mutex::new(released),
        fenced: Mutex::new(Some(fenced)),
        pairs: AtomicUsize::new(0),
        fences: AtomicUsize::new(0),
    });
    let gateway = Arc::new(
        GatewayPairing::open(PairingRuntimeDependencies {
            enrollments: registry.clone(),
            originals: registry.clone(),
            access: registry.clone(),
            policy: Arc::new(CedarPolicyEvaluator::new().unwrap()),
            receivers: held.clone(),
            clock: clock.clone(),
            gateway: Resource::new(
                OrganizationId::new("org").unwrap(),
                ResourceId::new("gateway").unwrap(),
            ),
            key_store: keys.clone(),
            identity,
            tunnel_secrets: Arc::new(crate::tunnel::application::MemoryDeviceSecrets::new()),
        })
        .unwrap(),
    );
    let native_socket =
        TcpEnrollmentAccept::new(tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap());
    let native_address = native_socket.local_address().unwrap();
    gateway.publish_native_endpoint(native_address).unwrap();
    let connections = Arc::new(NativeEnrollmentConnections::new(
        gateway.clone(),
        RuntimeDependencies::default().clock,
    ));
    let native = NativeEnrollmentListener::new(native_socket, connections);
    let (stop, stopped) = oneshot::channel();
    let listener = tokio::spawn(native.run(
        OsEntropy::default,
        async { stopped.await.unwrap() },
        |_| panic!("native listener failed"),
    ));
    let commands = Arc::new(PairingOwnerCommands::new(
        gateway.clone(),
        Arc::new(|| Box::new(OsEntropy)),
    ));
    let state = product(registry.clone()).with_pairing(commands);
    let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = socket.local_addr().unwrap();
    let serving = tokio::spawn(async move {
        axum::serve(socket, crate::server::entrypoint::http::router(state))
            .await
            .unwrap()
    });
    let params = tokio::task::block_in_place(|| {
        let mut client = ProductClient::connect(address, &token);
        signed(&mut client, 81)
    });
    let observed_socket = tokio::task::block_in_place(|| observer(address, &token, params.clone()));
    let paired = tokio::time::timeout(WAIT, parked).await.unwrap().unwrap();
    let staging = registry
        .pending_pairings()
        .unwrap()
        .iter()
        .find(|record| record.phase() == PairingPhase::Staging)
        .cloned()
        .unwrap();
    let original_id = staging.id();
    assert_eq!(staging.stage_binding().unwrap().0, paired.credential());
    assert!(staging.receiver_binding().is_none());
    assert!(
        matches!(
            registry.clone().acquire_stage(original_id),
            Err(PairingStoreError::StageOccupied)
        ),
        "physical Pair owns the stage before observer loss"
    );
    // Close the actual observing socket; its lack of a result cannot release a physical stage.
    match observed_socket.get_ref() {
        tungstenite::stream::MaybeTlsStream::Plain(socket) => {
            socket.shutdown(Shutdown::Both).unwrap()
        }
        _ => panic!("loopback plain socket"),
    };
    drop(observed_socket);
    assert!(matches!(
        registry.clone().acquire_stage(original_id),
        Err(PairingStoreError::StageOccupied)
    ));
    clock.0.store(staging.expires_at_ms() + 1, Ordering::SeqCst);
    let terminal = tokio::task::block_in_place(|| {
        let mut client = ProductClient::connect(address, &token);
        client.ok("phone.authorize", params.clone())
    });
    assert_eq!(terminal["phase"], "terminal");
    assert_eq!(terminal["terminal"]["cause"], "expired");
    assert_eq!(terminal["cleanupPending"], true);
    assert!(matches!(
        registry.clone().acquire_stage(original_id),
        Err(PairingStoreError::StageOccupied)
    ));
    assert_eq!(held.pairs.load(Ordering::SeqCst), 1);
    assert_eq!(held.fences.load(Ordering::SeqCst), 0);
    assert!(
        receivers
            .binding(paired.credential())
            .unwrap()
            .unwrap()
            .active
    );
    let closing = gateway.clone();
    let (started, has_started) = oneshot::channel();
    let mut shutdown = tokio::spawn(async move {
        started.send(()).unwrap();
        closing.shutdown().await
    });
    has_started.await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut shutdown)
            .await
            .is_err(),
        "physical Pair must retain actual shutdown drain while its barrier is held"
    );
    release.send(()).unwrap();
    let fenced = tokio::time::timeout(WAIT, fence_observed)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(WAIT, shutdown).await.unwrap().unwrap();
    let settled = registry.read_pairing(original_id).unwrap();
    assert_eq!(settled.phase(), PairingPhase::Terminal);
    assert_eq!(
        settled.terminal().unwrap().0,
        nessa_auth::domain::pairing::TerminalCause::Expired
    );
    assert!(!settled.cleanup_pending());
    assert_eq!(settled.id(), original_id);
    assert_eq!(fenced.receiver(), paired.receiver());
    assert!(fenced.epoch() > paired.epoch());
    assert_eq!(held.pairs.load(Ordering::SeqCst), 1);
    assert_eq!(held.fences.load(Ordering::SeqCst), 1);
    let binding = receivers.binding(paired.credential()).unwrap().unwrap();
    assert!(!binding.active);
    assert_eq!(binding.receiver_id, paired.receiver().as_str());
    assert_eq!(binding.access_epoch, fenced.epoch());
    // No canonical delegated publication occurred; the stopped stage is historical evidence.
    assert!(matches!(
        registry.read(paired.credential()).await,
        Err(AccessError::InvalidCredential)
    ));
    stop.send(()).unwrap();
    tokio::time::timeout(WAIT, listener)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    serving.abort();
    let _ = serving.await;
}
