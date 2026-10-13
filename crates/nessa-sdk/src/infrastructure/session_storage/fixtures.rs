//! Session records as main `4d1b7278` writes them, before `schemaVersion`.
//!
//! The batch is one `Opened` and one `InputAccepted` from `encode_batch`.
//! The checkpoint is `TranscriptFold::checkpoint` after one saved `Opened`.
//! Neither file contains a `schemaVersion` field.
//!
//! The role-copying batch is one `Opened` and one grant-started
//! `InputAccepted` from this build's encoder, with the grant's role added
//! back onto the turn as an earlier build of version 3 wrote it.

/// Semantic batch bytes from main's encoder.
pub const UNMARKED_SESSION_BATCH: &[u8] = include_bytes!("fixtures/unmarked_session_batch.json");

/// Transcript checkpoint bytes from main's encoder.
pub const UNMARKED_SESSION_CHECKPOINT: &[u8] =
    include_bytes!("fixtures/unmarked_session_checkpoint.json");

/// Semantic batch bytes whose turn carries a copy of its grant's role.
pub const ROLE_COPYING_SESSION_BATCH: &[u8] =
    include_bytes!("fixtures/role_copying_session_batch.json");
