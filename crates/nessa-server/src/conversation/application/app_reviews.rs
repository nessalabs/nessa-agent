//! The reviews an MCP App's destructive calls wait on (#348): gateway-owned,
//! shown in the conversation's `permissions` beside the agent's, answered
//! through the same `conversation.answer` and `conversation.cancel`, and
//! each ended exactly once — allowed, denied, expired, or withdrawn.
use super::mcp_apps::{McpAppRef, McpAppWithdrawal};
use super::view::{
    ConversationPermission, ConversationPermissionOption, ConversationPermissionOrigin,
};
use crate::product_contract::generated::MCP_APP_REVIEW_MS;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::oneshot;
use uuid::Uuid;

/// How long an app's review waits for the person: the protocol's
/// `x-mcpAppTiming.reviewMs`, its one statement.
pub const APP_REVIEW_DEADLINE: Duration = Duration::from_millis(MCP_APP_REVIEW_MS);
/// The option that allows an app's call.
pub const ALLOW: &str = "allow";
/// The option that denies it.
pub const DENY: &str = "deny";
/// The most app reviews one conversation has open at once, so that with the
/// agent's own they stay within the 64 a view carries.
pub const MAX_OPEN_APP_REVIEWS: usize = 16;
/// What an app review's identity starts with. An agent names its own
/// reviews, so an app's carries a UUID no agent's will match.
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
    Withdrawn(McpAppWithdrawal),
}

/// What an answer to a review did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReviewAnswer {
    /// No app review has that identity: it is the agent's, or unknown.
    NotAnAppReview,
    /// It was an app review, but it has already ended, or the option is not
    /// one it offered: nothing happens.
    Stale,
    /// It ended now, as the answer said.
    Ended,
}

/// Why no review was opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewRefusal {
    /// [`MAX_OPEN_APP_REVIEWS`] are open already.
    Full,
    /// The conversation ended.
    Ended,
}

struct Pending {
    review: ConversationPermission,
    app: McpAppRef,
    end: oneshot::Sender<ReviewEnd>,
}

/// One conversation's open app reviews.
#[derive(Default)]
pub struct AppReviews {
    state: Mutex<Reviews>,
}
#[derive(Default)]
struct Reviews {
    /// Open reviews, keyed by the order they were opened in.
    pending: BTreeMap<u64, Pending>,
    next: u64,
    /// The conversation ended: no review opens again.
    ended: bool,
}
impl Reviews {
    fn key(&self, permission: &str) -> Option<u64> {
        self.pending
            .iter()
            .find(|(_, open)| open.review.permission_id == permission)
            .map(|(key, _)| *key)
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
    /// Open the review `permission_id` ([`new_review_id`], taken first so
    /// that its request is on record before it is shown) of `app`'s call to
    /// `tool` on `server` with `arguments_json`; it stands, in the view,
    /// until it ends.
    pub fn open(
        self: &Arc<Self>,
        permission_id: String,
        app: &McpAppRef,
        server: &str,
        tool: &str,
        arguments_json: &str,
    ) -> Result<Waiting, ReviewRefusal> {
        let (end, ended) = oneshot::channel();
        let review = ConversationPermission {
            execution_id: app.execution_id.clone(),
            permission_id: permission_id.clone(),
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
        };
        let mut state = self.state.lock().expect("app reviews");
        if state.ended {
            return Err(ReviewRefusal::Ended);
        }
        if state.pending.len() >= MAX_OPEN_APP_REVIEWS {
            return Err(ReviewRefusal::Full);
        }
        state.next += 1;
        let key = state.next;
        state.pending.insert(
            key,
            Pending {
                review,
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
            return if is_app_review(permission) {
                ReviewAnswer::Stale
            } else {
                ReviewAnswer::NotAnAppReview
            };
        };
        if state.pending[&key].review.execution_id != execution {
            return ReviewAnswer::Stale;
        }
        let end = match option {
            ALLOW => ReviewEnd::Allowed(by),
            DENY => ReviewEnd::Denied(by),
            _ => return ReviewAnswer::Stale,
        };
        let open = state.pending.remove(&key).expect("present");
        let _ = open.end.send(end);
        ReviewAnswer::Ended
    }

    /// The person's cancellation of a review: a denial.
    pub fn cancel(&self, execution: &str, permission: &str, by: ReviewAnswerer) -> ReviewAnswer {
        self.answer(execution, permission, DENY, by)
    }

    /// Withdraw the review `permission`, when it is still open.
    pub fn withdraw(&self, permission: &str, cause: McpAppWithdrawal) {
        self.end_one(permission, ReviewEnd::Withdrawn(cause));
    }

    /// End the review `permission` with `end`, when it is still open.
    fn end_one(&self, permission: &str, end: ReviewEnd) {
        let open = {
            let mut state = self.state.lock().expect("app reviews");
            state
                .key(permission)
                .and_then(|key| state.pending.remove(&key))
        };
        if let Some(open) = open {
            let _ = open.end.send(end);
        }
    }

    /// Withdraw every review the mount `app` has open.
    pub fn withdraw_app(&self, app: &McpAppRef, cause: McpAppWithdrawal) {
        let ended: Vec<Pending> = {
            let mut state = self.state.lock().expect("app reviews");
            let keys: Vec<u64> = state
                .pending
                .iter()
                .filter(|(_, open)| &open.app == app)
                .map(|(key, _)| *key)
                .collect();
            keys.iter()
                .filter_map(|key| state.pending.remove(key))
                .collect()
        };
        for open in ended {
            let _ = open.end.send(ReviewEnd::Withdrawn(cause));
        }
    }

    /// The conversation ended: withdraw every review open, and open none
    /// again — a call admitted just before cannot leave one behind.
    pub fn end(&self) {
        let ended = {
            let mut state = self.state.lock().expect("app reviews");
            state.ended = true;
            std::mem::take(&mut state.pending)
        };
        for (_, open) in ended {
            let _ = open
                .end
                .send(ReviewEnd::Withdrawn(McpAppWithdrawal::ConversationEnded));
        }
    }
}

/// A new app review's identity.
pub fn new_review_id() -> String {
    format!("{APP_REVIEW_PREFIX}{}", Uuid::new_v4())
}

/// Whether `permission` is an app review's identity, open or not.
fn is_app_review(permission: &str) -> bool {
    permission
        .strip_prefix(APP_REVIEW_PREFIX)
        .is_some_and(|id| Uuid::try_parse(id).is_ok())
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
        end.unwrap_or(ReviewEnd::Withdrawn(McpAppWithdrawal::ConversationEnded))
    }
}

impl Drop for Waiting {
    fn drop(&mut self) {
        // Still waiting: the app's request went before the review ended.
        if self.ended.is_some() {
            self.reviews.end_one(
                &self.permission_id,
                ReviewEnd::Withdrawn(McpAppWithdrawal::RequestCancelled),
            );
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/conversation/app_reviews.rs"]
mod tests;
