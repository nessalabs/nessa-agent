//! A conversation's MCP Apps (#348): the reviews their destructive calls
//! wait on, and the one lock under which nothing is admitted, opened or
//! issued for a mount released or an opening ended. Kept by the service for
//! the conversation, not by its live agent: a release that comes while the
//! conversation is opening, or before, or across a close, is kept all the
//! same (`docs/design/mcp-app-calls.md`, "A conversation's apps").
//!
//! The reviews are gateway-owned, shown in the conversation's `permissions`
//! beside the agent's, answered through the same `conversation.answer` and
//! `conversation.cancel`, and each ended exactly once — allowed, denied,
//! expired, or withdrawn.
//!
//! Under the same lock (#390): the context each mount last gave the model,
//! held until a message admitted while the conversation is idle takes it.
//! Taken, it leaves the mount at once, and belongs to that message. A
//! release, an opening's end, a new opening and a delete drop only what is
//! still held, unsent; what a message took that went nowhere is reported
//! dropped as its [`Taken`] goes. Whatever drops a context, this is the one
//! owner of its drop's record: built here, from the update that held it and
//! who dropped it, and reported to [`DroppedContexts`] at the moment of
//! removal, after the lock — or, for what a message took, as the `Taken`
//! that holds it is dropped — never handed to the caller to record, so no
//! caller's cancellation, budget or panic can lose it: a close whose stop
//! runs past its budget
//! (`c15_a_close_whose_stop_runs_past_its_budget_drops_once_as_the_closers`),
//! a delete cut short by retirement mid-close
//! (`c15_a_delete_cut_short_by_retirement_records_each_drop_once`), and a
//! submission's task unwinding through what it took
//! (`c11_what_a_message_took_is_reported_dropped_as_it_goes_unless_the_agent_was_asked`)
//! each record every drop once. The
//! conversation's one update lock orders the updates themselves. And the app
//! messages in flight, by the turn each becomes, so one request is asked
//! about once at a time (`docs/design/mcp-app-calls.md`, "An app in its
//! conversation: the gateway").
use super::mcp_apps::{
    ContextDrop, DroppedContexts, McpAppAuditPhase, McpAppAuditRecord, McpAppInitiator, McpAppRef,
    McpAppWithdrawal,
};
use super::view::{
    ConversationPermission, ConversationPermissionOption, ConversationPermissionOptionEffect,
    ConversationPermissionOrigin,
};
use crate::product_contract::generated::MCP_APP_REVIEW_DEADLINE_MS;
use nessa_sdk::domain::agent_execution::prompts::{AppModelContext, UserMessage};
use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::oneshot;
use uuid::Uuid;

/// How long an app's review waits for the person: the protocol's
/// `x-mcpAppCallTiming.reviewDeadlineMs`, its one statement.
pub const APP_REVIEW_DEADLINE: Duration = Duration::from_millis(MCP_APP_REVIEW_DEADLINE_MS);
/// The option that allows an app's call.
pub const ALLOW: &str = "allow";
/// The option that denies it.
pub const DENY: &str = "deny";
/// The options an app's review offers, each with what it decides: the one
/// table both the view (`open`) and an answer (`answer`) read.
const OPTIONS: [(&str, &str, ConversationPermissionOptionEffect); 2] = [
    (ALLOW, "Allow", ConversationPermissionOptionEffect::Allow),
    (DENY, "Deny", ConversationPermissionOptionEffect::Deny),
];
/// The most app reviews one conversation has open at once, so that with the
/// agent's own they stay within the 64 a view carries.
pub const MAX_OPEN_APP_REVIEWS: usize = 16;
/// The most the open app reviews of one conversation may take of its view,
/// encoded, together: as much as one review of the agent's may. Past it an
/// app's reviews would push the transcript — and the app's own tool call —
/// out of a view bounded at 60 KB.
pub const MAX_APP_REVIEW_BYTES: usize = 16_000;
/// How many released mounts a conversation remembers.
pub const MAX_RELEASED_MOUNTS: usize = 1024;
/// The most mounts of one conversation that hold a context at once: as many
/// as one message carries, so a message carries every one held.
pub const MAX_HELD_CONTEXTS: usize = UserMessage::MAX_APP_MODEL_CONTEXTS;
/// What an app review's identity starts with.
const APP_REVIEW_PREFIX: &str = "app-";

