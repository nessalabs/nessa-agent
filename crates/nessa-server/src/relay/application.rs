//! The relay service: who is registered, who waits to be joined, and the
//! joined pairs, each bounded (`docs/design/auth/relay.md`, rows RL1 to RL8).
//!
//! ```text
//! a connection ──▶ Relay::admit (a connection permit, or closed)
//!   ──▶ Relay::handle: its first frame, within PREAMBLE_DEADLINE
//!     Serve{K}     ──▶ Challenge ──▶ Proof verified ──▶ Registered
//!                  ──▶ serve loop: Incoming{t} for each reader, Ping, Pong
//!     Connect{K}   ──▶ Offline (no registration) | Incoming{t} to K's serve loop
//!                  ──▶ K's Accept{K, t} within JOIN_DEADLINE ──▶ Joined, copy both ways
//!                  ──▶ no Accept in time, or past a limit ──▶ NoAnswer
//!     Accept{K, t} ──▶ handed to the reader waiting under t
//! ```
//!
//! Arrows are calls in order. No lock is held across an await: the table is
//! changed in short sections, and everything that waits (a read, a write, a
//! deadline) does so outside them. Every write and every wait for the other
//! side is bounded on the injected timer, so a peer that stops reading
//! cannot hold a task forever; a joined pair is the one exception, as it
//! may sit idle as long as the two ends like.
use super::wire::{next_frame, read_frame, write_frame};
use crate::timer::{within, Moment, Timer};
use nessa_protocol::relay::{
    key_id, GatewayKey, JoinToken, PublishedOrigin, RelayFrame, JOIN_DEADLINE, MISSED_PINGS,
    PING_INTERVAL, PREAMBLE_DEADLINE,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};
use tokio::{
    io::{split, AsyncRead, AsyncWrite},
    sync::{mpsc, oneshot, OwnedSemaphorePermit, Semaphore},
};

/// Where challenges and join tokens come from: the operating system's
/// random source in composition, a fixed sequence in a test.
pub(crate) trait RelayEntropy: Send + Sync {
    /// Fill `bytes` with fresh random bytes.
    fn fill(&self, bytes: &mut [u8]);
}

/// Who may register a key: whether a signature is that key's proof over a
/// challenge the relay sent. Composition gives the Ed25519 check.
pub(crate) trait RelayProofs: Send + Sync {
    /// Whether `signature` is `key`'s relay proof for `challenge`.
    fn verify(&self, key: &GatewayKey, challenge: &[u8; 32], signature: &[u8; 64]) -> bool;
}

/// How much the relay holds at once. Past any of these, what would go over
/// is refused at its first frame and nothing else is touched (row RL8).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RelayLimits {
    /// Open connections of every kind, the joined ones included.
    pub(crate) connections: usize,
    /// Gateways registered.
    pub(crate) registrations: usize,
    /// Readers waiting for one gateway to accept.
    pub(crate) waiting_per_gateway: usize,
    /// Joined pairs, and readers waiting to become one.
    pub(crate) joined: usize,
}

impl Default for RelayLimits {
    fn default() -> Self {
        Self {
            connections: 1024,
            registrations: 256,
            waiting_per_gateway: 8,
            joined: 384,
        }
    }
}

/// What the serve loop sends the gateway.
enum ToGateway {
    /// A reader to join.
    Incoming(JoinToken),
    /// A lookup for the gateway to allow or deny.
    Admit {
        token: JoinToken,
        nonce: [u8; 32],
        mac: [u8; 32],
    },
}

/// One registered gateway's serve connection.
struct Registration {
    /// Which serve connection this is, so a replaced one removes only itself.
    id: u64,
    /// Frames for its serve loop to send.
    incoming: mpsc::Sender<ToGateway>,
    /// Sent when a newer proved connection takes the key's place.
    replaced: oneshot::Sender<()>,
    /// The tunnel origin it last published. Empty means none.
    origin: PublishedOrigin,
}

/// A reader waiting to be told the tunnel origin, or refused.
struct LookupWait {
    key: GatewayKey,
    answer: oneshot::Sender<bool>,
}

