//! Absolute fixture deadlines cover proxy accept and both directions of copying.
//! These streams carry ciphertext only; the real retained client owns its clocks.
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::time::{Duration, Instant};

pub(super) struct Stream {
    socket: TcpStream,
    deadline: Instant,
}
fn left(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|left| !left.is_zero())
        .ok_or_else(|| io::ErrorKind::TimedOut.into())
}
impl Stream {
    pub(super) fn accept(listener: &TcpListener, deadline: Instant) -> io::Result<Self> {
        listener.set_nonblocking(true)?;
        loop {
            let remaining = left(deadline)?;
            match listener.accept() {
                Ok((socket, _)) => {
                    socket.set_nonblocking(false)?;
                    return Ok(Self { socket, deadline });
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(remaining.min(Duration::from_millis(5)));
                }
                Err(error) => return Err(error),
            }
        }
    }
    pub(super) fn connect(address: SocketAddr, deadline: Instant) -> io::Result<Self> {
        Ok(Self {
            socket: TcpStream::connect_timeout(&address, left(deadline)?)?,
            deadline,
        })
    }
    pub(super) fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            socket: self.socket.try_clone()?,
            deadline: self.deadline,
        })
    }
    pub(super) fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        self.socket.shutdown(how)
    }
}
impl Read for Stream {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        loop {
            self.socket
                .set_read_timeout(Some(left(self.deadline)?.min(Duration::from_millis(50))))?;
            match self.socket.read(bytes) {
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) => {}
                result => return result,
            }
        }
    }
}
impl Write for Stream {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.socket.set_write_timeout(Some(left(self.deadline)?))?;
        self.socket.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        left(self.deadline)?;
        self.socket.flush()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn an_unconnected_proxy_accept_ends_at_its_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let started = Instant::now();
        let result = Stream::accept(&listener, started + Duration::from_millis(30));
        assert!(matches!(result, Err(error) if error.kind() == io::ErrorKind::TimedOut));
    }
    #[test]
    fn a_connected_silent_proxy_read_ends_at_its_original_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let _silent = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        // Establish the connection before starting the read deadline. Scheduling
        // during setup must not turn the expected read timeout into accept failure.
        let (socket, _) = listener.accept().unwrap();
        let mut stream = Stream {
            socket,
            deadline: Instant::now() + Duration::from_millis(30),
        };
        let mut clone = stream.try_clone().unwrap();
        assert_eq!(
            stream.read(&mut [0; 1]).unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
        assert_eq!(
            clone.read(&mut [0; 1]).unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
        assert_eq!(
            clone.write_all(b"past deadline").unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
    }
}
