//! What a cut needs from outside the process, and from the admission lock.

use std::path::Path;

use async_trait::async_trait;

use super::super::domain::{DeletionFence, DeletionLog, Fingerprint, SourceCut, StoreKind};

/// Take the conversation admission lock for the cut, and release it on drop.
#[async_trait]
pub trait MutationAdmission: Send + Sync {
    /// Join admitted mutations. [`AdmissionFailure::Retired`] means the
    /// gateway is retiring and the cut must not start.
    async fn hold(&self) -> Result<Box<dyn AdmissionLease + Send + Sync>, AdmissionFailure>;
}

/// The held admission. Dropping it releases the lock.
///
/// `Sync` so the task that checks it before and after the pin can move
/// between threads. The check itself stays on the task that holds it.
pub trait AdmissionLease: Send + Sync {
    /// Whether retirement was signaled while this hold is still in force.
    fn retirement_signaled(&self) -> bool;
}

/// Why the hold was not taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdmissionFailure {
    /// The gateway is retiring. The cut aborts.
    Retired,
}

/// One writer-inventory store.
#[async_trait]
pub trait ExportSource: Send + Sync {
    fn kind(&self) -> StoreKind;

    /// While admission is held: record the watermark and keep a snapshot.
    ///
    /// A database store opens a SQLite read transaction. A small file store
    /// copies its bytes. `Err` means the store could not be pinned at all.
    /// Reading every record waits until [`PinnedStore::read`], after
    /// admission has been released.
    async fn pin(&self) -> Result<Box<dyn PinnedStore>, ()>;
}

/// A store frozen at the cut. Reading it does not take admission.
#[async_trait]
pub trait PinnedStore: Send {
    /// Records from the snapshot [`ExportSource::pin`] opened. A later write
    /// to the live store is not part of this cut.
    async fn read(&mut self) -> Result<SourceCut, ()>;
}

/// A cut whose bytes were copied while admission was held.
pub struct CopiedStore {
    cut: SourceCut,
}

impl CopiedStore {
    pub fn new(cut: SourceCut) -> Self {
        Self { cut }
    }
}

#[async_trait]
impl PinnedStore for CopiedStore {
    async fn read(&mut self) -> Result<SourceCut, ()> {
        Ok(self.cut.clone())
    }
}

/// Fences already committed, read while the hold lasts.
#[async_trait]
pub trait FenceSource: Send + Sync {
    async fn fences(&self) -> Result<Vec<DeletionFence>, ()>;
}

/// Why a deletion log was not replaced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogStoreError {
    /// The write did not replace the log.
    Failed,
    /// `base` is not the log on disk. Another writer stored first.
    Stale,
}

/// Ownership of one inventory reconciliation until the guard is dropped.
///
/// Implementations exclude other reconciliation holders for the same durable
/// inventory, including separately opened adapters.
pub trait InventoryReconciliationLease: Send + Sync {}

/// The deletion inventory file.
#[async_trait]
pub trait DeletionLogStore: Send + Sync {
    /// Exclude other captures through their source reads and final no-op or write.
    ///
    /// The guard is independent of the short lock used by `load` and `store`.
    /// Dropping a waiting caller must not leave a reconciliation holder behind
    /// (`a_cancelled_reconciliation_waiter_releases_its_eventual_lock`).
    async fn hold_reconciliation(&self) -> Result<Box<dyn InventoryReconciliationLease>, ()>;

    async fn load(&self) -> Result<DeletionLog, ()>;

    /// Replace the log with `next` when the stored log is still `base`.
    ///
    /// [`LogStoreError::Failed`] means this attempt did not replace the log.
    /// [`LogStoreError::Stale`] means another writer stored after `base` was
    /// read; the caller reconciles against the log now stored and tries again.
    async fn store(&self, base: &DeletionLog, next: &DeletionLog) -> Result<(), LogStoreError>;
}

/// Where a successful cut is written. Not called on abort.
#[async_trait]
pub trait ManifestPublisher: Send + Sync {
    async fn put(&self, manifest: &str, records: &[PublishedRecord]) -> Result<(), ()>;
}

/// One public record body to place beside the manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishedRecord {
    pub store: StoreKind,
    pub identity: String,
    pub bytes: Vec<u8>,
}

/// Unix milliseconds, injected.
pub trait ExportClock: Send + Sync {
    fn unix_ms(&self) -> u64;
}

/// Why a mac was not produced. The cut aborts before publish.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MacError {
    /// The key could not authenticate this text.
    Key,
}

/// Produce the mac of a manifest. A failure aborts the cut before publish.
pub trait Authenticator: Send + Sync {
    fn mac(&self, canonical: &str) -> Result<Fingerprint, MacError>;
}

/// The mac key. It is not written into the bundle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MacKey(pub [u8; 32]);

/// Who is asking to write the deletion inventory. A fact, not a verdict.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InventoryCause {
    /// A deletion, unshare, or credential revocation has committed.
    CommittedFence,
    /// The principal asked for a capture.
    Operator,
}

