//! A gateway serving through a relay, on in-memory streams and a manual
//! timer: against the real relay where the relay behaves, and against a
//! scripted one where it does not.
use super::*;
use crate::relay::{
    application::{Relay, RelayEntropy, RelayLimits, RelayProofs},
    infrastructure::Ed25519Proofs,
};
use crate::timer::manual::ManualTimer;
use nessa_auth::{adapters::pairing::NativeIdentity, application::pairing::PrivateKeyMaterial};
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use tokio::{
    io::{duplex, AsyncReadExt, AsyncWriteExt, DuplexStream},
    sync::{OwnedSemaphorePermit, Semaphore},
    task::JoinHandle,
};
use zeroize::Zeroizing;

/// How long, in real time, a test waits for something that should happen.
const REAL: Duration = Duration::from_secs(5);

fn identity(seed: u8) -> Arc<NativeIdentity> {
    Arc::new(NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([seed; 32]))).unwrap())
}

/// The gateway's key, or one that cannot make its proof.
struct Key {
    identity: Arc<NativeIdentity>,
    proves: bool,
}
impl RelayKey for Key {
    fn key(&self) -> GatewayKey {
        self.identity.public_key()
    }
    fn prove(&self, challenge: &[u8; 32]) -> Option<[u8; 64]> {
        self.proves
            .then(|| self.identity.sign_relay_challenge(challenge).unwrap())
    }
    fn sign_origin(&self, origin: &[u8]) -> Option<[u8; 64]> {
        self.proves
            .then(|| self.identity.sign_tunnel_origin(origin).unwrap())
    }
}

/// Relay permits from a pool of `size`.
struct Permits(Arc<Semaphore>);
impl RelayAdmission<OwnedSemaphorePermit> for Permits {
    fn admit(&self) -> Option<OwnedSemaphorePermit> {
        self.0.clone().try_acquire_owned().ok()
    }
}

/// Dials the real relay, in memory.
struct ToRelay(Arc<Relay<DuplexStream>>);
impl RelayDial<DuplexStream> for ToRelay {
    fn dial(&self) -> PortFuture<'_, io::Result<DuplexStream>> {
        let (gateway, relay) = duplex(64 * 1024);
        let permit = self.0.admit().expect("under the relay's limit");
        tokio::spawn(self.0.clone().handle(relay, permit));
        Box::pin(async move { Ok(gateway) })
    }
}

/// Hands the test the relay's end of every connection the gateway dials,
/// after failing the first `failing` dials with `InvalidInput`, as a host
/// name that resolves to nothing does.
struct Scripted {
    opened: mpsc::UnboundedSender<DuplexStream>,
    dials: Arc<AtomicUsize>,
    failing: usize,
}
impl RelayDial<DuplexStream> for Scripted {
    fn dial(&self) -> PortFuture<'_, io::Result<DuplexStream>> {
        if self.dials.fetch_add(1, Ordering::SeqCst) < self.failing {
            return Box::pin(async { Err(io::ErrorKind::InvalidInput.into()) });
        }
        let (gateway, relay) = duplex(64 * 1024);
        let _ = self.opened.send(relay);
        Box::pin(async move { Ok(gateway) })
    }
}

#[derive(Default)]
struct Counting(AtomicU8);
impl RelayEntropy for Counting {
    fn fill(&self, bytes: &mut [u8]) {
        let next = self.0.fetch_add(1, Ordering::SeqCst).wrapping_add(1);
        bytes.fill(next);
    }
}

struct Serving {
    timer: Arc<ManualTimer>,
    joined: mpsc::Receiver<Relayed<DuplexStream, OwnedSemaphorePermit>>,
    stop: watch::Sender<bool>,
    task: JoinHandle<()>,
    pool: Arc<Semaphore>,
}

fn serve(
    dial: Arc<dyn RelayDial<DuplexStream>>,
    key: Key,
    timer: Arc<ManualTimer>,
    pool: usize,
) -> Serving {
    let (joined, receiving) = mpsc::channel(4);
    let pool = Arc::new(Semaphore::new(pool));
    let (stop, stopped) = watch::channel(false);
    let (origin, address) = watch::channel(None);
    let through = ThroughRelay {
        dial,
        key: Arc::new(key),
        admission: Arc::new(Permits(pool.clone())),
        timer: timer.clone(),
        joined,
        address,
        secrets: Arc::new(crate::tunnel::application::MemoryDeviceSecrets::new()),
    };
    // The sender stays so the registration does not see the watch close.
    std::mem::forget(origin);
    let task = tokio::spawn(through.serve(Backoff::default(), stopped));
    Serving {
        timer,
        joined: receiving,
        stop,
        task,
        pool,
    }
}

