//! Local integrity is not a gateway hold, and a gateway answer is fresh only
//! when the part on disk matches the named digest.
use super::{
    confirmed_held_file, local_held_file, ArtifactFault, ArtifactFaultKind, ArtifactFreshness,
    ArtifactRequest, Pause, ReadFailure, ReadStop, StoppedSource,
};
use crate::read_only_sync::application::held_file::{
    FetchCause, FetchFailure, FetchProgress, FileStep, HeldFileSource, KeepRefusal, RangeAnswer,
};
use crate::read_only_sync::application::{GatewayError, HeldFileRefusal};
use crate::read_only_sync::domain::HeldRange;
use nessa_protocol::conversation::domain::ConversationId;
use nessa_protocol::product_contract::generated::ConversationErrorCode;
use nessa_sdk::domain::common::value_objects::Sha256Digest;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

struct Still;
impl Pause for Still {
    fn pause(&self, _: Duration, _: &dyn Fn() -> bool) {}
}

/// A fetch whose budget is not spent on the zero clock.
fn running(stop: Arc<ReadStop>) -> super::super::Stopping {
    super::super::Stopping {
        stop,
        clock: Arc::new(Held),
        until: u64::MAX,
    }
}

const CONVERSATION: &str = "00000000-0000-4000-8000-000000000001";
const MEDIA: &str = "application/octet-stream";

fn conversation() -> ConversationId {
    ConversationId::new(CONVERSATION).unwrap()
}
fn digest_of(bytes: &[u8]) -> String {
    Sha256Digest::from_bytes(Sha256::digest(bytes).into()).to_string()
}
fn request<'a>(
    conversation: &'a ConversationId,
    digest: &'a str,
    output: &'a std::path::Path,
) -> ArtifactRequest<'a> {
    ArtifactRequest::new(conversation, digest, MEDIA, 2, output)
}

struct Counting {
    bytes: &'static [u8],
    reads: Arc<AtomicUsize>,
    wrong_length: bool,
    corrupt: bool,
}
impl HeldFileSource for Counting {
    fn read(
        &mut self,
        _: &crate::read_only_sync::domain::HeldFile,
        range: HeldRange,
    ) -> Result<RangeAnswer, GatewayError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let start = usize::try_from(range.offset()).unwrap();
        let end = start + usize::try_from(range.length()).unwrap();
        let mut bytes = self.bytes[start..end].to_vec();
        if self.corrupt {
            bytes[0] ^= 0xff;
        }
        if self.wrong_length {
            bytes.pop();
        }
        Ok(RangeAnswer {
            offset: range.offset(),
            bytes,
        })
    }
}

#[test]
fn a_file_already_on_disk_is_a_local_match_and_asks_nothing() {
    let root = tempfile::tempdir().unwrap();
    let output = root.path().join("file.bin");
    let bytes = b"ab";
    fs::write(&output, bytes).unwrap();
    let reads = Arc::new(AtomicUsize::new(0));
    let mut source = Counting {
        bytes,
        reads: reads.clone(),
        wrong_length: false,
        corrupt: false,
    };
    let conversation = conversation();
    let digest = digest_of(bytes);
    let outcome = local_held_file(&request(&conversation, &digest, &output))
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(outcome, ArtifactFreshness::LocalMatch { bytes: 2 });
    // The source is in scope and stays unused: a local match has no range to ask.
    let _ = &mut source;
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    fs::write(&output, b"zz").unwrap();
    let refused = local_held_file(&request(&conversation, &digest, &output))
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(refused.kind(), ArtifactFaultKind::OutputExists);
    assert_eq!(reads.load(Ordering::SeqCst), 0);
}

