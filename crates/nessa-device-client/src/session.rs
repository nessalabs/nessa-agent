//! One scope's native session. Physical admission covers status, binding
//! selection, read and refusal recheck; saved queries share that admission.
use crate::infrastructure::{Monotonic, Wall};
use nessa_auth::{
    adapters::pairing::{
        rand::Error as EntropyError, CryptoRng, FilePairingState, ManualCode, NativeIdentity,
        OsEntropy, RngCore,
    },
    application::{
        pairing::{OwnerPhoneReservation, PrivateStateError},
        ports::Clock as WallClock,
    },
    domain::{
        pairing::{DeviceKey, DisclosedConsent, PublicIntent},
        CredentialId, ResourceId,
    },
};
use nessa_client_core::{
    pairing::{
        current_owner_phone, issued_owner_phone, prepare_owner_phone, restore_expected_owner_phone,
        restore_owner_phone, NativeClientError, NativeRetryOutcome, OwnerPhonePreparation,
        OwnerPhonePreparationError, OwnerPhoneRecoveryFailure, OwnerPhoneState,
    },
    product::{
        LocalProductConnector, ProductConnector, ProductFailure, ProductLimits, ProductSession,
        ProductWake, ThreadProductWake,
    },
    retained::{
        ArtifactFault, ArtifactFaultKind, ArtifactFetch, ArtifactFreshness, ArtifactRequest,
        CataloguePresence, FeedEvent, Pause, PhoneFence, PhoneLease, PhoneStamp, ReadFailure,
        ReadReport, ReadStop, RetainedCache, RetainedCataloguePage, RetainedDeviceReader,
        RetainedEnrollmentClient, RetainedEnrollmentStatus, RetainedOwnerStatus, RetainedPurge,
        RetainedTranscript, ThreadPause, HANDSHAKE,
    },
};
use nessa_protocol::{
    clock::Clock, conversation::domain::ConversationId, pairing::wire::NativePairingStatus,
    product::generated::ConversationViewCursor,
};
use nessa_sync::replication::{catalogue::EntryKey, domain::Scope};
use std::{
    error::Error,
    fmt::{Display, Formatter, Result as FormatResult},
    io::ErrorKind,
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, TryLockError, Weak},
    time::Duration,
};
use tokio::runtime::{Builder, Runtime};
use uuid::Uuid;

/// Session admission/configuration failure, independent of a completed status's
/// purge effect. Enrollment payloads preserve their original typed meaning.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionFailure {
    /// Another physical session operation is admitted.
    Busy,
    /// Scope admission was explicitly closed.
    Closed,
    /// An unexpected failure invalidated the session's synchronization owner.
    Damaged,
    /// The local runtime could not be constructed.
    Runtime(ErrorKind),
    /// Private-state operation failed; retain publication/cleanup payloads.
    PrivateState(PrivateStateError),
    /// The cache could not supply a saved projection.
    Cache(ReadFailure),
    /// Enrollment operation failed without a completed compound status.
    Enrollment(NativeClientError),
    /// A saved query has no issued receiver to select.
    NoCredential,
    /// A refresh was stopped before asking status.
    Stopped,
    /// Original protected product opening or physical drain failure.
    Product(ProductFailure),
    /// Complete original recovery/settlement facts when owner product selection refused.
    Owner(Arc<SessionStatus>),
    /// Complete original observation plus later actual issued-selection refusal.
    OwnerSelection {
        /// Earlier independently completed status facts.
        status: Arc<SessionStatus>,
        /// Actual selected issued-record refusal.
        error: NativeClientError,
    },
    /// A publication stamp is not the receiver bound now, so no bytes are read.
    Fenced(PhoneFence),
}
impl Display for SessionFailure {
    fn fmt(&self, output: &mut Formatter<'_>) -> FormatResult {
        write!(output, "{self:?}")
    }
}
impl Error for SessionFailure {}

/// Caller-owned enrollment socket seam. The retained read continues to use the
/// existing core transport owner, including its stop and deadline policy.
pub trait EnrollmentConnector: Send + Sync {
    /// Establish a bounded native TCP connection; no authority is inferred.
    fn connect(&self, address: SocketAddr) -> Result<TcpStream, NativeClientError>;
}
struct SocketConnector;
impl EnrollmentConnector for SocketConnector {
    fn connect(&self, address: SocketAddr) -> Result<TcpStream, NativeClientError> {
        TcpStream::connect_timeout(&address, HANDSHAKE)
            .map_err(|error| NativeClientError::Io(error.kind()))
    }
}

/// Selected-library entropy input; failures remain the RNG's typed failures.
pub trait EnrollmentEntropy: RngCore + CryptoRng + Send + 'static {}
impl<T: RngCore + CryptoRng + Send + 'static> EnrollmentEntropy for T {}
/// Caller-owned source of each enrollment operation's entropy input.
pub trait EnrollmentEntropyFactory: Send + Sync {
    /// Supply one operation's RNG; acquisition occurs through its existing
    /// fallible RNG interface inside the existing physical worker.
    fn create(&self) -> Box<dyn EnrollmentEntropy>;
}
struct OsEntropyFactory;
impl EnrollmentEntropyFactory for OsEntropyFactory {
    fn create(&self) -> Box<dyn EnrollmentEntropy> {
        Box::new(OsEntropy)
    }
}

/// Whether the retained cache path is a file. Absence is not an I/O failure.
enum CacheFilePresence {
    Absent,
    Present,
}
trait CacheFiles: Send + Sync {
    fn presence(&self, path: &Path) -> Result<CacheFilePresence, std::io::Error>;
}
struct OsCacheFiles;
impl CacheFiles for OsCacheFiles {
    fn presence(&self, path: &Path) -> Result<CacheFilePresence, std::io::Error> {
        match std::fs::metadata(path) {
            Ok(meta) if meta.is_file() => Ok(CacheFilePresence::Present),
            Ok(_) => Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                "cache path is not a file",
            )),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(CacheFilePresence::Absent),
            Err(error) => Err(error),
        }
    }
}
struct Entropy(Box<dyn EnrollmentEntropy>);
impl RngCore for Entropy {
    fn next_u32(&mut self) -> u32 {
        self.0.next_u32()
    }
    fn next_u64(&mut self) -> u64 {
        self.0.next_u64()
    }
    fn fill_bytes(&mut self, bytes: &mut [u8]) {
        self.0.fill_bytes(bytes);
    }
    fn try_fill_bytes(&mut self, bytes: &mut [u8]) -> Result<(), EntropyError> {
        self.0.try_fill_bytes(bytes)
    }
}
impl CryptoRng for Entropy {}

/// Parent admission or original preparation failure; neither erases the other owner.
#[derive(Debug)]
pub enum SessionPreparationFailure {
    /// Original parent admission failure.
    Session(SessionFailure),
    /// Original canonical preparation or private publication refusal.
    Preparation(OwnerPhonePreparationError),
}
impl Display for SessionPreparationFailure {
    fn fmt(&self, output: &mut Formatter<'_>) -> FormatResult {
        write!(output, "{self:?}")
    }
}
impl Error for SessionPreparationFailure {}

