//! Bounded physical enrollment connections, including detached caller ownership.
//!
//! The socket each connection runs on, its phase deadlines and its wake, are
//! `nessa_protocol::pairing::socket`, shared with the device's enrollment client.
use super::{
    environment_peers::EnvironmentPeers,
    protected::{ProtectedConnection, ProtectedSessions},
    tls_stream::TlsStream,
    BeginPairing, GatewayPairing, PairingRuntimeError,
};
use nessa_auth::{
    adapters::pairing::{CryptoRng, NativeTransport, PairingCryptoError, RngCore},
    application::pairing::{DeviceConnectionProof, PairingStoreError, PairingWorkerFault},
    domain::pairing::{AttemptFailure, PublicIntent},
};
use nessa_protocol::clock::Clock;
use nessa_protocol::pairing::socket::{
    begin_enrollment_phase, check_deadline, worker_fault, DeadlineError, DeadlineStream,
    NativeDeadline, NativeWakeReport, WakeEndpoint, WakeEndpoints, NATIVE_CONNECTION_CAPACITY,
};
use nessa_protocol::pairing::{
    wire::{self, NativePairingRequest, NativeWireError},
    DevicePairingStatus, EnrollmentChannel, NativeFrameError,
};
use std::{
    io::{ErrorKind, Read, Write},
    net::TcpStream,
    sync::{Arc, Mutex, PoisonError},
};
use tokio::{
    runtime::Handle,
    sync::{Notify, OwnedSemaphorePermit, Semaphore},
    task::JoinHandle,
};

/// Bound on protected product sessions, a pool of their own so sessions never
/// take the permits enrollment and pinned status need (design row PR10).
/// Equal to the connection bound, which also sizes the wake report.
const PRODUCT_SESSION_CAPACITY: usize = NATIVE_CONNECTION_CAPACITY;
/// Bound on paired environments' connections, a pool of their own for the
/// same reason; also the connection bound, which sizes the wake report.
const ENVIRONMENT_CAPACITY: usize = NATIVE_CONNECTION_CAPACITY;
/// Bound on connections a relay joined, and on readers being accepted
/// through it: a pool of their own, so readers a relay sends, however many,
/// never take the connection permits direct readers need
/// (`docs/design/auth/relay.md`). A relayed connection that opens a product
/// session or an environment moves to that shared pool like a direct one;
/// only a paired key gets that far.
pub(crate) const RELAY_CAPACITY: usize = 4;
/// Bound on connections that came through the web tunnel, from before their
/// WebSocket upgrade until they end: a pool of their own for the same reason
/// as the relay's, since anyone with the tunnel's public address can open
/// them (`docs/design/auth/tunnel.md`).
pub(crate) const TUNNEL_CAPACITY: usize = 4;

/// Redacted primary physical/protocol refusal, without peer input diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeConnectionFailure {
    /// Capacity is physically occupied or shutdown has excluded admission.
    Capacity,
    /// Actual TLS owner rejected this channel before enrollment admission.
    Crypto(PairingCryptoError),
    /// Current codec refused the bounded peer representation.
    Wire(NativeWireError),
    /// Wrong current enrollment phase or operation correlation.
    Phase,
    /// Actual socket operation failed, retaining its standard error category.
    Io(ErrorKind),
    /// Canonical pairing owner refused this operation.
    Runtime(PairingRuntimeError),
    /// Unexpected outer worker fault, not ordinary service unavailability.
    WorkerFault(PairingWorkerFault),
    /// The claim committed, but its reply could not be encoded or sent. The
    /// claim stands; the device recovers it with a pinned status request.
    ClaimReply(NativeFrameError),
    /// `openProduct` arrived but no protected product sessions are composed.
    ProductUnavailable,
    /// `openProduct` arrived while every product session permit is held, or
    /// after shutdown closed product admission.
    ProductCapacity,
    /// `openProduct` from a key that holds no active device credential.
    ProductUnregistered,
    /// Whether the key holds a device credential could not be read.
    ProductUnverifiable,
    /// `openEnvironment` arrived but no paired environments are composed.
    EnvironmentUnavailable,
    /// `openEnvironment` arrived while every environment permit is held, or
    /// after shutdown closed environment admission.
    EnvironmentCapacity,
    /// `openEnvironment` from a key that holds no credential to serve.
    EnvironmentUnregistered,
    /// Whether the key holds a credential to serve could not be read.
    EnvironmentUnverifiable,
    /// A connection through the tunnel whose key holds no active credential:
    /// closed after its handshake, before its first request is read.
    TunnelUnregistered,
    /// Whether a tunnel connection's key holds a credential could not be
    /// read; closed the same way.
    TunnelUnverifiable,
    /// A tunnel connection asked for anything but a product session or an
    /// environment: pairing, status and owner recovery stay on the local
    /// network, whatever the key holds (design row T2).
    TunnelLocalOnly,
}
/// Primary failure and independent failure to settle the original charged attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeConnectionError {
    /// Original actual failure, preserved if settlement also fails.
    pub failure: NativeConnectionFailure,
    /// No invented outcome when the canonical settlement failed.
    pub settlement: Option<PairingStoreError>,
}
impl From<NativeConnectionFailure> for NativeConnectionError {
    fn from(failure: NativeConnectionFailure) -> Self {
        Self {
            failure,
            settlement: None,
        }
    }
}
impl From<DeadlineError> for NativeConnectionError {
    fn from(error: DeadlineError) -> Self {
        match error {
            DeadlineError::Io(kind) => NativeConnectionFailure::Io(kind),
            DeadlineError::Phase => NativeConnectionFailure::Phase,
        }
        .into()
    }
}

