use super::domain::ConversationApprovalMode;
use crate::agents::AgentId;
use nessa_sdk::{
    application::agent_execution::{
        providers::{
            CompactionReportingCapability, ElicitationForwardingCapability,
            IncomingElicitationCapability, ModelSwitchReportingCapability,
            NativeHookSuppressionCapability, OperationCapabilities, PermissionDeferralCapability,
            PermissionDenialCapability, PolicyCloseSessionCapability, PolicyEndTurnCapability,
            PreToolPolicyCapability, ProviderIdentity,
        },
        sessions::{
            ArtifactRecord, CommittedViewState, HandoffKind, HandoffRecord, NoteDelivery,
            NoteRecord, NoteRetirementReason, NoteState, SessionSnapshot,
        },
    },
    domain::agent_execution::{
        permissions::PermissionEffect,
        prompts::{ImageReference, LinkedFile, MessageSender, UserMessage},
    },
};
use serde::Serialize;

/// Current confirmed selection used to enrich the product view from the same
/// catalog that powers agents.list. Equality lets Projection own replacements.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConversationSelectionView {
    pub agent: AgentId,
    pub model: String,
    pub approval_mode: ConversationApprovalMode,
}

/// A bounded replacement view. Its revision is transient and is not a durable event cursor.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationView {
    #[serde(skip_serializing)]
    pub selection: Option<ConversationSelectionView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_mode_change: Option<ConversationApprovalModeChangeView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_change: Option<ConversationModelChangeView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_change_effect: Option<ConversationModelChangeEffect>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<ConversationRuntime>,
    pub conversation_id: String,
    pub revision: String,
    pub messages: Vec<ConversationMessage>,
    pub pending: Vec<ConversationPending>,
    /// Every review waiting on a person now, as the live review owners hold
    /// them, oldest first: a small entry of fixed size each, never given up
    /// for the view's size. What each shows is read with
    /// `conversation.permissionReview`.
    pub permissions: Vec<ConversationReviewEntry>,
    /// Questions the agent is waiting on, oldest first.
    pub questions: Vec<ConversationQuestionEntry>,
    pub tools: Vec<ConversationTool>,
    pub capabilities: ConversationCapabilities,
    pub lifecycle: ConversationLifecycle,
    pub truncated: bool,
    /// Whether the bounded view contains every currently waiting identity.
    pub queue_complete: bool,
    /// Completeness of the committed transcript independently of display limits.
    pub transcript_state: ConversationTranscriptState,
    /// Why whole pending interactions were omitted from the bounded display.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interaction_view_error: Option<String>,
    /// The name the gateway gave the conversation from its first message —
    /// the same title `conversation.list` shows, from the same summary — or
    /// `None` before anything was said. Always on the wire, as `null` then.
    pub title: Option<String>,
    /// The conversation's latest lease: where its agent runs and under what
    /// limits, or why it could not. Absent before any lease was recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lease: Option<ConversationLeaseView>,
    /// Artifacts the conversation recorded, each held when it was recorded,
    /// the newest [`MAX_VIEW_ARTIFACTS`](super::projection::MAX_VIEW_ARTIFACTS)
    /// in the order they were recorded. Empty before any was recorded. A
    /// record is history: it stays when the hold is let go, as closing the
    /// conversation does, and whether the bytes are still held is asked
    /// when they are fetched.
    pub artifacts: Vec<ConversationArtifact>,
    /// Notes left in the conversation, the newest
    /// [`MAX_VIEW_NOTES`](super::projection::MAX_VIEW_NOTES) in the order
    /// they were left. Empty before any was left.
    pub notes: Vec<ConversationNote>,
    /// Switches and the fork the conversation began as, in the order they
    /// were recorded, at most [`MAX_VIEW_HANDOFFS`](super::projection::MAX_VIEW_HANDOFFS),
    /// the newest. Empty before any.
    pub handoffs: Vec<ConversationHandoff>,
    /// Records this read could not apply, oldest first, at most 32. Empty
    /// when every record could be read. Always sent, including when empty,
    /// and a chat with no turns may still list these.
    #[serde(default)]
    pub unreadable: Vec<ConversationUnreadable>,
    /// More records could not be read than [`Self::unreadable`] lists. The
    /// transcript ends with one line for that. Always sent.
    pub unreadable_truncated: bool,
}

/// One saved record the fold could not read. `id` is its physical position.
/// `after_execution_id` is the last turn accepted when it was skipped.
///
/// The line a transcript draws for this is where a future report action
/// attaches. This view does not carry a report control.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationUnreadable {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_execution_id: Option<String>,
}