/// One reader waiting for its gateway to accept.
struct Waiting<S> {
    key: GatewayKey,
    /// The gateway's new connection, with its connection permit: the
    /// reader's task holds both for as long as the pair is joined.
    deliver: oneshot::Sender<(S, OwnedSemaphorePermit)>,
}

struct Table<S> {
    registrations: HashMap<GatewayKey, Registration>,
    waiting: HashMap<JoinToken, Waiting<S>>,
    lookups: HashMap<JoinToken, LookupWait>,
    next: u64,
}

/// The relay. One per listening socket; every connection is handed to it.
pub(crate) struct Relay<S> {
    table: Mutex<Table<S>>,
    timer: Arc<dyn Timer>,
    entropy: Arc<dyn RelayEntropy>,
    proofs: Arc<dyn RelayProofs>,
    limits: RelayLimits,
    connections: Arc<Semaphore>,
    joined: Arc<Semaphore>,
}

impl<S> Relay<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    /// A relay holding nothing yet.
    pub(crate) fn new(
        timer: Arc<dyn Timer>,
        entropy: Arc<dyn RelayEntropy>,
        proofs: Arc<dyn RelayProofs>,
        limits: RelayLimits,
    ) -> Arc<Self> {
        Arc::new(Self {
            table: Mutex::new(Table {
                registrations: HashMap::new(),
                waiting: HashMap::new(),
                lookups: HashMap::new(),
                next: 0,
            }),
            timer,
            entropy,
            proofs,
            limits,
            connections: Arc::new(Semaphore::new(limits.connections)),
            joined: Arc::new(Semaphore::new(limits.joined)),
        })
    }

    /// A permit for one more connection, or `None` when the relay holds as
    /// many as it may: the caller closes that connection unread.
    pub(crate) fn admit(&self) -> Option<OwnedSemaphorePermit> {
        self.connections.clone().try_acquire_owned().ok()
    }

    /// Serve one admitted connection to its end, holding `permit` as long
    /// as the connection is open. Dropping the future closes the
    /// connection and takes back everything it entered in the relay.
    pub(crate) async fn handle(self: Arc<Self>, mut stream: S, permit: OwnedSemaphorePermit) {
        let deadline = self.timer.now() + PREAMBLE_DEADLINE;
        let Ok(Ok(first)) = within(&*self.timer, deadline, read_frame(&mut stream)).await else {
            return;
        };
        match first {
            RelayFrame::Serve { key } => self.serve(stream, key, deadline).await,
            RelayFrame::Connect { key } => self.connect(stream, key).await,
            RelayFrame::Lookup { key } => self.lookup(stream, key).await,
            // The permit goes with the stream to the reader's task.
            RelayFrame::Accept { key, token } => return self.accept(stream, key, token, permit),
            other => tracing::debug!(frame = ?other, "relay connection opened with another frame"),
        }
        drop(permit);
    }

    /// Prove `key`, register it in place of any earlier connection for it,
    /// then hand it its readers and keep it pinged until it goes (rows RL1
    /// to RL3 and RL7).
    async fn serve(&self, mut stream: S, key: GatewayKey, deadline: Moment) {
        let mut nonce = [0; 32];
        self.entropy.fill(&mut nonce);
        if !self
            .send_by(&mut stream, RelayFrame::Challenge { nonce }, deadline)
            .await
        {
            return;
        }
        let proof = within(&*self.timer, deadline, read_frame(&mut stream)).await;
        let Ok(Ok(RelayFrame::Proof { signature })) = proof else {
            return;
        };
        if !self.proofs.verify(&key, &nonce, &signature) {
            tracing::debug!(key = %key_id(&key), "relay proof refused");
            return;
        }
        let (incoming, mut tokens) =
            mpsc::channel(self.limits.waiting_per_gateway.saturating_mul(2).max(1));
        let (replaced, mut replacing) = oneshot::channel();
        let id = {
            let mut table = self.lock();
            if !table.registrations.contains_key(&key)
                && table.registrations.len() >= self.limits.registrations
            {
                return;
            }
            table.next += 1;
            let id = table.next;
            let earlier = table.registrations.insert(
                key,
                Registration {
                    id,
                    incoming,
                    replaced,
                    origin: PublishedOrigin::clear(),
                },
            );
            if let Some(earlier) = earlier {
                let _ = earlier.replaced.send(());
            }
            id
        };
        let (reader, mut writer) = split(stream);
        // Declared after the stream's halves, so it is dropped before them:
        // the key is gone from the table by the time the gateway sees its
        // connection close.
        let _entered = Registered {
            relay: self,
            key,
            id,
        };
        // Pings fall due every interval from the registration, however long
        // each write took.
        let mut next_ping = self.timer.now() + PING_INTERVAL;
        if self.send(&mut writer, RelayFrame::Registered).await {
            let mut unanswered = 0;
            let mut reading = Box::pin(next_frame(reader));
            loop {
                // What has arrived is taken before a ping falls due: a
                // `Pong` already read counts against the next one.
                tokio::select! {
                    biased;
                    _ = &mut replacing => break,
                    (reader, frame) = &mut reading => match frame {
                        Ok(RelayFrame::Pong) => {
                            unanswered = 0;
                            reading = Box::pin(next_frame(reader));
                        }
                        Ok(RelayFrame::Publish { origin }) => {
                            self.set_origin(key, id, *origin);
                            reading = Box::pin(next_frame(reader));
                        }
                        Ok(RelayFrame::Allow { token }) => {
                            self.answer_lookup(token, true);
                            reading = Box::pin(next_frame(reader));
                        }
                        Ok(RelayFrame::Deny { token }) => {
                            self.answer_lookup(token, false);
                            reading = Box::pin(next_frame(reader));
                        }
                        _ => break,
                    },
                    note = tokens.recv() => {
                        let Some(note) = note else { break };
                        let frame = match note {
                            ToGateway::Incoming(token) => RelayFrame::Incoming { token },
                            ToGateway::Admit { token, nonce, mac } => {
                                RelayFrame::Admit { token, nonce, mac }
                            }
                        };
                        if !self.send(&mut writer, frame).await {
                            break;
                        }
                    }
                    () = self.timer.sleep_until(next_ping) => {
                        if unanswered == MISSED_PINGS {
                            tracing::debug!("relay registration missed its pings");
                            break;
                        }
                        if !self.send(&mut writer, RelayFrame::Ping).await {
                            break;
                        }
                        unanswered += 1;
                        next_ping = next_ping + PING_INTERVAL;
                    }
                }
            }
        }
    }

    /// Tell a paired device the tunnel origin `key` has published, or refuse.
    async fn lookup(&self, mut stream: S, key: GatewayKey) {
        enum Prepared<I> {
            Offline,
            Full,
            Ready {
                token: JoinToken,
                nonce: [u8; 32],
                waiting: oneshot::Receiver<bool>,
                incoming: I,
            },
        }
        let prepared = {
            let mut table = self.lock();
            let incoming = table
                .registrations
                .get(&key)
                .map(|registration| registration.incoming.clone());
            if let Some(incoming) = incoming {
                let waiting = table
                    .lookups
                    .values()
                    .filter(|lookup| lookup.key == key)
                    .count();
                if waiting >= self.limits.waiting_per_gateway {
                    Prepared::Full
                } else {
                    let mut nonce = [0; 32];
                    self.entropy.fill(&mut nonce);
                    let token = loop {
                        let mut token = [0; 16];
                        self.entropy.fill(&mut token);
                        if !table.lookups.contains_key(&token) {
                            break token;
                        }
                    };
                    let (answer, waiting) = oneshot::channel();
                    table.lookups.insert(token, LookupWait { key, answer });
                    Prepared::Ready {
                        token,
                        nonce,
                        waiting,
                        incoming,
                    }
                }
            } else {
                Prepared::Offline
            }
        };
        let (token, nonce, waiting, incoming) = match prepared {
            Prepared::Offline => {
                self.send(&mut stream, RelayFrame::Offline).await;
                return;
            }
            Prepared::Full => {
                self.send(&mut stream, RelayFrame::Unlisted).await;
                return;
            }
            Prepared::Ready {
                token,
                nonce,
                waiting,
                incoming,
            } => (token, nonce, waiting, incoming),
        };
        let guard = LookupGuard { relay: self, token };
        if !self
            .send(&mut stream, RelayFrame::Challenge { nonce })
            .await
        {
            return;
        }
        let deadline = self.timer.now() + PREAMBLE_DEADLINE;
        let seal = within(&*self.timer, deadline, read_frame(&mut stream)).await;
        let Ok(Ok(RelayFrame::Seal { mac })) = seal else {
            self.send(&mut stream, RelayFrame::Unlisted).await;
            return;
        };
        if incoming
            .try_send(ToGateway::Admit { token, nonce, mac })
            .is_err()
        {
            self.send(&mut stream, RelayFrame::Unlisted).await;
            return;
        }
        let deadline = self.timer.now() + PREAMBLE_DEADLINE;
        let allowed = within(&*self.timer, deadline, waiting).await;
        if !matches!(allowed, Ok(Ok(true))) {
            self.send(&mut stream, RelayFrame::Unlisted).await;
            return;
        }
        let origin = self
            .lock()
            .registrations
            .get(&key)
            .map(|registration| registration.origin)
            .unwrap_or_else(PublishedOrigin::clear);
        self.send(
            &mut stream,
            RelayFrame::Located {
                origin: Box::new(origin),
            },
        )
        .await;
        drop(guard);
    }

    fn set_origin(&self, key: GatewayKey, id: u64, origin: PublishedOrigin) {
        let mut table = self.lock();
        if let Some(registration) = table.registrations.get_mut(&key) {
            if registration.id == id {
                registration.origin = origin;
            }
        }
    }

    fn answer_lookup(&self, token: JoinToken, allow: bool) {
        if let Some(waiting) = self.lock().lookups.remove(&token) {
            let _ = waiting.answer.send(allow);
        }
    }

    /// Join a reader to the gateway registered under `key`, or say why not
    /// (rows RL4, RL4b and RL6).
    async fn connect(&self, mut stream: S, key: GatewayKey) {
        let (token, joining, slot) = match self.wait_for(key) {
            Ok(waiting) => waiting,
            Err(answer) => {
                self.send(&mut stream, answer).await;
                return;
            }
        };
        let _entered = Waited { relay: self, token };
        let deadline = self.timer.now() + JOIN_DEADLINE;
        let gateway = within(&*self.timer, deadline, joining).await;
        let Ok(Ok((mut gateway, _gateway_permit))) = gateway else {
            self.send(&mut stream, RelayFrame::NoAnswer).await;
            return;
        };
        if !self.send(&mut stream, RelayFrame::Joined).await
            || !self.send(&mut gateway, RelayFrame::Joined).await
        {
            return;
        }
        join(stream, gateway).await;
        drop(slot);
    }

    /// Enter a reader as waiting for `key` and send its token to `key`'s
    /// serve loop. Else the answer for the reader, decided under the same
    /// lock: `Offline` when `key` has no registration, `NoAnswer` when a
    /// limit is reached. Nothing is left entered then.
    #[allow(clippy::type_complexity)]
    fn wait_for(
        &self,
        key: GatewayKey,
    ) -> Result<
        (
            JoinToken,
            oneshot::Receiver<(S, OwnedSemaphorePermit)>,
            OwnedSemaphorePermit,
        ),
        RelayFrame,
    > {
        let slot = self.joined.clone().try_acquire_owned().ok();
        let mut table = self.lock();
        let incoming = table
            .registrations
            .get(&key)
            .ok_or(RelayFrame::Offline)?
            .incoming
            .clone();
        let slot = slot.ok_or(RelayFrame::NoAnswer)?;
        let waiting = table.waiting.values().filter(|w| w.key == key).count();
        if waiting >= self.limits.waiting_per_gateway {
            return Err(RelayFrame::NoAnswer);
        }
        let token = loop {
            let mut token = [0; 16];
            self.entropy.fill(&mut token);
            if !table.waiting.contains_key(&token) {
                break token;
            }
        };
        let (deliver, joining) = oneshot::channel();
        incoming
            .try_send(ToGateway::Incoming(token))
            .map_err(|_| RelayFrame::NoAnswer)?;
        table.waiting.insert(token, Waiting { key, deliver });
        Ok((token, joining, slot))
    }

    /// Hand a gateway's new connection to the reader waiting under `token`
    /// (row RL5). A token the relay never sent, one already used, or one
    /// sent for another key is closed, and the waiting reader is untouched.
    fn accept(&self, stream: S, key: GatewayKey, token: JoinToken, permit: OwnedSemaphorePermit) {
        let mut table = self.lock();
        if !table.waiting.get(&token).is_some_and(|w| w.key == key) {
            tracing::debug!("relay accept for no waiting reader");
            return;
        }
        if let Some(waiting) = table.waiting.remove(&token) {
            // A reader whose deadline passed has gone; the stream is closed.
            let _ = waiting.deliver.send((stream, permit));
        }
    }

    /// Write one frame, bounded by the ping interval from now.
    async fn send(&self, stream: &mut (impl AsyncWrite + Unpin), frame: RelayFrame) -> bool {
        let deadline = self.timer.now() + PING_INTERVAL;
        self.send_by(stream, frame, deadline).await
    }

    /// Write one frame, bounded by `deadline`.
    async fn send_by(
        &self,
        stream: &mut (impl AsyncWrite + Unpin),
        frame: RelayFrame,
        deadline: Moment,
    ) -> bool {
        matches!(
            within(&*self.timer, deadline, write_frame(stream, frame)).await,
            Ok(Ok(()))
        )
    }
}