/// Immutable physical cleanup fact, independent of its retained typed outcome.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionCloseObservation {
    outcome: Result<(), SessionFailure>,
    released: bool,
}
impl SessionCloseObservation {
    /// Original result from the same actual cleanup invocation.
    pub fn outcome(&self) -> &Result<(), SessionFailure> {
        &self.outcome
    }
    /// Whether this cleanup owner confirmed release of its private scope.
    pub fn released(&self) -> bool {
        self.released
    }
}

// Acquisition is lazy: invalid or restored preparation consumes no entropy source.
struct PreparationEntropy {
    factory: Arc<dyn EnrollmentEntropyFactory>,
    value: Option<Entropy>,
}
impl PreparationEntropy {
    fn value(&mut self) -> &mut Entropy {
        self.value
            .get_or_insert_with(|| Entropy(self.factory.create()))
    }
}
impl RngCore for PreparationEntropy {
    fn next_u32(&mut self) -> u32 {
        self.value().next_u32()
    }
    fn next_u64(&mut self) -> u64 {
        self.value().next_u64()
    }
    fn fill_bytes(&mut self, bytes: &mut [u8]) {
        self.value().fill_bytes(bytes);
    }
    fn try_fill_bytes(&mut self, bytes: &mut [u8]) -> Result<(), EntropyError> {
        self.value().try_fill_bytes(bytes)
    }
}
impl CryptoRng for PreparationEntropy {}

/// Read completion retains withdrawals even when the read ends with a failure.
#[derive(Clone, Debug)]
pub struct SessionRead {
    result: Result<ReadReport, ReadFailure>,
    withdrawn: Box<[String]>,
}
impl SessionRead {
    /// Original bounded read completion.
    pub fn result(&self) -> &Result<ReadReport, ReadFailure> {
        &self.result
    }
    /// Actual withdrawals reported by that read, independently of success.
    pub fn withdrawn(&self) -> &[String] {
        &self.withdrawn
    }
}
/// The original worker result, retaining remote observations and later local refusal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionStatus {
    /// Current read-only enrollment scope, with its original typed payloads.
    ReadOnly(Result<RetainedEnrollmentStatus, NativeClientError>),
    /// Original owner scope; failure never falls through to manual enrollment.
    Owner(Result<RetainedOwnerStatus, OwnerPhoneRecoveryFailure>),
}
impl SessionStatus {
    fn settled_status(&self) -> Option<&NativePairingStatus> {
        match self {
            Self::ReadOnly(value) => value.as_ref().ok()?.enrollment().as_ref().ok(),
            Self::Owner(value) => value.as_ref().ok()?.status_if_settled(),
        }
    }
    fn receipt(&self) -> Option<&NativePairingStatus> {
        match self {
            Self::ReadOnly(value) => value.as_ref().ok()?.enrollment().as_ref().ok(),
            Self::Owner(value) => Some(value.as_ref().ok()?.recovery().ok()?.status()),
        }
    }
    /// Whether this physical operation captured a completed aggregate.
    pub fn completion_captured(&self) -> bool {
        match self {
            Self::ReadOnly(value) => value.is_ok(),
            Self::Owner(value) => value.is_ok(),
        }
    }
    /// Original remote observation, independent of a subsequent local refusal.
    pub fn observation(&self) -> Option<&NativePairingStatus> {
        self.receipt()
    }
    /// This physical operation's actual purge; absence is not durable history.
    pub fn purge(&self) -> Option<&Result<RetainedPurge, ReadFailure>> {
        match self {
            Self::ReadOnly(value) => value.as_ref().ok()?.purge(),
            Self::Owner(value) => value.as_ref().ok()?.purge(),
        }
    }
}

/// One admitted refresh's compound evidence. A status's outer error means no
/// completion was captured, not proof of no purge; durable lookup is separate.
#[derive(Debug)]
pub struct SessionRefresh {
    initial: SessionStatus,
    selection_failure: Option<NativeClientError>,
    read: Option<SessionRead>,
    recheck: Option<SessionStatus>,
}
impl SessionRefresh {
    /// Initial pinned status and its original operation purge.
    pub fn initial(&self) -> &SessionStatus {
        &self.initial
    }
    /// A contradictory or unreadable current binding refused product selection.
    pub fn selection_failure(&self) -> Option<NativeClientError> {
        self.selection_failure
    }
    /// Read outcome, absent when initial status/binding did not select a reader.
    pub fn read(&self) -> Option<&SessionRead> {
        self.read.as_ref()
    }
    /// Fresh refusal recheck and its own purge; generic network failures do not recheck.
    pub fn recheck(&self) -> Option<&SessionStatus> {
        self.recheck.as_ref()
    }
}

/// The first live view from `conversation.subscribe`. It does not advance a
/// physical checkpoint; [`SessionRefresh`] still does that.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FollowedConversation {
    incarnation: String,
    position: String,
    view_json: String,
}
impl FollowedConversation {
    /// Cursor incarnation the gateway stamped on this view.
    pub fn incarnation(&self) -> &str {
        &self.incarnation
    }
    /// Cursor position the gateway stamped on this view.
    pub fn position(&self) -> &str {
        &self.position
    }
    /// The product view JSON. The caller replaces what it shows with this.
    pub fn view_json(&self) -> &str {
        &self.view_json
    }
}

/// One admitted live read. Status and binding selection match a refresh. The
/// product call is `conversation.subscribe`, and only an authority refusal
/// asks status again.
#[derive(Debug)]
pub struct SessionFollow {
    initial: SessionStatus,
    selection_failure: Option<NativeClientError>,
    followed: Option<Result<FollowedConversation, ReadFailure>>,
    recheck: Option<SessionStatus>,
}
impl SessionFollow {
    /// Initial pinned status and its original operation purge.
    pub fn initial(&self) -> &SessionStatus {
        &self.initial
    }
    /// A contradictory or unreadable current binding refused product selection.
    pub fn selection_failure(&self) -> Option<NativeClientError> {
        self.selection_failure
    }
    /// The first view, absent when initial status or binding selected no reader.
    pub fn followed(&self) -> Option<&Result<FollowedConversation, ReadFailure>> {
        self.followed.as_ref()
    }
    /// Fresh refusal recheck. A conversation this device was not granted does
    /// not ask. A view already delivered does not hide a grant revoked before
    /// the feed's next batch.
    pub fn recheck(&self) -> Option<&SessionStatus> {
        self.recheck.as_ref()
    }
}

/// Issued selectors saved together by the existing private record owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalIssuedEnrollment {
    credential: CredentialId,
    receiver: ResourceId,
}
impl LocalIssuedEnrollment {
    /// Historical issued credential identity, not a bearer secret or fresh authority.
    pub fn credential(&self) -> &CredentialId {
        &self.credential
    }
    /// Historical receiver identity associated with that credential.
    pub fn receiver(&self) -> &ResourceId {
        &self.receiver
    }
}