/// What an app's review asks the person to allow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewAsk {
    /// Running a destructive tool of its server.
    RunTool,
    /// Sending one message in the conversation as them. Every message asks.
    SendMessage,
}

/// Who answered a review.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewAnswerer {
    pub principal_id: nessa_auth::domain::PrincipalId,
    pub surface_id: String,
    pub request_id: String,
}

/// How a review ended. Each ends once: the first of these wins.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReviewEnd {
    Allowed(ReviewAnswerer),
    Denied(ReviewAnswerer),
    Expired,
    /// Withdrawn, `by` whom: `None` is the app itself, its request gone.
    Withdrawn {
        cause: McpAppWithdrawal,
        by: Option<McpAppInitiator>,
    },
}

/// What an answer to a review did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReviewAnswer {
    /// No open app review has that identity: it is the agent's to answer.
    NotAnAppReview,
    /// It is an open app review, but the answer names another execution, or
    /// an option it did not offer: nothing happens.
    Stale,
    /// It ended now, as the answer said.
    Ended,
}

/// Why no review was opened, or nothing was issued.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewRefusal {
    /// [`MAX_OPEN_APP_REVIEWS`] are open, or they hold
    /// [`MAX_APP_REVIEW_BYTES`] already.
    Full,
    /// This one review alone is past [`MAX_APP_REVIEW_BYTES`].
    TooLarge,
    /// Its mount was released.
    Released,
    /// The conversation ended.
    Ended,
}

/// Why a context was not held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextRefusal {
    /// [`MAX_HELD_CONTEXTS`] other mounts hold one already.
    Full,
    /// Its mount was released, or the opening ended.
    Gone(ReviewRefusal),
}

struct Pending {
    review: ConversationPermission,
    bytes: usize,
    app: McpAppRef,
    end: oneshot::Sender<ReviewEnd>,
}

/// One conversation's apps, across its openings.
pub struct AppReviews {
    state: Mutex<Reviews>,
    /// The conversation's one context update at a time, held across its
    /// room check, its record and its hold ([`Self::one_update`]): so the
    /// order updates are recorded in is the order they are held in.
    updates: tokio::sync::Mutex<()>,
    /// Where each context dropped unsent is reported, as it is dropped.
    dropped: Arc<dyn DroppedContexts>,
}
#[derive(Default)]
struct Reviews {
    /// The current opening of the conversation's agent: each is a new
    /// epoch, and each app call is admitted against the one it resolved.
    epoch: u64,
    /// Open reviews, keyed by the order they were opened in.
    pending: BTreeMap<u64, Pending>,
    next: u64,
    /// What the open reviews take of the view, encoded.
    bytes: usize,
    /// The mounts released, newest last, at most [`MAX_RELEASED_MOUNTS`].
    released: VecDeque<McpAppRef>,
    /// The current opening ended: nothing is admitted, opened or issued
    /// for it again.
    ended: bool,
    /// The conversation was deleted: no opening begins again.
    deleted: bool,
    /// The context each mount last gave, in the order they were given, from
    /// at most [`MAX_HELD_CONTEXTS`] mounts.
    contexts: Vec<HeldContext>,
    /// The turns app messages in review or being sent will become: each at
    /// most once ([`AppReviews::start_message`]).
    messages: HashSet<String>,
}
/// One mount's context, as it gave it, and the record of the update that
/// gave it, which its drop is recorded against.
struct HeldContext {
    app: McpAppRef,
    context: AppModelContext,
    update: McpAppAuditRecord,
}
/// The records of the updates whose contexts `contexts` held: what a drop of
/// them is recorded against.
fn updates(contexts: Vec<HeldContext>) -> Vec<McpAppAuditRecord> {
    contexts.into_iter().map(|held| held.update).collect()
}
/// The record of the drop of what `update` held, `by` whom and why: the
/// update's own record — its conversation, app, call and request — with the
/// dropper as its initiator.
fn drop_record(
    update: McpAppAuditRecord,
    cause: ContextDrop,
    by: &McpAppInitiator,
) -> McpAppAuditRecord {
    McpAppAuditRecord {
        phase: McpAppAuditPhase::ContextDropped { cause },
        initiator: by.clone(),
        ..update
    }
}
impl Reviews {
    fn key(&self, permission: &str) -> Option<u64> {
        self.pending
            .iter()
            .find(|(_, open)| open.review.permission_id == permission)
            .map(|(key, _)| *key)
    }
    fn remove(&mut self, key: u64) -> Option<Pending> {
        let open = self.pending.remove(&key)?;
        self.bytes -= open.bytes;
        Some(open)
    }
    /// Whether anything may be admitted, opened or issued for `app`, in the
    /// opening `epoch`, now.
    fn live(&self, epoch: u64, app: &McpAppRef) -> Result<(), ReviewRefusal> {
        if self.ended || epoch != self.epoch {
            Err(ReviewRefusal::Ended)
        } else if self.released.contains(app) {
            Err(ReviewRefusal::Released)
        } else {
            Ok(())
        }
    }
}

