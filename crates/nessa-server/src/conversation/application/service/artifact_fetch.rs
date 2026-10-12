//! The agent's `artifacts_fetch` tool (issue #701, slice D3b): a file the
//! conversation holds, placed in the workspace where the conversation's
//! agent runs, under the agent's own Live lease, for the person whose turn
//! asked and only while that person's conversation holds exactly that file.
//!
//! ```text
//! fetch_artifact(conversation, call, stop)
//!   ──▶ the running turn and its Live agent lease? ── no ──▶ no_turn
//!   ──▶ `environment` is where the agent runs? ── no ──▶ environment_not_leased
//!   ──▶ digest, media type and size name one file? ── no ──▶ invalid
//!   ──▶ LeaseHold::fetch ── none here ──▶ fetch_unavailable
//!   ──▶ FetchRequests::take(lease, call, size) ── under way ──▶ duplicate_request
//!                                              ── past the budget ──▶ budget_exceeded
//!   ──▶ its first byte read for the call's person ── not held ──▶ not_held
//!   ──▶ the environment takes it, its bytes are written range by range,
//!       each read again from the hold ──▶ placed | alreadyPlaced | refused
//!   ──▶ within `fetch_wait`, else timed_out; stop ──▶ cancelled
//!   ──▶ FetchRequests::finish: its size given back unless the answer says it
//!       is placed, or there is no answer and every byte was delivered
//! ```
//!
//! Arrows are steps, in order. Nothing is asked of the environment until
//! the file's bytes were read once for the call's person, and every range
//! is read again for them, so a file the conversation lets go of while it
//! is written stops there (`not_held`) and nothing is placed. Stopped before
//! every byte is written, the environment is told so; stopped after, it may
//! still place the file, and its size stays counted against the lease.
use super::ConversationService;
use crate::conversation::application::ArtifactBudget;
use crate::conversation::application::{
    ConversationAttachments, EnvironmentFuture, FetchAsk, FetchFailure, FetchPlaced, FetchRequests,
    FetchTaken, HeldRange, HeldRangeError, HeldSource, HeldSourceFailure,
};
use nessa_auth::domain::OrganizationId;
use nessa_protocol::conversation::domain::ConversationId;
use nessa_protocol::lease::FetchRefusal;
use nessa_sdk::application::agent_execution::sessions::PublishedFile;
use nessa_sdk::domain::agent_execution::leases::LeaseId;
use nessa_sdk::domain::common::value_objects::{MediaType, Sha256Digest};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;
use tokio::sync::{watch, Semaphore};

/// Held-file ranges `artifacts_fetch` reads at once across the gateway,
/// apart from `server.artifact_reads`, so fetches never lock a surface out
/// of reading. Row `server.fetch_reads` in `docs/limits.md`.
pub const FETCH_READS: usize = 2;

/// How long a fetch's range waits before it asks again after the store
/// answered busy, one step per busy answer in a row, the last repeated:
/// it never rejoins the store's queue at once. Row
/// `artifact.fetch_busy_backoff` in `docs/limits.md`.
pub const FETCH_BUSY_BACKOFF: [Duration; 3] = [
    Duration::from_millis(250),
    Duration::from_millis(500),
    Duration::from_millis(1000),
];

/// One `artifacts_fetch` call.
#[derive(Clone, Debug)]
pub struct FetchCall {
    /// Where: must name where this conversation's agent runs now.
    pub environment: String,
    /// Which file: its SHA-256, lowercase hex.
    pub digest: String,
    /// Its media type.
    pub media_type: String,
    /// Its length.
    pub size: u64,
    /// Where it goes: absolute, or relative to that environment's workspace.
    pub destination: String,
    /// The tool call's identity.
    pub call: String,
}

/// What an `artifacts_fetch` came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FetchAnswer {
    /// The file is at `path` in the environment: placed now, or already
    /// exactly there.
    Placed {
        /// Whether it was already there, so nothing was written.
        already_placed: bool,
        /// Where it is, as the environment resolved it.
        path: String,
        /// Its SHA-256, lowercase hex.
        digest: String,
        /// Its length.
        size: u64,
    },
    /// It is not placed, or not known to be, with why.
    Refused(FetchCallRefusal),
}

