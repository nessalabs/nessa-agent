//! Local integrity is not a gateway hold, and a gateway answer is fresh only
//! when the part on disk matches the named digest.
use super::{
    confirmed_held_file, local_held_file, ArtifactFaultKind, ArtifactFreshness, ArtifactRequest,
    Pause, ReadFailure, ReadStop, StoppedSource,
};
use crate::read_only_sync::application::held_file::{HeldFileSource, RangeAnswer};
use crate::read_only_sync::application::GatewayError;
use crate::read_only_sync::domain::HeldRange;
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::domain::common::value_objects::Sha256Digest;
use sha2::{Digest, Sha256};
use std::fs;
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
