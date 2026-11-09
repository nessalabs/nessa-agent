//! A gateway serving through a relay (issue #266, slice R2a): stay
//! registered under its own key, and turn each reader the relay announces
//! into a connection for the native listener (`docs/design/auth/relay.md`).
//!
//! ```text
//! serve         ──▶ register, serve the registration, wait, register again
//! register      ──▶ dial ──▶ Serve{K} ──▶ Challenge ──▶ Proof ──▶ Registered
//! the registration:
//!   Ping            ──▶ Pong
//!   Incoming{t}     ──▶ a relay permit ──▶ accept leg: dial ──▶ Accept{K, t}
//!                       ──▶ Joined ──▶ the listener, with its permit
//!                   ──▶ no permit: t is dropped, the relay answers NoAnswer
//!   silent too long ──▶ the registration is lost: register again
//! stop asked    ──▶ the registration and every accept leg are dropped
//! ```
//!
//! Arrows are calls in order. Every dial, read and write is bounded on the
//! injected timer; nothing here ends the gateway, which keeps serving
//! direct readers whatever the relay does. What a relayed connection
//! carries afterwards is the same TLS a direct one does, which the relay
//! cannot read.
use super::super::wire::{next_frame, read_frame, write_frame};
use crate::timer::{within, Timer};
use crate::tunnel::application::DeviceTunnelSecrets;
use nessa_protocol::relay::{
    GatewayKey, JoinToken, PublishedOrigin, RelayFrame, JOIN_DEADLINE, PING_INTERVAL,
    PREAMBLE_DEADLINE,
};
use std::{future::Future, io, pin::Pin, sync::Arc, time::Duration};
use tokio::{
    io::{split, AsyncRead, AsyncWrite},
    sync::{mpsc, watch},
    task::JoinSet,
};

/// How long the registration may hear nothing before it counts as lost.
/// The relay pings every [`PING_INTERVAL`], so two of them missing means
/// the relay, or the path to it, has gone: a laptop waking from sleep finds
/// its old connection dead this way rather than by waiting on TCP.
const SILENCE: Duration = Duration::from_secs(PING_INTERVAL.as_secs() * 2);

/// How long to wait between registrations.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Backoff {
    /// After the first attempt that fails.
    pub(crate) first: Duration,
    /// The longest wait; each further failure doubles the wait up to it.
    pub(crate) most: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Self {
            first: Duration::from_secs(1),
            most: Duration::from_secs(60),
        }
    }
}

/// A future the ports return.
pub(crate) type PortFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Opens a connection to the relay: one for the registration, and a new one
/// for each reader accepted.
pub(crate) trait RelayDial<S>: Send + Sync {
    /// A new connection to the relay.
    fn dial(&self) -> PortFuture<'_, io::Result<S>>;
}

/// The gateway's own key: what it registers under, and its proof of holding
/// it. Composition gives the native identity.
pub(crate) trait RelayKey: Send + Sync {
    /// The key readers pin and ask the relay for.
    fn key(&self) -> GatewayKey;
    /// The key's relay proof over `challenge`, or `None` when it cannot be
    /// made.
    fn prove(&self, challenge: &[u8; 32]) -> Option<[u8; 64]>;
    /// The key's signature over a tunnel origin it is publishing, or `None`
    /// when it cannot be made. The phone checks this before it sends its
    /// tunnel secret to that origin.
    fn sign_origin(&self, origin: &[u8]) -> Option<[u8; 64]>;
}

/// The listener's relay permits, a pool of their own so relayed readers
/// never take the permits direct readers need.
pub(crate) trait RelayAdmission<P>: Send + Sync {
    /// A permit for one more relayed connection, or `None` when the pool is
    /// full or closed.
    fn admit(&self) -> Option<P>;
}

/// A reader the relay joined to this gateway, ready for the listener, and
/// the relay permit it holds until it ends.
pub(crate) struct Relayed<S, P> {
    pub(crate) stream: S,
    pub(crate) permit: P,
}

/// What serving through a relay is given.
pub(crate) struct ThroughRelay<S, P> {
    pub(crate) dial: Arc<dyn RelayDial<S>>,
    pub(crate) key: Arc<dyn RelayKey>,
    pub(crate) admission: Arc<dyn RelayAdmission<P>>,
    pub(crate) timer: Arc<dyn Timer>,
    /// Where joined readers go: the listener's side.
    pub(crate) joined: mpsc::Sender<Relayed<S, P>>,
    /// The tunnel origin to publish. `None` clears it.
    pub(crate) address: watch::Receiver<Option<PublishedOrigin>>,
    /// Paired devices' tunnel secrets, for a lookup.
    pub(crate) secrets: Arc<dyn DeviceTunnelSecrets>,
}

