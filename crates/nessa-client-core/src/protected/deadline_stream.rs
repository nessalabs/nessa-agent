//! The client socket under the native TLS connection: every physical read and
//! write asks the one absolute deadline and the cancellation first, and the
//! first refusal is kept as the connection's typed cause.
use crate::read_only_sync::application::{Cancellation, GatewayError, GatewayStream};
use nessa_protocol::clock::Clock;
use std::cell::Cell;
use std::io::{Error, ErrorKind, Read, Result as IoResult, Write};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

/// Longest one blocking wait before cancellation is asked again.
///
/// Shutting a cloned socket wakes a blocked receive on Unix. On Windows it
/// does not, so a stop during opening would otherwise sit until the handshake
/// deadline. Each slice asks again; `a_stop_during_a_long_wait_is_seen_on_the_next_slice`
/// keeps this bound.
pub(crate) const STOP_SLICE: Duration = Duration::from_millis(50);

pub(crate) struct DeadlineStream {
    stream: Box<dyn GatewayStream>,
    clock: Arc<dyn Clock>,
    cancellation: Arc<dyn Cancellation>,
    deadline: u64,
    failure: Rc<Cell<Option<GatewayError>>>,
    poll: Option<(u64, u64)>,
    partial_deadline: Option<u64>,
}
impl DeadlineStream {
    pub(crate) fn new(
        stream: Box<dyn GatewayStream>,
        clock: Arc<dyn Clock>,
        cancellation: Arc<dyn Cancellation>,
        deadline: u64,
    ) -> Self {
        Self {
            stream,
            clock,
            cancellation,
            deadline,
            failure: Rc::new(Cell::new(None)),
            poll: None,
            partial_deadline: None,
        }
    }
    pub(crate) fn remaining(&self) -> Result<Duration, GatewayError> {
        let result = self.budget();
        if let Err(error) = result {
            self.record_failure(error);
        }
        result
    }
    fn budget(&self) -> Result<Duration, GatewayError> {
        if let Some(error) = self.failure.get() {
            return Err(error);
        }
        if self.cancellation.cancelled() {
            return Err(GatewayError::Cancelled);
        }
        let now = self.clock.elapsed_ms();
        let deadline = self
            .partial_deadline
            .map_or(self.deadline, |partial| partial.min(self.deadline));
        // A cancellation time before the operation's deadline bounds the wait;
        // reaching it is a cancellation, not the operation timing out.
        if let Some(until) = self
            .cancellation
            .until_ms()
            .filter(|until| *until < deadline)
        {
            let remaining = until
                .checked_sub(now)
                .filter(|value| *value > 0)
                .ok_or(GatewayError::Cancelled)?;
            return Ok(Duration::from_millis(remaining));
        }
        let remaining = deadline
            .checked_sub(now)
            .filter(|value| *value > 0)
            .ok_or(GatewayError::TimedOut)?;
        Ok(Duration::from_millis(remaining))
    }
    pub(crate) fn begin_operation(&mut self, deadline: u64) {
        self.deadline = deadline;
        self.poll = None;
    }
    /// Polling bounds each physical wait without treating an idle tick as failure.
    /// Channel plaintext progress starts the total partial product-frame deadline;
    /// raw TLS control bytes do not. Later ticks do not reset that deadline.
    pub(crate) fn begin_poll(&mut self, deadline: Option<u64>, slice_ms: u64, frame_ms: u64) {
        self.deadline = deadline.unwrap_or(u64::MAX);
        self.poll = Some((self.clock.elapsed_ms().saturating_add(slice_ms), frame_ms));
    }
    pub(crate) fn complete_frame(&mut self) {
        self.partial_deadline = None;
    }
    pub(crate) fn note_partial(&mut self) -> Result<(), GatewayError> {
        if self.partial_deadline.is_none() {
            if let Some((_, frame_ms)) = self.poll {
                self.partial_deadline = Some(
                    self.clock
                        .elapsed_ms()
                        .checked_add(frame_ms)
                        .ok_or(GatewayError::TimedOut)?,
                );
            }
        }
        Ok(())
    }
    pub(crate) fn take_failure(&mut self) -> Option<GatewayError> {
        self.failure.take()
    }
    pub(crate) fn failure_owner(&self) -> Rc<Cell<Option<GatewayError>>> {
        self.failure.clone()
    }
    pub(crate) fn record_failure(&self, error: GatewayError) {
        if self.failure.get().is_none() {
            self.failure.set(Some(error));
            self.cancellation.record_failure(error);
        }
    }
    fn wait_budget(&mut self) -> IoResult<Duration> {
        let remaining = self.remaining().map_err(|error| self.refused(error))?;
        let poll_left = self
            .poll
            .map(|(until, _)| until.saturating_sub(self.clock.elapsed_ms()));
        if poll_left == Some(0) {
            return Err(ErrorKind::WouldBlock.into());
        }
        Ok(poll_left.map_or(remaining, |left| remaining.min(Duration::from_millis(left))))
    }
    fn physical<T>(&mut self, result: IoResult<T>) -> IoResult<T> {
        // Rustls may write a control reply during a read. Both directions of a
        // poll share its finite tick; buffers remain rustls-owned on WouldBlock.
        if self.poll.is_some()
            && result.as_ref().is_err_and(|error| {
                matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock)
            })
        {
            self.remaining().map_err(|error| self.refused(error))?;
            return Err(ErrorKind::WouldBlock.into());
        }
        result.inspect_err(|error| {
            // A wait the cancellation time cut short was cancelled, not timed out.
            let cause = match io_cause(error) {
                GatewayError::TimedOut
                    if self
                        .cancellation
                        .until_ms()
                        .is_some_and(|until| self.clock.elapsed_ms() >= until) =>
                {
                    GatewayError::Cancelled
                }
                cause => cause,
            };
            self.record_failure(cause);
        })
    }
    fn refused(&mut self, error: GatewayError) -> Error {
        self.record_failure(error);
        Error::other("gateway operation refused")
    }
    /// One direction of the budget, taken `STOP_SLICE` at a time. A slice that
    /// ends with no bytes is not the deadline; cancellation and the remaining
    /// budget are asked before the next slice.
    fn transfer<T>(
        &mut self,
        timeout: impl Fn(&mut dyn GatewayStream, Duration) -> IoResult<()>,
        mut operation: impl FnMut(&mut dyn GatewayStream) -> IoResult<T>,
    ) -> IoResult<T> {
        let mut left = None;
        loop {
            let available = self.wait_budget()?;
            let left = left.get_or_insert(available);
            let slice = (*left).min(available).min(STOP_SLICE);
            let prepared = timeout(&mut *self.stream, slice);
            self.physical(prepared)?;
            match operation(&mut *self.stream) {
                Err(error) if slice_elapsed(&error) => {
                    *left = left.saturating_sub(slice);
                    if left.is_zero() {
                        return self.physical(Err(error));
                    }
                }
                other => {
                    let value = self.physical(other)?;
                    self.remaining().map_err(|error| self.refused(error))?;
                    return Ok(value);
                }
            }
        }
    }
}
fn slice_elapsed(error: &Error) -> bool {
    matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock)
}
impl Read for DeadlineStream {
    fn read(&mut self, bytes: &mut [u8]) -> IoResult<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        self.transfer(
            |stream, timeout| stream.read_timeout(timeout),
            |stream| stream.read(bytes),
        )
    }
}
impl Write for DeadlineStream {
    fn write(&mut self, bytes: &[u8]) -> IoResult<usize> {
        self.transfer(
            |stream, timeout| stream.write_timeout(timeout),
            |stream| stream.write(bytes),
        )
    }
    fn flush(&mut self) -> IoResult<()> {
        self.transfer(
            |stream, timeout| stream.write_timeout(timeout),
            |stream| stream.flush(),
        )
    }
}
impl Drop for DeadlineStream {
    fn drop(&mut self) {
        let _ = self.stream.shutdown();
    }
}

/// The one classification of a physical I/O error. The peer ending the
/// connection is an untyped close (row W11) however the platform reports it:
/// a clean end of stream on one, a reset or abort (`WSAECONNRESET`,
/// `WSAECONNABORTED`) on Windows.
pub(crate) fn io_cause(error: &Error) -> GatewayError {
    match error.kind() {
        ErrorKind::TimedOut | ErrorKind::WouldBlock => GatewayError::TimedOut,
        ErrorKind::UnexpectedEof
        | ErrorKind::ConnectionReset
        | ErrorKind::ConnectionAborted
        | ErrorKind::BrokenPipe => GatewayError::Closed(None),
        _ => GatewayError::Transport,
    }
}
