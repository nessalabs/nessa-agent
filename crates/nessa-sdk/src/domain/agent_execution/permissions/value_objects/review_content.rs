//! What a pending review shows a person, held with the review while it waits.
use super::PermissionReviewDeadline;
use std::sync::atomic::{AtomicU64, Ordering};

/// Orders reviews by when they were opened, across every session of this
/// process: a key alone says nothing of order.
static OPENED: AtomicU64 = AtomicU64::new(0);

/// What a person is shown of one pending review: its own key, the tool's
/// title and name, the exact input, and the deadline it was published with.
///
/// The key is new for every review opened, so a review whose identities are
/// reused after a restart is never mistaken for the one before it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewContent {
    key: u128,
    opened: u64,
    title: Option<String>,
    tool_name: String,
    arguments_json: String,
    deadline: Option<PermissionReviewDeadline>,
}
impl ReviewContent {
    /// Content for a review opened now, under a key of its own.
    pub fn new(
        title: Option<String>,
        tool_name: String,
        arguments_json: String,
        deadline: Option<PermissionReviewDeadline>,
    ) -> Self {
        Self {
            key: uuid::Uuid::new_v4().as_u128(),
            opened: OPENED.fetch_add(1, Ordering::Relaxed),
            title,
            tool_name,
            arguments_json,
            deadline,
        }
    }
    /// This review's own key, as 32 lowercase hex digits.
    pub fn key(&self) -> String {
        format!("{:032x}", self.key)
    }
    /// When it was opened, relative to every other review of this process.
    pub fn opened(&self) -> u64 {
        self.opened
    }
    /// The tool's title as the provider observed it, if any.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }
    /// The reviewed tool's exact name.
    pub fn tool_name(&self) -> &str {
        &self.tool_name
    }
    /// The reviewed input, exactly as the agent gave it.
    pub fn arguments_json(&self) -> &str {
        &self.arguments_json
    }
    /// The deadline the review was published with, if it answers itself.
    pub fn deadline(&self) -> Option<&PermissionReviewDeadline> {
        self.deadline.as_ref()
    }
}
