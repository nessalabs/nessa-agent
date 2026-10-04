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

pub(super) struct DeadlineStream {
    stream: Box<dyn GatewayStream>,
    clock: Arc<dyn Clock>,
    cancellation: Arc<dyn Cancellation>,
    deadline: u64,
    failure: Rc<Cell<Option<GatewayError>>>,
}
impl DeadlineStream {
    pub(super) fn new(
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
        }
    }
    pub(super) fn remaining(&self) -> Result<Duration, GatewayError> {
        if self.cancellation.cancelled() {
            return Err(GatewayError::Cancelled);
        }
        let remaining = self
            .deadline
            .checked_sub(self.clock.elapsed_ms())
            .filter(|value| *value > 0)
            .ok_or(GatewayError::TimedOut)?;
        Ok(Duration::from_millis(remaining))
    }
    pub(super) fn begin_operation(&mut self, deadline: u64) {
        self.deadline = deadline;
    }
    pub(super) fn take_failure(&mut self) -> Option<GatewayError> {
        self.failure.take()
    }
    pub(super) fn failure_owner(&self) -> Rc<Cell<Option<GatewayError>>> {
        self.failure.clone()
    }
    fn record_failure(&self, error: GatewayError) {
        if self.failure.get().is_none() {
            self.failure.set(Some(error));
        }
    }
    fn physical<T>(&mut self, result: IoResult<T>) -> IoResult<T> {
        result.inspect_err(|error| {
            self.record_failure(io_cause(error));
        })
    }
    fn refused(&mut self, error: GatewayError) -> Error {
        self.record_failure(error);
        Error::other("gateway operation refused")
    }
}
impl Read for DeadlineStream {
    fn read(&mut self, bytes: &mut [u8]) -> IoResult<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        let timeout = self.remaining().map_err(|error| self.refused(error))?;
        let result = self.stream.read_timeout(timeout);
        self.physical(result)?;
        let result = self.stream.read(bytes);
        let read = self.physical(result)?;
        self.remaining().map_err(|error| self.refused(error))?;
        Ok(read)
    }
}
impl Write for DeadlineStream {
    fn write(&mut self, bytes: &[u8]) -> IoResult<usize> {
        let timeout = self.remaining().map_err(|error| self.refused(error))?;
        let result = self.stream.write_timeout(timeout);
        self.physical(result)?;
        let result = self.stream.write(bytes);
        let written = self.physical(result)?;
        self.remaining().map_err(|error| self.refused(error))?;
        Ok(written)
    }
    fn flush(&mut self) -> IoResult<()> {
        let timeout = self.remaining().map_err(|error| self.refused(error))?;
        let result = self.stream.write_timeout(timeout);
        self.physical(result)?;
        let result = self.stream.flush();
        self.physical(result)?;
        self.remaining().map_err(|error| self.refused(error))?;
        Ok(())
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
pub(super) fn io_cause(error: &Error) -> GatewayError {
    match error.kind() {
        ErrorKind::TimedOut | ErrorKind::WouldBlock => GatewayError::TimedOut,
        ErrorKind::UnexpectedEof
        | ErrorKind::ConnectionReset
        | ErrorKind::ConnectionAborted
        | ErrorKind::BrokenPipe => GatewayError::Closed(None),
        _ => GatewayError::Transport,
    }
}
