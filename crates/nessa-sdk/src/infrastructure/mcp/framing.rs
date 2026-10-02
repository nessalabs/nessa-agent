//! Newline-delimited JSON-RPC frames, bounded before they are buffered.
use super::McpError;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

/// The largest frame read from or written to a server or a stand-in, without
/// its newline: 16 MiB, what an ACP binding allows the frames it writes.
pub(crate) const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

/// How reading the next frame ended without one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FrameEnd {
    /// The peer closed its side, or reading failed.
    Closed,
    /// A frame grew past its bound before its newline arrived.
    TooLarge,
}

/// Reads one frame at a time. A partly read frame is kept on the reader, so
/// dropping [`Frames::next`] in a `select!` loses nothing.
pub(crate) struct Frames<R> {
    input: BufReader<R>,
    partial: Vec<u8>,
    limit: usize,
}
impl<R: AsyncRead + Unpin> Frames<R> {
    pub(crate) fn new(input: R, limit: usize) -> Self {
        Self {
            input: BufReader::new(input),
            partial: Vec::new(),
            limit,
        }
    }
    /// The next non-blank frame's bytes.
    pub(crate) async fn next(&mut self) -> Result<Vec<u8>, FrameEnd> {
        loop {
            let available = self.input.fill_buf().await.map_err(|_| FrameEnd::Closed)?;
            if available.is_empty() {
                return Err(FrameEnd::Closed);
            }
            let (taken, complete) = match available.iter().position(|byte| *byte == b'\n') {
                Some(end) => (end, true),
                None => (available.len(), false),
            };
            if self.partial.len() + taken > self.limit {
                return Err(FrameEnd::TooLarge);
            }
            self.partial.extend_from_slice(&available[..taken]);
            self.input.consume(taken + usize::from(complete));
            if complete {
                let frame = std::mem::take(&mut self.partial);
                if !frame.iter().all(u8::is_ascii_whitespace) {
                    return Ok(frame);
                }
            }
        }
    }
}

/// `value` as one frame: its JSON and a newline.
///
/// # Errors
///
/// [`McpError::TooLarge`] past [`MAX_FRAME_BYTES`].
pub(crate) fn encode(value: &Value) -> Result<Vec<u8>, McpError> {
    let mut bytes =
        serde_json::to_vec(value).map_err(|_| McpError::Malformed("unencodable".into()))?;
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(McpError::TooLarge("a JSON-RPC frame"));
    }
    bytes.push(b'\n');
    Ok(bytes)
}

/// Write one encoded frame and flush it.
pub(crate) async fn write<W: AsyncWrite + Unpin>(
    output: &mut W,
    frame: &[u8],
) -> std::io::Result<()> {
    output.write_all(frame).await?;
    output.flush().await
}
