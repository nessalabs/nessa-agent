//! What `nessa env serve` runs on: `launcher.rs` starts harnesses from the
//! host's own `config.json`, `commands.rs` runs command leases' commands,
//! `ledger.rs` is the append-only audit of its leases
//! (`environment/leases.jsonl` beside it), and `lock.rs` keeps one serving
//! process per data directory; `outbox.rs` stages what harnesses publish
//! (`environment/outbox/`) and serves each lease's publish point, and
//! `inbox.rs` is where fetched files arrive (`environment/inbox/`) and how
//! each is placed in the workspace. `dial.rs`
//! is how a paired environment reaches its gateway instead of being reached,
//! and `pairing.rs` is that machine's pairing (`environment/pairing`),
//! enrolled and asked whether the owner approved it; `self_update.rs` is
//! that machine taking its gateway's build when the gateway speaks another
//! lease protocol, and running it in place of its own. `grants.rs` is what
//! this host's owner grants each gateway (`environment/grants.json`), and
//! `grant_audit.rs` the journal of every change made to it; `cedar/` is
//! the lease policy deciding, over a peer's entry there, whether it may
//! lease an agent or run a command (the only place this crate's environment
//! role names Cedar). The runtime's
//! clock every wait in the application is measured on is
//! `crate::timer::TokioTimer`, shared with the relay.
mod cedar;
mod commands;
mod dial;
mod grant_audit;
mod grants;
mod inbox;
mod launcher;
mod ledger;
mod lock;
mod outbox;
mod pairing;
mod self_update;
pub(crate) use cedar::CedarLeasePolicy;
pub(crate) use commands::{ShepherdCommands, COMMAND_VARIABLES};
pub(crate) use dial::dial;
pub(crate) use grant_audit::FileGrantAudit;
pub(crate) use grants::{parse_peer, peer_name, printed, FileGrants};
pub(crate) use inbox::{note_umask, FileInbox};
pub(crate) use launcher::{ConfiguredLauncher, LaunchSpec};
pub(crate) use ledger::FileLedger;
pub(crate) use lock::{ServeLock, ServeLockError};
pub(crate) use outbox::FileOutbox;
pub(crate) use pairing::{Approved, FileEnrollmentCode, FilePairingEnvironment};
pub(crate) use self_update::{fetch, kept_for, run_in_place, this_build, FETCH_DEADLINE};
