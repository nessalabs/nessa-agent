use super::{parse, protocol, Envelope};
use crate::application::agent_execution::agents::AgentError;
use event_stream::ingestion::{
    CrLfPolicy, DecodeBudget, DecodeState, FinalLinePolicy, IncrementalDecoder, NewlineFramer,
    NewlineFramerConfig,
};
use serde_json::Value;
use std::time::Duration;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    time::Instant,
};

/// All partial input lives on the reader so cancelling `next` cannot lose bytes.
pub(crate) struct Reader<R> {
    input: R,
    framer: NewlineFramer,
    bytes: [u8; 8192],
    offset: usize,
    length: usize,
    limit: usize,
    failure: Option<AgentError>,
    decoding_yielded: bool,
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
            failure: None,
            decoding_yielded: false,
        }
    }
    /// A pending decode yielded for fairness rather than waiting for provider input.
    pub(crate) fn decoding_yielded(&self) -> bool {
        self.decoding_yielded
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
            }
            let step = self
                .framer
                .decode(&self.bytes[self.offset..self.length], budget);
            self.offset += step.consumed_bytes;
            if matches!(step.state, DecodeState::Failed(_)) {
                self.failure = Some(protocol("frame exceeds configured limit"));
            }
            if let Some(frame) = step.items.into_iter().next() {
                if !frame.item.as_bytes().iter().all(u8::is_ascii_whitespace) {
                    return parse(frame.item.as_bytes());
                }
            }
            // Even an uninterrupted stream of whitespace leaves closure responsive.
            self.decoding_yielded = true;
            tokio::task::yield_now().await;
            self.decoding_yielded = false;
        }
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
