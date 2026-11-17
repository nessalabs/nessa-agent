//! Cancellable I/O on an SSH standard descriptor, whether it is a pipe or
//! socket. Tokio's blocking `stdin()` would hold runtime shutdown open after
//! a command finished while the gateway still held its input open.
use std::{
    fs::File,
    io::{self, Read, Write},
    os::fd::{AsRawFd, OwnedFd},
    pin::Pin,
    task::{ready, Context, Poll},
};
use tokio::io::{unix::AsyncFd, AsyncRead, AsyncWrite, ReadBuf};

pub struct BackgroundIo(AsyncFd<File>);
impl BackgroundIo {
    /// Take the descriptor supplied at composition and make its I/O
    /// nonblocking before registering it with the runtime's readiness driver.
    pub fn from_fd(fd: OwnedFd) -> io::Result<Self> {
        let file = File::from(fd);
        // SAFETY: the owned file keeps this descriptor valid through both calls.
        let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
        if flags < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: F_SETFL updates the valid descriptor's status flags, retaining
        // its other flags. Both directions use this adapter when stdin and stdout share flags.
        if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(io::Error::last_os_error());
        }
        AsyncFd::new(file).map(Self)
    }
}
impl AsyncRead for BackgroundIo {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        output: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if output.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        loop {
            let mut readable = ready!(self.0.poll_read_ready(cx))?;
            match readable.try_io(|fd| {
                let count = fd.get_ref().read(output.initialize_unfilled())?;
                output.advance(count);
                Ok(())
            }) {
                Ok(result) => return Poll::Ready(result),
                Err(_) => continue,
            }
        }
    }
}

impl AsyncWrite for BackgroundIo {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        if bytes.is_empty() {
            return Poll::Ready(Ok(0));
        }
        loop {
            let mut writable = ready!(self.0.poll_write_ready(cx))?;
            match writable.try_io(|fd| fd.get_ref().write(bytes)) {
                Ok(result) => return Poll::Ready(result),
                Err(_) => continue,
            }
        }
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        // No user-space output buffer: each completed write reached the fd.
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_flush(cx)
    }
}

#[cfg(test)]
#[path = "../../../tests/conversation/background_io.rs"]
mod tests;
