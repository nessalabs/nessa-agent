//! What `nessa env serve` runs on: `launcher.rs` starts harnesses from the
//! host's own `config.json`, `ledger.rs` is the append-only audit of its
//! leases (`environment/leases.jsonl` beside it), and `lock.rs` keeps one
//! serving process per data directory.
mod launcher;
mod ledger;
mod lock;
pub(crate) use launcher::{ConfiguredLauncher, LaunchSpec};
pub(crate) use ledger::FileLedger;
pub(crate) use lock::{ServeLock, ServeLockError};
