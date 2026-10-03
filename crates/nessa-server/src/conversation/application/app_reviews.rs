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
//! Under the same lock (#390): the mounts the person allowed to send
//! messages in this opening, and the context each mount last gave the model,
//! held until a message takes it. A release or an opening's end lets go of
//! both with everything else (`docs/design/mcp-app-calls.md`, "An app in its
//! conversation").
use super::mcp_apps::{McpAppInitiator, McpAppRef, McpAppWithdrawal};
use super::view::{
    ConversationPermission, ConversationPermissionOption, ConversationPermissionOptionEffect,
    ConversationPermissionOrigin,
};
use crate::product_contract::generated::MCP_APP_REVIEW_DEADLINE_MS;
use nessa_sdk::domain::agent_execution::prompts::{AppModelContext, UserMessage};
use std::{
    collections::{BTreeMap, VecDeque},
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
/// The most mounts that hold a context at once, in one conversation: as
/// many as one message carries, so a message takes every one held.
pub const MAX_HELD_CONTEXTS: usize = UserMessage::MAX_APP_MODEL_CONTEXTS;
/// The most mounts one opening remembers allowing to send messages. Past it
/// the longest allowed is forgotten, and asks again.
pub const MAX_CONSENTED_MOUNTS: usize = 64;
/// What an app review's identity starts with.
const APP_REVIEW_PREFIX: &str = "app-";

/// What an app's review asks the person to allow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewAsk {
    /// Running a destructive tool of its server.
    RunTool,
    /// Sending a message in the conversation as them: the first of its
    /// mount's, in this opening.
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

struct Pending {
    review: ConversationPermission,
    bytes: usize,
    app: McpAppRef,
    ask: ReviewAsk,
    end: oneshot::Sender<ReviewEnd>,
}

/// One conversation's apps, across its openings.
#[derive(Default)]
pub struct AppReviews {
    state: Mutex<Reviews>,
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
    /// The mounts the person allowed to send messages in this opening,
    /// longest allowed first, at most [`MAX_CONSENTED_MOUNTS`].
    consented: VecDeque<McpAppRef>,
    /// The context each mount last gave, in the order they were given, from at most
    /// [`MAX_HELD_CONTEXTS`] mounts: held until a message takes it. A mount
    /// may have one pending beside it, not yet on record.
    contexts: Vec<HeldContext>,
    /// What the next held context is numbered: a message takes a context
    /// only if it is still the one it read.
    next_context: u64,
}
/// One mount's context, as it gave it.
struct HeldContext {
    app: McpAppRef,
    number: u64,
    context: AppModelContext,
    /// Its update is on record, so a message may take it. Until then it is
    /// pending: it holds its mount's place, and no message sees it.
    recorded: bool,
}

/// Why a context was not held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextRefusal {
    /// [`MAX_HELD_CONTEXTS`] other mounts hold one already.
    Full,
    /// Its mount was released, or the opening ended.
    Gone(ReviewRefusal),
}

/// The contexts held when a message read them, in the order given, and
/// which they were: [`AppReviews::took`] lets go of exactly these, and of
/// none replaced since.
pub struct HeldContexts {
    pub contexts: Vec<AppModelContext>,
    taken: Vec<(McpAppRef, u64)>,
}
impl Reviews {
    /// The person allowed `app` to send messages in the current opening.
    fn allow(&mut self, app: &McpAppRef) {
        if !self.consented.contains(app) {
            if self.consented.len() == MAX_CONSENTED_MOUNTS {
                self.consented.pop_front();
            }
            self.consented.push_back(app.clone());
        }
    }
    /// What an opening holds for its mounts besides reviews and tickets:
    /// let go of when it ends, as it begins.
    fn let_go_of_the_opening(&mut self) {
        self.consented.clear();
        self.contexts.clear();
    }
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
    /// A new opening of the conversation's agent: its epoch, which every app
    /// call that resolves it is admitted against. What was released is
    /// still released.
    pub fn begin(&self) -> u64 {
        let mut state = self.state.lock().expect("app reviews");
        state.epoch += 1;
        state.ended = state.deleted;
        state.bytes = 0;
        // Every opening is ended before the next begins. Should one ever not
        // be, its reviews are not carried into this one: let go of, each
        // wait reads its review as withdrawn by the system. Its tickets are
        // not let go of here; they run out their lifetime.
        state.pending.clear();
        state.let_go_of_the_opening();
        state.epoch
    }