/// One fixed eight-connection physical owner; admission queues no requests.
/// A connection a relay joined comes in on a relay permit instead, from a
/// pool of four, and holds it in place of a connection permit; one that came
/// through the tunnel likewise on a tunnel permit, from a pool of four.
/// A connection whose first envelope is `openProduct` moves to a pool of its
/// own, also eight, and returns its connection permit, so product sessions
/// never take the permits enrollment and pinned status need (design row PR10).
/// One whose first envelope is `openEnvironment` moves to a third pool, also
/// eight, the same way.
pub struct NativeEnrollmentConnections {
    gateway: Arc<GatewayPairing>,
    clock: Arc<dyn Clock>,
    capacity: Arc<Semaphore>,
    drained: Arc<Notify>,
    wake: Arc<Mutex<WakeEndpoints>>,
    product: Pool,
    sessions: Option<Arc<dyn ProtectedSessions>>,
    environment: Pool,
    peers: Option<Arc<dyn EnvironmentPeers>>,
    relay: Pool,
    tunnel: Pool,
}

/// Where an admitted connection came from, and the permit it holds when that
/// is not the connection pool's.
enum Source {
    Direct,
    Relay(RelayPermit),
    Tunnel(TunnelPermit),
}

/// A pool a connection moves to from the connection pool, selected by its
/// first envelope: its permits and the wake sweep of the connections on it.
#[derive(Clone)]
struct Pool {
    capacity: Arc<Semaphore>,
    wake: Arc<Mutex<WakeEndpoints>>,
    size: usize,
}

impl Pool {
    fn new(size: usize) -> Self {
        Self {
            capacity: Arc::new(Semaphore::new(size)),
            wake: Arc::new(Mutex::new(WakeEndpoints::new())),
            size,
        }
    }

    fn close(&self) {
        let mut wake = self.wake.lock().unwrap_or_else(PoisonError::into_inner);
        self.capacity.close();
        wake.close();
    }

    fn report(&self) -> Option<NativeWakeReport> {
        self.wake
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .report()
    }

