//! A published file's bytes, read off the host over an artifact channel
//! (issue #701), resumed where a channel that failed left off; and a fetched
//! file's bytes, written onto it the same way (slice D3b).
//!
//! ```text
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
//! ```
//!
//! Arrows are steps, in order. Nothing here checks the bytes: their size
//! and digest are checked by whoever keeps them, against what the host
//! published, so a resume that went wrong is a mismatch, never a file.
use super::{
    connector::ArtifactChannels,
    sftp::{ByteSink, ByteSource, SftpError, SftpSession, SinkWrite, SourceRead},
};
use crate::conversation::application::{
    ArtifactBytes, ArtifactChunk, ArtifactReadFailure, HeldSource, HeldSourceFailure,
    ARTIFACT_READ_RESUMES,
};
use std::{
    io,
    sync::{Arc, Mutex, PoisonError},
};
use tokio::{
    sync::{mpsc, watch},
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