/// What a scripted serving hands the test: the serving, the relay's end of
/// each dial, and how many dials there were.
type ScriptedServing = (
    Serving,
    mpsc::UnboundedReceiver<DuplexStream>,
    Arc<AtomicUsize>,
);

fn scripted(key: Key, pool: usize) -> ScriptedServing {
    scripted_failing(key, pool, 0)
}

fn scripted_failing(key: Key, pool: usize, failing: usize) -> ScriptedServing {
    let (opened, relay) = mpsc::unbounded_channel();
    let dials = Arc::new(AtomicUsize::new(0));
    let dial = Arc::new(Scripted {
        opened,
        dials: dials.clone(),
        failing,
    });
    (
        serve(dial, key, Arc::new(ManualTimer::default()), pool),
        relay,
        dials,
    )
}

fn key(proves: bool) -> Key {
    Key {
        identity: identity(1),
        proves,
    }
}

async fn send(stream: &mut DuplexStream, frame: RelayFrame) {
    stream.write_all(&frame.encode()).await.unwrap();
}

async fn receive(stream: &mut DuplexStream) -> RelayFrame {
    tokio::time::timeout(REAL, read_frame(stream))
        .await
        .expect("a frame in time")
        .expect("a frame")
}

/// The next connection the gateway dialed, from the relay's side.
async fn opened(relay: &mut mpsc::UnboundedReceiver<DuplexStream>) -> DuplexStream {
    tokio::time::timeout(REAL, relay.recv())
        .await
        .expect("a dial in time")
        .expect("the gateway still dials")
}

/// Play the relay's side of a registration on `stream`, with the real
/// proof checked.
async fn registered(stream: &mut DuplexStream, identity: &NativeIdentity) {
    assert_eq!(
        receive(stream).await,
        RelayFrame::Serve {
            key: identity.public_key()
        }
    );
    let nonce = [7; 32];
    send(stream, RelayFrame::Challenge { nonce }).await;
    let RelayFrame::Proof { signature } = receive(stream).await else {
        panic!("no proof");
    };
    assert!(Ed25519Proofs.verify(&identity.public_key(), &nonce, &signature));
    send(stream, RelayFrame::Registered).await;
}

/// The gateway closed this connection: it reads to its end.
async fn closed(stream: &mut DuplexStream) {
    let mut rest = Vec::new();
    tokio::time::timeout(REAL, stream.read_to_end(&mut rest))
        .await
        .expect("closed in time")
        .expect("read to the end");
    assert!(rest.is_empty(), "nothing more was sent: {rest:?}");
}

/// A gateway registered with the real relay is joined to a reader asking
/// for its key; the joined connection reaches the listener with a relay
/// permit, and bytes cross both ways.
#[tokio::test]
async fn a_reader_the_relay_announces_reaches_the_listener_on_a_relay_permit() {
    let timer = Arc::new(ManualTimer::default());
    let relay = Relay::new(
        timer.clone(),
        Arc::new(Counting::default()),
        Arc::new(Ed25519Proofs),
        RelayLimits::default(),
    );
    let gateway = identity(1);
    let mut serving = serve(
        Arc::new(ToRelay(relay.clone())),
        Key {
            identity: gateway.clone(),
            proves: true,
        },
        timer,
        4,
    );
    // A reader asking before the registration lands is told Offline; ask
    // until it is joined.
    let mut reader = loop {
        let mut reader = ToRelay(relay.clone()).dial().await.unwrap();
        send(
            &mut reader,
            RelayFrame::Connect {
                key: gateway.public_key(),
            },
        )
        .await;
        match receive(&mut reader).await {
            RelayFrame::Joined => break reader,
            RelayFrame::Offline => tokio::task::yield_now().await,
            other => panic!("unexpected {other:?}"),
        }
    };
    let Relayed { mut stream, permit } = tokio::time::timeout(REAL, serving.joined.recv())
        .await
        .expect("joined in time")
        .expect("a joined reader");
    assert_eq!(
        serving.pool.available_permits(),
        3,
        "it holds a relay permit"
    );
    reader.write_all(b"hello gateway").await.unwrap();
    let mut heard = [0; 13];
    stream.read_exact(&mut heard).await.unwrap();
    assert_eq!(&heard, b"hello gateway");
    stream.write_all(b"hello reader").await.unwrap();
    let mut heard = [0; 12];
    reader.read_exact(&mut heard).await.unwrap();
    assert_eq!(&heard, b"hello reader");
    drop(permit);
    assert_eq!(serving.pool.available_permits(), 4);
    serving.stop.send_replace(true);
    tokio::time::timeout(REAL, serving.task)
        .await
        .unwrap()
        .unwrap();
}