#[test]
fn a_gateway_answer_is_fresh_only_when_the_digest_matches() {
    let bytes = b"ab";
    let digest = digest_of(bytes);
    let conversation = conversation();
    let reads = Arc::new(AtomicUsize::new(0));
    let root = tempfile::tempdir().unwrap();
    let output = root.path().join("file.bin");
    let mut source = Counting {
        bytes,
        reads: reads.clone(),
        wrong_length: false,
        corrupt: false,
    };
    let fresh = confirmed_held_file(
        &mut source,
        &Still,
        &request(&conversation, &digest, &output),
        &running(ReadStop::new()),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        fresh,
        ArtifactFreshness::GatewayConfirmed {
            ranges: 1,
            resumed_from: 0,
        }
    );
    assert_eq!(fs::read(&output).unwrap(), bytes);
    assert_eq!(reads.load(Ordering::SeqCst), 1);

    let root = tempfile::tempdir().unwrap();
    let output = root.path().join("file.bin");
    let mut source = Counting {
        bytes,
        reads: Arc::new(AtomicUsize::new(0)),
        wrong_length: true,
        corrupt: false,
    };
    let fault = confirmed_held_file(
        &mut source,
        &Still,
        &request(&conversation, &digest, &output),
        &running(ReadStop::new()),
    )
    .unwrap()
    .unwrap_err();
    assert_eq!(fault.kind(), ArtifactFaultKind::WrongRange);
    assert!(!output.exists());

    let root = tempfile::tempdir().unwrap();
    let output = root.path().join("file.bin");
    let mut source = Counting {
        bytes,
        reads: Arc::new(AtomicUsize::new(0)),
        wrong_length: false,
        corrupt: true,
    };
    let fault = confirmed_held_file(
        &mut source,
        &Still,
        &request(&conversation, &digest, &output),
        &running(ReadStop::new()),
    )
    .unwrap()
    .unwrap_err();
    assert_eq!(
        fault.kind(),
        ArtifactFaultKind::Integrity {
            cleanup: super::ArtifactPartCleanup::Removed,
        }
    );
    assert_eq!(fault.reached(), 0);
    assert!(!output.exists());
}

#[test]
fn a_settled_integrity_failure_survives_a_stop_keep_did_not_see() {
    use crate::read_only_sync::application::held_file::{
        FetchCause, FetchFailure, FetchProgress, FileStep, PartCleanup,
    };
    let failure = FetchFailure {
        cause: FetchCause::Integrity {
            cleanup: PartCleanup::Left {
                remove: FileStep::Remove,
                restart: FileStep::Restart,
            },
        },
        progress: FetchProgress {
            resumed_from: 4,
            ..FetchProgress::default()
        },
    };
    let kept = super::publish_fetch(failure, false);
    assert_eq!(
        kept.kind(),
        ArtifactFaultKind::Integrity {
            cleanup: super::ArtifactPartCleanup::Left {
                remove: super::ArtifactFileStep::Remove,
                restart: super::ArtifactFileStep::Restart,
            },
        }
    );
    assert_eq!(
        super::publish_fetch(failure, true).kind(),
        ArtifactFaultKind::Gateway(ReadFailure::Stopped)
    );
}

#[test]
fn an_integrity_fault_keeps_a_part_that_could_not_be_removed() {
    use crate::read_only_sync::application::held_file::{
        FetchCause, FetchFailure, FetchProgress, FileStep, PartCleanup,
    };
    let fault = super::fault_from(FetchFailure {
        cause: FetchCause::Integrity {
            cleanup: PartCleanup::Left {
                remove: FileStep::Remove,
                restart: FileStep::Restart,
            },
        },
        progress: FetchProgress {
            resumed_from: 4,
            ..FetchProgress::default()
        },
    });
    assert_eq!(
        fault.kind(),
        ArtifactFaultKind::Integrity {
            cleanup: super::ArtifactPartCleanup::Left {
                remove: super::ArtifactFileStep::Remove,
                restart: super::ArtifactFileStep::Restart,
            },
        }
    );
    assert_eq!(fault.reached(), 0);
    assert_eq!(fault.resumed_from(), 4);
}

struct CancelAfterRange {
    bytes: &'static [u8],
    stop: Arc<ReadStop>,
}
impl HeldFileSource for CancelAfterRange {
    fn read(
        &mut self,
        _: &crate::read_only_sync::domain::HeldFile,
        range: HeldRange,
    ) -> Result<RangeAnswer, GatewayError> {
        let start = usize::try_from(range.offset()).unwrap();
        let end = start + usize::try_from(range.length()).unwrap();
        let bytes = self.bytes[start..end].to_vec();
        self.stop.stop();
        Ok(RangeAnswer {
            offset: range.offset(),
            bytes,
        })
    }
}

