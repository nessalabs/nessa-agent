//! Durable installation audit journal.
//!
//! ```text
//! InstallAudit -> journal (retained directory, lock, sequence, publication)
//!                        -> record (JSON mapping through domain constructors)
//! ```
//! Arrows mean calls. The journal owns filesystem authority; the record module
//! owns only the private persistence representation. Syntactically valid
//! abandoned reservations are preserved and ignored; this journal does not
//! reclaim their disk usage.

mod journal;
mod record;

pub use journal::DurableInstallAudit;
