//! A native TLS connection as an async byte stream, once its blocking phase
//! is over: the protected product session ([`super::protected`]) frames on
//! it, and an environment's lease frames ride it after `openEnvironment`,
//! read and written by either end.
//!
//! ```text
//! tokio socket <--ciphertext--> BufferedIo <--> NativeTransport (TLS, proof)
//!                                                  <--bytes--> AsyncRead + AsyncWrite
//! ```
//! Arrows are byte flow. The TLS state never touches the socket: the socket
//! is moved out of the blocking stream ([`DeadlineStream::take_socket`]) and
//! polled here, and TLS reads and writes in-memory ciphertext. This type
//! frames nothing and grants nothing: framing is its consumer's, and who the
//! peer is, and what it may do, is decided above it.
//!
//! Reading never polls the socket's write side, and writing never its read
//! side, so a reader and a writer split from one stream keep a waker each
//! (`reading_never_takes_the_writers_wakeup`). Ciphertext TLS produces while
//! reading (an alert, a key-update reply) waits in the buffer for the next
//! write. When the stream is dropped, whatever is still buffered gets one
//! nonblocking attempt to reach the socket, so a fatal alert TLS raised while
//! reading is sent rather than replaced by a bare close
//! (`a_fatal_alert_is_sent_before_the_close`). Once its connection owner
//! wakes it for shutdown, or [`TlsCloser::close`] ends it from outside, a
//! waiting reader is woken at once and every later read is refused, and a
//! closed stream refuses every write too.
use nessa_auth::{adapters::pairing::NativeTransport, application::pairing::DeviceConnectionProof};
use nessa_protocol::pairing::socket::DeadlineStream;
use std::{
    io::{Error, ErrorKind, Read, Result as IoResult, Write},
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, PoisonError,
    },
    task::{Context, Poll, Waker},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::TcpStream,
};

/// Socket reads one poll performs before yielding, so a peer that keeps the
/// socket readable cannot hold the task.
const READS_PER_POLL: usize = 8;
/// Ciphertext read from the socket at a time.
const SOCKET_READ_BYTES: usize = 16 * 1024;

/// Ends a [`TlsStream`] from outside it.
#[derive(Default)]
pub struct TlsCloser {
    closed: AtomicBool,
    reader: Mutex<Option<Waker>>,
}

impl TlsCloser {
    /// Refuse every later read and write, and wake a waiting reader.
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        if let Some(reader) = self
            .reader
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            reader.wake();
        }
    }

    /// Have `reader` woken by [`Self::close`]; `true` if already closed.
    fn register(&self, reader: &Waker) -> bool {
        let mut held = self.reader.lock().unwrap_or_else(PoisonError::into_inner);
        if self.closed.load(Ordering::SeqCst) {
            return true;
        }
        match held.as_ref() {
            Some(waker) if waker.will_wake(reader) => {}
            _ => *held = Some(reader.clone()),
        }
        false
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }
}

/// One native TLS connection as a byte stream, after its handshake and its
/// first envelope.
pub struct TlsStream<S: AsyncRead + AsyncWrite + Unpin = TcpStream> {
    transport: NativeTransport<DeadlineStream>,
    socket: S,
    closer: Arc<TlsCloser>,
}

