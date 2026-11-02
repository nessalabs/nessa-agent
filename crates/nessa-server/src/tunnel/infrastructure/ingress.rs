//! The tunnel's way in (`docs/design/auth/tunnel.md`): a loopback listener
//! the tunnel's program forwards each WebSocket to, and the listener's third
//! source of connections.
//!
//! ```text
//! cloudflared ── ws://127.0.0.1:port/native ──▶ TunnelIngress
//!   the device's tunnel secret, or 401 and no permit
//!   a tunnel permit, or 503
//!   the upgrade on /native within UPGRADE_DEADLINE, or refused
//!   a loopback pair: one end ──▶ TunneledAccept ──▶ the native listener
//!                    the other end ◀── bridge ──▶ the WebSocket
//! ```
//!
//! Arrows are the order each connection goes through. Nothing here reads
//! what crosses: past the upgrade it is the native TLS, and the listener
//! checks the key it proves before anything else.
use super::super::application::DeviceTunnelSecrets;
use crate::device_pairing::infrastructure::{
    Accepted, EnrollmentAccept, NativeEnrollmentConnections, TunnelPermit,
};
use nessa_client_core::tunnel::{bridge, loopback_pair, websocket_config};
use nessa_protocol::tunnel::{TunnelSecret, TUNNEL_PATH, TUNNEL_SECRET_HEADER};
use std::{
    io,
    net::TcpStream as StdStream,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    net::TcpListener,
    sync::{mpsc, watch, OwnedSemaphorePermit, Semaphore},
    task::JoinSet,
};
use tokio_tungstenite::tungstenite::{
    handshake::server::{ErrorResponse, Request, Response},
    http::StatusCode,
};

/// How long a connection has to finish its WebSocket upgrade.
const UPGRADE_DEADLINE: Duration = Duration::from_secs(10);
/// Sockets that have not yet presented a secret. Separate from the tunnel
/// permit pool: a stranger who never finishes the upgrade cannot pile up
/// tasks, and a matched secret still takes a tunnel permit only after it matches.
const PREAUTH_CAPACITY: usize = 4;
/// How long making the loopback pair may take.
const PAIR_DEADLINE: Duration = Duration::from_secs(5);

/// A connection that came through the tunnel, ready for the listener, and
/// the tunnel permit it holds until it ends.
pub(crate) struct Tunneled<P = TunnelPermit> {
    pub(crate) stream: StdStream,
    pub(crate) permit: P,
}

/// The listener's tunnel permits.
pub(crate) trait TunnelAdmission<P>: Send + Sync {
    /// A permit for one more tunnel connection, or `None` when the pool is
    /// full or closed.
    fn admit(&self) -> Option<P>;
}

impl TunnelAdmission<TunnelPermit> for NativeEnrollmentConnections {
    fn admit(&self) -> Option<TunnelPermit> {
        self.tunnel_permit()
    }
}

/// The loopback listener the tunnel forwards to.
// Started only by the Unix composition, beside the provider's program.
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) struct TunnelIngress<P = TunnelPermit> {
    listener: TcpListener,
    admission: Arc<dyn TunnelAdmission<P>>,
    secrets: Arc<dyn DeviceTunnelSecrets>,
    joined: mpsc::Sender<Tunneled<P>>,
}

#[cfg_attr(not(unix), allow(dead_code))]
impl<P: Send + 'static> TunnelIngress<P> {
    /// Serve `listener`, which composition bound on a loopback address.
    pub(crate) fn new(
        listener: TcpListener,
        admission: Arc<dyn TunnelAdmission<P>>,
        secrets: Arc<dyn DeviceTunnelSecrets>,
        joined: mpsc::Sender<Tunneled<P>>,
    ) -> Self {
        Self {
            listener,
            admission,
            secrets,
            joined,
        }
    }

    /// Take connections until `stop` is asked, then drop every bridge: the
    /// native side of each is closed, which its worker sees as the peer
    /// leaving. An accept that fails is skipped after a short pause; it
    /// never ends the gateway.
    pub(crate) async fn serve(self, mut stop: watch::Receiver<bool>) {
        let mut bridges = JoinSet::new();
        let preauth = Arc::new(Semaphore::new(PREAUTH_CAPACITY));
        loop {
            let accepted = tokio::select! {
                biased;
                _ = stop.wait_for(|stopped| *stopped) => return,
                Some(_) = bridges.join_next(), if !bridges.is_empty() => continue,
                accepted = self.listener.accept() => accepted,
            };
            let stream = match accepted {
                Ok((stream, _)) => stream,
                Err(error) => {
                    tracing::debug!(kind = ?error.kind(), "tunnel ingress accept failed");
                    tokio::select! {
                        biased;
                        _ = stop.wait_for(|stopped| *stopped) => return,
                        () = tokio::time::sleep(Duration::from_millis(50)) => {}
                    }
                    continue;
                }
            };
            // A socket that has not presented a secret holds a pre-auth slot,
            // not a tunnel permit. With none free the socket is closed and no
            // task is spawned. The secret is still checked before a permit.
            let Ok(slot) = preauth.clone().try_acquire_owned() else {
                drop(stream);
                continue;
            };
            let admission = self.admission.clone();
            let secrets = self.secrets.clone();
            let joined = self.joined.clone();
            bridges.spawn(async move {
                if let Err(error) = take(stream, admission, secrets, joined, slot).await {
                    tracing::debug!(%error, "tunnel connection refused");
                }
            });
        }
    }
}

