//! Typed JSON representations stay at the storage boundary. Semantic facts
//! replay through these mappings and shared application/domain validation.
//!
//! ```text
//! framed fact -> bounded decode preflight -> typed changes -> validated SessionSnapshot
//! ```
//! The arrows mean allocation admission, decoding and explicit inward mapping,
//! never provider replay. `decode` bounds token scratch, decoded fields, collection
//! structure and error trees before an owned fact exists. Its 160 MiB allowance
//! applies to each semantic batch. No previous history is re-encoded during a save.
//! Cancellation maps an undispatched local decision and caller separately from
//! scheduling edges and provider settlement reports.
mod cancellation;
pub(super) mod checkpoint;
pub(super) mod decode;
mod errors;
mod permissions;
mod queue_order;
mod records;
mod scheduling;
mod semantic;
mod settlement;
mod tools;
pub(super) use crate::application::agent_execution::sessions::validation::validate;
pub(super) use semantic::{
    decode_batch as decode_semantic_batch, encode_batch as encode_semantic_batch,
    encode_change as encode_semantic_change,
};