/// One review's wait. Dropping it before the review ended — the app's
/// request went — withdraws the review.
pub struct Waiting {
    reviews: Arc<AppReviews>,
    pub permission_id: String,
    ended: Option<oneshot::Receiver<ReviewEnd>>,
}

impl AppReviews {
    /// A conversation's apps, none of them released, no opening begun; each
    /// context they drop is reported to `dropped`.
    pub fn new(dropped: Arc<dyn DroppedContexts>) -> Self {
        Self {
            state: Mutex::default(),
            updates: tokio::sync::Mutex::default(),
            dropped,
        }
    }

    /// [`report_dropped`] to this conversation's recorder.
    fn report_dropped(
        &self,
        updates: Vec<McpAppAuditRecord>,
        cause: ContextDrop,
        by: &McpAppInitiator,
    ) {
        report_dropped(self.dropped.as_ref(), updates, cause, by);
    }

    /// A new opening of the conversation's agent: its epoch, which every app
    /// call that resolves it is admitted against. What was released is still
    /// released.
    pub fn begin(&self) -> u64 {
        let (epoch, dropped) = {
            let mut state = self.state.lock().expect("app reviews");
            state.epoch += 1;
            state.ended = state.deleted;
            state.bytes = 0;
            // Every opening is ended before the next begins. Should one ever
            // not be, its reviews are not carried into this one: let go of,
            // each wait reads its review as withdrawn by the system. Its
            // tickets are not let go of here; they run out their lifetime.
            // Nor are the contexts it held: an opening's apps give the new
            // one nothing, and they are dropped, by the system.
            state.pending.clear();
            (state.epoch, updates(std::mem::take(&mut state.contexts)))
        };
        self.report_dropped(
            dropped,
            ContextDrop::ConversationEnded,
            &McpAppInitiator::System,
        );
        epoch
    }

    /// `by` deleted the conversation: end the current opening, and begin no
    /// other; what was held of it — its released mounts — is let go, as
    /// nothing will name it again, and the contexts still held are dropped,
    /// `by` the deleter. `release` runs under the lock.
    pub fn delete(&self, by: &McpAppInitiator, release: impl FnOnce()) {
        let (ended, dropped) = {
            let mut state = self.state.lock().expect("app reviews");
            state.deleted = true;
            // Its opening's end let go of what was held for it already.
            if !state.ended {
                release();
            }
            state.ended = true;
            state.released.clear();
            state.bytes = 0;
            (
                std::mem::take(&mut state.pending),
                updates(std::mem::take(&mut state.contexts)),
            )
        };
        self.report_dropped(dropped, ContextDrop::ConversationEnded, by);
        withdraw_ended(ended, &McpAppInitiator::System);
    }

    /// One context update of the conversation at a time: held across its
    /// room check ([`Self::room`]), its record and its hold ([`Self::hold`]).
    pub async fn one_update(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.updates.lock().await
    }