/// RL16: pings are answered, and an announced reader with no relay permit
/// free is not dialed for: the registration goes on.
#[tokio::test]
async fn pings_are_answered_and_a_reader_past_the_pool_is_not_accepted() {
    let (serving, mut relay, dials) = scripted(key(true), 0);
    let mut registration = opened(&mut relay).await;
    registered(&mut registration, &identity(1)).await;
    send(&mut registration, RelayFrame::Incoming { token: [9; 16] }).await;
    send(&mut registration, RelayFrame::Ping).await;
    assert_eq!(receive(&mut registration).await, RelayFrame::Pong);
    assert_eq!(dials.load(Ordering::SeqCst), 1, "no accept leg was dialed");
    serving.stop.send_replace(true);
    tokio::time::timeout(REAL, serving.task)
        .await
        .unwrap()
        .unwrap();
    closed(&mut registration).await;
}

/// An accept leg sends `Accept` with its token on a new connection, and
/// hands the stream on only after `Joined`; bytes after it are the
/// reader's, untouched.
#[tokio::test]
async fn an_accept_leg_hands_on_its_stream_only_after_joined() {
    let (mut serving, mut relay, _) = scripted(key(true), 1);
    let mut registration = opened(&mut relay).await;
    registered(&mut registration, &identity(1)).await;
    send(&mut registration, RelayFrame::Incoming { token: [9; 16] }).await;
    let mut leg = opened(&mut relay).await;
    assert_eq!(
        receive(&mut leg).await,
        RelayFrame::Accept {
            key: identity(1).public_key(),
            token: [9; 16]
        }
    );
    assert!(serving.joined.try_recv().is_err(), "nothing before Joined");
    leg.write_all(&RelayFrame::Joined.encode()).await.unwrap();
    leg.write_all(b"tls").await.unwrap();
    let Relayed { mut stream, .. } = tokio::time::timeout(REAL, serving.joined.recv())
        .await
        .unwrap()
        .unwrap();
    let mut first = [0; 3];
    stream.read_exact(&mut first).await.unwrap();
    assert_eq!(&first, b"tls");
    serving.stop.send_replace(true);
    tokio::time::timeout(REAL, serving.task)
        .await
        .unwrap()
        .unwrap();
}

/// An accept leg the relay never joins gives its permit back at the join
/// deadline.
#[tokio::test]
async fn an_accept_leg_never_joined_gives_its_permit_back_at_the_deadline() {
    let (serving, mut relay, _) = scripted(key(true), 1);
    let mut registration = opened(&mut relay).await;
    registered(&mut registration, &identity(1)).await;
    send(&mut registration, RelayFrame::Incoming { token: [9; 16] }).await;
    let mut leg = opened(&mut relay).await;
    receive(&mut leg).await;
    assert_eq!(serving.pool.available_permits(), 0);
    let wait = serving
        .timer
        .started(|_, wait| wait.limit() == JOIN_DEADLINE)
        .await;
    serving.timer.advance_to(wait.deadline);
    closed(&mut leg).await;
    assert_eq!(serving.pool.available_permits(), 1);
    serving.stop.send_replace(true);
    tokio::time::timeout(REAL, serving.task)
        .await
        .unwrap()
        .unwrap();
}

/// RL15: a registration that hears nothing for two ping intervals is
/// dropped and made again after the first backoff wait.
#[tokio::test]
async fn a_silent_registration_is_dropped_and_made_again() {
    let (serving, mut relay, dials) = scripted(key(true), 1);
    let mut registration = opened(&mut relay).await;
    registered(&mut registration, &identity(1)).await;
    let silent = serving
        .timer
        .started(|_, wait| wait.limit() == SILENCE)
        .await;
    serving.timer.advance_to(silent.deadline);
    closed(&mut registration).await;
    let pause = serving
        .timer
        .started(|_, wait| {
            wait.started == silent.deadline && wait.limit() == Backoff::default().first
        })
        .await;
    assert_eq!(dials.load(Ordering::SeqCst), 1, "not before the wait");
    serving.timer.advance_to(pause.deadline);
    let mut again = opened(&mut relay).await;
    registered(&mut again, &identity(1)).await;
    serving.stop.send_replace(true);
    tokio::time::timeout(REAL, serving.task)
        .await
        .unwrap()
        .unwrap();
}

