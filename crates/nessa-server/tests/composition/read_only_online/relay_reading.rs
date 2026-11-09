//! Issue #266, slice R2c: a retained read through the gateway's relay, end
//! to end, against a real gateway process registered at a real relay
//! (`docs/design/auth/relay.md`, rows RL9 to RL11b).
//!
//! ```text
//! reader ── direct dial refused ──▶ (nothing)
//!        ── Connect{K} ──▶ relay ── Incoming ──▶ gateway ── Accept ──▶ relay
//!        ◀───────────── Joined, then the gateway's own TLS ─────────────▶
//! ```
//! `completion` owns the finite multi-pass fixture allowance; the cache owns
//! every catalogue/record continuation checkpoint. Tampered reads stay single-shot.
#[path = "relay_reading/completion.rs"]
mod completion;
#[path = "relay_reading/transport.rs"]
mod transport;
use super::fixtures::Gateway;
use crate::device_pairing::infrastructure::RELAY_CAPACITY;
use nessa_auth::{
    adapters::pairing::{relay_key, FilePairingState, NativeIdentity},
    application::{pairing::ClientPendingStore, ports::Clock as WallClock},
};
use nessa_client_core::retained::{
    ReadFailure, ReadReport, ReadStop, ReaderAccess, ReaderRole, RetainedCache,
    RetainedEnrollmentClient, RetainedTranscript,
};
use nessa_protocol::{
    clock::Clock as MonotonicClock,
    conversation::domain::ConversationId,
    pairing::wire::NativePairingStatus,
    relay::{RelayAddress, RelayFrame},
};
use serde_json::Value;
use std::{
    io::{Read, Write},
    net::{Shutdown, SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use tokio::runtime::Builder;
use uuid::Uuid;

struct Time(Instant);
impl MonotonicClock for Time {
    fn elapsed_ms(&self) -> u64 {
        u64::try_from(self.0.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}
struct Wall;
impl WallClock for Wall {
    fn unix_milliseconds(&self) -> u64 {
        123_000
    }
}

/// Port 0 is never a listener's: a direct dial to it is refused at once.
const REFUSED: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 0);

/// The fixture's paired device, as the gateway's first start left it: what
/// a retained read needs, and where the gateway and its relay listen.
struct Device {
    root: PathBuf,
    caches: PathBuf,
    address: SocketAddr,
    identity: NativeIdentity,
    pin: [u8; 44],
    credential: String,
    receiver: String,
    access_epoch: u64,
}
impl Device {
    /// Start the fixture's gateway in `mode`, relayed, under `directory`,
    /// and read its paired device's binding with a pinned status.
    fn start(directory: &Path, mode: &str) -> (Gateway, Self) {
        Self::start_with(directory, |root| Gateway::start_relayed(root, mode))
    }
    /// The same, with the gateway `start` starts under its root.
    fn start_with(directory: &Path, start: impl FnOnce(&Path) -> Gateway) -> (Gateway, Self) {
        let root = directory.join("gateway");
        let gateway = start(&root);
        let profile: Value =
            serde_json::from_slice(&std::fs::read(root.join("profile.json")).unwrap()).unwrap();
        let address: SocketAddr = profile["gatewayAddress"].as_str().unwrap().parse().unwrap();
        let state = Arc::new(
            FilePairingState::open(
                Path::new(profile["stateRoot"].as_str().unwrap()),
                Path::new(profile["stateDirectory"].as_str().unwrap()),
            )
            .unwrap(),
        );
        let caches = directory.join("cache");
        nessa_local_storage::create_directory(&caches).unwrap();
        let runtime = Builder::new_current_thread().enable_all().build().unwrap();
        let client = RetainedEnrollmentClient::new(
            state.clone(),
            caches.join("status.sqlite3"),
            Arc::new(Wall),
            Arc::new(Time(Instant::now())),
        )
        .unwrap();
        let status = runtime
            .block_on(client.status(TcpStream::connect(address).unwrap(), None))
            .unwrap();
        let NativePairingStatus::Active {
            receiver,
            access_epoch,
            ..
        } = status.enrollment().as_ref().unwrap()
        else {
            panic!("paired device must be active")
        };
        let (receiver, access_epoch) = (receiver.as_str().to_owned(), *access_epoch);
        runtime.block_on(client.shutdown());
        let saved = state.load_credential().unwrap().unwrap();
        let credential = saved.credential().as_str().to_owned();
        let (key, pin, _) = saved.into_enrollment().into_parts();
        let device = Self {
            root,
            caches,
            address,
            identity: NativeIdentity::restore(key).unwrap(),
            pin,
            credential,
            receiver,
            access_epoch,
        };
        (gateway, device)
    }
    /// The relay the gateway's current process serves and is registered at.
    fn relay(&self) -> RelayAddress {
        RelayAddress::try_from(std::fs::read_to_string(self.root.join("relay-address")).unwrap())
            .unwrap()
    }
    fn cache(&self, name: &str) -> RetainedCache {
        RetainedCache::open(
            &self.caches.join(name),
            Arc::new(Wall),
            Arc::new(Time(Instant::now())),
        )
        .unwrap()
    }
    fn read(
        &self,
        cache: &mut RetainedCache,
        address: SocketAddr,
        relay: Option<&RelayAddress>,
    ) -> Result<ReadReport, ReadFailure> {
        self.read_for(cache, address, relay, Duration::from_secs(5), 8)
    }
    fn read_for(
        &self,
        cache: &mut RetainedCache,
        address: SocketAddr,
        relay: Option<&RelayAddress>,
        budget: Duration,
        max_withdrawn: usize,
    ) -> Result<ReadReport, ReadFailure> {
        cache.read(
            ReaderAccess {
                role: ReaderRole::DeviceOwner,
                address,
                relay,
                identity: &self.identity,
                pin: self.pin,
                credential: &self.credential,
                receiver: &self.receiver,
                access_epoch: self.access_epoch,
                client_id: "relay-reading-test",
            },
            &ReadStop::new(),
            budget,
            max_withdrawn,
            &mut Vec::new(),
        )
    }
    /// A read whose direct dial is refused, retried while the gateway has
    /// not yet registered at `relay`: until it does, the relay answers
    /// `Offline`, which the read reports as `Away`, or the relay is not yet
    /// listening, which it reports as `Unreachable`.
    fn read_once_registered(
        &self,
        cache: &mut RetainedCache,
        relay: &RelayAddress,
    ) -> Result<ReadReport, ReadFailure> {
        let registered = Instant::now() + Duration::from_secs(20);
        loop {
            match self.read(cache, REFUSED, Some(relay)) {
                Err(ReadFailure::Unreachable | ReadFailure::Away)
                    if Instant::now() < registered =>
                {
                    thread::sleep(Duration::from_millis(100));
                }
                read => return read,
            }
        }
    }
    /// Wait until the gateway is registered at its relay: a raw `Connect`
    /// for its key is joined rather than answered `Offline`.
    fn wait_registered(&self) {
        let relay = self.relay();
        let key = relay_key(&self.pin).unwrap();
        let registered = Instant::now() + Duration::from_secs(20);
        loop {
            if ask(&relay, key).1 == RelayFrame::Joined {
                return;
            }
            assert!(Instant::now() < registered, "never registered at the relay");
            thread::sleep(Duration::from_millis(100));
        }
    }
}

/// Send `Connect{key}` to `relay` and read its three-byte answer; the
/// stream is returned open, so a joined one keeps its gateway connection.
fn ask(relay: &RelayAddress, key: [u8; 32]) -> (TcpStream, RelayFrame) {
    let mut stream = TcpStream::connect(relay.as_str()).unwrap();
    stream
        .write_all(&RelayFrame::Connect { key }.encode())
        .unwrap();
    let mut answer = [0; 3];
    stream.read_exact(&mut answer).unwrap();
    let frame = [
        RelayFrame::Joined,
        RelayFrame::Offline,
        RelayFrame::NoAnswer,
    ]
    .into_iter()
    .find(|frame| frame.encode() == answer)
    .expect("a reader's answer");
    (stream, frame)
}

/// RL9: a reader whose direct dial fails and whose relay joins it holds the
/// same protected session a direct reader does: it is admitted the same,
/// reads the same conversations, and leaves the same checkpoint, so a read
/// by the other path from that cache has nothing new to save.
#[test]
fn a_relayed_read_is_the_same_session_as_a_direct_one() {
    let directory = tempfile::tempdir().unwrap();
    let (_gateway, device) = Device::start(directory.path(), "");
    let relay = device.relay();
    device.wait_registered();

    let mut relayed = device.cache("relayed.sqlite3");
    // Force one committed-catalogue partial inside the same total allowance.
    // Later calls use the cache's saved checkpoints, never a new cache/cursor.
    let mut catalogue = None;
    let started = Instant::now();
    let report = completion::finish(|budget| {
        let first = catalogue.is_none();
        let report = device.read_for(
            &mut relayed,
            REFUSED,
            Some(&relay),
            budget,
            if first { 0 } else { 8 },
        )?;
        if first {
            assert!(report.moved && !report.complete);
            let saved = relayed.conversation_ids(&device.receiver).unwrap();
            assert!(saved.len() >= 2);
            catalogue = Some(saved);
        }
        Ok(report)
    })
    .unwrap();
    assert!(
        report.complete,
        "first relayed read after {:?}: {report:?}",
        started.elapsed()
    );
    assert!(report.conversations >= 2);
    assert_eq!(
        relayed.conversation_ids(&device.receiver).unwrap(),
        catalogue.unwrap()
    );

    let mut direct = device.cache("direct.sqlite3");
    assert!(
        completion::finish(|budget| {
            device.read_for(&mut direct, device.address, None, budget, 8)
        })
        .unwrap()
        .complete
    );
    assert_eq!(
        relayed.conversation_ids(&device.receiver).unwrap(),
        direct.conversation_ids(&device.receiver).unwrap()
    );
    // The same bytes, not only the same list: each conversation's saved
    // positions and its projected view agree.
    let saved = transcripts(&mut relayed, &device.receiver);
    assert!(saved.len() >= 2 && saved.iter().any(|(_, (downloaded, ..), _)| *downloaded > 0));
    assert_eq!(saved, transcripts(&mut direct, &device.receiver));

    let again = device.read(&mut relayed, device.address, None).unwrap();
    assert!(
        again.complete && !again.moved,
        "direct resumes the relayed checkpoint"
    );
    let again = device.read(&mut direct, REFUSED, Some(&relay)).unwrap();
    assert!(
        again.complete && !again.moved,
        "relayed resumes the direct checkpoint"
    );
}

/// Each conversation `cache` holds for `receiver`: its id, its saved
/// positions (downloaded, applied, facts) and its projected view.
fn transcripts(cache: &mut RetainedCache, receiver: &str) -> Vec<(String, (u64, u64, u64), Value)> {
    let scope = cache.saved_catalogue_scope(receiver).unwrap().unwrap();
    cache
        .conversation_ids(receiver)
        .unwrap()
        .into_iter()
        .map(|id| {
            let RetainedTranscript::Retained(snapshot) = cache
                .transcript_view(
                    receiver,
                    scope.origin().as_str(),
                    &ConversationId::new(&id).unwrap(),
                    Uuid::nil(),
                )
                .unwrap()
            else {
                panic!("{id} has no retained transcript")
            };
            let progress = snapshot.progress();
            (
                id,
                (progress.downloaded(), progress.applied(), progress.facts()),
                serde_json::to_value(snapshot.view()).unwrap(),
            )
        })
        .collect()
}

/// What a man-in-the-middle relay does to the session once it has joined
/// the reader.
#[derive(Clone, Copy, Debug)]
enum Tamper {
    /// Pass everything through as it came: the control.
    Nothing,
    /// Flip one bit of the gateway's first TLS record.
    Alter,
    /// Leave out the gateway's first TLS record.
    Drop,
    /// Send the gateway's first TLS record twice.
    Replay,
    /// Send 16 bytes from after the gateway's first TLS record before it.
    Reorder,
    /// Pass the handshake through, then flip one bit of the first protected
    /// bytes the gateway sends after the reader's second flight.
    AlterAfterHandshake,
}

/// Copy `from` into `to` until either ends, counting each chunk into
/// `chunks`, then close `to` for writing.
fn forward(mut from: transport::Stream, mut to: transport::Stream, chunks: Arc<AtomicUsize>) {
    let mut buffer = [0; 16 * 1024];
    while let Ok(read) = from.read(&mut buffer) {
        if read == 0 {
            break;
        }
        // Counted before it is passed on, so nothing the other side sends
        // in answer is read before the count moves.
        chunks.fetch_add(1, Ordering::SeqCst);
        if to.write_all(&buffer[..read]).is_err() {
            break;
        }
    }
    let _ = to.shutdown(Shutdown::Write);
}

/// A relay in front of `upstream` that passes the preamble through and then
/// does `tamper` to what the gateway sends; it takes one reader. It answers
/// whether it saw `Joined` and did what `tamper` says, so a read that fails
/// for any other reason fails the test.
fn tampering_relay(upstream: RelayAddress, tamper: Tamper) -> (RelayAddress, JoinHandle<bool>) {
    tampering_relay_for(upstream, tamper, Duration::from_secs(5))
}

fn tampering_relay_for(
    upstream: RelayAddress,
    tamper: Tamper,
    budget: Duration,
) -> (RelayAddress, JoinHandle<bool>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = RelayAddress::try_from(listener.local_addr().unwrap().to_string()).unwrap();
    let deadline = Instant::now() + budget;
    let relaying = thread::spawn(move || {
        let Ok(mut reader) = transport::Stream::accept(&listener, deadline) else {
            return false;
        };
        let tampered = (|| {
            let mut gateway =
                transport::Stream::connect(upstream.as_str().parse().ok()?, deadline).ok()?;
            let mut connect = [0; 35];
            reader.read_exact(&mut connect).ok()?;
            gateway.write_all(&connect).ok()?;
            let mut joined = [0; 3];
            gateway.read_exact(&mut joined).ok()?;
            if joined.as_slice() != RelayFrame::Joined.encode() {
                return None;
            }
            reader.write_all(&joined).ok()?;
            // The reader's bytes go through untouched, counted by chunk.
            let sent = Arc::new(AtomicUsize::new(0));
            let copying = {
                let (from, to) = (reader.try_clone().ok()?, gateway.try_clone().ok()?);
                let sent = sent.clone();
                thread::spawn(move || forward(from, to, sent))
            };
            let done = (|| {
                if let Tamper::AlterAfterHandshake = tamper {
                    // The reader's second flight ends its handshake; what the
                    // gateway sends after it is protected application data.
                    let mut buffer = [0; 16 * 1024];
                    loop {
                        let read = gateway.read(&mut buffer).ok()?;
                        if read == 0 {
                            return None;
                        }
                        let after = sent.load(Ordering::SeqCst) >= 2;
                        if after {
                            buffer[read / 2] ^= 1;
                        }
                        reader.write_all(&buffer[..read]).ok()?;
                        if after {
                            break;
                        }
                    }
                } else {
                    // The gateway's first TLS record whole (a 5-byte header
                    // naming its length), so a replay repeats a complete
                    // record rather than a fragment; then 16 bytes more.
                    let mut first = vec![0; 5];
                    gateway.read_exact(&mut first).ok()?;
                    let length = usize::from(u16::from_be_bytes([first[3], first[4]]));
                    first.resize(5 + length, 0);
                    gateway.read_exact(&mut first[5..]).ok()?;
                    let mut second = vec![0; 16];
                    gateway.read_exact(&mut second).ok()?;
                    let start = match tamper {
                        Tamper::Nothing => [first, second].concat(),
                        Tamper::Alter => {
                            first[8] ^= 1;
                            [first, second].concat()
                        }
                        Tamper::Drop => second,
                        Tamper::Replay => [first.clone(), first, second].concat(),
                        Tamper::Reorder => [second, first].concat(),
                        Tamper::AlterAfterHandshake => unreachable!(),
                    };
                    reader.write_all(&start).ok()?;
                }
                Some(())
            })();
            if done.is_some() {
                let _ = std::io::copy(&mut gateway, &mut reader);
            }
            let _ = reader.shutdown(Shutdown::Both);
            let _ = gateway.shutdown(Shutdown::Both);
            let _ = copying.join();
            done
        })();
        let _ = reader.shutdown(Shutdown::Both);
        tampered.is_some()
    });
    (address, relaying)
}

#[test]
fn a_proxy_whose_reader_never_connects_expires_before_joining() {
    let unused = TcpListener::bind("127.0.0.1:0").unwrap();
    for budget in [Duration::ZERO, Duration::from_millis(30)] {
        let upstream = RelayAddress::try_from(unused.local_addr().unwrap().to_string()).unwrap();
        let (_address, relaying) = tampering_relay_for(upstream, Tamper::Nothing, budget);
        assert!(!relaying.join().unwrap(), "no session was relayed");
        let (_address, relaying) = impostor_relay_for(unused.local_addr().unwrap(), budget);
        assert!(!relaying.join().unwrap(), "no impostor session was relayed");
    }
}

/// A relay that joins its one reader to `impostor` itself, a gateway with
/// its own key, rather than to the gateway the reader asked for. It answers
/// whether it joined the reader and passed on the impostor's bytes.
fn impostor_relay(impostor: SocketAddr) -> (RelayAddress, JoinHandle<bool>) {
    impostor_relay_for(impostor, Duration::from_secs(5))
}

fn impostor_relay_for(impostor: SocketAddr, budget: Duration) -> (RelayAddress, JoinHandle<bool>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = RelayAddress::try_from(listener.local_addr().unwrap().to_string()).unwrap();
    let deadline = Instant::now() + budget;
    let relaying = thread::spawn(move || {
        let Ok(mut reader) = transport::Stream::accept(&listener, deadline) else {
            return false;
        };
        let answered = (|| {
            let mut connect = [0; 35];
            reader.read_exact(&mut connect).ok()?;
            let gateway = transport::Stream::connect(impostor, deadline).ok()?;
            reader.write_all(&RelayFrame::Joined.encode()).ok()?;
            let copying = {
                let (from, to) = (reader.try_clone().ok()?, gateway.try_clone().ok()?);
                thread::spawn(move || forward(from, to, Arc::default()))
            };
            let answered = Arc::new(AtomicUsize::new(0));
            forward(
                gateway.try_clone().ok()?,
                reader.try_clone().ok()?,
                answered.clone(),
            );
            let _ = reader.shutdown(Shutdown::Both);
            let _ = gateway.shutdown(Shutdown::Both);
            let _ = copying.join();
            (answered.load(Ordering::SeqCst) > 0).then_some(())
        })();
        let _ = reader.shutdown(Shutdown::Both);
        answered.is_some()
    });
    (address, relaying)
}

/// RL10: a relay that alters, drops, reorders or replays the gateway's
/// bytes after the join, during the handshake or after it, or that answers
/// as a gateway itself, ends the read's TLS session: the read is
/// `Unreachable` and nothing reaches the cache.
#[test]
fn a_relay_that_tampers_with_the_session_is_unreachable_and_saves_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let (_gateway, device) = Device::start(directory.path(), "");
    device.wait_registered();
    let relay = device.relay();
    let untouched = |cache: &mut RetainedCache| {
        assert!(cache.conversation_ids(&device.receiver).unwrap().is_empty());
    };
    // The same relay in front, passing everything through, serves the read:
    // what fails below fails for what it does to the bytes.
    let mut control = device.cache("control.sqlite3");
    assert!(
        completion::finish(|budget| {
            let (faithful, relaying) = tampering_relay_for(relay.clone(), Tamper::Nothing, budget);
            let read = device.read_for(&mut control, REFUSED, Some(&faithful), budget, 8);
            assert!(relaying.join().unwrap(), "the control relayed the session");
            read
        })
        .unwrap()
        .complete
    );
    for tamper in [
        Tamper::Alter,
        Tamper::Drop,
        Tamper::Replay,
        Tamper::Reorder,
        Tamper::AlterAfterHandshake,
    ] {
        let mut cache = device.cache(&format!("{tamper:?}.sqlite3"));
        let (tampering, relaying) = tampering_relay(relay.clone(), tamper);
        assert_eq!(
            device.read(&mut cache, REFUSED, Some(&tampering)),
            Err(ReadFailure::Unreachable),
            "{tamper:?}"
        );
        assert!(relaying.join().unwrap(), "{tamper:?} was done");
        untouched(&mut cache);
    }

    // Another real gateway, with a key of its own, answering in its place.
    let other = tempfile::tempdir().unwrap();
    let (_impostor, impostor) = Device::start(other.path(), "");
    let mut cache = device.cache("impostor.sqlite3");
    let (answering, relaying) = impostor_relay(impostor.address);
    assert_eq!(
        device.read(&mut cache, REFUSED, Some(&answering)),
        Err(ReadFailure::Unreachable)
    );
    assert!(relaying.join().unwrap(), "the impostor answered");
    untouched(&mut cache);

    // The real relay still serves the reader.
    let mut cache = device.cache("after.sqlite3");
    assert!(
        completion::finish(|budget| {
            device.read_for(&mut cache, REFUSED, Some(&relay), budget, 8)
        })
        .unwrap()
        .complete
    );
}

/// RL11: a reader whose credential the owner revoked is refused through
/// the relay exactly as it is on a direct connection.
#[test]
fn a_revoked_reader_is_refused_through_the_relay_as_directly() {
    let directory = tempfile::tempdir().unwrap();
    let (gateway, device) = Device::start(directory.path(), "");
    drop(gateway);
    let _revoked = Gateway::start_relayed(&device.root, "revoke");
    let relay = device.relay();
    device.wait_registered();

    let mut direct = device.cache("direct.sqlite3");
    let refused = device.read(&mut direct, device.address, None);
    assert_eq!(refused, Err(ReadFailure::Refused));
    let mut relayed = device.cache("relayed.sqlite3");
    assert_eq!(device.read_once_registered(&mut relayed, &relay), refused);
    assert!(relayed
        .conversation_ids(&device.receiver)
        .unwrap()
        .is_empty());
}

/// RL11b: readers asking the relay for the gateway faster than it takes
/// them are joined only up to its relay pool, and the rest are answered
/// `NoAnswer`; a reader dialing directly is admitted while the joined ones
/// hold the pool.
#[test]
fn readers_past_the_gateways_relay_pool_get_no_answer_while_direct_reads_go_on() {
    const ASKED: usize = 12;
    const POOL: usize = RELAY_CAPACITY;
    let directory = tempfile::tempdir().unwrap();
    let (_gateway, device) = Device::start(directory.path(), "");
    device.wait_registered();
    let relay = device.relay();
    let key = relay_key(&device.pin).unwrap();
    let (answered, answers) = mpsc::channel();
    let asking: Vec<_> = (0..ASKED)
        .map(|_| {
            let (relay, answered) = (relay.clone(), answered.clone());
            thread::spawn(move || {
                let _ = answered.send(ask(&relay, key));
            })
        })
        .collect();
    drop(answered);
    // Joined readers are answered at once, the rest at the relay's join
    // deadline. The probe that saw the registration closed its stream, but
    // the gateway may not yet have seen it close and freed that permit, so
    // the pool can be one short.
    let mut held = Vec::new();
    let mut refused = 0;
    while held.len() < POOL - 1 {
        let (stream, frame) = answers.recv().unwrap();
        match frame {
            RelayFrame::Joined => held.push(stream),
            RelayFrame::NoAnswer => refused += 1,
            other => panic!("{other:?}"),
        }
    }

    // The joined readers hold the gateway's relay permits, unread, until
    // its handshake deadline; a direct reader is admitted all the same.
    let mut direct = device.cache("direct.sqlite3");
    assert!(
        device
            .read(&mut direct, device.address, None)
            .unwrap()
            .complete
    );

    for (stream, frame) in answers {
        match frame {
            RelayFrame::Joined => held.push(stream),
            RelayFrame::NoAnswer => refused += 1,
            other => panic!("{other:?}"),
        }
    }
    for asked in asking {
        asked.join().unwrap();
    }
    assert_eq!(held.len() + refused, ASKED);
    assert!(
        held.len() <= POOL,
        "the gateway's relay pool is {POOL}: {} joined",
        held.len()
    );
}

/// A relay this test process serves, which outlives the gateways it
/// starts; dropping it stops it and waits until it has.
struct TestRelay {
    address: RelayAddress,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    serving: Option<JoinHandle<()>>,
}
impl TestRelay {
    fn start() -> Self {
        use crate::relay::{
            application::{Relay, RelayLimits},
            infrastructure::{serve_tcp, Ed25519Proofs, OsRelayEntropy},
        };
        use crate::timer::{Timer, TokioTimer};
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let (bound, address) = mpsc::channel();
        let serving = thread::spawn(move || {
            let runtime = Builder::new_current_thread().enable_all().build().unwrap();
            runtime.block_on(async move {
                let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                bound.send(socket.local_addr().unwrap()).unwrap();
                let timer: Arc<dyn Timer> = Arc::new(TokioTimer::new());
                let relay = Relay::new(
                    timer.clone(),
                    Arc::new(OsRelayEntropy),
                    Arc::new(Ed25519Proofs),
                    RelayLimits::default(),
                );
                serve_tcp(socket, relay, timer, async {
                    let _ = stopped.await;
                })
                .await;
            });
        });
        let address = address.recv().unwrap();
        Self {
            address: RelayAddress::try_from(address.to_string()).unwrap(),
            stop: Some(stop),
            serving: Some(serving),
        }
    }
}
impl Drop for TestRelay {
    fn drop(&mut self) {
        let _ = self.stop.take().map(|stop| stop.send(()));
        if let Some(serving) = self.serving.take() {
            let _ = serving.join();
        }
    }
}

/// RL12 (R3): a gateway that stops serving is `Away` to a reader whose
/// direct dial fails, once its relay has dropped the registration; what
/// the reader cached is unchanged and still readable; and the reader reads
/// fresh again once the gateway is back.
#[test]
fn a_gateway_that_stops_is_away_with_the_cache_kept_and_reads_again_once_back() {
    let relay = TestRelay::start();
    let at = relay.address.as_str().to_owned();
    let directory = tempfile::tempdir().unwrap();
    let (gateway, device) = Device::start_with(directory.path(), |root| {
        Gateway::start_relayed_at(root, "", &at)
    });
    assert_eq!(device.relay(), relay.address);
    let mut cache = device.cache("away.sqlite3");
    let read = device
        .read_once_registered(&mut cache, &relay.address)
        .unwrap();
    assert!(read.complete);
    let held = cache.conversation_ids(&device.receiver).unwrap();
    assert!(held.len() >= 2);

    // The gateway's serve connection closes with its process, and the relay
    // drops its registration when it sees that.
    drop(gateway);
    let dropped = Instant::now() + Duration::from_secs(20);
    let away = loop {
        match device.read(&mut cache, REFUSED, Some(&relay.address)) {
            Err(ReadFailure::Unreachable) if Instant::now() < dropped => {
                thread::sleep(Duration::from_millis(100));
            }
            ended => break ended,
        }
    };
    assert_eq!(away, Err(ReadFailure::Away));
    assert_eq!(cache.conversation_ids(&device.receiver).unwrap(), held);

    let _back = Gateway::start_relayed_at(&device.root, "", &at);
    let again = device
        .read_once_registered(&mut cache, &relay.address)
        .unwrap();
    assert!(again.complete);
    assert_eq!(cache.conversation_ids(&device.receiver).unwrap(), held);
}

/// One connection through a [`Tap`]: the relay frame it opened with, and
/// the bytes each way, final once `closed` is.
struct Tapped {
    kind: u8,
    up: Arc<AtomicUsize>,
    down: Arc<AtomicUsize>,
    closed: Arc<AtomicUsize>,
    /// Both sockets, shut when the tap is dropped, and the two copies.
    sockets: Vec<TcpStream>,
    copies: Vec<JoinHandle<()>>,
}
impl Tapped {
    fn ended(&self) -> bool {
        self.closed.load(Ordering::SeqCst) == 2
    }
    fn bytes(&self) -> (usize, usize) {
        (
            self.up.load(Ordering::SeqCst),
            self.down.load(Ordering::SeqCst),
        )
    }
}

/// Passes every connection it takes on to `upstream` untouched, counting
/// the bytes each way, as a network observer between the two would see
/// them. Dropping it stops taking connections, shuts those it took, and
/// waits for their copies to end.
struct Tap {
    address: SocketAddr,
    connections: Arc<std::sync::Mutex<Vec<Tapped>>>,
    stopped: Arc<std::sync::atomic::AtomicBool>,
    accepting: Option<JoinHandle<()>>,
}
impl Tap {
    fn start(upstream: String) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let connections = Arc::new(std::sync::Mutex::new(Vec::new()));
        let stopped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let accepting = {
            let (connections, stopped) = (connections.clone(), stopped.clone());
            thread::spawn(move || {
                for taken in listener.incoming() {
                    if stopped.load(Ordering::SeqCst) {
                        return;
                    }
                    let Ok(near) = taken else { continue };
                    let Ok(far) = TcpStream::connect(&upstream) else {
                        continue;
                    };
                    // The first frame's type byte names the connection;
                    // peeking leaves it to be forwarded with the rest.
                    let mut head = [0; 2];
                    let kind = match near.peek(&mut head) {
                        Ok(2) => head[1],
                        _ => 0,
                    };
                    let mut tapped = Tapped {
                        kind,
                        up: Arc::default(),
                        down: Arc::default(),
                        closed: Arc::default(),
                        sockets: [near.try_clone(), far.try_clone()]
                            .into_iter()
                            .flatten()
                            .collect(),
                        copies: Vec::new(),
                    };
                    for (from, to, count) in [
                        (near.try_clone(), far.try_clone(), tapped.up.clone()),
                        (far.try_clone(), near.try_clone(), tapped.down.clone()),
                    ] {
                        let (Ok(from), Ok(to)) = (from, to) else {
                            continue;
                        };
                        let closed = tapped.closed.clone();
                        tapped.copies.push(thread::spawn(move || {
                            count_through(from, to, &count);
                            closed.fetch_add(1, Ordering::SeqCst);
                        }));
                    }
                    connections.lock().unwrap().push(tapped);
                }
            })
        };
        Self {
            address,
            connections,
            stopped,
            accepting: Some(accepting),
        }
    }
    fn relay(&self) -> RelayAddress {
        RelayAddress::try_from(self.address.to_string()).unwrap()
    }
    /// Wait until every connection it took that opened with `kind` (any,
    /// for `None`) has ended, then their bytes, in the order they came.
    fn ended(&self, kind: Option<u8>) -> Vec<(usize, usize)> {
        let until = Instant::now() + Duration::from_secs(20);
        loop {
            let connections = self.connections.lock().unwrap();
            let of_kind: Vec<_> = connections
                .iter()
                .filter(|c| kind.is_none_or(|kind| c.kind == kind))
                .collect();
            if of_kind.iter().all(|c| c.ended()) {
                return of_kind.iter().map(|c| c.bytes()).collect();
            }
            drop(connections);
            assert!(Instant::now() < until, "a tapped connection never ended");
            thread::sleep(Duration::from_millis(20));
        }
    }
    /// The bytes so far of every connection it took that opened with `kind`.
    fn so_far(&self, kind: u8) -> Vec<(usize, usize)> {
        let connections = self.connections.lock().unwrap();
        connections
            .iter()
            .filter(|c| c.kind == kind)
            .map(Tapped::bytes)
            .collect()
    }
}
impl Drop for Tap {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        // Wakes the accept, which then sees the stop.
        let _ = TcpStream::connect(self.address);
        if let Some(accepting) = self.accepting.take() {
            let _ = accepting.join();
        }
        // Every copy it started ends once its sockets are shut.
        let connections = std::mem::take(&mut *self.connections.lock().unwrap());
        for mut tapped in connections {
            for socket in &tapped.sockets {
                let _ = socket.shutdown(Shutdown::Both);
            }
            for copy in tapped.copies.drain(..) {
                let _ = copy.join();
            }
        }
    }
}

