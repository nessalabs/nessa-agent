//! Immutable chunks stream one semantic checkpoint codec without an aggregate buffer.
use super::TranscriptError;
use std::{
    io::{self, Read, Write},
    sync::Arc,
};

/// Maximum retained byte allocation for one checkpoint chunk.
pub const MAX_TRANSCRIPT_CHECKPOINT_CHUNK_BYTES: usize = 1024 * 1024;

/// Ordered immutable chunks of the validated semantic continuation codec.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranscriptCheckpoint(Box<[Arc<[u8]>]>);
impl TranscriptCheckpoint {
    /// Persist every chunk in this order, atomically with the receiver's applied position.
    pub fn chunks(&self) -> impl ExactSizeIterator<Item = &[u8]> {
        self.0.iter().map(AsRef::as_ref)
    }
    /// Accept untrusted ordered chunks. Restore validates JSON, exact scope and full state.
    ///
    /// # Errors
    /// Refuses empty or oversized chunks before retaining their allocations.
    pub fn from_chunks(chunks: Vec<Vec<u8>>) -> Result<Self, TranscriptError> {
        if chunks.is_empty()
            || chunks.iter().any(|chunk| {
                chunk.is_empty() || chunk.len() > MAX_TRANSCRIPT_CHECKPOINT_CHUNK_BYTES
            })
        {
            return Err(TranscriptError::Checkpoint);
        }
        super::snapshot::decode::preflight_checkpoint(ChunkReader {
            chunks: &chunks,
            index: 0,
            offset: 0,
        })
        .map_err(|_| TranscriptError::Checkpoint)?;
        let mut output = ChunkWriter::new();
        for chunk in chunks {
            output
                .write_all(&chunk)
                .map_err(|_| TranscriptError::Checkpoint)?;
        }
        Ok(output.finish())
    }
    pub(super) fn reader(&self) -> ChunkReader<'_> {
        ChunkReader {
            chunks: &self.0,
            index: 0,
            offset: 0,
        }
    }
}
pub(super) struct ChunkWriter {
    chunks: Vec<Arc<[u8]>>,
    current: Vec<u8>,
}
impl ChunkWriter {
    pub fn new() -> Self {
        Self {
            chunks: Vec::new(),
            current: Vec::with_capacity(MAX_TRANSCRIPT_CHECKPOINT_CHUNK_BYTES),
        }
    }
    pub fn finish(mut self) -> TranscriptCheckpoint {
        if !self.current.is_empty() {
            self.chunks.push(Arc::from(self.current.into_boxed_slice()));
        }
        TranscriptCheckpoint(self.chunks.into_boxed_slice())
    }
}
impl Write for ChunkWriter {
    fn write(&mut self, mut bytes: &[u8]) -> io::Result<usize> {
        let written = bytes.len();
        while !bytes.is_empty() {
            let size = bytes
                .len()
                .min(MAX_TRANSCRIPT_CHECKPOINT_CHUNK_BYTES - self.current.len());
            self.current.extend_from_slice(&bytes[..size]);
            bytes = &bytes[size..];
            if self.current.len() == MAX_TRANSCRIPT_CHECKPOINT_CHUNK_BYTES {
                self.chunks.push(Arc::from(
                    std::mem::replace(
                        &mut self.current,
                        Vec::with_capacity(MAX_TRANSCRIPT_CHECKPOINT_CHUNK_BYTES),
                    )
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
