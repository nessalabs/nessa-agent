//! What a cut is allowed to hold. No clock, no files, no serde.

mod deletion;
mod deletion_decode;
mod encoding;
mod inventory;
mod manifest;
mod record;
mod verification;

pub use deletion::{
    Append, CredentialKind, DeletionEntry, DeletionError, DeletionFence, DeletionLog, FenceKind,
    GrantRole,
};
pub use inventory::{StoreKind, WRITER_INVENTORY};
pub use manifest::{AuthenticatedManifest, Manifest, EXPORT_HEADER};
pub use record::{
    outbox_watermark, CapturedRecord, DeferredEntry, Fingerprint, RecordBody, RecordError,
    Sensitivity, SkipReason, SkippedRecord, SourceCut, Watermark,
};

pub use deletion_decode::DeletionReadError;
pub(crate) use encoding::encode_identity;
pub use verification::{ListedRecord, ListedStore, ManifestListing, ManifestReadError};
