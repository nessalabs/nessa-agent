//! Bounded physical enrollment connections, including detached caller ownership.
pub(super) mod wake;
use super::worker::worker_fault;
use super::{
    wire::{self, NativePairingRequest, NativeWireError},
    BeginPairing, EnrollmentChannel, GatewayPairing, NativeFrameError, PairingRuntimeError,
};
use crate::app::ports::Clock;
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
use wake::{NativeWakeReport, WakeEndpoint, WakeEndpoints, NATIVE_CONNECTION_CAPACITY};

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
pub struct NativeEnrollmentConnections {
    gateway: Arc<GatewayPairing>,
    clock: Arc<dyn Clock>,
    capacity: Arc<Semaphore>,
    drained: Arc<Notify>,
    wake: Mutex<WakeEndpoints>,
}
impl NativeEnrollmentConnections {
    /// Compose the actual canonical enrollment runtime before accepting sockets.
    pub fn new(gateway: Arc<GatewayPairing>, clock: Arc<dyn Clock>) -> Self {
        Self {
            gateway,
            clock,
            capacity: Arc::new(Semaphore::new(NATIVE_CONNECTION_CAPACITY)),
            drained: Arc::new(Notify::new()),
            wake: Mutex::new(WakeEndpoints::new()),
        }
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
                .try_clone()
                .map_err(|error| NativeConnectionFailure::Io(error.kind()))?,
        )
        .map_err(|error| NativeConnectionFailure::Io(error.kind()))?;
        let (stream, deadline) = DeadlineStream::new(stream, self.clock.clone())?;
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
        let worker = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            // Locals unwind physical IO and its runtime before the original permit.
            let runtime = gateway;
            let physical = PhysicalEndpoint(_permit.endpoint.clone());
            let result = serve_exchange(&runtime, &handle, stream, deadline, entropy);
            drop(physical);
            result
        });
        drop(wake);
        Ok(NativeConnectionTask { worker, lease })
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
    worker: JoinHandle<Result<(), NativeConnectionError>>,
    lease: Arc<ConnectionPermit>,
}
impl NativeConnectionTask {
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
struct PhysicalEndpoint(Option<Arc<WakeEndpoint>>);
impl Drop for PhysicalEndpoint {
    fn drop(&mut self) {
        if let Some(endpoint) = &self.0 {
            endpoint.physically_returned();
        }
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
pub(super) struct DeadlineStream {
    stream: TcpStream,
    deadline: Arc<NativeDeadline>,
}
impl DeadlineStream {
    pub(super) fn new(
        stream: TcpStream,
        clock: Arc<dyn Clock>,
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
                stream,
                deadline: deadline.clone(),
            },
            deadline,
        ))
    }
    pub(super) fn blocking(&self) -> Result<(), NativeConnectionError> {
        self.stream
            .set_nonblocking(false)
            .map_err(|error| NativeConnectionFailure::Io(error.kind()).into())
    }
    fn remaining(&self) -> IoResult<Duration> {
        self.deadline.remaining()
    }
}
impl Read for DeadlineStream {
    fn read(&mut self, bytes: &mut [u8]) -> IoResult<usize> {
        self.stream.set_read_timeout(Some(self.remaining()?))?;
        self.stream.read(bytes)
    }
}
impl Write for DeadlineStream {
    fn write(&mut self, bytes: &[u8]) -> IoResult<usize> {
        self.stream.set_write_timeout(Some(self.remaining()?))?;
        self.stream.write(bytes)
    }
    fn flush(&mut self) -> IoResult<()> {
        self.remaining()?;
        self.stream.flush()
    }
}
fn serve_exchange<R: RngCore + CryptoRng>(
    gateway: &GatewayPairing,
    handle: &Handle,
    stream: DeadlineStream,
    deadline: Arc<NativeDeadline>,
    mut entropy: R,
) -> Result<(), NativeConnectionError> {
    stream.blocking()?;
    let channel = NativeTransport::accept(stream, gateway.identity())
        .map_err(|error| NativeConnectionError::from(NativeConnectionFailure::Crypto(error)))?;
    let mut channel = EnrollmentChannel::new(channel);
    let result = exchange(gateway, handle, &mut channel, &deadline, &mut entropy);
    if let Err(error) = &result {
        if answers_with_refusal(error.failure) {
            // Best effort: the primary failure is what this connection reports,
            // whether or not the peer receives the refusal.
            write_reply(&mut channel, wire::encode_refused()).ok();
        }
    }
    result
}
/// A decision the gateway made on a readable channel gets a redacted `Refused`
/// reply; physical failures do not, because the channel may be unusable.
/// `native_expired_and_used_codes_are_refused_without_a_claim` covers the reply.
fn answers_with_refusal(failure: NativeConnectionFailure) -> bool {
    match failure {
        NativeConnectionFailure::Runtime(_)
        | NativeConnectionFailure::Phase
        | NativeConnectionFailure::Wire(_) => true,
        NativeConnectionFailure::Capacity
        | NativeConnectionFailure::Crypto(_)
        | NativeConnectionFailure::Io(_)
        | NativeConnectionFailure::WorkerFault(_) => false,
    }
}
fn exchange<R: RngCore + CryptoRng>(
    gateway: &GatewayPairing,
    handle: &Handle,
    channel: &mut EnrollmentChannel<DeadlineStream>,
    deadline: &NativeDeadline,
    entropy: &mut R,
) -> Result<(), NativeConnectionError> {
    begin_enrollment_phase(deadline)?;
    let first = read_request(channel)?;
    check_deadline(deadline)?;
    let public = match first {
        NativePairingRequest::Status(public) => {
            return send_status(gateway, channel, public);
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
        BeginPairing::Existing(_) => return send_status(gateway, channel, public),
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
                NativeConnectionFailure::Io(ErrorKind::TimedOut | ErrorKind::WouldBlock) => {
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
    handle
        .block_on(gateway.finish(channel.transport(), *handshake, &message))
        .map_err(|error| NativeConnectionError::from(NativeConnectionFailure::Runtime(error)))?;
    // After the claim commits, a failed reply is an IO failure of this connection;
    // the claim stands (`native_claim_reply_loss_preserves_pinned_status`).
    send_status(gateway, channel, public)
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
    let bytes = loop {
        match channel.receive_envelope() {
            Err(NativeFrameError::Io(ErrorKind::WouldBlock)) => std::thread::yield_now(),
            result => break result.map_err(frame_error)?,
        }
    };
    wire::decode_request(&bytes).map_err(|error| NativeConnectionFailure::Wire(error).into())
}
fn write_reply<S: Read + Write>(
    channel: &mut EnrollmentChannel<S>,
    bytes: Result<Vec<u8>, NativeWireError>,
) -> Result<(), NativeConnectionError> {
    let bytes =
        bytes.map_err(|error| NativeConnectionError::from(NativeConnectionFailure::Wire(error)))?;
    loop {
        match channel.send_envelope(&bytes) {
            Err(NativeFrameError::Io(ErrorKind::WouldBlock)) => std::thread::yield_now(),
            result => return result.map_err(frame_error),
        }
    }
}
fn send_status<S: Read + Write>(
    gateway: &GatewayPairing,
    channel: &mut EnrollmentChannel<S>,
    public: PublicIntent,
) -> Result<(), NativeConnectionError> {
    let status = gateway
        .status(channel.transport(), public)
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
