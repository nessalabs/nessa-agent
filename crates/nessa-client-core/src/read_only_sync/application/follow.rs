//! The bounded `follow` loop: subscribe to one conversation, run the recheck
//! pass from the durable checkpoint, then one pass per view whose cursor
//! moved, until the pass budget, an idle wait, or the subscription ends
//! (read-only sync example, "Following a conversation", rows W1–W17).
//!
//! ```text
//! subscribe --> recheck pass --> wait --> view (cursor moved) --> pass --> wait ...
//!     |              |            |
//!  refused        failed       idle / ended / closed
//!                    |
//!          source_preparing --> the same pass again (bounded)
//! ```
//! Arrows are calls in order on one connection, one operation at a time. The
//! session keeps views that arrive during a pass, so `wait` returns at once
//! when one is held. A view is what `conversation.read` would return now, so
//! one whose cursor did not move changed only live facts (title, agent
//! state) and no record: it runs no pass. This loop decides only what ends
//! the run; composition asks the pinned status again for an end whose cause
//! `asks_status` names (row PC5).
use super::{device::asks_status, GatewayAttempt, GatewayError};
use nessa_protocol::product_contract::generated::RecordReadErrorCode;
use std::num::NonZeroUsize;

/// Where the gateway's fold had read when it took a view: the committed
/// position in one stored history. Two views at the same cursor saw the same
/// records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ViewCursor {
    pub(crate) incarnation: String,
    pub(crate) position: String,
}

/// The reasons the product schema publishes for `conversation.subscriptionEnded`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SubscriptionEnd {
    Lagging,
    Refused,
    SourceClosed,
    TooLarge,
}
impl SubscriptionEnd {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Lagging => "lagging",
            Self::Refused => "refused",
            Self::SourceClosed => "source_closed",
            Self::TooLarge => "too_large",
        }
    }
}

/// How the gateway ended the subscription. `cause` is the typed refusal a
/// `refused` end carries, so an authority refusal asks the pinned status
/// again as a refused read does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Ended {
    pub(crate) reason: SubscriptionEnd,
    pub(crate) cause: Option<GatewayError>,
}

/// What one wait for a view found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Wait {
    /// The newest view: kept from an earlier operation (`during_pass`), or
    /// read while waiting. Older kept views are dropped: each view replaces
    /// the one before it.
    View {
        cursor: ViewCursor,
        during_pass: bool,
    },
    /// The gateway ended the subscription; no view follows.
    Ended(Ended),
}

/// Why a pass ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Trigger {
    /// The pass after subscribing, from the durable checkpoint.
    Recheck,
    /// A view whose cursor moved.
    View,
    /// The previous pass stopped at its page budget.
    Incomplete,
    /// The previous attempt was answered `source_preparing`: the same pass
    /// again, at once.
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

/// Discovery before subscribing: an attempt answered
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
            Some(cause) if preparing(cause) => {
                tracing::warn!(
                    limit = "client.preparing_attempts",
                    "product session hit an operational limit"
                );
                return Ok(discovery);
            }
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
pub(crate) struct FollowPass<R> {
    pub(crate) report: R,
    pub(crate) result: PassResult,
}

/// An admitted subscription: its identity, the connection operation that
/// opened it, and its first frame. The gateway answers a subscribe after its
/// first read and sends that view right after the reply, so the first frame
/// was taken before the recheck pass starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Subscribed {
    pub(crate) subscription: String,
    pub(crate) operation: u64,
    pub(crate) first: Wait,
}

/// The one connection, its cache and its finite driver, one operation per call.
pub(crate) trait FollowSession {
    type Report;
    /// Subscribe and take the first frame, in one operation.
    fn subscribe(&mut self) -> Result<Subscribed, GatewayError>;
    /// Under the ordinary operation deadline; `TimedOut` means nothing came.
    fn wait(&mut self) -> Result<Wait, GatewayError>;
    /// `Err` when the pass could not begin: nothing was read or saved, so it
    /// has no report.
    fn pass(&mut self) -> Result<FollowPass<Self::Report>, GatewayError>;
}

/// The standard output could not take a line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OutputLost;

/// Lines in arrival order. A refused line stops the loop.
pub(crate) trait FollowEvents<R> {
    fn subscribed(&mut self, subscribed: &Subscribed) -> Result<(), OutputLost>;
    fn pass(&mut self, trigger: Trigger, report: &R) -> Result<(), OutputLost>;
    /// A view whose cursor moved; a pass follows.
    fn view(&mut self, cursor: &ViewCursor, during_pass: bool) -> Result<(), OutputLost>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EndReason {
    PassesExhausted,
    Idle,
    SubscribeRefused,
    Unauthorized,
    Unavailable,
    PassFailed,
    SubscriptionEnded(SubscriptionEnd),
    /// The gateway closed the connection. On the native profile a close
    /// carries no reason, so this claims nothing about why.
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

/// Run the follow loop until one of the `EndReason`s. Every pass counts
/// toward `max_passes`, the recheck included.
pub(crate) fn follow<S: FollowSession, E: FollowEvents<S::Report>>(
    session: &mut S,
    events: &mut E,
    max_passes: NonZeroUsize,
) -> Result<End, OutputLost> {
    let subscribed = match session.subscribe() {
        Ok(subscribed) => subscribed,
        Err(cause @ GatewayError::Subscription(_)) => {
            return Ok(end(EndReason::SubscribeRefused, Some(cause)))
        }
        Err(cause) => return Ok(failed(cause)),
    };
    events.subscribed(&subscribed)?;
    // The recheck pass reads at least what the first view saw, so a later
    // view at the same cursor brings no record the cache lacks.
    let mut seen = match subscribed.first {
        Wait::View { cursor, .. } => cursor,
        Wait::Ended(ended) => return Ok(subscription_ended(ended)),
    };
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
            // Not ready yet. The same pass again, uncounted, until
            // its attempts are spent; then the cause ends the run.
            PassResult::Failed(Some(cause))
                if preparing(cause) && attempts < PREPARING_ATTEMPTS =>
            {
                trigger = Trigger::Preparing;
                continue;
            }
            PassResult::Failed(Some(cause)) if preparing(cause) => {
                tracing::warn!(
                    limit = "client.preparing_attempts",
                    "product session hit an operational limit"
                );
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
        loop {
            match session.wait() {
                Ok(Wait::View {
                    cursor,
                    during_pass,
                }) if cursor != seen => {
                    events.view(&cursor, during_pass)?;
                    seen = cursor;
                    trigger = Trigger::View;
                    break;
                }
                // Live facts changed, no record did: nothing to copy.
                Ok(Wait::View { .. }) => {}
                Ok(Wait::Ended(ended)) => return Ok(subscription_ended(ended)),
                Err(GatewayError::TimedOut) => return Ok(end(EndReason::Idle, None)),
                Err(cause) => return Ok(failed(cause)),
            }
        }
    }
}

fn subscription_ended(ended: Ended) -> End {
    end(EndReason::SubscriptionEnded(ended.reason), ended.cause)
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
#[path = "../../../tests/read_only_sync/application/follow.rs"]
mod tests;
