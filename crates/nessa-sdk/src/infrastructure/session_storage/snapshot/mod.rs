//! Typed JSON representations stay at the storage boundary. `journal` writes
//! changed metadata and history tails, then replays each completed checkpoint
//! through these mappings and shared application/domain validation.
//!
//! ```text
//! JSONL line -> bounded decode preflight -> changed records -> validated SessionSnapshot
//! ```
//! The arrows mean allocation admission, decoding and explicit inward mapping,
//! never provider replay. `decode` bounds token scratch, decoded fields, collection
//! structure, error trees and ordered change indices before an owned record exists.
//! Its 160 MiB allowance is per changed invocation, not a total-history/RSS limit;
//! complete checkpoints can contain many individually valid historical turns.
//! Restoration checks every checkpoint, so validation work can grow faster than
//! the journal's byte size. No previous history is re-encoded during a normal save.
//! Cancellation maps an undispatched local decision and caller separately from
//! scheduling edges and provider settlement reports.
mod cancellation;
mod decode;
mod errors;
mod journal;
mod permissions;
mod records;
mod scheduling;
mod settlement;
mod tools;
pub(super) use crate::application::agent_execution::sessions::validation::validate;
pub(super) use journal::{encode, read};