/// Why an `artifacts_fetch` did not place its file, or does not know.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FetchCallRefusal {
    /// No turn of this conversation is running here under a Live agent
    /// lease: nobody it could be fetched for.
    NoTurn,
    /// `environment` is not where this conversation's agent runs, so the
    /// agent holds no lease there to fetch under.
    EnvironmentNotLeased,
    /// A fetch for the same tool call is under way.
    DuplicateRequest,
    /// Where the agent runs takes no files: this gateway's own machine.
    FetchUnavailable,
    /// The digest, media type or size do not name a file one could hold.
    Invalid,
    /// The conversation does not hold exactly that file for the call's
    /// person, or let go of it while it was written. Nothing is placed.
    NotHeld,
    /// It would take the lease past what its fetches may write.
    BudgetExceeded,
    /// The environment did not place it, with its reason.
    NotPlaced(FetchRefusal),
    /// The file's bytes could not be read here. Nothing is placed.
    SourceUnavailable,
    /// The bytes could not be written to the environment. Nothing is placed.
    ChannelUnavailable,
    /// No answer came: placed only if every byte was delivered first.
    Unanswered,
    /// Not answered within the call's wait: it may still be placed once
    /// every byte was delivered.
    TimedOut,
    /// The call was cancelled while it waited: it may still be placed once
    /// every byte was delivered.
    Cancelled,
}

impl FetchCallRefusal {
    /// Whether this answer itself says the call placed nothing; `None` when
    /// it says nothing of it, and only whether every byte was delivered can.
    fn placed_nothing(self) -> Option<bool> {
        match self {
            Self::Unanswered | Self::TimedOut | Self::Cancelled => None,
            _ => Some(true),
        }
    }
}

impl ConversationService {
    /// Place `call`'s file in conversation `id`'s environment, under its Live
    /// agent lease, for its running turn's person, and wait at most this
    /// gateway's fetch wait for it to be placed. `stop`, once true, stops
    /// the wait as its caller gone.
    pub async fn fetch_artifact(
        &self,
        id: &ConversationId,
        call: FetchCall,
        mut stop: watch::Receiver<bool>,
    ) -> FetchAnswer {
        let refused = FetchAnswer::Refused;
        let Some(live) = self.live(id).await else {
            return refused(FetchCallRefusal::NoTurn);
        };
        let Some(snapshot) = live.agent.session_manager().snapshot().await else {
            return refused(FetchCallRefusal::NoTurn);
        };
        let Some((lease, _turn, actor)) =
            super::environment_tools::running_turn(&snapshot, &call.call)
        else {
            return refused(FetchCallRefusal::NoTurn);
        };
        if &lease != live.lease.id() || !live.lease.is_live() {
            return refused(FetchCallRefusal::NoTurn);
        }
        let environments = &self.inner.environment;
        // Where the agent runs now is the only lease it holds, compared as
        // `artifacts_publish` compares it.
        let placed = match environments.placement(id).await {
            Ok(placed) => placed,
            Err(_) => return refused(FetchCallRefusal::EnvironmentNotLeased),
        };
        if call.environment != placed.as_deref().unwrap_or(super::HERE) {
            return refused(FetchCallRefusal::EnvironmentNotLeased);
        }
        if *stop.borrow() {
            return refused(FetchCallRefusal::Cancelled);
        }
        let Some(file) = Sha256Digest::parse(&format!("sha256:{}", call.digest))
            .ok()
            .zip(MediaType::parse(&call.media_type).ok())
            .and_then(|(digest, media_type)| {
                PublishedFile::new(digest, media_type, call.size).ok()
            })
        else {
            return refused(FetchCallRefusal::Invalid);
        };
        let source = Arc::new(HeldFile {
            attachments: self.inner.attachments.clone(),
            reads: self.inner.fetch_reads.clone(),
            organization_id: live.organization_id.clone(),
            principal_id: actor.principal_id().to_owned(),
            conversation_id: id.clone(),
            file: file.clone(),
        });
        let delivered = Arc::new(AtomicBool::new(false));
        let ask = FetchAsk {
            request: call.call.clone(),
            destination: call.destination,
            file,
            source: source.clone(),
            delivered: delivered.clone(),
        };
        let Some(fetching) = live.lease.fetch(ask) else {
            return refused(FetchCallRefusal::FetchUnavailable);
        };
        let size = call.size;
        let taken = live
            .fetches
            .take(&lease, &call.call, size, ArtifactBudget::LEASE.fetch_bytes);
        if let Err(taken) = taken {
            return refused(match taken {
                FetchTaken::Duplicate => FetchCallRefusal::DuplicateRequest,
                FetchTaken::OverBudget => FetchCallRefusal::BudgetExceeded,
            });
        }
        // However this call ends, its call is let go; ended with no word on
        // it, what it reserved is given back unless every byte was
        // delivered, the one point after which the host may place it.
        let mut taken = Taken {
            requests: live.fetches.clone(),
            lease,
            call: call.call.clone(),
            size,
            delivered,
            placed_nothing: None,
        };
        // Read once for the call's person before the environment is asked.
        if let Err(failure) = source.read(0, 1).await {
            return refused(source_refusal(failure));
        }
        let answer = tokio::time::timeout(environments.fetch_wait(), async {
            tokio::pin!(fetching);
            let fetched = loop {
                tokio::select! {
                    biased;
                    fetched = &mut fetching => break fetched,
                    changed = stop.changed() => {
                        if changed.is_err() || *stop.borrow() {
                            return refused(FetchCallRefusal::Cancelled);
                        }
                    }
                }
            };
            fetched.map_or_else(
                |failure| refused(failure_refusal(failure)),
                |placed| {
                    let (already_placed, path) = match placed {
                        FetchPlaced::Placed { path } => (false, path),
                        FetchPlaced::AlreadyPlaced { path } => (true, path),
                    };
                    FetchAnswer::Placed {
                        already_placed,
                        path,
                        digest: call.digest.clone(),
                        size,
                    }
                },
            )
        })
        .await
        .unwrap_or(refused(FetchCallRefusal::TimedOut));
        taken.placed_nothing = match &answer {
            FetchAnswer::Placed { already_placed, .. } => Some(*already_placed),
            FetchAnswer::Refused(refusal) => refusal.placed_nothing(),
        };
        answer
    }
}