/// RL15: a relay answering the registration with anything but its preamble
/// is dialed again after a wait that doubles each time.
#[tokio::test]
async fn a_refused_registration_is_tried_again_after_a_doubling_wait() {
    let (serving, mut relay, _) = scripted(key(true), 1);
    let first = Backoff::default().first;
    for wait in [first, first * 2, first * 4] {
        let mut registration = opened(&mut relay).await;
        receive(&mut registration).await;
        send(&mut registration, RelayFrame::NoAnswer).await;
        closed(&mut registration).await;
        let pause = serving
            .timer
            .started(|_, found| found.limit() == wait)
            .await;
        serving.timer.advance_to(pause.deadline);
    }
    opened(&mut relay).await;
    serving.stop.send_replace(true);
    tokio::time::timeout(REAL, serving.task)
        .await
        .unwrap()
        .unwrap();
}

/// RL17: a key that cannot make its proof stops serving through the relay
/// at once: nothing is dialed again.
#[tokio::test]
async fn a_key_that_cannot_prove_stops_registering() {
    let (serving, mut relay, dials) = scripted(key(false), 1);
    let mut registration = opened(&mut relay).await;
    receive(&mut registration).await;
    send(&mut registration, RelayFrame::Challenge { nonce: [7; 32] }).await;
    tokio::time::timeout(REAL, serving.task)
        .await
        .expect("serving ended by itself")
        .unwrap();
    closed(&mut registration).await;
    assert_eq!(dials.load(Ordering::SeqCst), 1);
}

/// Stop ends serving mid-registration, and with it every accept leg and the
/// relay permit it holds.
#[tokio::test]
async fn stop_ends_the_registration_and_its_accept_legs() {
    let (serving, mut relay, _) = scripted(key(true), 1);
    let mut registration = opened(&mut relay).await;
    registered(&mut registration, &identity(1)).await;
    send(&mut registration, RelayFrame::Incoming { token: [9; 16] }).await;
    let mut leg = opened(&mut relay).await;
    receive(&mut leg).await;
    assert_eq!(serving.pool.available_permits(), 0);
    serving.stop.send_replace(true);
    tokio::time::timeout(REAL, serving.task)
        .await
        .unwrap()
        .unwrap();
    closed(&mut registration).await;
    closed(&mut leg).await;
    assert_eq!(serving.pool.available_permits(), 1);
}

/// RL15: a dial that fails with `InvalidInput`, as a host name resolving to
/// nothing does, is a failure like any other and is tried again; only the
/// key's own proof stops registering.
#[tokio::test]
async fn a_dial_refused_as_invalid_input_is_tried_again() {
    let (serving, mut relay, dials) = scripted_failing(key(true), 1, 1);
    let pause = serving
        .timer
        .started(|_, wait| wait.limit() == Backoff::default().first)
        .await;
    assert_eq!(dials.load(Ordering::SeqCst), 1);
    serving.timer.advance_to(pause.deadline);
    let mut registration = opened(&mut relay).await;
    registered(&mut registration, &identity(1)).await;
    serving.stop.send_replace(true);
    tokio::time::timeout(REAL, serving.task)
        .await
        .unwrap()
        .unwrap();
}

/// RL15: registrations dropped at once wait 1 s, then 2 s; one that lasted
/// the longest wait starts the waits over at 1 s.
#[tokio::test]
async fn a_lasting_registration_starts_the_waits_over() {
    let (serving, mut relay, _) = scripted(key(true), 1);
    let timer = serving.timer.clone();
    let first = Backoff::default().first;
    let most = Backoff::default().most;
    // Waits begun at `at` for `limit`: the backoff, never a deadline.
    let pause_from = |at: crate::timer::Moment, limit: Duration| {
        let timer = timer.clone();
        async move {
            timer
                .started(move |_, wait| wait.started == at && wait.limit() == limit)
                .await
        }
    };
    for wait in [first, first * 2] {
        let mut registration = opened(&mut relay).await;
        registered(&mut registration, &identity(1)).await;
        let dropped_at = timer.now();
        drop(registration);
        let pause = pause_from(dropped_at, wait).await;
        timer.advance_to(pause.deadline);
    }
    let mut registration = opened(&mut relay).await;
    registered(&mut registration, &identity(1)).await;
    // A first ping answered: the gateway has taken the registration, so
    // the time it lasts is counted from here.
    send(&mut registration, RelayFrame::Ping).await;
    assert_eq!(receive(&mut registration).await, RelayFrame::Pong);
    // Kept alive with a ping every interval, until it has lasted `most`.
    let since = timer.now();
    while timer.now().since(since) < most {
        timer.advance_to(timer.now() + PING_INTERVAL);
        send(&mut registration, RelayFrame::Ping).await;
        assert_eq!(receive(&mut registration).await, RelayFrame::Pong);
    }
    let dropped_at = timer.now();
    drop(registration);
    let pause = pause_from(dropped_at, first).await;
    assert_eq!(pause.limit(), first);
    serving.stop.send_replace(true);
    tokio::time::timeout(REAL, serving.task)
        .await
        .unwrap()
        .unwrap();
}

