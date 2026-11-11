//! The conversation read model: what a conversation is, as the gateway
//! answers it and a device reads it back.
//!
//! ```text
//! view --> domain
//! catalogue_payload --> catalogue_metadata --> domain
//! read_scope --> domain (catalogue identity)
//! ```
//! Arrows are compile-time dependencies.
//!
//! - `domain`: conversation identity, model and approval choice, the summary a
//!   list shows, and the identity of an owner's catalogue.
//! - `view`: the `conversation.read` shape. The one bounded fold of committed
//!   SDK records into it is `nessa-projection`, so this crate does not depend
//!   on the SDK. Live gateway reads and a device's offline `show` both read
//!   through it.
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
pub mod fence_capture;
pub mod read_scope;
pub mod view;
