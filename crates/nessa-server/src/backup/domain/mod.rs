//! What a cut is allowed to hold. No clock, no files, no serde.

mod deletion;
mod inventory;
mod manifest;
mod record;

pub use deletion::{
    Append, CredentialKind, DeletionEntry, DeletionError, DeletionFence, DeletionLog, FenceKind,
    GrantRole,
};
pub use inventory::{StoreKind, WRITER_INVENTORY};
pub use manifest::{AuthenticatedManifest, Manifest};
pub use record::{
    outbox_watermark, CapturedRecord, DeferredEntry, Fingerprint, RecordBody, RecordError,
    Sensitivity, SkipReason, SkippedRecord, SourceCut, Watermark,
};