/// Product status of the last physical committed read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationTranscriptState {
    NotLoaded,
    Partial,
    CompleteEmpty,
    Complete,
    Stale,
    Unknown,
}
impl From<CommittedViewState> for ConversationTranscriptState {
    fn from(state: CommittedViewState) -> Self {
        match state {
            CommittedViewState::NotLoaded => Self::NotLoaded,
            CommittedViewState::Partial => Self::Partial,
            CommittedViewState::CompleteEmpty => Self::CompleteEmpty,
            CommittedViewState::Complete => Self::Complete,
            CommittedViewState::Stale => Self::Stale,
            CommittedViewState::Unknown => Self::Unknown,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationApprovalModeChangeView {
    pub request_id: String,
    pub requested_mode: String,
    pub status: ConversationApprovalModeChangeStatus,
}
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationApprovalModeChangeStatus {
    Changing,
    RecoveryRequired,
}
/// An acknowledged model change follows the native harness boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ConversationModelChangeEffect {
    /// Claude uses the selection for subsequent responses.
    SubsequentResponses,
    /// Codex snapshots the selection when the next native turn starts.
    NextTurn,
}
/// The pending request is separate from the last durably confirmed selection.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationModelChangeView {
    pub request_id: String,
    pub requested_model: String,
    pub status: ConversationModelChangeStatus,
}
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationModelChangeStatus {
    Changing,
    RecoveryRequired,
}
/// Current provider attachment lifecycle. It grants no operation authority.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationLifecycle {
    pub phase: ConversationLifecyclePhase,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<ConversationStartupFailure>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence_failure: Option<ConversationAttachmentEvidenceFailure>,
}
/// Bounded diagnostic for late mandatory attachment-audit evidence failure.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationAttachmentEvidenceFailure {
    pub code: ConversationAttachmentEvidenceFailureCode,
    pub message: String,
}
/// The only cause of phase-independent attachment evidence failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationAttachmentEvidenceFailureCode {
    Audit,
}
/// Product-facing provider attachment phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationLifecyclePhase {
    Absent,
    Starting,
    Attached,
    Failed,
}
/// Bounded diagnostic for the latest failed provider attachment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationStartupFailure {
    pub code: ConversationStartupFailureCode,
    pub message: String,
}
/// Stable category for a provider attachment failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationStartupFailureCode {
    Audit,
    Provider,
    Storage,
    Cleanup,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationMessage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authentication_required: Option<bool>,
    /// This failed turn also failed independent required work, beside an
    /// authentication refusal or a usage limit. The person-facing diagnostic
    /// stays in `error`. Absent for a pure refusal or a pure usage limit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required_work_failed: Option<bool>,
    pub parts: Vec<ConversationPart>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steering_offset: Option<usize>,
    #[serde(skip)]
    pub event_count: usize,
    /// Byte length of `parts[*].text`, updated when that text is set, replaced, or removed.
    /// `the_text_budget_keeps_a_running_total`.
    #[serde(skip)]
    pub retained_text: usize,
    pub execution_id: String,
    pub user_text: String,
    pub attachments: Vec<ConversationAttachment>,
    pub files: Vec<ConversationLinkedFile>,
    /// The app that wrote this turn on the person's behalf; absent when the
    /// person wrote it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<ConversationMessageApp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steering_target: Option<String>,
    pub status: ConversationMessageStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// A provider usage limit on this failed turn: which harness, how it can
    /// end, and the reset instant when the harness named a trustworthy one.
    /// The projection publishes those facts from the saved error. Whether a
    /// continuation is scheduled is added when the gateway publishes the view.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_limit: Option<ConversationUsageLimit>,
}

/// Which harness reported a usage limit. OpenCode is not a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationUsageLimitProvider {
    /// Claude ACP.
    Claude,
    /// Codex ACP.
    Codex,
}

/// How a published usage limit can end.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationUsageLimitRecovery {
    /// `resets_at_ms` is an absolute Unix instant in milliseconds.
    TimedReset,
    /// The account needs a person. No reset is published.
    UserAction,
    /// A usage limit with no trustworthy reset instant.
    Unknown,
}

