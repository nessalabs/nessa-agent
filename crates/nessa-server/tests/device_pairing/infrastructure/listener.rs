//! The listener's accept policy, driven through a scripted accept port.
use super::support::{sockets, Fixture, WAIT};
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

/// Hands out a fixed sequence of accept results, then waits forever. Records
/// when each accept was asked for.
struct ScriptedAccept {
    script: VecDeque<Accepted>,
    asked_at: Arc<Mutex<Vec<Instant>>>,
}
impl EnrollmentAccept for ScriptedAccept {
    async fn accept(&mut self) -> Accepted {
        self.asked_at.lock().unwrap().push(Instant::now());
        match self.script.pop_front() {
            Some(accepted) => accepted,
            None => std::future::pending().await,
        }
    }
}

fn os_error(code: i32) -> Accepted {
    Accepted::AcceptFailed(IoError::from_raw_os_error(code))
}
fn failed(kind: ErrorKind) -> Accepted {
    Accepted::AcceptFailed(IoError::from(kind))
}

#[cfg(unix)]
const OUT_OF_DESCRIPTORS: [i32; 2] = [libc::EMFILE, libc::ENFILE];
#[cfg(windows)]
const OUT_OF_DESCRIPTORS: [i32; 2] = [10024, 10024];

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
    let [emfile, enfile] = OUT_OF_DESCRIPTORS;
    let mut script = VecDeque::from([
        failed(ErrorKind::ConnectionAborted),
        failed(ErrorKind::ConnectionReset),
        failed(ErrorKind::Interrupted),
        Accepted::ConnectionUnusable(IoError::from(ErrorKind::Other)),
    ]);
    for exhausted in 0..8 {
        script.push_back(os_error(if exhausted % 2 == 0 { emfile } else { enfile }));
    }
    script.push_back(Accepted::Connection(first_server));
    script.push_back(os_error(emfile));
    script.push_back(Accepted::Connection(second_server));
    let asked_at = Arc::new(Mutex::new(Vec::new()));
    let connections = Arc::new(NativeEnrollmentConnections::new(
        fixture.gateway.clone(),
        RuntimeDependencies::default().clock,
    ));
    let listener = NativeEnrollmentListener::new(
        ScriptedAccept {
            script,
            asked_at: asked_at.clone(),
        },
        connections.clone(),
    );
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
    let gaps: Vec<Duration> = asked_at
        .windows(2)
        .map(|pair| pair[1].duration_since(pair[0]))
        .collect();
    let ms = Duration::from_millis;
    assert_eq!(
        gaps,
        [
            // Four per-connection failures: no pause.
            ms(0),
            ms(0),
            ms(0),
            ms(0),
            // Eight exhaustion failures: 50 ms doubling to at most 1 s.
            ms(50),
            ms(100),
            ms(200),
            ms(400),
            ms(800),
            ms(1000),
            ms(1000),
            ms(1000),
            // A served connection resets the backoff.
            ms(0),
            ms(50),
            ms(0),
        ]
    );
    fixture.gateway.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_listener_stops_on_a_listening_socket_failure() {
    let fixture = Fixture::new().await;
    fixture
        .gateway
        .create(fixture.session.clone(), OsEntropy)
        .await
        .unwrap();
    // The device never speaks, so its worker is blocked in the TLS read until
    // shutdown wakes it.
    let (server, device) = sockets();
    let target = device.local_addr().unwrap();
    let connections = Arc::new(NativeEnrollmentConnections::new(
        fixture.gateway.clone(),
        RuntimeDependencies::default().clock,
    ));
    // A connection in flight, then the listening socket fails.
    let listener = NativeEnrollmentListener::new(
        ScriptedAccept {
            script: VecDeque::from([
                Accepted::Connection(server),
                failed(ErrorKind::InvalidInput),
            ]),
            asked_at: Arc::new(Mutex::new(Vec::new())),
        },
        connections.clone(),
    );
    let reported = Arc::new(Mutex::new(None));
    let report = reported.clone();
    let outcome = tokio::time::timeout(
        WAIT,
        listener.run(OsEntropy::default, std::future::pending(), move |kind| {
            *report.lock().unwrap() = Some(kind);
        }),
    )
    .await
    .expect("a listening-socket failure ends the run");
    assert_eq!(outcome.unwrap_err().kind(), ErrorKind::InvalidInput);
    assert_eq!(*reported.lock().unwrap(), Some(ErrorKind::InvalidInput));
    // The in-flight connection was woken and drained before `run` returned.
    let woken: Vec<_> = connections
        .wake_report()
        .unwrap()
        .iter()
        .map(|outcome| outcome.target())
        .collect();
    assert_eq!(woken, [target]);
    drop(device);
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
