//! The deadline-and-wake socket both ends of a native pairing connection use.
//!
//! A connection runs in phases, each with its own deadline: the TLS handshake
//! ([`TLS_DEADLINE`]) and then enrollment ([`ENROLLMENT_DEADLINE`]). The
//! deadline is read from an injected monotonic [`Clock`], and a wake from the
//! connection's owner stops a blocked read within one [`WAKE_TICK`].
use super::wake::{WakeEndpoint, WAKE_TICK};
use crate::clock::Clock;
use std::{
    io::{Error, ErrorKind, Read, Result as IoResult, Write},
    net::TcpStream,
    sync::{Arc, Mutex},
    task::Waker,
    time::Duration,
};

/// How long the TLS handshake phase may last.
pub const TLS_DEADLINE: Duration = Duration::from_secs(10);
/// How long the enrollment phase may last once it begins.
pub const ENROLLMENT_DEADLINE: Duration = Duration::from_secs(30);

/// Why the deadline socket refused: the socket's own error category, or a
/// call that does not apply in the stream's current phase.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeadlineError {
    /// The socket operation failed, or the phase deadline passed (`TimedOut`).
    Io(ErrorKind),
    /// The call does not apply once the socket was moved to an async owner.
    Phase,
}

pub struct NativeDeadline {
    clock: Arc<dyn Clock>,
    expires_ms: Mutex<u64>,
}
impl NativeDeadline {
    fn remaining(&self) -> IoResult<Duration> {
        self.expires_ms
            .lock()
            .map_err(|_| Error::other("native deadline owner unavailable"))?
            .checked_sub(self.clock.elapsed_ms())
            .filter(|milliseconds| *milliseconds > 0)
            .map(Duration::from_millis)
            .ok_or_else(|| Error::from(ErrorKind::TimedOut))
    }
}
/// The socket calls a `DeadlineStream` makes. `TcpStream` is the real one;
/// tests substitute a recording double.
pub trait NativeSocket: Read + Write {
    fn set_nonblocking(&self, nonblocking: bool) -> IoResult<()>;
    fn set_read_timeout(&self, timeout: Option<Duration>) -> IoResult<()>;
    fn set_write_timeout(&self, timeout: Option<Duration>) -> IoResult<()>;
}
impl NativeSocket for TcpStream {
    fn set_nonblocking(&self, nonblocking: bool) -> IoResult<()> {
        TcpStream::set_nonblocking(self, nonblocking)
    }
    fn set_read_timeout(&self, timeout: Option<Duration>) -> IoResult<()> {
        TcpStream::set_read_timeout(self, timeout)
    }
    fn set_write_timeout(&self, timeout: Option<Duration>) -> IoResult<()> {
        TcpStream::set_write_timeout(self, timeout)
    }
}