/// The usage-limit facts a transcript card reads. `auto_resuming` is absent
/// on the projection and set only when a schedule has been accepted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationUsageLimit {
    pub provider: ConversationUsageLimitProvider,
    pub recovery: ConversationUsageLimitRecovery,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_resuming: Option<bool>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationPending {
    pub execution_id: String,
    pub text: String,
    pub attachments: Vec<ConversationAttachment>,
    pub files: Vec<ConversationLinkedFile>,
    /// As [`ConversationMessage::app`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<ConversationMessageApp>,
    pub mode: ConversationPendingMode,
}
/// The MCP App that wrote a turn on the person's behalf: the tool call whose
/// UI it is, and the server and tool that call was to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationMessageApp {
    pub execution_id: String,
    pub tool_id: String,
    pub server: String,
    pub tool: String,
}
impl ConversationMessageApp {
    /// The app that wrote `message`, or `None` when the person did.
    pub fn of(message: &UserMessage) -> Option<Self> {
        match message.sender() {
            MessageSender::Person => None,
            MessageSender::App(app) => Some(Self {
                execution_id: app.execution_id().as_str().into(),
                tool_id: app.tool_id().as_str().into(),
                server: app.tool().server().into(),
                tool: app.tool().tool().into(),
            }),
        }
    }
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
/// One review waiting on a person, as a view lists it: its key and who asked,
/// with the start of the tool's name to say what it is while the rest is
/// read. Its encoding never exceeds `MAX_REVIEW_ENTRY_BYTES`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationReviewEntry {
    /// The review's own key, 32 lowercase hex digits: new for every review
    /// opened, so a review whose identities are reused is a new entry.
    pub review: String,
    pub origin: ConversationReviewKind,
    /// The start of the reviewed tool's name, clipped so its encoding takes
    /// at most `MAX_REVIEW_ENTRY_TOOL_NAME_BYTES`.
    pub tool_name: String,
}
/// Who asked for a review an entry lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ConversationReviewKind {
    Harness,
    App,
}
/// A pending review with all it shows: what `conversation.permissionReview`
/// reads for one entry of the view.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationPermission {
    /// The key the view lists it under: an answer names it, so a review
    /// whose identities are reused is never answered by mistake.
    pub review: String,
    pub execution_id: String,
    pub permission_id: String,
    pub tool_id: String,
    pub title: String,
    pub tool_name: String,
    /// The reviewed input, exactly as the agent gave it.
    pub arguments_json: String,
    pub options: Vec<ConversationPermissionOption>,
    /// Who asked for the review: the agent, or an MCP App.
    pub origin: ConversationPermissionOrigin,
    /// What the review asks the person to allow. The agent's reviews ask a
    /// tool; an app's say what the gateway's `ReviewAsk` opened them for.
    pub ask: ConversationPermissionAsk,
    /// Present when this review answers itself if nobody does.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deadline: Option<ConversationPermissionDeadline>,
}
/// What happens if nobody answers a review, and when.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationPermissionDeadline {
    pub wait_ms: u64,
    /// Unix milliseconds when the deadline decides. A reloaded client counts
    /// down from this, not from `wait_ms` measured again at reload.
    pub due_at_ms: u64,
    pub effect: ConversationPermissionOptionEffect,
    /// Past-tense reason recorded if the deadline wins. The pending card does
    /// not show this sentence.
    pub reason: String,
    /// The only refusal offered stops the agent's turn.
    pub stops_turn: bool,
}
/// What a review asks the person to allow: running a tool, or an MCP App
/// sending one message in the conversation as them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationPermissionAsk {
    Tool,
    Message,
}
/// Who asked for a review. For [`ConversationPermissionOrigin::Harness`],
/// the review's execution and tool are the agent's call being reviewed; for
/// [`ConversationPermissionOrigin::App`], they are the app — the tool call
/// whose UI it is — and `server` and `tool` the tool it asked to call, `tool`
/// being the review's `tool_name` (`review_of` in `app_reviews` builds both
/// from one name; `nessa-server/tests/conversation/app_reviews.rs` asserts they agree).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ConversationPermissionOrigin {
    Harness,
    App { server: String, tool: String },
}
#[derive(Clone, Debug, Serialize)]
pub struct ConversationPermissionOption {
    pub id: String,
    pub label: String,
    /// What choosing it decides, as the domain classified the offer: a surface
    /// picks an option by this, never by its label or identifier.
    pub effect: ConversationPermissionOptionEffect,
}
/// Whether an option allows or denies the reviewed request. Only an option
/// deciding that one request is published: the projection offers no review
/// with another (`projection.rs`, "a review reaching beyond its request").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationPermissionOptionEffect {
    Allow,
    Deny,
}
impl From<PermissionEffect> for ConversationPermissionOptionEffect {
    fn from(effect: PermissionEffect) -> Self {
        match effect {
            PermissionEffect::Allow => Self::Allow,
            PermissionEffect::Deny => Self::Deny,
        }
    }
}
/// One question waiting on a person, as a view lists it: its key, and the
/// start of the agent's message to say what it is while the rest is read.
/// Its encoding never exceeds `MAX_QUESTION_ENTRY_BYTES`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationQuestionEntry {
    /// The question's own key, 32 lowercase hex digits: new for every
    /// question asked, so one whose identities are reused is a new entry.
    pub question: String,
    /// The start of the agent's message, clipped so its encoding takes at
    /// most `MAX_QUESTION_ENTRY_MESSAGE_BYTES`.
    pub message: String,
}
/// One question an agent is waiting on an answer to, with all it asks.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationQuestion {
    /// The key the view lists it under: an answer names it.
    pub question: String,
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
    /// Whether an answer must include this question; declining the whole ask
    /// is still possible.
    pub required: bool,
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
    /// What the call does, as the provider categorised it. Empty until it says.
    pub kind: String,
    pub status: String,
    /// The MCP server and tool the call went to, once the harness named them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp: Option<ConversationMcpTool>,
    /// The call's structured result as JSON text, when it was reported and fits
    /// in [`MAX_STRUCTURED_CONTENT_BYTES`]; its text stays in `details`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub structured_content: Option<String>,
    /// The hooks choice of the process start this call ran under, read from
    /// the call's saved events. Absent when that start recorded none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_hooks: Option<NativeHooksChoice>,
    /// When the session first heard of the call, in Unix milliseconds.
    /// Absent for a call saved before the time was recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at_ms: Option<u64>,
    /// When the session heard the call finished or failed, in Unix
    /// milliseconds. Absent while it runs, and for a call saved before the
    /// time was recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at_ms: Option<u64>,
    /// The files the call changed, as the agent reported them, in its order:
    /// at most [`MAX_TOOL_CHANGES`]. The same changes are written into
    /// `details` as text; these are for a reader that draws them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<ConversationToolChange>,
}
/// The most file changes one tool call lists. The schema states it again;
/// `nessa-server/tests/conversation/agreement.rs` holds the two together.
pub const MAX_TOOL_CHANGES: usize = 16;
/// The most bytes of one side of a file change a view carries, before or
/// after. Past it the side is cut at a character boundary and the change says
/// so (`cut`). The schema states it again; the agreement test holds the two
/// together.
pub const MAX_TOOL_CHANGE_SIDE_BYTES: usize = 7000;
/// The most bytes of a changed file's path a view carries; a longer one is
/// cut. Stated again by the schema, held to it by the agreement test.
pub const MAX_TOOL_CHANGE_PATH_BYTES: usize = 4096;
/// One file a tool call changed: its path, and its text before and after.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ConversationToolChange {
    /// The file's path as the agent named it, unresolved, at most
    /// [`MAX_TOOL_CHANGE_PATH_BYTES`].
    pub path: String,
    /// The text before the change. Absent when the agent did not say — a
    /// file it created reads that way — which differs from an empty file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    /// The text after the change, which may be empty.
    pub after: String,
    /// A side was longer than [`MAX_TOOL_CHANGE_SIDE_BYTES`] and is cut.
    pub cut: bool,
}
/// The most bytes of a tool's structured result a view carries. The schema
/// states it again; `nessa-server/tests/conversation/agreement.rs` holds the two together.
pub const MAX_STRUCTURED_CONTENT_BYTES: usize = 16384;
/// An MCP tool's identity, copied from the SDK's validated `McpTool`, and the
/// UI resource the tool declared, as its server last listed it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ConversationMcpTool {
    pub server: String,
    pub tool: String,
    /// The tool's `ui://` resource, filled when the view is read
    /// ([`McpToolUis`](super::McpToolUis)); absent when the tool has none or
    /// it is not known.
    #[serde(rename = "resourceUri", skip_serializing_if = "Option::is_none")]
    pub resource_uri: Option<String>,
    /// The call's arguments, one JSON object encoded, as the gateway's MCP
    /// connection saw them. At most
    /// [`MAX_MCP_ARGUMENTS_BYTES`](nessa_sdk::domain::agent_execution::tools::MAX_MCP_ARGUMENTS_BYTES),
    /// the bound the schema publishes. Absent when the call did not go
    /// through that connection, the harness named no call id it could match,
    /// or the arguments did not fit — never cut.
    #[serde(rename = "argumentsJson", skip_serializing_if = "Option::is_none")]
    pub arguments_json: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationCapabilities {
    pub queue: bool,
    pub steer: bool,
    pub resume: bool,
    pub permissions: bool,
    /// The opened agent advertised image input and this gateway can supply the bytes.
    pub image_input: bool,
    pub agent_features: ConversationAgentFeatures,
}

