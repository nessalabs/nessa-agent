use nessa_sdk::domain::agent_execution::prompts::{ImageReference, LinkedFile};
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
    /// Questions the agent is waiting on, oldest first.
    pub questions: Vec<ConversationQuestion>,
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
    pub attachments: Vec<ConversationAttachment>,
    pub files: Vec<ConversationLinkedFile>,
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
    pub attachments: Vec<ConversationAttachment>,
    pub files: Vec<ConversationLinkedFile>,
    pub mode: ConversationPendingMode,
}
/// One image a turn referred to. The view never carries its bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationAttachment {
    pub digest: String,
    pub mime_type: String,
    pub size: u64,
}
impl From<&ImageReference> for ConversationAttachment {
    fn from(image: &ImageReference) -> Self {
        Self {
            digest: image.digest().to_string(),
            mime_type: image.media_type().as_str().into(),
            size: image.size(),
        }
    }
}
/// One file a turn pointed the agent at. The view carries the path, because
/// the path is the whole of what the turn carried; nothing was ever read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationLinkedFile {
    pub path: String,
}
impl From<&LinkedFile> for ConversationLinkedFile {
    fn from(file: &LinkedFile) -> Self {
        Self {
            path: file.path().into(),
        }
    }
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
/// One question an agent is waiting on an answer to.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationQuestion {
    pub execution_id: String,
    pub question_id: String,
    /// The agent's own framing of why it is asking.
    pub message: String,
    pub questions: Vec<ConversationAsked>,
}
/// One thing asked, and what may be answered.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationAsked {
    pub key: String,
    pub prompt: String,
    pub header: Option<String>,
    /// Whether several options may be chosen rather than one.
    pub multi_select: bool,
    /// Whether an answer in the answerer's own words is accepted.
    pub free_text: bool,
    pub options: Vec<ConversationAnswerOption>,
}
/// One offered answer: what is recorded, and what is read.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationAnswerOption {
    pub value: String,
    pub label: String,
    pub description: Option<String>,
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
#[serde(rename_all = "camelCase")]
pub struct ConversationCapabilities {
    pub queue: bool,
    pub steer: bool,
    pub resume: bool,
    pub permissions: bool,
    /// The opened agent advertised image input and this gateway can supply the bytes.
    pub image_input: bool,
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
