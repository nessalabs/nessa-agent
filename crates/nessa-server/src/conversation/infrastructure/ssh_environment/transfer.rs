//! A published file's bytes, read off the host over an artifact channel
//! (issue #701), resumed where a channel that failed left off.
//!
//! ```text
//! first next() ──▶ a task: open a channel ──▶ sftp INIT ──▶ read_file(path, reached)
//!   chunks, in file order ──▶ a queue of a few ──▶ next() hands each on
//!   the channel failed ──▶ resume from `reached` on a new one, ARTIFACT_READ_RESUMES times
//!   no such file, another size ──▶ Changed
//!   the lease no longer routed here ──▶ LeaseEnded, at once
//!   past ARTIFACT_READ_RESUMES, or the host misspoke ──▶ Unavailable
//! dropped ──▶ the task is stopped, its channel ended
//! ```
//!
//! Arrows are steps, in order. Nothing here checks the bytes: their size
//! and digest are checked by whoever keeps them, against what the host
//! published, so a resume that went wrong is a mismatch, never a file.
use super::{
    connector::ArtifactChannels,
    sftp::{ByteSink, SftpError, SftpSession, SinkWrite},
};
use crate::conversation::application::{
    ArtifactBytes, ArtifactChunk, ArtifactReadFailure, ARTIFACT_READ_RESUMES,
};
use std::{io, sync::Arc};
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