    fn drained(&self) -> bool {
        self.capacity.available_permits() == self.size
    }
}
impl NativeEnrollmentConnections {
    /// Compose the actual canonical enrollment runtime before accepting sockets.
    /// Without [`Self::with_protected_sessions`], `openProduct` is refused.
    pub fn new(gateway: Arc<GatewayPairing>, clock: Arc<dyn Clock>) -> Self {
        Self {
            gateway,
            clock,
            capacity: Arc::new(Semaphore::new(NATIVE_CONNECTION_CAPACITY)),
            drained: Arc::new(Notify::new()),
            wake: Arc::new(Mutex::new(WakeEndpoints::new())),
            product: Pool::new(PRODUCT_SESSION_CAPACITY),
            sessions: None,
            environment: Pool::new(ENVIRONMENT_CAPACITY),
            peers: None,
            relay: Pool::new(RELAY_CAPACITY),
            tunnel: Pool::new(TUNNEL_CAPACITY),
        }
    }
    /// Serve a connection whose first envelope is `openProduct` with
    /// `sessions`, on a product session permit (design rows PR1, PR10, PR13).
    pub fn with_protected_sessions(mut self, sessions: Arc<dyn ProtectedSessions>) -> Self {
        self.sessions = Some(sessions);
        self
    }
    /// Serve a connection whose first envelope is `openEnvironment` with
    /// `peers`, on an environment permit. Without it, `openEnvironment` is
    /// refused.
    pub fn with_environment_peers(mut self, peers: Arc<dyn EnvironmentPeers>) -> Self {
        self.peers = Some(peers);
        self
    }
    /// Admit one accepted native TCP socket; the blocking worker owns all inputs.
    /// Dropping this future leaves the permit with the worker until it ends
    /// (`native_shutdown_keeps_original_physical_capacity`).
    pub async fn serve<R: RngCore + CryptoRng + Send + 'static>(
        &self,
        stream: TcpStream,
        entropy: R,
    ) -> Result<(), NativeConnectionError> {
        self.start(stream, entropy)?.wait().await
    }
    /// A permit for one more connection through the relay, or `None` when
    /// the relay pool is full or closed. A gateway takes it before it
    /// accepts a reader the relay announced, and the connection holds it
    /// once [`Self::start_relayed`] admits it.
    pub fn relay_permit(&self) -> Option<RelayPermit> {
        let permit = self.relay.capacity.clone().try_acquire_owned().ok()?;
        Some(RelayPermit {
            permit: Some(permit),
            drained: self.drained.clone(),
        })
    }
    /// A permit for one more connection through the tunnel, or `None` when
    /// the tunnel pool is full or closed. The tunnel's ingress takes it
    /// before it upgrades a WebSocket, and the connection holds it once
    /// [`Self::start_tunneled`] admits it.
    pub fn tunnel_permit(&self) -> Option<TunnelPermit> {
        let permit = self.tunnel.capacity.clone().try_acquire_owned().ok()?;
        Some(TunnelPermit(RelayPermit {
            permit: Some(permit),
            drained: self.drained.clone(),
        }))
    }
    pub(super) fn start<R: RngCore + CryptoRng + Send + 'static>(
        &self,
        stream: TcpStream,
        entropy: R,
    ) -> Result<NativeConnectionTask, NativeConnectionError> {
        self.start_from(stream, entropy, Source::Direct)
    }
    /// Admit a connection the relay joined, on the relay permit it holds
    /// rather than a connection permit. A closed owner admits nothing.
    pub(super) fn start_relayed<R: RngCore + CryptoRng + Send + 'static>(
        &self,
        stream: TcpStream,
        permit: RelayPermit,
        entropy: R,
    ) -> Result<NativeConnectionTask, NativeConnectionError> {
        self.start_from(stream, entropy, Source::Relay(permit))
    }
    /// Admit a connection that came through the tunnel, on the tunnel permit
    /// it holds. Its key must hold an active credential before anything it
    /// sends is read. A closed owner admits nothing.
    pub(super) fn start_tunneled<R: RngCore + CryptoRng + Send + 'static>(
        &self,
        stream: TcpStream,
        permit: TunnelPermit,
        entropy: R,
    ) -> Result<NativeConnectionTask, NativeConnectionError> {
        self.start_from(stream, entropy, Source::Tunnel(permit))
    }
    fn start_from<R: RngCore + CryptoRng + Send + 'static>(
        &self,
        stream: TcpStream,
        entropy: R,
        source: Source,
    ) -> Result<NativeConnectionTask, NativeConnectionError> {
        let endpoint = WakeEndpoint::new(
            stream
                .peer_addr()
                .map_err(|error| NativeConnectionFailure::Io(error.kind()))?,
        );
        let (stream, deadline) = DeadlineStream::new(stream, self.clock.clone(), endpoint.clone())?;
        // The sweep a connection starts on: the connection pool's, or the
        // relay's or tunnel's pool's for one that came that way.
        let connections = match source {
            Source::Direct => self.wake.clone(),
            Source::Relay(_) => self.relay.wake.clone(),
            Source::Tunnel(_) => self.tunnel.wake.clone(),
        };
        // `close` closes the semaphore under this lock, so a permit acquired here
        // is registered before any wake sweep, and a closed owner admits nothing.
        let mut wake = connections.lock().unwrap_or_else(PoisonError::into_inner);
        let capacity = match &source {
            Source::Direct => &self.capacity,
            Source::Relay(_) => &self.relay.capacity,
            Source::Tunnel(_) => &self.tunnel.capacity,
        };
        if capacity.is_closed() {
            return Err(NativeConnectionFailure::Capacity.into());
        }
        let tunneled = matches!(source, Source::Tunnel(_));
        let permit = match source {
            Source::Direct => self.capacity.clone().try_acquire_owned().ok(),
            Source::Relay(mut relayed) | Source::Tunnel(TunnelPermit(mut relayed)) => {
                relayed.permit.take()
            }
        }
        .ok_or(NativeConnectionError::from(
            NativeConnectionFailure::Capacity,
        ))?;
        wake.register(&endpoint);
        let lease = Arc::new(ConnectionPermit {
            permit: Some(permit),
            drained: self.drained.clone(),
            endpoint: Some(endpoint),
        });
        let permit = lease.clone();
        let gateway = self.gateway.clone();
        let handle = Handle::current();
        let admission = |pool: &Pool| Admission {
            capacity: pool.capacity.clone(),
            wake: pool.wake.clone(),
            connections: connections.clone(),
            endpoint: endpoint_of(&lease),
            drained: self.drained.clone(),
        };
        let admissions = Admissions {
            product: self.sessions.as_ref().map(|sessions| ProductAdmission {
                sessions: sessions.clone(),
                pool: admission(&self.product),
            }),
            environment: self.peers.as_ref().map(|peers| EnvironmentAdmission {
                peers: peers.clone(),
                pool: admission(&self.environment),
            }),
        };
        let worker = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            // Locals unwind physical IO and its runtime before the original permit.
            let runtime = gateway;
            serve_exchange(
                &runtime, &handle, stream, deadline, entropy, admissions, tunneled,
            )
        });
        drop(wake);
        Ok(NativeConnectionTask {
            worker,
            lease,
            sessions: self.sessions.clone(),
            peers: self.peers.clone(),
        })
    }
    pub(crate) fn close(&self) {
        let mut wake = self.wake.lock().unwrap_or_else(PoisonError::into_inner);
        self.capacity.close();
        wake.close();
        drop(wake);
        self.product.close();
        self.environment.close();
        self.relay.close();
        self.tunnel.close();
    }
    /// The wake sweep from the first shutdown, if it has happened. A successful
    /// wake does not mean the worker has finished; `shutdown` reports that.
    pub fn wake_report(&self) -> Option<NativeWakeReport> {
        self.wake
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .report()
    }
    /// The product sessions' wake sweep from the first shutdown, if it has
    /// happened; as with [`Self::wake_report`], not evidence of a drain.
    pub fn product_wake_report(&self) -> Option<NativeWakeReport> {
        self.product.report()
    }
    /// The paired environments' wake sweep from the first shutdown, if it
    /// has happened; likewise not evidence of a drain.
    pub fn environment_wake_report(&self) -> Option<NativeWakeReport> {
        self.environment.report()
    }
    /// The relayed connections' wake sweep from the first shutdown, if it
    /// has happened; likewise not evidence of a drain.
    pub fn relay_wake_report(&self) -> Option<NativeWakeReport> {
        self.relay.report()
    }
    /// The tunnel connections' wake sweep from the first shutdown, if it has
    /// happened; likewise not evidence of a drain.
    pub fn tunnel_wake_report(&self) -> Option<NativeWakeReport> {
        self.tunnel.report()
    }
    /// Exclude new sockets and await actual blocked/unwinding closure drain,
    /// of connections, relayed connections, product sessions and
    /// environments alike. A relay permit a gateway took for a reader it is
    /// still accepting counts until that accept ends.
    pub async fn shutdown(&self) {
        self.close();
        loop {
            let notified = self.drained.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.capacity.available_permits() == NATIVE_CONNECTION_CAPACITY
                && self.product.drained()
                && self.environment.drained()
                && self.relay.drained()
                && self.tunnel.drained()
            {
                return;
            }
            notified.await;
        }
    }
}
pub(super) struct NativeConnectionTask {
    worker: JoinHandle<Result<Served, NativeConnectionError>>,
    lease: Arc<ConnectionPermit>,
    sessions: Option<Arc<dyn ProtectedSessions>>,
    peers: Option<Arc<dyn EnvironmentPeers>>,
}
impl NativeConnectionTask {
    /// The blocking worker's outcome. A connection it handed to the product
    /// returns its connection permit and is served here on its product
    /// session permit until the session ends (design rows PR1, PR10, PR11);
    /// one it handed to the paired environments, likewise on its environment
    /// permit.
    pub(super) async fn complete(self) -> NativeConnectionCompletion {
        let mut lease = Some(self.lease);
        let outcome = self
            .worker
            .await
            .map_err(|error| {
                NativeConnectionError::from(NativeConnectionFailure::WorkerFault(worker_fault(
                    error,
                )))
            })
            .and_then(|result| result);
        let outcome = match outcome {
            Ok(Served::Enrollment) => Ok(()),
            Ok(Served::Product(channel, product)) => match self.sessions {
                Some(sessions) => {
                    drop(lease.take());
                    let served = match ProtectedConnection::open(channel.into_transport()) {
                        Ok(connection) => {
                            sessions.serve(connection).await;
                            Ok(())
                        }
                        Err(error) => Err(NativeConnectionFailure::Io(error.kind()).into()),
                    };
                    drop(product);
                    served
                }
                None => Err(NativeConnectionFailure::ProductUnavailable.into()),
            },
            Ok(Served::Environment(channel, environment)) => match self.peers {
                Some(peers) => {
                    drop(lease.take());
                    let served = match TlsStream::open(channel.into_transport()) {
                        Ok(stream) => {
                            peers.serve(stream).await;
                            Ok(())
                        }
                        Err(error) => Err(NativeConnectionFailure::Io(error.kind()).into()),
                    };
                    drop(environment);
                    served
                }
                None => Err(NativeConnectionFailure::EnvironmentUnavailable.into()),
            },
            Err(error) => Err(error),
        };
        NativeConnectionCompletion { outcome, lease }
    }
    async fn wait(self) -> Result<(), NativeConnectionError> {
        self.complete().await.into_result()
    }
}
pub(super) struct NativeConnectionCompletion {
    outcome: Result<(), NativeConnectionError>,
    lease: Option<Arc<ConnectionPermit>>,
}
impl NativeConnectionCompletion {
    pub(super) fn into_result(self) -> Result<(), NativeConnectionError> {
        drop(self.lease);
        self.outcome
    }
}
struct ConnectionPermit {
    endpoint: Option<Arc<WakeEndpoint>>,
    permit: Option<OwnedSemaphorePermit>,
    drained: Arc<Notify>,
}
impl Drop for ConnectionPermit {
    fn drop(&mut self) {
        drop(self.endpoint.take());
        drop(self.permit.take());
        self.drained.notify_waiters();
    }
}
fn endpoint_of(lease: &ConnectionPermit) -> Arc<WakeEndpoint> {
    lease
        .endpoint
        .clone()
        .expect("a live connection permit holds its endpoint")
}
/// A relay pool permit, taken before a reader the relay announced is
/// accepted and held by its connection once admitted. Dropping it wakes a
/// shutdown waiting for the drain.
pub struct RelayPermit {
    permit: Option<OwnedSemaphorePermit>,
    drained: Arc<Notify>,
}
impl Drop for RelayPermit {
    fn drop(&mut self) {
        drop(self.permit.take());
        self.drained.notify_waiters();
    }
}
impl std::fmt::Debug for RelayPermit {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("RelayPermit")
    }
}
/// A tunnel pool permit, taken before the tunnel's ingress upgrades a
/// WebSocket and held by its connection once admitted. Dropping it wakes a
/// shutdown waiting for the drain.
pub struct TunnelPermit(RelayPermit);
impl std::fmt::Debug for TunnelPermit {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("TunnelPermit")
    }
}
/// What the blocking worker may move a connection to, by its first envelope.
struct Admissions {
    product: Option<ProductAdmission>,
    environment: Option<EnvironmentAdmission>,
}
impl Admissions {
    /// The tunnel's edge: whether the key a connection proved holds an
    /// active credential, as a paired device or as an environment that may
    /// serve. The same reads `openProduct` and `openEnvironment` make before
    /// they take a permit, asked before anything the connection sends.
    fn admit_tunneled(
        &self,
        handle: &Handle,
        proof: &DeviceConnectionProof,
    ) -> Result<(), NativeConnectionError> {
        let mut unverifiable = false;
        if let Some(product) = &self.product {
            match product.sessions.admits(proof) {
                Ok(true) => return Ok(()),
                Ok(false) => {}
                Err(_) => unverifiable = true,
            }
        }
        if let Some(environment) = &self.environment {
            match handle.block_on(environment.peers.admits(proof)) {
                Ok(true) => return Ok(()),
                Ok(false) => {}
                Err(_) => unverifiable = true,
            }
        }
        Err(if unverifiable {
            NativeConnectionFailure::TunnelUnverifiable
        } else {
            NativeConnectionFailure::TunnelUnregistered
        }
        .into())
    }
}
/// Admission to the product session pool, taken by the blocking worker when
/// the first envelope is `openProduct`.
struct ProductAdmission {
    sessions: Arc<dyn ProtectedSessions>,
    pool: Admission,
}
/// Admission to the environment pool, taken by the blocking worker when the
/// first envelope is `openEnvironment`.
struct EnvironmentAdmission {
    peers: Arc<dyn EnvironmentPeers>,
    pool: Admission,
}
/// One connection's way into a pool of its own.
struct Admission {
    capacity: Arc<Semaphore>,
    wake: Arc<Mutex<WakeEndpoints>>,
    connections: Arc<Mutex<WakeEndpoints>>,
    endpoint: Arc<WakeEndpoint>,
    drained: Arc<Notify>,
}
impl Admission {
    /// Take a permit and move the connection's wake endpoint to the pool's
    /// sweep. `close` closes the pool under the same lock, so a connection
    /// admitted here is swept, and a closed pool admits nothing; a full or
    /// closed one is `full`.
    fn admit(&self, full: NativeConnectionFailure) -> Result<PoolPermit, NativeConnectionError> {
        let mut wake = self.wake.lock().unwrap_or_else(PoisonError::into_inner);
        let permit = self
            .capacity
            .clone()
            .try_acquire_owned()
            .map_err(|_| NativeConnectionError::from(full))?;
        wake.register(&self.endpoint);
        drop(wake);
        self.connections
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.endpoint);
        Ok(PoolPermit {
            endpoint: Some(self.endpoint.clone()),
            permit: Some(permit),
            drained: self.drained.clone(),
        })
    }
}
/// One product session's or environment's permit, held until it ends.
pub(super) struct PoolPermit {
    endpoint: Option<Arc<WakeEndpoint>>,
    permit: Option<OwnedSemaphorePermit>,
    drained: Arc<Notify>,
}
impl Drop for PoolPermit {
    fn drop(&mut self) {
        drop(self.endpoint.take());
        drop(self.permit.take());
        self.drained.notify_waiters();
    }
}

