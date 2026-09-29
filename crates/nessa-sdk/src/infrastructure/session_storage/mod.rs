//! Storage adapters retain session history behind exclusive writer leases.
//! Memory storage keeps snapshots for its shared lifetime. Record storage
//! appends semantic facts to one SQLite runtime and folds them on load. The
//! bounded read source maps validated physical frames into sync-engine records
//! without taking the writer lease or repairing an incomplete tail.
//!
//! ```text
//! SessionStorage::open -> SessionStorageLease <- SessionManager
//!                                      |-> memory snapshot
//!                                      |-> semantic facts -> SQLite runtime
//!                                      |-> bounded read source -> sync engine
//! MessageCommitClock <--------------------- Tokio monotonic clock adapter
//! ```
//! Arrows show calls and representation mapping. A complete framed fact is
//! folded into a validated snapshot. Pending operations retain the lease until
//! they finish. Erasing a session resets its stream under that lease. `paths`
//! identifies stale JSONL history so the record adapter refuses it unchanged.

mod memory;
mod message_commit_clock;
mod paths;
mod record;
mod record_source;
mod record_writer;
mod snapshot;
mod stream_fact;
pub use memory::InMemoryStorage;
pub use message_commit_clock::RuntimeMessageCommitClock;
pub use record::RecordStorage;
pub use record_source::{physical_record_schema, NessaRecordSource};