/// Immutable local correlation. Remote claimed/approved status must corroborate it.
/// It holds public bytes only; the restored native identity owns the zeroizing seed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalEnrollment {
    intent: PublicIntent,
    device_key: DeviceKey,
    gateway_pin: [u8; 44],
    issued: Option<LocalIssuedEnrollment>,
}
impl LocalEnrollment {
    /// Exact invitation/attempt/consent metadata from the saved record.
    pub fn intent(&self) -> PublicIntent {
        self.intent
    }
    /// Saved device's public key, derived by the Auth identity owner.
    pub fn device_key(&self) -> DeviceKey {
        self.device_key
    }
    /// Gateway SPKI authenticated by the enrollment that saved this record.
    pub fn gateway_pin(&self) -> [u8; 44] {
        self.gateway_pin
    }
    /// Issued selectors, absent for a pending record.
    pub fn issued(&self) -> Option<&LocalIssuedEnrollment> {
        self.issued.as_ref()
    }
}

struct Gate {
    closed: bool,
    read: Option<Arc<ReadStop>>,
    enrollment: Weak<RetainedEnrollmentClient>,
    product: Option<ProductSession>,
}
struct ProductDependencies {
    connector: Arc<dyn ProductConnector>,
    wake: Arc<dyn ProductWake>,
    limits: ProductLimits,
}
impl Default for ProductDependencies {
    fn default() -> Self {
        Self {
            connector: Arc::new(LocalProductConnector),
            wake: Arc::new(ThreadProductWake::default()),
            limits: ProductLimits::default(),
        }
    }
}
trait NativePrivateState: OwnerPhoneState {}
impl<T: OwnerPhoneState> NativePrivateState for T {}

