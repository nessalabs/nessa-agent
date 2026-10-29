//! A published file's bytes, read off the host (issue #701): over an
//! artifact channel, resumed where a channel that failed left off
//! ([`SftpBytes`]); or, on a paired connection, which has nothing beside
//! its lease frames (issue #706), over those frames ([`FrameBytes`]). And
//! a fetched file's bytes, written onto an artifact channel the same way
//! (slice D3b); never over the frames.
//!
//! ```text
//! SftpBytes:
//! first next() ──▶ a task: open a channel ──▶ sftp INIT ──▶ read_file(path, reached)
//!   chunks, in file order ──▶ a queue of a few ──▶ next() hands each on
//!   the channel failed ──▶ resume from `reached` on a new one, ARTIFACT_READ_RESUMES times
//!   no such file, another size ──▶ Changed
//!   the lease no longer routed here ──▶ LeaseEnded, at once
//!   past ARTIFACT_READ_RESUMES, or the host misspoke ──▶ Unavailable
//! dropped ──▶ the task is stopped, its channel ended
//! write_part(path, size, source):
//!   open a channel ──▶ sftp INIT ──▶ write_file(path, size, acknowledged)
//!     each range read from the conversation's hold ──▶ WRITE
//!   the channel failed ──▶ resume from what the host has and acknowledged,
//!     on a new one, ARTIFACT_READ_RESUMES times; past that ──▶ ChannelUnavailable
//!   the hold let go ──▶ NotHeld;  not readable here ──▶ SourceUnavailable
//!   the lease no longer routed here ──▶ LeaseEnded, at once
//! FrameBytes:
//! next() ──▶ ask ranges of MAX_DATA_BYTES in file order, at most
//!            MAX_ARTIFACT_READS_IN_FLIGHT unanswered (HostLink::read_artifact)
//!   ──▶ the first range's answer: its bytes ──▶ handed on, as many as came
//!   unreadable there ──▶ Changed
//!   busy ──▶ Unavailable (the host misspoke)
//!   not staged there, not asked, or the lease's route gone ──▶ LeaseEnded
//! ```
//!
//! Arrows are steps, in order. Nothing here checks the bytes: their size
//! and digest are checked by whoever keeps them, against what the host
//! published, so a resume that went wrong is a mismatch, never a file. A
//! paired connection's frames have no channel of their own to fail: when
//! the connection goes, the lease goes with it, so nothing is resumed.
use super::{
    link::{ArtifactReadAnswer, HostLink},
    sftp::{ByteSink, ByteSource, SftpError, SftpSession, SinkWrite, SourceRead},
};
use crate::conversation::application::{
    ArtifactBytes, ArtifactChannels, ArtifactChunk, ArtifactReadFailure, HeldSource,
    HeldSourceFailure, ARTIFACT_READ_RESUMES,
};
use nessa_protocol::lease::{ArtifactReadRefusal, MAX_ARTIFACT_READS_IN_FLIGHT, MAX_DATA_BYTES};
use std::{
    collections::VecDeque,
    io,
    sync::{Arc, Mutex, PoisonError},
};
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
};

/// Chunks read ahead of whoever pulls them.
const AHEAD: usize = 4;

type Chunk = Result<Vec<u8>, ArtifactReadFailure>;

/// One published file's bytes on a host.
pub(crate) struct SftpBytes {
    start: Option<Start>,
    chunks: Option<mpsc::Receiver<Chunk>>,
    task: Option<JoinHandle<()>>,
}

struct Start {
    channels: Arc<dyn ArtifactChannels>,
    path: String,
    size: u64,
    gone: watch::Receiver<()>,
}

impl SftpBytes {
    /// The bytes of `path`, `size` long, read over `channels` while the
    /// lease's `gone` is open.
    pub(crate) fn new(
        channels: Arc<dyn ArtifactChannels>,
        path: String,
        size: u64,
        gone: watch::Receiver<()>,
    ) -> Self {
        Self {
            start: Some(Start {
                channels,
                path,
                size,
                gone,
            }),
            chunks: None,
            task: None,
        }
    }
}

impl ArtifactBytes for SftpBytes {
    fn next(&mut self) -> ArtifactChunk<'_> {
        Box::pin(async move {
            if let Some(start) = self.start.take() {
                let (sender, chunks) = mpsc::channel(AHEAD);
                self.task = Some(tokio::spawn(read(start, sender)));
                self.chunks = Some(chunks);
            }
            let Some(chunks) = self.chunks.as_mut() else {
                return Ok(None);
            };
            match chunks.recv().await {
                Some(Ok(chunk)) => Ok(Some(chunk)),
                Some(Err(failure)) => {
                    self.chunks = None;
                    Err(failure)
                }
                None => Ok(None),
            }
        })
    }
}