impl InventoryCause {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CommittedFence => "committed-fence",
            Self::Operator => "operator",
        }
    }
}

/// Which write was asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InventoryAction {
    /// Record fences a command just committed.
    Record,
    /// Capture because the principal asked.
    Capture,
}

impl InventoryAction {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Record => "deletion.inventory.record",
            Self::Capture => "deletion.inventory.capture",
        }
    }
}

/// Facts Cedar sees for one inventory write. `kind` is who is asking:
/// `gateway`, `person`, `surface`, `device`, `peer`, `environment-peer`,
/// or `worker`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InventoryFacts<'a> {
    pub kind: &'a str,
    pub cause: InventoryCause,
    pub action: InventoryAction,
}

/// Cedar's answer, naming the policies that decided it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InventoryDecision {
    Allow { policies: Vec<String> },
    Deny(InventoryDenial),
}

/// Why a write was not allowed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InventoryDenial {
    /// A forbid policy named these ids.
    Forbidden { policies: Vec<String> },
    /// No policy permitted it.
    NotPermitted,
    /// The policy could not be evaluated. Nothing is written.
    Unavailable,
}

/// Whether this principal may write the deletion inventory.
pub trait InventoryCapturePolicyEvaluator: Send + Sync {
    fn evaluate(&self, facts: &InventoryFacts<'_>) -> InventoryDecision;
}

/// Why a policy decision was not stored. The log must not be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecisionAuditError {
    /// The decision was not stored.
    NotStored,
}

/// Persist one inventory policy decision before the log is read or written.
///
/// The adapter assigns the record's identity and observation time.
/// [`DecisionAuditError::NotStored`] means the decision was not stored; the
/// caller must not read or write the log.
pub trait InventoryDecisionAudit: Send + Sync {
    fn record(
        &self,
        facts: &InventoryFacts<'_>,
        decision: &InventoryDecision,
    ) -> Result<(), DecisionAuditError>;
}

/// Which restore transition was asked. A fact for Cedar, not a verdict.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestoreAction {
    Stage,
    Accept,
    Fetch,
}

impl RestoreAction {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stage => "backup.restore.stage",
            Self::Accept => "backup.restore.accept",
            Self::Fetch => "backup.deferred.fetch",
        }
    }
}

/// Who is asking to stage, accept, or fetch. `kind` is the same principal
/// fact the inventory policies already see.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RestoreFacts<'a> {
    pub kind: &'a str,
    pub cause: InventoryCause,
    pub action: RestoreAction,
}

/// Whether this principal may stage, accept, or fetch a deferred conversation.
pub trait RestorePolicyEvaluator: Send + Sync {
    fn evaluate(&self, facts: &RestoreFacts<'_>) -> InventoryDecision;
}

/// One restore audit line, before the adapter adds its clock and record id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RestoreEvidence<'a> {
    pub target: &'a str,
    pub before: &'a str,
    pub after: &'a str,
    pub cause: &'a str,
    pub initiator: &'a str,
    pub decision: &'a str,
    pub policies: &'a [String],
    /// `lease receipt` lines for an acceptance. Empty for every other transition.
    pub attestations: &'a str,
}

/// Persist one restore transition. Failure means the effect must not be
/// reported as done. The adapter assigns identity and observation time.
pub trait RestoreAudit: Send + Sync {
    fn append(&self, evidence: &RestoreEvidence<'_>) -> Result<(), DecisionAuditError>;
}

/// Bytes copied while a deferred conversation is fetched.
pub trait FetchProgress {
    fn report(&mut self, read: u64, total: u64);
}

/// Why a deferred source did not produce a session file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionSourceError {
    Missing,
    Interrupted,
    Unreadable,
}

/// Size and stream fingerprint of one conversation in a session file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionCut {
    pub size: u64,
    pub digest: crate::backup::domain::Fingerprint,
}

/// The store that can still serve a deferred conversation's exact stream.
pub trait DeferredSessionSource {
    fn write_session(
        &self,
        conversation_id: &str,
        destination: &Path,
        progress: &mut dyn FetchProgress,
    ) -> Result<(), SessionSourceError>;

    fn inspect(&self, conversation_id: &str, path: &Path)
        -> Result<SessionCut, SessionSourceError>;
}

/// An operator attestation that one restored lease's executor was stopped.
pub trait LeaseReceipts {
    fn receipt(&self, lease_id: &str) -> Option<String>;
}

impl Authenticator for MacKey {
    /// HMAC-SHA256 of `canonical`. The key stays in `self`.
    fn mac(&self, canonical: &str) -> Result<Fingerprint, MacError> {
        use hmac::{Hmac, Mac};
        use sha2::Sha256;
        let mut mac =
            Hmac::<Sha256>::new_from_slice(&self.0).expect("HMAC-SHA256 accepts a 32-byte key");
        mac.update(canonical.as_bytes());
        let bytes = mac.finalize().into_bytes();
        let mut digest = [0u8; 32];
        digest.copy_from_slice(&bytes);
        Ok(Fingerprint::from_digest(digest))
    }
}