#[test]
fn a_stop_during_the_part_reread_does_not_keep_the_file() {
    let bytes = b"ab";
    let conversation = conversation();
    let digest = digest_of(bytes);
    let root = tempfile::tempdir().unwrap();
    let output = root.path().join("file.bin");
    let stop = ReadStop::new();
    let ending = running(stop.clone());
    let mut source = CancelAfterRange {
        bytes,
        stop: stop.clone(),
    };
    let fault = confirmed_held_file(
        &mut source,
        &Still,
        &request(&conversation, &digest, &output),
        &ending,
    )
    .unwrap()
    .unwrap_err();
    assert_eq!(
        fault.kind(),
        ArtifactFaultKind::Gateway(ReadFailure::Stopped)
    );
    assert!(!output.exists());
}

#[test]
fn a_stopped_source_is_not_a_fresh_file_and_asks_nothing() {
    let bytes = b"ab";
    let reads = Arc::new(AtomicUsize::new(0));
    let stop = ReadStop::new();
    stop.stop();
    let mut source = StoppedSource {
        source: Counting {
            bytes,
            reads: reads.clone(),
            wrong_length: false,
            corrupt: false,
        },
        stop,
    };
    let root = tempfile::tempdir().unwrap();
    let output = root.path().join("file.bin");
    let conversation = conversation();
    let digest = digest_of(bytes);
    let fault = confirmed_held_file(
        &mut source,
        &Still,
        &request(&conversation, &digest, &output),
        &running(ReadStop::new()),
    )
    .unwrap()
    .unwrap_err();
    assert_eq!(
        fault.kind(),
        ArtifactFaultKind::Gateway(ReadFailure::Stopped)
    );
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    assert!(!output.exists());
}

struct Wall;
impl nessa_auth::application::ports::Clock for Wall {
    fn unix_milliseconds(&self) -> u64 {
        1
    }
}
struct Held;
impl nessa_protocol::clock::Clock for Held {
    fn elapsed_ms(&self) -> u64 {
        0
    }
}
fn cache() -> (tempfile::TempDir, super::super::RetainedCache) {
    let root = tempfile::tempdir().unwrap();
    let private = root.path().join("cache");
    nessa_local_storage::create_directory(&private).unwrap();
    let cache = super::super::RetainedCache::open(
        &private.join("cache.sqlite3"),
        Arc::new(Wall),
        Arc::new(Held),
    )
    .unwrap();
    (root, cache)
}
fn access<'a>(
    address: std::net::SocketAddr,
    relay: Option<&'a nessa_protocol::relay::RelayAddress>,
    identity: &'a nessa_auth::adapters::pairing::NativeIdentity,
    pin: &'a [u8; 44],
) -> super::super::ReaderAccess<'a> {
    super::super::ReaderAccess {
        role: super::super::ReaderRole::DeviceOwner,
        address,
        relay,
        identity,
        pin: *pin,
        credential: "issued",
        receiver: "receiver",
        access_epoch: 1,
        client_id: "nessa-ios",
    }
}
fn missing_request<'a>(
    conversation: &'a ConversationId,
    digest: &'a str,
    output: &'a std::path::Path,
) -> ArtifactRequest<'a> {
    ArtifactRequest::new(conversation, digest, MEDIA, 2, output)
}

#[test]
fn a_budget_that_cancels_a_range_is_unreachable_and_a_stop_stays_stopped() {
    let stop = ReadStop::new();
    let ending = super::super::Stopping {
        stop: stop.clone(),
        clock: Arc::new(Held),
        until: 0,
    };
    let fault = ArtifactFault::progress(ArtifactFaultKind::Gateway(ReadFailure::Stopped), 4, 1);
    let spent = super::classify_artifact_fault(fault, &stop, &ending);
    assert_eq!(
        spent.kind(),
        ArtifactFaultKind::Gateway(ReadFailure::Unreachable)
    );
    assert_eq!(spent.reached(), 4);
    assert_eq!(spent.resumed_from(), 1);
    stop.stop();
    let stopped = super::classify_artifact_fault(fault, &stop, &ending);
    assert_eq!(
        stopped.kind(),
        ArtifactFaultKind::Gateway(ReadFailure::Stopped)
    );
}

#[test]
fn a_settled_refusal_survives_a_later_stop() {
    let stop = ReadStop::new();
    let ending = running(stop.clone());
    let fault = ArtifactFault::progress(ArtifactFaultKind::Gateway(ReadFailure::Refused), 4, 1);
    stop.stop();
    let kept = super::classify_artifact_fault(fault, &stop, &ending);
    assert_eq!(
        kept.kind(),
        ArtifactFaultKind::Gateway(ReadFailure::Refused)
    );
    assert_eq!(kept.reached(), 4);
    assert_eq!(kept.resumed_from(), 1);
    assert_eq!(
        super::dial_failure(ReadFailure::Refused, &stop, &ending, false),
        ReadFailure::Refused
    );
}

