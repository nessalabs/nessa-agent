//! One volatile setup slot around canonical invitation and authentication owners.
use super::worker::worker_fault;
use super::{RegistrationError, RegistrationWorker};
use crate::device_pairing::application::{
    Approval, CleanupError, DevicePairingStatus, DeviceStatusError, FreshStage, OwnerError,
    PairingOwner, PairingReceivers, ReadDevicePairing, ReceiverError, SettleCleanup,
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
            AttemptFailure, AttemptId, ConsentIntentId, DeviceKey, InvitationId, PairingPhase,
            PairingPolicy, PairingRecord, PublicIntent,
        },
        CredentialId, Resource,
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
    /// The receiver authority refused or failed while reading an Active
    /// enrollment's current receiver.
    Receiver(ReceiverError),
    /// An ended enrollment's receiver cleanup did not complete; the record
    /// keeps the obligation and its first cause.
    Cleanup(CleanupError),
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
    /// The canonical receiver authority an approved enrollment is paired with
    /// and an ended one is fenced at.
    pub receivers: Arc<dyn PairingReceivers>,
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
    /// Counts admitted owner commands, so shutdown can close admission and
    /// wait for them, approval's receiver work included (design row D5). It
    /// bounds nothing: the product socket bounds its own requests.
    owner_work: Arc<Semaphore>,
    owner_drained: Arc<Notify>,
    available: Arc<Mutex<Option<AvailableSetup>>>,
}
impl GatewayPairing {
    /// Settle this gateway's unfinished records before serving. Every one is
    /// offered to Auth's expiry first, so a past-due record keeps Expired as its
    /// first cause. A record still Available afterwards lost its volatile PAKE
    /// setup with the previous process, so it ends Restarted. Composition calls
    /// this before accepting native or owner requests.
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
            let record = enrollments
                .expire_pairing_if_due(record.id(), clock)
                .map_err(PairingRuntimeError::Enrollment)?;
            if record.phase() == PairingPhase::Available {
                enrollments
                    .end_pairing(record.id(), RuntimeEnd::Restarted, clock)
                    .map_err(PairingRuntimeError::Enrollment)?;
            }
        }
        Ok(Self {
            dependencies: Arc::new(dependencies),
            registration: RegistrationWorker::new(),
            creating: Arc::new(Semaphore::new(1)),
            create_drained: Arc::new(Notify::new()),
            owner_work: Arc::new(Semaphore::new(Semaphore::MAX_PERMITS)),
            owner_drained: Arc::new(Notify::new()),
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
            enrollments: &dependencies.enrollments,
            receivers: dependencies.receivers.as_ref(),
            gateway: &dependencies.gateway,
            policy: PairingPolicy::initial(),
            clock: dependencies.clock.as_ref(),
        }
    }
    async fn owner_command<T: Send + 'static>(
        &self,
        command: impl FnOnce(&PairingOwner<'_>, &Handle) -> Result<T, OwnerError> + Send + 'static,
    ) -> Result<T, PairingRuntimeError> {
        let lease = OwnerPermit {
            permit: Some(
                self.owner_work
                    .clone()
                    .try_acquire_owned()
                    .map_err(|_| PairingRuntimeError::Busy)?,
            ),
            drained: self.owner_drained.clone(),
        };
        let dependencies = self.dependencies.clone();
        let available = self.available.clone();
        let handle = Handle::current();
        // Owner commands, including any expiry they settle, do store work, so
        // they run on a blocking worker
        // (`native_owner_store_work_runs_off_the_async_thread`). The lease
        // moves into the worker: a caller that goes away does not end it.
        tokio::task::spawn_blocking(move || {
            let _lease = lease;
            run_owner(&dependencies, &available, &handle, command)
        })
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
            runtime.create_blocking(&handle, &session, entropy)
        })
        .await
        .map_err(|error| PairingRuntimeError::WorkerFault(worker_fault(error)))?
    }
    /// Runs on create's one blocking job. Every stage runs inline in it, never
    /// as a second blocking job, so a one-thread blocking pool cannot deadlock
    /// (`native_gateway_runs_on_a_one_thread_blocking_pool`).
    fn create_blocking<R: RngCore + CryptoRng>(
        &self,
        handle: &Handle,
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
        let prepared = run_owner(
            &self.dependencies,
            &self.available,
            handle,
            |owner, handle| {
                handle.block_on(owner.prepare(
                    session,
                    InvitationId::new(invite),
                    ConsentIntentId::new(consent),
                ))
            },
        )
        .map_err(PairingRuntimeError::Owner)?;
        let registered = self
            .registration
            .register(entropy, prepared.record().id(), &self.dependencies.identity)
            .map_err(PairingRuntimeError::Registration)?;
        run_owner(
            &self.dependencies,
            &self.available,
            handle,
            |owner, handle| {
                let mut slot = self.available.blocking_lock();
                let record = handle.block_on(owner.create(prepared))?;
                *slot = Some(AvailableSetup {
                    id: record.id(),
                    setup: Arc::new(registered.setup),
                });
                Ok(CreatedInvitation {
                    record,
                    code: registered.code,
                })
            },
        )
        .map_err(PairingRuntimeError::Owner)
    }
    /// Disclose the open invitation's public metadata: no owner identity, code,
    /// other invitations or grant selectors. Auth decides expiry here, through
    /// `expire_pairing_if_due`; `begin` and `finish` settle expiry after Auth
    /// refuses them.
    ///
    /// Blocking: it makes synchronous store calls, including an expiry write when the invitation is
    /// due. Drive it from a blocking worker, as
    /// `NativeEnrollmentConnections` does with `Handle::block_on` inside
    /// `spawn_blocking`, never directly on an async worker thread. The device
    /// steps are blocking because their TLS transport is a synchronous
    /// `Read + Write` stream; they are async only to share the setup lock.
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
    ///
    /// Blocking: it makes synchronous store calls (the reservation is a durable write) and runs the
    /// CPU-bound OPAQUE ServerLogin. Drive it from a blocking worker, as
    /// `NativeEnrollmentConnections` does with `Handle::block_on` inside
    /// `spawn_blocking`, never directly on an async worker thread. The device
    /// steps are blocking because their TLS transport is a synchronous
    /// `Read + Write` stream; they are async only to share the setup lock.
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
    ///
    /// Blocking: it runs OPAQUE's KE3 check and makes synchronous store writes (the claim, or the
    /// settlement of a failed attempt). Drive it from a blocking worker, as
    /// `NativeEnrollmentConnections` does with `Handle::block_on` inside
    /// `spawn_blocking`, never directly on an async worker thread. The device
    /// steps are blocking because their TLS transport is a synchronous
    /// `Read + Write` stream; they are async only to share the setup lock.
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
    ///
    /// Blocking: it makes synchronous store calls, including an expiry write when the record is
    /// due, and reads the store while holding the setup lock. Drive it from a blocking worker, as
    /// `NativeEnrollmentConnections` does with `Handle::block_on` inside
    /// `spawn_blocking`, never directly on an async worker thread. The device
    /// steps are blocking because their TLS transport is a synchronous
    /// `Read + Write` stream; they are async only to share the setup lock.
    pub async fn status<S: Read + Write>(
        &self,
        channel: &NativeTransport<S>,
        public: PublicIntent,
    ) -> Result<DevicePairingStatus, PairingRuntimeError> {
        self.verify_channel(channel)?;
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
        let status = ReadDevicePairing {
            enrollments: self.dependencies.enrollments.as_ref(),
            receivers: self.dependencies.receivers.as_ref(),
            clock: self.dependencies.clock.as_ref(),
        }
        .execute(
            public.invitation(),
            public.attempt(),
            channel.device_proof(),
        )
        .map_err(|error| match error {
            DeviceStatusError::Enrollment(error) => PairingRuntimeError::Enrollment(error),
            DeviceStatusError::Receiver(error) => PairingRuntimeError::Receiver(error),
        })?;
        // The read may have expired the open invitation; drop its setup.
        discard_ended(&mut *self.available.lock().await, &self.dependencies);
        Ok(status)
    }
    /// Current protected owner status; historical Active is not read authority.
    pub async fn owner_status(
        &self,
        session: &AuthenticatedSession,
        id: InvitationId,
    ) -> Result<PairingRecord, PairingRuntimeError> {
        let session = session.clone();
        self.owner_command(move |owner, handle| handle.block_on(owner.status(&session, id)))
            .await
    }
    /// Approve the exact claimed key and carry the enrollment through to an
    /// issued credential: stage, receiver, publication (design rows P19–P25,
    /// O5, O6). The server mints the stage's credential and correlation from
    /// `entropy`; a retry keeps the stage the record already has. A step that
    /// does not complete leaves the record where it stands, and the record is
    /// what is returned: approving again continues from there.
    pub async fn approve<R: RngCore + CryptoRng + Send + 'static>(
        &self,
        session: &AuthenticatedSession,
        id: InvitationId,
        key: DeviceKey,
        mut entropy: R,
    ) -> Result<PairingRecord, PairingRuntimeError> {
        let mut bytes = [0; 32];
        entropy
            .try_fill_bytes(&mut bytes)
            .map_err(|_| PairingRuntimeError::Entropy)?;
        let credential = CredentialId::new(format!("device-{}", hex(&bytes[..16])))
            .map_err(|_| PairingRuntimeError::Entropy)?;
        let mut request = [0; 16];
        request.copy_from_slice(&bytes[16..]);
        let fresh = FreshStage {
            credential,
            request: AttemptId::new(request),
        };
        let session = session.clone();
        let Approval { record, stopped } = self
            .owner_command(move |owner, handle| {
                handle.block_on(owner.approve(&session, id, key, fresh))
            })
            .await?;
        if let Some(stopped) = stopped {
            tracing::warn!(
                ?stopped,
                phase = ?record.phase(),
                "device pairing activation stopped; approving again continues it"
            );
        }
        Ok(record)
    }
    /// Record an owner decision. `Approve` here records consent only; `approve`
    /// carries it on to Active. An enrollment that ends after it was staged has its
    /// receiver settled at once when nothing else holds its stage; otherwise
    /// the record keeps `cleanup_pending` for the stage's holder or the next
    /// reconciliation, and is returned that way (design row P23).
    pub async fn decide(
        &self,
        session: &AuthenticatedSession,
        id: InvitationId,
        decision: OwnerDecision,
    ) -> Result<PairingRecord, PairingRuntimeError> {
        let session = session.clone();
        self.owner_command(move |owner, handle| {
            let record = handle.block_on(owner.decide(&session, id, decision))?;
            if !record.cleanup_pending() {
                return Ok(record);
            }
            let settled = handle.block_on(
                SettleCleanup {
                    enrollments: owner.enrollments,
                    receivers: owner.receivers,
                    clock: owner.clock,
                }
                .execute(id),
            );
            match settled {
                Ok(record) => Ok(record),
                Err(error) => {
                    tracing::warn!(?error, "device pairing cleanup left pending");
                    owner
                        .enrollments
                        .read_pairing(id)
                        .map_err(OwnerError::Enrollment)
                }
            }
        })
        .await
    }
    /// Settle every ended enrollment of this gateway whose receiver cleanup is
    /// pending: lookup only, then fence or no-receiver completion (design rows
    /// S7, D5). Composition runs it before the native bind and after the
    /// native and owner drains. Each record is tried; the first failure is
    /// returned and every unsettled record keeps its obligation (rows S8, D6).
    pub async fn reconcile_cleanup(&self) -> Result<(), PairingRuntimeError> {
        let dependencies = self.dependencies.clone();
        let handle = Handle::current();
        tokio::task::spawn_blocking(move || {
            let cleanup = SettleCleanup {
                enrollments: &dependencies.enrollments,
                receivers: dependencies.receivers.as_ref(),
                clock: dependencies.clock.as_ref(),
            };
            let mut first = None;
            for record in dependencies
                .enrollments
                .pending_pairings()
                .map_err(PairingRuntimeError::Enrollment)?
            {
                if record.intent().resource() != &dependencies.gateway || !record.cleanup_pending()
                {
                    continue;
                }
                if let Err(error) = handle.block_on(cleanup.execute(record.id())) {
                    first.get_or_insert(PairingRuntimeError::Cleanup(error));
                }
            }
            first.map_or(Ok(()), Err)
        })
        .await
        .map_err(|error| PairingRuntimeError::WorkerFault(worker_fault(error)))?
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
    /// A store refusal of a device reservation or claim. After any Auth domain
    /// refusal, ask Auth's expiry, which changes nothing unless the record is
    /// due, so a past-due record does not stay open whichever rule refused
    /// first. A store failure settles nothing. The original refusal is returned
    /// either way.
    async fn refused_by_store(
        &self,
        id: InvitationId,
        error: PairingStoreError,
    ) -> PairingRuntimeError {
        if matches!(error, PairingStoreError::Domain(_)) {
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
    /// Exclude physical registration and owner commands, and wait for their
    /// actual workers to drain, an approval's receiver work included.
    pub async fn shutdown(&self) {
        self.creating.close();
        self.owner_work.close();
        self.registration.shutdown().await;
        drain(&self.creating, &self.create_drained, 1).await;
        drain(
            &self.owner_work,
            &self.owner_drained,
            Semaphore::MAX_PERMITS,
        )
        .await;
    }
}

/// Wait until every permit of a closed `capacity` has come back.
async fn drain(capacity: &Semaphore, drained: &Notify, permits: usize) {
    loop {
        let notified = drained.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if capacity.available_permits() == permits {
            return;
        }
        notified.await;
    }
}

/// One admitted owner command, held by its worker until it returns.
struct OwnerPermit {
    permit: Option<OwnedSemaphorePermit>,
    drained: Arc<Notify>,
}
impl Drop for OwnerPermit {
    fn drop(&mut self) {
        drop(self.permit.take());
        self.drained.notify_waiters();
    }
}

/// Lower-case hex, for identities the server mints from random bytes.
pub(super) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
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
/// Drop the open invitation's setup if its stored record has ended. A failed
/// read leaves the setup for the next path that reads the record.
fn discard_ended(slot: &mut Option<AvailableSetup>, dependencies: &PairingRuntimeDependencies) {
    let ended = slot.as_ref().is_some_and(|setup| {
        dependencies
            .enrollments
            .read_pairing(setup.id)
            .is_ok_and(|record| record.phase() != PairingPhase::Available)
    });
    if ended {
        *slot = None;
    }
}
/// One owner command on the current blocking thread, then drop the open
/// invitation's setup if the command ended it. Callers are already on a
/// blocking worker; this starts no other.
fn run_owner<T>(
    dependencies: &PairingRuntimeDependencies,
    available: &Mutex<Option<AvailableSetup>>,
    handle: &Handle,
    command: impl FnOnce(&PairingOwner<'_>, &Handle) -> Result<T, OwnerError>,
) -> Result<T, OwnerError> {
    let result = command(&GatewayPairing::owner_from(dependencies), handle);
    discard_ended(&mut available.blocking_lock(), dependencies);
    result
}
