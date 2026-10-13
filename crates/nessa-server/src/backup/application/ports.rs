//! What a cut needs from outside the process, and from the admission lock.

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

/// The deletion inventory file.
#[async_trait]
pub trait DeletionLogStore: Send + Sync {
    async fn load(&self) -> Result<DeletionLog, ()>;

    /// Replace the log with `next` only when `next` was produced by appending
    /// to the loaded log. A failure leaves the previous log in place.
    async fn store(&self, next: &DeletionLog) -> Result<(), ()>;
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