#[test]
fn a_spent_budget_is_unreachable_and_an_explicit_stop_stays_stopped() {
    let identity = nessa_auth::adapters::pairing::NativeIdentity::generate(
        &mut nessa_auth::adapters::pairing::OsEntropy,
    )
    .unwrap();
    let pin = identity.public_spki();
    let conversation = conversation();
    let digest = digest_of(b"ab");
    let (root, mut cache) = cache();
    let output = root.path().join("held.bin");
    let request = missing_request(&conversation, &digest, &output);
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], 9));
    let stop = ReadStop::new();
    assert_eq!(
        cache.fetch_artifact(
            access(address, None, &identity, &pin),
            &stop,
            Duration::from_millis(0),
            &request,
            &Still,
        ),
        Err(ReadFailure::Unreachable)
    );
    assert!(stop.sockets.lock().unwrap().is_empty());
    let stop = ReadStop::new();
    stop.stop();
    assert_eq!(
        cache.fetch_artifact(
            access(address, None, &identity, &pin),
            &stop,
            Duration::from_secs(5),
            &request,
            &Still,
        ),
        Err(ReadFailure::Stopped)
    );
    assert!(stop.sockets.lock().unwrap().is_empty());
}

#[test]
fn a_stop_does_not_publish_a_local_match() {
    let bytes = b"ab";
    let conversation = conversation();
    let digest = digest_of(bytes);
    let root = tempfile::tempdir().unwrap();
    let output = root.path().join("held.bin");
    fs::write(&output, bytes).unwrap();
    let request = request(&conversation, &digest, &output);
    let stop = ReadStop::new();
    stop.stop();
    let identity = nessa_auth::adapters::pairing::NativeIdentity::generate(
        &mut nessa_auth::adapters::pairing::OsEntropy,
    )
    .unwrap();
    let pin = identity.public_spki();
    let (_cache_root, mut cache) = cache();
    assert_eq!(
        cache.fetch_artifact(
            access(
                std::net::SocketAddr::from(([127, 0, 0, 1], 9)),
                None,
                &identity,
                &pin,
            ),
            &stop,
            Duration::from_secs(5),
            &request,
            &Still,
        ),
        Err(ReadFailure::Stopped)
    );
    let during = ReadStop::new();
    let stopped = super::local_held_outcome(&during, || {
        during.stop();
        local_held_file(&request)
    });
    assert_eq!(stopped, Err(ReadFailure::Stopped));
}

#[test]
fn a_settled_local_fault_survives_a_later_stop() {
    let bytes = b"ab";
    let conversation = conversation();
    let digest = digest_of(bytes);
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("held.bin");
    fs::create_dir(&directory).unwrap();
    let occupied = request(&conversation, &digest, &directory);
    let stop = ReadStop::new();
    let outcome = super::local_held_outcome(&stop, || {
        let found = local_held_file(&occupied);
        stop.stop();
        found
    });
    let Ok(Some(Err(fault))) = outcome else {
        panic!("settled output fault was replaced: {outcome:?}");
    };
    assert_eq!(fault.kind(), ArtifactFaultKind::OutputExists);
    assert!(stop.is_stopped());

    let unnamed = root.path().join("a".repeat(220));
    let stop = ReadStop::new();
    let outcome = super::local_held_outcome(&stop, || {
        let found = local_held_file(&request(&conversation, &digest, &unnamed));
        stop.stop();
        found
    });
    let Ok(Some(Err(fault))) = outcome else {
        panic!("settled name fault was replaced: {outcome:?}");
    };
    assert_eq!(
        fault.kind(),
        ArtifactFaultKind::File(super::ArtifactFileStep::Name)
    );
}

