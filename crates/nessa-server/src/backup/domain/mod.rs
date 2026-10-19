//! What a cut is allowed to hold. No clock, no files, no serde.

mod deletion;
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

pub(crate) use encoding::encode_identity;
pub(crate) use record::MAX_NAME_BYTES;
pub use verification::{ListedRecord, ListedRecords, ListedStore, ManifestListing};