/// Upgrade one connection, pair it, hand one end to the listener and bridge
/// the other until either side ends.
#[allow(clippy::result_large_err)]
async fn take<P: Send>(
    stream: tokio::net::TcpStream,
    admission: Arc<dyn TunnelAdmission<P>>,
    secrets: Arc<dyn DeviceTunnelSecrets>,
    joined: mpsc::Sender<Tunneled<P>>,
    slot: OwnedSemaphorePermit,
) -> io::Result<()> {
    stream.set_nodelay(true)?;
    // Filled only after the secret matches, and before the upgrade response
    // is written. Dropped if the upgrade then fails, so the permit is not kept.
    let held: Arc<Mutex<Option<P>>> = Arc::new(Mutex::new(None));
    let held_in = held.clone();
    let upgrade = tokio_tungstenite::accept_hdr_async_with_config(
        stream,
        move |request: &Request, response: Response| {
            admit_request(request, response, &secrets, &admission, &held_in)
        },
        Some(websocket_config()),
    );
    let websocket = tokio::time::timeout(UPGRADE_DEADLINE, upgrade)
        .await
        .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))?
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let permit = held
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .take()
        .ok_or_else(|| io::Error::from(io::ErrorKind::PermissionDenied))?;
    // Authentication and the upgrade are done. The tunnel permit bounds the
    // open session; this slot is only for a handshake that has not finished.
    drop(slot);
    let (ours, theirs) = loopback_pair(PAIR_DEADLINE).await?;
    joined
        .send(Tunneled {
            stream: theirs,
            permit,
        })
        .await
        .map_err(|_| io::Error::from(io::ErrorKind::NotConnected))?;
    bridge(websocket, ours).await;
    Ok(())
}

/// `/native` with the secret in the header, or `/native/<secret>` when the
/// client cannot set a header. The header wins when both are present.
/// Anything else is 404. A missing or unknown secret is 401 and takes no
/// permit. A known secret with a full pool is 503 and takes no permit.
#[allow(clippy::result_large_err)]
fn admit_request<P>(
    request: &Request,
    response: Response,
    secrets: &Arc<dyn DeviceTunnelSecrets>,
    admission: &Arc<dyn TunnelAdmission<P>>,
    held: &Mutex<Option<P>>,
) -> Result<Response, ErrorResponse> {
    let presented = presented_secret(request)?;
    if !secrets.accepts(&presented) {
        return Err(refused(StatusCode::UNAUTHORIZED));
    }
    let Some(permit) = admission.admit() else {
        return Err(refused(StatusCode::SERVICE_UNAVAILABLE));
    };
    *held.lock().unwrap_or_else(|error| error.into_inner()) = Some(permit);
    Ok(response)
}

/// The secret on this upgrade, or the status that refuses it.
#[allow(clippy::result_large_err)]
fn presented_secret(request: &Request) -> Result<[u8; 32], ErrorResponse> {
    let path = request.uri().path();
    let from_path = if path == TUNNEL_PATH {
        None
    } else if let Some(rest) = path.strip_prefix(TUNNEL_PATH) {
        let rest = rest
            .strip_prefix('/')
            .filter(|rest| !rest.is_empty() && !rest.contains('/'));
        let Some(rest) = rest else {
            return Err(refused(StatusCode::NOT_FOUND));
        };
        Some(rest)
    } else {
        return Err(refused(StatusCode::NOT_FOUND));
    };
    if let Some(value) = request.headers().get(TUNNEL_SECRET_HEADER) {
        let Ok(text) = value.to_str() else {
            return Err(refused(StatusCode::UNAUTHORIZED));
        };
        return TunnelSecret::decode(text)
            .map(|secret| *secret.expose())
            .ok_or_else(|| refused(StatusCode::UNAUTHORIZED));
    }
    from_path
        .and_then(TunnelSecret::decode)
        .map(|secret| *secret.expose())
        .ok_or_else(|| refused(StatusCode::UNAUTHORIZED))
}

fn refused(status: StatusCode) -> ErrorResponse {
    let mut refused = ErrorResponse::new(None);
    *refused.status_mut() = status;
    refused
}

/// The listening socket and the connections that came through the tunnel,
/// as one accept. A closed tunnel channel (no tunnel configured, or it
/// stopped) leaves the listening socket alone.
pub(crate) struct TunneledAccept<A> {
    socket: A,
    joined: mpsc::Receiver<Tunneled>,
    open: bool,
}

impl<A> TunneledAccept<A> {
    /// Accept on `socket` and take each connection arriving on `joined`.
    pub(crate) fn new(socket: A, joined: mpsc::Receiver<Tunneled>) -> Self {
        Self {
            socket,
            joined,
            open: true,
        }
    }
}

impl<A: EnrollmentAccept> EnrollmentAccept for TunneledAccept<A> {
    /// Both waits are cancel-safe, and neither is preferred, as in the
    /// relay's accept: a flood on one source cannot keep the other waiting.
    async fn accept(&mut self) -> Accepted {
        loop {
            tokio::select! {
                accepted = self.socket.accept() => return accepted,
                tunneled = self.joined.recv(), if self.open => match tunneled {
                    Some(Tunneled { stream, permit }) => return Accepted::Tunneled(stream, permit),
                    None => self.open = false,
                },
            }
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/tunnel/ingress.rs"]
mod tests;
