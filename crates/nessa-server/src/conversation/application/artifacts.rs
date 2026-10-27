//! Files an environment publishes under a lease, kept for the conversation
//! (issue #701; `docs/design/runtime-architecture.md`, "Artifact channel
//! bounds and resumption").
//!
//! ```text
//! LeaseHold::artifacts ──▶ ArtifactOffer { file, bytes, answer }, one at a time
//!   over the lease's budget? ──▶ refused budgetExceeded, nothing read
//!   no room for its record? ──▶ refused budgetExceeded, nothing read
//!   name or type unreadable? ──▶ refused invalid, nothing read
//!   ConversationAttachments::keep_published(file, bytes, record)
//!     the conversation already holds it ──▶ record ──▶ alreadyHeld, nothing read
//!     else bytes pulled ──▶ size and digest checked ──▶ hold audited, usable
//!       ──▶ record: SessionManager::record_artifact; refused ──▶ taken back
//!     interrupted, mismatched, lease ended, or not recorded ──▶ refused, no hold
//!   answer ──▶ the host lets go of its staged copy
//!   asked by the agent's tool call (its `request`) ──▶ that call's person is
//!     the record's actor; its waiter told the outcome after the answer
//! ```
//!
//! Arrows are steps, in order. Nothing is kept from bytes not checked
//! against the digest the host published, and a hold is made only once
//! its bytes are. A file is recorded only once held, and taken back when
//! the conversation will not record it: the two never disagree for longer
//! than that.
//! Every offer is answered, by its answer's drop if by nothing else.
//!
//! A file the agent asked for with `artifacts_publish` is offered here the
//! same way, on the same receiver, and decided by the same budgets: what
//! differs is who asked. Its offer names the tool call ([`ArtifactOffer::request`]),
//! and the call's person, held in [`PublishRequests`] from before the
//! environment was asked, is the actor its record and hold name; the lease
//! stays the cause. An offer naming a request nobody here asked is refused
//! unread: nobody would be named as having asked for it.
use super::{
    environment::LiveLease, ArtifactKept, ArtifactRecordGate, ConversationAttachments,
    PublishedArtifact,
};
use nessa_auth::domain::OrganizationId;
use nessa_protocol::{
    conversation::domain::ConversationId,
    lease::{Collection, CollectionRefusal, StagedArtifact, MAX_ARTIFACT_PATH_BYTES},
};
use nessa_sdk::application::agent_execution::{
    agents::Agent,
    permissions::ActionContext,
    sessions::{ArtifactName, ArtifactRecord, ArtifactRecordError, PublishedFile},
};
use nessa_sdk::domain::{
    agent_execution::{executions::ExecutionId, leases::LeaseId},
    common::value_objects::{MediaType, Sha256Digest},
};
use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex, PoisonError},
};
use tokio::sync::oneshot;

/// One file the host published under a lease, offered to be kept.
pub struct ArtifactOffer {
    /// What the host says the file is; nothing of it is checked yet.
    pub file: StagedArtifact,
    /// Its bytes, read only when they are pulled.
    pub bytes: Box<dyn ArtifactBytes>,
    /// Tells the host what became of it.
    pub answer: ArtifactAnswer,
    /// The agent's tool call it answers, when the gateway asked for it;
    /// `None` for a file a harness published by itself.
    pub request: Option<String>,
}

/// What [`ArtifactBytes::next`] answers.
pub type ArtifactChunk<'a> =
    Pin<Box<dyn Future<Output = Result<Option<Vec<u8>>, ArtifactReadFailure>> + Send + 'a>>;

/// A published file's bytes, pulled in order.
pub trait ArtifactBytes: Send {
    /// The next chunk, or `None` once every byte the host published was
    /// read. Nothing is read until this is first asked.
    fn next(&mut self) -> ArtifactChunk<'_>;
}

/// Why a published file's bytes stopped before their end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArtifactReadFailure {
    /// The channel to the host failed past what resuming covers.
    Unavailable,
    /// The lease is no longer held there.
    LeaseEnded,
    /// The host's file is not what it published: gone, or another size.
    Changed,
}

impl ArtifactReadFailure {
    /// What the host is told of a file whose read failed this way.
    pub fn refusal(self) -> CollectionRefusal {
        match self {
            Self::Unavailable => CollectionRefusal::ChannelUnavailable,
            Self::LeaseEnded => CollectionRefusal::LeaseEnded,
            Self::Changed => CollectionRefusal::Mismatch,
        }
    }
}

