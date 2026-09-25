use super::{envelope::MAX_JSON_ITEMS, parse_within, protocol, Envelope};
use crate::application::agent_execution::agents::AgentError;
use event_stream::ingestion::{
    CrLfPolicy, DecodeBudget, DecodeState, FinalLinePolicy, IncrementalDecoder, NewlineFramer,
    NewlineFramerConfig,
};
use serde_json::Value;
use std::time::Duration;
#[cfg(unix)]
use std::{io, os::fd::AsRawFd};
#[cfg(windows)]
use std::{io, os::windows::io::AsRawHandle, ptr};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    time::Instant,
};
#[cfg(windows)]
use windows_sys::Win32::System::Pipes::PeekNamedPipe;

/// All partial input lives on the reader so cancelling `next` cannot lose bytes.
pub(crate) struct Reader<R> {
    input: R,
    framer: NewlineFramer,
    bytes: [u8; 8192],
    offset: usize,
    length: usize,
    limit: usize,
    /// Values and object keys one frame may hold.
    json_items: usize,
    failure: Option<AgentError>,
    decoding_yielded: bool,
    frame_in_progress: bool,
}
impl<R: AsyncRead + Unpin> Reader<R> {
    pub(crate) fn new(input: R, limit: usize) -> Self {
        Self {
            input,
            framer: NewlineFramer::new(NewlineFramerConfig {
                max_frame_bytes: limit,
                emit_empty_frames: false,
                crlf: CrLfPolicy::PreserveCarriageReturn,
                final_line: FinalLinePolicy::RejectUnterminated,
            })
            .expect("validated nonzero frame limit"),
            bytes: [0; 8192],
            offset: 0,
            length: 0,
            limit,
            json_items: MAX_JSON_ITEMS,
            failure: None,
            decoding_yielded: false,
            frame_in_progress: false,
        }
    }
    /// Take frames holding up to `items` values and keys from the next one
    /// on, for a connection about to read one answer larger than the rest of
    /// the protocol sends. The protocol's own bound is [`MAX_JSON_ITEMS`].
    pub(crate) fn allow_json_items(&mut self, items: usize) {
        self.json_items = items;
    }
    /// A pending decode yielded for fairness rather than waiting for provider input.
    pub(crate) fn decoding_yielded(&self) -> bool {
        self.decoding_yielded
    }
    /// Bytes for the next frame were observed, so Pending means an incomplete
    /// frame rather than an empty transport boundary.
    pub(crate) fn frame_in_progress(&self) -> bool {
        self.frame_in_progress
    }
    fn frame_in_progress_after_consuming(current: bool, bytes: &[u8]) -> bool {
        if let Some(last_newline) = bytes.iter().rposition(|byte| *byte == b'\n') {
            last_newline + 1 < bytes.len()
        } else {
            current || !bytes.is_empty()
        }
    }
    pub(crate) async fn next(&mut self) -> Result<Envelope, AgentError> {
        self.decoding_yielded = false;
        loop {
            if let Some(error) = &self.failure {
                return Err(error.clone());
            }
            let budget = DecodeBudget {
                max_items: 1,
                max_bytes: self.limit,
                max_work_units: self.bytes.len(),
            };
            if self.offset == self.length {
                // The previous decode may have stopped at its one-item budget
                // after retaining later complete frames internally. Drain those
                // frames before consulting transport readiness.
                let step = self.framer.decode(&[], budget);
                if matches!(step.state, DecodeState::Failed(_)) {
                    self.failure = Some(protocol("frame exceeds configured limit"));
                }
                if let Some(frame) = step.items.into_iter().next() {
                    if !frame.item.as_bytes().iter().all(u8::is_ascii_whitespace) {
                        return parse_within(frame.item.as_bytes(), self.json_items);
                    }
                }
                self.length = self
                    .input
                    .read(&mut self.bytes)
                    .await
                    .map_err(|_| AgentError::Transport("stdout read failed".into()))?;
                self.offset = 0;
                if self.length == 0 {
                    let step = self.framer.finish(budget);
                    let error = if matches!(step.state, DecodeState::Failed(_)) {
                        protocol("unterminated JSON-RPC frame")
                    } else {
                        AgentError::Transport("provider stdout closed".into())
                    };
                    self.failure = Some(error.clone());
                    return Err(error);
                }
                self.frame_in_progress = true;
            }
            // The framer can consume bytes belonging to the following frame
            // while returning the preceding one. Track the consumed prefix,
            // including bytes retained privately by the framer.
            let available = &self.bytes[self.offset..self.length];
            let step = self.framer.decode(available, budget);
            let consumed = step.consumed_bytes;
            let frame_in_progress = Self::frame_in_progress_after_consuming(
                self.frame_in_progress,
                &available[..consumed],
            );
            self.offset += step.consumed_bytes;
            self.frame_in_progress = frame_in_progress;
            if matches!(step.state, DecodeState::Failed(_)) {
                self.failure = Some(protocol("frame exceeds configured limit"));
            }
            if let Some(frame) = step.items.into_iter().next() {
                if !frame.item.as_bytes().iter().all(u8::is_ascii_whitespace) {
                    return parse_within(frame.item.as_bytes(), self.json_items);
                }
            }
            // Even an uninterrupted stream of whitespace leaves closure responsive.
            self.decoding_yielded = true;
            tokio::task::yield_now().await;
            self.decoding_yielded = false;
        }
    }
}

