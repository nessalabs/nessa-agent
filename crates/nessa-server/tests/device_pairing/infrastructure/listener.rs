//! The listener's accept policy, driven through a scripted accept port.
use super::support::{sockets, FaultyStore, Fixture, WAIT};
use nessa_auth::domain::pairing::ConsentClass;
use nessa_auth::{
    adapters::pairing::{GatewayTrust, NativeIdentity, NativeTransport, OsEntropy},
    domain::pairing::AttemptId,
};
use nessa_protocol::pairing::{
    wire::{decode_reply, encode_request, NativePairingReply, NativePairingRequest},
    EnrollmentChannel,
};
use nessa_server::{
    app::dependencies::RuntimeDependencies,
    device_pairing::infrastructure::{
        Accepted, EnrollmentAccept, NativeConnectionFailure, NativeEnrollmentConnections,
        NativeEnrollmentListener,
    },
};
use std::{
    collections::VecDeque,
    io::{Error as IoError, ErrorKind},
    net::TcpStream,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{sync::oneshot, time::Instant};

/// OS codes the tests script, per platform.
#[cfg(unix)]
mod codes {
    /// Per-connection network errors accept(2) passes back.
    pub const CONNECTION_FAILED: &[i32] = &[libc::EPROTO, libc::EHOSTUNREACH];
    /// Resource exhaustion.
    pub const EXHAUSTED: &[i32] = &[libc::EMFILE, libc::ENFILE, libc::ENOBUFS, libc::ENOMEM];
    /// The listening socket is invalid.
    pub const LISTENER_INVALID: i32 = libc::EBADF;
}
#[cfg(windows)]
mod codes {
    pub const CONNECTION_FAILED: &[i32] = &[];
    /// WSAEMFILE, WSAENOBUFS.
    pub const EXHAUSTED: &[i32] = &[10024, 10055];
    /// WSAEBADF.
    pub const LISTENER_INVALID: i32 = 10009;
}
/// A code no platform uses.
const UNRECOGNISED: i32 = 99_999;

/// One step of a scripted accept port.
enum Step {
    /// The next accept returns this.
    Give(Accepted),
    /// Wait for this signal before going on.
    Await(oneshot::Receiver<()>),
}
/// Hands out a fixed sequence of accept results, then waits forever. Records
/// when each accept was asked for.
struct ScriptedAccept {
    script: VecDeque<Step>,
    asked_at: Arc<Mutex<Vec<Instant>>>,
}
impl ScriptedAccept {
    fn new(script: impl IntoIterator<Item = Step>) -> (Self, Arc<Mutex<Vec<Instant>>>) {
        let asked_at = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                script: script.into_iter().collect(),
                asked_at: asked_at.clone(),
            },
            asked_at,
        )
    }
}
impl EnrollmentAccept for ScriptedAccept {
    async fn accept(&mut self) -> Accepted {
        self.asked_at.lock().unwrap().push(Instant::now());
        loop {
            match self.script.pop_front() {
                Some(Step::Give(accepted)) => return accepted,
                Some(Step::Await(signal)) => {
                    signal.await.ok();
                }
                None => std::future::pending::<()>().await,
            }
        }
    }
}

fn os_error(code: i32) -> Step {
    Step::Give(Accepted::AcceptFailed(IoError::from_raw_os_error(code)))
}
fn failed(kind: ErrorKind) -> Step {
    Step::Give(Accepted::AcceptFailed(IoError::from(kind)))
}