    /// Whether an update of `app` in the opening `epoch` may be given now:
    /// its mount not released, the opening not ended, and, when it is to
    /// hold a context (`holds`), room for it — this mount holds one, or
    /// fewer than [`MAX_HELD_CONTEXTS`] others do. Asked under
    /// [`Self::one_update`], so nothing takes the room before the hold.
    pub fn room(&self, epoch: u64, app: &McpAppRef, holds: bool) -> Result<(), ContextRefusal> {
        let state = self.state.lock().expect("app reviews");
        state.live(epoch, app).map_err(ContextRefusal::Gone)?;
        let others = state
            .contexts
            .iter()
            .filter(|held| &held.app != app)
            .count();
        if holds && others >= MAX_HELD_CONTEXTS {
            return Err(ContextRefusal::Full);
        }
        Ok(())
    }

    /// The update of `app`, on record as `update`, is what it gives the
    /// model now: `context`, in place of what it held, or nothing. A release
    /// or an end that came after its room was checked came after it too, and
    /// it is not held: then `Err`, when there was a context to hold.
    pub fn hold(
        &self,
        epoch: u64,
        app: &McpAppRef,
        context: Option<AppModelContext>,
        update: &McpAppAuditRecord,
    ) -> Result<(), NotHeld> {
        let mut state = self.state.lock().expect("app reviews");
        if state.live(epoch, app).is_err() {
            return match context {
                Some(_) => Err(NotHeld),
                None => Ok(()),
            };
        }
        state.contexts.retain(|held| &held.app != app);
        if let Some(context) = context {
            state.contexts.push(HeldContext {
                app: app.clone(),
                context,
                update: update.clone(),
            });
        }
        Ok(())
    }

    /// The app message that becomes the turn `execution`, in flight — in
    /// review or being sent — until the answer is dropped, however its call
    /// ends: `None` while another with the same turn is.
    #[must_use]
    pub fn start_message(self: &Arc<Self>, execution: &str) -> Option<MessageInFlight> {
        let mut state = self.state.lock().expect("app reviews");
        state
            .messages
            .insert(execution.to_owned())
            .then(|| MessageInFlight {
                reviews: self.clone(),
                execution: execution.to_owned(),
            })
    }

    /// The contexts held now, in the order they were given.
    #[cfg(test)]
    pub fn held(&self) -> Vec<AppModelContext> {
        self.state
            .lock()
            .expect("app reviews")
            .contexts
            .iter()
            .map(|held| held.context.clone())
            .collect()
    }

    /// Take every context held, in the order they were given, for a message
    /// admitted while the conversation is idle: they leave their mounts now,
    /// and free their room. Nothing is ever put back. A release or an end
    /// from now on finds them gone, and a mount's newer update is held anew.
    /// Should the message then go nowhere, each is reported dropped unsent
    /// as the [`Taken`] goes.
    #[must_use]
    pub fn take_held(&self) -> Taken {
        let taken = std::mem::take(&mut self.state.lock().expect("app reviews").contexts);
        let (contexts, updates) = taken
            .into_iter()
            .map(|held| (held.context, held.update))
            .unzip();
        Taken {
            contexts,
            updates,
            dropped: self.dropped.clone(),
            armed: true,
        }
    }

    /// Whether `app` may be admitted in the opening `epoch` now.
    pub fn admit(&self, epoch: u64, app: &McpAppRef) -> Result<(), ReviewRefusal> {
        self.state.lock().expect("app reviews").live(epoch, app)
    }

