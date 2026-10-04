//! The conversation read model: what a conversation is, as the gateway
//! answers it and a device reads it back.
//!
//! ```text
//! projection --> view --> domain
//!            --> tool_uis (port: a tool's declared UI)
//! catalogue_payload --> catalogue_metadata --> domain
//! read_scope --> domain (catalogue identity)
//! ```
//! Arrows are compile-time dependencies.
//!
//! - `domain`: conversation identity, model and approval choice, the summary a
//!   list shows, and the identity of an owner's catalogue.
//! - `view`: the `conversation.read` shape, and `projection`, the one bounded
//!   fold of committed SDK records into it. Live gateway reads and a device's
//!   offline `show` both read through `projection`.
//! - `catalogue_metadata` and `catalogue_payload`: a catalogue entry's
//!   metadata and its one stored representation, encoded and decoded together.
//! - `read_scope`: the scope a passive read is admitted for, and the checks
//!   both ends make of a source scope against it.
//!
//! Admission, ownership, deletion and the receiver binding stay with the
//! gateway's conversation context; nothing here grants a read.
pub mod catalogue_metadata;
pub mod catalogue_payload;
pub mod domain;
pub mod projection;
pub mod read_scope;
pub mod tool_uis;
pub mod view;
