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
/// `Sync` so the future that checks it before and after each read can move
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

    /// Read the store. `Err` means the store could not be listed at all.
    async fn capture(&self) -> Result<SourceCut, ()>;
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

/// Produce the mac of a manifest. A failure aborts the cut before publish.
pub trait Authenticator: Send + Sync {
    fn mac(&self, canonical: &str) -> Result<Fingerprint, ()>;
}

/// The mac key. It is not written into the bundle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MacKey(pub [u8; 32]);

impl Authenticator for MacKey {
    /// HMAC-SHA256 of `canonical`. The key stays in `self`.
    fn mac(&self, canonical: &str) -> Result<Fingerprint, ()> {
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
