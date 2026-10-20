//! The agent's `artifacts_fetch` tool (issue #701, slice D3b): a file the
//! conversation holds, placed in the workspace where the conversation's
//! agent runs, under the agent's own Live lease, for the person whose turn
//! asked and only while that person's conversation holds exactly that file.
//!
//! ```text
//! fetch_artifact(conversation, call, stop)
//!   ──▶ the running turn and its Live agent lease? ── no ──▶ no_turn
//!   ──▶ `environment` is where the agent runs? ── no ──▶ environment_not_leased
//!   ──▶ digest, media type, size and destination name one file and one
//!       place? ── no ──▶ invalid
//!   ──▶ LeaseHold::fetch ── none here ──▶ fetch_unavailable
//!   ──▶ FetchRequests::take(lease, call, size) ── under way ──▶ duplicate_request
//!                                              ── past the budget ──▶ budget_exceeded
//!   ──▶ FetchAsked{lease, call, file, destination, the turn's person}
//!       while the lease is the latest, Live, with room ── not ──▶ no_turn
//!       ── as many fetches under way as a lease may have ──▶ busy
//!       ── unsaved ──▶ audit_unavailable, the environment never asked
//!   ──▶ its first byte read for the call's person ── not held ──▶ not_held
//!   ──▶ the environment takes it, its bytes are written range by range,
//!       each read again from the hold ──▶ placed | alreadyPlaced | refused
//!   ──▶ within `fetch_wait`, else timed_out; stop ──▶ cancelled; with no
//!       answer, either says whether every byte was delivered (`delivered`)
//!   ──▶ FetchRequests::finish: its size given back unless the answer says it
//!       is placed, or there is no answer and every byte was delivered
//!   ──▶ FetchEnded{lease, call, outcome}, for every FetchAsked, whatever
//!       its answer, `delivered` with it ── unsaved ──▶ answered
//!       `recorded: false`, a placement or a refusal alike
//! ```
//!
//! Arrows are steps, in order. Nothing is asked of the environment until
//! the fetch is recorded asked, naming the person it is for, and the file's
//! bytes were read once for them; every range is read again for them, so a
//! file the conversation lets go of while it is written stops there
//! (`not_held`) and nothing is placed. Stopped before every byte is written,
//! the environment is told so; stopped after, it may still place the file,
//! and its size stays counted against the lease.
//!
//! A refusal decided before the fetch is recorded asked is answered with no
//! record, as a command not yet decided is. Every fetch recorded asked is
//! recorded ended once, with what it came to, while its lease is still the
//! conversation's latest: timed out and cancelled too, which `fetch_artifact`
//! records after the wait or the stop dropped the fetch. An end with no
//! answer (`unanswered`, `timed_out`, `cancelled`) carries whether every byte
//! was delivered, the one fact that says whether it may yet be placed: not
//! delivered, nothing is, the environment perhaps never asked at all.
use super::{ConversationService, LiveConversation};
use crate::conversation::application::ArtifactBudget;
use crate::conversation::application::{
    ConversationAttachments, EnvironmentFuture, FetchAsk, FetchFailure, FetchPlaced, FetchRequests,
    FetchTaken, HeldRange, HeldRangeError, HeldSource, HeldSourceFailure,
};
use nessa_auth::domain::OrganizationId;
use nessa_protocol::conversation::domain::ConversationId;
use nessa_protocol::lease::FetchRefusal;
use nessa_sdk::application::agent_execution::sessions::{
    CurrentLease, EnvironmentFetchRefusal, EnvironmentPath, FetchOutcome, FetchRefusalReason,
    LeaseRecord, PublishedFile,
};
use nessa_sdk::domain::agent_execution::leases::{LeaseId, LeasePhase};
use nessa_sdk::domain::common::value_objects::{MediaType, Sha256Digest};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, MutexGuard, PoisonError,
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
        /// Whether its end is recorded: false when it could not be saved,
        /// even once more. The end stays retained and goes with the
        /// conversation's next save, but the records do not hold it yet.
        recorded: bool,
    },
    /// It is not placed, or not known to be, with why.
    Refused(FetchCallRefusal),
    /// As [`Self::Refused`], for a fetch recorded asked whose end could not
    /// be saved, even once more: the end stays retained and goes with the
    /// conversation's next save, but the records do not hold it yet.
    RefusedUnrecorded(FetchCallRefusal),
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
    /// The running turn's Drive grant does not allow `artifacts_fetch`
    /// (row J28), or its decision could not be recorded in the Drive tool
    /// audit. Nothing was placed.
    NotAllowed,
    /// Where the agent runs takes no files: this gateway's own machine.
    FetchUnavailable,
    /// The digest, media type or size do not name a file one could hold.
    Invalid,
    /// The conversation does not hold exactly that file for the call's
    /// person, or let go of it while it was written. Nothing is placed.
    NotHeld,
    /// It would take the lease past what its fetches may write.
    BudgetExceeded,
    /// The lease has as many fetches under way as it may have at once. The
    /// environment is not asked.
    Busy,
    /// The fetch could not be recorded asked, so the environment was never
    /// asked. Nothing is placed.
    AuditUnavailable,
    /// The environment did not place it, with its reason.
    NotPlaced(FetchRefusal),
    /// The file's bytes could not be read here. Nothing is placed.
    SourceUnavailable,
    /// The bytes could not be written to the environment. Nothing is placed.
    ChannelUnavailable,
    /// No answer came. Only when every byte was `delivered` first may it be
    /// placed; otherwise nothing is.
    Unanswered {
        /// Whether every byte was delivered to the environment.
        delivered: bool,
    },
    /// Not answered within the call's wait. Only when every byte was
    /// `delivered` first may it still be placed; otherwise nothing is,
    /// the environment perhaps never asked.
    TimedOut {
        /// Whether every byte was delivered to the environment.
        delivered: bool,
    },
    /// The call was cancelled while it waited. Only when every byte was
    /// `delivered` first may it still be placed; otherwise nothing is,
    /// the environment perhaps never asked.
    Cancelled {
        /// Whether every byte was delivered to the environment.
        delivered: bool,
    },
}

