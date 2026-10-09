//! Lease frames over a byte stream, both ways: [`FrameStream`] reads one
//! body at a time and [`write_frame`] writes one frame. The frames
//! themselves are `nessa_protocol::lease`'s.
use nessa_protocol::{
    lease::{encode, MAX_FRAME_BYTES},
    pairing::FrameReader,
};
use serde::Serialize;
use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Frame bodies read from a stream. Cancellation-safe: a read cancelled
/// part-way keeps what it read for the next one.
pub(crate) struct FrameStream<R> {
    reader: R,
    frames: FrameReader,
}

impl<R: AsyncRead + Unpin> FrameStream<R> {
    pub(crate) fn new(reader: R) -> Self {
        Self {
            reader,
            frames: FrameReader::new(MAX_FRAME_BYTES),
        }
    }

    /// The next frame's body, or `None` once the stream ended, mid-frame
    /// included: what an ended stream cut short was never sent.
    ///
    /// # Errors
    /// A read that failed, or a frame announced larger than the bound, after
    /// which the stream is no longer framed.
    pub(crate) async fn next(&mut self) -> io::Result<Option<Vec<u8>>> {
        loop {
            let buffer = self
                .frames
                .unfilled()
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "lease frame too large"))?;
            let read = self.reader.read(buffer).await?;
            if read == 0 {
                return Ok(None);
            }
            if let Some(body) = self
                .frames
                .filled(read)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "lease frame too large"))?
            {
                return Ok(Some(body));
            }
        }
    }
}

/// Write `frame` and flush it.
///
/// # Errors
/// A frame too large to send, or a write that failed.
pub(crate) async fn write_frame<W: AsyncWrite + Unpin, T: Serialize>(
    writer: &mut W,
    frame: &T,
) -> io::Result<()> {
    let bytes = encode(frame).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    writer.write_all(&bytes).await?;
    writer.flush().await
}
