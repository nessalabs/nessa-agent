//! One volatile setup slot around canonical invitation and authentication owners.
use super::worker::worker_fault;
use super::{RegistrationError, RegistrationWorker};
use crate::device_pairing::application::{
    DevicePairingStatus, OwnerError, PairingOwner, ReadDevicePairing,
};
use nessa_auth::{
    adapters::pairing::{
        credential_request_fingerprint, CryptoRng, ManualCode, NativeIdentity, NativeTransport,
        PairingCryptoError, RngCore, ServerAttempt, ServerInvitation,
    },
    application::{
        pairing::{
            AttemptReservation, AuthorizePairing, GatewayKeyStore, OwnerDecision, PairingStore,
            PairingStoreError, PairingWorkerFault, RuntimeEnd,
        },
        ports::{AccessReader, Clock, PolicyEvaluator},
        session::AuthenticatedSession,
    },
    domain::{
        pairing::{
            AttemptFailure, AttemptId, ConsentIntentId, InvitationId, PairingError, PairingPhase,
            PairingPolicy, PairingRecord, PublicIntent,
        },
        Resource,
    },
};
use std::{
    io::{Read, Write},
    sync::Arc,
};
use tokio::{
    runtime::Handle,
    sync::{Mutex, Notify, OwnedSemaphorePermit, Semaphore},
};
/// Runtime refusals preserve canonical owner and unexpected physical worker facts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PairingRuntimeError {
    /// Current owner command is refused.
    Owner(OwnerError),
    /// Physical registration refused or faulted before authority publication.
    Registration(RegistrationError),
    /// Canonical enrollment read/transition refused.
    Enrollment(PairingStoreError),
    /// Public context or selected crypto operation refused before admission.
    Crypto(PairingCryptoError),
    /// A charged handshake failed; settlement failure leaves its original Pending receipt.
    Handshake {
        /// Typed primary cryptographic failure.
        failure: PairingCryptoError,
        /// Independent failure to persist the actual attempt's outcome.
        settlement: Option<PairingStoreError>,
    },
    /// The original owned create worker ended unexpectedly.
    WorkerFault(PairingWorkerFault),
    /// Another local create owns preparation/publication; no queue is retained.
    Busy,
    /// Fallible entropy for new public identities is unavailable.
    Entropy,
    /// No canonical Available slot with owned volatile setup exists.
    NoInvitation,
}
/// Trusted composition inputs; native key is restored by its private storage owner.
pub struct PairingRuntimeDependencies {
    /// Enrollment store (the credential registry in real composition). Every
    /// enrollment decision is Auth's, made through this port.
    pub enrollments: Arc<dyn PairingStore>,
    /// Current credential and membership reader for owner admission.
    pub access: Arc<dyn AccessReader>,
    /// Existing current policy evaluator (Cedar in real composition).
    pub policy: Arc<dyn PolicyEvaluator>,
    /// Existing injected absolute clock.
    pub clock: Arc<dyn Clock>,
    /// Composition-resolved exact gateway read resource.
    pub gateway: Resource,
    /// The gateway's private key store. It is not read here: holding it keeps
    /// its process lock for as long as the runtime or any of its workers lives,
    /// so nothing else can open the gateway's private state meanwhile
    /// (`native_create_observer_loss_keeps_original_owner_until_drain`).
    pub key_store: Arc<dyn GatewayKeyStore>,
    /// Durable gateway Ed25519 identity restored before this owner opens.
    pub identity: NativeIdentity,
}
/// One-time protected local result; code is neither cloned nor serializable.
pub struct CreatedInvitation {
    record: PairingRecord,
    code: ManualCode,
}
impl CreatedInvitation {
    /// Canonical invitation acknowledged by the registry commit.
    pub fn record(&self) -> &PairingRecord {
        &self.record
    }
    /// Borrow code only for the authorized local display sink. Sink copies have
    /// their own erasure contract; diagnostics must use ManualCode's redaction.
    pub fn code(&self) -> &ManualCode {
        &self.code
    }
}
/// Original physical server handshake or an already admitted status receipt.
pub enum BeginPairing {
    /// One original ServerLogin owns this exact attempt/context.
    Admitted(Box<ServerHandshake>),
    /// Identical retry reads status without another crypto dispatch or charge.
    Existing(DevicePairingStatus),
}
/// Bounded non-serializable original PAKE state; response is borrowed for output.
pub struct ServerHandshake {
    public: PublicIntent,
    state: ServerAttempt,
    response: Vec<u8>,
}
impl ServerHandshake {
    /// Borrow selected-library KE2 bytes; the native frame owner bounds output.
    pub fn response(&self) -> &[u8] {
        &self.response
    }
    /// Metadata of the attempt this handshake was admitted for.
    pub fn public(&self) -> PublicIntent {
        self.public
    }
}
struct AvailableSetup {
    id: InvitationId,
    setup: Arc<ServerInvitation>,
}
/// Native enrollment runtime. Canonical records own lifecycle; this slot only
/// owns volatile secret memory. Exclusive creation does not retain queued requests.
pub struct GatewayPairing {
    dependencies: Arc<PairingRuntimeDependencies>,
    registration: RegistrationWorker,
    creating: Arc<Semaphore>,
    create_drained: Arc<Notify>,
    available: Arc<Mutex<Option<AvailableSetup>>>,
}
impl GatewayPairing {
    /// Settle this gateway's unfinished records before serving. An Available
    /// record lost its volatile PAKE setup with the previous process, so it ends
    /// Restarted. Every other unfinished record is offered to Auth's expiry, so
    /// one past its deadline ends Expired. Composition calls this before
    /// accepting native or owner requests.
    pub fn open(dependencies: PairingRuntimeDependencies) -> Result<Self, PairingRuntimeError> {
        let enrollments = dependencies.enrollments.as_ref();
        let clock = dependencies.clock.as_ref();
        for record in enrollments
            .pending_pairings()
            .map_err(PairingRuntimeError::Enrollment)?
        {
            if record.intent().resource() != &dependencies.gateway {
                continue;
            }
            if record.phase() == PairingPhase::Available {
                enrollments.end_pairing(record.id(), RuntimeEnd::Restarted, clock)
            } else {
                enrollments.expire_pairing_if_due(record.id(), clock)
            }
            .map_err(PairingRuntimeError::Enrollment)?;
        }
        Ok(Self {
            dependencies: Arc::new(dependencies),
            registration: RegistrationWorker::new(),
            creating: Arc::new(Semaphore::new(1)),
            create_drained: Arc::new(Notify::new()),
            available: Arc::new(Mutex::new(None)),
        })
    }
    fn owner_from(dependencies: &PairingRuntimeDependencies) -> PairingOwner<'_> {
        PairingOwner {
            authorization: AuthorizePairing {
                access: dependencies.access.as_ref(),
                policy: dependencies.policy.as_ref(),
                clock: dependencies.clock.as_ref(),
            },
            enrollments: dependencies.enrollments.as_ref(),
            gateway: &dependencies.gateway,
            policy: PairingPolicy::initial(),
            clock: dependencies.clock.as_ref(),
        }
    }
    async fn owner_command<T: Send + 'static>(
        &self,
        command: impl FnOnce(&PairingOwner<'_>, &Handle) -> Result<T, OwnerError> + Send + 'static,
    ) -> Result<T, PairingRuntimeError> {
        let dependencies = self.dependencies.clone();
        let handle = Handle::current();
        tokio::task::spawn_blocking(move || command(&Self::owner_from(&dependencies), &handle))
            .await
            .map_err(|error| PairingRuntimeError::WorkerFault(worker_fault(error)))?
            .map_err(PairingRuntimeError::Owner)
    }
    /// Read the permanent gateway key only for the native TLS composition owner.
    pub fn identity(&self) -> &NativeIdentity {
        &self.dependencies.identity
    }
    /// Discover bounded unfinished enrollments after a lost owner response.
    /// Each result asks the same exact-intent owner used by `owner_status`;
    /// this surface exposes no stored code or broad credential history.
    pub async fn pending(
        &self,
        session: &AuthenticatedSession,
    ) -> Result<Box<[PairingRecord]>, PairingRuntimeError> {
        self.expire_due().await?;
        let session = session.clone();
        self.owner_command(move |owner, handle| handle.block_on(owner.pending(&session)))
            .await
    }
    /// Create one code after current preparation and successful physical registration.
    /// Observer loss retains this admitted physical closure through registry publication.
    /// The slot lock is held across the whole registry create, then this same
    /// poll installs setup before exposing the local result. No await follows commit.
    pub async fn create<R: RngCore + CryptoRng + Send + 'static>(
        self: &Arc<Self>,
        session: AuthenticatedSession,
        entropy: R,
    ) -> Result<CreatedInvitation, PairingRuntimeError> {
        let permit = self
            .creating
            .clone()
            .try_acquire_owned()
            .map_err(|_| PairingRuntimeError::Busy)?;
        let lease = CreatePermit {
            permit: Some(permit),
            drained: self.create_drained.clone(),
        };
        let owner = self.clone();
        let handle = Handle::current();
        tokio::task::spawn_blocking(move || {
            let _lease = lease;
            // Reverse local drop order releases the runtime/private owner before capacity.
            let runtime = owner;
            handle.block_on(runtime.create_owned(&session, entropy))
        })
        .await
        .map_err(|error| PairingRuntimeError::WorkerFault(worker_fault(error)))?
    }
    async fn create_owned<R: RngCore + CryptoRng + Send + 'static>(
        &self,
        session: &AuthenticatedSession,
        mut entropy: R,
    ) -> Result<CreatedInvitation, PairingRuntimeError> {
        let mut bytes = [0; 32];
        entropy
            .try_fill_bytes(&mut bytes)
            .map_err(|_| PairingRuntimeError::Entropy)?;
        let mut invite = [0; 16];
        invite.copy_from_slice(&bytes[..16]);
        let mut consent = [0; 16];
        consent.copy_from_slice(&bytes[16..]);
        // A past-due invitation would otherwise hold the slot and the live limit.
        self.expire_due().await?;
        let session = session.clone();
        let prepared = self
            .owner_command(move |owner, handle| {
                handle.block_on(owner.prepare(
                    &session,
                    InvitationId::new(invite),
                    ConsentIntentId::new(consent),
                ))
            })
            .await?;
        let registered = self
            .registration
            .register(entropy, prepared.record().id(), &self.dependencies.identity)
            .await
            .map_err(PairingRuntimeError::Registration)?;
        let available = self.available.clone();
        self.owner_command(move |owner, handle| {
            let mut slot = available.blocking_lock();
            let record = handle.block_on(owner.create(prepared))?;
            *slot = Some(AvailableSetup {
                id: record.id(),
                setup: Arc::new(registered.setup),
            });
            Ok(CreatedInvitation {
                record,
                code: registered.code,
            })
        })
        .await
    }
    /// Disclose the open invitation's public metadata: no owner identity, code,
    /// other invitations or grant selectors. Auth decides expiry here, through
    /// `expire_pairing_if_due`, and again when `begin` charges the attempt.
    pub async fn hello(&self, attempt: AttemptId) -> Result<PublicIntent, PairingRuntimeError> {
        let id = self
            .available
            .lock()
            .await
            .as_ref()
            .map(|setup| setup.id)
            .ok_or(PairingRuntimeError::NoInvitation)?;
        let record = self.expire(id).await?;
        if record.phase() != PairingPhase::Available {
            return Err(PairingRuntimeError::NoInvitation);
        }
        PublicIntent::from_record(&record, attempt)
            .map_err(|error| PairingRuntimeError::Enrollment(PairingStoreError::Domain(error)))
    }
    /// Charge before ServerLogin, binding proof and exporter from this actual
    /// channel. Public metadata is compared with its canonical record before charge.
    pub async fn begin<S: Read + Write>(
        &self,
        channel: &NativeTransport<S>,
        public: PublicIntent,
        request: &[u8],
        entropy: &mut (impl RngCore + CryptoRng),
    ) -> Result<BeginPairing, PairingRuntimeError> {
        self.verify_channel(channel)?;
        // Expiry is decided by `reserve_attempt` below; a refusal for expiry is
        // settled in `refused_by_store`.
        let record = self
            .dependencies
            .enrollments
            .read_pairing(public.invitation())
            .map_err(PairingRuntimeError::Enrollment)?;
        if PublicIntent::from_record(&record, public.attempt())
            .map_err(|error| PairingRuntimeError::Enrollment(PairingStoreError::Domain(error)))?
            != public
        {
            return Err(PairingRuntimeError::Crypto(
                PairingCryptoError::InvalidContext,
            ));
        }
        let input = credential_request_fingerprint(request).map_err(PairingRuntimeError::Crypto)?;
        let reservation = match self.dependencies.enrollments.reserve_attempt(
            public.invitation(),
            public.attempt(),
            channel.device_proof(),
            input,
            self.dependencies.clock.as_ref(),
        ) {
            Ok(reservation) => reservation,
            Err(error) => return Err(self.refused_by_store(public.invitation(), error).await),
        };
        if matches!(reservation, AttemptReservation::Existing(_)) {
            return self
                .status(channel, public)
                .await
                .map(BeginPairing::Existing);
        }
        let slot = self.available.lock().await;
        let setup = slot
            .as_ref()
            .filter(|slot| slot.id == public.invitation())
            .map(|slot| slot.setup.clone());
        drop(slot);
        let result = setup
            .ok_or(PairingCryptoError::Unavailable)
            .and_then(|setup| setup.start(entropy, request, channel.pairing_context(public)?));
        match result {
            Ok((state, response)) => Ok(BeginPairing::Admitted(Box::new(ServerHandshake {
                public,
                state,
                response,
            }))),
            Err(failure) => Err(self.settle_crypto_failure(channel, public, failure)),
        }
    }
    /// Validate the original KE3 then commit claim before secret erasure/receipt.
    pub async fn finish<S: Read + Write>(
        &self,
        channel: &NativeTransport<S>,
        handshake: ServerHandshake,
        message: &[u8],
    ) -> Result<PairingRecord, PairingRuntimeError> {
        self.verify_channel(channel)?;
        let current = channel
            .pairing_context(handshake.public)
            .map_err(PairingRuntimeError::Crypto)?;
        if handshake.state.context().as_bytes() != current.as_bytes() {
            return Err(PairingRuntimeError::Crypto(
                PairingCryptoError::InvalidContext,
            ));
        }
        let confirmed = handshake
            .state
            .finish(message)
            .map_err(|failure| self.settle_crypto_failure(channel, handshake.public, failure))?;
        let record = match self
            .dependencies
            .enrollments
            .confirm_claim(confirmed.proof(), self.dependencies.clock.as_ref())
        {
            Ok(record) => record,
            Err(error) => {
                return Err(self
                    .refused_by_store(handshake.public.invitation(), error)
                    .await)
            }
        };
        // The commit's own record decides the discard: nothing that can fail
        // runs between a committed claim and its reply.
        self.discard_if_ended(&record).await;
        Ok(record)
    }
    /// Same-key exact-attempt receipt; Pending does not cancel its original worker.
    pub async fn status<S: Read + Write>(
        &self,
        channel: &NativeTransport<S>,
        public: PublicIntent,
    ) -> Result<DevicePairingStatus, PairingRuntimeError> {
        self.verify_channel(channel)?;
        let record = self.expire(public.invitation()).await?;
        if PublicIntent::from_record(&record, public.attempt())
            .map_err(|error| PairingRuntimeError::Enrollment(PairingStoreError::Domain(error)))?
            != public
        {
            return Err(PairingRuntimeError::Crypto(
                PairingCryptoError::InvalidContext,
            ));
        }
        ReadDevicePairing {
            enrollments: self.dependencies.enrollments.as_ref(),
        }
        .execute(
            public.invitation(),
            public.attempt(),
            channel.device_proof(),
        )
        .map_err(PairingRuntimeError::Enrollment)
    }
    /// Current protected owner status; historical Active is not read authority.
    pub async fn owner_status(
        &self,
        session: &AuthenticatedSession,
        id: InvitationId,
    ) -> Result<PairingRecord, PairingRuntimeError> {
        self.expire(id).await?;
        let session = session.clone();
        self.owner_command(move |owner, handle| handle.block_on(owner.status(&session, id)))
            .await
    }
    /// Commit explicit owner consent/termination and erase only the matching slot.
    /// Physical cleanup remains a canonical obligation for its separate coordinator.
    pub async fn decide(
        &self,
        session: &AuthenticatedSession,
        id: InvitationId,
        decision: OwnerDecision,
    ) -> Result<PairingRecord, PairingRuntimeError> {
        // Expiry first, so a past-due enrollment keeps Expired as its cause.
        self.expire(id).await?;
        let session = session.clone();
        let record = self
            .owner_command(move |owner, handle| {
                handle.block_on(owner.decide(&session, id, decision))
            })
            .await?;
        self.discard_if_ended(&record).await;
        Ok(record)
    }
    fn verify_channel<S: Read + Write>(
        &self,
        channel: &NativeTransport<S>,
    ) -> Result<(), PairingRuntimeError> {
        if channel.gateway_spki() != self.dependencies.identity.public_spki() {
            return Err(PairingRuntimeError::Crypto(
                PairingCryptoError::InvalidContext,
            ));
        }
        Ok(())
    }
    /// Settle this connection's own admitted attempt after its exchange failed
    /// before KE3. Auth refuses to fail an attempt that is no longer Pending, so a
    /// committed claim stays claimed.
    pub(crate) fn end_attempt<S: Read + Write>(
        &self,
        channel: &NativeTransport<S>,
        public: PublicIntent,
        cause: AttemptFailure,
    ) -> Result<(), PairingStoreError> {
        self.dependencies
            .enrollments
            .fail_attempt(
                public.invitation(),
                public.attempt(),
                channel.device_proof(),
                cause,
                self.dependencies.clock.as_ref(),
            )
            .map(|_| ())
    }
    fn settle_crypto_failure<S: Read + Write>(
        &self,
        channel: &NativeTransport<S>,
        public: PublicIntent,
        failure: PairingCryptoError,
    ) -> PairingRuntimeError {
        let cause = if failure == PairingCryptoError::InvalidProof {
            AttemptFailure::InvalidProof
        } else {
            AttemptFailure::VerifierUnavailable
        };
        let settlement = self
            .dependencies
            .enrollments
            .fail_attempt(
                public.invitation(),
                public.attempt(),
                channel.device_proof(),
                cause,
                self.dependencies.clock.as_ref(),
            )
            .err();
        PairingRuntimeError::Handshake {
            failure,
            settlement,
        }
    }
    /// Ask Auth to expire this record if it is due, then drop the volatile setup
    /// if the record has ended. Returns the record Auth returned.
    async fn expire(&self, id: InvitationId) -> Result<PairingRecord, PairingRuntimeError> {
        let record = self
            .dependencies
            .enrollments
            .expire_pairing_if_due(id, self.dependencies.clock.as_ref())
            .map_err(PairingRuntimeError::Enrollment)?;
        self.discard_if_ended(&record).await;
        Ok(record)
    }
    /// `expire` for every unfinished record of this gateway.
    async fn expire_due(&self) -> Result<(), PairingRuntimeError> {
        for record in self
            .dependencies
            .enrollments
            .pending_pairings()
            .map_err(PairingRuntimeError::Enrollment)?
        {
            if record.intent().resource() == &self.dependencies.gateway {
                self.expire(record.id()).await?;
            }
        }
        Ok(())
    }
    /// A store refusal of a device step. When Auth refused because the invitation
    /// is past due, settle the expiry so neither the record nor the setup stays
    /// open. The original refusal is returned either way.
    async fn refused_by_store(
        &self,
        id: InvitationId,
        error: PairingStoreError,
    ) -> PairingRuntimeError {
        if error == PairingStoreError::Domain(PairingError::Expired) {
            // A failed settlement leaves the record for the next path that reads it.
            self.expire(id).await.ok();
        }
        PairingRuntimeError::Enrollment(error)
    }
    async fn discard_if_ended(&self, record: &PairingRecord) {
        if record.phase() != PairingPhase::Available {
            let mut slot = self.available.lock().await;
            if slot.as_ref().is_some_and(|slot| slot.id == record.id()) {
                *slot = None;
            }
        }
    }
    /// Exclude physical registration and wait for its actual worker to drain.
    pub async fn shutdown(&self) {
        self.creating.close();
        self.registration.shutdown().await;
        loop {
            let notified = self.create_drained.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.creating.available_permits() == 1 {
                return;
            }
            notified.await;
        }
    }
}

struct CreatePermit {
    permit: Option<OwnedSemaphorePermit>,
    drained: Arc<Notify>,
}
impl Drop for CreatePermit {
    fn drop(&mut self) {
        drop(self.permit.take());
        self.drained.notify_waiters();
    }
}
