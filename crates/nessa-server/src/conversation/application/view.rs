use crate::agents::domain::AgentId;
use crate::conversation::domain::ConversationApprovalMode;
use nessa_sdk::{
    application::agent_execution::{
        providers::{
            CompactionReportingCapability, ElicitationForwardingCapability,
            IncomingElicitationCapability, ModelSwitchReportingCapability,
            NativeHookSuppressionCapability, OperationCapabilities, PermissionDeferralCapability,
            PermissionDenialCapability, PolicyCloseSessionCapability, PolicyEndTurnCapability,
            PreToolPolicyCapability,
        },
        sessions::CommittedViewState,
    },
    domain::agent_execution::{
        permissions::PermissionEffect,
        prompts::{ImageReference, LinkedFile},
    },
};
use serde::Serialize;

/// Internal fixed selection used to enrich the product view from the same
/// catalog that powers agents.list. It is not an independent wire field.
#[derive(Clone, Debug)]
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
    /// Who asked for the review: the agent, or an MCP App.
    pub origin: ConversationPermissionOrigin,
}
/// Who asked for a review. For [`ConversationPermissionOrigin::Harness`],
/// the review's execution and tool are the agent's call being reviewed; for
/// [`ConversationPermissionOrigin::App`], they are the app — the tool call
/// whose UI it is — and `server` and `tool` the tool it asked to call, `tool`
/// being the review's `tool_name` (`review_of` in `app_reviews` builds both
/// from one name; `tests/conversation/app_reviews.rs` asserts they agree).
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
}
/// The most bytes of a tool's structured result a view carries. The schema
/// states it again; `tests/conversation/agreement.rs` holds the two together.
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
    pub(crate) fn read_only() -> Self {
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
    UnsupportedNotImplemented,
    SupportedForCurrentInvocation
});
support_enum!(PolicyCloseSessionSupport {
    Unknown,
    UnsupportedNotImplemented,
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
                PolicyEndTurnCapability::UnsupportedNotImplemented => {
                    PolicyEndTurnSupport::UnsupportedNotImplemented
                }
                PolicyEndTurnCapability::SupportedForCurrentInvocation => {
                    PolicyEndTurnSupport::SupportedForCurrentInvocation
                }
            },
            policy_close_session: match value.policy_close_session() {
                PolicyCloseSessionCapability::Unknown => PolicyCloseSessionSupport::Unknown,
                PolicyCloseSessionCapability::UnsupportedNotImplemented => {
                    PolicyCloseSessionSupport::UnsupportedNotImplemented
                }
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

/// Non-secret runtime facts selected by server composition.
#[derive(Clone, Debug, Serialize)]
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
    /// Stable identity for a runtime-owned declined-review notice; empty otherwise.
    pub notice_id: String,
}
