//! Native Rust enrollment/recovery, with one physically retained client KSF worker.
use super::connection::wake::{NativeWakeReport, WakeEndpoint, WakeEndpoints};
use super::worker::worker_fault;
use super::{
    connection::{begin_enrollment_phase, check_deadline, DeadlineStream, NativeDeadline},
    wire::{self, NativePairingReply, NativePairingRequest, NativePairingStatus, NativeWireError},
    EnrollmentChannel, NativeConnectionError, NativeConnectionFailure, NativeFrameError,
};
use crate::app::ports::Clock;
use nessa_auth::{
    adapters::pairing::{
        ClientAttempt, CryptoRng, GatewayTrust, ManualCode, NativeIdentity, NativeTransport,
        PairingCryptoError, RngCore,
    },
    application::pairing::{ClientPendingStore, PairingWorkerFault, PrivateStateError},
    domain::pairing::{AttemptId, DisclosedConsent, PublicIntent},
};
use std::{
    io::{ErrorKind, Read, Write},
    net::TcpStream,
    sync::{Arc, Mutex, PoisonError},
};
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore};

/// Typed native client failure; no peer/library diagnostic or secret is included.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeClientError {
    /// Another physical client operation owns the single worker, or shutdown began.
    Busy,
    /// Recover the existing exact pending operation rather than create another key.
    PendingExists,
    /// This device already holds an issued credential; it does not enroll again.
    Enrolled,
    /// The original canonical receipt is still pending or already claimed.
    OriginalNotRetryable,
    /// Status requires an existing durable pending key/pin/attempt.
    NoPending,
    /// Physical private-state failure, preserved beyond PAKE's redacted callback error.
    Storage(PrivateStateError),
    /// Selected crypto/TLS operation failed.
    Crypto(PairingCryptoError),
    /// Actual bounded wire refused the peer representation.
    Wire(NativeWireError),
    /// Peer phase/operation was not the expected canonical enrollment exchange.
    Phase,
    /// The gateway answered `Refused`: no open invitation, an expired or used
    /// code, no attempts left, or a status request it would not answer. The
    /// reply is redacted, so the reason is not known here.
    Refused,
    /// Actual socket operation failed.
    Io(ErrorKind),
    /// Injected fallible entropy was unavailable before an operation was published.
    Entropy,
    /// Unexpected physical worker fault, retained internally as such.
    WorkerFault(PairingWorkerFault),
}
/// Original canonical NOT-CLAIMED receipt and resulting same-enrollment receipt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeRetryOutcome {
    /// The original attempt's outcome as the gateway reported it before the retry.
    pub original: NativePairingStatus,
    /// New attempt's historical enrollment state, not product authority.
    pub retried: NativePairingStatus,
}
/// Enrollment and pinned status. When the gateway reports Active, the issued
/// credential replaces the pending record before the status is returned. The
/// credential identifies the device's key binding; it is not read authority.
pub struct NativeEnrollmentClient {
    pending: Arc<dyn ClientPendingStore>,
    clock: Arc<dyn Clock>,
    capacity: Arc<Semaphore>,
    drained: Arc<Notify>,
    wake: Mutex<WakeEndpoints>,
}
impl NativeEnrollmentClient {
    /// Inject the retained private state owner; no filesystem path or entropy ambient seam.
    pub fn new(pending: Arc<dyn ClientPendingStore>, clock: Arc<dyn Clock>) -> Self {
        Self {
            pending,
            clock,
            capacity: Arc::new(Semaphore::new(1)),
            drained: Arc::new(Notify::new()),
            wake: Mutex::new(WakeEndpoints::new()),
        }
    }
    /// Enroll once using address+code, committing pending seed/pin/context before KE3.
    /// A lost reply remains recoverable through `status` using the exact saved identity.
    pub async fn enroll<R: RngCore + CryptoRng + Send + 'static>(
        &self,
        stream: TcpStream,
        code: ManualCode,
        entropy: R,
    ) -> Result<NativePairingStatus, NativeClientError> {
        let endpoint = client_endpoint(&stream)?;
        let (stream, deadline) = DeadlineStream::new(stream, self.clock.clone(), endpoint.clone())
            .map_err(physical_error)?;
        self.dispatch_owned(vec![endpoint], move |pending| {
            if pending
                .load_pending()
                .map_err(NativeClientError::Storage)?
                .is_some()
            {
                return Err(NativeClientError::PendingExists);
            }
            if pending
                .load_credential()
                .map_err(NativeClientError::Storage)?
                .is_some()
            {
                return Err(NativeClientError::Enrolled);
            }
            let mut entropy = entropy;
            let identity =
                NativeIdentity::generate(&mut entropy).map_err(NativeClientError::Crypto)?;
            stream.blocking().map_err(physical_error)?;
            let channel =
                NativeTransport::connect(stream, &identity, GatewayTrust::ManualBootstrap)
                    .map_err(NativeClientError::Crypto)?;
            let mut channel = EnrollmentChannel::new(channel);
            begin_enrollment_phase(&deadline).map_err(physical_error)?;
            complete_enrollment(
                &mut channel,
                &deadline,
                pending,
                &identity,
                &code,
                &mut entropy,
                None,
            )
        })
        .await
    }
    /// Retry only after fresh strict-pin status proves the exact original attempt
    /// terminal and not claimed. No externally supplied outcome flag is accepted.
    pub async fn retry<R: RngCore + CryptoRng + Send + 'static>(
        &self,
        original_stream: TcpStream,
        retry_stream: TcpStream,
        code: ManualCode,
        entropy: R,
    ) -> Result<NativeRetryOutcome, NativeClientError> {
        let original_endpoint = client_endpoint(&original_stream)?;
        let retry_endpoint = client_endpoint(&retry_stream)?;
        let endpoints = vec![original_endpoint.clone(), retry_endpoint.clone()];
        let clock = self.clock.clone();
        self.dispatch_owned(endpoints, move |pending| {
            let saved = pending
                .load_pending()
                .map_err(NativeClientError::Storage)?
                .ok_or(NativeClientError::NoPending)?;
            let (key, pin, public) = saved.into_parts();
            let identity = NativeIdentity::restore(key).map_err(NativeClientError::Crypto)?;
            let (stream, deadline) =
                DeadlineStream::new(original_stream, clock.clone(), original_endpoint)
                    .map_err(physical_error)?;
            stream.blocking().map_err(physical_error)?;
            let channel = NativeTransport::connect(stream, &identity, GatewayTrust::Pinned(pin))
                .map_err(NativeClientError::Crypto)?;
            let mut channel = EnrollmentChannel::new(channel);
            begin_enrollment_phase(&deadline).map_err(physical_error)?;
            send(&mut channel, NativePairingRequest::Status(public))?;
            let original = receive_status(&mut channel, public, None)?;
            check_deadline(&deadline).map_err(physical_error)?;
            if !matches!(original, NativePairingStatus::Unclaimed { .. }) {
                return Err(NativeClientError::OriginalNotRetryable);
            }
            drop(channel);
            // No new attempt is generated until the original terminal receipt is read.
            let (stream, deadline) =
                DeadlineStream::new(retry_stream, clock, retry_endpoint).map_err(physical_error)?;
            stream.blocking().map_err(physical_error)?;
            let channel = NativeTransport::connect(stream, &identity, GatewayTrust::Pinned(pin))
                .map_err(NativeClientError::Crypto)?;
            let mut channel = EnrollmentChannel::new(channel);
            begin_enrollment_phase(&deadline).map_err(physical_error)?;
            let mut entropy = entropy;
            let retried = complete_enrollment(
                &mut channel,
                &deadline,
                pending,
                &identity,
                &code,
                &mut entropy,
                Some(public),
            )?;
            Ok(NativeRetryOutcome { original, retried })
        })
        .await
    }
    /// Restore the exact seed/pin, from the pending record or the issued
    /// credential that replaced it, and read status on a fresh strict-key
    /// channel. A prior transient disclosure can be supplied to refuse
    /// conflicting retry scope.
    ///
    /// An Active status is saved before it is returned: the credential replaces
    /// the pending record in one publication (design row A10). If that save
    /// fails the pending record stays, and the next status delivers the same
    /// credential again. A credential other than the one already saved is
    /// refused as a storage conflict. A Terminal status for this enrollment, or
    /// an Unclaimed one (this device's attempt failed or was superseded),
    /// removes the device's record, pending or credential, before it is
    /// returned (design rows A14, A16), so the device can enroll again. Only a
    /// Pending attempt keeps it; `retry` reads the original status itself.
    pub async fn status(
        &self,
        stream: TcpStream,
        received: Option<DisclosedConsent>,
    ) -> Result<NativePairingStatus, NativeClientError> {
        let endpoint = client_endpoint(&stream)?;
        let (stream, deadline) = DeadlineStream::new(stream, self.clock.clone(), endpoint.clone())
            .map_err(physical_error)?;
        self.dispatch_owned(vec![endpoint], move |pending| {
            let saved = match pending.load_pending().map_err(NativeClientError::Storage)? {
                Some(saved) => saved,
                None => pending
                    .load_credential()
                    .map_err(NativeClientError::Storage)?
                    .ok_or(NativeClientError::NoPending)?
                    .into_enrollment(),
            };
            let (key, pin, public) = saved.into_parts();
            let identity = NativeIdentity::restore(key).map_err(NativeClientError::Crypto)?;
            stream.blocking().map_err(physical_error)?;
            let channel = NativeTransport::connect(stream, &identity, GatewayTrust::Pinned(pin))
                .map_err(NativeClientError::Crypto)?;
            let mut channel = EnrollmentChannel::new(channel);
            begin_enrollment_phase(&deadline).map_err(physical_error)?;
            send(&mut channel, NativePairingRequest::Status(public))?;
            let status = receive_status(&mut channel, public, received.as_ref())?;
            check_deadline(&deadline).map_err(physical_error)?;
            match &status {
                NativePairingStatus::Active {
                    credential,
                    receiver,
                    ..
                } => pending
                    .save_credential(credential, receiver, public)
                    .map_err(NativeClientError::Storage)?,
                // Authenticated end of this enrollment: the record goes, and
                // the returned status is the trusted end signal (row A14).
                // This device's attempt is no longer pending (failed or
                // superseded): it can never claim, so the person enters a code
                // again and enroll reserves a new attempt (row A16).
                NativePairingStatus::Terminal { .. } | NativePairingStatus::Unclaimed { .. } => {
                    pending
                        .end_enrollment(public)
                        .map_err(NativeClientError::Storage)?
                }
                NativePairingStatus::Pending(_)
                | NativePairingStatus::Claimed(_)
                | NativePairingStatus::Approved(_)
                | NativePairingStatus::Staging(_) => {}
            }
            Ok(status)
        })
        .await
    }
    async fn dispatch_owned<T: Send + 'static>(
        &self,
        endpoints: Vec<Arc<WakeEndpoint>>,
        work: impl FnOnce(&dyn ClientPendingStore) -> Result<T, NativeClientError> + Send + 'static,
    ) -> Result<T, NativeClientError> {
        let (worker, lease) = {
            // `shutdown` closes the semaphore under this lock, so a permit acquired
            // here is registered before any wake sweep.
            let mut wake = self.wake.lock().unwrap_or_else(PoisonError::into_inner);
            let permit = self
                .capacity
                .clone()
                .try_acquire_owned()
                .map_err(|_| NativeClientError::Busy)?;
            for endpoint in &endpoints {
                wake.register(endpoint);
            }
            let lease = Arc::new(ClientPermit {
                permit: Some(permit),
                drained: self.drained.clone(),
                endpoints,
            });
            let permit = lease.clone();
            let pending = self.pending.clone();
            let worker = tokio::task::spawn_blocking(move || {
                let _permit = permit;
                // The real private owner drops before capacity on success and unwind.
                let store = pending;
                work(store.as_ref())
            });
            (worker, lease)
        };
        let result = worker
            .await
            .map_err(|error| NativeClientError::WorkerFault(worker_fault(error)))?;
        drop(lease);
        result
    }
    /// The wake sweep from the first shutdown, if it has happened. A successful
    /// wake does not mean the worker has finished; `shutdown` reports that.
    pub fn wake_report(&self) -> Option<NativeWakeReport> {
        self.wake
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .report()
    }
    /// Exclude new operations and await actual KSF/IO/private-storage closure drain.
    pub async fn shutdown(&self) {
        {
            let mut wake = self.wake.lock().unwrap_or_else(PoisonError::into_inner);
            self.capacity.close();
            wake.close();
        }
        loop {
            let notified = self.drained.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.capacity.available_permits() == 1 {
                return;
            }
            notified.await;
        }
    }
}
fn client_endpoint(stream: &TcpStream) -> Result<Arc<WakeEndpoint>, NativeClientError> {
    let target = stream
        .peer_addr()
        .map_err(|error| NativeClientError::Io(error.kind()))?;
    Ok(WakeEndpoint::new(target))
}
struct ClientPermit {
    endpoints: Vec<Arc<WakeEndpoint>>,
    permit: Option<OwnedSemaphorePermit>,
    drained: Arc<Notify>,
}
impl Drop for ClientPermit {
    fn drop(&mut self) {
        self.endpoints.clear();
        drop(self.permit.take());
        self.drained.notify_waiters();
    }
}
fn complete_enrollment<R: RngCore + CryptoRng>(
    channel: &mut EnrollmentChannel<DeadlineStream>,
    deadline: &NativeDeadline,
    pending: &dyn ClientPendingStore,
    identity: &NativeIdentity,
    code: &ManualCode,
    entropy: &mut R,
    expected: Option<PublicIntent>,
) -> Result<NativePairingStatus, NativeClientError> {
    let mut attempt = [0; 16];
    entropy
        .try_fill_bytes(&mut attempt)
        .map_err(|_| NativeClientError::Entropy)?;
    send(
        channel,
        NativePairingRequest::Hello(AttemptId::new(attempt)),
    )?;
    let NativePairingReply::Hello(public) = receive(channel)? else {
        return Err(NativeClientError::Phase);
    };
    if public.attempt() != AttemptId::new(attempt) {
        return Err(NativeClientError::Phase);
    }
    if expected.is_some_and(|original: PublicIntent| {
        original.attempt() == AttemptId::new(attempt)
            || original.with_attempt(AttemptId::new(attempt)) != public
    }) {
        return Err(NativeClientError::Phase);
    }
    let (state, request) =
        ClientAttempt::start(entropy, code).map_err(NativeClientError::Crypto)?;
    send(channel, NativePairingRequest::Begin { public, request })?;
    let NativePairingReply::Challenge {
        public: received,
        response,
    } = receive(channel)?
    else {
        return Err(NativeClientError::Phase);
    };
    if received != public {
        return Err(NativeClientError::Phase);
    }
    check_deadline(deadline).map_err(physical_error)?;
    let context = channel
        .transport()
        .pairing_context(public)
        .map_err(NativeClientError::Crypto)?;
    let mut pending_error = None;
    let message = state.finish(entropy, code, &response, &context, |authenticated| {
        check_deadline(deadline).map_err(|error| {
            pending_error = Some(physical_error(error));
            PairingCryptoError::PendingStorage
        })?;
        pending
            .save_pending(
                identity.key_material(),
                &authenticated.gateway_spki(),
                public,
                expected,
            )
            .map_err(|error| {
                pending_error = Some(NativeClientError::Storage(error));
                PairingCryptoError::PendingStorage
            })
    });
    let message =
        message.map_err(|error| pending_error.unwrap_or(NativeClientError::Crypto(error)))?;
    // Saving may be slow; expiry after durable save sends no KE3 and keeps recovery state.
    check_deadline(deadline).map_err(physical_error)?;
    send(channel, NativePairingRequest::Confirm { public, message })?;
    let status = receive_status(channel, public, None)?;
    check_deadline(deadline).map_err(physical_error)?;
    Ok(status)
}

