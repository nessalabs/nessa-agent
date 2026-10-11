//! How a machine paired as an environment learns that its owner approved it
//! (issue #706, part 1c): read what the pairing saved, ask the gateway while
//! it is pending, and decide from the answer whether to wait, go on, or
//! stop with a reason.
//!
//! ```text
//! saved? ──▶ approved ──▶ go on with it
//!        ──▶ nothing ──▶ stop: not paired (no gateway asked)
//!        ──▶ pending ──▶ ask ──▶ approved (now saved) ──▶ read it again
//!                            ──▶ waiting, or no answer ──▶ wait ──▶ saved?
//!                            ──▶ ended, not paired, unusable ──▶ stop
//! stop asked ──▶ none, before anything else that is ready
//! ```
//!
//! Arrows are steps, in order. The pairing is a port so every answer,
//! failures included, can be given by a test.
use super::redial::stopped;
use std::{future::Future, time::Duration};
use tokio::sync::watch;

/// What the pairing holds on this machine.
#[derive(Debug)]
pub(crate) enum Saved<A> {
    /// The owner approved it: what dialing needs.
    Approved(A),
    /// Enrolled, not yet approved.
    Pending,
    /// Nothing was ever paired here.
    Nothing,
}

/// The gateway's answer to "is this pairing approved?".
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Answer {
    /// Still waiting for the owner.
    Waiting,
    /// Approved; the pairing has saved what dialing needs.
    Approved,
    /// Denied, cancelled or expired; enrolling again needs a new code.
    Ended,
    /// The gateway holds no such pairing.
    NotPaired,
    /// A pairing this machine cannot use, or a gateway answering out of
    /// turn: no later ask changes it.
    Unusable(String),
    /// No answer this time (out of reach, busy, refusing, or a save that
    /// failed and keeps its pending record): asking again may answer.
    Unanswered(String),
}

/// Why waiting for approval stopped without it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum NotApproved {
    /// Nothing was paired here, or the gateway holds no such pairing.
    NotPaired,
    /// The gateway ended the pairing without approving it.
    Ended,
    /// The pairing cannot be used: the reason says what to do.
    Unusable(String),
}

/// This machine's pairing as an environment, read and asked about.
pub(crate) trait EnvironmentPairing: Send + Sync {
    /// What dialing needs once approved.
    type Approval;
    /// Read what the pairing holds; an error is a pairing that cannot be read.
    fn saved(&self) -> impl Future<Output = Result<Saved<Self::Approval>, String>> + Send;
    /// Ask the gateway about the pairing.
    fn ask(&self) -> impl Future<Output = Answer> + Send;
}

/// What `pairing` holds once the owner has approved it, asking every `poll`
/// until then; `None` when `stop` says true first, which wins over anything
/// else ready at the same moment.
///
/// # Errors
/// Why the pairing will not be approved.
pub(crate) async fn approval<P: EnvironmentPairing>(
    pairing: &P,
    poll: Duration,
    mut stop: watch::Receiver<bool>,
) -> Result<Option<P::Approval>, NotApproved> {
    let mut said = false;
    loop {
        if *stop.borrow_and_update() {
            return Ok(None);
        }
        match pairing.saved().await.map_err(NotApproved::Unusable)? {
            Saved::Approved(approval) => return Ok(Some(approval)),
            Saved::Nothing => return Err(NotApproved::NotPaired),
            Saved::Pending => {}
        }
        let answer = tokio::select! {
            biased;
            () = stopped(&mut stop) => return Ok(None),
            answer = pairing.ask() => answer,
        };
        match answer {
            // Saved before it was answered: read it at once.
            Answer::Approved => continue,
            Answer::Ended => return Err(NotApproved::Ended),
            Answer::NotPaired => return Err(NotApproved::NotPaired),
            Answer::Unusable(reason) => return Err(NotApproved::Unusable(reason)),
            Answer::Waiting if !said => {
                said = true;
                tracing::info!("waiting for the owner to approve this machine");
            }
            Answer::Waiting => {}
            Answer::Unanswered(reason) => {
                tracing::warn!(%reason, "the pairing's status was not read");
            }
        }
        tokio::select! {
            biased;
            () = stopped(&mut stop) => return Ok(None),
            () = tokio::time::sleep(poll) => {}
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/env_serve/approval.rs"]
mod tests;
