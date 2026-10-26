//! Injected socket/independent stop/wake effects for the product owner.
use nessa_protocol::tunnel::TunnelAddress;
use std::io::{Read, Result as IoResult, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// The sole worker's physical byte stream.
pub trait ProductStream: Read + Write + Send {
    /// Bound the next read wait.
    fn read_timeout(&self, timeout: Duration) -> IoResult<()>;
    /// Bound the next write wait.
    fn write_timeout(&self, timeout: Duration) -> IoResult<()>;
    /// Close the owned socket after physical settlement.
    fn shutdown(&self) -> IoResult<()>;
}
/// A separate handle that interrupts a blocked worker without its I/O lock.
pub trait ProductInterrupt: Send + Sync {
    /// Request physical shutdown; repeated calls are allowed.
    fn interrupt(&self);
}
/// The independently stoppable socket returned before TLS starts.
pub struct ConnectedProduct {
    /// Worker-owned bytes.
    pub stream: Box<dyn ProductStream>,
    /// Caller-retained independent stop handle.
    pub interrupt: Arc<dyn ProductInterrupt>,
}
/// Injected bounded physical connect.
pub trait ProductConnector: Send + Sync {
    /// Connect the original numeric address within the handshake budget.
    fn connect(&self, address: SocketAddr, timeout: Duration) -> IoResult<ConnectedProduct>;
}
/// Versioned wake port. Waiting on a previously observed revision avoids a lost
/// notification between checking shared state and parking.
pub trait ProductWake: Send + Sync {
    /// Capture the current notification revision.
    fn revision(&self) -> u64;
    /// Wait only while this revision remains current, for at most `timeout`.
    fn wait(&self, revision: u64, timeout: Duration);
    /// Notify all request, event, opening and drain observers.
    fn wake(&self);
}
/// Condvar adapter for the injected wake port.
#[derive(Default)]
pub struct ThreadProductWake {
    revision: Mutex<u64>,
    changed: Condvar,
}
impl ProductWake for ThreadProductWake {
    fn revision(&self) -> u64 {
        *self
            .revision
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }
    fn wait(&self, revision: u64, timeout: Duration) {
        let current = self
            .revision
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let _ = self
            .changed
            .wait_timeout_while(current, timeout, |value| *value == revision);
    }
    fn wake(&self) {
        let mut revision = self
            .revision
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        *revision = revision.wrapping_add(1);
        self.changed.notify_all();
    }
}
/// Real TCP composition adapter; the stop handle owns a cloned descriptor.
/// With a tunnel it dials the way a retained read does (`dial_gateway`):
/// direct first, then the gateway's web tunnel once the direct dial has not
/// connected within the head start or was refused. Either way the socket
/// carries the same pinned TLS.
#[derive(Default)]
pub struct LocalProductConnector {
    tunnel: Option<TunnelAddress>,
}
impl LocalProductConnector {
    /// A connector that falls back on `tunnel`, the gateway's public address
    /// as it last named it, when the direct dial does not connect.
    pub fn reaching(tunnel: Option<TunnelAddress>) -> Self {
        Self { tunnel }
    }
}
struct Socket(TcpStream);
impl Read for Socket {
    fn read(&mut self, bytes: &mut [u8]) -> IoResult<usize> {
        self.0.read(bytes)
    }
}
impl Write for Socket {
    fn write(&mut self, bytes: &[u8]) -> IoResult<usize> {
        self.0.write(bytes)
    }
    fn flush(&mut self) -> IoResult<()> {
        self.0.flush()
    }
}
impl ProductStream for Socket {
    fn read_timeout(&self, timeout: Duration) -> IoResult<()> {
        self.0.set_read_timeout(Some(timeout))
    }
    fn write_timeout(&self, timeout: Duration) -> IoResult<()> {
        self.0.set_write_timeout(Some(timeout))
    }
    fn shutdown(&self) -> IoResult<()> {
        self.0.shutdown(Shutdown::Both)
    }
}
impl ProductInterrupt for Socket {
    fn interrupt(&self) {
        let _ = self.0.shutdown(Shutdown::Both);
    }
}
impl ProductConnector for LocalProductConnector {
    fn connect(&self, address: SocketAddr, timeout: Duration) -> IoResult<ConnectedProduct> {
        let socket = crate::retained::dial_gateway(
            &address.to_string(),
            None,
            self.tunnel.as_ref(),
            timeout,
        )?;
        let interrupt = Arc::new(Socket(socket.try_clone()?));
        Ok(ConnectedProduct {
            stream: Box::new(Socket(socket)),
            interrupt,
        })
    }
}
