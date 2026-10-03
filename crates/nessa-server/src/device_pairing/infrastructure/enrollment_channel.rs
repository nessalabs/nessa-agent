//! Retained complete-envelope progress over one original authenticated TLS owner.
use super::wire::{NativeWireError, MAX_ENROLLMENT_ENVELOPE_BYTES};
use nessa_auth::adapters::pairing::NativeTransport;
use std::io::{ErrorKind, Read, Write};

/// Framing refusal preserves representation failures separately from physical IO.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeFrameError {
    /// The native complete-envelope publication refused its announced size.
    Wire(NativeWireError),
    /// Original IO category; WouldBlock retains progress for the next call.
    Io(ErrorKind),
}

/// Complete native enrollment envelopes on one actual completed TLS connection.
/// Client and listener consumers retain this owner through their exchange.
/// It does not construct authentication, enrollment, or product authority.
/// `native_framing_retains_partial_io_and_output` exercises retained IO progress.
pub struct EnrollmentChannel<S: Read + Write> {
    transport: NativeTransport<S>,
    prefix: [u8; 4],
    prefix_read: usize,
    body: Option<Vec<u8>>,
    body_read: usize,
    refused: Option<NativeWireError>,
    output: Option<Vec<u8>>,
    output_written: usize,
}
impl<S: Read + Write> EnrollmentChannel<S> {
    /// Move the original completed TLS/proof owner into its consuming frame owner.
    pub fn new(transport: NativeTransport<S>) -> Self {
        Self {
            transport,
            prefix: [0; 4],
            prefix_read: 0,
            body: None,
            body_read: 0,
            refused: None,
            output: None,
            output_written: 0,
        }
    }
    /// Borrow actual key/context evidence for Auth admission and confirmation.
    pub fn transport(&self) -> &NativeTransport<S> {
        &self.transport
    }
    /// Read one complete envelope. Partial prefix/body progress survives WouldBlock.
    /// An oversized prefix is refused before a body read or allocation; repeated
    /// calls retain that refusal. Syntax remains the current wire codec's owner.
    pub fn receive_envelope(&mut self) -> Result<Vec<u8>, NativeFrameError> {
        if let Some(error) = self.refused {
            return Err(NativeFrameError::Wire(error));
        }
        while self.prefix_read < self.prefix.len() {
            let count = match self.transport.read(&mut self.prefix[self.prefix_read..]) {
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                other => other.map_err(|error| NativeFrameError::Io(error.kind()))?,
            };
            if count == 0 {
                return Err(NativeFrameError::Io(ErrorKind::UnexpectedEof));
            }
            self.prefix_read += count;
        }
        if self.body.is_none() {
            let length = u32::from_be_bytes(self.prefix) as usize;
            let error = if length > MAX_ENROLLMENT_ENVELOPE_BYTES {
                Some(NativeWireError::TooLarge)
            } else {
                None
            };
            if let Some(error) = error {
                self.refused = Some(error);
                return Err(NativeFrameError::Wire(error));
            }
            self.body = Some(vec![0; length]);
        }
        let body = self.body.as_mut().expect("validated frame body");
        while self.body_read < body.len() {
            let count = match self.transport.read(&mut body[self.body_read..]) {
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                other => other.map_err(|error| NativeFrameError::Io(error.kind()))?,
            };
            if count == 0 {
                return Err(NativeFrameError::Io(ErrorKind::UnexpectedEof));
            }
            self.body_read += count;
        }
        self.prefix_read = 0;
        self.body_read = 0;
        Ok(self.body.take().expect("complete frame body"))
    }
    /// Write/flush one complete envelope. On WouldBlock, call again with the same
    /// bytes; a different envelope is refused while output is pending. The original
    /// offsets include accepted plaintext and pending TLS flush separately.
    pub fn send_envelope(&mut self, bytes: &[u8]) -> Result<(), NativeFrameError> {
        if let Some(output) = &self.output {
            if &output[4..] != bytes {
                return Err(NativeFrameError::Wire(NativeWireError::Invalid));
            }
        } else {
            if bytes.len() > MAX_ENROLLMENT_ENVELOPE_BYTES {
                return Err(NativeFrameError::Wire(NativeWireError::TooLarge));
            }
            let mut output = Vec::with_capacity(4 + bytes.len());
            output.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
            output.extend_from_slice(bytes);
            self.output = Some(output);
        }
        let output = self.output.as_ref().expect("owned frame output");
        while self.output_written < output.len() {
            let count = match self.transport.write(&output[self.output_written..]) {
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                other => other.map_err(|error| NativeFrameError::Io(error.kind()))?,
            };
            if count == 0 {
                return Err(NativeFrameError::Io(ErrorKind::WriteZero));
            }
            self.output_written += count;
        }
        loop {
            match self.transport.flush() {
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                other => other.map_err(|error| NativeFrameError::Io(error.kind()))?,
            }
            break;
        }
        self.output = None;
        self.output_written = 0;
        Ok(())
    }
}
