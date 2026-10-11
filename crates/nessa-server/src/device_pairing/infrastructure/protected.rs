//! The protected product phase of a native connection: after a first
//! `openProduct` envelope, the same TLS connection carries length-prefixed
//! product frames, served on the async runtime.
//!
//! ```text
//! tokio socket <--ciphertext--> BufferedIo <--> NativeTransport (TLS, proof)
//!                                                  <--frames--> product session
//! ```
//! Arrows are byte flow. The byte stream is [`TlsStream`]'s; this type owns
//! framing at the published directional bounds, and grants nothing. Who the
//! peer is, and what it may read, is decided by the product session's
//! authentication and admission (design rows PR1, PR3).
use super::tls_stream::TlsStream;
use futures_util::{Sink, Stream};
use nessa_auth::{
    adapters::pairing::NativeTransport,
    application::{pairing::DeviceConnectionProof, ports::AccessError},
};
use nessa_protocol::pairing::socket::DeadlineStream;
use nessa_protocol::pairing::{
    encode_frame, FrameReader, MAX_PROTECTED_REQUEST_BYTES, MAX_PROTECTED_RESPONSE_BYTES,
};
use std::{
    future::Future,
    io::{Error, ErrorKind, Result as IoResult},
    pin::Pin,
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::TcpStream,
};

/// The composed product side of protected native connections.
pub trait ProtectedSessions: Send + Sync {
    /// Whether a connection whose TLS handshake proved `proof` may take a
    /// product session permit: its key holds an active device credential for
    /// this gateway. Asked before the permit is taken, so peers that are not
    /// paired devices cannot hold the pool; the session still authenticates
    /// in full (design row PR15).
    fn admits(&self, proof: &DeviceConnectionProof) -> Result<bool, AccessError>;
    /// Serve one connection until its session ends. The connection has moved
    /// to the product pool and returned its connection permit; its product
    /// session permit is held by the caller for the whole of the returned
    /// future (design row PR10).
    fn serve(&self, connection: ProtectedConnection) -> Pin<Box<dyn Future<Output = ()> + Send>>;
}

/// One protected native connection: a stream of incoming frame bodies and a
/// sink of outgoing ones. Incoming frames above [`MAX_PROTECTED_REQUEST_BYTES`]
/// end the stream before their body is read; outgoing frames above
/// [`MAX_PROTECTED_RESPONSE_BYTES`] are refused. Once its connection owner
/// wakes it for shutdown, the reading task is woken at once and the TLS stream
/// refuses every further read and write (`DeadlineStream`), before or after
/// authentication (`protected_sessions_have_their_own_pool_and_are_woken_by_shutdown`).
///
/// The byte stream under it, and how its reads and writes share the socket,
/// are [`TlsStream`]'s.
pub struct ProtectedConnection<S: AsyncRead + AsyncWrite + Unpin = TcpStream> {
    stream: TlsStream<S>,
    frames: FrameReader,
}
impl ProtectedConnection {
    /// Take the socket out of `transport` and register it with the runtime.
    pub(super) fn open(transport: NativeTransport<DeadlineStream>) -> IoResult<Self> {
        TlsStream::open(transport).map(Self::framing)
    }
}
impl<S: AsyncRead + AsyncWrite + Unpin> ProtectedConnection<S> {
    fn framing(stream: TlsStream<S>) -> Self {
        Self {
            stream,
            frames: FrameReader::new(MAX_PROTECTED_REQUEST_BYTES),
        }
    }
    /// A connection over `socket` after a TLS handshake whose socket was
    /// taken; the test double for the socket's single-waker behaviour.
    #[cfg(test)]
    pub(super) fn over(transport: NativeTransport<DeadlineStream>, socket: S) -> Self {
        Self::framing(TlsStream::over(transport, socket))
    }
    /// The device key this connection's TLS handshake proved. Possession
    /// evidence only; the credential it may use is the registry's decision.
    pub fn device_proof(&self) -> &DeviceConnectionProof {
        self.stream.device_proof()
    }
    fn poll_frame(&mut self, context: &mut Context<'_>) -> Poll<Option<IoResult<Vec<u8>>>> {
        loop {
            let buffer = match self.frames.unfilled() {
                Ok(buffer) => buffer,
                Err(_) => return Poll::Ready(Some(Err(too_large()))),
            };
            let mut buffer = ReadBuf::new(buffer);
            match Pin::new(&mut self.stream).poll_read(context, &mut buffer) {
                Poll::Ready(Ok(())) => match buffer.filled().len() {
                    0 => return Poll::Ready(None),
                    count => match self.frames.filled(count) {
                        Ok(Some(body)) => return Poll::Ready(Some(Ok(body))),
                        Ok(None) => {}
                        Err(_) => return Poll::Ready(Some(Err(too_large()))),
                    },
                },
                Poll::Ready(Err(error)) => return Poll::Ready(Some(Err(error))),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}
impl<S: AsyncRead + AsyncWrite + Unpin> Stream for ProtectedConnection<S> {
    type Item = IoResult<Vec<u8>>;
    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.get_mut().poll_frame(context)
    }
}
impl<S: AsyncRead + AsyncWrite + Unpin> Sink<Vec<u8>> for ProtectedConnection<S> {
    type Error = Error;
    /// Ready once the previous frame's ciphertext has reached the socket, so at
    /// most one outgoing frame is buffered.
    fn poll_ready(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<IoResult<()>> {
        self.get_mut().stream.poll_send(context)
    }
    fn start_send(self: Pin<&mut Self>, body: Vec<u8>) -> IoResult<()> {
        let frame = encode_frame(MAX_PROTECTED_RESPONSE_BYTES, &body).map_err(|_| too_large())?;
        self.get_mut().stream.send(&frame)
    }
    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<IoResult<()>> {
        Pin::new(&mut self.get_mut().stream).poll_flush(context)
    }
    fn poll_close(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<IoResult<()>> {
        Pin::new(&mut self.get_mut().stream).poll_shutdown(context)
    }
}

fn too_large() -> Error {
    Error::new(ErrorKind::InvalidData, "native frame exceeds its bound")
}

#[cfg(test)]
#[path = "../../../tests/device_pairing/infrastructure/protected_waker.rs"]
mod tests;
