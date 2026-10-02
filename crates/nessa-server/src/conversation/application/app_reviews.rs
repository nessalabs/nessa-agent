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
use super::mcp_apps::{McpAppInitiator, McpAppRef, McpAppWithdrawal};
use super::view::{
    ConversationPermission, ConversationPermissionOption, ConversationPermissionOrigin,
};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::oneshot;
use uuid::Uuid;

/// How long an app's review waits for the person.
pub const APP_REVIEW_DEADLINE: Duration = Duration::from_secs(5 * 60);
/// The option that allows an app's call.
pub const ALLOW: &str = "allow";
/// The option that denies it.
pub const DENY: &str = "deny";
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
/// What an app review's identity starts with.
const APP_REVIEW_PREFIX: &str = "app-";

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
        // wait reads its review as withdrawn by the system.
        state.pending.clear();
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
            std::mem::take(&mut state.pending)
        };
        withdraw_ended(ended, &McpAppInitiator::System);
    }

    /// Whether `app` may be admitted in the opening `epoch` now.
    pub fn admit(&self, epoch: u64, app: &McpAppRef) -> Result<(), ReviewRefusal> {
        self.state.lock().expect("app reviews").live(epoch, app)
    }

    /// Open the review `permission_id` ([`new_review_id`], taken first so
    /// that its request is on record before it is shown) of `app`'s call to
    /// `tool` on `server` with `arguments_json`; it stands, in the view,
    /// until it ends.
    pub fn open(
        self: &Arc<Self>,
        epoch: u64,
        permission_id: String,
        app: &McpAppRef,
        server: &str,
        tool: &str,
        arguments_json: &str,
    ) -> Result<Waiting, ReviewRefusal> {
        let (end, ended) = oneshot::channel();
        let review = review_of(&permission_id, app, server, tool, arguments_json);
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
        let end = match option {
            ALLOW => ReviewEnd::Allowed(by),
            DENY => ReviewEnd::Denied(by),
            _ => return ReviewAnswer::Stale,
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
            std::mem::take(&mut state.pending)
        };
        withdraw_ended(ended, by);
    }
}

/// Whether the review of `app`'s call to `tool` on `server` with
/// `arguments_json` fits within [`MAX_APP_REVIEW_BYTES`] on its own.
pub fn fits(
    permission_id: &str,
    app: &McpAppRef,
    server: &str,
    tool: &str,
    arguments_json: &str,
) -> bool {
    encoded_len(&review_of(permission_id, app, server, tool, arguments_json))
        <= MAX_APP_REVIEW_BYTES
}

fn review_of(
    permission_id: &str,
    app: &McpAppRef,
    server: &str,
    tool: &str,
    arguments_json: &str,
) -> ConversationPermission {
    ConversationPermission {
        execution_id: app.execution_id.clone(),
        permission_id: permission_id.to_owned(),
        tool_id: app.tool_id.clone(),
        title: format!("An app asks to run {tool} on {server}"),
        tool_name: tool.to_owned(),
        arguments_json: arguments_json.to_owned(),
        options: vec![
            ConversationPermissionOption {
                id: ALLOW.into(),
                label: "Allow".into(),
            },
            ConversationPermissionOption {
                id: DENY.into(),
                label: "Deny".into(),
            },
        ],
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
        // The sender goes only with its conversation's registry.
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