/// An away phone, whose saved tunnel address no longer answers, asks the
/// relay the gateway is registered on and is told the address it publishes
/// now. A stranger who does not hold a paired device's secret is not.
#[tokio::test]
async fn an_away_phone_is_told_the_new_tunnel_address_through_the_relay() {
    use crate::relay::gateway::infrastructure::TcpRelayDial;
    use crate::relay::infrastructure::serve_tcp;
    use crate::timer::TokioTimer;
    use crate::tunnel::application::MemoryDeviceSecrets;
    use nessa_client_core::tunnel::published_address;
    use nessa_protocol::relay::{PublishedOrigin, RelayAddress};
    use nessa_protocol::tunnel::{TunnelAddress, TunnelSecret};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let relay_address = RelayAddress::try_from(format!("127.0.0.1:{port}")).unwrap();
    let timer = Arc::new(TokioTimer::new());
    let service = Relay::new(
        timer.clone(),
        Arc::new(Counting::default()),
        Arc::new(Ed25519Proofs),
        RelayLimits::default(),
    );
    let (relay_stop, relay_stopped) = tokio::sync::oneshot::channel::<()>();
    let relay_task = tokio::spawn(serve_tcp(listener, service, timer.clone(), async move {
        let _ = relay_stopped.await;
    }));
    let secrets = Arc::new(MemoryDeviceSecrets::new());
    let secret_bytes = secrets.issue("phone", &[4; 32], "owner").unwrap();
    let secret = TunnelSecret::new(secret_bytes);
    let gateway = identity(4);
    let first = PublishedOrigin::new("https://old.trycloudflare.com").unwrap();
    let second = PublishedOrigin::new("https://new.trycloudflare.com").unwrap();
    let (publish, address) = watch::channel(Some(first));
    let (joined, _receiving) = mpsc::channel(4);
    let (stop, stopped) = watch::channel(false);
    let through = ThroughRelay {
        dial: Arc::new(TcpRelayDial(relay_address.as_str().to_owned())),
        key: Arc::new(Key {
            identity: gateway.clone(),
            proves: true,
        }),
        admission: Arc::new(Permits(Arc::new(Semaphore::new(4)))),
        timer,
        joined,
        address,
        secrets,
    };
    let task = tokio::spawn(through.serve(Backoff::default(), stopped));
    let key = gateway.public_key();
    let saved = TunnelAddress::try_from("https://old.trycloudflare.com".to_owned()).unwrap();
    let mut heard = None;
    let mut last_error = None;
    for _ in 0..40 {
        let (relay_address, key, secret) = (relay_address.clone(), key, secret);
        match tokio::task::spawn_blocking(move || {
            published_address(&relay_address, &key, &secret, Duration::from_secs(1))
        })
        .await
        .unwrap()
        {
            Ok(origin) => {
                heard = origin;
                break;
            }
            Err(error) => {
                last_error = Some(error);
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
    assert_eq!(
        heard.as_ref(),
        Some(&saved),
        "the phone is told the published address, last error: {last_error:?}"
    );
    publish.send_replace(Some(second));
    let mut heard = None;
    for _ in 0..40 {
        let (relay_address, key, secret) = (relay_address.clone(), key, secret);
        match tokio::task::spawn_blocking(move || {
            published_address(&relay_address, &key, &secret, Duration::from_secs(1))
        })
        .await
        .unwrap()
        {
            Ok(Some(origin)) if origin.as_str() == "https://new.trycloudflare.com" => {
                heard = Some(origin);
                break;
            }
            _ => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }
    let heard = heard.expect("the away phone is told the new address");
    assert_ne!(heard, saved);
    let stranger = TunnelSecret::new([0; 32]);
    let (relay_address, key) = (relay_address.clone(), key);
    let refused = tokio::task::spawn_blocking(move || {
        published_address(&relay_address, &key, &stranger, Duration::from_secs(2))
    })
    .await
    .unwrap();
    assert!(
        refused.is_err(),
        "a stranger is not told the address: {refused:?}"
    );
    stop.send_replace(true);
    tokio::time::timeout(REAL, task).await.unwrap().unwrap();
    drop(relay_stop);
    tokio::time::timeout(REAL, relay_task)
        .await
        .unwrap()
        .unwrap();
}
