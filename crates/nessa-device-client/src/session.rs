//! One scope's native session. Physical admission covers status, binding
//! selection, read and refusal recheck; saved queries share that admission.
use crate::infrastructure::{Monotonic, Wall};
use nessa_auth::{
    adapters::pairing::{
        rand::Error as EntropyError, CryptoRng, FilePairingState, ManualCode, NativeIdentity,
        OsEntropy, RngCore,
    },
    application::{
        pairing::{ClientPendingStore, PrivateStateError},
        ports::Clock as WallClock,
    },
    domain::{
        pairing::{DeviceKey, DisclosedConsent, PublicIntent},
        CredentialId, ResourceId,
    },
};
use nessa_client_core::{
    pairing::{NativeClientError, NativeRetryOutcome},
    retained::{
        ReadFailure, ReadReport, ReadStop, RetainedCache, RetainedCataloguePage,
        RetainedDeviceReader, RetainedEnrollmentClient, RetainedEnrollmentStatus, RetainedPurge,
        RetainedTranscript, HANDSHAKE,
    },
};
use nessa_protocol::{
    clock::Clock, conversation::domain::ConversationId, pairing::wire::NativePairingStatus,
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
/// One admitted refresh's compound evidence. A status's outer error means no
/// completion was captured, not proof of no purge; durable lookup is separate.
#[derive(Debug)]
pub struct SessionRefresh {
    initial: Result<RetainedEnrollmentStatus, NativeClientError>,
    selection_failure: Option<NativeClientError>,
    read: Option<SessionRead>,
    recheck: Option<Result<RetainedEnrollmentStatus, NativeClientError>>,
}
impl SessionRefresh {
    /// Initial pinned status and its original operation purge.
    pub fn initial(&self) -> &Result<RetainedEnrollmentStatus, NativeClientError> {
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
    pub fn recheck(&self) -> Option<&Result<RetainedEnrollmentStatus, NativeClientError>> {
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
}
struct Driver {
    runtime: Runtime,
    enrollment: Arc<RetainedEnrollmentClient>,
    state: Arc<dyn ClientPendingStore>,
    connector: Arc<dyn EnrollmentConnector>,
    entropy: Arc<dyn EnrollmentEntropyFactory>,
    address: SocketAddr,
    path: PathBuf,
    wall: Arc<dyn WallClock>,
    clock: Arc<dyn Clock>,
    cache: Option<RetainedCache>,
    received: Option<DisclosedConsent>,
}
struct Dependencies {
    state: Arc<dyn ClientPendingStore>,
    connector: Arc<dyn EnrollmentConnector>,
    entropy: Arc<dyn EnrollmentEntropyFactory>,
    path: PathBuf,
    address: SocketAddr,
    wall: Arc<dyn WallClock>,
    clock: Arc<dyn Clock>,
    runtime: Runtime,
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
    fn status(&mut self) -> Result<RetainedEnrollmentStatus, NativeClientError> {
        self.synchronize_received()?;
        let stream = self.connector.connect(self.address)?;
        let status = self
            .runtime
            .block_on(self.enrollment.status(stream, self.received.clone()))?;
        if let Ok(value) = status.enrollment() {
            self.remember(value);
        }
        Ok(status)
    }
    fn refresh(&mut self, stop: &Arc<ReadStop>) -> SessionRefresh {
        let initial = self.status();
        let mut outcome = SessionRefresh {
            initial,
            selection_failure: None,
            read: None,
            recheck: None,
        };
        let status = match &outcome.initial {
            Ok(status) => match status.enrollment() {
                Ok(status) => status,
                Err(_) => return outcome,
            },
            Err(_) => return outcome,
        };
        // Load the current immutable issued record after status may have saved
        // or removed it. A pending/terminal status starts no product read.
        let reader = match RetainedDeviceReader::from_current(status, self.state.as_ref()) {
            Ok(Some(reader)) => reader,
            Ok(None) => return outcome,
            Err(error) => {
                outcome.selection_failure = Some(error);
                return outcome;
            }
        };
        let address = self.address;
        let mut withdrawn = Vec::new();
        let result = self.cache().and_then(|cache| {
            cache.read(
                reader.access(address, "nessa-ios"),
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
impl Drop for Driver {
    fn drop(&mut self) {
        self.drain();
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
        let state = Arc::new(
            FilePairingState::open(root, directory).map_err(SessionFailure::PrivateState)?,
        );
        let wall = Arc::new(Wall);
        let clock = Arc::new(Monotonic::new());
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
            connector: Arc::new(SocketConnector),
            entropy: Arc::new(OsEntropyFactory),
            runtime,
        })
    }
    fn compose(dependencies: Dependencies) -> Result<Self, SessionFailure> {
        let Dependencies {
            state,
            path,
            address,
            wall,
            clock,
            connector,
            entropy,
            runtime,
        } = dependencies;
        let enrollment = Arc::new(
            RetainedEnrollmentClient::new(state.clone(), path.clone(), wall.clone(), clock.clone())
                .map_err(SessionFailure::Cache)?,
        );
        let gate = Gate {
            closed: false,
            read: None,
            enrollment: Arc::downgrade(&enrollment),
        };
        Ok(Self {
            gate: Mutex::new(gate),
            driver: Mutex::new(Some(Driver {
                runtime,
                enrollment,
                state,
                connector,
                entropy,
                address,
                path,
                wall,
                clock,
                cache: None,
                received: None,
            })),
        })
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
        Ok(outcome)
    }
    /// Fresh pinned status retains per-operation purge and outer worker failure.
    pub fn status(
        &self,
    ) -> Result<Result<RetainedEnrollmentStatus, NativeClientError>, SessionFailure> {
        self.enter(None)
            .map(|mut operation| operation.driver().status())
    }
    /// Read current local correlation without dialing or granting read authority.
    /// `local_enrollment` tests cover absence, records, failures and held admission.
    pub fn local_enrollment(&self) -> Result<Option<LocalEnrollment>, SessionFailure> {
        self.enter(None)?.driver().local_enrollment()
    }
    /// One admitted status/binding/read/refusal-recheck. A caller can stop this
    /// handle before dispatch; no status is then asked (`stopped_before_dispatch`).
    pub fn refresh(&self, stop: Arc<ReadStop>) -> Result<SessionRefresh, SessionFailure> {
        let mut operation = self.enter(Some(stop.clone()))?;
        if stop.is_stopped() {
            return Err(SessionFailure::Stopped);
        }
        Ok(operation.driver().refresh(&stop))
    }
    /// Saved selection derives receiver from the issued record and origin/stream
    /// from actual retained scope; address metadata is not used as scope.
    pub fn catalogue_page(
        &self,
        after: Option<&EntryKey>,
        maximum: usize,
    ) -> Result<Option<RetainedCataloguePage>, SessionFailure> {
        let mut operation = self.enter(None)?;
        let driver = operation.driver();
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
    /// Project the current issued receiver's saved transcript; no transport call.
    pub fn transcript(
        &self,
        conversation: &ConversationId,
        revision: Uuid,
    ) -> Result<RetainedTranscript, SessionFailure> {
        let mut operation = self.enter(None)?;
        let driver = operation.driver();
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
        let mut gate = self.gate.lock().map_err(|_| SessionFailure::Damaged)?;
        gate.closed = true;
        if let Some(read) = &gate.read {
            read.stop();
        }
        if let Some(client) = gate.enrollment.upgrade() {
            client.request_shutdown();
        }
        Ok(())
    }
    /// Wait for admitted physical work and release this private scope. Calls
    /// after close refuse; repeated close remains closed. This blocking drain
    /// belongs on the worker, while request_close can bypass its operation queue.
    pub fn close(&self) -> Result<(), SessionFailure> {
        self.request_close()?;
        let mut driver = self.driver.lock().map_err(|_| SessionFailure::Damaged)?;
        if let Some(driver) = driver.take() {
            drop(driver);
        }
        Ok(())
    }
}
impl Drop for NativeSession {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

#[cfg(test)]
#[path = "../tests/session/local_enrollment.rs"]
mod local_enrollment_tests;
#[cfg(test)]
#[path = "../tests/session/lifecycle.rs"]
mod tests;
