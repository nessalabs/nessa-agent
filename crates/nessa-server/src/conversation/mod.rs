//! Product conversations bind authenticated owners to one SDK Agent.
//! `application -> domain` enforces access; `infrastructure -> application` stores metadata.
//! The SDK owns execution, queueing, permissions, and provider cleanup.
//!
//! Images and uploaded files retain their original stored references. Linked
//! files instead name paths already present in the execution workspace:
//!
//! ```text
//!   submit ──stored references──▶ ConversationAttachments::holds
//!          │                       (does this conversation own these bytes?)
//!          └─linked file paths──▶ ConversationFileLinkAudit::record
//!                                  (who named which path, and when?)
//!   dispatch ──uploaded files──▶ lease-bound input resolver
//!                                  (verified placement and outcome evidence)
//! ```
//!
//! Stored references keep their holds through accepted input and recovery.
//! Prepared paths belong only to the execution copy, never the saved input.
//! A linked path has no upload behind it; naming one records sender evidence
//! without opening it. See [ADR 0013](../../../../docs/adr/done/0013-files-by-path-not-by-payload.md)
//! for linked paths, and [phone uploads](../../../../docs/design/phone-file-uploads.md)
//! for stored-file delivery.
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

#[cfg(test)]
#[path = "../../tests/conversation/passive_read.rs"]
mod passive_read_tests;
