//! What a gateway serving through a relay runs on: TCP to the relay, the
//! native identity's proof, and the listener's relay permits and accept.
use super::application::{PortFuture, RelayAdmission, RelayDial, RelayKey, Relayed};
use crate::device_pairing::infrastructure::{
    Accepted, EnrollmentAccept, GatewayPairing, NativeEnrollmentConnections, RelayPermit,
};
use nessa_protocol::relay::GatewayKey;
use socket2::{SockRef, TcpKeepalive};
use std::{io, sync::Arc, time::Duration};
use tokio::{net::TcpStream, sync::mpsc};

/// How long a relay connection may sit silent before TCP starts asking
/// whether the relay is still there, as on the relay's own side.
const KEEPALIVE: Duration = Duration::from_secs(60);

/// Dials the relay at `host:port`. A host name is looked up at each dial,
/// inside the deadline the caller puts on it.
pub(crate) struct TcpRelayDial(pub(crate) String);

impl RelayDial<TcpStream> for TcpRelayDial {
    fn dial(&self) -> PortFuture<'_, io::Result<TcpStream>> {
        Box::pin(async move {
            let stream = TcpStream::connect(self.0.as_str()).await?;
            // Small preamble frames go out at once.
            stream.set_nodelay(true)?;
            SockRef::from(&stream).set_tcp_keepalive(&TcpKeepalive::new().with_time(KEEPALIVE))?;
            Ok(stream)
        })
    }
}

/// The gateway's native identity: readers pin its key, and it signs the
/// relay's challenges.
pub(crate) struct IdentityKey(pub(crate) Arc<GatewayPairing>);

impl RelayKey for IdentityKey {
    fn key(&self) -> GatewayKey {
        self.0.identity().public_key()
    }
    fn prove(&self, challenge: &[u8; 32]) -> Option<[u8; 64]> {
        self.0.identity().sign_relay_challenge(challenge).ok()
    }
    fn sign_origin(&self, origin: &[u8]) -> Option<[u8; 64]> {
        self.0.identity().sign_tunnel_origin(origin).ok()
    }
}

impl RelayAdmission<RelayPermit> for NativeEnrollmentConnections {
    fn admit(&self) -> Option<RelayPermit> {
        self.relay_permit()
    }
}

/// The listening socket and the readers the relay joined, as one accept.
/// A closed relay channel (no relay configured, or it stopped) leaves the
/// listening socket alone.
pub(crate) struct RelayedAccept<A> {
    socket: A,
    joined: mpsc::Receiver<Relayed<TcpStream, RelayPermit>>,
    open: bool,
}

impl<A> RelayedAccept<A> {
    /// Accept on `socket` and take each reader arriving on `joined`.
    pub(crate) fn new(socket: A, joined: mpsc::Receiver<Relayed<TcpStream, RelayPermit>>) -> Self {
        Self {
            socket,
            joined,
            open: true,
        }
    }
}

impl<A: EnrollmentAccept> EnrollmentAccept for RelayedAccept<A> {
    /// Both waits are cancel-safe: the socket's by its own contract, and a
    /// channel receive loses nothing when dropped. Neither is preferred
    /// (the select picks at random among those ready), so a flood of direct
    /// connections cannot keep joined relay readers waiting, nor the other
    /// way round.
    async fn accept(&mut self) -> Accepted {
        loop {
            tokio::select! {
                accepted = self.socket.accept() => return accepted,
                relayed = self.joined.recv(), if self.open => match relayed {
                    Some(Relayed { stream, permit }) => {
                        return match stream.into_std() {
                            Ok(stream) => Accepted::Relayed(stream, permit),
                            Err(error) => Accepted::ConnectionUnusable(error),
                        };
                    }
                    None => self.open = false,
                },
            }
        }
    }
}