/// Sending one answer to the host: done once the answer is queued for it.
pub type ArtifactAnswering = Pin<Box<dyn Future<Output = ()> + Send>>;

/// The one answer the host is given about an offer. Dropped unanswered, it
/// answers that the file was not kept, so the host never waits on nobody.
pub struct ArtifactAnswer {
    send: Option<Box<dyn FnOnce(Collection) -> ArtifactAnswering + Send>>,
}

impl ArtifactAnswer {
    /// `send` starts sending an outcome; what it returns is done once the
    /// outcome is queued for the host.
    pub fn new(send: impl FnOnce(Collection) -> ArtifactAnswering + Send + 'static) -> Self {
        Self {
            send: Some(Box::new(send)),
        }
    }

    /// Answer `outcome`, and wait until it is queued for the host. The send
    /// runs on a task of its own, so it completes even when this wait is
    /// given up.
    pub async fn answer(mut self, outcome: Collection) {
        if let Some(send) = self.send.take() {
            let sending = send(outcome);
            match tokio::runtime::Handle::try_current() {
                Ok(runtime) => {
                    let _ = runtime.spawn(sending).await;
                }
                Err(_) => sending.await,
            }
        }
    }
}

impl Drop for ArtifactAnswer {
    /// Not kept: sent on a task of its own, or, outside a runtime, as far
    /// as it goes at once.
    fn drop(&mut self) {
        if let Some(send) = self.send.take() {
            let sending = send(Collection::Refused {
                reason: CollectionRefusal::NotKept,
            });
            match tokio::runtime::Handle::try_current() {
                Ok(runtime) => {
                    runtime.spawn(sending);
                }
                Err(_) => {
                    if futures_util::FutureExt::now_or_never(sending).is_none() {
                        tracing::error!("an artifact's answer found no room; it is not sent");
                    }
                }
            }
        }
    }
}

/// Most times a published file's read is resumed on a new channel after
/// its channel failed, from the offset it reached. Row
/// `artifact.read_resumes` in `docs/limits.md`.
pub const ARTIFACT_READ_RESUMES: u32 = 3;

/// Most one lease may publish and have kept: past either, a file is
/// refused unread. Rows `artifact.environment_files` and `artifact.environment_bytes`
/// in `docs/limits.md`. And most one lease's agent may fetch into its
/// environment: past it, a fetch is refused before the environment is
/// asked. Row `artifact.fetch_bytes`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ArtifactBudget {
    pub(crate) files: u32,
    pub(crate) bytes: u64,
    pub(crate) fetch_bytes: u64,
}

impl ArtifactBudget {
    pub(crate) const LEASE: Self = Self {
        files: 64,
        bytes: 1024 * 1024 * 1024,
        fetch_bytes: 1024 * 1024 * 1024,
    };
}

/// The fetches a live conversation's agent asked for under its lease
/// (`artifacts_fetch`, slice D3b): which tool calls are under way, and how
/// many bytes each lease's fetches were given. A fetch's size is reserved
/// before the environment is asked, and given back only once its answer
/// says it placed nothing; one whose answer never came stays counted.
#[derive(Default)]
pub(crate) struct FetchRequests {
    leases: Mutex<HashMap<LeaseId, Fetched>>,
}

#[derive(Default)]
struct Fetched {
    bytes: u64,
    calls: std::collections::HashSet<String>,
}

/// Why a fetch was not taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FetchTaken {
    /// A fetch for the same tool call is under way.
    Duplicate,
    /// It would take the lease past its budget.
    OverBudget,
}

impl FetchRequests {
    fn leases(&self) -> std::sync::MutexGuard<'_, HashMap<LeaseId, Fetched>> {
        self.leases.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Take `call` under `lease`, reserving `size` of `budget` bytes.
    pub(crate) fn take(
        &self,
        lease: &LeaseId,
        call: &str,
        size: u64,
        budget: u64,
    ) -> Result<(), FetchTaken> {
        let mut leases = self.leases();
        let fetched = leases.entry(lease.clone()).or_default();
        if fetched.calls.contains(call) {
            return Err(FetchTaken::Duplicate);
        }
        let bytes = fetched.bytes.saturating_add(size);
        if bytes > budget {
            return Err(FetchTaken::OverBudget);
        }
        fetched.bytes = bytes;
        fetched.calls.insert(call.to_owned());
        Ok(())
    }

