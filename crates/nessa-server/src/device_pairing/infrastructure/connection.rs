//! Bounded physical enrollment connections, including detached caller ownership.
pub(super) mod wake;
use super::worker::worker_fault;
use super::{
    protected::{ProtectedConnection, ProtectedSessions},
    wire::{self, NativePairingRequest, NativeWireError},
    BeginPairing, EnrollmentChannel, GatewayPairing, NativeFrameError, PairingRuntimeError,
};
use crate::app::ports::Clock;
use crate::device_pairing::application::DevicePairingStatus;
use nessa_auth::{
    adapters::pairing::{CryptoRng, NativeTransport, PairingCryptoError, RngCore},
    application::pairing::{PairingStoreError, PairingWorkerFault},
    domain::pairing::{AttemptFailure, PublicIntent},
};
use std::{
    io::{Error, ErrorKind, Read, Result as IoResult, Write},
    net::TcpStream,
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};
use tokio::{
    runtime::Handle,
    sync::{Notify, OwnedSemaphorePermit, Semaphore},
    task::JoinHandle,
};
use wake::{NativeWakeReport, WakeEndpoint, WakeEndpoints, NATIVE_CONNECTION_CAPACITY, WAKE_TICK};

const TLS_DEADLINE: Duration = Duration::from_secs(10);
const ENROLLMENT_DEADLINE: Duration = Duration::from_secs(30);

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