impl Drop for SftpBytes {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

/// Read the whole file into `sender`, resuming, then end it: by closing it
/// once every byte was sent, or with the failure.
async fn read(start: Start, sender: mpsc::Sender<Chunk>) {
    let Start {
        channels,
        path,
        size,
        mut gone,
    } = start;
    let mut reached = 0;
    let mut resumes = 0;
    let lost = async move { while gone.changed().await.is_ok() {} };
    tokio::pin!(lost);
    let failure = loop {
        let attempt = attempt(channels.as_ref(), &path, reached, size, &sender);
        let outcome = tokio::select! {
            outcome = attempt => outcome,
            () = &mut lost => break ArtifactReadFailure::LeaseEnded,
        };
        match outcome {
            Ok(()) => return,
            Err((SftpError::Unavailable(kind), at)) if resumes < ARTIFACT_READ_RESUMES => {
                tracing::warn!(?kind, reached = at, "an artifact channel failed; resuming");
                resumes += 1;
                reached = at;
            }
            // Nobody pulls the bytes any more.
            Err((SftpError::Sink(_), _)) => return,
            Err((SftpError::NoSuchFile | SftpError::SizeChanged, _)) => {
                break ArtifactReadFailure::Changed
            }
            Err((error, _)) => {
                tracing::warn!(%error, "a published file could not be read off its host");
                break ArtifactReadFailure::Unavailable;
            }
        }
    };
    let _ = sender.send(Err(failure)).await;
}

/// One channel's read, from `offset`: the error and how far it reached.
async fn attempt(
    channels: &dyn ArtifactChannels,
    path: &str,
    offset: u64,
    size: u64,
    sender: &mpsc::Sender<Chunk>,
) -> Result<(), (SftpError, u64)> {
    let channel = channels
        .open()
        .map_err(|error| (SftpError::Unavailable(error.kind()), offset))?;
    // Held until the read is done: dropping it ends the channel.
    let _keep = channel.keep;
    let mut session = SftpSession::start(channel.from_host, channel.to_host)
        .await
        .map_err(|error| (error, offset))?;
    let mut sink = Sender(sender);
    session
        .read_file(path, offset, size, &mut sink)
        .await
        .map(|_| ())
        .map_err(|failure| (failure.error, failure.reached))
}

/// The queue to whoever pulls, as an sftp read's sink.
struct Sender<'a>(&'a mpsc::Sender<Chunk>);

impl ByteSink for Sender<'_> {
    fn write(&mut self, chunk: Vec<u8>) -> SinkWrite<'_> {
        Box::pin(async move {
            self.0
                .send(Ok(chunk))
                .await
                .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))
        })
    }
}

/// Why a fetched file's part was not written whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WriteFailure {
    /// The conversation let go of the file.
    NotHeld,
    /// The file's bytes could not be read here.
    SourceUnavailable,
    /// The channel failed more often than it is resumed, the host misspoke,
    /// or the part is not there to write.
    ChannelUnavailable,
    /// The lease is no longer routed here.
    LeaseEnded,
}

/// Write the whole of `size` bytes from `source` to `path` over `channels`,
/// resuming on a new channel when one fails, while the lease's `gone` is
/// open.
pub(crate) async fn write_part(
    channels: &dyn ArtifactChannels,
    path: &str,
    size: u64,
    source: Arc<dyn HeldSource>,
    mut gone: watch::Receiver<()>,
) -> Result<(), WriteFailure> {
    let mut reached = 0;
    let mut resumes = 0;
    let lost = async move { while gone.changed().await.is_ok() {} };
    tokio::pin!(lost);
    loop {
        let read = Arc::new(Mutex::new(None));
        let mut from = Held {
            source: source.as_ref(),
            failure: read.clone(),
        };
        let attempt = write_attempt(channels, path, size, reached, &mut from);
        let outcome = tokio::select! {
            outcome = attempt => outcome,
            () = &mut lost => return Err(WriteFailure::LeaseEnded),
        };
        match outcome {
            Ok(()) => return Ok(()),
            Err((SftpError::Source, _)) => {
                let failure = *read.lock().unwrap_or_else(PoisonError::into_inner);
                return Err(match failure {
                    Some(HeldSourceFailure::NotHeld) => WriteFailure::NotHeld,
                    _ => WriteFailure::SourceUnavailable,
                });
            }
            Err((SftpError::Unavailable(kind), at)) if resumes < ARTIFACT_READ_RESUMES => {
                tracing::warn!(
                    ?kind,
                    reached = at,
                    "an artifact channel failed while writing; resuming"
                );
                resumes += 1;
                reached = at;
            }
            // The host let its part go: only a lease's end or the fetch
            // given up does that.
            Err((SftpError::NoSuchFile, _)) => return Err(WriteFailure::LeaseEnded),
            Err((error, _)) => {
                tracing::warn!(%error, "a fetched file could not be written to its host");
                return Err(WriteFailure::ChannelUnavailable);
            }
        }
    }
}

