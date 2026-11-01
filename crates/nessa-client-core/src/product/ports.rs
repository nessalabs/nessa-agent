//! Injected socket/independent stop/wake effects for the product owner.
use nessa_auth::application::pairing::ClientPendingStore;
use nessa_protocol::relay::RelayAddress;
use nessa_protocol::tunnel::{TunnelAddress, TunnelSecret};
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
/// With a tunnel it dials direct first, then the gateway's web tunnel once
/// the direct dial has not connected within the head start or was refused
/// (`retained::dial_native`). Either way the socket carries the same pinned
/// TLS.
#[derive(Default)]
pub struct LocalProductConnector {
    tunnel: Option<TunnelAddress>,
    /// The device's saved tunnel secret, read on each connect.
    secrets: Option<Arc<dyn ClientPendingStore>>,
    /// The relay to ask when the saved tunnel address does not connect.
    /// Set after open, when the phone already knows which relay the gateway uses.
    relay: Arc<Mutex<Option<RelayAddress>>>,
}
impl LocalProductConnector {
    /// A connector that falls back on `tunnel`, the gateway's public address
    /// as it last named it, when the direct dial does not connect. `secrets`
    /// supplies the device's tunnel secret. `relay` is shared so the phone
    /// can name the relay after open.
    pub fn reaching(
        tunnel: Option<TunnelAddress>,
        secrets: Option<Arc<dyn ClientPendingStore>>,
        relay: Arc<Mutex<Option<RelayAddress>>>,
    ) -> Self {
        Self {
            tunnel,
            secrets,
            relay,
        }
    }

    /// The slot [`Self::reaching`] stored the relay in.
    pub fn relay(&self) -> Arc<Mutex<Option<RelayAddress>>> {
        self.relay.clone()
    }

    fn secret(&self) -> Option<TunnelSecret> {
        let bytes = self.secrets.as_ref()?.load_tunnel_secret().ok()??;
        Some(TunnelSecret::new(bytes))
    }

    /// The origin the relay now publishes, dialed with this device's secret.
    /// `None` when this connector has no relay or no secret to ask with.
    fn published(
        &self,
        secret: Option<&TunnelSecret>,
        timeout: Duration,
    ) -> Option<IoResult<std::net::TcpStream>> {
        let secret = *secret?;
        let relay = self
            .relay
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()?;
        let credential = self.secrets.as_ref()?.load_credential().ok()??;
        let key = nessa_auth::adapters::pairing::relay_key(credential.gateway_pin())?;
        Some(
            match crate::tunnel::published_address(&relay, &key, &secret, timeout) {
                Ok(Some(origin)) => crate::tunnel::dial(&origin, &secret, timeout),
                Ok(None) => Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "the gateway has no tunnel address",
                )),
                Err(error) => Err(error),
            },
        )
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
        let secret = self.secret();
        let tunnel = self.tunnel.as_ref().zip(secret.as_ref());
        let socket = match crate::retained::dial_native(address, tunnel, timeout) {
            Ok(socket) => socket,
            Err(error) => self
                .published(secret.as_ref(), timeout)
                .unwrap_or(Err(error))?,
        };
        let interrupt = Arc::new(Socket(socket.try_clone()?));
        Ok(ConnectedProduct {
            stream: Box::new(Socket(socket)),
            interrupt,
        })
    }
}