#[test]
fn a_failed_handshake_releases_the_socket_the_stop_registered() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let accepted = Arc::new(AtomicUsize::new(0));
    let listening = accepted.clone();
    let peer = std::thread::spawn(move || {
        if listener.accept().is_ok() {
            listening.store(1, Ordering::SeqCst);
        }
    });
    let identity = nessa_auth::adapters::pairing::NativeIdentity::generate(
        &mut nessa_auth::adapters::pairing::OsEntropy,
    )
    .unwrap();
    let pin = identity.public_spki();
    let conversation = conversation();
    let digest = digest_of(b"ab");
    let (root, mut cache) = cache();
    let output = root.path().join("held.bin");
    let request = missing_request(&conversation, &digest, &output);
    let stop = ReadStop::new();
    let failure = cache
        .fetch_artifact(
            access(address, None, &identity, &pin),
            &stop,
            Duration::from_secs(5),
            &request,
            &Still,
        )
        .unwrap_err();
    assert_eq!(failure, ReadFailure::Unreachable);
    peer.join().unwrap();
    assert_eq!(accepted.load(Ordering::SeqCst), 1);
    assert!(stop.sockets.lock().unwrap().is_empty());
}

#[test]
fn an_offline_relay_makes_the_artifact_fetch_away() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address =
        nessa_protocol::relay::RelayAddress::try_from(listener.local_addr().unwrap().to_string())
            .unwrap();
    let serving = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        nessa_protocol::relay::read_frame(&mut stream).unwrap();
        stream
            .write_all(&nessa_protocol::relay::RelayFrame::Offline.encode())
            .unwrap();
    });
    let identity = nessa_auth::adapters::pairing::NativeIdentity::generate(
        &mut nessa_auth::adapters::pairing::OsEntropy,
    )
    .unwrap();
    let pin = identity.public_spki();
    let conversation = conversation();
    let digest = digest_of(b"ab");
    let (root, mut cache) = cache();
    let output = root.path().join("held.bin");
    let request = missing_request(&conversation, &digest, &output);
    let stop = ReadStop::new();
    let failure = cache
        .fetch_artifact(
            access(
                std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
                Some(&address),
                &identity,
                &pin,
            ),
            &stop,
            Duration::from_secs(5),
            &request,
            &Still,
        )
        .unwrap_err();
    serving.join().unwrap();
    assert_eq!(failure, ReadFailure::Away);
    assert!(stop.sockets.lock().unwrap().is_empty());
}

struct BusyOnce {
    bytes: &'static [u8],
    reads: Arc<AtomicUsize>,
    asked: Arc<AtomicBool>,
}
impl HeldFileSource for BusyOnce {
    fn read(
        &mut self,
        _: &crate::read_only_sync::domain::HeldFile,
        range: HeldRange,
    ) -> Result<RangeAnswer, GatewayError> {
        let n = self.reads.fetch_add(1, Ordering::SeqCst);
        if n == 0 {
            self.asked.store(true, Ordering::SeqCst);
            return Err(GatewayError::HeldFile(HeldFileRefusal::Conversation(
                ConversationErrorCode::TemporarilyUnavailable,
            )));
        }
        let start = usize::try_from(range.offset()).unwrap();
        let end = start + usize::try_from(range.length()).unwrap();
        Ok(RangeAnswer {
            offset: range.offset(),
            bytes: self.bytes[start..end].to_vec(),
        })
    }
}

#[test]
fn a_stop_during_a_busy_pause_returns_within_one_slice() {
    let bytes = b"ab";
    let conversation = conversation();
    let digest = digest_of(bytes);
    let root = tempfile::tempdir().unwrap();
    let output = root.path().join("file.bin");
    let reads = Arc::new(AtomicUsize::new(0));
    let asked = Arc::new(AtomicBool::new(false));
    let stop = ReadStop::new();
    let mut source = BusyOnce {
        bytes,
        reads: reads.clone(),
        asked: asked.clone(),
    };
    let ending = running(stop.clone());
    let stopping = stop.clone();
    let stopper = thread::spawn(move || {
        let started = Instant::now();
        while !asked.load(Ordering::SeqCst) {
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "the busy range was not asked"
            );
            thread::yield_now();
        }
        thread::sleep(Duration::from_millis(20));
        stopping.stop();
    });
    let started = Instant::now();
    let fault = confirmed_held_file(
        &mut source,
        &super::ThreadPause,
        &request(&conversation, &digest, &output),
        &ending,
    )
    .unwrap()
    .unwrap_err();
    let elapsed = started.elapsed();
    stopper.join().unwrap();
    assert_eq!(
        fault.kind(),
        ArtifactFaultKind::Gateway(ReadFailure::Stopped)
    );
    // The first busy delay is 250ms. One stop slice is 50ms, so a pause that
    // observes the latch is back well before that delay.
    assert!(
        elapsed < Duration::from_millis(200),
        "pause ignored the stop: {elapsed:?}"
    );
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    assert!(!output.exists());
}