    /// `call` is over; `placed_nothing` gives its `size` back.
    pub(crate) fn finish(&self, lease: &LeaseId, call: &str, size: u64, placed_nothing: bool) {
        let mut leases = self.leases();
        if let Some(fetched) = leases.get_mut(lease) {
            fetched.calls.remove(call);
            if placed_nothing {
                fetched.bytes = fetched.bytes.saturating_sub(size);
            }
        }
    }
}

/// What a file the agent asked for came to, once the conversation answered
/// its offer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishedOutcome {
    /// Held, already held, or refused, as the host was told.
    pub collection: Collection,
    /// Its name, as the environment published it.
    pub name: String,
    /// Its SHA-256, lowercase hex, as the environment published it.
    pub digest: String,
    /// Its length, as the environment published it.
    pub size: u64,
}

/// One publish the agent asked for whose outcome has not settled.
struct Asked {
    /// The turn the call was made in: the one its record names.
    turn: ExecutionId,
    /// The tool call's person, with the call as the request.
    actor: ActionContext,
    /// Where its outcome goes; `None` once its call stopped waiting, or
    /// once its offer is being kept, which then holds it.
    answer: Option<oneshot::Sender<PublishedOutcome>>,
    /// Whether its offer arrived and is being kept: a second offer naming
    /// it is not this request's.
    collecting: bool,
}

/// Who asked for an offer being kept, taken from [`PublishRequests`].
struct Asking {
    turn: ExecutionId,
    actor: ActionContext,
    answer: Option<oneshot::Sender<PublishedOutcome>>,
}

/// The publishes a live conversation's agent asked for under its lease,
/// by lease and tool call, from before the environment is asked until the
/// outcome of the offer answering each settles, or the environment says it
/// offers none. While a request is held, asking it again is a duplicate,
/// its offer being read included; once it settles, it may be asked again.
/// Who asked outlives the call's wait: a call that stops waiting still
/// leaves its person named on a file that arrives after.
#[derive(Default)]
pub(crate) struct PublishRequests {
    asked: Mutex<HashMap<(LeaseId, String), Asked>>,
}

impl PublishRequests {
    fn asked(&self) -> std::sync::MutexGuard<'_, HashMap<(LeaseId, String), Asked>> {
        self.asked.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Hold `request` under `lease` for `actor`, asking in `turn`, and
    /// answer where its outcome arrives; `None` when that request is
    /// already held.
    pub(crate) fn ask(
        &self,
        lease: &LeaseId,
        request: &str,
        turn: ExecutionId,
        actor: ActionContext,
    ) -> Option<oneshot::Receiver<PublishedOutcome>> {
        let mut asked = self.asked();
        let key = (lease.clone(), request.to_owned());
        if asked.contains_key(&key) {
            return None;
        }
        let (answer, answered) = oneshot::channel();
        asked.insert(
            key,
            Asked {
                turn,
                actor,
                answer: Some(answer),
                collecting: false,
            },
        );
        Some(answered)
    }

    /// The environment offers nothing for `request`: it is let go, unless
    /// its offer is already being kept, whose outcome settles it.
    pub(crate) fn forget(&self, lease: &LeaseId, request: &str) {
        let mut asked = self.asked();
        let key = (lease.clone(), request.to_owned());
        if asked.get(&key).is_some_and(|asked| !asked.collecting) {
            asked.remove(&key);
        }
    }

    /// Its call stopped waiting: nobody is told the outcome, and who asked
    /// is kept for the offer that may still arrive.
    pub(crate) fn abandon(&self, lease: &LeaseId, request: &str) {
        if let Some(asked) = self.asked().get_mut(&(lease.clone(), request.to_owned())) {
            asked.answer = None;
        }
    }

    /// Who asked for `request`, its offer now being kept; `None` when
    /// nobody here asked, or its first offer is already being kept. It
    /// stays held, so asking again is a duplicate, until [`Self::settle`].
    fn take(&self, lease: &LeaseId, request: &str) -> Option<Asking> {
        let mut asked = self.asked();
        let asked = asked
            .get_mut(&(lease.clone(), request.to_owned()))
            .filter(|asked| !asked.collecting)?;
        asked.collecting = true;
        Some(Asking {
            turn: asked.turn.clone(),
            actor: asked.actor.clone(),
            answer: asked.answer.take(),
        })
    }

