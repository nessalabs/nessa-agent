//! Storage adapters retain session history behind exclusive writer leases.
//! Memory storage keeps snapshots for its shared lifetime. Local storage appends
//! private JSONL changes and reconstructs validated snapshots on load.
//!
//! ```text
//! SessionStorage::open -> SessionStorageLease <- SessionManager
//!                                      |-> memory snapshot
//!                                      |-> JSONL changes -> private journal
//! ```
//! Arrows show calls and representation mapping. Each completed journal line is
//! one logical snapshot replacement; unchanged earlier history is not rewritten.
//! Pending file operations retain the lease until they finish. Erasing a session
//! removes its history under that lease and keeps the lease's exclusion. JSON mapping and
//! checkpoint validation live in `snapshot`; file ownership and sync live in `local`.
//! `paths` encodes exact identities into case-fold-safe journal and lease filenames.
//! `snapshot::baseline` projects a validated legacy snapshot into deterministic
//! bounded pieces plus an integrity seal. It performs no file read, stream append,
//! authority selection, or provider effect; composition must own the cutover.

mod local;
mod memory;
mod paths;
mod snapshot;
pub use local::LocalFileStorage;
pub use memory::InMemoryStorage;
pub use snapshot::{
    decode_baseline, encode_baseline, BaselineExport, BaselinePiece, BaselineSeal, BaselineSection,
};