impl ConversationCapabilities {
    pub fn read_only() -> Self {
        Self {
            queue: false,
            steer: false,
            resume: false,
            permissions: false,
            image_input: false,
            agent_features: OperationCapabilities::default().into(),
        }
    }
}

/// User-visible support facts for provider transport and Nessa policy integration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationAgentFeatures {
    pub permission_denial: PermissionDenialSupport,
    pub native_hook_suppression: NativeHookSuppressionSupport,
    pub native_hooks: NativeHooksChoice,
    pub compaction_reporting: CompactionReportingSupport,
    pub model_switch_reporting: ModelSwitchReportingSupport,
    pub permission_deferral: PermissionDeferralSupport,
    pub elicitation_forwarding: ElicitationForwardingSupport,
    pub pre_tool_policy: PreToolPolicySupport,
    pub policy_end_turn: PolicyEndTurnSupport,
    pub policy_close_session: PolicyCloseSessionSupport,
    pub incoming_elicitation: IncomingElicitationSupport,
}

macro_rules! support_enum {
    ($name:ident { $($variant:ident),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $($variant),+ }
    };
}

support_enum!(PermissionDenialSupport {
    Unknown,
    Unsupported,
    SupportedForOfferedPermissionReviews
});
support_enum!(NativeHookSuppressionSupport {
    Unknown,
    Unsupported,
    SupportedForUserConfiguredHooks
});
support_enum!(NativeHooksChoice { Off, On });
support_enum!(CompactionReportingSupport {
    UnsupportedNotImplemented,
    SupportedWithInvocationCorrelation
});
support_enum!(ModelSwitchReportingSupport {
    UnsupportedNotImplemented,
    SupportedAfterValidatedSwitch
});
support_enum!(PermissionDeferralSupport {
    UnsupportedNotImplemented,
    Unsupported,
    SupportedWithNonterminalOutcome
});
support_enum!(ElicitationForwardingSupport {
    Unknown,
    Unsupported,
    SupportedWithCorrelatedRoundTrip
});
support_enum!(PreToolPolicySupport {
    Unknown,
    UnsupportedNotImplemented,
    SupportedAtPermissionGate
});
support_enum!(PolicyEndTurnSupport {
    Unknown,
    Unsupported,
    SupportedForCurrentInvocation
});
support_enum!(PolicyCloseSessionSupport {
    Unknown,
    Unsupported,
    SupportedForSession
});
support_enum!(IncomingElicitationSupport {
    Unknown,
    Unsupported,
    SupportedWithCorrelatedRoundTrip
});

