//! Issue #266, slice R2c: a retained read through the gateway's relay, end
//! to end, against a real gateway process registered at a real relay
//! (`docs/design/auth/relay.md`, rows RL9 to RL11b).
//!
//! ```text
//! reader ── direct dial refused ──▶ (nothing)
//!        ── Connect{K} ──▶ relay ── Incoming ──▶ gateway ── Accept ──▶ relay
//!        ◀───────────── Joined, then the gateway's own TLS ─────────────▶
//! ```
use super::fixtures::Gateway;
use crate::device_pairing::infrastructure::RELAY_CAPACITY;
use nessa_auth::{
    adapters::pairing::{relay_key, FilePairingState, NativeIdentity},
    application::{pairing::ClientPendingStore, ports::Clock as WallClock},
};
use nessa_client_core::retained::{
    ReadFailure, ReadReport, ReadStop, ReaderAccess, ReaderRole, RetainedCache,
    RetainedEnrollmentClient,
};
use nessa_protocol::{
    clock::Clock as MonotonicClock,
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
            Duration::from_secs(5),
            8,
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

    let mut relayed = device.cache("relayed.sqlite3");
    let report = device.read_once_registered(&mut relayed, &relay).unwrap();
    assert!(report.complete);
    assert!(report.conversations >= 2);

    let mut direct = device.cache("direct.sqlite3");
    assert!(
        device
            .read(&mut direct, device.address, None)
            .unwrap()
            .complete
    );
    assert_eq!(
        relayed.conversation_ids(&device.receiver).unwrap(),
        direct.conversation_ids(&device.receiver).unwrap()
    );

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

/// What a man-in-the-middle relay does to the session once it has joined
/// the reader.
#[derive(Clone, Copy, Debug)]
enum Tamper {
    /// Pass everything through as it came: the control.
    Nothing,
    /// Flip one bit of the gateway's first bytes.
    Alter,
    /// Leave out a stretch of the gateway's first bytes.
    Drop,
    /// Send the gateway's first chunk twice.
    Replay,
    /// Swap the order of the gateway's first two chunks.
    Reorder,
    /// Pass the handshake through, then flip one bit of the first protected
    /// bytes the gateway sends after the reader's second flight.
    AlterAfterHandshake,
}

/// Copy `from` into `to` until either ends, counting each chunk into
/// `chunks`, then close `to` for writing.
fn forward(mut from: TcpStream, mut to: TcpStream, chunks: Arc<AtomicUsize>) {
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
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = RelayAddress::try_from(listener.local_addr().unwrap().to_string()).unwrap();
    let relaying = thread::spawn(move || {
        let Ok((mut reader, _)) = listener.accept() else {
            return false;
        };
        let tampered = (|| {
            let mut gateway = TcpStream::connect(upstream.as_str()).ok()?;
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
                    let mut chunk = || {
                        let mut bytes = vec![0; 16];
                        gateway.read_exact(&mut bytes).map(|()| bytes).ok()
                    };
                    let (mut first, second) = (chunk()?, chunk()?);
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

/// A relay that joins its one reader to `impostor` itself, a gateway with
/// its own key, rather than to the gateway the reader asked for. It answers
/// whether it joined the reader and passed on the impostor's bytes.
fn impostor_relay(impostor: SocketAddr) -> (RelayAddress, JoinHandle<bool>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = RelayAddress::try_from(listener.local_addr().unwrap().to_string()).unwrap();
    let relaying = thread::spawn(move || {
        let Ok((mut reader, _)) = listener.accept() else {
            return false;
        };
        let answered = (|| {
            let mut connect = [0; 35];
            reader.read_exact(&mut connect).ok()?;
            let gateway = TcpStream::connect(impostor).ok()?;
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
    let (faithful, relaying) = tampering_relay(relay.clone(), Tamper::Nothing);
    assert!(
        device
            .read(&mut control, REFUSED, Some(&faithful))
            .unwrap()
            .complete
    );
    assert!(relaying.join().unwrap(), "the control relayed the session");
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
        device
            .read(&mut cache, REFUSED, Some(&relay))
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