impl<S, P> ThroughRelay<S, P>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    P: Send + 'static,
{
    /// Stay registered until `stop` is asked, registering again after each
    /// registration ends. The wait doubles after each attempt that fails,
    /// from `backoff.first` to `backoff.most`, and starts over after a
    /// registration that lasted at least `backoff.most`, so a relay that
    /// takes each registration and drops it is not dialed in a loop. A key
    /// that cannot make its proof stops it at once: registering again
    /// cannot change that.
    pub(crate) async fn serve(self, backoff: Backoff, mut stop: watch::Receiver<bool>) {
        let mut wait = backoff.first;
        loop {
            let registered = tokio::select! {
                biased;
                () = stopped(&mut stop) => return,
                registered = self.register() => registered,
            };
            match registered {
                Ok(stream) => {
                    let since = self.timer.now();
                    self.serve_registration(stream, stop.clone()).await;
                    if self.timer.now().since(since) >= backoff.most {
                        wait = backoff.first;
                    }
                }
                Err(Unregistered::Unprovable) => {
                    tracing::error!("the gateway key cannot make its relay proof; not registering");
                    return;
                }
                Err(Unregistered::Failed(error)) => {
                    tracing::warn!(%error, ?wait, "registering with the relay failed")
                }
            }
            tokio::select! {
                biased;
                () = stopped(&mut stop) => return,
                () = self.timer.sleep(wait) => {}
            }
            wait = (wait * 2).min(backoff.most);
        }
    }

    /// Dial the relay and prove the key, within one preamble deadline.
    async fn register(&self) -> Result<S, Unregistered> {
        let deadline = self.timer.now() + PREAMBLE_DEADLINE;
        let key = self.key.key();
        let preamble = async {
            let mut stream = self.dial.dial().await?;
            write_frame(&mut stream, RelayFrame::Serve { key }).await?;
            let RelayFrame::Challenge { nonce } = read_frame(&mut stream).await? else {
                return Err(unexpected("a challenge").into());
            };
            let signature = self.key.prove(&nonce).ok_or(Unregistered::Unprovable)?;
            write_frame(&mut stream, RelayFrame::Proof { signature }).await?;
            match read_frame(&mut stream).await? {
                RelayFrame::Registered => Ok::<_, Unregistered>(stream),
                _ => Err(unexpected("Registered").into()),
            }
        };
        within(&*self.timer, deadline, preamble)
            .await
            .unwrap_or_else(|_| Err(io::Error::from(io::ErrorKind::TimedOut).into()))
    }

    /// Answer pings and accept readers until the registration ends, falls
    /// silent for [`SILENCE`], or stop is asked. Accept legs still under
    /// way end with it.
    async fn serve_registration(&self, stream: S, mut stop: watch::Receiver<bool>) {
        let (reader, mut writer) = split(stream);
        let mut reading = Box::pin(next_frame(reader));
        let mut legs = JoinSet::new();
        let mut silent = self.timer.now() + SILENCE;
        let mut published = self.address.clone();
        let mut watch_address = true;
        let current = *published.borrow_and_update();
        if let Some(origin) = current {
            if !self.publish(&mut writer, origin).await {
                return;
            }
        }
        loop {
            tokio::select! {
                biased;
                () = stopped(&mut stop) => return,
                // Finished legs are reaped before another frame is read, so
                // a relay that keeps announcing readers cannot pile up
                // finished tasks behind the four running ones.
                Some(_) = legs.join_next(), if !legs.is_empty() => {}
                (reader, frame) = &mut reading => {
                    silent = self.timer.now() + SILENCE;
                    match frame {
                        Ok(RelayFrame::Ping) => {
                            // A relay that stops reading must not hold up stop.
                            let answered = tokio::select! {
                                biased;
                                () = stopped(&mut stop) => return,
                                answered = self.send(&mut writer, RelayFrame::Pong) => answered,
                            };
                            if !answered {
                                return;
                            }
                        }
                        Ok(RelayFrame::Incoming { token }) => self.incoming(&mut legs, token),
                        Ok(RelayFrame::Admit { token, nonce, mac }) => {
                            let allow = self.secrets.authorize_lookup(&self.key.key(), &nonce, &mac);
                            let frame = if allow {
                                RelayFrame::Allow { token }
                            } else {
                                RelayFrame::Deny { token }
                            };
                            let answered = tokio::select! {
                                biased;
                                () = stopped(&mut stop) => return,
                                answered = self.send(&mut writer, frame) => answered,
                            };
                            if !answered {
                                return;
                            }
                        }
                        Ok(other) => {
                            tracing::debug!(frame = ?other, "relay sent an unexpected frame");
                            return;
                        }
                        Err(error) => {
                            tracing::info!(kind = ?error.kind(), "relay registration ended");
                            return;
                        }
                    }
                    reading = Box::pin(next_frame(reader));
                }
                changed = published.changed(), if watch_address => {
                    match changed {
                        Ok(()) => {
                            let origin = match *published.borrow_and_update() {
                                Some(origin) => origin,
                                None => PublishedOrigin::clear(),
                            };
                            if !self.publish(&mut writer, origin).await {
                                return;
                            }
                        }
                        Err(_) => watch_address = false,
                    }
                }
                () = self.timer.sleep_until(silent) => {
                    tracing::info!("relay registration went silent");
                    return;
                }
            }
        }
    }