/// Stays at zero until 30ms after the busy answer, then reports the budget spent.
struct BudgetClock {
    origin: Instant,
    asked_ms: AtomicU64,
}
impl nessa_protocol::clock::Clock for BudgetClock {
    fn elapsed_ms(&self) -> u64 {
        let asked = self.asked_ms.load(Ordering::SeqCst);
        if asked == 0 {
            return 0;
        }
        let now = u64::try_from(self.origin.elapsed().as_millis()).unwrap_or(u64::MAX);
        if now.saturating_sub(asked) >= 30 {
            1
        } else {
            0
        }
    }
}

struct BusyBeforeBudget {
    reads: Arc<AtomicUsize>,
    clock: Arc<BudgetClock>,
}
impl HeldFileSource for BusyBeforeBudget {
    fn read(
        &mut self,
        _: &crate::read_only_sync::domain::HeldFile,
        _: HeldRange,
    ) -> Result<RangeAnswer, GatewayError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let now = u64::try_from(self.clock.origin.elapsed().as_millis())
            .unwrap_or(1)
            .max(1);
        self.clock.asked_ms.store(now, Ordering::SeqCst);
        Err(GatewayError::HeldFile(HeldFileRefusal::Conversation(
            ConversationErrorCode::TemporarilyUnavailable,
        )))
    }
}

#[test]
fn a_budget_that_ends_during_a_busy_pause_is_unreachable() {
    let bytes = b"ab";
    let conversation = conversation();
    let digest = digest_of(bytes);
    let root = tempfile::tempdir().unwrap();
    let output = root.path().join("file.bin");
    let reads = Arc::new(AtomicUsize::new(0));
    let clock = Arc::new(BudgetClock {
        origin: Instant::now(),
        asked_ms: AtomicU64::new(0),
    });
    let ending = super::super::Stopping {
        stop: ReadStop::new(),
        clock: clock.clone(),
        until: 1,
    };
    let mut source = BusyBeforeBudget {
        reads: reads.clone(),
        clock,
    };
    let started = Instant::now();
    let fault = confirmed_held_file(
        &mut source,
        &super::ThreadPause,
        &request(&conversation, &digest, &output),
        &ending,
    )
    .unwrap()
    .unwrap_err();
    let elapsed = started.elapsed();
    assert_eq!(
        fault.kind(),
        ArtifactFaultKind::Gateway(ReadFailure::Stopped)
    );
    assert_eq!(
        super::classify_artifact_fault(fault, &ending.stop, &ending).kind(),
        ArtifactFaultKind::Gateway(ReadFailure::Unreachable)
    );
    // The first busy delay is 250ms. The budget becomes spent 30ms into that
    // pause, so a wait that observes it is back within one stop slice.
    assert!(
        elapsed < Duration::from_millis(200),
        "pause ignored the budget: {elapsed:?}"
    );
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    assert!(!output.exists());
    assert!(!ending.stop.is_stopped());
}

struct SyncStops {
    stop: Arc<ReadStop>,
    linked: bool,
}
impl super::HeldPart for SyncStops {
    fn sync_held(&mut self) -> Result<(), KeepRefusal> {
        self.stop.stop();
        Ok(())
    }
    fn link_held(&mut self) -> Result<(), KeepRefusal> {
        self.linked = true;
        Ok(())
    }
    fn retire_held(&mut self) -> Result<(), KeepRefusal> {
        Ok(())
    }
}

#[test]
fn a_stop_during_sync_does_not_link() {
    let stop = ReadStop::new();
    let mut part = SyncStops {
        stop: stop.clone(),
        linked: false,
    };
    let ending = running(stop.clone());
    let end = super::publish_part(&ending, &mut part);
    assert!(!part.linked);
    assert_eq!(end.phase, super::Publication::Stopped);
    assert_eq!(end.result, Err(KeepRefusal::Stopped));
    assert!(stop.is_stopped());
}

