//! The relay's listening socket and its random source.
use super::application::{Relay, RelayEntropy};
use crate::timer::Timer;
use socket2::{SockRef, TcpKeepalive};
use std::{future::Future, sync::Arc, time::Duration};
use tokio::{
    net::{TcpListener, TcpStream},
    task::JoinSet,
};

/// How long a connection may sit silent before TCP starts asking whether
/// the other end is still there. A joined pair has no idle limit of its
/// own, so this is what ends one whose end went away without a word.
const KEEPALIVE: Duration = Duration::from_secs(60);
/// Pause after a failed accept: the process may be out of descriptors, and
/// trying again at once would spin.
const ACCEPT_PAUSE: Duration = Duration::from_millis(100);

/// The operating system's random source.
pub(crate) struct OsRelayEntropy;

impl RelayEntropy for OsRelayEntropy {
    fn fill(&self, bytes: &mut [u8]) {
        // A relay that cannot draw a challenge or a token cannot work safely.
        getrandom::fill(bytes).expect("the operating system's random source failed");
    }
}

/// Accept on `listener` and hand every connection to `relay`, until `stop`
/// completes; then close every connection still open.
///
/// A connection past the relay's connection limit is closed unread. A
/// failed accept is logged and retried after a pause: the relay serves
/// every peer through one socket and gives up on none of them for another's
/// fault.
pub(crate) async fn serve_tcp(
    listener: TcpListener,
    relay: Arc<Relay<TcpStream>>,
    timer: Arc<dyn Timer>,
    stop: impl Future<Output = ()>,
) {
    let mut connections = JoinSet::new();
    tokio::pin!(stop);
    loop {
        tokio::select! {
            biased;
            () = &mut stop => break,
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    let Some(permit) = relay.admit() else {
                        tracing::debug!("relay at its connection limit; closed one");
                        continue;
                    };
                    // Small preamble frames go out at once.
                    let _ = stream.set_nodelay(true);
                    let _ = SockRef::from(&stream)
                        .set_tcp_keepalive(&TcpKeepalive::new().with_time(KEEPALIVE));
                    connections.spawn(relay.clone().handle(stream, permit));
                }
                Err(error) => {
                    tracing::warn!(kind = ?error.kind(), "relay accept failed; pausing");
                    tokio::select! {
                        biased;
                        () = &mut stop => break,
                        () = timer.sleep(ACCEPT_PAUSE) => {}
                    }
                }
            },
        }
    }
    connections.shutdown().await;
}

#[cfg(test)]
#[path = "../../tests/relay/tcp.rs"]
mod tests;