impl From<OperationCapabilities> for ConversationAgentFeatures {
    fn from(value: OperationCapabilities) -> Self {
        Self {
            permission_denial: match value.permission_denial() {
                PermissionDenialCapability::Unknown => PermissionDenialSupport::Unknown,
                PermissionDenialCapability::Unsupported => PermissionDenialSupport::Unsupported,
                PermissionDenialCapability::SupportedForOfferedPermissionReviews => {
                    PermissionDenialSupport::SupportedForOfferedPermissionReviews
                }
            },
            native_hooks: if value.native_hooks_allowed() {
                NativeHooksChoice::On
            } else {
                NativeHooksChoice::Off
            },
            native_hook_suppression: match value.native_hook_suppression() {
                NativeHookSuppressionCapability::Unknown => NativeHookSuppressionSupport::Unknown,
                NativeHookSuppressionCapability::Unsupported => {
                    NativeHookSuppressionSupport::Unsupported
                }
                NativeHookSuppressionCapability::SupportedForUserConfiguredHooks => {
                    NativeHookSuppressionSupport::SupportedForUserConfiguredHooks
                }
            },
            compaction_reporting: match value.compaction_reporting() {
                CompactionReportingCapability::UnsupportedNotImplemented => {
                    CompactionReportingSupport::UnsupportedNotImplemented
                }
                CompactionReportingCapability::SupportedWithInvocationCorrelation => {
                    CompactionReportingSupport::SupportedWithInvocationCorrelation
                }
            },
            model_switch_reporting: match value.model_switch_reporting() {
                ModelSwitchReportingCapability::UnsupportedNotImplemented => {
                    ModelSwitchReportingSupport::UnsupportedNotImplemented
                }
                ModelSwitchReportingCapability::SupportedAfterValidatedSwitch => {
                    ModelSwitchReportingSupport::SupportedAfterValidatedSwitch
                }
            },
            permission_deferral: match value.permission_deferral() {
                PermissionDeferralCapability::UnsupportedNotImplemented => {
                    PermissionDeferralSupport::UnsupportedNotImplemented
                }
                PermissionDeferralCapability::Unsupported => PermissionDeferralSupport::Unsupported,
                PermissionDeferralCapability::SupportedWithNonterminalOutcome => {
                    PermissionDeferralSupport::SupportedWithNonterminalOutcome
                }
            },
            elicitation_forwarding: match value.elicitation_forwarding() {
                ElicitationForwardingCapability::Unknown => ElicitationForwardingSupport::Unknown,
                ElicitationForwardingCapability::Unsupported => {
                    ElicitationForwardingSupport::Unsupported
                }
                ElicitationForwardingCapability::SupportedWithCorrelatedRoundTrip => {
                    ElicitationForwardingSupport::SupportedWithCorrelatedRoundTrip
                }
            },
            pre_tool_policy: match value.pre_tool_policy() {
                PreToolPolicyCapability::Unknown => PreToolPolicySupport::Unknown,
                PreToolPolicyCapability::UnsupportedNotImplemented => {
                    PreToolPolicySupport::UnsupportedNotImplemented
                }
                PreToolPolicyCapability::SupportedAtPermissionGate => {
                    PreToolPolicySupport::SupportedAtPermissionGate
                }
            },
            policy_end_turn: match value.policy_end_turn() {
                PolicyEndTurnCapability::Unknown => PolicyEndTurnSupport::Unknown,
                PolicyEndTurnCapability::Unsupported => PolicyEndTurnSupport::Unsupported,
                PolicyEndTurnCapability::SupportedForCurrentInvocation => {
                    PolicyEndTurnSupport::SupportedForCurrentInvocation
                }
            },
            policy_close_session: match value.policy_close_session() {
                PolicyCloseSessionCapability::Unknown => PolicyCloseSessionSupport::Unknown,
                PolicyCloseSessionCapability::Unsupported => PolicyCloseSessionSupport::Unsupported,
                PolicyCloseSessionCapability::SupportedForSession => {
                    PolicyCloseSessionSupport::SupportedForSession
                }
            },
            incoming_elicitation: match value.incoming_elicitation() {
                IncomingElicitationCapability::Unknown => IncomingElicitationSupport::Unknown,
                IncomingElicitationCapability::Unsupported => {
                    IncomingElicitationSupport::Unsupported
                }
                IncomingElicitationCapability::SupportedWithCorrelatedRoundTrip => {
                    IncomingElicitationSupport::SupportedWithCorrelatedRoundTrip
                }
            },
        }
    }
}
/// The conversations one list names, and whether they are all of them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConversationList {
    /// Most recently updated first, at most
    /// [`MAX_LISTED_CONVERSATIONS`](super::MAX_LISTED_CONVERSATIONS).
    pub conversations: Vec<ConversationListEntry>,
    /// Whether these are every conversation the caller has under the list's
    /// filter: `false` when the bound left some out, or when a stored record
    /// or summary of the caller's own could not be read back, which lasts
    /// until an operator repairs it. Nobody else's conversations are read to
    /// answer it, so nobody else's damage makes it `false`. A conversation
    /// missing from a complete list is not there under that filter: deleted,
    /// under the other filter, or one the gateway has no summary for (nothing
    /// was said in it, or its summary was never written).
    pub complete: bool,
}
/// One page of an observation pass, and whether the pass is finished.
///
/// [`ConversationList`] stays the newest-first bound. This is the creation-order
/// page a caller walks when that list is not the whole catalogue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConversationObservation {
    pub conversations: Vec<ConversationListEntry>,
    /// True only when this page finishes the pass.
    pub complete: bool,
    /// Where the next page resumes. Absent when the pass is finished, or when
    /// this page cannot resume.
    pub cursor: Option<ConversationObservationCursor>,
}
/// The cursor an observation pass echoes. Creation order, not newest-first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConversationObservationCursor {
    pub incarnation: String,
    pub boundary: u64,
    pub creation: u64,
    pub id: String,
}
/// One conversation as a row in a list of them: what it is called, the last
/// thing said in it, and when. The product boundary maps it to the wire.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConversationListEntry {
    pub conversation_id: String,
    pub title: Option<String>,
    pub preview: Option<String>,
    pub created_at_ms: u64,
    /// When something was last said in it; its creation time until then.
    pub updated_at_ms: u64,
    /// Open on this gateway with a turn under way.
    pub running: bool,
    /// Somebody archived it, and nothing has been said in it since.
    pub archived: bool,
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

