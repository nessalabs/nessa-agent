//! The listener's accept policy, driven through a scripted accept port.
use super::support::{sockets, FaultyStore, Fixture, WAIT};
use nessa_auth::{
    adapters::pairing::{GatewayTrust, NativeIdentity, NativeTransport, OsEntropy},
    domain::pairing::AttemptId,
};
use nessa_server::{
    app::dependencies::RuntimeDependencies,
    device_pairing::infrastructure::{
        wire::{decode_reply, encode_request, NativePairingReply, NativePairingRequest},
        Accepted, EnrollmentAccept, EnrollmentChannel, NativeConnectionFailure,
        NativeEnrollmentConnections, NativeEnrollmentListener,
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
/// gaps between accepts are the backoff itself.
#[tokio::test(start_paused = true)]
async fn native_listener_skips_connection_failures_and_backs_off_on_exhaustion() {
    let fixture = Fixture::new().await;
    fixture
        .gateway
        .create(fixture.session.clone(), OsEntropy)
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
    assert_eq!(actual, gaps);
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
        .create(fixture.session.clone(), OsEntropy)
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