    /// Open the review `permission_id` ([`new_review_id`], taken first so
    /// that its request is on record before it is shown) of what `app`
    /// asks: to run `tool` on `server` with `arguments_json`, or — `tool`
    /// its own — to send the message `arguments_json` shows. It stands, in
    /// the view, until it ends.
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        self: &Arc<Self>,
        epoch: u64,
        permission_id: String,
        ask: ReviewAsk,
        app: &McpAppRef,
        server: &str,
        tool: &str,
        arguments_json: &str,
    ) -> Result<Waiting, ReviewRefusal> {
        let (end, ended) = oneshot::channel();
        let review = review_of(&permission_id, ask, app, server, tool, arguments_json);
        let bytes = encoded_len(&review);
        if bytes > MAX_APP_REVIEW_BYTES {
            return Err(ReviewRefusal::TooLarge);
        }
        let mut state = self.state.lock().expect("app reviews");
        state.live(epoch, app)?;
        if state.pending.len() >= MAX_OPEN_APP_REVIEWS || state.bytes + bytes > MAX_APP_REVIEW_BYTES
        {
            return Err(ReviewRefusal::Full);
        }
        state.next += 1;
        let key = state.next;
        state.bytes += bytes;
        state.pending.insert(
            key,
            Pending {
                review,
                bytes,
                app: app.clone(),
                end,
            },
        );
        Ok(Waiting {
            reviews: self.clone(),
            permission_id,
            ended: Some(ended),
        })
    }

    /// Run `issue` for `app` in the opening `epoch`, under the lock its
    /// release and the opening's end take: so nothing is issued for a mount
    /// released or an opening ended, whatever the interleaving.
    pub fn issue<T>(
        &self,
        epoch: u64,
        app: &McpAppRef,
        issue: impl FnOnce() -> T,
    ) -> Result<T, ReviewRefusal> {
        let state = self.state.lock().expect("app reviews");
        state.live(epoch, app)?;
        Ok(issue())
    }

    /// The reviews open now, oldest first.
    pub fn reviews(&self) -> Vec<ConversationPermission> {
        self.state
            .lock()
            .expect("app reviews")
            .pending
            .values()
            .map(|pending| pending.review.clone())
            .collect()
    }

    /// The person's answer to the review `permission` of `execution`.
    pub fn answer(
        &self,
        execution: &str,
        permission: &str,
        option: &str,
        by: ReviewAnswerer,
    ) -> ReviewAnswer {
        let mut state = self.state.lock().expect("app reviews");
        let Some(key) = state.key(permission) else {
            return ReviewAnswer::NotAnAppReview;
        };
        if state.pending[&key].review.execution_id != execution {
            return ReviewAnswer::Stale;
        }
        let end = match OPTIONS.iter().find(|&&(id, ..)| id == option) {
            Some((.., ConversationPermissionOptionEffect::Allow)) => ReviewEnd::Allowed(by),
            Some((.., ConversationPermissionOptionEffect::Deny)) => ReviewEnd::Denied(by),
            None => return ReviewAnswer::Stale,
        };
        let open = state.remove(key).expect("present");
        let _ = open.end.send(end);
        ReviewAnswer::Ended
    }

    /// The person's cancellation of a review: a denial.
    pub fn cancel(&self, execution: &str, permission: &str, by: ReviewAnswerer) -> ReviewAnswer {
        self.answer(execution, permission, DENY, by)
    }

    /// Withdraw the review `permission` as its app's: its request went.
    pub fn withdraw(&self, permission: &str) {
        self.end_one(
            permission,
            ReviewEnd::Withdrawn {
                cause: McpAppWithdrawal::RequestCancelled,
                by: None,
            },
        );
    }

    /// End the review `permission` with `end`, when it is still open.
    fn end_one(&self, permission: &str, end: ReviewEnd) {
        let open = {
            let mut state = self.state.lock().expect("app reviews");
            state.key(permission).and_then(|key| state.remove(key))
        };
        if let Some(open) = open {
            let _ = open.end.send(end);
        }
    }

    /// `by` released the mount `app`: withdraw its reviews, open nothing for
    /// it again, and run `release` — letting go of what was issued to it —
    /// under the same lock, so nothing is issued after it; drop its context
    /// still held, if any, `by` the releaser. Idempotent.
    pub fn release_app(&self, app: &McpAppRef, by: &McpAppInitiator, release: impl FnOnce()) {
        let (ended, dropped): (Vec<Pending>, _) = {
            let mut state = self.state.lock().expect("app reviews");
            if !state.released.contains(app) {
                if state.released.len() == MAX_RELEASED_MOUNTS {
                    state.released.pop_front();
                }
                state.released.push_back(app.clone());
            }
            let keys: Vec<u64> = state
                .pending
                .iter()
                .filter(|(_, open)| &open.app == app)
                .map(|(key, _)| *key)
                .collect();
            let (dropped, kept) = std::mem::take(&mut state.contexts)
                .into_iter()
                .partition(|held| &held.app == app);
            state.contexts = kept;
            release();
            (
                keys.into_iter()
                    .filter_map(|key| state.remove(key))
                    .collect(),
                updates(dropped),
            )
        };
        self.report_dropped(dropped, ContextDrop::Released, by);
        for open in ended {
            let _ = open.end.send(ReviewEnd::Withdrawn {
                cause: McpAppWithdrawal::AppTornDown,
                by: Some(by.clone()),
            });
        }
    }

    /// `by` ended the opening `epoch`: withdraw every review open, admit,
    /// open and issue nothing for it again, run `release` under the same
    /// lock, and drop every context still held, `by` whoever ended it — all
    /// before this returns, so a caller that goes on to await anything has
    /// already reported every drop. Idempotent: a second end, or the end of
    /// an opening already gone, withdraws and drops nothing and does not run
    /// `release`.
    pub fn end(&self, epoch: u64, by: &McpAppInitiator, release: impl FnOnce()) {
        let (ended, dropped) = {
            let mut state = self.state.lock().expect("app reviews");
            if state.ended || epoch != state.epoch {
                return;
            }
            state.ended = true;
            release();
            state.bytes = 0;
            (
                std::mem::take(&mut state.pending),
                updates(std::mem::take(&mut state.contexts)),
            )
        };
        self.report_dropped(dropped, ContextDrop::ConversationEnded, by);
        withdraw_ended(ended, by);
    }
}