    /// The conversation was deleted: end the current opening, and begin no
    /// other; what was held of it — its released mounts — is let go, as
    /// nothing will name it again. `release` runs under the lock.
    pub fn delete(&self, release: impl FnOnce()) {
        let ended = {
            let mut state = self.state.lock().expect("app reviews");
            state.deleted = true;
            // Its opening's end let go of what was held for it already.
            if !state.ended {
                release();
            }
            state.ended = true;
            state.released.clear();
            state.bytes = 0;
            state.let_go_of_the_opening();
            std::mem::take(&mut state.pending)
        };
        withdraw_ended(ended, &McpAppInitiator::System);
    }

    /// Whether the person allowed `app` to send messages in the opening
    /// `epoch`, where it may still be admitted.
    pub fn consented(&self, epoch: u64, app: &McpAppRef) -> Result<bool, ReviewRefusal> {
        let state = self.state.lock().expect("app reviews");
        state.live(epoch, app)?;
        Ok(state.consented.contains(app))
    }

    /// The person allowed `app` to send messages in the opening `epoch`:
    /// it does not ask again until it is released or the opening ends.
    pub fn consent(&self, epoch: u64, app: &McpAppRef) -> Result<(), ReviewRefusal> {
        let mut state = self.state.lock().expect("app reviews");
        state.live(epoch, app)?;
        state.allow(app);
        Ok(())
    }

    /// Hold `context` pending, as what `app` gives the model next, in the
    /// opening `epoch`: its number, for [`Self::recorded_context`] once its
    /// update is on record, or [`Self::discard_context`] if it cannot be.
    /// Until then what the mount held before is what a message takes.
    pub fn stage_context(
        &self,
        epoch: u64,
        app: &McpAppRef,
        context: AppModelContext,
    ) -> Result<u64, ContextRefusal> {
        let mut state = self.state.lock().expect("app reviews");
        state.live(epoch, app).map_err(ContextRefusal::Gone)?;
        let mut mounts: Vec<&McpAppRef> = Vec::new();
        for held in &state.contexts {
            if &held.app != app && !mounts.contains(&&held.app) {
                mounts.push(&held.app);
            }
        }
        if mounts.len() >= MAX_HELD_CONTEXTS {
            return Err(ContextRefusal::Full);
        }
        state.next_context += 1;
        let number = state.next_context;
        state.contexts.push(HeldContext {
            app: app.clone(),
            number,
            context,
            recorded: false,
        });
        Ok(number)
    }

    /// The update that staged context `number` is on record: it replaces
    /// what its mount held, unless a later one of the mount's already did.
    /// One let go of meanwhile — its mount released, its opening ended —
    /// stays let go of: the release came after it.
    pub fn recorded_context(&self, number: u64) {
        let mut state = self.state.lock().expect("app reviews");
        let Some(app) = state
            .contexts
            .iter()
            .find(|held| held.number == number)
            .map(|held| held.app.clone())
        else {
            return;
        };
        let newer = state
            .contexts
            .iter()
            .any(|held| held.app == app && held.recorded && held.number > number);
        state.contexts.retain(|held| {
            held.app != app
                || if newer {
                    held.number != number
                } else {
                    held.number >= number || !held.recorded
                }
        });
        if let Some(held) = state.contexts.iter_mut().find(|held| held.number == number) {
            held.recorded = true;
        }
    }

    /// Let go of the pending context `number`: its update could not be
    /// recorded, so it was never given.
    pub fn discard_context(&self, number: u64) {
        let mut state = self.state.lock().expect("app reviews");
        state
            .contexts
            .retain(|held| held.recorded || held.number != number);
    }