/// One channel's write, from what is acknowledged: the error and how far
/// the host acknowledged.
async fn write_attempt(
    channels: &dyn ArtifactChannels,
    path: &str,
    size: u64,
    known: u64,
    source: &mut dyn ByteSource,
) -> Result<(), (SftpError, u64)> {
    let channel = channels
        .open()
        .map_err(|error| (SftpError::Unavailable(error.kind()), known))?;
    // Held until the write is done: dropping it ends the channel.
    let _keep = channel.keep;
    let mut session = SftpSession::start(channel.from_host, channel.to_host)
        .await
        .map_err(|error| (error, known))?;
    session
        .write_file(path, size, known, source)
        .await
        .map(|_| ())
        .map_err(|failure| (failure.error, failure.reached))
}

/// The conversation's hold, as an sftp write's source, keeping why a read
/// failed.
struct Held<'a> {
    source: &'a dyn HeldSource,
    failure: Arc<Mutex<Option<HeldSourceFailure>>>,
}

impl ByteSource for Held<'_> {
    fn read(&mut self, offset: u64, max: u32) -> SourceRead<'_> {
        Box::pin(async move {
            self.source
                .read(offset, max as usize)
                .await
                .map_err(|failure| {
                    *self.failure.lock().unwrap_or_else(PoisonError::into_inner) = Some(failure);
                    io::Error::other("the held file could not be read")
                })
        })
    }
}

/// One published file's bytes on a paired environment, read over its lease
/// frames a range at a time.
pub(crate) struct FrameBytes {
    link: Arc<HostLink>,
    lease: String,
    artifact: u32,
    size: u64,
    /// The first offset not yet asked.
    asked: u64,
    /// Ranges asked and not yet handed on, in file order: where each
    /// answer arrives.
    pending: VecDeque<oneshot::Receiver<ArtifactReadAnswer>>,
    /// A failure was handed on: nothing more is read.
    failed: bool,
}

impl FrameBytes {
    /// The bytes of the file published as `artifact` under `lease`, `size`
    /// long, read over `link`'s frames.
    pub(crate) fn new(link: Arc<HostLink>, lease: String, artifact: u32, size: u64) -> Self {
        Self {
            link,
            lease,
            artifact,
            size,
            asked: 0,
            pending: VecDeque::new(),
            failed: false,
        }
    }

    /// The next range in file order, once read: its bytes, or why the file
    /// cannot be read on. Keeps as many ranges asked as may be.
    async fn read_next(&mut self) -> Result<Option<Vec<u8>>, ArtifactReadFailure> {
        while self.pending.len() < MAX_ARTIFACT_READS_IN_FLIGHT && self.asked < self.size {
            let length = (self.size - self.asked).min(MAX_DATA_BYTES as u64);
            let length = u32::try_from(length).unwrap_or(u32::MAX);
            let answer = self
                .link
                .read_artifact(&self.lease, self.artifact, self.asked, length)
                .await
                .ok_or(ArtifactReadFailure::LeaseEnded)?;
            self.pending.push_back(answer);
            self.asked += u64::from(length);
        }
        let Some(answer) = self.pending.pop_front() else {
            return Ok(None);
        };
        // Its wait ends with the lease's route.
        let data = match answer.await {
            Ok(Ok(data)) => data,
            Ok(Err(ArtifactReadRefusal::NotStaged)) | Err(_) => {
                return Err(ArtifactReadFailure::LeaseEnded)
            }
            Ok(Err(ArtifactReadRefusal::Unreadable)) => return Err(ArtifactReadFailure::Changed),
            Ok(Err(ArtifactReadRefusal::Busy)) => {
                tracing::warn!(
                    lease = self.lease,
                    artifact = self.artifact,
                    "the host refused a read this gateway kept within its bound"
                );
                return Err(ArtifactReadFailure::Unavailable);
            }
        };
        Ok(Some(data))
    }
}

impl ArtifactBytes for FrameBytes {
    fn next(&mut self) -> ArtifactChunk<'_> {
        Box::pin(async move {
            if self.failed {
                return Ok(None);
            }
            let next = self.read_next().await;
            if next.is_err() {
                self.failed = true;
                self.pending.clear();
            }
            next
        })
    }
}
