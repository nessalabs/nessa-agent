//! Bounded physical enrollment connections, including detached caller ownership.
//!
//! The socket each connection runs on, its phase deadlines and its wake, are
//! `nessa_protocol::pairing::socket`, shared with the device's enrollment client.
use super::{
    protected::{ProtectedConnection, ProtectedSessions},
    BeginPairing, GatewayPairing, PairingRuntimeError,
};
use nessa_auth::{
    adapters::pairing::{CryptoRng, NativeTransport, PairingCryptoError, RngCore},
    application::pairing::{PairingStoreError, PairingWorkerFault},
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
/// A connection whose first envelope is `openProduct` moves to a pool of its
/// own, also eight, and returns its connection permit, so product sessions
/// never take the permits enrollment and pinned status need (design row PR10).
pub struct NativeEnrollmentConnections {
    gateway: Arc<GatewayPairing>,
    clock: Arc<dyn Clock>,
    capacity: Arc<Semaphore>,
    drained: Arc<Notify>,
    wake: Arc<Mutex<WakeEndpoints>>,
    product_capacity: Arc<Semaphore>,
    product_wake: Arc<Mutex<WakeEndpoints>>,
    sessions: Option<Arc<dyn ProtectedSessions>>,
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
            product_capacity: Arc::new(Semaphore::new(PRODUCT_SESSION_CAPACITY)),
            product_wake: Arc::new(Mutex::new(WakeEndpoints::new())),
            sessions: None,
        }
    }
    /// Serve a connection whose first envelope is `openProduct` with
    /// `sessions`, on a product session permit (design rows PR1, PR10, PR13).
    pub fn with_protected_sessions(mut self, sessions: Arc<dyn ProtectedSessions>) -> Self {
        self.sessions = Some(sessions);
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
    pub(super) fn start<R: RngCore + CryptoRng + Send + 'static>(
        &self,
        stream: TcpStream,
        entropy: R,
    ) -> Result<NativeConnectionTask, NativeConnectionError> {
        let endpoint = WakeEndpoint::new(
            stream
                .peer_addr()
                .map_err(|error| NativeConnectionFailure::Io(error.kind()))?,
        );
        let (stream, deadline) = DeadlineStream::new(stream, self.clock.clone(), endpoint.clone())?;
        // `close` closes the semaphore under this lock, so a permit acquired here
        // is registered before any wake sweep, and a closed owner admits nothing.
        let mut wake = self.wake.lock().unwrap_or_else(PoisonError::into_inner);
        let permit = self
            .capacity
            .clone()
            .try_acquire_owned()
            .map_err(|_| NativeConnectionError::from(NativeConnectionFailure::Capacity))?;
        wake.register(&endpoint);
        let lease = Arc::new(ConnectionPermit {
            permit: Some(permit),
            drained: self.drained.clone(),
            endpoint: Some(endpoint),
        });
        let permit = lease.clone();
        let gateway = self.gateway.clone();
        let handle = Handle::current();
        let product = self.sessions.as_ref().map(|sessions| ProductAdmission {
            sessions: sessions.clone(),
            capacity: self.product_capacity.clone(),
            wake: self.product_wake.clone(),
            connections: self.wake.clone(),
            endpoint: endpoint_for_product(&lease),
            drained: self.drained.clone(),
        });
        let worker = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            // Locals unwind physical IO and its runtime before the original permit.
            let runtime = gateway;
            serve_exchange(&runtime, &handle, stream, deadline, entropy, product)
        });
        drop(wake);
        Ok(NativeConnectionTask {
            worker,
            lease,
            sessions: self.sessions.clone(),
        })
    }
    pub(crate) fn close(&self) {
        let mut wake = self.wake.lock().unwrap_or_else(PoisonError::into_inner);
        self.capacity.close();
        wake.close();
        drop(wake);
        let mut wake = self
            .product_wake
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        self.product_capacity.close();
        wake.close();
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
        self.product_wake
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .report()
    }
    /// Exclude new sockets and await actual blocked/unwinding closure drain,
    /// of connections and product sessions alike.
    pub async fn shutdown(&self) {
        self.close();
        loop {
            let notified = self.drained.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.capacity.available_permits() == NATIVE_CONNECTION_CAPACITY
                && self.product_capacity.available_permits() == PRODUCT_SESSION_CAPACITY
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
}
impl NativeConnectionTask {
    /// The blocking worker's outcome. A connection it handed to the product
    /// returns its connection permit and is served here on its product
    /// session permit until the session ends (design rows PR1, PR10, PR11).
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
        let outcome = match (outcome, self.sessions) {
            (Ok(Served::Enrollment), _) => Ok(()),
            (Ok(Served::Product(channel, product)), Some(sessions)) => {
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
            (Ok(Served::Product(..)), None) => {
                Err(NativeConnectionFailure::ProductUnavailable.into())
            }
            (Err(error), _) => Err(error),
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
fn endpoint_for_product(lease: &ConnectionPermit) -> Arc<WakeEndpoint> {
    lease
        .endpoint
        .clone()
        .expect("a live connection permit holds its endpoint")
}
/// Admission to the product session pool, taken by the blocking worker when
/// the first envelope is `openProduct`.
struct ProductAdmission {
    sessions: Arc<dyn ProtectedSessions>,
    capacity: Arc<Semaphore>,
    wake: Arc<Mutex<WakeEndpoints>>,
    connections: Arc<Mutex<WakeEndpoints>>,
    endpoint: Arc<WakeEndpoint>,
    drained: Arc<Notify>,
}
impl ProductAdmission {
    /// Take a product permit and move the connection's wake endpoint to the
    /// product sweep. `close` closes the pool under the same lock, so a
    /// session admitted here is swept, and a closed pool admits nothing.
    fn admit(&self) -> Result<ProductPermit, NativeConnectionError> {
        let mut wake = self.wake.lock().unwrap_or_else(PoisonError::into_inner);
        let permit =
            self.capacity.clone().try_acquire_owned().map_err(|_| {
                NativeConnectionError::from(NativeConnectionFailure::ProductCapacity)
            })?;
        wake.register(&self.endpoint);
        drop(wake);
        self.connections
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.endpoint);
        Ok(ProductPermit {
            endpoint: Some(self.endpoint.clone()),
            permit: Some(permit),
            drained: self.drained.clone(),
        })
    }
}
/// One product session's permit, held until the session ends.
pub(super) struct ProductPermit {
    endpoint: Option<Arc<WakeEndpoint>>,
    permit: Option<OwnedSemaphorePermit>,
    drained: Arc<Notify>,
}
impl Drop for ProductPermit {
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
    Product(Box<EnrollmentChannel<DeadlineStream>>, ProductPermit),
}
fn serve_exchange<R: RngCore + CryptoRng>(
    gateway: &GatewayPairing,
    handle: &Handle,
    stream: DeadlineStream,
    deadline: Arc<NativeDeadline>,
    mut entropy: R,
    product: Option<ProductAdmission>,
) -> Result<Served, NativeConnectionError> {
    stream.blocking()?;
    let channel = NativeTransport::accept(stream, gateway.identity())
        .map_err(|error| NativeConnectionError::from(NativeConnectionFailure::Crypto(error)))?;
    let mut channel = EnrollmentChannel::new(channel);
    let result = exchange(
        gateway,
        handle,
        &mut channel,
        &deadline,
        &mut entropy,
        product.as_ref(),
    );
    match result {
        Ok(Exchanged::ProductSelected(permit)) => Ok(Served::Product(Box::new(channel), permit)),
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
        | NativeConnectionFailure::ProductUnverifiable => true,
        NativeConnectionFailure::Capacity
        | NativeConnectionFailure::Crypto(_)
        | NativeConnectionFailure::Io(_)
        | NativeConnectionFailure::WorkerFault(_)
        | NativeConnectionFailure::ClaimReply(_) => false,
    }
}
/// How an exchange ended on a readable channel.
enum Exchanged {
    /// Enrollment or status answered.
    Finished,
    /// The first envelope selected the product session, admitted to its pool.
    ProductSelected(ProductPermit),
}
/// One enrollment exchange. Only a first envelope can select the product
/// session, only when `product` admission is composed, and only with a free
/// product session permit (design rows PR1, PR2, PR10, PR13).
fn exchange<R: RngCore + CryptoRng>(
    gateway: &GatewayPairing,
    handle: &Handle,
    channel: &mut EnrollmentChannel<DeadlineStream>,
    deadline: &NativeDeadline,
    entropy: &mut R,
    product: Option<&ProductAdmission>,
) -> Result<Exchanged, NativeConnectionError> {
    begin_enrollment_phase(deadline)?;
    let first = read_request(channel)?;
    check_deadline(deadline)?;
    let public = match first {
        NativePairingRequest::OpenProduct => {
            let product = product.ok_or(NativeConnectionFailure::ProductUnavailable)?;
            // Only a paired device's key may take a product permit (row PR15).
            match product.sessions.admits(channel.transport().device_proof()) {
                Ok(true) => {}
                Ok(false) => return Err(NativeConnectionFailure::ProductUnregistered.into()),
                Err(_) => return Err(NativeConnectionFailure::ProductUnverifiable.into()),
            }
            return product.admit().map(Exchanged::ProductSelected);
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