/// One fixed eight-connection physical owner; admission queues no requests.
/// Enrollment connections and protected product sessions share its permits
/// (design row PR10).
pub struct NativeEnrollmentConnections {
    gateway: Arc<GatewayPairing>,
    clock: Arc<dyn Clock>,
    capacity: Arc<Semaphore>,
    drained: Arc<Notify>,
    wake: Mutex<WakeEndpoints>,
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
            wake: Mutex::new(WakeEndpoints::new()),
            sessions: None,
        }
    }
    /// Serve a connection whose first envelope is `openProduct` with
    /// `sessions`, on the same permit (design rows PR1, PR13).
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
        let product = self.sessions.is_some();
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
    }
    /// The wake sweep from the first shutdown, if it has happened. A successful
    /// wake does not mean the worker has finished; `shutdown` reports that.
    pub fn wake_report(&self) -> Option<NativeWakeReport> {
        self.wake
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .report()
    }
    /// Exclude new sockets and await actual blocked/unwinding closure drain.
    pub async fn shutdown(&self) {
        self.close();
        loop {
            let notified = self.drained.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.capacity.available_permits() == NATIVE_CONNECTION_CAPACITY {
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
    /// The blocking worker's outcome; a connection it handed to the product
    /// is served here, still holding the original permit, until the session
    /// ends (design rows PR1, PR11).
    pub(super) async fn complete(self) -> NativeConnectionCompletion {
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
            (Ok(Served::Product(channel)), Some(sessions)) => {
                match ProtectedConnection::open(channel.into_transport()) {
                    Ok(connection) => {
                        sessions.serve(connection).await;
                        Ok(())
                    }
                    Err(error) => Err(NativeConnectionFailure::Io(error.kind()).into()),
                }
            }
            (Ok(Served::Product(_)), None) => {
                Err(NativeConnectionFailure::ProductUnavailable.into())
            }
            (Err(error), _) => Err(error),
        };
        NativeConnectionCompletion {
            outcome,
            lease: self.lease,
        }
    }
    async fn wait(self) -> Result<(), NativeConnectionError> {
        self.complete().await.into_result()
    }
}
pub(super) struct NativeConnectionCompletion {
    outcome: Result<(), NativeConnectionError>,
    lease: Arc<ConnectionPermit>,
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
pub(super) struct NativeDeadline {
    clock: Arc<dyn Clock>,
    expires_ms: Mutex<u64>,
}
impl NativeDeadline {
    fn remaining(&self) -> IoResult<Duration> {
        self.expires_ms
            .lock()
            .map_err(|_| Error::other("native deadline owner unavailable"))?
            .checked_sub(self.clock.elapsed_ms())
            .filter(|milliseconds| *milliseconds > 0)
            .map(Duration::from_millis)
            .ok_or_else(|| Error::from(ErrorKind::TimedOut))
    }
}
/// The socket calls a `DeadlineStream` makes. `TcpStream` is the real one;
/// tests substitute a recording double.
pub(super) trait NativeSocket: Read + Write {
    fn set_nonblocking(&self, nonblocking: bool) -> IoResult<()>;
    fn set_read_timeout(&self, timeout: Option<Duration>) -> IoResult<()>;
    fn set_write_timeout(&self, timeout: Option<Duration>) -> IoResult<()>;
}
impl NativeSocket for TcpStream {
    fn set_nonblocking(&self, nonblocking: bool) -> IoResult<()> {
        TcpStream::set_nonblocking(self, nonblocking)
    }
    fn set_read_timeout(&self, timeout: Option<Duration>) -> IoResult<()> {
        TcpStream::set_read_timeout(self, timeout)
    }
    fn set_write_timeout(&self, timeout: Option<Duration>) -> IoResult<()> {
        TcpStream::set_write_timeout(self, timeout)
    }
}

/// A native socket bounded by its phase deadline and by its owner's wake.
///
/// Reads cap each OS wait at `WAKE_TICK` and retry a wait that ended without
/// data, checking the wake flag and the deadline between waits, so a worker
/// blocked reading fails with ConnectionAborted within one tick on every OS
/// (`native_shutdown_keeps_original_physical_capacity`). No bytes are consumed
/// by a timed-out read, so retrying it is safe.
///
/// A send is never retried: after a timed-out send the transport may have
/// taken part of the buffer (Winsock calls the connection indeterminate), so
/// neither resending nor continuing is safe. A write checks the flag, then
/// makes one send with the rest of the phase deadline as its timeout. If it
/// times out, the stream fails for good with TimedOut and nothing touches the
/// socket again (`timed_out_send_is_terminal_and_never_retried`). A worker
/// blocked in a send is therefore not woken by the flag; it ends at its
/// deadline.
///
/// A protected product session moves the socket out ([`Self::take_socket`])
/// and the TLS state then reads and writes in-memory [`BufferedIo`], which the
/// async owner fills from and drains to that socket.
pub(super) struct DeadlineStream<S: NativeSocket = TcpStream> {
    io: NativeIo<S>,
    deadline: Arc<NativeDeadline>,
    wake: Arc<WakeEndpoint>,
    /// Set by a timed-out send; every later call fails with it.
    failed: Option<ErrorKind>,
}
enum NativeIo<S> {
    Socket(S),
    Buffered(BufferedIo),
}
/// Ciphertext between the TLS state and the async socket owner. Reads with
/// nothing buffered are `WouldBlock`, never end of stream, until the socket
/// itself ended.
#[derive(Default)]
pub(super) struct BufferedIo {
    inbound: Vec<u8>,
    inbound_read: usize,
    ended: bool,
    outbound: Vec<u8>,
    outbound_written: usize,
}
impl BufferedIo {
    /// Ciphertext the socket delivered.
    pub(super) fn receive(&mut self, bytes: &[u8]) {
        if self.inbound_read == self.inbound.len() {
            self.inbound.clear();
            self.inbound_read = 0;
        }
        self.inbound.extend_from_slice(bytes);
    }
    /// The socket reached end of stream.
    pub(super) fn end(&mut self) {
        self.ended = true;
    }
    /// Ciphertext not yet written to the socket.
    pub(super) fn unsent(&self) -> &[u8] {
        &self.outbound[self.outbound_written..]
    }
    /// Record that the socket accepted `count` bytes of [`Self::unsent`].
    pub(super) fn sent(&mut self, count: usize) {
        self.outbound_written += count;
        if self.outbound_written == self.outbound.len() {
            self.outbound.clear();
            self.outbound_written = 0;
        }
    }
    fn read(&mut self, bytes: &mut [u8]) -> IoResult<usize> {
        let available = &self.inbound[self.inbound_read..];
        if available.is_empty() {
            return if self.ended {
                Ok(0)
            } else {
                Err(Error::from(ErrorKind::WouldBlock))
            };
        }
        let count = available.len().min(bytes.len());
        bytes[..count].copy_from_slice(&available[..count]);
        self.inbound_read += count;
        Ok(count)
    }
}
impl<S: NativeSocket> DeadlineStream<S> {
    pub(super) fn new(
        stream: S,
        clock: Arc<dyn Clock>,
        wake: Arc<WakeEndpoint>,
    ) -> Result<(Self, Arc<NativeDeadline>), NativeConnectionError> {
        let expires_ms = clock
            .elapsed_ms()
            .checked_add(TLS_DEADLINE.as_millis() as u64)
            .ok_or_else(|| {
                NativeConnectionError::from(NativeConnectionFailure::Io(ErrorKind::InvalidData))
            })?;
        let deadline = Arc::new(NativeDeadline {
            clock,
            expires_ms: Mutex::new(expires_ms),
        });
        Ok((
            Self {
                io: NativeIo::Socket(stream),
                deadline: deadline.clone(),
                wake,
                failed: None,
            },
            deadline,
        ))
    }
    pub(super) fn blocking(&self) -> Result<(), NativeConnectionError> {
        match &self.io {
            NativeIo::Socket(stream) => stream
                .set_nonblocking(false)
                .map_err(|error| NativeConnectionFailure::Io(error.kind()).into()),
            NativeIo::Buffered(_) => Err(NativeConnectionFailure::Phase.into()),
        }
    }
    /// Move the socket out for an async owner; this stream then buffers.
    /// `None` if it was already taken.
    pub(super) fn take_socket(&mut self) -> Option<S> {
        match std::mem::replace(&mut self.io, NativeIo::Buffered(BufferedIo::default())) {
            NativeIo::Socket(stream) => Some(stream),
            buffered => {
                self.io = buffered;
                None
            }
        }
    }
    /// The in-memory ciphertext, once the socket was taken.
    pub(super) fn buffered(&mut self) -> Option<&mut BufferedIo> {
        match &mut self.io {
            NativeIo::Buffered(io) => Some(io),
            NativeIo::Socket(_) => None,
        }
    }
    /// The time left in the phase: refused once the stream has failed, once
    /// woken, or past the deadline.
    fn usable_for(&self) -> IoResult<Duration> {
        if let Some(kind) = self.failed {
            return Err(Error::from(kind));
        }
        if self.wake.woken() {
            return Err(Error::from(ErrorKind::ConnectionAborted));
        }
        self.deadline.remaining()
    }
}
/// An OS wait that ended without data: WouldBlock on Unix, TimedOut on Windows.
fn wait_elapsed(error: &Error) -> bool {
    matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut)
}
impl<S: NativeSocket> Read for DeadlineStream<S> {
    fn read(&mut self, bytes: &mut [u8]) -> IoResult<usize> {
        loop {
            // Buffered: the protected phase has no phase deadline; its owner
            // polls the wake flag, and refuses here once woken.
            let wait = match &mut self.io {
                NativeIo::Buffered(io) => {
                    if self.wake.woken() {
                        return Err(Error::from(ErrorKind::ConnectionAborted));
                    }
                    return io.read(bytes);
                }
                NativeIo::Socket(_) => self.usable_for()?.min(WAKE_TICK),
            };
            let NativeIo::Socket(stream) = &mut self.io else {
                unreachable!("buffered reads returned above");
            };
            stream.set_read_timeout(Some(wait))?;
            match stream.read(bytes) {
                Err(error) if wait_elapsed(&error) => continue,
                result => return result,
            }
        }
    }
}
impl<S: NativeSocket> Write for DeadlineStream<S> {
    fn write(&mut self, bytes: &[u8]) -> IoResult<usize> {
        if let NativeIo::Buffered(io) = &mut self.io {
            if self.wake.woken() {
                return Err(Error::from(ErrorKind::ConnectionAborted));
            }
            io.outbound.extend_from_slice(bytes);
            return Ok(bytes.len());
        }
        let wait = self.usable_for()?;
        let NativeIo::Socket(stream) = &mut self.io else {
            unreachable!("buffered writes returned above");
        };
        stream.set_write_timeout(Some(wait))?;
        match stream.write(bytes) {
            Err(error) if wait_elapsed(&error) => {
                self.failed = Some(ErrorKind::TimedOut);
                Err(Error::from(ErrorKind::TimedOut))
            }
            result => result,
        }
    }
    fn flush(&mut self) -> IoResult<()> {
        if let NativeIo::Buffered(_) = &self.io {
            return Ok(());
        }
        self.usable_for()?;
        let NativeIo::Socket(stream) = &mut self.io else {
            unreachable!("buffered flushes returned above");
        };
        stream.flush()
    }
}

#[cfg(test)]
#[path = "../../../tests/device_pairing/infrastructure/deadline_stream.rs"]
mod deadline_stream_tests;

/// What the blocking worker did with its connection.
pub(super) enum Served {
    /// An enrollment exchange or status, finished.
    Enrollment,
    /// The first envelope was `openProduct`: the TLS connection, for the
    /// product session.
    Product(Box<EnrollmentChannel<DeadlineStream>>),
}
fn serve_exchange<R: RngCore + CryptoRng>(
    gateway: &GatewayPairing,
    handle: &Handle,
    stream: DeadlineStream,
    deadline: Arc<NativeDeadline>,
    mut entropy: R,
    product: bool,
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
        product,
    );
    match result {
        Ok(Exchanged::ProductSelected) => Ok(Served::Product(Box::new(channel))),
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
        | NativeConnectionFailure::ProductUnavailable => true,
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
    /// The first envelope selected the product session.
    ProductSelected,
}
/// One enrollment exchange. Only a first envelope can select the product
/// session, and only when `product` says one is composed (design rows PR1,
/// PR2, PR13).
fn exchange<R: RngCore + CryptoRng>(
    gateway: &GatewayPairing,
    handle: &Handle,
    channel: &mut EnrollmentChannel<DeadlineStream>,
    deadline: &NativeDeadline,
    entropy: &mut R,
    product: bool,
) -> Result<Exchanged, NativeConnectionError> {
    begin_enrollment_phase(deadline)?;
    let first = read_request(channel)?;
    check_deadline(deadline)?;
    let public = match first {
        NativePairingRequest::OpenProduct if product => return Ok(Exchanged::ProductSelected),
        NativePairingRequest::OpenProduct => {
            return Err(NativeConnectionFailure::ProductUnavailable.into())
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
pub(super) fn begin_enrollment_phase(
    deadline: &NativeDeadline,
) -> Result<(), NativeConnectionError> {
    check_deadline(deadline)?;
    let expires_ms = deadline
        .clock
        .elapsed_ms()
        .checked_add(ENROLLMENT_DEADLINE.as_millis() as u64)
        .ok_or_else(|| {
            NativeConnectionError::from(NativeConnectionFailure::Io(ErrorKind::InvalidData))
        })?;
    *deadline.expires_ms.lock().map_err(|_| {
        NativeConnectionError::from(NativeConnectionFailure::Io(ErrorKind::Other))
    })? = expires_ms;
    Ok(())
}
pub(super) fn check_deadline(deadline: &NativeDeadline) -> Result<(), NativeConnectionError> {
    deadline
        .remaining()
        .map(|_| ())
        .map_err(|error| NativeConnectionFailure::Io(error.kind()).into())
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