/// A device on its own OS thread: TLS, then Hello; reports whether the
/// gateway answered.
fn device_says_hello(stream: TcpStream, pin: [u8; 44]) -> oneshot::Receiver<bool> {
    let (answered, receiver) = oneshot::channel();
    std::thread::spawn(move || {
        let identity = NativeIdentity::generate(&mut OsEntropy).unwrap();
        let Ok(transport) = NativeTransport::connect(stream, &identity, GatewayTrust::Pinned(pin))
        else {
            answered.send(false).ok();
            return;
        };
        let mut channel = EnrollmentChannel::new(transport);
        let attempt = AttemptId::new([113; AttemptId::LENGTH]);
        channel
            .send_envelope(&encode_request(&NativePairingRequest::Hello(attempt)).unwrap())
            .unwrap();
        let reply = channel.receive_envelope().map(|bytes| decode_reply(&bytes));
        answered
            .send(matches!(reply, Ok(Ok(NativePairingReply::Hello(_)))))
            .ok();
    });
    receiver
}

fn connections(fixture: &Fixture) -> Arc<NativeEnrollmentConnections> {
    Arc::new(NativeEnrollmentConnections::new(
        fixture.gateway.clone(),
        RuntimeDependencies::default().clock,
    ))
}

/// Paused time: each pause advances the clock by exactly its length, so the
/// gaps between accepts are the backoff itself. A worker that finishes while
/// the last accept is waiting is joined first, and that accept is asked for
/// again at the same instant. Those re-entries are trailing `0ns` gaps; the
/// scripted backoff is the prefix.
#[tokio::test(start_paused = true)]
async fn native_listener_skips_connection_failures_and_backs_off_on_exhaustion() {
    let fixture = Fixture::new().await;
    fixture
        .gateway
        .create(fixture.session.clone(), ConsentClass::DeviceRead, OsEntropy)
        .await
        .unwrap();
    let pin = fixture.gateway.identity().public_spki();
    let (first_server, first_device) = sockets();
    let (second_server, second_device) = sockets();
    let ms = Duration::from_millis;
    let mut script = Vec::new();
    let mut gaps = Vec::new();
    // Per-connection failures are skipped: the next accept follows at once.
    let mut skipped = vec![
        failed(ErrorKind::ConnectionAborted),
        failed(ErrorKind::ConnectionReset),
        failed(ErrorKind::Interrupted),
        Step::Give(Accepted::ConnectionUnusable(IoError::from(
            ErrorKind::Other,
        ))),
    ];
    skipped.extend(codes::CONNECTION_FAILED.iter().map(|code| os_error(*code)));
    for step in skipped {
        script.push(step);
        gaps.push(ms(0));
    }
    // Exhaustion and anything unrecognised pause: 50 ms doubling to 1 s.
    let mut paused: Vec<Step> = codes::EXHAUSTED
        .iter()
        .map(|code| os_error(*code))
        .collect();
    paused.push(os_error(UNRECOGNISED));
    paused.push(failed(ErrorKind::Other));
    paused.push(failed(ErrorKind::InvalidInput));
    while paused.len() < 8 {
        paused.push(os_error(codes::EXHAUSTED[0]));
    }
    for (step, wait) in paused
        .into_iter()
        .zip([50, 100, 200, 400, 800, 1000, 1000, 1000])
    {
        script.push(step);
        gaps.push(ms(wait));
    }
    // An accepted socket that cannot be prepared does not reset the backoff.
    script.push(Step::Give(Accepted::ConnectionUnusable(IoError::from(
        ErrorKind::Other,
    ))));
    gaps.push(ms(0));
    script.push(os_error(codes::EXHAUSTED[0]));
    gaps.push(ms(1000));
    // A served connection resets it.
    script.push(Step::Give(Accepted::Connection(first_server)));
    gaps.push(ms(0));
    script.push(os_error(codes::EXHAUSTED[0]));
    gaps.push(ms(50));
    script.push(Step::Give(Accepted::Connection(second_server)));
    gaps.push(ms(0));
    let (accept, asked_at) = ScriptedAccept::new(script);
    let listener = NativeEnrollmentListener::new(accept, connections(&fixture));
    let (stop, stopped) = oneshot::channel::<()>();
    let running = tokio::spawn(listener.run(
        OsEntropy::default,
        async {
            stopped.await.ok();
        },
        |kind| panic!("listener stopped on {kind:?}"),
    ));
    // Both connections after the failures are served.
    assert!(device_says_hello(first_device, pin).await.unwrap());
    assert!(device_says_hello(second_device, pin).await.unwrap());
    stop.send(()).unwrap();
    running.await.unwrap().unwrap();
    let asked_at = asked_at.lock().unwrap().clone();
    let actual: Vec<Duration> = asked_at
        .windows(2)
        .map(|pair| pair[1].duration_since(pair[0]))
        .collect();
    assert!(
        actual.starts_with(&gaps),
        "accept gaps {actual:?} do not start with the backoff {gaps:?}"
    );
    assert!(
        actual[gaps.len()..].iter().all(Duration::is_zero),
        "only an instantaneous re-entry may follow the backoff, got {actual:?}"
    );
    fixture.gateway.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn native_listener_stop_ends_a_pause() {
    let fixture = Fixture::new().await;
    let (accept, asked_at) = ScriptedAccept::new([os_error(codes::EXHAUSTED[0])]);
    let listener = NativeEnrollmentListener::new(accept, connections(&fixture));
    let (stop, stopped) = oneshot::channel::<()>();
    let start = Instant::now();
    let running = tokio::spawn(listener.run(
        OsEntropy::default,
        async {
            stopped.await.ok();
        },
        |kind| panic!("listener stopped on {kind:?}"),
    ));
    // Stop 10 ms into the 50 ms pause.
    tokio::time::sleep(Duration::from_millis(10)).await;
    stop.send(()).unwrap();
    running.await.unwrap().unwrap();
    assert_eq!(start.elapsed(), Duration::from_millis(10));
    assert_eq!(asked_at.lock().unwrap().len(), 1);
    fixture.gateway.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_listener_stops_on_a_listening_socket_failure() {
    let store = std::sync::OnceLock::new();
    let fixture = Fixture::with_store(|registry| {
        let faulty = FaultyStore::new(registry);
        store.set(faulty.clone()).ok();
        faulty
    })
    .await;
    let store = store.get().unwrap().clone();
    fixture
        .gateway
        .create(fixture.session.clone(), ConsentClass::DeviceRead, OsEntropy)
        .await
        .unwrap();
    let pin = fixture.gateway.identity().public_spki();
    // An admitted worker parks in its Hello's store read; then the listening
    // socket fails.
    let (entered, release) = store.park_next_read();
    let (server, device) = sockets();
    let target = device.local_addr().unwrap();
    let answered = device_says_hello(device, pin);
    let connections = connections(&fixture);
    let (accept, _) = ScriptedAccept::new([
        Step::Give(Accepted::Connection(server)),
        Step::Await(entered),
        os_error(codes::LISTENER_INVALID),
    ]);
    let listener = NativeEnrollmentListener::new(accept, connections.clone());
    let reported = Arc::new(Mutex::new(None));
    let report = reported.clone();
    let parked = store.clone();
    let outcome = tokio::time::timeout(
        WAIT,
        listener.run(OsEntropy::default, std::future::pending(), move |kind| {
            // The admitted worker is still running: the drain has not happened.
            *report.lock().unwrap() = Some((kind, parked.parked()));
            release.send(()).ok();
        }),
    )
    .await
    .expect("a listening-socket failure ends the run");
    let expected = IoError::from_raw_os_error(codes::LISTENER_INVALID).kind();
    assert_eq!(outcome.unwrap_err().kind(), expected);
    assert_eq!(*reported.lock().unwrap(), Some((expected, true)));
    // The admitted connection was woken and drained before `run` returned.
    assert!(!store.parked());
    let woken: Vec<_> = connections
        .wake_report()
        .unwrap()
        .iter()
        .map(|outcome| outcome.target())
        .collect();
    assert_eq!(woken, [target]);
    answered.await.ok();
    // Admission is closed for good.
    let (late, _peer) = sockets();
    assert_eq!(
        connections
            .serve(late, OsEntropy)
            .await
            .unwrap_err()
            .failure,
        NativeConnectionFailure::Capacity
    );
    fixture.gateway.shutdown().await;
}

/// A connection the relay joined is served like a direct one, on a relay
/// permit of its own pool, in both directions (issue #266, slice R2a, row
/// RL16): with all eight connection permits held by silent direct peers, a
/// relayed device is still answered while a ninth direct one is refused;
/// with all four relay permits held, a relayed connection is refused while
/// the pools stay apart. The drain waits for every relay permit, even one
/// not yet in a connection.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_listener_serves_relayed_connections_on_their_own_permits() {
    let fixture = Fixture::new().await;
    // An open invitation, so a device's Hello is answered.
    fixture
        .gateway
        .create(fixture.session.clone(), ConsentClass::DeviceRead, OsEntropy)
        .await
        .unwrap();
    let pin = fixture.gateway.identity().public_spki();
    let connections = connections(&fixture);
    let relayed: Vec<_> = (0..4)
        .map(|_| connections.relay_permit().expect("a relay permit"))
        .collect();
    assert!(
        connections.relay_permit().is_none(),
        "the relay pool holds four"
    );
    let mut relayed = relayed.into_iter();
    // Eight direct peers that connect and never speak hold every
    // connection permit in TLS.
    let mut script = Vec::new();
    let mut silent = Vec::new();
    for _ in 0..8 {
        let (server, device) = sockets();
        script.push(Step::Give(Accepted::Connection(server)));
        silent.push(device);
    }
    let (ninth_server, ninth_device) = sockets();
    let ninth_answer = device_says_hello(ninth_device, pin);
    script.push(Step::Give(Accepted::Connection(ninth_server)));
    let (relayed_server, relayed_device) = sockets();
    let relayed_answer = device_says_hello(relayed_device, pin);
    script.push(Step::Give(Accepted::Relayed(
        relayed_server,
        relayed.next().unwrap(),
    )));
    let (accept, _) = ScriptedAccept::new(script);
    let listener = NativeEnrollmentListener::new(accept, connections.clone());
    let (stop, stopped) = oneshot::channel::<()>();
    let mut running = tokio::spawn(listener.run(
        OsEntropy::default,
        async {
            stopped.await.ok();
        },
        |kind| panic!("listener stopped on {kind:?}"),
    ));
    assert!(
        !tokio::time::timeout(WAIT, ninth_answer)
            .await
            .unwrap()
            .unwrap(),
        "a ninth direct peer finds every connection permit held"
    );
    assert!(
        tokio::time::timeout(WAIT, relayed_answer)
            .await
            .unwrap()
            .unwrap(),
        "a relayed device is answered on its relay permit"
    );
    stop.send(()).unwrap();
    // Three relay permits are still held, as by accepts under way: the
    // drain waits for them.
    assert!(
        tokio::time::timeout(Duration::from_millis(200), &mut running)
            .await
            .is_err(),
        "the drain waited for the relay permits still held"
    );
    drop(relayed);
    tokio::time::timeout(WAIT, running)
        .await
        .expect("the drain ends once every relay permit is back")
        .unwrap()
        .unwrap();
    drop(silent);
    fixture.gateway.shutdown().await;
}

/// Tunnel rows T1 and T3: a connection that came through the tunnel is
/// served on its own pool of four, and a key holding no credential is
/// closed right after TLS, before anything it sends is read. Pairing stays
/// on the local network: even with an invitation open, its Hello is never
/// answered, while the same Hello over a direct connection is.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tunneled_key_with_no_credential_is_closed_before_its_first_request() {
    let fixture = Fixture::new().await;
    fixture
        .gateway
        .create(fixture.session.clone(), ConsentClass::DeviceRead, OsEntropy)
        .await
        .unwrap();
    let pin = fixture.gateway.identity().public_spki();
    let connections = connections(&fixture);
    let tunneled: Vec<_> = (0..4)
        .map(|_| connections.tunnel_permit().expect("a tunnel permit"))
        .collect();
    assert!(
        connections.tunnel_permit().is_none(),
        "the tunnel pool holds four"
    );
    assert!(
        connections.relay_permit().is_some(),
        "apart from the relay pool"
    );
    let mut tunneled = tunneled.into_iter();
    let (tunnel_server, tunnel_device) = sockets();
    let tunnel_answer = device_says_hello(tunnel_device, pin);
    let (direct_server, direct_device) = sockets();
    let direct_answer = device_says_hello(direct_device, pin);
    let (accept, _) = ScriptedAccept::new([
        Step::Give(Accepted::Tunneled(tunnel_server, tunneled.next().unwrap())),
        Step::Give(Accepted::Connection(direct_server)),
    ]);
    let listener = NativeEnrollmentListener::new(accept, connections.clone());
    let (stop, stopped) = oneshot::channel::<()>();
    let running = tokio::spawn(listener.run(
        OsEntropy::default,
        async {
            stopped.await.ok();
        },
        |kind| panic!("listener stopped on {kind:?}"),
    ));
    assert!(
        !tokio::time::timeout(WAIT, tunnel_answer)
            .await
            .unwrap()
            .unwrap(),
        "an unpaired key through the tunnel is never answered"
    );
    assert!(
        tokio::time::timeout(WAIT, direct_answer)
            .await
            .unwrap()
            .unwrap(),
        "the same Hello on the local network is"
    );
    stop.send(()).unwrap();
    drop(tunneled);
    tokio::time::timeout(WAIT, running)
        .await
        .expect("the drain ends once every tunnel permit is back")
        .unwrap()
        .unwrap();
    fixture.gateway.shutdown().await;
}