    /// The outcome of `request`'s offer is answered: it is let go.
    fn settle(&self, lease: &LeaseId, request: &str) {
        self.asked().remove(&(lease.clone(), request.to_owned()));
    }
}

/// The turn a file the lease's issuer's work published is recorded under:
/// the running turn once the conversation has taken it in, and none before
/// (a record may name only a turn the conversation accepted). The one rule
/// for both the room a file is checked against and the record committed.
async fn issuer_turn(agent: &Agent) -> Option<ExecutionId> {
    let turn = agent.active_execution_id()?;
    agent
        .session_manager()
        .accepts_turn(&turn)
        .await
        .then_some(turn)
}

/// Who a kept file is recorded as asked for by.
enum Asker {
    /// The lease's issuer, for a file its work published.
    Issuer(ActionContext),
    /// The person of the turn the agent asked in, with the call as their
    /// request.
    Turn(ExecutionId, ActionContext),
}

/// What one lease's files are kept for: the conversation, and the lease
/// with the person who asked for its work.
pub(crate) struct ArtifactCollector {
    pub(crate) organization_id: OrganizationId,
    pub(crate) conversation_id: ConversationId,
    pub(crate) lease: LeaseId,
    pub(crate) issuer: ActionContext,
    pub(crate) attachments: Option<Arc<dyn ConversationAttachments>>,
    pub(crate) budget: ArtifactBudget,
    /// What this lease's files kept so far took: files, bytes.
    spent: (u32, u64),
}

impl ArtifactCollector {
    pub(crate) fn new(
        organization_id: OrganizationId,
        conversation_id: ConversationId,
        lease: LeaseId,
        issuer: ActionContext,
        attachments: Option<Arc<dyn ConversationAttachments>>,
        budget: ArtifactBudget,
    ) -> Self {
        Self {
            organization_id,
            conversation_id,
            lease,
            issuer,
            attachments,
            budget,
            spent: (0, 0),
        }
    }

    /// Keep `offer` for the conversation `agent` runs under `lease`, and
    /// answer it. Nothing is read once the lease stops being Live. An offer
    /// the agent asked for is attributed to the call's person in
    /// `requests`, whose waiter is told the outcome once the host is.
    pub(crate) async fn collect(
        &mut self,
        offer: ArtifactOffer,
        agent: &Agent,
        lease: &LiveLease,
        requests: &PublishRequests,
    ) -> PublishedOutcome {
        let ArtifactOffer {
            file,
            bytes,
            answer,
            request,
        } = offer;
        let asked = request
            .as_ref()
            .map(|request| requests.take(&self.lease, request));
        let settles = matches!(asked, Some(Some(_)));
        let (asker, waiter) = match asked {
            None => (Some(Asker::Issuer(self.issuer.clone())), None),
            Some(Some(asked)) => (Some(Asker::Turn(asked.turn, asked.actor)), asked.answer),
            // Asked by nobody here: nobody to name as having asked.
            Some(None) => (None, None),
        };
        let described = (file.name.clone(), file.digest.clone(), file.size);
        let outcome = match (lease.is_live(), asker) {
            (_, None) => Err(CollectionRefusal::Invalid),
            (false, _) => Err(CollectionRefusal::LeaseEnded),
            (true, Some(asker)) => self.keep(file, lease.while_live(bytes), agent, asker).await,
        };
        let outcome = match outcome {
            Ok(ArtifactKept::Held) => Collection::Held,
            Ok(ArtifactKept::AlreadyHeld) => Collection::AlreadyHeld,
            Err(reason) => Collection::Refused { reason },
        };
        // The host is answered, its answer queued, before the call is; and
        // the request let go first, so the call answered is free to ask it
        // again. Only this offer's own outcome settles its request: one
        // refused as not this request's leaves it held.
        answer.answer(outcome).await;
        if let Some(request) = request.filter(|_| settles) {
            requests.settle(&self.lease, &request);
        }
        let (name, digest, size) = described;
        let published = PublishedOutcome {
            collection: outcome,
            name,
            digest,
            size,
        };
        if let Some(waiter) = waiter {
            let _ = waiter.send(published.clone());
        }
        published
    }