    /// Start an accept leg for `token`, if a relay permit is free.
    fn incoming(&self, legs: &mut JoinSet<()>, token: JoinToken) {
        let Some(permit) = self.admission.admit() else {
            tracing::debug!("relay permits all held; a relayed reader is not accepted");
            return;
        };
        let dial = self.dial.clone();
        let key = self.key.key();
        let timer = self.timer.clone();
        let joined = self.joined.clone();
        legs.spawn(async move {
            let deadline = timer.now() + JOIN_DEADLINE;
            let leg = async {
                let mut stream = dial.dial().await?;
                write_frame(&mut stream, RelayFrame::Accept { key, token }).await?;
                match read_frame(&mut stream).await? {
                    RelayFrame::Joined => Ok(stream),
                    _ => Err(unexpected("Joined")),
                }
            };
            match within(&*timer, deadline, leg).await {
                Ok(Ok(stream)) => {
                    // A listener that has stopped takes nothing; the stream
                    // and its permit are dropped with the send.
                    let _ = joined.send(Relayed { stream, permit }).await;
                }
                Ok(Err(error)) => tracing::debug!(kind = ?error.kind(), "relay accept failed"),
                Err(_) => tracing::debug!("relay accept timed out"),
            }
        });
    }

    /// Publish `origin`, signed by this gateway's key. An empty origin clears
    /// the one the relay holds. A key that cannot sign sends nothing: an
    /// unsigned origin would be an address a phone cannot trust.
    async fn publish(
        &self,
        stream: &mut (impl AsyncWrite + Unpin),
        origin: PublishedOrigin,
    ) -> bool {
        let Some(frame) = self.signed_publish(origin) else {
            tracing::error!("the gateway key cannot sign its tunnel origin");
            return true;
        };
        self.send(stream, frame).await
    }

    fn signed_publish(&self, origin: PublishedOrigin) -> Option<RelayFrame> {
        if origin.as_bytes().is_empty() {
            return Some(RelayFrame::Publish {
                origin: Box::new(PublishedOrigin::clear()),
            });
        }
        let text = origin.text()?;
        let signature = self.key.sign_origin(text.as_bytes())?;
        Some(RelayFrame::Publish {
            origin: Box::new(PublishedOrigin::signed(&signature, text)?),
        })
    }

    /// Write one frame on the registration, bounded by the ping interval.
    async fn send(&self, stream: &mut (impl AsyncWrite + Unpin), frame: RelayFrame) -> bool {
        let deadline = self.timer.now() + PING_INTERVAL;
        matches!(
            within(&*self.timer, deadline, write_frame(stream, frame)).await,
            Ok(Ok(()))
        )
    }
}

/// Resolves once stop is asked. A dropped sender is a stop too, as for the
/// listener: nothing can ask for one later, and registering on with no
/// listener behind it would announce a gateway that is not serving.
async fn stopped(stop: &mut watch::Receiver<bool>) {
    let _ = stop.wait_for(|stop| *stop).await;
}

/// Why a registration was not made.
#[derive(Debug)]
enum Unregistered {
    /// The gateway key cannot make its relay proof: trying again cannot
    /// change that.
    Unprovable,
    /// Dialing, the relay's answer, or the deadline: worth trying again.
    Failed(io::Error),
}

impl From<io::Error> for Unregistered {
    fn from(error: io::Error) -> Self {
        Self::Failed(error)
    }
}

fn unexpected(wanted: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("the relay did not answer with {wanted}"),
    )
}

#[cfg(test)]
#[path = "../../../tests/relay/gateway.rs"]
mod tests;
