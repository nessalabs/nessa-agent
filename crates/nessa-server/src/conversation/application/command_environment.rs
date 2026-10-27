//! The command port: an environment that runs one bounded command at a time
//! for a conversation's agent, under a command lease (ADR 252 rows L14, L15;
//! issue #700).
//!
//! ```text
//! CommandEnvironment::grant(lease, command) ──▶ GrantedCommand | CommandRefusal
//!   (the service checks the terms granted, then records CommandIssued with
//!    them, before the hold runs)
//! CommandHold::run(granted timeout, stop) ──▶ the command runs ──▶ CommandResult
//!   stop said ──▶ the environment stops it ──▶ CommandResult{Stopped}
//!   publications() ──▶ files the command publishes, collected while it runs
//!   the hold dropped unrun ──▶ the environment ends the lease, unanswered
//! ```
//!
//! Arrows are calls, in order. A command lease has no harness and keeps no
//! session: it runs one argument vector once and its result ends it. Every
//! wait is bounded by the adapter: a grant by its answer bound, a run by the
//! command's own timeout and what stopping it takes.
use super::{environment::GrantedTerms, ArtifactOffer, EnvironmentFuture};
use nessa_protocol::lease::CollectionRefusal;
use nessa_sdk::domain::agent_execution::leases::{
    CommandExit, CommandRefusal, CommandWork, LeaseCleanup, LeaseEndCause, LeaseId,
};
use tokio::sync::{mpsc, watch};

/// Where commands run. Implemented in infrastructure, beside the environment
/// it runs them in.
pub(crate) trait CommandEnvironment: Send + Sync {
    /// Whether the environment is known reachable now: `Some(true)` with a
    /// connection open, `None` when not known without reaching it. No I/O.
    fn reachable(&self) -> Option<bool>;
    /// Admit `lease` to run `command` once. Answers within a bound of its
    /// own, with the terms the environment granted, which may be narrower
    /// than asked; a refusal is the environment's, and nothing ran.
    fn grant<'a>(
        &'a self,
        lease: &'a LeaseId,
        command: &'a CommandWork,
    ) -> EnvironmentFuture<'a, Result<GrantedCommand, CommandRefusal>>;
}

/// A command lease an environment admitted: its hold, and the terms it
/// granted, as it answered them. The service checks those against what it
/// asked, records them, and runs the hold with the timeout it checked.
pub(crate) struct GrantedCommand {
    pub(crate) hold: Box<dyn CommandHold>,
    pub(crate) granted: GrantedTerms,
}

/// An environment's side of one admitted command lease. Dropped unrun, the
/// environment is asked to end it, unanswered.
pub(crate) trait CommandHold: Send {
    /// Run the command once and answer how it ended. `timeout_ms` is the
    /// timeout it was granted, as the service checked and recorded it: the
    /// environment waits for its answer that long and what stopping it
    /// takes. `stop`, once it holds a cause, stops it, and the command
    /// answers as stopped for that cause.
    fn run(
        self: Box<Self>,
        timeout_ms: u64,
        stop: watch::Receiver<Option<LeaseEndCause>>,
    ) -> EnvironmentFuture<'static, CommandResult>;

    /// Files the command publishes while it runs, until the lease is no
    /// longer routed. `None` when the command has no publish point. Taken
    /// once, before [`Self::run`].
    fn publications(&mut self) -> Option<mpsc::Receiver<ArtifactOffer>> {
        None
    }
}

/// What running a command came to, as the environment said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandResult {
    pub exit: CommandExit,
    /// The newest of its standard output that was kept.
    pub stdout: Vec<u8>,
    /// The newest of its standard error that was kept.
    pub stderr: Vec<u8>,
    /// Bytes it printed that were not kept.
    pub dropped_bytes: u64,
    /// What releasing it took; `None` when the environment could not say.
    pub cleanup: Option<LeaseCleanup>,
    /// Each file the command published, in the order it was offered.
    pub published: Vec<CommandPublication>,
}

/// One file a command published, as `run` lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandPublication {
    /// Its name, as the environment published it.
    pub name: String,
    /// Its SHA-256, lowercase hex, as the environment published it.
    pub digest: String,
    /// Its length, as the environment published it.
    pub size: u64,
    /// Held, already held, or refused.
    pub outcome: PublicationOutcome,
}

/// What the conversation made of one file a command published.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublicationOutcome {
    /// Read, verified, and held, with the command lease as cause.
    Held,
    /// The conversation already held exactly this file.
    AlreadyHeld,
    /// Not kept.
    Refused(CollectionRefusal),
}

impl CommandResult {
    /// The answer for a command whose environment said nothing.
    // Its callers are the SSH adapter's, which is Unix only.
    #[cfg_attr(not(unix), allow(dead_code))]
    pub(crate) fn unanswered() -> Self {
        Self {
            exit: CommandExit::Unanswered,
            stdout: Vec::new(),
            stderr: Vec::new(),
            dropped_bytes: 0,
            cleanup: None,
            published: Vec::new(),
        }
    }
}