impl Reader<tokio::process::ChildStdout> {
    /// Read bytes already present in the provider pipe without depending on the
    /// async reactor having delivered its readiness notification yet.
    #[cfg(unix)]
    pub(crate) fn read_ready_os_bytes(&mut self) -> Result<bool, AgentError> {
        if self.offset != self.length {
            return Ok(true);
        }
        loop {
            // SAFETY: `bytes` is writable for its full declared length and the
            // borrowed child stdout descriptor remains open for this call.
            let read = unsafe {
                libc::read(
                    self.input.as_raw_fd(),
                    self.bytes.as_mut_ptr().cast(),
                    self.bytes.len(),
                )
            };
            if read > 0 {
                self.offset = 0;
                self.length = read as usize;
                self.frame_in_progress = true;
                return Ok(true);
            }
            if read == 0 {
                let error = AgentError::Transport("provider stdout closed".into());
                self.failure = Some(error.clone());
                return Err(error);
            }
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            if error.kind() == io::ErrorKind::WouldBlock {
                return Ok(false);
            }
            return Err(AgentError::Transport("stdout read failed".into()));
        }
    }

    /// Observe bytes already present in a Windows provider pipe. `PeekNamedPipe`
    /// does not consume them; marking the frame in progress makes the worker await
    /// the continuously owned async read before dispatch.
    #[cfg(windows)]
    pub(crate) fn read_ready_os_bytes(&mut self) -> Result<bool, AgentError> {
        if self.offset != self.length {
            return Ok(true);
        }
        let mut available = 0;
        // SAFETY: the child stdout handle remains owned by `self.input`; every
        // optional output pointer except `available` is intentionally null.
        let result = unsafe {
            PeekNamedPipe(
                self.input.as_raw_handle().cast(),
                ptr::null_mut(),
                0,
                ptr::null_mut(),
                &mut available,
                ptr::null_mut(),
            )
        };
        if result == 0 {
            let source = io::Error::last_os_error();
            let error = AgentError::Transport(format!("stdout readiness probe failed: {source}"));
            self.failure = Some(error.clone());
            return Err(error);
        }
        if available == 0 {
            return Ok(false);
        }
        self.frame_in_progress = true;
        Ok(true)
    }
}

/// Encode and validate the exact outgoing frame before admission or other state changes.
/// The configured limit excludes the newline delimiter, matching inbound framing.
pub(crate) fn encode(value: Value, limit: usize) -> Result<Vec<u8>, AgentError> {
    let mut bytes = serde_json::to_vec(&value).map_err(|_| protocol("could not encode request"))?;
    if bytes.len() > limit {
        return Err(AgentError::InvalidInput(
            "encoded request exceeds frame limit".into(),
        ));
    }
    bytes.push(b'\n');
    Ok(bytes)
}

/// Time allowed to write any frame, however small.
const WRITE_BASE: Duration = Duration::from_secs(1);
/// Extra time allowed for each whole mebibyte of a frame.
const WRITE_PER_MIB: Duration = Duration::from_secs(1);

/// How long writing a frame of `frame_bytes` may take before it is a
/// [`AgentError::Deadline`]: one second, plus what [`large_frame_allowance`]
/// adds. Nothing under a mebibyte gets more than the one second every frame
/// always had.
///
/// A pipe holds tens of kibibytes, so writing a frame means waiting for the
/// peer to read it. One second is ample for the small frames that were the
/// only kind until a message could carry images; it is not an honest bound
/// for sixteen mebibytes read by an agent that parses as it goes. A peer
/// slower than one mebibyte a second is still treated as stalled. The
/// operation deadline passed to [`send_encoded`] still applies on top.
pub(crate) fn write_allowance(frame_bytes: usize) -> Duration {
    WRITE_BASE + large_frame_allowance(frame_bytes)
}

/// The part of [`write_allowance`] that grows with the frame: a second for
/// each whole mebibyte, and nothing below one. A fixed acknowledgement
/// deadline that has to cover the write as well is extended by this much.
///
/// Total for any length: a frame larger than a configured limit can ever be
/// still answers a duration rather than overflowing.
pub(crate) fn large_frame_allowance(frame_bytes: usize) -> Duration {
    let mebibytes = u32::try_from(frame_bytes / (1024 * 1024)).unwrap_or(u32::MAX);
    WRITE_PER_MIB.saturating_mul(mebibytes)
}

/// Write a frame already encoded and validated by `encode`, preserving its bytes.
pub(crate) async fn send_encoded<W: AsyncWrite + Unpin>(
    output: &mut W,
    bytes: &[u8],
    deadline: Duration,
    operation_deadline: Option<Instant>,
) -> Result<(), AgentError> {
    let write_deadline = Instant::now() + deadline;
    let deadline = operation_deadline.map_or(write_deadline, |limit| limit.min(write_deadline));
    tokio::select! { biased;
        _ = tokio::time::sleep_until(deadline) => Err(AgentError::Deadline),
        result = output.write_all(bytes) => result.map_err(|_| AgentError::Transport("stdin write failed".into())),
    }
}

#[cfg(test)]
#[path = "../../../tests/infrastructure/json_rpc/transport.rs"]
mod tests;