/// The conversation's latest lease as its records fold. A lease of a kind
/// this build cannot read is shown as [`ConversationLeaseState::Unreadable`]
/// with nothing else claimed about it, rather than failing the view.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationLeaseView {
    pub state: ConversationLeaseState,
    /// Which issuance of this conversation's leases it is. Absent for an
    /// unreadable lease, which does not say.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment: Option<ConversationLeaseEnvironment>,
    /// The environment's name when it is a machine of its own: its SSH
    /// destination, or a paired environment's `peer:<key>`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<ConversationLeaseSandbox>,
    /// Why it is ending or ended: the first cause recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cause: Option<ConversationLeaseCause>,
    /// What the environment reported releasing, once it has.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cleanup: Option<ConversationLeaseCleanup>,
    /// Why it was refused, when it was.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<ConversationLeaseRefusal>,
    /// Events that arrived after it ended and were dropped.
    pub dropped_events: u64,
    /// Command leases running under it now (row L14); absent when none is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commands: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationLeaseState {
    Live,
    Ending,
    Ended,
    Interrupted,
    Refused,
    Unreadable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationLeaseEnvironment {
    /// The gateway's own machine.
    Here,
    /// A machine reached over SSH, named by `host`.
    Ssh,
    /// A paired environment that dials in, named by `host`.
    Peer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationLeaseSandbox {
    /// Whatever the agent's harness encloses by default; nothing more.
    HarnessDefault,
    /// Nothing encloses the work: it runs as the environment's account.
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationLeaseCause {
    Stopped,
    Closed,
    Revoked,
    Expired,
    Lost,
    Idle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationLeaseCleanup {
    Confirmed,
    Forced,
    NotHeld,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationLeaseRefusal {
    SandboxUnavailable,
    EnvironmentUnreachable,
    EnvironmentVersionMismatch,
    EnvironmentBusy,
    AgentUnavailable,
    EnvironmentPlatformUnsupported,
    EnvironmentInstallFailed,
}

/// One artifact the conversation recorded: a file its agent published under
/// a lease, collected, verified by its digest and held when it was recorded.
/// The record outlives the hold.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationArtifact {
    /// The turn that was running when it was recorded, if one was.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_id: Option<String>,
    /// The lease it was published under.
    pub lease: String,
    /// The name it was published under, for people only.
    pub name: String,
    /// SHA-256 of its bytes: `sha256:` and 64 lowercase hexadecimal digits.
    pub digest: String,
    /// What it was declared as.
    pub mime_type: String,
    /// Its length in bytes.
    pub size: u64,
}
impl From<&ArtifactRecord> for ConversationArtifact {
    fn from(record: &ArtifactRecord) -> Self {
        Self {
            execution_id: record.turn.as_ref().map(|turn| turn.as_str().into()),
            lease: record.lease.as_str().into(),
            name: record.name.as_str().into(),
            digest: record.file.digest().to_string(),
            mime_type: record.file.media_type().as_str().into(),
            size: record.file.size(),
        }
    }
}

/// One note left in the conversation, as everyone who reads it sees it: who
/// left it, what it says, and where it stands.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationNote {
    pub message_id: String,
    pub text: String,
    pub accepted_at_ms: u64,
    pub delivery: ConversationNoteDelivery,
    pub delivery_state: ConversationNoteState,
    /// The turn that took it, when one did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_id: Option<String>,
    /// Why it was retired, when it was.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retired_reason: Option<ConversationNoteRetiredReason>,
    /// The last turn accepted before it, which it is shown after.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_execution_id: Option<String>,
    pub author: ConversationNoteAuthor,
}
impl ConversationNote {
    /// `note` as `snapshot`, which holds it, says it stands.
    pub fn of(snapshot: &SessionSnapshot, note: &NoteRecord) -> Self {
        let state = snapshot.note_state(note);
        Self {
            message_id: note.id.as_str().into(),
            text: note.text.as_str().into(),
            accepted_at_ms: note.accepted_at_ms,
            delivery: match note.delivery {
                NoteDelivery::RecordOnly => ConversationNoteDelivery::RecordOnly,
                NoteDelivery::NextTurn => ConversationNoteDelivery::NextTurn,
            },
            delivery_state: match &state {
                NoteState::Recorded => ConversationNoteState::Recorded,
                NoteState::Pending => ConversationNoteState::Pending,
                NoteState::Assigned(_) => ConversationNoteState::Assigned,
                NoteState::Retired(_) => ConversationNoteState::Retired,
            },
            execution_id: match &state {
                NoteState::Assigned(turn) => Some(turn.as_str().into()),
                _ => None,
            },
            retired_reason: match &state {
                NoteState::Retired(retirement) => Some(match retirement.reason {
                    NoteRetirementReason::Expired => ConversationNoteRetiredReason::Expired,
                    NoteRetirementReason::PermissionLost => {
                        ConversationNoteRetiredReason::PermissionLost
                    }
                }),
                _ => None,
            },
            after_execution_id: note
                .after_execution
                .as_ref()
                .map(|turn| turn.as_str().into()),
            author: ConversationNoteAuthor {
                kind: match note.author {
                    nessa_sdk::domain::agent_execution::executions::Initiator::Owner => {
                        ConversationNoteAuthorKind::Owner
                    }
                    nessa_sdk::domain::agent_execution::executions::Initiator::Grant(_) => {
                        ConversationNoteAuthorKind::Grant
                    }
                },
                surface: note.actor.surface_id().into(),
            },
        }
    }
}
/// What a note was left for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationNoteDelivery {
    RecordOnly,
    NextTurn,
}
/// Where a note stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationNoteState {
    Recorded,
    Pending,
    Assigned,
    Retired,
}
/// Why a note was retired.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationNoteRetiredReason {
    Expired,
    PermissionLost,
}
/// A point after which the conversation's agent ran in a native session that
/// had not seen the turns before it: a switch to another agent or model, or
/// the fork the conversation began as. The first turn after it was given
/// those turns as text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationHandoff {
    pub handoff_id: String,
    pub kind: ConversationHandoffKind,
    pub at_ms: u64,
    /// The last turn the conversation held when it was recorded, which it is
    /// shown after; absent before any turn, as a fork always is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_execution_id: Option<String>,
    /// A switch's agent and model before and after it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<ConversationHandoffAgent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<ConversationHandoffAgent>,
    /// A fork's copied turns, oldest first; empty for a switch.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub turns: Vec<ConversationCarriedTurn>,
    /// Copied turns left out: by the copy's own bounds, or by this view's.
    #[serde(skip_serializing_if = "is_zero")]
    pub omitted_turns: u32,
}
fn is_zero(value: &u32) -> bool {
    *value == 0
}
impl ConversationHandoff {
    /// `handoff` as a view shows it.
    pub fn of(handoff: &HandoffRecord) -> Self {
        let agent = |identity: &ProviderIdentity| ConversationHandoffAgent {
            agent: AgentId::of_provider(identity.name()).map(|agent| agent.name().into()),
            model: identity.model_id().into(),
        };
        let (kind, from, to, turns, omitted_turns) = match &handoff.kind {
            HandoffKind::Switch { before, after } => (
                ConversationHandoffKind::Switch,
                Some(agent(before)),
                Some(agent(after)),
                Vec::new(),
                0,
            ),
            HandoffKind::Fork { history, .. } => (
                ConversationHandoffKind::Fork,
                None,
                None,
                history
                    .turns()
                    .iter()
                    .map(|turn| ConversationCarriedTurn {
                        said: turn.said().map(Into::into),
                        reply: turn.reply().map(Into::into),
                    })
                    .collect(),
                history.omitted(),
            ),
        };
        Self {
            handoff_id: handoff.id.as_str().into(),
            kind,
            at_ms: handoff.at_ms,
            after_execution_id: handoff
                .after_execution
                .as_ref()
                .map(|turn| turn.as_str().into()),
            from,
            to,
            turns,
            omitted_turns,
        }
    }
}
/// Which handoff it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationHandoffKind {
    Switch,
    Fork,
}
/// An agent and model a switch left or went to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationHandoffAgent {
    /// The agent, when the history's provider is one Nessa has.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    pub model: String,
}
/// One turn a fork copied: what was said, and the agent's final reply.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationCarriedTurn {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub said: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply: Option<String>,
}

/// Who left a note: under which authority, from which surface.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationNoteAuthor {
    pub kind: ConversationNoteAuthorKind,
    pub surface: String,
}
/// Under which authority a note was left.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationNoteAuthorKind {
    Owner,
    Grant,
}

/// Non-secret runtime facts selected by server composition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ConversationRuntime {
    pub model: String,
    pub provider: String,
    pub workspace: String,
}

/// Ordered provider output and local notices at their retained execution-local offsets.
/// A tool call is one part, at its first update's offset; its state is its `tools` entry
/// (`tests/conversation/tool_parts.rs`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationPart {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    pub offset: usize,
    pub kind: String,
    pub text: String,
    pub tool_id: String,
    /// Decimal identity of a local notice; empty otherwise.
    ///
    /// A review notice uses the declined-review id. A hook notice and a
    /// deadline notice use the permission id. Those identities are
    /// independent, so one decimal may name each of them.
    /// [`notice_kind`](Self::notice_kind) says which decision it is.
    pub notice_id: String,
    /// `review`, `hook`, or `deadline` on a local notice; empty on every other part.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub notice_kind: String,
}