impl FetchCallRefusal {
    /// Whether this answer says the call placed nothing: every refusal
    /// does, except one with no answer after every byte was delivered.
    fn placed_nothing(self) -> bool {
        match self {
            Self::Unanswered { delivered }
            | Self::TimedOut { delivered }
            | Self::Cancelled { delivered } => !delivered,
            _ => true,
        }
    }
}

/// Why a fetch's call stopped waiting before its fetch answered.
#[derive(Clone, Copy)]
enum Gone {
    TimedOut,
    Cancelled,
}

impl ConversationService {
    /// Place `call`'s file in conversation `id`'s environment, under its Live
    /// agent lease, for its running turn's person, and wait at most this
    /// gateway's fetch wait for it to be placed. `stop`, once true, stops
    /// the wait as its caller gone. Every step that can wait, the stores'
    /// reads included, is inside both: none holds the call past either.
    /// Only recording the end of a fetch recorded asked follows them, as a
    /// command's end follows its run.
    pub async fn fetch_artifact(
        &self,
        id: &ConversationId,
        call: FetchCall,
        mut stop: watch::Receiver<bool>,
    ) -> FetchAnswer {
        let wait = self.inner.environment.fetch_wait();
        let asked = Mutex::new(None);
        let delivered = Arc::new(AtomicBool::new(false));
        let answered = tokio::time::timeout(wait, async {
            // An answer already there is taken before the stop, as the
            // publish path takes it, so a stop never hides an outcome the
            // host already gave.
            tokio::select! {
                biased;
                answer = self.fetch_within(id, call, &asked, &delivered) => Ok(answer),
                () = cancelled(&mut stop) => Err(Gone::Cancelled),
            }
        })
        .await
        .unwrap_or(Err(Gone::TimedOut));
        // The fetch is answered or dropped by now, so whether every byte was
        // delivered is final: it was set, if ever, before `Delivered` was
        // queued, and a fetch dropped before then told the environment so.
        let delivered = delivered.load(Ordering::SeqCst);
        let gone = answered.as_ref().err().copied();
        let answer = answered.unwrap_or_else(|gone| {
            FetchAnswer::Refused(match gone {
                Gone::TimedOut => FetchCallRefusal::TimedOut { delivered },
                Gone::Cancelled => FetchCallRefusal::Cancelled { delivered },
            })
        });
        let asked = asked.into_inner().unwrap_or_else(PoisonError::into_inner);
        let Some(asked) = asked else {
            return answer;
        };
        // An outcome is set once the fetch answered; without one, the wait
        // or the stop dropped it, and `gone` says which.
        let outcome = match (&asked.outcome, gone) {
            (Some(outcome), _) => outcome.clone(),
            (None, Some(Gone::TimedOut)) => {
                FetchOutcome::Refused(FetchRefusalReason::TimedOut { delivered })
            }
            (None, Some(Gone::Cancelled)) => {
                FetchOutcome::Refused(FetchRefusalReason::Cancelled { delivered })
            }
            // Answered with no outcome set: no answer this call can record.
            (None, None) => FetchOutcome::Refused(FetchRefusalReason::Unanswered { delivered }),
        };
        let recorded = self.end_fetch(&asked, outcome).await;
        match answer {
            FetchAnswer::Placed {
                already_placed,
                path,
                digest,
                size,
                ..
            } => FetchAnswer::Placed {
                already_placed,
                path,
                digest,
                size,
                recorded,
            },
            FetchAnswer::Refused(refusal) if !recorded => FetchAnswer::RefusedUnrecorded(refusal),
            refused => refused,
        }
    }