struct Driver {
    runtime: Runtime,
    enrollment: Arc<RetainedEnrollmentClient>,
    state: Arc<dyn NativePrivateState>,
    connector: Arc<dyn EnrollmentConnector>,
    entropy: Arc<dyn EnrollmentEntropyFactory>,
    files: Arc<dyn CacheFiles>,
    address: SocketAddr,
    path: PathBuf,
    wall: Arc<dyn WallClock>,
    clock: Arc<dyn Clock>,
    pause: Arc<dyn Pause>,
    cache: Option<RetainedCache>,
    received: Option<DisclosedConsent>,
    product: ProductDependencies,
    lease: PhoneLease,
}
struct SelectedReader {
    reader: RetainedDeviceReader,
    address: SocketAddr,
}
struct Dependencies {
    state: Arc<dyn NativePrivateState>,
    connector: Arc<dyn EnrollmentConnector>,
    entropy: Arc<dyn EnrollmentEntropyFactory>,
    files: Arc<dyn CacheFiles>,
    path: PathBuf,
    address: SocketAddr,
    wall: Arc<dyn WallClock>,
    clock: Arc<dyn Clock>,
    pause: Arc<dyn Pause>,
    runtime: Runtime,
    product: ProductDependencies,
}
impl Driver {
    fn local_enrollment(&self) -> Result<Option<LocalEnrollment>, SessionFailure> {
        // Single-record store contract and driver admission serialize native writes;
        // these reads do not claim an atomic snapshot against out-of-band writers.
        let pending = self
            .state
            .load_pending()
            .map_err(SessionFailure::PrivateState)?;
        let issued = self
            .state
            .load_credential()
            .map_err(SessionFailure::PrivateState)?;
        let (saved, issued) = match (pending, issued) {
            (None, None) => return Ok(None),
            (Some(_), Some(_)) => {
                return Err(SessionFailure::PrivateState(PrivateStateError::Conflict))
            }
            (Some(pending), None) => (pending, None),
            (None, Some(issued)) => {
                let selectors = LocalIssuedEnrollment {
                    credential: issued.credential().clone(),
                    receiver: issued.receiver().clone(),
                };
                (issued.into_enrollment(), Some(selectors))
            }
        };
        let (key, gateway_pin, intent) = saved.into_parts();
        let identity = NativeIdentity::restore(key)
            .map_err(|error| SessionFailure::Enrollment(NativeClientError::Crypto(error)))?;
        Ok(Some(LocalEnrollment {
            intent,
            device_key: identity.device_key(),
            gateway_pin,
            issued,
        }))
    }
    fn cache(&mut self) -> Result<&mut RetainedCache, ReadFailure> {
        if self.cache.is_none() {
            self.cache = Some(RetainedCache::open(
                &self.path,
                self.wall.clone(),
                self.clock.clone(),
            )?);
        }
        Ok(self
            .cache
            .as_mut()
            .expect("cache assigned before borrowing"))
    }
    fn synchronize_received(&mut self) -> Result<(), NativeClientError> {
        let current = match self
            .state
            .load_pending()
            .map_err(NativeClientError::Storage)?
        {
            Some(pending) => Some(pending.intent()),
            None => self
                .state
                .load_credential()
                .map_err(NativeClientError::Storage)?
                .map(|issued| issued.intent()),
        };
        if self
            .received
            .as_ref()
            .is_some_and(|prior| Some(prior.public()) != current)
        {
            self.received = None;
        }
        Ok(())
    }
    fn remember(&mut self, status: &NativePairingStatus) {
        self.received = status.consent().cloned();
    }
    /// A successful purge releases the bound receiver before any observation is
    /// required. An active observation binds only when this operation did not purge.
    fn note_publication(&mut self, observation: Option<&NativePairingStatus>, purged: bool) {
        if purged {
            self.lease.release_receiver();
            return;
        }
        if let Some(NativePairingStatus::Active { receiver, .. }) = observation {
            let _ = self.lease.bind(receiver.as_str());
        }
    }
    fn note_binding(&mut self, status: &SessionStatus) {
        let purged = matches!(status.purge(), Some(Ok(_)));
        self.note_publication(status.observation(), purged);
    }
    fn select_reader(
        &self,
        initial: &SessionStatus,
    ) -> Result<Option<SelectedReader>, NativeClientError> {
        let Some(status) = initial.settled_status() else {
            return Ok(None);
        };
        if !matches!(status, NativePairingStatus::Active { .. }) {
            return Ok(None);
        }
        let (selection, address) = match initial {
            SessionStatus::ReadOnly(_) => (
                RetainedDeviceReader::from_current(status, self.state.as_ref()),
                self.address,
            ),
            SessionStatus::Owner(Ok(owner)) => {
                let recovered = owner
                    .recovery()
                    .expect("settled owner has a successful recovery");
                (
                    issued_owner_phone(self.state.as_ref(), recovered)
                        .and_then(|issued| RetainedDeviceReader::from_issued(status, issued)),
                    recovered.preparation().original().endpoint(),
                )
            }
            SessionStatus::Owner(Err(_)) => {
                unreachable!("settled owner has captured completion")
            }
        };
        match selection {
            Ok(Some(reader)) => Ok(Some(SelectedReader { reader, address })),
            Ok(None) => Ok(None),
            Err(error) => Err(error),
        }
    }
    /// The stamp may read only while it names the lease and the saved credential.
    fn accept_publication(&mut self, stamp: &PhoneStamp) -> Result<(), SessionFailure> {
        self.lease.accept(stamp).map_err(SessionFailure::Fenced)?;
        let issued = self
            .state
            .load_credential()
            .map_err(SessionFailure::PrivateState)?;
        match issued {
            Some(issued) if issued.receiver().as_str() == stamp.receiver() => Ok(()),
            Some(issued) => {
                let _ = self.lease.bind(issued.receiver().as_str());
                Err(SessionFailure::Fenced(PhoneFence::Replaced))
            }
            None => {
                self.lease.release_receiver();
                Err(SessionFailure::Fenced(PhoneFence::Unbound))
            }
        }
    }
    fn status(&mut self) -> SessionStatus {
        // Actual same-owner state decides the path; errors never become read-only fallback.
        let expected = match current_owner_phone(self.state.as_ref()) {
            Ok(Some(value)) => Some(Arc::new(value)),
            Ok(None) => None,
            Err(error) => {
                return SessionStatus::Owner(Err(OwnerPhoneRecoveryFailure::Preparation(error)))
            }
        };
        if let Some(expected) = expected {
            let value = (|| {
                self.synchronize_received()
                    .map_err(OwnerPhoneRecoveryFailure::Native)?;
                let stream = self
                    .connector
                    .connect(expected.original().endpoint())
                    .map_err(OwnerPhoneRecoveryFailure::Native)?;
                self.runtime
                    .block_on(self.enrollment.status_owner_phone(
                        stream,
                        expected,
                        self.received.clone(),
                    ))
                    .map_err(OwnerPhoneRecoveryFailure::Native)
            })();
            let status = SessionStatus::Owner(value);
            if let Some(value) = status.observation() {
                self.remember(value);
            }
            self.note_binding(&status);
            status
        } else {
            let value = (|| {
                self.synchronize_received()?;
                let stream = self.connector.connect(self.address)?;
                self.runtime
                    .block_on(self.enrollment.status(stream, self.received.clone()))
            })();
            let status = SessionStatus::ReadOnly(value);
            if let Some(value) = status.observation() {
                self.remember(value);
            }
            self.note_binding(&status);
            status
        }
    }
    fn refresh(&mut self, stop: &Arc<ReadStop>) -> SessionRefresh {
        let initial = self.status();
        let mut outcome = SessionRefresh {
            initial,
            selection_failure: None,
            read: None,
            recheck: None,
        };
        let Some(status) = outcome.initial.settled_status() else {
            return outcome;
        };
        if !matches!(status, NativePairingStatus::Active { .. }) {
            return outcome;
        }
        // Load the current immutable issued record after status may have saved
        // or removed it. A pending/terminal status starts no product read.
        let selected = match self.select_reader(&outcome.initial) {
            Ok(Some(selected)) => selected,
            Ok(None) => return outcome,
            Err(error) => {
                outcome.selection_failure = Some(error);
                return outcome;
            }
        };
        let mut withdrawn = Vec::new();
        let result = self.cache().and_then(|cache| {
            cache.read(
                selected.reader.access(selected.address, "nessa-ios"),
                stop,
                Duration::from_secs(5),
                128,
                &mut withdrawn,
            )
        });
        if result == Err(ReadFailure::Refused) && !stop.is_stopped() {
            outcome.recheck = Some(self.status());
        }
        outcome.read = Some(SessionRead {
            result,
            withdrawn: withdrawn.into_boxed_slice(),
        });
        outcome
    }
    /// Live read of one conversation through `conversation.subscribe`. The
    /// checkpoint pull stays on [`Self::refresh`].
    fn follow(
        &mut self,
        conversation: &ConversationId,
        after: Option<ConversationViewCursor>,
        stop: &Arc<ReadStop>,
    ) -> SessionFollow {
        let initial = self.status();
        let mut outcome = SessionFollow {
            initial,
            selection_failure: None,
            followed: None,
            recheck: None,
        };
        let Some(status) = outcome.initial.settled_status() else {
            return outcome;
        };
        if !matches!(status, NativePairingStatus::Active { .. }) {
            return outcome;
        }
        let selected = match self.select_reader(&outcome.initial) {
            Ok(Some(selected)) => selected,
            Ok(None) => return outcome,
            Err(error) => {
                outcome.selection_failure = Some(error);
                return outcome;
            }
        };
        let cache = match self.cache() {
            Ok(cache) => cache,
            Err(failure) => {
                outcome.followed = Some(Err(failure));
                return outcome;
            }
        };
        let (followed, authority) = match cache.subscribe(
            selected.reader.access(selected.address, "nessa-ios"),
            conversation,
            after,
            stop,
            Duration::from_secs(5),
        ) {
            Err(error) => (Err(error.failure()), error.asks_status()),
            Ok(mut feed) => take_settled(&mut feed),
        };
        if authority && !stop.is_stopped() {
            outcome.recheck = Some(self.status());
        }
        outcome.followed = Some(followed);
        outcome
    }
    fn saved_scope(&mut self) -> Result<Option<Scope>, SessionFailure> {
        let issued = self
            .state
            .load_credential()
            .map_err(SessionFailure::PrivateState)?
            .ok_or(SessionFailure::NoCredential)?;
        self.cache()
            .map_err(SessionFailure::Cache)?
            .saved_catalogue_scope(issued.receiver().as_str())
            .map_err(SessionFailure::Cache)
    }
    fn drain(&self) {
        self.runtime.block_on(self.enrollment.shutdown());
    }
}
/// A gateway fault that is this device's authority being refused. Status is
/// asked again, as it is after a refused catalogue read. A stop, a local
/// match, and a transport failure are not.
fn artifact_authority_refused(outcome: Result<ArtifactFreshness, ArtifactFault>) -> bool {
    matches!(
        outcome,
        Err(fault) if matches!(fault.kind(), ArtifactFaultKind::Gateway(ReadFailure::Refused))
    )
}

/// A status that produced no reader. A connector failure stays that failure.
/// A completed status without an active reader has nothing to fetch with.
fn absent_reader(status: &SessionStatus) -> SessionFailure {
    match status {
        SessionStatus::ReadOnly(Err(error)) => SessionFailure::Enrollment(*error),
        SessionStatus::Owner(Err(_)) => SessionFailure::Owner(Arc::new(status.clone())),
        _ => SessionFailure::NoCredential,
    }
}

