use crate::app::ports::Clock;
use crate::read_only_sync::application::{Cancellation, GatewayError, GatewayStream};
use std::{
    cell::Cell,
    io::{self, Read, Write},
    rc::Rc,
    sync::Arc,
    time::Duration,
};

pub(super) struct DeadlineStream {
    stream: Box<dyn GatewayStream>,
    clock: Arc<dyn Clock>,
    cancellation: Arc<dyn Cancellation>,
    deadline: u64,
    upgrade_remaining: Option<usize>,
    failure: Rc<Cell<Option<GatewayError>>>,
}
impl DeadlineStream {
    pub(super) fn new(
        stream: Box<dyn GatewayStream>,
        clock: Arc<dyn Clock>,
        cancellation: Arc<dyn Cancellation>,
        deadline: u64,
        upgrade_bytes: usize,
    ) -> Self {
        Self {
            stream,
            clock,
            cancellation,
            deadline,
            upgrade_remaining: Some(upgrade_bytes),
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
    pub(super) fn finish_upgrade(&mut self) {
        self.upgrade_remaining = None;
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
    fn physical<T>(&mut self, result: io::Result<T>) -> io::Result<T> {
        result.inspect_err(|error| {
            self.record_failure(io_cause(error));
        })
    }
    fn refused(&mut self, error: GatewayError) -> io::Error {
        self.record_failure(error);
        io::Error::other("gateway operation refused")
    }
}
impl Read for DeadlineStream {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        let timeout = self.remaining().map_err(|error| self.refused(error))?;
        let count = match self.upgrade_remaining {
            Some(0) => return Err(self.refused(GatewayError::UpgradeTooLarge)),
            Some(remaining) => bytes.len().min(remaining),
            None => bytes.len(),
        };
        let result = self.stream.read_timeout(timeout);
        self.physical(result)?;
        let result = self.stream.read(&mut bytes[..count]);
        let read = self.physical(result)?;
        if let Some(remaining) = self.upgrade_remaining.as_mut() {
            *remaining -= read;
        }
        self.remaining().map_err(|error| self.refused(error))?;
        Ok(read)
    }
}
impl Write for DeadlineStream {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let timeout = self.remaining().map_err(|error| self.refused(error))?;
        let result = self.stream.write_timeout(timeout);
        self.physical(result)?;
        let result = self.stream.write(bytes);
        let written = self.physical(result)?;
        self.remaining().map_err(|error| self.refused(error))?;
        Ok(written)
    }
    fn flush(&mut self) -> io::Result<()> {
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

pub(super) fn io_cause(error: &io::Error) -> GatewayError {
    match error.kind() {
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => GatewayError::TimedOut,
        _ => GatewayError::Transport,
    }
}