    /// [`Self::fetch_artifact`] without its wait or its stop, which drop
    /// this future: a call reserved by then is let go by [`Taken`]'s drop.
    /// `asked` is set once the fetch is recorded asked, and its outcome
    /// once the fetch answered, so its end is recorded however it ends. A
    /// placement is answered `recorded: false` here: whether its end is
    /// recorded is for `fetch_artifact` to say.
    async fn fetch_within(
        &self,
        id: &ConversationId,
        call: FetchCall,
        asked: &Mutex<Option<Asked>>,
        delivered: &Arc<AtomicBool>,
    ) -> FetchAnswer {
        let refused = FetchAnswer::Refused;
        let Some(live) = self.live(id).await else {
            return refused(FetchCallRefusal::NoTurn);
        };
        let Some(snapshot) = live.agent.session_manager().snapshot().await else {
            return refused(FetchCallRefusal::NoTurn);
        };
        let Some((lease, turn, actor, initiation)) =
            super::environment_tools::running_turn(&snapshot, &call.call)
        else {
            return refused(FetchCallRefusal::NoTurn);
        };
        if &lease != live.lease.id() || !live.lease.is_live() {
            return refused(FetchCallRefusal::NoTurn);
        }
        let checked = super::environment_tools::GatewayToolCall {
            conversation: id,
            turn: &turn,
            actor: &actor,
            initiation: &initiation,
            tool: "artifacts_fetch",
        };
        if let Some(reason) =
            super::environment_tools::drive_refusal(&self.inner.drive, &live, checked).await
        {
            tracing::warn!(conversation_id = %id, %reason, "a Drive grant's tool call was refused");
            return refused(FetchCallRefusal::NotAllowed);
        }
        // Where the agent runs now is the only lease it holds, compared as
        // `artifacts_publish` compares it.
        let placed = match self.inner.environment.runs_on(id).await {
            Ok(placed) => placed,
            Err(_) => return refused(FetchCallRefusal::EnvironmentNotLeased),
        };
        if call.environment != placed.as_deref().unwrap_or(super::HERE) {
            return refused(FetchCallRefusal::EnvironmentNotLeased);
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
        let Ok(destination) = EnvironmentPath::new(call.destination.as_str()) else {
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
        let ask = FetchAsk {
            request: call.call.clone(),
            destination: call.destination,
            file: file.clone(),
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
            lease: lease.clone(),
            call: call.call.clone(),
            size,
            delivered: delivered.clone(),
            placed_nothing: None,
        };
        // Recorded asked, for the turn's person, before anything is asked of
        // the environment: `fetching` is not yet polled, so no frame is
        // queued.
        let owed = Asked {
            live: live.clone(),
            conversation: id.clone(),
            lease: lease.clone(),
            request: call.call.clone(),
            outcome: None,
        };
        let record = LeaseRecord::FetchAsked {
            lease,
            request: call.call.clone(),
            file,
            destination,
            actor,
        };
        let answer = match self.ask_fetch(record, owed, asked).await {
            Err(refusal) => refused(refusal),
            Ok(()) => {
                // Read once for the call's person before the environment is
                // asked.
                let fetched = match source.read(0, 1).await {
                    Err(failure) => Err(source_failure(failure)),
                    Ok(_) => fetching.await,
                };
                // Answered, whether every byte was delivered is final.
                let delivered = delivered.load(Ordering::SeqCst);
                let (answer, outcome) =
                    answered(id, &call.call, &call.digest, size, fetched, delivered);
                set_outcome(asked, outcome);
                answer
            }
        };
        taken.placed_nothing = match &answer {
            FetchAnswer::Placed { already_placed, .. } => Some(*already_placed),
            FetchAnswer::Refused(refusal) | FetchAnswer::RefusedUnrecorded(refusal) => {
                Some(refusal.placed_nothing())
            }
        };
        answer
    }

    /// Record `record`, a fetch's asking, while its lease is the
    /// conversation's latest, Live, with fewer than
    /// [`CurrentLease::MAX_ASKED_FETCHES`] fetches under way, and set
    /// `asked` to `owed` the moment it is folded, so its end is owed from
    /// then on whatever drops this call. Refused [`FetchCallRefusal::NoTurn`]
    /// once the lease is not, [`FetchCallRefusal::Busy`] when it has no room,
    /// and [`FetchCallRefusal::AuditUnavailable`] when the record could not
    /// be saved; folded but unsaved, its end is recorded beside it, so the
    /// lease never counts a fetch that was never asked.
    async fn ask_fetch(
        &self,
        record: LeaseRecord,
        owed: Asked,
        asked: &Mutex<Option<Asked>>,
    ) -> Result<(), FetchCallRefusal> {
        let live = owed.live.clone();
        let id = owed.conversation.clone();
        let committed = live
            .agent
            .session_manager()
            .record_lease(|current| {
                let live_lease = current.filter(|current| {
                    current.held().is_some_and(|held| {
                        held.id() == &owed.lease && held.phase() == LeasePhase::Live
                    })
                });
                match live_lease {
                    None => (Vec::new(), Asking::NotLive),
                    Some(current) if current.asked_fetches() >= CurrentLease::MAX_ASKED_FETCHES => {
                        (Vec::new(), Asking::Busy)
                    }
                    Some(_) => {
                        *lock(asked) = Some(owed);
                        (vec![record], Asking::Asked)
                    }
                }
            })
            .await;
        match committed {
            Ok(commit) if commit.decided == Asking::NotLive => Err(FetchCallRefusal::NoTurn),
            Ok(commit) if commit.decided == Asking::Busy => Err(FetchCallRefusal::Busy),
            Ok(commit) => match commit.saved {
                Ok(()) => Ok(()),
                Err(error) => {
                    tracing::error!(%error, conversation_id = %id, "a fetch could not be recorded asked; the environment is not asked");
                    set_outcome(
                        asked,
                        FetchOutcome::Refused(FetchRefusalReason::AuditUnavailable),
                    );
                    Err(FetchCallRefusal::AuditUnavailable)
                }
            },
            // Refused before anything was retained: no end is owed.
            Err(error) => {
                lock(asked).take();
                tracing::error!(%error, conversation_id = %id, "a fetch could not be recorded asked; the environment is not asked");
                Err(FetchCallRefusal::AuditUnavailable)
            }
        }
    }

    /// Record the end of `asked` as `outcome`, while its lease is still the
    /// conversation's latest; whether the records hold it. An unsaved end is
    /// saved once more. Once a later lease replaced it, the stream's lease is
    /// that later one and cannot take it, and the end is logged instead.
    async fn end_fetch(&self, asked: &Asked, outcome: FetchOutcome) -> bool {
        let manager = asked.live.agent.session_manager();
        let refused = match &outcome {
            FetchOutcome::Refused(reason) => Some(*reason),
            FetchOutcome::Placed { .. } | FetchOutcome::AlreadyPlaced { .. } => None,
        };
        let ended = LeaseRecord::FetchEnded {
            lease: asked.lease.clone(),
            request: asked.request.clone(),
            outcome,
        };
        let committed = manager
            .record_lease(|current| {
                if current
                    .and_then(CurrentLease::held)
                    .is_some_and(|held| held.id() == &asked.lease)
                {
                    (vec![ended], true)
                } else {
                    (Vec::new(), false)
                }
            })
            .await;
        let recorded = match &committed {
            Ok(commit) if !commit.decided => false,
            Ok(commit) if commit.saved.is_ok() => true,
            Ok(_) => manager.save_retained_lease_records().await.is_ok(),
            Err(_) => false,
        };
        if !recorded {
            tracing::error!(
                conversation_id = %asked.conversation,
                lease = asked.lease.as_str(),
                request = asked.request.as_str(),
                refused = ?refused,
                replaced = matches!(&committed, Ok(commit) if !commit.decided),
                error = ?committed.err(),
                "a fetch's end could not be recorded"
            );
        }
        recorded
    }
}

/// A fetch recorded asked, whose end is owed.
struct Asked {
    live: Arc<LiveConversation>,
    conversation: ConversationId,
    lease: LeaseId,
    request: String,
    /// What it came to, once it answered.
    outcome: Option<FetchOutcome>,
}

/// What recording a fetch's asking came to, under the lock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Asking {
    Asked,
    /// Its lease has as many fetches under way as it may.
    Busy,
    /// Its lease is no longer the latest, or no longer Live.
    NotLive,
}

fn lock(asked: &Mutex<Option<Asked>>) -> MutexGuard<'_, Option<Asked>> {
    asked.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Say what the fetch `asked` names came to, when one is owed.
fn set_outcome(asked: &Mutex<Option<Asked>>, outcome: FetchOutcome) {
    if let Some(asked) = lock(asked).as_mut() {
        asked.outcome = Some(outcome);
    }
}

/// The answer to a fetch the environment was asked for, and the outcome its
/// end records, `delivered` saying whether every byte reached it. A path no
/// fetch could name is no answer this gateway can read: unanswered.
fn answered(
    id: &ConversationId,
    request: &str,
    digest: &str,
    size: u64,
    fetched: Result<FetchPlaced, FetchFailure>,
    delivered: bool,
) -> (FetchAnswer, FetchOutcome) {
    let failure = match fetched {
        Err(failure) => failure,
        Ok(placed) => {
            let (already_placed, path) = match placed {
                FetchPlaced::Placed { path } => (false, path),
                FetchPlaced::AlreadyPlaced { path } => (true, path),
            };
            match EnvironmentPath::new(path.as_str()) {
                Ok(recorded) => {
                    let outcome = if already_placed {
                        FetchOutcome::AlreadyPlaced { path: recorded }
                    } else {
                        FetchOutcome::Placed { path: recorded }
                    };
                    let answer = FetchAnswer::Placed {
                        already_placed,
                        path,
                        digest: digest.to_owned(),
                        size,
                        recorded: false,
                    };
                    return (answer, outcome);
                }
                Err(_) => {
                    tracing::warn!(
                        conversation_id = %id,
                        request,
                        path_bytes = path.len(),
                        "an environment placed a fetch at a path no fetch can name; it is answered unanswered"
                    );
                    FetchFailure::Unanswered
                }
            }
        }
    };
    (
        FetchAnswer::Refused(failure_refusal(failure, delivered)),
        FetchOutcome::Refused(failure_reason(failure, delivered)),
    )
}
/// Resolves once `stop` says the caller is gone, or can no longer say.
async fn cancelled(stop: &mut watch::Receiver<bool>) {
    while !*stop.borrow_and_update() {
        if stop.changed().await.is_err() {
            return;
        }
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

fn source_failure(failure: HeldSourceFailure) -> FetchFailure {
    match failure {
        HeldSourceFailure::NotHeld => FetchFailure::NotHeld,
        HeldSourceFailure::Unavailable => FetchFailure::SourceUnavailable,
    }
}

fn failure_refusal(failure: FetchFailure, delivered: bool) -> FetchCallRefusal {
    match failure {
        FetchFailure::Refused(reason) => FetchCallRefusal::NotPlaced(reason),
        FetchFailure::NotHeld => FetchCallRefusal::NotHeld,
        FetchFailure::SourceUnavailable => FetchCallRefusal::SourceUnavailable,
        FetchFailure::ChannelUnavailable => FetchCallRefusal::ChannelUnavailable,
        FetchFailure::Unanswered => FetchCallRefusal::Unanswered { delivered },
    }
}

/// What a fetch's end records for `failure`.
fn failure_reason(failure: FetchFailure, delivered: bool) -> FetchRefusalReason {
    match failure {
        FetchFailure::Refused(reason) => {
            FetchRefusalReason::Environment(environment_refusal(reason))
        }
        FetchFailure::NotHeld => FetchRefusalReason::NotHeld,
        FetchFailure::SourceUnavailable => FetchRefusalReason::SourceUnavailable,
        FetchFailure::ChannelUnavailable => FetchRefusalReason::ChannelUnavailable,
        FetchFailure::Unanswered => FetchRefusalReason::Unanswered { delivered },
    }
}

/// The environment's reason, as a fetch's end records it.
fn environment_refusal(reason: FetchRefusal) -> EnvironmentFetchRefusal {
    match reason {
        FetchRefusal::Unavailable => EnvironmentFetchRefusal::Unavailable,
        FetchRefusal::OutsideWorkspace => EnvironmentFetchRefusal::OutsideWorkspace,
        FetchRefusal::NoDirectory => EnvironmentFetchRefusal::NoDirectory,
        FetchRefusal::Occupied => EnvironmentFetchRefusal::Occupied,
        FetchRefusal::Size => EnvironmentFetchRefusal::Size,
        FetchRefusal::Invalid => EnvironmentFetchRefusal::Invalid,
        FetchRefusal::Busy => EnvironmentFetchRefusal::Busy,
        FetchRefusal::Mismatch => EnvironmentFetchRefusal::Mismatch,
        FetchRefusal::PlacingFailed => EnvironmentFetchRefusal::PlacingFailed,
        FetchRefusal::AuditUnavailable => EnvironmentFetchRefusal::AuditUnavailable,
        FetchRefusal::LeaseEnded => EnvironmentFetchRefusal::LeaseEnded,
    }
}
