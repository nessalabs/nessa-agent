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
    attestation_mentions, attested_pair, audit_path, conversation_is_fenced, credential_is_fenced,
    decode_deferred, deferred_for_open, deferred_session_name, encode_deferred, encode_journal,
    extra_session_allowed, fresher_fences, host_volume, leases_to_interrupt, metadata_omitted,
    normalized_root, path_identity, paths_are_distinct, paths_conflict, paths_conflict_on, posture,
    read_journal, receipt_is_usable, relative_path_is_safe, restore_lock_path, seal_attestation,
    seal_files, seal_staging_id, session_sidecar, share_grant_is_fenced, staging_id_is_usable,
    DeferredLine, Freshness, Journal, LeaseMark, MetadataFacts, MetadataRow, Posture, RestoreFault,
    SealedFile, VolumeCase, VolumeProbe, IDENTITY_BODY, REPLAY_BODY,
};
pub use verification::{ListedRecord, ListedRecords, ListedStore, ManifestListing};