    /// `app` asks to give the model nothing, in the opening `epoch`: its
    /// number, for [`Self::recorded_clear`] once that is on record. Nothing
    /// is let go of until then.
    pub fn stage_clear(&self, epoch: u64, app: &McpAppRef) -> Result<u64, ReviewRefusal> {
        let mut state = self.state.lock().expect("app reviews");
        state.live(epoch, app)?;
        state.next_context += 1;
        Ok(state.next_context)
    }

    /// The clear `number` of `app` is on record: let go of what the mount
    /// gave before it, held or pending. An update given after it stands,
    /// whichever was recorded first.
    pub fn recorded_clear(&self, app: &McpAppRef, number: u64) {
        let mut state = self.state.lock().expect("app reviews");
        state
            .contexts
            .retain(|held| &held.app != app || held.number > number);
    }

    /// The contexts held now, in the order they were given, for a message
    /// to carry: those on record, never one pending.
    pub fn held_contexts(&self) -> HeldContexts {
        let state = self.state.lock().expect("app reviews");
        let held = state.contexts.iter().filter(|held| held.recorded);
        HeldContexts {
            contexts: held.clone().map(|held| held.context.clone()).collect(),
            taken: held.map(|held| (held.app.clone(), held.number)).collect(),
        }
    }

    /// A message carried `taken`: let go of each that is still held as it
    /// was read. One replaced since is newer than what was sent, and is
    /// kept for the next message.
    pub fn took(&self, taken: &HeldContexts) {
        let mut state = self.state.lock().expect("app reviews");
        state.contexts.retain(|held| {
            !taken
                .taken
                .iter()
                .any(|(app, number)| &held.app == app && held.number == *number)
        });
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
                ask,
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
        // Allowing a mount's first message allows the mount, in this
        // opening: its other first messages, waiting on reviews of their
        // own, are allowed with it, by the same answer — as they would be,
        // unasked, had they come a moment later.
        if matches!(end, ReviewEnd::Allowed(_)) && open.ask == ReviewAsk::SendMessage {
            let epoch = state.epoch;
            if state.live(epoch, &open.app).is_ok() {
                state.allow(&open.app);
            }
            let siblings: Vec<u64> = state
                .pending
                .iter()
                .filter(|(_, other)| other.app == open.app && other.ask == ReviewAsk::SendMessage)
                .map(|(key, _)| *key)
                .collect();
            for key in siblings {
                if let Some(other) = state.remove(key) {
                    let _ = other.end.send(end.clone());
                }
            }
        }
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
    /// under the same lock, so nothing is issued after it. Idempotent.
    pub fn release_app(&self, app: &McpAppRef, by: &McpAppInitiator, release: impl FnOnce()) {
        let ended: Vec<Pending> = {
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
            state.consented.retain(|consented| consented != app);
            state.contexts.retain(|held| &held.app != app);
            release();
            keys.into_iter()
                .filter_map(|key| state.remove(key))
                .collect()
        };
        for open in ended {
            let _ = open.end.send(ReviewEnd::Withdrawn {
                cause: McpAppWithdrawal::AppTornDown,
                by: Some(by.clone()),
            });
        }
    }

    /// `by` ended the opening `epoch`: withdraw every review open, admit,
    /// open and issue nothing for it again, and run `release` under the same
    /// lock. Idempotent: a second end, or the end of an opening already
    /// gone, withdraws nothing and does not run `release`.
    pub fn end(&self, epoch: u64, by: &McpAppInitiator, release: impl FnOnce()) {
        let ended = {
            let mut state = self.state.lock().expect("app reviews");
            if state.ended || epoch != state.epoch {
                return;
            }
            state.ended = true;
            release();
            state.bytes = 0;
            state.let_go_of_the_opening();
            std::mem::take(&mut state.pending)
        };
        withdraw_ended(ended, by);
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