/// Copy `from` into `to` until either ends, adding each chunk's length to
/// `count`, then close both for the direction copied.
fn count_through(mut from: TcpStream, mut to: TcpStream, count: &AtomicUsize) {
    let mut buffer = [0; 16 * 1024];
    while let Ok(read) = from.read(&mut buffer) {
        if read == 0 {
            break;
        }
        count.fetch_add(read, Ordering::SeqCst);
        if to.write_all(&buffer[..read]).is_err() {
            break;
        }
    }
    let _ = to.shutdown(Shutdown::Write);
    let _ = from.shutdown(Shutdown::Read);
}

/// R5 (`docs/design/auth/relay.md`, "What the relay sees"): what a joined
/// pair costs over a direct read, counted by taps in front of the relay
/// and of the gateway. The relay adds exactly its preamble frames, and
/// copies every other byte each way unchanged: the reader's leg carries
/// what the gateway's accept leg carries, but for `Connect` where the
/// gateway sends `Accept` (each is then told `Joined`), and
/// the gateway's serve connection carries its registration, one
/// `Incoming` per join, and a `Ping` and `Pong` per interval.
#[test]
fn a_joined_pair_costs_its_preamble_frames_over_a_direct_read() {
    let key = [0; 32];
    let kind = |frame: RelayFrame| frame.encode()[1];
    let length = |frame: RelayFrame| frame.encode().len();
    let token = [0; 16];
    let (connect, joined) = (
        length(RelayFrame::Connect { key }),
        length(RelayFrame::Joined),
    );
    let accept = length(RelayFrame::Accept { key, token });
    let incoming = length(RelayFrame::Incoming { token });
    let registration_up =
        length(RelayFrame::Serve { key }) + length(RelayFrame::Proof { signature: [0; 64] });
    let registration_down =
        length(RelayFrame::Challenge { nonce: [0; 32] }) + length(RelayFrame::Registered);
    let ping = length(RelayFrame::Ping);
    assert_eq!(ping, length(RelayFrame::Pong));

    let relay = TestRelay::start();
    let tap = Tap::start(relay.address.as_str().to_owned());
    let at = tap.relay();
    let directory = tempfile::tempdir().unwrap();
    let (_gateway, device) = Device::start_with(directory.path(), |root| {
        Gateway::start_relayed_at(root, "", at.as_str())
    });
    let direct_tap = Tap::start(device.address.to_string());

    let mut relayed = device.cache("relayed.sqlite3");
    assert!(
        device
            .read_once_registered(&mut relayed, &at)
            .unwrap()
            .complete
    );
    let mut direct = device.cache("direct.sqlite3");
    assert!(
        device
            .read(&mut direct, direct_tap.address, None)
            .unwrap()
            .complete
    );

    // Reads asked before the gateway registered were answered `Offline`;
    // the last reader leg is the one joined.
    let readers = tap.ended(Some(kind(RelayFrame::Connect { key })));
    let accepts = tap.ended(Some(kind(RelayFrame::Accept { key, token })));
    let [.., (reader_up, reader_down)] = readers[..] else {
        panic!("no reader dialed the relay")
    };
    let [(accept_up, accept_down)] = accepts[..] else {
        panic!("one join, one accept: {accepts:?}")
    };
    // Each side is told `Joined`, then is given the other's bytes.
    assert_eq!(
        reader_up - connect,
        accept_down - joined,
        "the reader's bytes, unchanged"
    );
    assert_eq!(
        reader_down - joined,
        accept_up - accept,
        "the gateway's bytes, unchanged"
    );
    let [(serve_up, serve_down)] = tap.so_far(kind(RelayFrame::Serve { key }))[..] else {
        panic!("one serve connection")
    };
    let pongs = (serve_up - registration_up) / ping;
    assert_eq!(serve_up, registration_up + pongs * ping);
    // The relay may have pinged once more than the gateway has answered.
    let pings = (serve_down - registration_down - incoming) / ping;
    assert_eq!(serve_down, registration_down + incoming + pings * ping);
    assert!(pings == pongs || pings == pongs + 1);

    let [(direct_up, direct_down)] = direct_tap.ended(None)[..] else {
        panic!("one direct read")
    };
    eprintln!(
        "R5: direct read {direct_up} B up, {direct_down} B down; through the relay the \
         reader's leg {reader_up} B up, {reader_down} B down, the gateway's accept leg \
         {accept_up} B up, {accept_down} B down; one Incoming of {incoming} B; \
         registration {registration_up} B up, {registration_down} B down; Ping and Pong \
         {ping} B each"
    );
}