impl<S> Relay<S> {
    fn lock(&self) -> MutexGuard<'_, Table<S>> {
        self.table.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A serve connection's registration, removed when the connection's task
/// ends however it ends, unless a newer one has taken the key.
struct Registered<'a, S> {
    relay: &'a Relay<S>,
    key: GatewayKey,
    id: u64,
}

impl<S> Drop for Registered<'_, S> {
    fn drop(&mut self) {
        let mut table = self.relay.lock();
        if table
            .registrations
            .get(&self.key)
            .is_some_and(|registration| registration.id == self.id)
        {
            table.registrations.remove(&self.key);
        }
        table.lookups.retain(|_, lookup| lookup.key != self.key);
    }
}

/// Removes a lookup that ends before the gateway answers it.
struct LookupGuard<'a, S> {
    relay: &'a Relay<S>,
    token: JoinToken,
}

impl<S> Drop for LookupGuard<'_, S> {
    fn drop(&mut self) {
        self.relay.lock().lookups.remove(&self.token);
    }
}

/// A reader's waiting entry, removed when its task ends however it ends.
/// After a join the entry is already gone and this does nothing.
struct Waited<'a, S> {
    relay: &'a Relay<S>,
    token: JoinToken,
}

impl<S> Drop for Waited<'_, S> {
    fn drop(&mut self) {
        self.relay.lock().waiting.remove(&self.token);
    }
}

/// Copy both ways until either side ends, then close both (row RL6). There
/// is no half-close: what one side still sends after the other stopped
/// sending is dropped with the pair. The two ends speak TLS, whose own
/// close needs nothing more.
async fn join<S>(reader: S, gateway: S)
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let (mut reader_in, mut reader_out) = split(reader);
    let (mut gateway_in, mut gateway_out) = split(gateway);
    tokio::select! {
        _ = tokio::io::copy(&mut reader_in, &mut gateway_out) => {}
        _ = tokio::io::copy(&mut gateway_in, &mut reader_out) => {}
    }
}

#[cfg(test)]
#[path = "../../tests/relay/relay.rs"]
mod tests;
