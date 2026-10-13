//! The shared protected frame owner. Retained operations keep terminal deadlines;
//! product polling preserves partial TLS/frame progress across idle ticks.
mod deadline_stream;
use crate::read_only_sync::application::GatewayError;
pub(crate) use deadline_stream::{io_cause, DeadlineStream};
use nessa_auth::adapters::pairing::NativeTransport;
use nessa_protocol::pairing::{encode_frame, FrameReader, MAX_PROTECTED_REQUEST_BYTES};
use std::io::{Error, ErrorKind, Read, Write};

/// Product frames over the pinned TLS connection.
pub(crate) struct Channel {
    pub(crate) transport: NativeTransport<DeadlineStream>,
    pub(crate) frames: FrameReader,
}
impl Channel {
    pub(crate) fn send(&mut self, text: &str) -> Result<(), GatewayError> {
        let frame = encode_frame(MAX_PROTECTED_REQUEST_BYTES, text.as_bytes())
            .map_err(|_| GatewayError::RequestTooLarge)?;
        let result = self
            .transport
            .write_all(&frame)
            .and_then(|()| self.transport.flush());
        result.map_err(|error| self.io_failure(&error))
    }
    pub(crate) fn receive(&mut self) -> Result<String, GatewayError> {
        loop {
            let buffer = self
                .frames
                .unfilled()
                .map_err(|_| GatewayError::ResponseTooLarge)?;
            let count = match self.transport.read(buffer) {
                Ok(0) => return Err(GatewayError::Closed(None)),
                Ok(count) => count,
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(error) => return Err(self.io_failure(&error)),
            };
            if let Some(body) = self
                .frames
                .filled(count)
                .map_err(|_| GatewayError::ResponseTooLarge)?
            {
                return String::from_utf8(body).map_err(|_| GatewayError::Protocol);
            }
        }
    }
    /// One complete message or an idle tick. FrameReader and rustls retain their
    /// partial progress; DeadlineStream owns the total partial-frame budget.
    pub(crate) fn poll(&mut self) -> Result<Option<String>, GatewayError> {
        self.transport.stream_mut().remaining()?;
        loop {
            let buffer = self
                .frames
                .unfilled()
                .map_err(|_| GatewayError::ResponseTooLarge)?;
            let count = match self.transport.read(buffer) {
                Ok(0) => return Err(GatewayError::Closed(None)),
                Ok(count) => count,
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    self.transport.stream_mut().remaining()?;
                    return Ok(None);
                }
                Err(error) => return Err(self.io_failure(&error)),
            };
            self.transport.stream_mut().note_partial()?;
            self.transport.stream_mut().remaining()?;
            if let Some(body) = self
                .frames
                .filled(count)
                .map_err(|_| GatewayError::ResponseTooLarge)?
            {
                self.transport.stream_mut().complete_frame();
                return String::from_utf8(body)
                    .map(Some)
                    .map_err(|_| GatewayError::Protocol);
            }
            return Ok(None);
        }
    }
    /// The cause the stream recorded comes first (a deadline, cancellation,
    /// or physical error it saw); otherwise the error TLS returned, such as
    /// an end without TLS close, by the same classification.
    fn io_failure(&mut self, error: &Error) -> GatewayError {
        self.transport
            .stream_mut()
            .take_failure()
            .unwrap_or_else(|| io_cause(error))
    }
}

#[cfg(test)]
#[path = "../tests/protected/mod.rs"]
mod tests;
