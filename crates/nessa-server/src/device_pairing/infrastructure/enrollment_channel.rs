//! Retained complete-envelope progress over one original authenticated TLS owner.
use super::frames::{encode_frame, FrameReader, FrameTooLarge};
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
    reader: FrameReader,
    output: Option<Vec<u8>>,
    output_written: usize,
}
impl<S: Read + Write> EnrollmentChannel<S> {
    /// Move the original completed TLS/proof owner into its consuming frame owner.
    pub fn new(transport: NativeTransport<S>) -> Self {
        Self {
            transport,
            reader: FrameReader::new(MAX_ENROLLMENT_ENVELOPE_BYTES),
            output: None,
            output_written: 0,
        }
    }
    /// Give the completed TLS/proof owner back, for the protected product
    /// phase that follows an `openProduct` envelope. Any partial envelope
    /// progress is discarded with this framing owner.
    pub fn into_transport(self) -> NativeTransport<S> {
        self.transport
    }
    /// Borrow actual key/context evidence for Auth admission and confirmation.
    pub fn transport(&self) -> &NativeTransport<S> {
        &self.transport
    }
    /// Read one complete envelope. Partial prefix/body progress survives WouldBlock.
    /// An oversized prefix is refused before a body read or allocation; repeated
    /// calls retain that refusal. Syntax remains the current wire codec's owner.
    pub fn receive_envelope(&mut self) -> Result<Vec<u8>, NativeFrameError> {
        loop {
            let buffer = self.reader.unfilled().map_err(too_large)?;
            let count = match self.transport.read(buffer) {
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                other => other.map_err(|error| NativeFrameError::Io(error.kind()))?,
            };
            if count == 0 {
                return Err(NativeFrameError::Io(ErrorKind::UnexpectedEof));
            }
            if let Some(body) = self.reader.filled(count).map_err(too_large)? {
                return Ok(body);
            }
        }
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
            self.output =
                Some(encode_frame(MAX_ENROLLMENT_ENVELOPE_BYTES, bytes).map_err(too_large)?);
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

fn too_large(_: FrameTooLarge) -> NativeFrameError {
    NativeFrameError::Wire(NativeWireError::TooLarge)
}
