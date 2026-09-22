//! Product conversations bind authenticated owners to one SDK Agent.
//! `application -> domain` enforces access; `infrastructure -> application` stores metadata.
//! The SDK owns execution, queueing, permissions, and provider cleanup.
//!
//! A message carries images by reference and points at every other file by
//! absolute path. The two are checked in opposite ways, because they travel in
//! opposite ways:
//!
//! ```text
//!   submit ──images──▶ ConversationAttachments::holds   (does this conversation
//!          │                                             own these bytes?)
//!          └──files───▶ LinkedFile::new                  (can this path be said?)
//!                              │
//!                              └──▶ ConversationFileLinkAudit  (who pointed the
//!                                     agent here — before admission)
//! ```
//!
//! Arrows are calls, in the order `submit` makes them. An image has an upload
//! behind it, so there is ownership to verify and a hold to release on close. A
//! path has nothing behind it: nothing was uploaded, nothing is held, and
//! nothing here opens the file — so what the gateway keeps instead is evidence
//! of who pointed the agent at it. See
//! [ADR 0013](../../../../docs/adr/done/0013-files-by-path-not-by-payload.md).
pub mod application;
pub mod domain;
pub mod infrastructure;
