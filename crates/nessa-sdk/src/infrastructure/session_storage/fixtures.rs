//! Session records as main `4d1b7278` writes them, before `schemaVersion`.
//!
//! The batch is one `Opened` and one `InputAccepted` from `encode_batch`.
//! The checkpoint is `TranscriptFold::checkpoint` after one saved `Opened`.
//! Neither file contains a `schemaVersion` field.

/// Semantic batch bytes from main's encoder.
pub const UNMARKED_SESSION_BATCH: &[u8] = include_bytes!("fixtures/unmarked_session_batch.json");

/// Transcript checkpoint bytes from main's encoder.
pub const UNMARKED_SESSION_CHECKPOINT: &[u8] =
    include_bytes!("fixtures/unmarked_session_checkpoint.json");
