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
//!
//! A list of conversations is read beside all of that rather than through it:
//! ownership records say whose each conversation is, a separate summary store
//! says what was said last, and only live state already in memory says whether
//! a turn is running. Nothing is opened to draw one.
//!
//! Archiving is a flag in that summary. Deleting is the one command that
//! removes what a conversation holds — its history, uploads, and summary —
//! after a tombstone in the ownership store has fenced it, its agent has been
//! asked to delete its own record of the session, and its deletion is on
//! record; its evidence stays. The order is `ConversationService::delete`'s,
//! and a deletion that did not finish is finished from its tombstone when the
//! gateway starts.
pub mod application;
pub mod domain;
pub mod infrastructure;

#[cfg(test)]
#[path = "../../tests/conversation/agreement.rs"]
mod agreement_tests;