/// The contexts a message took ([`AppReviews::take_held`]), and the records
/// of the updates that gave them: what a drop of them is recorded against.
///
/// The one owner of what the message took. Dropped before the agent was
/// asked to take the message — refused, failed, its task unwinding or let
/// go of — each is reported dropped unsent, by the system, to the
/// conversation's recorder (row C11). From the moment the agent is asked
/// ([`Self::asking`]) they follow the message: taken, or unknown — the agent
/// could not say, or the task failed mid-ask — the message's own record
/// covers them, and dropping this reports nothing (row C11b). Only the
/// agent's refusal hands them back ([`Self::refused`]).
///
/// Its fields are private, so a caller outside this module has no way to
/// empty its records and silence a drop: what it reports is what it took. The message takes the contexts out with
/// [`Self::take_contexts`]; the records stay.
pub struct Taken {
    contexts: Vec<AppModelContext>,
    updates: Vec<McpAppAuditRecord>,
    dropped: Arc<dyn DroppedContexts>,
    /// Whether dropping this reports each as dropped unsent.
    armed: bool,
}
impl Taken {
    /// The contexts taken, in the order given, for the message to carry.
    /// The records of the updates that gave them stay here: what a drop is
    /// reported against.
    pub fn take_contexts(&mut self) -> Vec<AppModelContext> {
        std::mem::take(&mut self.contexts)
    }

    /// The agent is being asked to take the message that carries these:
    /// from here they follow it, and dropping this reports nothing.
    pub fn asking(&mut self) {
        self.armed = false;
    }

    /// The agent refused the message: they went nowhere. Each is reported
    /// dropped unsent, by the system, as this goes; the app may give it
    /// again.
    pub fn refused(mut self) {
        self.armed = true;
    }
}
impl Drop for Taken {
    fn drop(&mut self) {
        if self.armed {
            report_dropped(
                self.dropped.as_ref(),
                std::mem::take(&mut self.updates),
                ContextDrop::NotSent,
                &McpAppInitiator::System,
            );
        }
    }
}

/// Report each context whose update is in `updates` to `dropped` as dropped
/// unsent, for `cause`, `by` whom: called once they are out of the state and
/// the lock is let go of, before anything is awaited — the one place a
/// drop's record is built and sent.
fn report_dropped(
    dropped: &dyn DroppedContexts,
    updates: Vec<McpAppAuditRecord>,
    cause: ContextDrop,
    by: &McpAppInitiator,
) {
    for update in updates {
        dropped.context_dropped(drop_record(update, cause, by));
    }
}

