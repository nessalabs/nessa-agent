//! What `nessa env serve` runs on: `launcher.rs` starts harnesses from the
//! host's own `config.json`, `commands.rs` runs command leases' commands,
//! `ledger.rs` is the append-only audit of its leases
//! (`environment/leases.jsonl` beside it), and `lock.rs` keeps one serving
//! process per data directory; `outbox.rs` stages what harnesses publish
//! (`environment/outbox/`) and serves each lease's publish point. `dial.rs`
//! is how a paired environment reaches its gateway instead of being reached.
mod commands;
mod dial;
mod launcher;
mod ledger;
mod lock;
mod outbox;
pub(crate) use commands::{ShepherdCommands, COMMAND_VARIABLES};
pub(crate) use dial::dial;
pub(crate) use launcher::{ConfiguredLauncher, LaunchSpec};
pub(crate) use ledger::FileLedger;
pub(crate) use lock::{ServeLock, ServeLockError};
pub(crate) use outbox::FileOutbox;
