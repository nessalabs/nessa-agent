//! The real peer connector: a TCP connect on the async runtime. It has no
//! timeout of its own; the peer commands bound it by their injected deadline
//! clock and drop it at that deadline, which abandons the attempt and closes
//! its socket.
use crate::peer_gateways::application::{PeerConnectFuture, PeerConnector};
use std::net::SocketAddr;

/// Connects over the operating system's TCP stack.
pub struct TcpPeerConnector;
impl PeerConnector for TcpPeerConnector {
    fn connect(&self, address: SocketAddr) -> PeerConnectFuture<'_> {
        Box::pin(async move {
            let stream = tokio::net::TcpStream::connect(address).await?.into_std()?;
            // The enrollment client reads and writes it blocking, on a worker.
            stream.set_nonblocking(false)?;
            Ok(stream)
        })
    }
}