/// Views the server still marks `stale` are one replay, sent immediately.
/// Keep the newest and stop when one is settled, the cursor stops advancing,
/// or the feed ends. The loop also stops after 32 views. That view may still
/// be stale: its cursor is where the next follow resumes, and `transcriptState`
/// in the view JSON says whether it had settled. A view already applied wins
/// over a later failure so the caller can resume from its cursor. An authority
/// refusal that ends the feed, including `conversation.unsubscribe`, still asks
/// status again when that view is kept.
fn take_settled(
    feed: &mut nessa_client_core::retained::ConversationFeed,
) -> (Result<FollowedConversation, ReadFailure>, bool) {
    let mut latest = None;
    let mut previous = None;
    for _ in 0..32 {
        let event = match feed.next_event() {
            Ok(event) => event,
            Err(error) => {
                let authority = error.asks_status() || close_asks_status(feed);
                return settled_or_error(latest, error.failure(), authority);
            }
        };
        if let Some(error) = event.terminal_failure() {
            let authority = error.asks_status() || close_asks_status(feed);
            return settled_or_error(latest, error.failure(), authority);
        }
        let FeedEvent::View { cursor, view } = event else {
            continue;
        };
        let stale = view.transcript_state == "stale";
        let position = cursor.position.clone();
        let stalled = previous.as_ref() == Some(&position);
        let followed = serde_json::to_string(&view).map(|view_json| FollowedConversation {
            incarnation: cursor.incarnation,
            position: cursor.position,
            view_json,
        });
        match followed {
            Ok(view) => latest = Some(view),
            Err(_) => {
                return settled_or_error(latest, ReadFailure::Protocol, close_asks_status(feed));
            }
        }
        if !stale || stalled {
            break;
        }
        previous = Some(position);
    }
    settled_or_error(latest, ReadFailure::Protocol, close_asks_status(feed))
}

/// Whether stopping the feed refused this credential. A view already applied
/// does not hide that refusal.
fn close_asks_status(feed: &mut nessa_client_core::retained::ConversationFeed) -> bool {
    feed.close().is_err_and(|error| error.asks_status())
}

/// A view already applied is the result. `authority` is whether the failure
/// that stopped the feed refused this credential, so a kept view does not
/// hide a grant revocation.
fn settled_or_error(
    latest: Option<FollowedConversation>,
    failure: ReadFailure,
    authority: bool,
) -> (Result<FollowedConversation, ReadFailure>, bool) {
    match latest {
        Some(view) => (Ok(view), authority),
        None => (Err(failure), authority),
    }
}
impl Drop for Driver {
    fn drop(&mut self) {
        self.drain();
    }
}

