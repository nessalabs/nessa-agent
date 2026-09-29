//! Storage adapters retain session history behind exclusive writer leases.
//! Memory storage keeps snapshots for its shared lifetime. Record storage
//! appends semantic facts to one SQLite runtime and folds them on load.
//!
//! ```text
//! SessionStorage::open -> SessionStorageLease <- SessionManager
//!                                      |-> memory snapshot
//!                                      |-> semantic facts -> SQLite runtime
//! ```
//! Arrows show calls and representation mapping. A complete framed fact is
//! folded into a validated snapshot. Pending operations retain the lease until
//! they finish. Erasing a session resets its stream under that lease. `paths`
//! identifies stale JSONL history so the record adapter refuses it unchanged.

mod memory;
mod paths;
mod record;
mod record_writer;
mod snapshot;
mod stream_fact;
pub use memory::InMemoryStorage;
pub use record::RecordStorage;
