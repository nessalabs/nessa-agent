//! The protected product phase of a native connection: after a first
//! `openProduct` envelope, the same TLS connection carries length-prefixed
//! product frames, served on the async runtime.
//!
//! ```text
//! tokio socket <--ciphertext--> BufferedIo <--> NativeTransport (TLS, proof)
//!                                                  <--frames--> product session
//! ```
//! Arrows are byte flow. The TLS state never touches the socket: the socket is
//! moved out of the blocking stream ([`DeadlineStream::take_socket`]) and
//! polled here, and TLS reads and writes in-memory ciphertext. This type owns
//! framing at the published directional bounds; it grants nothing. Who the peer is, and what it may read, is decided by the
//! product session's authentication and admission (design rows PR1, PR3).
use super::connection::DeadlineStream;
use super::frames::{encode_frame, FrameReader};
use crate::product::generated::MAX_RECORD_RESPONSE_BYTES;
use crate::protocol::MAX_PAYLOAD_BYTES;
use futures_util::{Sink, Stream};
use nessa_auth::{adapters::pairing::NativeTransport, application::pairing::DeviceConnectionProof};
use std::{
    future::Future,
    io::{Error, ErrorKind, Read, Result as IoResult, Write},
    pin::Pin,
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::TcpStream,
};

/// Largest product frame the gateway reads: the protocol's request bound.
pub const MAX_PROTECTED_REQUEST_BYTES: usize = MAX_PAYLOAD_BYTES as usize;
/// Largest product frame the gateway writes: the generated response bound.
pub const MAX_PROTECTED_RESPONSE_BYTES: usize = MAX_RECORD_RESPONSE_BYTES;
/// Socket reads one poll performs before yielding, so a peer that keeps the
/// socket readable cannot hold the task.
const READS_PER_POLL: usize = 8;
/// Ciphertext read from the socket at a time.
const SOCKET_READ_BYTES: usize = 16 * 1024;

/// The composed product side of protected native connections.
pub trait ProtectedSessions: Send + Sync {
    /// Serve one connection until its session ends. The connection's permit is
    /// held by the caller for the whole of the returned future.
    fn serve(&self, connection: ProtectedConnection) -> Pin<Box<dyn Future<Output = ()> + Send>>;
}

/// One protected native connection: a stream of incoming frame bodies and a
/// sink of outgoing ones. Incoming frames above [`MAX_PROTECTED_REQUEST_BYTES`]
/// end the stream before their body is read; outgoing frames above
/// [`MAX_PROTECTED_RESPONSE_BYTES`] are refused. Once its connection owner
/// wakes it for shutdown, the TLS stream refuses every further read and write
/// (`DeadlineStream`). The session loop polls this stream at least once per
/// current-state interval and its writer has write deadlines, so a woken
/// session ends without a timer of its own
/// (`protected_sessions_share_permits_and_are_woken_by_shutdown`).
pub struct ProtectedConnection {
    transport: NativeTransport<DeadlineStream>,
    socket: TcpStream,
    frames: FrameReader,
}
impl ProtectedConnection {
    /// Take the socket out of `transport` and register it with the runtime.
    pub(super) fn open(mut transport: NativeTransport<DeadlineStream>) -> IoResult<Self> {
        let socket = transport
            .stream_mut()
            .take_socket()
            .ok_or_else(|| Error::from(ErrorKind::NotConnected))?;
        socket.set_nonblocking(true)?;
        let socket = TcpStream::from_std(socket)?;
        Ok(Self {
            transport,
            socket,
            frames: FrameReader::new(MAX_PROTECTED_REQUEST_BYTES),
        })
    }
    /// The device key this connection's TLS handshake proved. Possession
    /// evidence only; the credential it may use is the registry's decision.
    pub fn device_proof(&self) -> &DeviceConnectionProof {
        self.transport.device_proof()
    }
    /// Write buffered ciphertext to the socket until none is left.
    fn poll_send(&mut self, context: &mut Context<'_>) -> Poll<IoResult<()>> {
        loop {
            let io = self
                .transport
                .stream_mut()
                .buffered()
                .expect("protected connections buffer");
            if io.unsent().is_empty() {
                return Poll::Ready(Ok(()));
            }
            match Pin::new(&mut self.socket).poll_write(context, io.unsent()) {
                Poll::Ready(Ok(0)) => return Poll::Ready(Err(Error::from(ErrorKind::WriteZero))),
                Poll::Ready(Ok(count)) => io.sent(count),
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
    fn poll_frame(&mut self, context: &mut Context<'_>) -> Poll<Option<IoResult<Vec<u8>>>> {
        for _ in 0..READS_PER_POLL {
            // Plaintext TLS already holds comes first.
            loop {
                let buffer = match self.frames.unfilled() {
                    Ok(buffer) => buffer,
                    Err(_) => return Poll::Ready(Some(Err(too_large()))),
                };
                match self.transport.read(buffer) {
                    Ok(0) => return Poll::Ready(None),
                    Ok(count) => match self.frames.filled(count) {
                        Ok(Some(body)) => return Poll::Ready(Some(Ok(body))),
                        Ok(None) => {}
                        Err(_) => return Poll::Ready(Some(Err(too_large()))),
                    },
                    Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                    Err(error) if error.kind() == ErrorKind::Interrupted => {}
                    Err(error) => return Poll::Ready(Some(Err(error))),
                }
            }
            // Reading may have produced TLS output (an alert, a key update).
            if let Poll::Ready(Err(error)) = self.poll_send(context) {
                return Poll::Ready(Some(Err(error)));
            }
            let mut bytes = [0; SOCKET_READ_BYTES];
            let mut buffer = ReadBuf::new(&mut bytes);
            match Pin::new(&mut self.socket).poll_read(context, &mut buffer) {
                Poll::Ready(Ok(())) => {
                    let io = self
                        .transport
                        .stream_mut()
                        .buffered()
                        .expect("protected connections buffer");
                    match buffer.filled() {
                        [] => io.end(),
                        received => io.receive(received),
                    }
                }
                Poll::Ready(Err(error)) => return Poll::Ready(Some(Err(error))),
                Poll::Pending => return Poll::Pending,
            }
        }
        context.waker().wake_by_ref();
        Poll::Pending
    }
}
impl Stream for ProtectedConnection {
    type Item = IoResult<Vec<u8>>;
    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.get_mut().poll_frame(context)
    }
}
impl Sink<Vec<u8>> for ProtectedConnection {
    type Error = Error;
    /// Ready once the previous frame's ciphertext has reached the socket, so at
    /// most one outgoing frame is buffered.
    fn poll_ready(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<IoResult<()>> {
        let this = self.get_mut();
        this.poll_send(context)
    }
    fn start_send(self: Pin<&mut Self>, body: Vec<u8>) -> IoResult<()> {
        let this = self.get_mut();
        let frame = encode_frame(MAX_PROTECTED_RESPONSE_BYTES, &body).map_err(|_| too_large())?;
        this.transport.write_all(&frame)?;
        this.transport.flush()
    }
    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<IoResult<()>> {
        let this = self.get_mut();
        match this.poll_send(context) {
            Poll::Ready(Ok(())) => Pin::new(&mut this.socket).poll_flush(context),
            other => other,
        }
    }
    fn poll_close(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<IoResult<()>> {
        let this = self.get_mut();
        match this.poll_send(context) {
            Poll::Ready(Ok(())) => Pin::new(&mut this.socket).poll_shutdown(context),
            other => other,
        }
    }
}

fn too_large() -> Error {
    Error::new(ErrorKind::InvalidData, "native frame exceeds its bound")
}
