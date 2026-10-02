//! The reviews an MCP App's destructive calls wait on (#348): gateway-owned,
//! shown in the conversation's `permissions` beside the agent's, answered
//! through the same `conversation.answer` and `conversation.cancel`, and
//! each ended exactly once — allowed, denied, expired, or withdrawn.
use super::mcp_apps::{McpAppRef, McpAppWithdrawal};
use super::view::{
    ConversationPermission, ConversationPermissionOption, ConversationPermissionOrigin,
};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::sync::oneshot;

/// How long an app's review waits for the person.
pub const APP_REVIEW_DEADLINE: Duration = Duration::from_secs(5 * 60);
/// The option that allows an app's call.
pub const ALLOW: &str = "allow";
/// The option that denies it.
pub const DENY: &str = "deny";

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

struct Pending {
    review: ConversationPermission,
    app: McpAppRef,
    end: oneshot::Sender<ReviewEnd>,
}

/// One conversation's open app reviews.
#[derive(Default)]
pub struct AppReviews {
    pending: Mutex<BTreeMap<String, Pending>>,
    next: AtomicU64,
}

/// One review's wait. Dropping it before the review ended — the app's
/// request went — withdraws the review.
pub struct Waiting {
    reviews: Arc<AppReviews>,
    pub permission_id: String,
    ended: Option<oneshot::Receiver<ReviewEnd>>,
}

impl AppReviews {
    /// Open a review of `app`'s call to `tool` on `server` with
    /// `arguments_json`; it stands, in the view, until it ends.
    pub fn open(
        self: &Arc<Self>,
        app: &McpAppRef,
        server: &str,
        tool: &str,
        arguments_json: &str,
    ) -> Waiting {
        let permission_id = format!("app-{}", self.next.fetch_add(1, Ordering::Relaxed) + 1);
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
        self.pending.lock().expect("app reviews").insert(
            permission_id.clone(),
            Pending {
                review,
                app: app.clone(),
                end,
            },
        );
        Waiting {
            reviews: self.clone(),
            permission_id,
            ended: Some(ended),
        }
    }

    /// The reviews open now, oldest first.
    pub fn reviews(&self) -> Vec<ConversationPermission> {
        self.pending
            .lock()
            .expect("app reviews")
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
        let mut pending = self.pending.lock().expect("app reviews");
        let Some(open) = pending.get(permission) else {
            return if permission.starts_with("app-") {
                ReviewAnswer::Stale
            } else {
                ReviewAnswer::NotAnAppReview
            };
        };
        if open.review.execution_id != execution {
            return ReviewAnswer::Stale;
        }
        let end = match option {
            ALLOW => ReviewEnd::Allowed(by),
            DENY => ReviewEnd::Denied(by),
            _ => return ReviewAnswer::Stale,
        };
        let open = pending.remove(permission).expect("present");
        let _ = open.end.send(end);
        ReviewAnswer::Ended
    }

    /// The person's cancellation of a review: a denial.
    pub fn cancel(&self, execution: &str, permission: &str, by: ReviewAnswerer) -> ReviewAnswer {
        self.answer(execution, permission, DENY, by)
    }

    /// End the review `permission` with `end`, when it is still open.
    fn end(&self, permission: &str, end: ReviewEnd) -> bool {
        let open = self.pending.lock().expect("app reviews").remove(permission);
        open.map(|open| open.end.send(end)).is_some()
    }

    /// Withdraw every review `app` has open.
    pub fn withdraw_app(&self, app: &McpAppRef, cause: McpAppWithdrawal) {
        let ended: Vec<Pending> = {
            let mut pending = self.pending.lock().expect("app reviews");
            let ids: Vec<String> = pending
                .iter()
                .filter(|(_, open)| &open.app == app)
                .map(|(id, _)| id.clone())
                .collect();
            ids.iter().filter_map(|id| pending.remove(id)).collect()
        };
        for open in ended {
            let _ = open.end.send(ReviewEnd::Withdrawn(cause));
        }
    }

    /// Withdraw every review open: the conversation ended.
    pub fn withdraw_all(&self, cause: McpAppWithdrawal) {
        let ended = std::mem::take(&mut *self.pending.lock().expect("app reviews"));
        for (_, open) in ended {
            let _ = open.end.send(ReviewEnd::Withdrawn(cause));
        }
    }
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
                self.reviews.end(&self.permission_id, ReviewEnd::Expired);
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
            self.reviews.end(
                &self.permission_id,
                ReviewEnd::Withdrawn(McpAppWithdrawal::RequestCancelled),
            );
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/conversation/app_reviews.rs"]
mod tests;
