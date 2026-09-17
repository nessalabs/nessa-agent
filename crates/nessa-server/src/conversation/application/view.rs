use serde::Serialize;

/// A bounded replacement view. Its revision is transient and is not a durable event cursor.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationView {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<ConversationRuntime>,
    pub conversation_id: String,
    pub revision: String,
    pub messages: Vec<ConversationMessage>,
    pub pending: Vec<ConversationPending>,
    pub permissions: Vec<ConversationPermission>,
    pub tools: Vec<ConversationTool>,
    pub capabilities: ConversationCapabilities,
    pub truncated: bool,
    /// Whether the bounded view contains every currently waiting identity.
    pub queue_complete: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_view_error: Option<String>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationMessage {
    pub parts: Vec<ConversationPart>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steering_offset: Option<usize>,
    #[serde(skip)]
    pub event_count: usize,
    pub execution_id: String,
    pub user_text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steering_target: Option<String>,
    pub status: ConversationMessageStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationPending {
    pub execution_id: String,
    pub text: String,
    pub mode: ConversationPendingMode,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationPermission {
    pub execution_id: String,
    pub permission_id: String,
    pub tool_id: String,
    pub title: String,
    pub tool_name: String,
    pub arguments_json: String,
    pub options: Vec<ConversationPermissionOption>,
}
#[derive(Clone, Debug, Serialize)]
pub struct ConversationPermissionOption {
    pub id: String,
    pub label: String,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationTool {
    pub input: String,
    pub details: String,
    pub execution_id: String,
    pub tool_id: String,
    pub title: String,
    pub status: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct ConversationCapabilities {
    pub queue: bool,
    pub steer: bool,
    pub resume: bool,
    pub permissions: bool,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionReceipt {
    pub execution_id: String,
    pub disposition: ConversationDisposition,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationMessageStatus {
    Queued,
    Running,
    Completed,
    Cancelled,
    Failed,
    Injected,
    Unresolved,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationPendingMode {
    Queued,
    Steering,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationDisposition {
    Queued,
    Injected,
    Settled,
}

/// Result of an exact pending-order request; a stale view is safe to refresh.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationReorderOutcome {
    Applied,
    Unchanged,
    QueueChanged,
    PriorityConflict,
}

/// Non-secret runtime facts selected by server composition.
#[derive(Clone, Debug, Serialize)]
pub struct ConversationRuntime {
    pub model: String,
    pub provider: String,
    pub workspace: String,
}

/// Ordered provider observations, addressed by their retained execution-local offset.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationPart {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    pub offset: usize,
    pub kind: String,
    pub text: String,
    pub tool_id: String,
}
