//! Durable installation audit journal.
//!
//! ```text
//! InstallAudit -> journal (retained directory, lock, sequence, publication)
//!                        -> record (JSON mapping through domain constructors)
//! ```
//! Arrows mean calls. The journal owns filesystem authority; the record module
//! owns only the private persistence representation.

mod journal;
mod record;

pub use journal::DurableInstallAudit;
