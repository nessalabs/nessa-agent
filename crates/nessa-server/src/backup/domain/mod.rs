//! What a cut is allowed to hold. No clock, no files, no serde.

mod deletion;
mod encoding;
mod inventory;
mod manifest;
mod record;
mod restore;
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

pub(crate) use encoding::{decode_identity, encode_identity};
pub(crate) use record::MAX_NAME_BYTES;
pub use restore::{
    conversation_is_fenced, credential_is_fenced, decode_deferred, deferred_for_open,
    deferred_session_name, encode_deferred, encode_journal, extra_session_allowed, fresher_fences,
    leases_to_interrupt, metadata_omitted, paths_are_distinct, posture, read_journal,
    receipt_is_usable, relative_path_is_safe, seal_files, share_grant_is_fenced, DeferredLine,
    Freshness, Journal, LeaseMark, MetadataFacts, MetadataRow, Posture, RestoreFault, SealedFile,
    IDENTITY_BODY, REPLAY_BODY,
};
pub use verification::{ListedRecord, ListedRecords, ListedStore, ManifestListing};
