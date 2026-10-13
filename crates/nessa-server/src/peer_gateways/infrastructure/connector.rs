//! The real peer connector: a TCP connect on the async runtime, and the relay
//! leg a status dial races it with when the peer's record keeps a relay, with
//! [`raced`], the race itself, over whatever connector the commands hold.
//! Each leg has no timeout of its own; the peer commands bound them by their injected
//! deadline clock and drop them at that deadline, which abandons the attempt
//! and closes its socket.
//!
//! ```text
//! direct connect ─────────────▶ connected first: used, the relay never dialed
//!   not connected after DIRECT_HEAD_START, or refused sooner:
//!   relay leg: dial the relay ── Connect{K} ──▶ Joined: used
//! the first to connect is used and the other dropped; both failing is the
//! relay's error
//! ```
use crate::peer_gateways::application::{PeerConnectFuture, PeerConnector};
use crate::relay::wire::{read_frame, write_frame};
use nessa_protocol::relay::{GatewayKey, RelayAddress, RelayFrame, DIRECT_HEAD_START};
use std::future::Future;
use std::io;
use std::net::{SocketAddr, TcpStream};
use std::sync::{Mutex, PoisonError};

/// Connects over the operating system's TCP stack.
pub struct TcpPeerConnector;
impl PeerConnector for TcpPeerConnector {
    fn connect(&self, address: SocketAddr) -> PeerConnectFuture<'_> {
        Box::pin(async move {
            let stream = tokio::net::TcpStream::connect(address).await?;
            blocking(stream)
        })
    }
    fn relay<'a>(&'a self, relay: &'a RelayAddress, key: GatewayKey) -> PeerConnectFuture<'a> {
        Box::pin(relay_leg(relay, key))
    }
}

/// The enrollment client reads and writes the stream blocking, on a worker.
fn blocking(stream: tokio::net::TcpStream) -> io::Result<TcpStream> {
    let stream = stream.into_std()?;
    stream.set_nonblocking(false)?;
    Ok(stream)
}

/// `direct`, raced with the relay leg `relay` once `direct` has not
/// connected within [`DIRECT_HEAD_START`] or has failed. No relay: `direct`
/// alone. A relay that refuses while `direct` still waits leaves its error
/// in `refused`, so a caller that drops this at its deadline still reports
/// the relay's answer rather than its own timeout.
pub(super) async fn raced(
    direct: impl Future<Output = io::Result<TcpStream>>,
    relay: Option<impl Future<Output = io::Result<TcpStream>>>,
    refused: &Mutex<Option<io::Error>>,
) -> io::Result<TcpStream> {
    let Some(leg) = relay else {
        return direct.await;
    };
    tokio::pin!(direct);
    let direct_failed = tokio::select! {
        biased;
        connected = &mut direct => match connected {
            Ok(stream) => return Ok(stream),
            Err(_) => true,
        },
        () = tokio::time::sleep(DIRECT_HEAD_START) => false,
    };
    tokio::pin!(leg);
    if direct_failed {
        return leg.await;
    }
    tokio::select! {
        connected = &mut direct => match connected {
            Ok(stream) => Ok(stream),
            Err(_) => leg.await,
        },
        joined = &mut leg => match joined {
            Ok(stream) => Ok(stream),
            // A slow direct connect may still connect; if not, the relay's
            // answer is the more telling one, kept for a caller that gives
            // up on the direct connect first.
            Err(error) => {
                *refused.lock().unwrap_or_else(PoisonError::into_inner) = Some(error);
                let connected = direct.await;
                let error = refused.lock().unwrap_or_else(PoisonError::into_inner).take();
                connected.map_err(|direct| error.unwrap_or(direct))
            }
        },
    }
}

/// Dial the relay at `address`, its name looked up now, and ask it for
/// `key`. `Joined` returns the stream with the peer's own bytes next on it;
/// `Offline` and `NoAnswer` are `ConnectionRefused`.
async fn relay_leg(address: &RelayAddress, key: GatewayKey) -> io::Result<TcpStream> {
    let mut stream = tokio::net::TcpStream::connect(address.as_str()).await?;
    // Small preamble frames go out at once.
    stream.set_nodelay(true)?;
    write_frame(&mut stream, RelayFrame::Connect { key }).await?;
    match read_frame(&mut stream).await? {
        RelayFrame::Joined => blocking(stream),
        RelayFrame::Offline => Err(io::Error::new(
            io::ErrorKind::ConnectionRefused,
            "the peer is not registered at its relay",
        )),
        RelayFrame::NoAnswer => Err(io::Error::new(
            io::ErrorKind::ConnectionRefused,
            "the peer did not accept through its relay in time",
        )),
        _ => Err(io::ErrorKind::InvalidData.into()),
    }
}

#[cfg(test)]
#[path = "../../../tests/peer_gateways/connector.rs"]
mod tests;
