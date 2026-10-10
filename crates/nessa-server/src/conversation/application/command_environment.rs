//! The command port: an environment that runs one bounded command at a time
//! for a conversation's agent, under a command lease (ADR 252 rows L14, L15;
//! issue #700).
//!
//! ```text
//! CommandEnvironment::grant(lease, command) ──▶ CommandHold | CommandRefusal
//!   (the service records CommandIssued between the two)
//! CommandHold::run(stop) ──▶ the command runs ──▶ CommandResult
//!   stop said ──▶ the environment stops it ──▶ CommandResult{Stopped}
//!   the hold dropped unrun ──▶ the environment ends the lease, unanswered
//! ```
//!
//! Arrows are calls, in order. A command lease has no harness and keeps no
//! session: it runs one argument vector once and its result ends it. Every
//! wait is bounded by the adapter: a grant by its answer bound, a run by the
//! command's own timeout and what stopping it takes.
use super::EnvironmentFuture;
use nessa_sdk::domain::agent_execution::leases::{
    CommandExit, CommandRefusal, CommandWork, LeaseCleanup, LeaseEndCause, LeaseId,
};
use tokio::sync::watch;

/// Where commands run. Implemented in infrastructure, beside the environment
/// it runs them in.
pub(crate) trait CommandEnvironment: Send + Sync {
    /// Whether the environment is known reachable now: `Some(true)` with a
    /// connection open, `None` when not known without reaching it. No I/O.
    fn reachable(&self) -> Option<bool>;
    /// Admit `lease` to run `command` once. Answers within a bound of its
    /// own; a refusal is the environment's, and nothing ran.
    fn grant<'a>(
        &'a self,
        lease: &'a LeaseId,
        command: &'a CommandWork,
    ) -> EnvironmentFuture<'a, Result<Box<dyn CommandHold>, CommandRefusal>>;
}

/// An environment's side of one admitted command lease. Dropped unrun, the
/// environment is asked to end it, unanswered.
pub(crate) trait CommandHold: Send {
    /// Run the command once and answer how it ended. `stop`, once it holds a
    /// cause, stops it, and the command answers as stopped for that cause.
    fn run(
        self: Box<Self>,
        stop: watch::Receiver<Option<LeaseEndCause>>,
    ) -> EnvironmentFuture<'static, CommandResult>;
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
}

impl CommandResult {
    /// The answer for a command whose environment said nothing.
    pub(crate) fn unanswered() -> Self {
        Self {
            exit: CommandExit::Unanswered,
            stdout: Vec::new(),
            stderr: Vec::new(),
            dropped_bytes: 0,
            cleanup: None,
        }
    }
}
