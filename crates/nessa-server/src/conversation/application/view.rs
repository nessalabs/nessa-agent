use nessa_sdk::{
    application::agent_execution::providers::{
        CompactionReportingCapability, ElicitationForwardingCapability,
        IncomingElicitationCapability, ModelSwitchReportingCapability,
        NativeHookSuppressionCapability, OperationCapabilities, PermissionDeferralCapability,
        PermissionDenialCapability, PolicyCloseSessionCapability, PolicyEndTurnCapability,
        PreToolPolicyCapability,
    },
    domain::agent_execution::prompts::{ImageReference, LinkedFile},
};
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
    pub lifecycle: ConversationLifecycle,
    pub truncated: bool,
    /// Whether the bounded view contains every currently waiting identity.
    pub queue_complete: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_view_error: Option<String>,
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
    UnsupportedNotImplemented,
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
                IncomingElicitationCapability::UnsupportedNotImplemented => {
                    IncomingElicitationSupport::UnsupportedNotImplemented
                }
                IncomingElicitationCapability::SupportedWithCorrelatedRoundTrip => {
                    IncomingElicitationSupport::SupportedWithCorrelatedRoundTrip
                }
            },
        }
    }
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
