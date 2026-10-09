//! Storage adapters retain session history behind exclusive writer leases.
//! Memory storage keeps snapshots for its shared lifetime. Record storage
//! appends unpublished semantic units and save completions to one SQLite runtime.
//! `save_group` owns their shared lineage/publication codec; replay retains the
//! private continuation and exposes only completed saves. The
//! bounded read source maps validated physical frames into sync-engine records
//! without taking the writer lease or repairing an incomplete tail. An
//! identity-only lookup lets the authorized host compare scope before opening
//! a worker; the expected-identity constructor rechecks the stream key. Bounded
//! product reads reuse terminal-discovery metadata/hash in a sixteen-entry cache
//! and return [`RecordReadStatus::Preparing`] until a captured tail is validated.
//! That cache retains no worker or semantic body.
//! `save_batch` arms one save's remaining physical events so their first append
//! commits a bounded chunk in one SQLite transaction. A rolled-back chunk
//! continues one event at a time.
//! `record_changes` publishes payloadless interest only at durable save completion
//! and Reset receipts. Closure preserves actual writer/read physical ownership.
//! Public producer/source/discovery acceptance lives under the external storage
//! integration tests; inherited private codec/allocation fixtures remain limited
//! implementation observations, not public acceptance.
//! TranscriptFold validates unit checkpoints through the one SDK session fold. A
//! record this build cannot read, or that contradicts the prefix already folded,
//! ends the fold there. The chat opens from that prefix. Later bytes stay on
//! disk and are not folded. A committed read cache advances from a fixed head
//! and remains separate from the writer's observed state.
//!
//! ```text
//! SessionStorage::open -> SessionStorageLease <- SessionManager
//!                                      |-> memory snapshot
//!                                      |-> units + completion -> SQLite runtime
//! RecordStorage -> identity metadata -> expected bounded read source -> sync engine
//! RecordStorage -> transcript fold -> committed gateway view
//! RecordStorage -> creation control stream -> principal command lease/receipts
//! RecordStorage -> bounded committed-change watches (no read or permission)
//! OwnershipStore -> ownership.sqlite3 snapshot rows
//!                -> ownership::settlement strict proof/Completion codec
//! MessageCommitClock <--------------------- Tokio monotonic clock adapter
//! ```
//! Arrows show calls and representation mapping. A complete outer save publishes
//! its validated snapshot. Pending operations retain the lease until
//! they finish. Erasing a session resets its stream under that lease. `paths`
//! identifies stale JSONL history so the record adapter refuses it unchanged.

mod creation;
mod memory;
mod message_commit_clock;
mod ownership;
mod paths;
mod record;
mod record_changes;
mod save_batch;
pub use record_changes::MAX_RECORD_CHANGE_WATCHES;
mod record_lifecycle;
mod record_source;
mod record_writer;
mod save_group;
mod snapshot;
mod stream_fact;
mod terminal_discovery;
pub use memory::InMemoryStorage;
pub use message_commit_clock::RuntimeMessageCommitClock;
pub use ownership::SqliteOwnershipStore;
pub use record::{RecordStorage, MAX_STORED_RECORD_BYTES};
pub use record_source::{
    physical_record_schema, NessaRecordSource, RecordStreamIdentity,
    MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
};
pub use terminal_discovery::RecordReadStatus;
mod transcript;
pub use transcript::{
    TranscriptCheckpoint, TranscriptError, TranscriptFold, TranscriptTransaction,
    MAX_TRANSCRIPT_CHECKPOINT_CHUNK_BYTES,
};

use crate::application::agent_execution::sessions::StorageError;

/// A saved record that cannot join the prefix already folded.
///
/// Unmarked bytes, another format version, a corrupt body, another chat's
/// identity, a body past the size limit, and a unit that contradicts the
/// prefix stop the fold. The chat stays open on that prefix.
pub(super) fn truncates_history(error: &StorageError) -> bool {
    matches!(
        error,
        StorageError::AnotherVersion { .. }
            | StorageError::Corrupt(_)
            | StorageError::IdentityMismatch
            | StorageError::TooLarge
    )
}

/// One warning for a fold that stopped early. The diagnostic text of a corrupt
/// body stays out of the log. No protocol field carries this to the app.
pub(super) fn warn_truncated(session: &str, position: u64, error: &StorageError) {
    let found = match error {
        StorageError::AnotherVersion { found } => *found,
        _ => None,
    };
    let reason = match error {
        StorageError::AnotherVersion { .. } => "another_version",
        StorageError::IdentityMismatch => "identity",
        _ => "unreadable",
    };
    tracing::warn!(
        session,
        position,
        reason,
        found,
        "session history truncated at the first unreadable record"
    );
}