/// Every key may serve: what a paired worker's or phone's key holds.
struct EveryKeyServes;
impl nessa_server::device_pairing::infrastructure::EnvironmentPeers for EveryKeyServes {
    fn admits<'a>(
        &'a self,
        _proof: &'a nessa_auth::application::pairing::DeviceConnectionProof,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<bool, nessa_auth::application::ports::AccessError>,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async { Ok(true) })
    }
    fn serve(
        &self,
        _stream: nessa_server::device_pairing::infrastructure::TlsStream,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        Box::pin(async {})
    }
}

#[tokio::test]
async fn a_credentialed_key_through_the_tunnel_cannot_pair() {
    let fixture = Fixture::new().await;
    fixture
        .gateway
        .create(fixture.session.clone(), ConsentClass::DeviceRead, OsEntropy)
        .await
        .unwrap();
    let pin = fixture.gateway.identity().public_spki();
    let connections = Arc::new(
        NativeEnrollmentConnections::new(
            fixture.gateway.clone(),
            RuntimeDependencies::default().clock,
        )
        .with_environment_peers(Arc::new(EveryKeyServes)),
    );
    let permit = connections.tunnel_permit().expect("a tunnel permit");
    let (tunnel_server, tunnel_device) = sockets();
    let tunnel_answer = device_says_hello(tunnel_device, pin);
    let (accept, _) = ScriptedAccept::new([Step::Give(Accepted::Tunneled(tunnel_server, permit))]);
    let listener = NativeEnrollmentListener::new(accept, connections.clone());
    let (stop, stopped) = oneshot::channel::<()>();
    let running = tokio::spawn(listener.run(
        OsEntropy::default,
        async {
            stopped.await.ok();
        },
        |kind| panic!("listener stopped on {kind:?}"),
    ));
    assert!(
        !tokio::time::timeout(WAIT, tunnel_answer)
            .await
            .unwrap()
            .unwrap(),
        "pairing, status and recovery stay on the local network, whatever the key holds"
    );
    stop.send(()).unwrap();
    tokio::time::timeout(WAIT, running)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    fixture.gateway.shutdown().await;
}