struct LinkHolds {
    output: Arc<AtomicBool>,
    in_link: Arc<AtomicBool>,
    stopping: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
}
impl super::HeldPart for LinkHolds {
    fn sync_held(&mut self) -> Result<(), KeepRefusal> {
        Ok(())
    }
    fn link_held(&mut self) -> Result<(), KeepRefusal> {
        self.output.store(true, Ordering::SeqCst);
        self.in_link.store(true, Ordering::SeqCst);
        let started = Instant::now();
        while !self.stopping.load(Ordering::SeqCst) {
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "stop was not asked during the link"
            );
            thread::yield_now();
        }
        thread::sleep(Duration::from_millis(50));
        self.done.store(true, Ordering::SeqCst);
        Ok(())
    }
    fn retire_held(&mut self) -> Result<(), KeepRefusal> {
        Ok(())
    }
}

#[test]
fn a_stop_during_the_link_waits_until_the_link_is_published() {
    let stop = ReadStop::new();
    let output = Arc::new(AtomicBool::new(false));
    let in_link = Arc::new(AtomicBool::new(false));
    let stopping = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));
    let mut part = LinkHolds {
        output: output.clone(),
        in_link: in_link.clone(),
        stopping: stopping.clone(),
        done: done.clone(),
    };
    let stopping_flag = stopping.clone();
    let in_link_flag = in_link.clone();
    let done_flag = done.clone();
    let gate = stop.clone();
    let stopper = thread::spawn(move || {
        let started = Instant::now();
        while !in_link_flag.load(Ordering::SeqCst) {
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "the link did not take the gate"
            );
            thread::yield_now();
        }
        stopping_flag.store(true, Ordering::SeqCst);
        let waited = Instant::now();
        gate.stop();
        assert!(
            done_flag.load(Ordering::SeqCst),
            "stop returned while the link was still inside the gate"
        );
        assert!(
            waited.elapsed() >= Duration::from_millis(40),
            "stop did not wait for the link"
        );
    });
    let ending = running(stop.clone());
    let end = super::publish_part(&ending, &mut part);
    stopper.join().unwrap();
    assert!(output.load(Ordering::SeqCst));
    assert_eq!(end.phase, super::Publication::Linked);
    assert_eq!(end.result, Ok(()));
    assert!(stop.is_stopped());
}

struct RetireFails {
    step: FileStep,
    linked: bool,
}
impl super::HeldPart for RetireFails {
    fn sync_held(&mut self) -> Result<(), KeepRefusal> {
        Ok(())
    }
    fn link_held(&mut self) -> Result<(), KeepRefusal> {
        self.linked = true;
        Ok(())
    }
    fn retire_held(&mut self) -> Result<(), KeepRefusal> {
        Err(KeepRefusal::Published { step: self.step })
    }
}

#[test]
fn a_linked_output_keeps_its_cleanup_failure() {
    for step in [FileStep::Remove, FileStep::SyncDirectory] {
        let mut part = RetireFails {
            step,
            linked: false,
        };
        let stop = ReadStop::new();
        let ending = running(stop.clone());
        let end = super::publish_part(&ending, &mut part);
        assert!(part.linked);
        assert_eq!(end.phase, super::Publication::Linked);
        assert_eq!(end.result, Err(KeepRefusal::Published { step }));
        let failure = FetchFailure {
            cause: FetchCause::Published { step },
            progress: FetchProgress {
                reached: 2,
                ..FetchProgress::default()
            },
        };
        let linked = ArtifactFaultKind::Linked {
            step: super::file_step(step),
        };
        let fault = super::fault_from(failure);
        assert_eq!(fault.kind(), linked);
        assert_eq!(fault.reached(), 2);
        assert_eq!(super::publish_fetch(failure, false).kind(), linked);
        stop.stop();
        let ending = super::super::Stopping {
            stop: stop.clone(),
            clock: Arc::new(Held),
            until: 0,
        };
        assert_eq!(
            super::classify_artifact_fault(fault, &stop, &ending).kind(),
            linked
        );
    }
}

struct ToggleClock {
    spent: AtomicBool,
}
impl nessa_protocol::clock::Clock for ToggleClock {
    fn elapsed_ms(&self) -> u64 {
        u64::from(self.spent.load(Ordering::SeqCst))
    }
}
fn budget(clock: Arc<ToggleClock>, stop: Arc<ReadStop>) -> super::super::Stopping {
    super::super::Stopping {
        stop,
        clock,
        until: 1,
    }
}

