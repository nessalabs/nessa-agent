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

pub use encoding::{decode_identity, encode_identity};
pub use record::MAX_NAME_BYTES;
pub use restore::{
    attestation_mentions, attested_pair, audit_path, commands_after_restore,
    conversation_is_fenced, credential_is_fenced, credentials_imported_on_restore,
    decode_accepted_restore, decode_deferred, deferred_conversation_ids, deferred_for_open,
    deferred_session_name, encode_accepted_restore, encode_deferred, encode_journal,
    extra_session_allowed, fresher_fences, historical_commands_are_blocked, host_volume,
    leases_to_interrupt, metadata_omitted, normalized_root, path_identity, paths_are_distinct,
    paths_conflict, paths_conflict_on, posture, read_journal, receipt_is_usable,
    relative_path_is_safe, restore_lock_path, seal_attestation, seal_files, seal_staging_id,
    session_sidecar, share_grant_is_fenced, staging_id_is_usable, AcceptedRestorePointer,
    DeferredLine, Freshness, Journal, LeaseMark, MetadataFacts, MetadataRow, Posture, RestoreFault,
    SealedFile, VolumeCase, VolumeProbe, ACCEPTED_RESTORE_HEADER, IDENTITY_BODY, JOURNAL_HEADER,
    REPLAY_BODY,
};
pub use verification::{ListedRecord, ListedRecords, ListedStore, ManifestListing};
