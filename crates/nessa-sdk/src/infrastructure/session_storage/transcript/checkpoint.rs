//! Immutable chunks stream one semantic checkpoint codec without an aggregate buffer.
use super::super::snapshot::decode::SchemaMarker;
use super::TranscriptError;
use crate::application::agent_execution::sessions::StorageError;
use std::io::{self, ErrorKind, Read, Write};
use std::sync::Arc;

/// Map a checkpoint read failure. Another format version keeps its typed
/// refusal. Every other read failure is a malformed checkpoint.
pub(super) fn storage_refusal(error: StorageError) -> TranscriptError {
    match error {
        StorageError::AnotherVersion { .. } => TranscriptError::Decision(error),
        _ => TranscriptError::Checkpoint,
    }
}

/// Maximum retained byte allocation for one checkpoint chunk.
pub const MAX_TRANSCRIPT_CHECKPOINT_CHUNK_BYTES: usize = 1024 * 1024;

/// Ordered immutable chunks of the validated semantic continuation codec.
///
/// `marker` is decided when the chunks are encoded or accepted. Restore uses
/// it and does not walk the bytes a second time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranscriptCheckpoint {
    chunks: Box<[Arc<[u8]>]>,
    marker: SchemaMarker,
}
impl TranscriptCheckpoint {
    /// Persist every chunk in this order, atomically with the receiver's applied position.
    pub fn chunks(&self) -> impl ExactSizeIterator<Item = &[u8]> {
        self.chunks.iter().map(AsRef::as_ref)
    }
    /// Accept untrusted ordered chunks. Restore validates JSON, exact scope and full state.
    ///
    /// # Errors
    /// Refuses empty or oversized chunks before retaining their allocations.
    /// A checkpoint whose `schemaVersion` is another unsigned integer is
    /// [`TranscriptError::Decision`] carrying
    /// [`StorageError::AnotherVersion`]. A marker that is not an unsigned
    /// integer is [`TranscriptError::Checkpoint`]. An absent marker is
    /// accepted here; restore reads it as this build's shape, or refuses it
    /// as another version when that read fails.
    pub fn from_chunks(chunks: Vec<Vec<u8>>) -> Result<Self, TranscriptError> {
        if chunks.is_empty()
            || chunks.iter().any(|chunk| {
                chunk.is_empty() || chunk.len() > MAX_TRANSCRIPT_CHECKPOINT_CHUNK_BYTES
            })
        {
            return Err(TranscriptError::Checkpoint);
        }
        let marker = super::snapshot::decode::preflight_checkpoint(ChunkReader {
            chunks: &chunks,
            index: 0,
            offset: 0,
        })
        .map_err(storage_refusal)?;
        let mut output = ChunkWriter::new();
        for chunk in chunks {
            output
                .write_all(&chunk)
                .map_err(|_| TranscriptError::Checkpoint)?;
        }
        let mut checkpoint = output.finish();
        checkpoint.marker = marker;
        Ok(checkpoint)
    }
    /// A body that did not decode. An unmarked checkpoint is the earlier
    /// shape. A current-version body is malformed.
    pub(super) fn unreadable_body(&self) -> TranscriptError {
        match self.marker {
            SchemaMarker::Unmarked => {
                TranscriptError::Decision(StorageError::AnotherVersion { found: None })
            }
            SchemaMarker::Current => TranscriptError::Checkpoint,
        }
    }
    pub(super) fn reader(&self) -> ChunkReader<'_> {
        ChunkReader {
            chunks: &self.chunks,
            index: 0,
            offset: 0,
        }
    }
}
pub(super) struct ChunkWriter {
    chunks: Vec<Arc<[u8]>>,
    current: Vec<u8>,
    limit: Option<usize>,
    written: usize,
    limit_exceeded: bool,
}
impl ChunkWriter {
    pub fn new() -> Self {
        Self::with_limit(None)
    }
    pub fn with_limit(limit: Option<usize>) -> Self {
        Self {
            chunks: Vec::new(),
            current: Vec::with_capacity(
                limit.map_or(MAX_TRANSCRIPT_CHECKPOINT_CHUNK_BYTES, |bytes| {
                    bytes.min(MAX_TRANSCRIPT_CHECKPOINT_CHUNK_BYTES)
                }),
            ),
            limit,
            written: 0,
            limit_exceeded: false,
        }
    }
    pub fn limit_exceeded(&self) -> bool {
        self.limit_exceeded
    }
    pub fn finish(mut self) -> TranscriptCheckpoint {
        if !self.current.is_empty() {
            self.chunks.push(Arc::from(self.current.into_boxed_slice()));
        }
        TranscriptCheckpoint {
            chunks: self.chunks.into_boxed_slice(),
            marker: SchemaMarker::Current,
        }
    }
}
impl Write for ChunkWriter {
    fn write(&mut self, mut bytes: &[u8]) -> io::Result<usize> {
        let written = bytes.len();
        let total = self.written.checked_add(written);
        if total.is_none_or(|total| self.limit.is_some_and(|limit| total > limit)) {
            self.limit_exceeded = true;
            return Err(ErrorKind::FileTooLarge.into());
        }
        self.written = total.expect("checkpoint byte addition was checked");
        while !bytes.is_empty() {
            let size = bytes
                .len()
                .min(MAX_TRANSCRIPT_CHECKPOINT_CHUNK_BYTES - self.current.len());
            self.current.extend_from_slice(&bytes[..size]);
            bytes = &bytes[size..];
            if self.current.len() == MAX_TRANSCRIPT_CHECKPOINT_CHUNK_BYTES {
                let capacity = self
                    .limit
                    .map_or(MAX_TRANSCRIPT_CHECKPOINT_CHUNK_BYTES, |limit| {
                        let retained = (self.chunks.len() + 1)
                            .saturating_mul(MAX_TRANSCRIPT_CHECKPOINT_CHUNK_BYTES);
                        limit
                            .saturating_sub(retained)
                            .min(MAX_TRANSCRIPT_CHECKPOINT_CHUNK_BYTES)
                    });
                self.chunks.push(Arc::from(
                    std::mem::replace(&mut self.current, Vec::with_capacity(capacity))
                        .into_boxed_slice(),
                ));
            }
        }
        Ok(written)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
pub(super) struct ChunkReader<'a, C = Arc<[u8]>> {
    chunks: &'a [C],
    index: usize,
    offset: usize,
}
impl<C: AsRef<[u8]>> Read for ChunkReader<'_, C> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        let Some(chunk) = self.chunks.get(self.index).map(AsRef::as_ref) else {
            return Ok(0);
        };
        let length = output.len().min(chunk.len() - self.offset);
        output[..length].copy_from_slice(&chunk[self.offset..self.offset + length]);
        self.offset += length;
        if self.offset == chunk.len() {
            self.index += 1;
            self.offset = 0;
        }
        Ok(length)
    }
}

#[cfg(test)]
#[path = "../../../../tests/infrastructure/session_storage/transcript_checkpoint_budget.rs"]
mod budget_tests;