/// A native socket bounded by its phase deadline and by its owner's wake.
///
/// Reads cap each OS wait at `WAKE_TICK` and retry a wait that ended without
/// data, checking the wake flag and the deadline between waits, so a worker
/// blocked reading fails with ConnectionAborted within one tick on every OS
/// (`native_shutdown_keeps_original_physical_capacity`). No bytes are consumed
/// by a timed-out read, so retrying it is safe.
///
/// A send is never retried: after a timed-out send the transport may have
/// taken part of the buffer (Winsock calls the connection indeterminate), so
/// neither resending nor continuing is safe. A write checks the flag, then
/// makes one send with the rest of the phase deadline as its timeout. If it
/// times out, the stream fails for good with TimedOut and nothing touches the
/// socket again (`timed_out_send_is_terminal_and_never_retried`). A worker
/// blocked in a send is therefore not woken by the flag; it ends at its
/// deadline.
///
/// A protected product session moves the socket out ([`Self::take_socket`])
/// and the TLS state then reads and writes in-memory [`BufferedIo`], which the
/// async owner fills from and drains to that socket.
pub struct DeadlineStream<S: NativeSocket = TcpStream> {
    io: NativeIo<S>,
    deadline: Arc<NativeDeadline>,
    wake: Arc<WakeEndpoint>,
    /// Set by a timed-out send; every later call fails with it.
    failed: Option<ErrorKind>,
}
enum NativeIo<S> {
    Socket(S),
    Buffered(BufferedIo),
}
/// Ciphertext between the TLS state and the async socket owner. Reads with
/// nothing buffered are `WouldBlock`, never end of stream, until the socket
/// itself ended.
#[derive(Default)]
pub struct BufferedIo {
    inbound: Vec<u8>,
    inbound_read: usize,
    ended: bool,
    outbound: Vec<u8>,
    outbound_written: usize,
}
impl BufferedIo {
    /// Ciphertext the socket delivered.
    pub fn receive(&mut self, bytes: &[u8]) {
        if self.inbound_read == self.inbound.len() {
            self.inbound.clear();
            self.inbound_read = 0;
        }
        self.inbound.extend_from_slice(bytes);
    }
    /// The socket reached end of stream.
    pub fn end(&mut self) {
        self.ended = true;
    }
    /// Ciphertext not yet written to the socket.
    pub fn unsent(&self) -> &[u8] {
        &self.outbound[self.outbound_written..]
    }
    /// Record that the socket accepted `count` bytes of [`Self::unsent`].
    pub fn sent(&mut self, count: usize) {
        self.outbound_written += count;
        if self.outbound_written == self.outbound.len() {
            self.outbound.clear();
            self.outbound_written = 0;
        }
    }
    fn read(&mut self, bytes: &mut [u8]) -> IoResult<usize> {
        let available = &self.inbound[self.inbound_read..];
        if available.is_empty() {
            return if self.ended {
                Ok(0)
            } else {
                Err(Error::from(ErrorKind::WouldBlock))
            };
        }
        let count = available.len().min(bytes.len());
        bytes[..count].copy_from_slice(&available[..count]);
        self.inbound_read += count;
        Ok(count)
    }
}
impl<S: NativeSocket> DeadlineStream<S> {
    pub fn new(
        stream: S,
        clock: Arc<dyn Clock>,
        wake: Arc<WakeEndpoint>,
    ) -> Result<(Self, Arc<NativeDeadline>), DeadlineError> {
        let expires_ms = clock
            .elapsed_ms()
            .checked_add(TLS_DEADLINE.as_millis() as u64)
            .ok_or(DeadlineError::Io(ErrorKind::InvalidData))?;
        let deadline = Arc::new(NativeDeadline {
            clock,
            expires_ms: Mutex::new(expires_ms),
        });
        Ok((
            Self {
                io: NativeIo::Socket(stream),
                deadline: deadline.clone(),
                wake,
                failed: None,
            },
            deadline,
        ))
    }
    pub fn blocking(&self) -> Result<(), DeadlineError> {
        match &self.io {
            NativeIo::Socket(stream) => stream
                .set_nonblocking(false)
                .map_err(|error| DeadlineError::Io(error.kind())),
            NativeIo::Buffered(_) => Err(DeadlineError::Phase),
        }
    }
    /// Have the async `reader` woken when the owner wakes this socket; `true`
    /// if it already was, and its IO must stop.
    pub fn wake_reader(&self, reader: &Waker) -> bool {
        self.wake.wake_reader(reader)
    }
    /// Move the socket out for an async owner; this stream then buffers.
    /// `None` if it was already taken.
    pub fn take_socket(&mut self) -> Option<S> {
        match std::mem::replace(&mut self.io, NativeIo::Buffered(BufferedIo::default())) {
            NativeIo::Socket(stream) => Some(stream),
            buffered => {
                self.io = buffered;
                None
            }
        }
    }
    /// The in-memory ciphertext, once the socket was taken.
    pub fn buffered(&mut self) -> Option<&mut BufferedIo> {
        match &mut self.io {
            NativeIo::Buffered(io) => Some(io),
            NativeIo::Socket(_) => None,
        }
    }
    /// The time left in the phase: refused once the stream has failed, once
    /// woken, or past the deadline.
    fn usable_for(&self) -> IoResult<Duration> {
        if let Some(kind) = self.failed {
            return Err(Error::from(kind));
        }
        if self.wake.woken() {
            return Err(Error::from(ErrorKind::ConnectionAborted));
        }
        self.deadline.remaining()
    }
}
/// An OS wait that ended without data: WouldBlock on Unix, TimedOut on Windows.
fn wait_elapsed(error: &Error) -> bool {
    matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut)
}
impl<S: NativeSocket> Read for DeadlineStream<S> {
    fn read(&mut self, bytes: &mut [u8]) -> IoResult<usize> {
        loop {
            // Buffered: the protected phase has no phase deadline; its owner
            // polls the wake flag, and refuses here once woken.
            let wait = match &mut self.io {
                NativeIo::Buffered(io) => {
                    if self.wake.woken() {
                        return Err(Error::from(ErrorKind::ConnectionAborted));
                    }
                    return io.read(bytes);
                }
                NativeIo::Socket(_) => self.usable_for()?.min(WAKE_TICK),
            };
            let NativeIo::Socket(stream) = &mut self.io else {
                unreachable!("buffered reads returned above");
            };
            stream.set_read_timeout(Some(wait))?;
            match stream.read(bytes) {
                Err(error) if wait_elapsed(&error) => continue,
                result => return result,
            }
        }
    }
}
impl<S: NativeSocket> Write for DeadlineStream<S> {
    fn write(&mut self, bytes: &[u8]) -> IoResult<usize> {
        if let NativeIo::Buffered(io) = &mut self.io {
            if self.wake.woken() {
                return Err(Error::from(ErrorKind::ConnectionAborted));
            }
            io.outbound.extend_from_slice(bytes);
            return Ok(bytes.len());
        }
        let wait = self.usable_for()?;
        let NativeIo::Socket(stream) = &mut self.io else {
            unreachable!("buffered writes returned above");
        };
        stream.set_write_timeout(Some(wait))?;
        match stream.write(bytes) {
            Err(error) if wait_elapsed(&error) => {
                self.failed = Some(ErrorKind::TimedOut);
                Err(Error::from(ErrorKind::TimedOut))
            }
            result => result,
        }
    }
    fn flush(&mut self) -> IoResult<()> {
        if let NativeIo::Buffered(_) = &self.io {
            return Ok(());
        }
        self.usable_for()?;
        let NativeIo::Socket(stream) = &mut self.io else {
            unreachable!("buffered flushes returned above");
        };
        stream.flush()
    }
}
/// Give the connection the enrollment phase's deadline, counted from now.
pub fn begin_enrollment_phase(deadline: &NativeDeadline) -> Result<(), DeadlineError> {
    check_deadline(deadline)?;
    let expires_ms = deadline
        .clock
        .elapsed_ms()
        .checked_add(ENROLLMENT_DEADLINE.as_millis() as u64)
        .ok_or(DeadlineError::Io(ErrorKind::InvalidData))?;
    *deadline
        .expires_ms
        .lock()
        .map_err(|_| DeadlineError::Io(ErrorKind::Other))? = expires_ms;
    Ok(())
}
/// Refuse once the phase deadline has passed.
pub fn check_deadline(deadline: &NativeDeadline) -> Result<(), DeadlineError> {
    deadline
        .remaining()
        .map(|_| ())
        .map_err(|error| DeadlineError::Io(error.kind()))
}

#[cfg(test)]
#[path = "../../../tests/pairing/socket/deadline_stream.rs"]
mod tests;