/// What the blocking worker did with its connection.
pub(super) enum Served {
    /// An enrollment exchange or status, finished.
    Enrollment,
    /// The first envelope was `openProduct`: the TLS connection, for the
    /// product session, and its product session permit.
    Product(Box<EnrollmentChannel<DeadlineStream>>, PoolPermit),
    /// The first envelope was `openEnvironment`, answered `serving`: the TLS
    /// connection, for the paired environment, and its environment permit.
    Environment(Box<EnrollmentChannel<DeadlineStream>>, PoolPermit),
}
fn serve_exchange<R: RngCore + CryptoRng>(
    gateway: &GatewayPairing,
    handle: &Handle,
    stream: DeadlineStream,
    deadline: Arc<NativeDeadline>,
    mut entropy: R,
    admissions: Admissions,
    tunneled: bool,
) -> Result<Served, NativeConnectionError> {
    stream.blocking()?;
    let channel = NativeTransport::accept(stream, gateway.identity())
        .map_err(|error| NativeConnectionError::from(NativeConnectionFailure::Crypto(error)))?;
    // Through the tunnel, nothing is read from a key the gateway issued no
    // credential to: no pairing, no status, no product (design row T2). The
    // refusal is the close itself, with nothing said.
    if tunneled {
        admissions.admit_tunneled(handle, channel.device_proof())?;
    }
    let mut channel = EnrollmentChannel::new(channel);
    let result = exchange(
        gateway,
        handle,
        &mut channel,
        &deadline,
        &mut entropy,
        &admissions,
        tunneled,
    );
    match result {
        Ok(Exchanged::ProductSelected(permit)) => Ok(Served::Product(Box::new(channel), permit)),
        Ok(Exchanged::EnvironmentSelected(permit)) => {
            Ok(Served::Environment(Box::new(channel), permit))
        }
        Ok(Exchanged::Finished) => Ok(Served::Enrollment),
        Err(error) => {
            if answers_with_refusal(error.failure) {
                // Best effort: the primary failure is what this connection reports,
                // whether or not the peer receives the refusal.
                write_reply(&mut channel, wire::encode_refused()).ok();
            }
            Err(error)
        }
    }
}
/// A decision the gateway made on a readable channel gets a redacted `Refused`
/// reply; physical failures do not, because the channel may be unusable.
/// `native_expired_and_used_codes_are_refused_without_a_claim` covers the reply.
fn answers_with_refusal(failure: NativeConnectionFailure) -> bool {
    match failure {
        NativeConnectionFailure::Runtime(_)
        | NativeConnectionFailure::Phase
        | NativeConnectionFailure::Wire(_)
        | NativeConnectionFailure::ProductUnavailable
        | NativeConnectionFailure::ProductCapacity
        | NativeConnectionFailure::ProductUnregistered
        | NativeConnectionFailure::ProductUnverifiable
        | NativeConnectionFailure::EnvironmentUnavailable
        | NativeConnectionFailure::EnvironmentCapacity
        | NativeConnectionFailure::EnvironmentUnregistered
        | NativeConnectionFailure::EnvironmentUnverifiable => true,
        NativeConnectionFailure::Capacity
        | NativeConnectionFailure::Crypto(_)
        | NativeConnectionFailure::Io(_)
        | NativeConnectionFailure::WorkerFault(_)
        | NativeConnectionFailure::ClaimReply(_)
        | NativeConnectionFailure::TunnelUnregistered
        | NativeConnectionFailure::TunnelUnverifiable
        | NativeConnectionFailure::TunnelLocalOnly => false,
    }
}
/// How an exchange ended on a readable channel.
enum Exchanged {
    /// Enrollment or status answered.
    Finished,
    /// The first envelope selected the product session, admitted to its pool.
    ProductSelected(PoolPermit),
    /// The first envelope selected the environment phase, admitted to its
    /// pool and answered `serving`.
    EnvironmentSelected(PoolPermit),
}
/// One enrollment exchange. Only a first envelope can select the product
/// session, only when `product` admission is composed, and only with a free
/// product session permit (design rows PR1, PR2, PR10, PR13); the same holds
/// for the environment phase and its admission.
fn exchange<R: RngCore + CryptoRng>(
    gateway: &GatewayPairing,
    handle: &Handle,
    channel: &mut EnrollmentChannel<DeadlineStream>,
    deadline: &NativeDeadline,
    entropy: &mut R,
    admissions: &Admissions,
    tunneled: bool,
) -> Result<Exchanged, NativeConnectionError> {
    begin_enrollment_phase(deadline)?;
    let first = read_request(channel)?;
    check_deadline(deadline)?;
    // A credential lets a tunnel connection in, and only to its session:
    // pairing a code, a status and owner recovery need the local network.
    if tunneled
        && !matches!(
            first,
            NativePairingRequest::OpenProduct | NativePairingRequest::OpenEnvironment
        )
    {
        return Err(NativeConnectionFailure::TunnelLocalOnly.into());
    }
    let public = match first {
        NativePairingRequest::RecoverOwnerPhone { request_id, digest } => {
            let (public, status) = gateway
                .recover_owner_phone(channel.transport(), request_id, &digest)
                .map_err(|error| {
                    NativeConnectionError::from(NativeConnectionFailure::Runtime(error))
                })?;
            check_deadline(deadline)?;
            return write_reply(
                channel,
                wire::encode_owner_phone_recovery(request_id, digest, public, &status),
            )
            .map(|()| Exchanged::Finished);
        }
        NativePairingRequest::OpenProduct => {
            let product = admissions
                .product
                .as_ref()
                .ok_or(NativeConnectionFailure::ProductUnavailable)?;
            // Only a paired device's key may take a product permit (row PR15).
            match product.sessions.admits(channel.transport().device_proof()) {
                Ok(true) => {}
                Ok(false) => return Err(NativeConnectionFailure::ProductUnregistered.into()),
                Err(_) => return Err(NativeConnectionFailure::ProductUnverifiable.into()),
            }
            return product
                .pool
                .admit(NativeConnectionFailure::ProductCapacity)
                .map(Exchanged::ProductSelected);
        }
        NativePairingRequest::OpenEnvironment => {
            let environment = admissions
                .environment
                .as_ref()
                .ok_or(NativeConnectionFailure::EnvironmentUnavailable)?;
            // Only a key that may serve takes an environment permit.
            let admitted =
                handle.block_on(environment.peers.admits(channel.transport().device_proof()));
            match admitted {
                Ok(true) => {}
                Ok(false) => return Err(NativeConnectionFailure::EnvironmentUnregistered.into()),
                Err(_) => return Err(NativeConnectionFailure::EnvironmentUnverifiable.into()),
            }
            let permit = environment
                .pool
                .admit(NativeConnectionFailure::EnvironmentCapacity)?;
            write_reply(channel, wire::encode_serving())?;
            return Ok(Exchanged::EnvironmentSelected(permit));
        }
        NativePairingRequest::Status(public) => {
            return send_status(gateway, handle, channel, public).map(|()| Exchanged::Finished);
        }
        NativePairingRequest::Hello(attempt) => {
            handle.block_on(gateway.hello(attempt)).map_err(|error| {
                NativeConnectionError::from(NativeConnectionFailure::Runtime(error))
            })?
        }
        _ => return Err(NativeConnectionFailure::Phase.into()),
    };
    write_reply(channel, wire::encode_hello(public))?;
    let NativePairingRequest::Begin {
        public: received,
        request,
    } = read_request(channel)?
    else {
        return Err(NativeConnectionFailure::Phase.into());
    };
    check_deadline(deadline)?;
    if received != public {
        return Err(NativeConnectionFailure::Phase.into());
    }
    let handshake = match handle
        .block_on(gateway.begin(channel.transport(), public, &request, entropy))
        .map_err(|error| NativeConnectionError::from(NativeConnectionFailure::Runtime(error)))?
    {
        BeginPairing::Existing(status) => {
            return write_reply(channel, wire::encode_status(public, &status))
                .map(|()| Exchanged::Finished);
        }
        BeginPairing::Admitted(handshake) => handshake,
    };
    // From here, this closure exclusively owns the original completion opportunity.
    let confirmation: Result<Vec<u8>, NativeConnectionError> = (|| {
        write_reply(
            channel,
            wire::encode_challenge(public, handshake.response()),
        )?;
        check_deadline(deadline)?;
        let request = read_request(channel)?;
        admit_confirmation(deadline, public, request)
    })();
    let message = match confirmation {
        Ok(message) => message,
        Err(mut error) => {
            let cause = match error.failure {
                NativeConnectionFailure::Io(ErrorKind::TimedOut) => {
                    AttemptFailure::HandshakeDeadline
                }
                NativeConnectionFailure::Io(_) => AttemptFailure::ConnectionClosed,
                _ => AttemptFailure::InvalidProof,
            };
            error.settlement = gateway
                .end_attempt(channel.transport(), public, cause)
                .err();
            return Err(error);
        }
    };
    let record = handle
        .block_on(gateway.finish(channel.transport(), *handshake, &message))
        .map_err(|error| NativeConnectionError::from(NativeConnectionFailure::Runtime(error)))?;
    // The reply comes from the committed record, with no further read, and any
    // failure from here is `ClaimReply`: the claim stands
    // (`native_committed_claim_is_reported_without_a_later_read`,
    // `native_claim_reply_loss_preserves_pinned_status`).
    let status = DevicePairingStatus::Claimed(Box::new(record));
    let bytes = wire::encode_status(public, &status).map_err(|error| {
        NativeConnectionError::from(NativeConnectionFailure::ClaimReply(NativeFrameError::Wire(
            error,
        )))
    })?;
    write_reply(channel, Ok(bytes))
        .map(|()| Exchanged::Finished)
        .map_err(|error| match error.failure {
            NativeConnectionFailure::Io(kind) => {
                NativeConnectionFailure::ClaimReply(NativeFrameError::Io(kind)).into()
            }
            NativeConnectionFailure::Wire(wire) => {
                NativeConnectionFailure::ClaimReply(NativeFrameError::Wire(wire)).into()
            }
            _ => error,
        })
}
fn admit_confirmation(
    deadline: &NativeDeadline,
    public: PublicIntent,
    request: NativePairingRequest,
) -> Result<Vec<u8>, NativeConnectionError> {
    // Decoded data may already be buffered; this decision precedes PAKE finish.
    check_deadline(deadline)?;
    match request {
        NativePairingRequest::Confirm {
            public: received,
            message,
        } if received == public => Ok(message),
        _ => Err(NativeConnectionFailure::Phase.into()),
    }
}
fn read_request<S: Read + Write>(
    channel: &mut EnrollmentChannel<S>,
) -> Result<NativePairingRequest, NativeConnectionError> {
    // `DeadlineStream` retries elapsed waits itself, so every error here is final.
    let bytes = channel.receive_envelope().map_err(frame_error)?;
    wire::decode_request(&bytes).map_err(|error| NativeConnectionFailure::Wire(error).into())
}
fn write_reply<S: Read + Write>(
    channel: &mut EnrollmentChannel<S>,
    bytes: Result<Vec<u8>, NativeWireError>,
) -> Result<(), NativeConnectionError> {
    let bytes =
        bytes.map_err(|error| NativeConnectionError::from(NativeConnectionFailure::Wire(error)))?;
    channel.send_envelope(&bytes).map_err(frame_error)
}
fn send_status<S: Read + Write>(
    gateway: &GatewayPairing,
    handle: &Handle,
    channel: &mut EnrollmentChannel<S>,
    public: PublicIntent,
) -> Result<(), NativeConnectionError> {
    let status = handle
        .block_on(gateway.status(channel.transport(), public))
        .map_err(|error| NativeConnectionError::from(NativeConnectionFailure::Runtime(error)))?;
    write_reply(channel, wire::encode_status(public, &status))
}

fn frame_error(error: NativeFrameError) -> NativeConnectionError {
    match error {
        NativeFrameError::Io(kind) => NativeConnectionFailure::Io(kind),
        NativeFrameError::Wire(error) => NativeConnectionFailure::Wire(error),
    }
    .into()
}