struct SpendAfterRange {
    bytes: &'static [u8],
    clock: Arc<ToggleClock>,
}
impl HeldFileSource for SpendAfterRange {
    fn read(
        &mut self,
        _: &crate::read_only_sync::domain::HeldFile,
        range: HeldRange,
    ) -> Result<RangeAnswer, GatewayError> {
        let start = usize::try_from(range.offset()).unwrap();
        let end = start + usize::try_from(range.length()).unwrap();
        self.clock.spent.store(true, Ordering::SeqCst);
        Ok(RangeAnswer {
            offset: range.offset(),
            bytes: self.bytes[start..end].to_vec(),
        })
    }
}

#[test]
fn a_budget_that_expires_after_the_range_does_not_publish() {
    let bytes = b"ab";
    let conversation = conversation();
    let digest = digest_of(bytes);
    let root = tempfile::tempdir().unwrap();
    let output = root.path().join("file.bin");
    let clock = Arc::new(ToggleClock {
        spent: AtomicBool::new(false),
    });
    let stop = ReadStop::new();
    let ending = budget(clock.clone(), stop.clone());
    let mut source = SpendAfterRange { bytes, clock };
    let fault = confirmed_held_file(
        &mut source,
        &Still,
        &request(&conversation, &digest, &output),
        &ending,
    )
    .unwrap()
    .unwrap_err();
    assert_eq!(
        fault.kind(),
        ArtifactFaultKind::Gateway(ReadFailure::Stopped)
    );
    assert_eq!(
        super::classify_artifact_fault(fault, &stop, &ending).kind(),
        ArtifactFaultKind::Gateway(ReadFailure::Unreachable)
    );
    assert!(!output.exists());
    assert!(!stop.is_stopped());
}

struct SyncSpends {
    clock: Arc<ToggleClock>,
    linked: bool,
}
impl super::HeldPart for SyncSpends {
    fn sync_held(&mut self) -> Result<(), KeepRefusal> {
        self.clock.spent.store(true, Ordering::SeqCst);
        Ok(())
    }
    fn link_held(&mut self) -> Result<(), KeepRefusal> {
        self.linked = true;
        Ok(())
    }
    fn retire_held(&mut self) -> Result<(), KeepRefusal> {
        Ok(())
    }
}

#[test]
fn a_budget_that_expires_during_sync_does_not_link() {
    let clock = Arc::new(ToggleClock {
        spent: AtomicBool::new(false),
    });
    let stop = ReadStop::new();
    let ending = budget(clock.clone(), stop.clone());
    let mut part = SyncSpends {
        clock,
        linked: false,
    };
    let end = super::publish_part(&ending, &mut part);
    assert!(!part.linked);
    assert_eq!(end.phase, super::Publication::Stopped);
    assert_eq!(end.result, Err(KeepRefusal::Stopped));
    let failure = FetchFailure {
        cause: FetchCause::Gateway(GatewayError::Cancelled),
        progress: FetchProgress {
            reached: 2,
            ..FetchProgress::default()
        },
    };
    let fault = super::publish_fetch(failure, true);
    assert_eq!(
        super::classify_artifact_fault(fault, &stop, &ending).kind(),
        ArtifactFaultKind::Gateway(ReadFailure::Unreachable)
    );
}

struct LinkSpends {
    clock: Arc<ToggleClock>,
    linked: bool,
}
impl super::HeldPart for LinkSpends {
    fn sync_held(&mut self) -> Result<(), KeepRefusal> {
        Ok(())
    }
    fn link_held(&mut self) -> Result<(), KeepRefusal> {
        self.clock.spent.store(true, Ordering::SeqCst);
        self.linked = true;
        Ok(())
    }
    fn retire_held(&mut self) -> Result<(), KeepRefusal> {
        Ok(())
    }
}

#[test]
fn a_budget_that_expires_during_the_link_stays_published() {
    let clock = Arc::new(ToggleClock {
        spent: AtomicBool::new(false),
    });
    let stop = ReadStop::new();
    let ending = budget(clock.clone(), stop);
    let mut part = LinkSpends {
        clock,
        linked: false,
    };
    let end = super::publish_part(&ending, &mut part);
    assert!(part.linked);
    assert!(ending.spent());
    assert_eq!(end.phase, super::Publication::Linked);
    assert_eq!(end.result, Ok(()));
}