    /// Keep `file` for the conversation, asked for by `asker`.
    async fn keep(
        &mut self,
        file: StagedArtifact,
        bytes: Box<dyn ArtifactBytes>,
        agent: &Agent,
        asker: Asker,
    ) -> Result<ArtifactKept, CollectionRefusal> {
        let (files, spent) = self.spent;
        let over =
            files >= self.budget.files || spent.saturating_add(file.size) > self.budget.bytes;
        if over {
            return Err(CollectionRefusal::BudgetExceeded);
        }
        let attachments = self
            .attachments
            .clone()
            .ok_or(CollectionRefusal::ChannelUnavailable)?;
        // The path is the environment's to name and the protocol's to bound;
        // a channel is never opened for one past that bound.
        if !file.path.starts_with('/') || file.path.len() > MAX_ARTIFACT_PATH_BYTES {
            return Err(CollectionRefusal::Invalid);
        }
        let name = ArtifactName::new(file.name.clone()).map_err(|_| CollectionRefusal::Invalid)?;
        let published = Sha256Digest::parse(&format!("sha256:{}", file.digest))
            .ok()
            .zip(MediaType::parse(&file.media_type).ok())
            .and_then(|(digest, media_type)| PublishedFile::new(digest, media_type, file.size).ok())
            .ok_or(CollectionRefusal::Invalid)?;
        let manager = agent.session_manager();
        let (turn, actor) = match asker {
            Asker::Issuer(actor) => (issuer_turn(agent).await, actor),
            Asker::Turn(turn, actor) => (Some(turn), actor),
        };
        let record = ArtifactRecord {
            lease: self.lease.clone(),
            turn,
            name,
            file: published,
            actor: actor.clone(),
        };
        // Nothing read for a file the conversation could not record; the
        // same record already kept has room, as it needs no more. The room
        // is asked for the very record the gate commits, its turn decided
        // once, above.
        match manager.has_artifact_room(&record).await {
            Some(false) => return Err(CollectionRefusal::BudgetExceeded),
            Some(true) => {}
            None => return Err(CollectionRefusal::LeaseEnded),
        }
        let size = file.size;
        let refused = Arc::new(Mutex::new(None));
        let record = self.record_gate(record, agent.clone(), refused.clone());
        let kept = attachments
            .keep_published(PublishedArtifact {
                organization_id: self.organization_id.clone(),
                conversation_id: self.conversation_id.clone(),
                lease: self.lease.as_str().to_owned(),
                requested_by: actor,
                file,
                bytes,
                record,
            })
            .await
            .map_err(|refusal| {
                // Not recorded: the record's refusal is what the host hears.
                match refused
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take()
                {
                    Some(ArtifactRecordError::Full) => CollectionRefusal::BudgetExceeded,
                    Some(ArtifactRecordError::NotThisLease) => CollectionRefusal::LeaseEnded,
                    Some(_) => CollectionRefusal::NotKept,
                    None => refusal,
                }
            })?;
        // Every file kept counts toward the lease's files, one already held
        // too; only bytes read count toward its bytes.
        let read = match kept {
            ArtifactKept::Held => size,
            ArtifactKept::AlreadyHeld => 0,
        };
        self.spent = (files.saturating_add(1), spent.saturating_add(read));
        Ok(kept)
    }

    /// The conversation's record of a published file, asked for by whoever
    /// keeps it once it is held, so a file is never kept that the
    /// conversation did not record, nor recorded unheld: a
    /// lease that ended meanwhile, or a conversation with no room left, is
    /// a refusal, stashed in `refused` for the host to be told. `record`
    /// is committed as given, its turn already decided ([`issuer_turn`] for
    /// the issuer's, the call's own for one the agent asked for), so it is
    /// the record whose room was checked.
    fn record_gate(
        &self,
        record: ArtifactRecord,
        agent: Agent,
        refused: Arc<Mutex<Option<ArtifactRecordError>>>,
    ) -> ArtifactRecordGate {
        let conversation_id = self.conversation_id.clone();
        Box::new(move || {
            Box::pin(async move {
                match agent.session_manager().record_artifact(record).await {
                    Ok(()) => true,
                    // Retained, and written with the next save, as a lease
                    // record is: the record stands.
                    Err(ArtifactRecordError::Storage(error)) => {
                        tracing::error!(%conversation_id, %error, "an artifact record was retained but not yet saved");
                        true
                    }
                    Err(error) => {
                        *refused.lock().unwrap_or_else(PoisonError::into_inner) = Some(error);
                        false
                    }
                }
            })
        })
    }
}
