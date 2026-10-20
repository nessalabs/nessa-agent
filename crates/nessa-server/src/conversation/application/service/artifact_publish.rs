//! The agent's `artifacts_publish` tool (issue #701, slice D3a): a file in
//! the workspace where the conversation's agent runs, published under the
//! agent's own Live lease for the person whose turn asked, and kept by the
//! same collector, budgets and channel as a file a harness publishes.
//!
//! ```text
//! publish_artifact(conversation, call, stop)
//!   ──▶ the running turn and its Live agent lease? ── no ──▶ no_turn
//!   ──▶ `environment` is where the agent runs? ── no ──▶ environment_not_leased
//!   ──▶ PublishRequests::ask(lease, call) (the call's person) ── held ──▶ duplicate_request
//!   ──▶ LeaseHold::publish ── none here ──▶ publish_unavailable
//!         not published / refused / unanswered ──▶ that, and the ask let go
//!   ──▶ offered: ArtifactCollector::collect ──▶ the host answered ──▶ the waiter told
//!   ──▶ held | alreadyHeld | refused, within `publish_wait`, else timed_out
//! stop (the call cancelled) ──▶ only the waiter goes; who asked is kept for the offer
//! ```
//!
//! Arrows are steps, in order. The call's person is the actor its record
//! and hold name, with the tool call as their request; the agent's lease is
//! the cause, as for every file kept under it. Who asked is held before the
//! environment is asked, so the offer can never arrive before it.
use super::ConversationService;
use crate::conversation::application::{
    PublishAsk, PublishFailure, PublishRequests, PublishedOutcome,
};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_protocol::lease::{Collection, CollectionRefusal, PublishRefusal};
use nessa_sdk::domain::agent_execution::leases::LeaseId;
use std::sync::Arc;
use tokio::sync::{oneshot, watch};

/// One `artifacts_publish` call.
#[derive(Clone, Debug)]
pub struct PublishCall {
    /// Where: must name where this conversation's agent runs now.
    pub environment: String,
    /// The file: absolute, or relative to that environment's workspace.
    pub path: String,
    /// Its media type, when the agent names one.
    pub media_type: Option<String>,
    /// The tool call's identity, the request the file is published for.
    pub call: String,
}

/// What an `artifacts_publish` came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PublishAnswer {
    /// The conversation holds it now, or already held exactly this file.
    Kept {
        /// Whether it was read now, or already held.
        already_held: bool,
        /// Its name, as the environment published it.
        name: String,
        /// Its SHA-256, lowercase hex.
        digest: String,
        /// Its length.
        size: u64,
    },
    /// It is not kept, with why.
    Refused(PublishCallRefusal),
}

/// Why an `artifacts_publish` did not keep its file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublishCallRefusal {
    /// No turn of this conversation is running here under a Live agent
    /// lease: nobody it could be published for.
    NoTurn,
    /// `environment` is not where this conversation's agent runs, so the
    /// agent holds no lease there to publish under.
    EnvironmentNotLeased,
    /// A publish for the same tool call is already under way.
    DuplicateRequest,
    /// The running turn's Drive grant does not allow `artifacts_publish`
    /// (row J28), or its decision could not be recorded in the Drive tool
    /// audit. Nothing was published.
    NotAllowed,
    /// Where the agent runs publishes no files: this gateway's own machine.
    PublishUnavailable,
    /// The environment did not publish it, with its reason.
    NotPublished(PublishRefusal),
    /// The conversation did not keep it, with its reason.
    NotKept(CollectionRefusal),
    /// The connection to the environment was lost, or the lease no longer
    /// held there, before it said whether it staged the file.
    Unanswered,
    /// Not kept within the call's wait: it may still be kept, and is then
    /// recorded as any file is.
    TimedOut,
    /// The call was cancelled while it waited.
    Cancelled,
}