fn physical_error(error: NativeConnectionError) -> NativeClientError {
    match error.failure {
        NativeConnectionFailure::Io(kind) => NativeClientError::Io(kind),
        _ => NativeClientError::Phase,
    }
}
fn send<S: Read + Write>(
    channel: &mut EnrollmentChannel<S>,
    request: NativePairingRequest,
) -> Result<(), NativeClientError> {
    let bytes = wire::encode_request(&request).map_err(NativeClientError::Wire)?;
    // `DeadlineStream` retries elapsed waits itself, so every error here is final.
    channel.send_envelope(&bytes).map_err(frame_error)
}
fn receive<S: Read + Write>(
    channel: &mut EnrollmentChannel<S>,
) -> Result<NativePairingReply, NativeClientError> {
    let bytes = channel.receive_envelope().map_err(frame_error)?;
    match wire::decode_reply(&bytes).map_err(NativeClientError::Wire)? {
        NativePairingReply::Refused => Err(NativeClientError::Refused),
        reply => Ok(reply),
    }
}
fn receive_status<S: Read + Write>(
    channel: &mut EnrollmentChannel<S>,
    public: PublicIntent,
    received: Option<&DisclosedConsent>,
) -> Result<NativePairingStatus, NativeClientError> {
    let NativePairingReply::Status(status) = receive(channel)? else {
        return Err(NativeClientError::Phase);
    };
    status
        .correlate(public, received)
        .map_err(NativeClientError::Wire)?;
    Ok(status)
}

fn frame_error(error: NativeFrameError) -> NativeClientError {
    match error {
        NativeFrameError::Io(kind) => NativeClientError::Io(kind),
        NativeFrameError::Wire(error) => NativeClientError::Wire(error),
    }
}