/// One fetch's call, let go however the call ends.
struct Taken {
    requests: Arc<FetchRequests>,
    lease: LeaseId,
    call: String,
    size: u64,
    /// Set once the environment may place it.
    delivered: Arc<AtomicBool>,
    /// What its answer said of it, when it said.
    placed_nothing: Option<bool>,
}

impl Drop for Taken {
    fn drop(&mut self) {
        let placed_nothing = self
            .placed_nothing
            .unwrap_or_else(|| !self.delivered.load(Ordering::SeqCst));
        self.requests
            .finish(&self.lease, &self.call, self.size, placed_nothing);
    }
}

/// A held file's bytes, read for the person a fetch is for.
struct HeldFile {
    /// `None` on a gateway that keeps no files: it holds none to fetch.
    attachments: Option<Arc<dyn ConversationAttachments>>,
    /// The gateway's fetch read permits: one held for each range read.
    reads: Arc<Semaphore>,
    organization_id: OrganizationId,
    principal_id: String,
    conversation_id: ConversationId,
    file: PublishedFile,
}

impl HeldSource for HeldFile {
    fn read(
        &self,
        offset: u64,
        max: usize,
    ) -> EnvironmentFuture<'_, Result<Vec<u8>, HeldSourceFailure>> {
        Box::pin(async move {
            let Some(attachments) = &self.attachments else {
                return Err(HeldSourceFailure::NotHeld);
            };
            let mut busy = 0;
            loop {
                let range = HeldRange {
                    organization_id: self.organization_id.clone(),
                    principal_id: self.principal_id.clone(),
                    conversation_id: self.conversation_id.clone(),
                    file: self.file.clone(),
                    offset,
                    max_bytes: max,
                };
                let read = {
                    let Ok(_permit) = self.reads.acquire().await else {
                        return Err(HeldSourceFailure::Unavailable);
                    };
                    attachments.read_held(range).await
                };
                match read {
                    Ok(bytes) => return Ok(bytes),
                    // Asked again after a pause, its permit let go, for as
                    // long as the call waits.
                    Err(HeldRangeError::Busy) => {
                        let step = FETCH_BUSY_BACKOFF[busy.min(FETCH_BUSY_BACKOFF.len() - 1)];
                        busy += 1;
                        tokio::time::sleep(step).await;
                    }
                    Err(HeldRangeError::NotHeld) => return Err(HeldSourceFailure::NotHeld),
                    Err(HeldRangeError::Unavailable) => return Err(HeldSourceFailure::Unavailable),
                }
            }
        })
    }
}

fn source_refusal(failure: HeldSourceFailure) -> FetchCallRefusal {
    match failure {
        HeldSourceFailure::NotHeld => FetchCallRefusal::NotHeld,
        HeldSourceFailure::Unavailable => FetchCallRefusal::SourceUnavailable,
    }
}

fn failure_refusal(failure: FetchFailure) -> FetchCallRefusal {
    match failure {
        FetchFailure::Refused(reason) => FetchCallRefusal::NotPlaced(reason),
        FetchFailure::NotHeld => FetchCallRefusal::NotHeld,
        FetchFailure::SourceUnavailable => FetchCallRefusal::SourceUnavailable,
        FetchFailure::ChannelUnavailable => FetchCallRefusal::ChannelUnavailable,
        FetchFailure::Unanswered => FetchCallRefusal::Unanswered,
    }
}
