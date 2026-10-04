//! The bounded `watch` loop: register one record watch, run the recheck pass
//! from the durable checkpoint, then one pass per hint until the pass budget,
//! an idle wait, or the connection ends (committed change watches, rows
//! W2–W15).
//!
//! ```text
//! register --> recheck pass --> wait --> hint --> pass --> wait ...
//!     |              |            |
//!  refused        failed       idle / ended / closed
//!                    |
//!          source_preparing --> the same pass again (bounded)
//! ```
//! Arrows are calls in order on one connection, one operation at a time. The
//! session keeps hints that arrive during a pass, so `wait` returns at once
//! when one is held. This loop decides only what ends the run; composition
//! asks the pinned status again for an end whose cause `asks_status` names
//! (row PC5).
use super::{device::asks_status, GatewayAttempt, GatewayError};
use crate::product_contract::generated::{ChangeWatchEndReason, RecordReadErrorCode};
use std::num::NonZeroUsize;

/// What one wait for a hint found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Wait {
    /// At least one hint: kept from an earlier operation (`during_pass`), or
    /// read while waiting.
    Hint { during_pass: bool },
    /// The gateway ended the watch; nothing more will be hinted.
    Ended(ChangeWatchEndReason),
}

/// Why a pass ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Trigger {
    /// The pass after registration, from the durable checkpoint (row W4).
    Recheck,
    Hint,
    /// The previous pass stopped at its page budget (row W7).
    Incomplete,
    /// The previous attempt was answered `source_preparing`: the same pass
    /// again, at once (row W17).
    Preparing,
}

/// Attempts one pass may make while the gateway answers `source_preparing`,
/// the first included. The gateway keeps its preparation progress between
/// attempts, so each one resumes it; the bound keeps a source that never
/// becomes ready an explicit failure.
pub(crate) const PREPARING_ATTEMPTS: usize = 4;

/// The gateway's answer that the source is not ready yet: its progress is
/// retained, and the same read may be asked again.
fn preparing(cause: GatewayError) -> bool {
    cause == GatewayError::Record(RecordReadErrorCode::SourcePreparing)
}

/// Discovery before registration (row W17): an attempt answered
/// `source_preparing` kept the session and the gateway's progress, so it is
/// asked again at once, up to `PREPARING_ATTEMPTS` attempts. Returns the last
/// attempt, whatever it answered; an `Err` from `attempt` is returned as is.
pub(crate) fn discover<R, E>(
    mut attempt: impl FnMut() -> Result<GatewayAttempt<R>, E>,
) -> Result<GatewayAttempt<R>, E> {
    let mut attempts = 1;
    loop {
        let discovery = attempt()?;
        match discovery.outcome.failure {
            Some(cause) if preparing(cause) && attempts < PREPARING_ATTEMPTS => attempts += 1,
            _ => return Ok(discovery),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PassResult {
    Complete,
    Incomplete,
    /// The pass did not succeed: with the gateway's cause, or with none when
    /// the cache or driver refused (its report holds that cause).
    Failed(Option<GatewayError>),
}

/// One pass: its presentation and what it means for the loop.
pub(crate) struct WatchPass<R> {
    pub(crate) report: R,
    pub(crate) result: PassResult,
}

/// An admitted registration: the watch identity and the connection operation
/// that registered it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Registered {
    pub(crate) watch: String,
    pub(crate) operation: u64,
}

/// The one connection, its cache and its finite driver, one operation per call.
pub(crate) trait WatchSession {
    type Report;
    fn register(&mut self) -> Result<Registered, GatewayError>;
    /// Under the ordinary operation deadline; `TimedOut` means nothing came.
    fn wait(&mut self) -> Result<Wait, GatewayError>;
    /// `Err` when the pass could not begin: nothing was read or saved, so it
    /// has no report.
    fn pass(&mut self) -> Result<WatchPass<Self::Report>, GatewayError>;
}

/// The standard output could not take a line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OutputLost;

/// Lines in arrival order. A refused line stops the loop (row W14).
pub(crate) trait WatchEvents<R> {
    fn registered(&mut self, registered: &Registered) -> Result<(), OutputLost>;
    fn pass(&mut self, trigger: Trigger, report: &R) -> Result<(), OutputLost>;
    fn hint(&mut self, during_pass: bool) -> Result<(), OutputLost>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EndReason {
    PassesExhausted,
    Idle,
    RegistrationRefused,
    Unauthorized,
    Unavailable,
    PassFailed,
    WatchEnded(ChangeWatchEndReason),
    /// The gateway closed the connection. On the native profile a close
    /// carries no reason, so this claims nothing about why (row W11).
    ConnectionClosed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct End {
    pub(crate) reason: EndReason,
    pub(crate) cause: Option<GatewayError>,
}
impl End {
    /// A bounded run that ended with nothing wrong (owner's decision: idle
    /// is one).
    pub(crate) fn clean(self) -> bool {
        matches!(self.reason, EndReason::PassesExhausted | EndReason::Idle)
    }
}

/// Run the watch until one of the `EndReason`s. Every pass counts toward
/// `max_passes`, the recheck included.
pub(crate) fn follow<S: WatchSession, E: WatchEvents<S::Report>>(
    session: &mut S,
    events: &mut E,
    max_passes: NonZeroUsize,
) -> Result<End, OutputLost> {
    let registered = match session.register() {
        Ok(registered) => registered,
        Err(cause @ GatewayError::Watch(_)) => {
            return Ok(end(EndReason::RegistrationRefused, Some(cause)))
        }
        Err(cause) => return Ok(failed(cause)),
    };
    events.registered(&registered)?;
    let mut trigger = Trigger::Recheck;
    let mut passes = 0_usize;
    let mut attempts = 0_usize;
    loop {
        let pass = match session.pass() {
            Ok(pass) => pass,
            Err(cause) => return Ok(failed(cause)),
        };
        events.pass(trigger, &pass.report)?;
        attempts += 1;
        match pass.result {
            // Row W17: not ready yet. The same pass again, uncounted, until
            // its attempts are spent; then the cause ends the run.
            PassResult::Failed(Some(cause))
                if preparing(cause) && attempts < PREPARING_ATTEMPTS =>
            {
                trigger = Trigger::Preparing;
                continue;
            }
            _ => {}
        }
        attempts = 0;
        passes += 1;
        match pass.result {
            PassResult::Failed(Some(cause)) => return Ok(failed(cause)),
            PassResult::Failed(None) => return Ok(end(EndReason::PassFailed, None)),
            _ if passes >= max_passes.get() => return Ok(end(EndReason::PassesExhausted, None)),
            PassResult::Incomplete => {
                trigger = Trigger::Incomplete;
                continue;
            }
            PassResult::Complete => {}
        }
        match session.wait() {
            Ok(Wait::Hint { during_pass }) => {
                events.hint(during_pass)?;
                trigger = Trigger::Hint;
            }
            Ok(Wait::Ended(reason)) => return Ok(end(EndReason::WatchEnded(reason), None)),
            Err(GatewayError::TimedOut) => return Ok(end(EndReason::Idle, None)),
            Err(cause) => return Ok(failed(cause)),
        }
    }
}

fn end(reason: EndReason, cause: Option<GatewayError>) -> End {
    End { reason, cause }
}

fn failed(cause: GatewayError) -> End {
    let reason = match cause {
        GatewayError::Closed(_) => EndReason::ConnectionClosed,
        error if asks_status(error) => EndReason::Unauthorized,
        _ => EndReason::Unavailable,
    };
    end(reason, Some(cause))
}

#[cfg(test)]
#[path = "../../../tests/read_only_sync/application/watch.rs"]
mod tests;
