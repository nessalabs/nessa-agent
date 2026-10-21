//! Local integrity is not a gateway hold, and a gateway answer is fresh only
//! when the part on disk matches the named digest.
use super::{
    confirmed_held_file, local_held_file, ArtifactFault, ArtifactFaultKind, ArtifactFreshness,
    ArtifactRequest, Pause, ReadFailure, ReadStop, StoppedSource,
};
use crate::read_only_sync::application::held_file::{HeldFileSource, RangeAnswer};
use crate::read_only_sync::application::GatewayError;
use crate::read_only_sync::domain::HeldRange;
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::domain::common::value_objects::Sha256Digest;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

struct Still;
impl Pause for Still {
    fn pause(&self, _: Duration) {}
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
    )
    .unwrap()
    .unwrap_err();
    assert_eq!(fault.kind(), ArtifactFaultKind::Integrity);
    assert_eq!(fault.reached(), 0);
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