impl TlsStream {
    /// Take the socket out of `transport` and register it with the runtime.
    pub fn open(mut transport: NativeTransport<DeadlineStream>) -> IoResult<Self> {
        let socket = transport
            .stream_mut()
            .take_socket()
            .ok_or_else(|| Error::from(ErrorKind::NotConnected))?;
        socket.set_nonblocking(true)?;
        let socket = TcpStream::from_std(socket)?;
        Ok(Self::over(transport, socket))
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> TlsStream<S> {
    /// A stream over `socket` after a TLS handshake whose socket was taken.
    pub(super) fn over(transport: NativeTransport<DeadlineStream>, socket: S) -> Self {
        Self {
            transport,
            socket,
            closer: Arc::default(),
        }
    }

    /// The device key this connection's TLS handshake proved. Possession
    /// evidence only; what that key may do is the registry's decision.
    pub fn device_proof(&self) -> &DeviceConnectionProof {
        self.transport.device_proof()
    }

    /// What ends this stream from outside it.
    pub fn closer(&self) -> Arc<TlsCloser> {
        self.closer.clone()
    }

    /// Hand `bytes` to TLS whole: their ciphertext waits in the buffer for
    /// [`Self::poll_send`].
    pub(super) fn send(&mut self, bytes: &[u8]) -> IoResult<()> {
        if self.closer.is_closed() {
            return Err(Error::from(ErrorKind::ConnectionAborted));
        }
        self.transport.write_all(bytes)?;
        self.transport.flush()
    }

    /// Write buffered ciphertext to the socket until none is left.
    pub(super) fn poll_send(&mut self, context: &mut Context<'_>) -> Poll<IoResult<()>> {
        loop {
            let io = self
                .transport
                .stream_mut()
                .buffered()
                .expect("an environment connection buffers");
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
}

impl<S: AsyncRead + AsyncWrite + Unpin> AsyncRead for TlsStream<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<IoResult<()>> {
        let this = self.get_mut();
        // Closed from outside, or woken by the listener's shutdown: refused.
        if this.closer.register(context.waker())
            || this.transport.stream_mut().wake_reader(context.waker())
        {
            return Poll::Ready(Err(Error::from(ErrorKind::ConnectionAborted)));
        }
        for _ in 0..READS_PER_POLL {
            // Plaintext TLS already holds comes first.
            loop {
                match this.transport.read(buffer.initialize_unfilled()) {
                    Ok(count) => {
                        buffer.advance(count);
                        return Poll::Ready(Ok(()));
                    }
                    Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                    Err(error) if error.kind() == ErrorKind::Interrupted => {}
                    Err(error) => return Poll::Ready(Err(error)),
                }
            }
            let mut bytes = [0; SOCKET_READ_BYTES];
            let mut received = ReadBuf::new(&mut bytes);
            match Pin::new(&mut this.socket).poll_read(context, &mut received) {
                Poll::Ready(Ok(())) => {
                    let io = this
                        .transport
                        .stream_mut()
                        .buffered()
                        .expect("an environment connection buffers");
                    match received.filled() {
                        [] => io.end(),
                        filled => io.receive(filled),
                    }
                }
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Pending => return Poll::Pending,
            }
        }
        context.waker().wake_by_ref();
        Poll::Pending
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> AsyncWrite for TlsStream<S> {
    /// Accepts bytes once the previous write's ciphertext has reached the
    /// socket, so at most one write's ciphertext is buffered.
    fn poll_write(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<IoResult<usize>> {
        let this = self.get_mut();
        if this.closer.is_closed() {
            return Poll::Ready(Err(Error::from(ErrorKind::ConnectionAborted)));
        }
        match this.poll_send(context) {
            Poll::Ready(Ok(())) => {}
            Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
            Poll::Pending => return Poll::Pending,
        }
        this.send(bytes)?;
        // One attempt to send it now; what is left goes with the next write
        // or flush.
        match this.poll_send(context) {
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            _ => Poll::Ready(Ok(bytes.len())),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<IoResult<()>> {
        let this = self.get_mut();
        match this.poll_send(context) {
            Poll::Ready(Ok(())) => Pin::new(&mut this.socket).poll_flush(context),
            other => other,
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<IoResult<()>> {
        let this = self.get_mut();
        match this.poll_send(context) {
            Poll::Ready(Ok(())) => Pin::new(&mut this.socket).poll_shutdown(context),
            other => other,
        }
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> Drop for TlsStream<S> {
    /// One bounded flush: nonblocking writes until the buffer is empty or the
    /// socket would block, so a fatal alert TLS raised while reading is sent
    /// rather than replaced by a bare close.
    fn drop(&mut self) {
        let mut context = Context::from_waker(futures_util::task::noop_waker_ref());
        let _ = self.poll_send(&mut context);
    }
}