impl ConversationService {
    /// Publish `call`'s file for conversation `id`'s running turn, under
    /// its Live agent lease, and wait at most this gateway's publish wait
    /// for it to be kept. `stop`, once true, stops the wait as its caller
    /// gone; a file already asked for may still be kept, named for the
    /// call's person.
    pub async fn publish_artifact(
        &self,
        id: &ConversationId,
        call: PublishCall,
        mut stop: watch::Receiver<bool>,
    ) -> PublishAnswer {
        let refused = PublishAnswer::Refused;
        let Some(live) = self.live(id).await else {
            return refused(PublishCallRefusal::NoTurn);
        };
        let Some(snapshot) = live.agent.session_manager().snapshot().await else {
            return refused(PublishCallRefusal::NoTurn);
        };
        let Some((lease, turn, actor, initiation)) =
            super::environment_tools::running_turn(&snapshot, &call.call)
        else {
            return refused(PublishCallRefusal::NoTurn);
        };
        if &lease != live.lease.id() || !live.lease.is_live() {
            return refused(PublishCallRefusal::NoTurn);
        }
        let checked = super::environment_tools::GatewayToolCall {
            conversation: id,
            turn: &turn,
            actor: &actor,
            initiation: &initiation,
            tool: "artifacts_publish",
        };
        if let Some(reason) =
            super::environment_tools::drive_refusal(&self.inner.drive, &live, checked).await
        {
            tracing::warn!(conversation_id = %id, %reason, "a Drive grant's tool call was refused");
            return refused(PublishCallRefusal::NotAllowed);
        }
        let environments = &self.inner.environment;
        // Where the agent runs now is the only lease it holds; the name is
        // compared as `environments_list` gives it.
        let placed = match environments.runs_on(id).await {
            Ok(placed) => placed,
            Err(_) => return refused(PublishCallRefusal::EnvironmentNotLeased),
        };
        if call.environment != placed.as_deref().unwrap_or(super::HERE) {
            return refused(PublishCallRefusal::EnvironmentNotLeased);
        }
        if *stop.borrow() {
            return refused(PublishCallRefusal::Cancelled);
        }
        let requests = live.publishes.clone();
        // Held before the environment is asked: its offer cannot overtake it.
        let Some(outcome) = requests.ask(&lease, &call.call, turn, actor) else {
            return refused(PublishCallRefusal::DuplicateRequest);
        };
        let ask = PublishAsk {
            request: call.call.clone(),
            path: call.path,
            media_type: call.media_type,
        };
        let Some(publishing) = live.lease.publish(ask) else {
            requests.forget(&lease, &call.call);
            return refused(PublishCallRefusal::PublishUnavailable);
        };
        let wait = environments.publish_wait();
        let offered = offer(
            publishing,
            requests.clone(),
            lease.clone(),
            call.call.clone(),
        );
        let waited = tokio::time::timeout(wait, async {
            tokio::pin!(outcome);
            let mut offered = Some(offered);
            loop {
                tokio::select! {
                    biased;
                    kept = &mut outcome => match kept {
                        Ok(kept) => break kept_answer(kept),
                        // Let go unanswered: either nothing was offered,
                        // which the environment's answer says, or the
                        // conversation let the lease go first.
                        Err(_) => {
                            if let Some(offered) = offered.take() {
                                if let Ok(Err(failure)) = offered.await {
                                    break refused(failure_refusal(failure));
                                }
                            }
                            break refused(PublishCallRefusal::NotKept(CollectionRefusal::LeaseEnded));
                        }
                    },
                    failed = async { offered.as_mut().expect("polled while present").await },
                        if offered.is_some() =>
                    {
                        offered = None;
                        if let Ok(Err(failure)) = failed {
                            break refused(failure_refusal(failure));
                        }
                    }
                    changed = stop.changed() => {
                        if changed.is_err() || *stop.borrow() {
                            requests.abandon(&lease, &call.call);
                            break refused(PublishCallRefusal::Cancelled);
                        }
                    }
                }
            }
        })
        .await;
        waited.unwrap_or_else(|_| {
            requests.abandon(&lease, &call.call);
            refused(PublishCallRefusal::TimedOut)
        })
    }
}

/// Ask the environment on a task of its own, so an ask it answers after the
/// call stopped waiting still lets `request` go when nothing will be offered
/// for it, and keeps who asked for an offer that arrives late. The hold
/// answers by the time the lease is no longer held, which bounds the task.
/// Answers the environment's answer.
fn offer(
    publishing: crate::conversation::application::EnvironmentFuture<
        'static,
        Result<(), PublishFailure>,
    >,
    requests: Arc<PublishRequests>,
    lease: LeaseId,
    request: String,
) -> oneshot::Receiver<Result<(), PublishFailure>> {
    let (answer, answered) = oneshot::channel();
    tokio::spawn(async move {
        let result = publishing.await;
        if result.is_err() {
            requests.forget(&lease, &request);
        }
        let _ = answer.send(result);
    });
    answered
}

/// The collector's outcome as the call's answer.
fn kept_answer(kept: PublishedOutcome) -> PublishAnswer {
    let already_held = match kept.collection {
        Collection::Held => false,
        Collection::AlreadyHeld => true,
        Collection::Refused { reason } => {
            return PublishAnswer::Refused(PublishCallRefusal::NotKept(reason))
        }
    };
    PublishAnswer::Kept {
        already_held,
        name: kept.name,
        digest: kept.digest,
        size: kept.size,
    }
}

fn failure_refusal(failure: PublishFailure) -> PublishCallRefusal {
    match failure {
        PublishFailure::NotPublished(reason) => PublishCallRefusal::NotPublished(reason),
        PublishFailure::Refused(reason) => PublishCallRefusal::NotKept(reason),
        PublishFailure::Unanswered => PublishCallRefusal::Unanswered,
    }
}