/// Physical product child, with no parent ownership. Its clock is read only
/// through the child.
pub struct SessionProduct {
    pub(crate) value: ProductSession,
}
impl SessionProduct {
    /// Borrow the independently owned core child; this does not retain its parent.
    pub fn session(&self) -> &ProductSession {
        &self.value
    }
}
/// One private scope and one physical session operation. Methods block and belong
/// on a dedicated native worker. Swift composition must reserve admission before
/// enqueueing, signal close before queueing drain, and await close before reuse.
/// `NativeDeviceSession` exposes this owner through the generated Swift ABI.
pub struct NativeSession {
    driver: Mutex<Option<Driver>>,
    gate: Mutex<Gate>,
}
struct Operation<'a> {
    driver: MutexGuard<'a, Option<Driver>>,
    gate: &'a Mutex<Gate>,
}
impl Operation<'_> {
    fn driver(&mut self) -> &mut Driver {
        self.driver.as_mut().expect("admitted live driver")
    }
}
impl Drop for Operation<'_> {
    fn drop(&mut self) {
        if let Ok(mut gate) = self.gate.lock() {
            gate.read = None;
        }
    }
}
impl NativeSession {
    /// Composition entry: open an existing private directory beneath a trusted
    /// root. Cache creation is lazy; no socket or pairing is performed here.
    pub fn open(
        root: &Path,
        directory: &Path,
        address: SocketAddr,
    ) -> Result<Self, SessionFailure> {
        Self::open_with_clock(root, directory, address, Arc::new(Monotonic::new()))
    }
    /// Compose a private session over its host's monotonic elapsed-time clock.
    /// The clock controls retained reads and product socket budgets; it must
    /// satisfy [`Clock`]'s monotonic contract. Auth's wall clock is independent.
    pub fn open_with_clock(
        root: &Path,
        directory: &Path,
        address: SocketAddr,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, SessionFailure> {
        let state = Arc::new(
            FilePairingState::open(root, directory).map_err(SessionFailure::PrivateState)?,
        );
        let wall = Arc::new(Wall);
        let runtime = Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| SessionFailure::Runtime(error.kind()))?;
        Self::compose(Dependencies {
            state,
            path: root.join(directory).join("retained.sqlite3"),
            address,
            wall,
            clock,
            pause: Arc::new(ThreadPause),
            connector: Arc::new(SocketConnector),
            entropy: Arc::new(OsEntropyFactory),
            files: Arc::new(OsCacheFiles),
            runtime,
            product: ProductDependencies::default(),
        })
    }
    fn compose(dependencies: Dependencies) -> Result<Self, SessionFailure> {
        let Dependencies {
            state,
            path,
            address,
            wall,
            clock,
            pause,
            connector,
            entropy,
            files,
            runtime,
            product,
        } = dependencies;
        let (instance, identity) = Self::lease_identity(entropy.as_ref())?;
        let lease = PhoneLease::new(instance, identity);
        let enrollment = Arc::new(
            RetainedEnrollmentClient::owner_phone(
                state.clone(),
                path.clone(),
                wall.clone(),
                clock.clone(),
            )
            .map_err(SessionFailure::Cache)?,
        );
        let gate = Gate {
            closed: false,
            read: None,
            enrollment: Arc::downgrade(&enrollment),
            product: None,
        };
        Ok(Self {
            gate: Mutex::new(gate),
            driver: Mutex::new(Some(Driver {
                runtime,
                enrollment,
                state,
                connector,
                entropy,
                files,
                address,
                path,
                wall,
                clock,
                pause,
                cache: None,
                received: None,
                product,
                lease,
            })),
        })
    }
    /// The published lease id and the identity `accept` compares, from one
    /// fill of the injected entropy. The identity is not returned on a stamp.
    /// A source that refuses is an enrollment entropy failure, and compose
    /// returns before the session exists.
    fn lease_identity(
        entropy: &dyn EnrollmentEntropyFactory,
    ) -> Result<(Uuid, Uuid), SessionFailure> {
        let mut source = entropy.create();
        let mut bytes = [0; 32];
        source
            .try_fill_bytes(&mut bytes)
            .map_err(|_| SessionFailure::Enrollment(NativeClientError::Entropy))?;
        let mut instance = [0; 16];
        let mut identity = [0; 16];
        instance.copy_from_slice(&bytes[..16]);
        identity.copy_from_slice(&bytes[16..]);
        Ok((Uuid::from_bytes(instance), Uuid::from_bytes(identity)))
    }
    fn enter(&self, stop: Option<Arc<ReadStop>>) -> Result<Operation<'_>, SessionFailure> {
        let driver = match self.driver.try_lock() {
            Ok(value) => value,
            Err(TryLockError::WouldBlock) => return Err(SessionFailure::Busy),
            Err(TryLockError::Poisoned(_)) => return Err(SessionFailure::Damaged),
        };
        let mut gate = self.gate.lock().map_err(|_| SessionFailure::Damaged)?;
        if gate.closed || driver.is_none() {
            return Err(SessionFailure::Closed);
        }
        gate.read = stop;
        Ok(Operation {
            driver,
            gate: &self.gate,
        })
    }
    /// Prepare only a newly allocated original under the admitted private owner.
    /// A matching Prepared replay restores its key; it never consumes new entropy.
    pub fn prepare_owner_original(
        &self,
        reservation: OwnerPhoneReservation,
    ) -> Result<OwnerPhonePreparation, SessionPreparationFailure> {
        let mut operation = self
            .enter(None)
            .map_err(SessionPreparationFailure::Session)?;
        let driver = operation.driver();
        let mut entropy = PreparationEntropy {
            factory: driver.entropy.clone(),
            value: None,
        };
        prepare_owner_phone(driver.state.as_ref(), reservation, &mut entropy)
            .map_err(SessionPreparationFailure::Preparation)
    }
    /// Restore an existing original; missing or incomplete private history refuses.
    /// This never falls back to initial preparation or creates another key.
    pub fn restore_owner_original(
        &self,
        reservation: &OwnerPhoneReservation,
    ) -> Result<OwnerPhonePreparation, SessionPreparationFailure> {
        let mut operation = self
            .enter(None)
            .map_err(SessionPreparationFailure::Session)?;
        restore_owner_phone(operation.driver().state.as_ref(), reservation)
            .map_err(SessionPreparationFailure::Preparation)
    }
    /// Recover the original over its pinned native channel without local adoption.
    /// Parent admission and the retained worker own physical completion independently
    /// of the observer; outer faults leave effects unconfirmed, not absent.
    pub fn recover_owner_original(
        &self,
        expected: Arc<OwnerPhonePreparation>,
    ) -> Result<Result<RetainedOwnerStatus, OwnerPhoneRecoveryFailure>, SessionFailure> {
        let mut operation = self.enter(None)?;
        let driver = operation.driver();
        let result = (|| {
            restore_expected_owner_phone(driver.state.as_ref(), &expected)
                .map_err(OwnerPhoneRecoveryFailure::Preparation)?;
            let stream = driver
                .connector
                .connect(expected.original().endpoint())
                .map_err(OwnerPhoneRecoveryFailure::Native)?;
            driver
                .runtime
                .block_on(driver.enrollment.recover_owner_phone(
                    stream,
                    expected,
                    driver.received.clone(),
                ))
                .map_err(OwnerPhoneRecoveryFailure::Native)
        })();
        if let Ok(value) = &result {
            if let Ok(recovered) = value.recovery() {
                driver.remember(recovered.status());
            }
        }
        Ok(result)
    }

    /// Use the existing physical enrollment worker; no replacement key on failure.
    pub fn enroll(&self, code: ManualCode) -> Result<NativePairingStatus, SessionFailure> {
        let mut operation = self.enter(None)?;
        let driver = operation.driver();
        driver
            .synchronize_received()
            .map_err(SessionFailure::Enrollment)?;
        let stream = driver
            .connector
            .connect(driver.address)
            .map_err(SessionFailure::Enrollment)?;
        let status = driver
            .runtime
            .block_on(
                driver
                    .enrollment
                    .enroll(stream, code, Entropy(driver.entropy.create())),
            )
            .map_err(SessionFailure::Enrollment)?;
        driver.remember(&status);
        driver.note_publication(Some(&status), false);
        Ok(status)
    }
    /// Existing exact-operation retry owner decides eligibility before a new attempt.
    pub fn retry(&self, code: ManualCode) -> Result<NativeRetryOutcome, SessionFailure> {
        let mut operation = self.enter(None)?;
        let driver = operation.driver();
        driver
            .synchronize_received()
            .map_err(SessionFailure::Enrollment)?;
        let original = driver
            .connector
            .connect(driver.address)
            .map_err(SessionFailure::Enrollment)?;
        let retry = driver
            .connector
            .connect(driver.address)
            .map_err(SessionFailure::Enrollment)?;
        let outcome = driver
            .runtime
            .block_on(driver.enrollment.retry(
                original,
                retry,
                code,
                Entropy(driver.entropy.create()),
            ))
            .map_err(SessionFailure::Enrollment)?;
        driver.remember(outcome.retried());
        driver.note_publication(Some(outcome.retried()), false);
        Ok(outcome)
    }
    /// Fresh pinned status retains per-operation purge and outer worker failure.
    pub fn status(&self) -> Result<SessionStatus, SessionFailure> {
        self.enter(None)
            .map(|mut operation| operation.driver().status())
    }
    /// Read current local correlation without dialing or granting read authority.
    /// `local_enrollment` tests cover absence, records, failures and held admission.
    pub fn local_enrollment(&self) -> Result<Option<LocalEnrollment>, SessionFailure> {
        self.enter(None)?.driver().local_enrollment()
    }
    /// One admitted status/binding/checkpoint read/refusal-recheck. Live
    /// conversation activity is [`Self::follow`]. A caller can stop this
    /// handle before dispatch; no status is then asked (`stopped_before_dispatch`).
    pub fn refresh(&self, stop: Arc<ReadStop>) -> Result<SessionRefresh, SessionFailure> {
        let mut operation = self.enter(Some(stop.clone()))?;
        if stop.is_stopped() {
            return Err(SessionFailure::Stopped);
        }
        Ok(operation.driver().refresh(&stop))
    }
    /// One admitted live read of `conversation` through `conversation.subscribe`.
    /// `after` is the last cursor this caller applied. Views the gateway still
    /// marks stale are read until one is settled or the cursor stops advancing,
    /// then the subscription is closed. A physical checkpoint is not advanced.
    pub fn follow(
        &self,
        conversation: &ConversationId,
        after: Option<ConversationViewCursor>,
        stop: Arc<ReadStop>,
    ) -> Result<SessionFollow, SessionFailure> {
        let mut operation = self.enter(Some(stop.clone()))?;
        if stop.is_stopped() {
            return Err(SessionFailure::Stopped);
        }
        Ok(operation.driver().follow(conversation, after, &stop))
    }
    /// Install the original protected product child before its worker can dial.
    /// The saved issued key/pin/credential supply proof; actual gateway Ready
    /// supplies capabilities. Waits occur on the child outside driver admission.
    pub fn open_product(&self, instance: String) -> Result<SessionProduct, SessionFailure> {
        let mut operation = self.enter(None)?;
        let previous = self
            .gate
            .lock()
            .map_err(|_| SessionFailure::Damaged)?
            .product
            .clone();
        if let Some(previous) = previous {
            // Keep the old child registered until its physical join. Its original
            // failure remains on escaped historical handles, independent of this opening.
            let _ = previous.close();
        }
        let driver = operation.driver();
        let owner = current_owner_phone(driver.state.as_ref());
        let mut owner_status = None;
        let product_address = match owner {
            Ok(None) => driver.address,
            Ok(Some(expected)) => {
                // Product opening needs an actual issued selector before its fresh
                // recovery; initial adoption belongs to the explicit status flow.
                if driver
                    .state
                    .load_credential()
                    .map_err(SessionFailure::PrivateState)?
                    .is_none()
                {
                    return Err(SessionFailure::NoCredential);
                }
                let address = expected.original().endpoint();
                let status = driver.status();
                if !matches!(
                    status.settled_status(),
                    Some(NativePairingStatus::Active { .. })
                ) {
                    return Err(SessionFailure::Owner(Arc::new(status)));
                }
                owner_status = Some(status);
                address
            }
            Err(OwnerPhonePreparationError::Private(error)) => {
                // No remote observation exists at this preflight boundary; keep
                // the existing original physical private-state failure family.
                return Err(SessionFailure::PrivateState(error));
            }
            Err(error) => {
                return Err(SessionFailure::Owner(Arc::new(SessionStatus::Owner(Err(
                    OwnerPhoneRecoveryFailure::Preparation(error),
                )))))
            }
        };
        let issued = if let Some(SessionStatus::Owner(Ok(owner))) = &owner_status {
            let recovered = owner
                .recovery()
                .expect("successful owner status has recovery");
            issued_owner_phone(driver.state.as_ref(), recovered).map_err(|error| {
                // Preserve the completed owner facts together with this later selection refusal.
                SessionFailure::OwnerSelection {
                    status: Arc::new(owner_status.as_ref().unwrap().clone()),
                    error,
                }
            })?
        } else {
            let pending = driver
                .state
                .load_pending()
                .map_err(SessionFailure::PrivateState)?;
            let issued = driver
                .state
                .load_credential()
                .map_err(SessionFailure::PrivateState)?;
            match (pending, issued) {
                (Some(_), Some(_)) => {
                    return Err(SessionFailure::PrivateState(PrivateStateError::Conflict))
                }
                (_, None) => return Err(SessionFailure::NoCredential),
                (None, Some(issued)) => issued,
            }
        };
        let credential = issued.credential().as_str().to_owned();
        let (key, pin, _) = issued.into_enrollment().into_parts();
        let identity = NativeIdentity::restore(key)
            .map_err(|error| SessionFailure::Enrollment(NativeClientError::Crypto(error)))?;
        let child = ProductSession::prepare(
            product_address,
            Arc::new(identity),
            pin,
            credential,
            instance,
            driver.product.connector.clone(),
            driver.clock.clone(),
            driver.product.wake.clone(),
            driver.product.limits,
        );
        {
            let mut gate = self.gate.lock().map_err(|_| SessionFailure::Damaged)?;
            if gate.closed {
                return Err(SessionFailure::Closed);
            }
            gate.product = Some(child.clone());
        }
        child.start().map_err(SessionFailure::Product)?;
        Ok(SessionProduct { value: child })
    }
    /// Bind the lease to the saved issued receiver without dialing.
    /// The same receiver keeps its stamp. No credential leaves the lease unbound.
    /// A receiver with a successful purge receipt is not bound again, even when
    /// that credential is still stored. A cache path that cannot be stated is
    /// a cache failure and does not mint a stamp.
    pub fn bind_saved_receiver(&self) -> Result<PhoneStamp, SessionFailure> {
        let mut operation = self.enter(None)?;
        let driver = operation.driver();
        let issued = driver
            .state
            .load_credential()
            .map_err(SessionFailure::PrivateState)?
            .ok_or(SessionFailure::NoCredential)?;
        let receiver = issued.receiver().as_str();
        let present = match driver
            .files
            .presence(&driver.path)
            .map_err(|_| SessionFailure::Cache(ReadFailure::Cache))?
        {
            CacheFilePresence::Absent => false,
            CacheFilePresence::Present => true,
        };
        if present {
            let purged = driver
                .cache()
                .map_err(SessionFailure::Cache)?
                .purge_receipt(receiver)
                .map_err(SessionFailure::Cache)?;
            if purged.is_some() {
                if driver
                    .lease
                    .stamp()
                    .ok()
                    .is_some_and(|stamp| stamp.receiver() == receiver)
                {
                    driver.lease.release_receiver();
                }
                return Err(SessionFailure::Fenced(PhoneFence::Purged));
            }
        }
        driver.lease.bind(receiver).map_err(SessionFailure::Fenced)
    }
    /// The stamp currently admitted, if a receiver is bound.
    pub fn publication(&self) -> Result<PhoneStamp, SessionFailure> {
        let mut operation = self.enter(None)?;
        operation
            .driver()
            .lease
            .stamp()
            .map_err(SessionFailure::Fenced)
    }
    /// Saved transcript for `stamp`'s receiver. A replaced stamp reads nothing.
    pub fn published_transcript(
        &self,
        stamp: &PhoneStamp,
        conversation: &ConversationId,
        revision: Uuid,
    ) -> Result<RetainedTranscript, SessionFailure> {
        let mut operation = self.enter(None)?;
        let driver = operation.driver();
        driver.accept_publication(stamp)?;
        let Some(scope) = driver.saved_scope()? else {
            return Ok(RetainedTranscript::NotLoaded);
        };
        driver
            .cache()
            .map_err(SessionFailure::Cache)?
            .transcript_view(
                scope.receiver().as_str(),
                scope.origin().as_str(),
                conversation,
                revision,
            )
            .map_err(SessionFailure::Cache)
    }
    /// Saved catalogue page for `stamp`'s receiver. A replaced stamp reads nothing.
    pub fn published_catalogue_page(
        &self,
        stamp: &PhoneStamp,
        after: Option<&EntryKey>,
        maximum: usize,
    ) -> Result<Option<RetainedCataloguePage>, SessionFailure> {
        let mut operation = self.enter(None)?;
        let driver = operation.driver();
        driver.accept_publication(stamp)?;
        let Some(scope) = driver.saved_scope()? else {
            return Ok(None);
        };
        driver
            .cache()
            .map_err(SessionFailure::Cache)?
            .catalogue_page(
                scope.receiver().as_str(),
                scope.origin().as_str(),
                scope.stream().as_str(),
                after,
                maximum,
            )
            .map_err(SessionFailure::Cache)
    }
    /// Stored presence of one conversation. Absence is not a tombstone.
    /// A replaced stamp reads nothing.
    pub fn catalogue_presence(
        &self,
        stamp: &PhoneStamp,
        conversation: &str,
    ) -> Result<CataloguePresence, SessionFailure> {
        let mut operation = self.enter(None)?;
        let driver = operation.driver();
        driver.accept_publication(stamp)?;
        driver
            .cache()
            .map_err(SessionFailure::Cache)?
            .catalogue_presence(stamp.receiver(), conversation)
            .map_err(SessionFailure::Cache)
    }
    /// Fetch one held file for `stamp`'s receiver. A replaced stamp does not dial.
    /// A file already at the request's output is a local match and does not dial either.
    /// A stop already set, or set while that file is read, is `Stopped` and not a match.
    /// Elapsed time is this call on the session clock, including the status read
    /// when the file is not already local. A gateway authority refusal asks
    /// status again, as a refused catalogue read does. That includes a product
    /// handshake that refuses, and a refusal that settled before a later stop.
    /// A stop the fetch itself observed does not ask.
    pub fn fetch_artifact(
        &self,
        stamp: &PhoneStamp,
        request: &ArtifactRequest<'_>,
        stop: Arc<ReadStop>,
    ) -> Result<ArtifactFetch, SessionFailure> {
        let mut operation = self.enter(Some(stop.clone()))?;
        if stop.is_stopped() {
            return Err(SessionFailure::Stopped);
        }
        let driver = operation.driver();
        driver.accept_publication(stamp)?;
        let started = driver.clock.elapsed_ms();
        let inspected = nessa_client_core::retained::local_held_outcome(&stop, || {
            nessa_client_core::retained::local_held_file(request)
        });
        match inspected {
            Err(ReadFailure::Stopped) => return Err(SessionFailure::Stopped),
            Err(error) => return Err(SessionFailure::Cache(error)),
            Ok(Some(outcome)) => {
                let elapsed = driver.clock.elapsed_ms().saturating_sub(started);
                return Ok(ArtifactFetch::decided(outcome, elapsed));
            }
            Ok(None) => {}
        }
        let status = driver.status();
        driver.accept_publication(stamp)?;
        let selected = match driver.select_reader(&status) {
            Ok(Some(selected)) => selected,
            Ok(None) => return Err(absent_reader(&status)),
            Err(error) => return Err(SessionFailure::Enrollment(error)),
        };
        if selected.reader.receiver() != stamp.receiver() {
            return Err(SessionFailure::Fenced(PhoneFence::Replaced));
        }
        let pause = Arc::clone(&driver.pause);
        let fetched = match driver
            .cache()
            .map_err(SessionFailure::Cache)?
            .fetch_artifact(
                selected.reader.access(selected.address, "nessa-ios"),
                &stop,
                Duration::from_secs(5),
                request,
                pause.as_ref(),
            ) {
            Ok(fetched) => fetched,
            // The handshake refused before a fetch existed. Ask status again
            // so the revocation can purge, then return that refusal.
            Err(ReadFailure::Refused) => {
                let _ = driver.status();
                return Err(SessionFailure::Cache(ReadFailure::Refused));
            }
            Err(error) => return Err(SessionFailure::Cache(error)),
        };
        let outcome = fetched.outcome();
        // A range refusal that already settled, including one a later stop
        // did not cause. Ask status again so the revocation can purge.
        if artifact_authority_refused(outcome) {
            let _ = driver.status();
        }
        let elapsed = driver.clock.elapsed_ms().saturating_sub(started);
        Ok(ArtifactFetch::decided(outcome, elapsed))
    }
    /// Historical lookup remains available after key removal. This receiver
    /// selects original purge evidence, not read or command authority.
    pub fn purge_receipt(&self, receiver: &str) -> Result<Option<RetainedPurge>, SessionFailure> {
        self.enter(None)?
            .driver()
            .cache()
            .map_err(SessionFailure::Cache)?
            .purge_receipt(receiver)
            .map_err(SessionFailure::Cache)
    }
    /// Immediately exclude future admission and wake current enrollment/read
    /// sockets. An OS connect can still wait up to the core HANDSHAKE bound.
    pub fn request_close(&self) -> Result<(), SessionFailure> {
        let (read, enrollment, product, damaged) = {
            let (mut gate, damaged) = match self.gate.lock() {
                Ok(gate) => (gate, false),
                Err(error) => (error.into_inner(), true),
            };
            gate.closed = true;
            (
                gate.read.clone(),
                gate.enrollment.upgrade(),
                gate.product.clone(),
                damaged,
            )
        };
        if let Some(read) = read {
            read.stop();
        }
        if let Some(client) = enrollment {
            client.request_shutdown();
        }
        if let Some(product) = product {
            product.request_close();
        }
        if damaged {
            Err(SessionFailure::Damaged)
        } else {
            Ok(())
        }
    }
    /// Wait for admitted physical work and release this private scope. Calls
    /// after close refuse; repeated close remains closed. This blocking drain
    /// belongs on the worker, while request_close can bypass its operation queue.
    pub fn close(&self) -> Result<(), SessionFailure> {
        self.close_observed().outcome
    }
    /// Join actual owned work and capture release separately from historical failure.
    /// An unknown earlier unwind with a poisoned empty slot proves no release.
    pub fn close_observed(&self) -> SessionCloseObservation {
        let requested = self.request_close();
        let (mut driver, damaged_driver) = match self.driver.lock() {
            Ok(driver) => (driver, false),
            Err(error) => (error.into_inner(), true),
        };
        // A damaged already-empty slot cannot distinguish prior successful
        // destruction from a prior unwind after take(). Current nonempty
        // destruction can establish the fact in this invocation, without a flag.
        let original_driver_present = driver.is_some();
        let (product, damaged_gate) = match self.gate.lock() {
            Ok(gate) => (gate.product.clone(), false),
            Err(error) => (error.into_inner().product.clone(), true),
        };
        let child_result = product.map_or(Ok(()), |child| {
            child.close().map_err(SessionFailure::Product)
        });
        if let Some(mut value) = driver.take() {
            value.lease.close();
            drop(value);
        }
        // This line is unreachable if physical join or destruction unwinds.
        // Hold the same driver guard through the immutable receipt capture.
        let released = driver.is_none() && (!damaged_driver || original_driver_present);
        let outcome = if damaged_driver || damaged_gate {
            Err(SessionFailure::Damaged)
        } else {
            requested.and(child_result)
        };
        SessionCloseObservation { outcome, released }
    }
}
impl Drop for NativeSession {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

#[cfg(test)]
#[path = "../tests/session/follow_close.rs"]
mod follow_close;
#[cfg(test)]
#[path = "../tests/session/local_enrollment.rs"]
mod local_enrollment_tests;
#[cfg(test)]
#[path = "../tests/session/lifecycle.rs"]
mod tests;

#[cfg(test)]
#[path = "../tests/product/parent.rs"]
pub(crate) mod product_tests;

#[cfg(test)]
#[path = "../tests/session/owner_original.rs"]
mod owner_original_tests;

#[cfg(test)]
#[path = "../tests/session/owner_recovery.rs"]
mod owner_recovery_tests;

#[cfg(test)]
#[path = "../tests/session/phone.rs"]
mod phone_tests;

#[cfg(test)]
use nessa_auth::application::pairing::{ClientPendingStore, OwnerPhoneOriginalStore};