/// An update on record whose context was not held: its mount was released,
/// or its opening ended, after its room was checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotHeld;

/// An app message in flight, by the turn it becomes. Dropped — its call
/// ended, however: answered, refused, its caller gone, its task panicking —
/// the turn is free for the same request again.
pub struct MessageInFlight {
    reviews: Arc<AppReviews>,
    execution: String,
}
impl Drop for MessageInFlight {
    fn drop(&mut self) {
        // A panic elsewhere that poisoned the lock does not keep the turn.
        let mut state = self
            .reviews
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.messages.remove(&self.execution);
    }
}

/// Whether the review of what `app` asks ([`AppReviews::open`]) fits within
/// [`MAX_APP_REVIEW_BYTES`] on its own.
pub fn fits(
    permission_id: &str,
    ask: ReviewAsk,
    app: &McpAppRef,
    server: &str,
    tool: &str,
    arguments_json: &str,
) -> bool {
    encoded_len(&review_of(
        permission_id,
        ask,
        app,
        server,
        tool,
        arguments_json,
    )) <= MAX_APP_REVIEW_BYTES
}

fn review_of(
    permission_id: &str,
    ask: ReviewAsk,
    app: &McpAppRef,
    server: &str,
    tool: &str,
    arguments_json: &str,
) -> ConversationPermission {
    ConversationPermission {
        execution_id: app.execution_id.clone(),
        permission_id: permission_id.to_owned(),
        tool_id: app.tool_id.clone(),
        title: match ask {
            ReviewAsk::RunTool => format!("An app asks to run {tool} on {server}"),
            ReviewAsk::SendMessage => {
                format!("The {tool} app on {server} asks to send a message as you")
            }
        },
        tool_name: tool.to_owned(),
        arguments_json: arguments_json.to_owned(),
        options: OPTIONS
            .iter()
            .map(|&(id, label, effect)| ConversationPermissionOption {
                id: id.into(),
                label: label.into(),
                effect,
            })
            .collect(),
        origin: ConversationPermissionOrigin::App {
            server: server.to_owned(),
            tool: tool.to_owned(),
        },
    }
}

/// What a review takes of the view, encoded.
fn encoded_len(review: &ConversationPermission) -> usize {
    serde_json::to_vec(review).map_or(usize::MAX, |encoded| encoded.len())
}

/// Tell each review in `ended` that it was withdrawn as its conversation
/// ended, by `by`.
fn withdraw_ended(ended: BTreeMap<u64, Pending>, by: &McpAppInitiator) {
    for (_, open) in ended {
        let _ = open.end.send(ReviewEnd::Withdrawn {
            cause: McpAppWithdrawal::ConversationEnded,
            by: Some(by.clone()),
        });
    }
}

/// A new app review's identity.
pub fn new_review_id() -> String {
    format!("{APP_REVIEW_PREFIX}{}", Uuid::new_v4())
}

impl Waiting {
    /// How the review ended: as answered or withdrawn, or expired once
    /// `deadline` passes with no answer.
    pub async fn ended(mut self, deadline: Duration) -> ReviewEnd {
        // Borrowed, not taken: dropped mid-wait, this still withdraws.
        let ended = self.ended.as_mut().expect("waited once");
        let end = match tokio::time::timeout(deadline, &mut *ended).await {
            Ok(end) => end,
            Err(_) => {
                // The deadline: end it as expired — unless an answer ended
                // it first, in which case that answer is what it ended with.
                self.reviews
                    .end_one(&self.permission_id, ReviewEnd::Expired);
                ended.await
            }
        };
        self.ended = None;
        // The sender goes with its conversation's registry, or with an
        // opening that began over this one: either way, withdrawn as the
        // conversation ended, by the system.
        end.unwrap_or(ReviewEnd::Withdrawn {
            cause: McpAppWithdrawal::ConversationEnded,
            by: Some(McpAppInitiator::System),
        })
    }
}

impl Drop for Waiting {
    fn drop(&mut self) {
        // Still waiting: the app's request went before the review ended.
        if self.ended.is_some() {
            self.reviews.withdraw(&self.permission_id);
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/conversation/app_reviews.rs"]
mod tests;
