//! Generated from protocol/product/v1.json. Do not edit.
//! Bounds are validated at the transport boundary; these are payload types only.
#![allow(dead_code)]
use crate::product_contract::generated::SessionCloseReason;
use nessa_auth::application::dto::{
    CredentialGrantDto, CredentialMetadataDto, MembershipInputDto, PrincipalInputDto,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionChallenge {
    pub min_version: u64,
    pub max_version: u64,
    pub nonce: String,
    pub expires_at: u64,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProductClientMetadata {
    pub id: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionAuthenticateParams {
    pub min_version: u64,
    pub max_version: u64,
    pub nonce: String,
    pub credential: String,
    pub client: ProductClientMetadata,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProductSessionReady {
    pub version: u64,
    pub gateway_id: String,
    pub principal_id: String,
    pub organization_id: String,
    pub membership_id: String,
    pub credential_id: String,
    pub audience_id: String,
    pub expires_at: Option<u64>,
    pub grants: Vec<CredentialGrantDto>,
    pub methods: Vec<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CredentialIssueParams {
    pub request_id: String,
    pub principal: PrincipalInputDto,
    pub membership: MembershipInputDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    pub grants: Vec<CredentialGrantDto>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CredentialListParams {}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CredentialRevokeParams {
    pub request_id: String,
    pub credential_id: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IssuedCredentialResult {
    pub credential: CredentialMetadataDto,
    pub secret: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExistingCredentialResult {
    pub credential: CredentialMetadataDto,
    pub secret_unavailable: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CredentialListResult {
    pub credentials: Vec<CredentialMetadataDto>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CredentialRevokeResult {
    pub credential_id: String,
    pub revision: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PrincipalKind {
    Human,
    Integration,
    Agent,
}
impl PrincipalKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::Integration => "integration",
            Self::Agent => "agent",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MembershipRole {
    Admin,
    Member,
}
impl MembershipRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::Member => "member",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MembershipState {
    Active,
    Disabled,
}
impl MembershipState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Disabled => "disabled",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionTermination {
    pub code: SessionCloseReason,
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
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
impl ConversationMessageStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
            Self::Injected => "injected",
            Self::Unresolved => "unresolved",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationPendingMode {
    Queued,
    Steering,
}
impl ConversationPendingMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Steering => "steering",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationDisposition {
    Queued,
    Injected,
    Settled,
}
impl ConversationDisposition {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Injected => "injected",
            Self::Settled => "settled",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationLifecyclePhase {
    Absent,
    Starting,
    Attached,
    Failed,
}
impl ConversationLifecyclePhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Absent => "absent",
            Self::Starting => "starting",
            Self::Attached => "attached",
            Self::Failed => "failed",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationStartupFailureCode {
    Audit,
    Provider,
    Storage,
    Cleanup,
}
impl ConversationStartupFailureCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Audit => "audit",
            Self::Provider => "provider",
            Self::Storage => "storage",
            Self::Cleanup => "cleanup",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationStartupFailure {
    pub code: ConversationStartupFailureCode,
    pub message: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationAttachmentEvidenceFailure {
    pub code: String,
    pub message: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationLifecycle {
    pub phase: ConversationLifecyclePhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<ConversationStartupFailure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_failure: Option<ConversationAttachmentEvidenceFailure>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionDenialSupport {
    Unknown,
    Unsupported,
    SupportedForOfferedPermissionReviews,
}
impl PermissionDenialSupport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Unsupported => "unsupported",
            Self::SupportedForOfferedPermissionReviews => {
                "supported_for_offered_permission_reviews"
            }
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeHookSuppressionSupport {
    Unknown,
    Unsupported,
    SupportedForUserConfiguredHooks,
}
impl NativeHookSuppressionSupport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Unsupported => "unsupported",
            Self::SupportedForUserConfiguredHooks => "supported_for_user_configured_hooks",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionReportingSupport {
    UnsupportedNotImplemented,
    SupportedWithInvocationCorrelation,
}
impl CompactionReportingSupport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UnsupportedNotImplemented => "unsupported_not_implemented",
            Self::SupportedWithInvocationCorrelation => "supported_with_invocation_correlation",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelSwitchReportingSupport {
    UnsupportedNotImplemented,
    SupportedAfterValidatedSwitch,
}
impl ModelSwitchReportingSupport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UnsupportedNotImplemented => "unsupported_not_implemented",
            Self::SupportedAfterValidatedSwitch => "supported_after_validated_switch",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionDeferralSupport {
    UnsupportedNotImplemented,
    SupportedWithNonterminalOutcome,
}
impl PermissionDeferralSupport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UnsupportedNotImplemented => "unsupported_not_implemented",
            Self::SupportedWithNonterminalOutcome => "supported_with_nonterminal_outcome",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ElicitationForwardingSupport {
    Unknown,
    Unsupported,
    SupportedWithCorrelatedRoundTrip,
}
impl ElicitationForwardingSupport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Unsupported => "unsupported",
            Self::SupportedWithCorrelatedRoundTrip => "supported_with_correlated_round_trip",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PreToolPolicySupport {
    Unknown,
    UnsupportedNotImplemented,
    SupportedAtPermissionGate,
}
impl PreToolPolicySupport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::UnsupportedNotImplemented => "unsupported_not_implemented",
            Self::SupportedAtPermissionGate => "supported_at_permission_gate",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyEndTurnSupport {
    Unknown,
    UnsupportedNotImplemented,
    SupportedForCurrentInvocation,
}
impl PolicyEndTurnSupport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::UnsupportedNotImplemented => "unsupported_not_implemented",
            Self::SupportedForCurrentInvocation => "supported_for_current_invocation",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyCloseSessionSupport {
    Unknown,
    UnsupportedNotImplemented,
    SupportedForSession,
}
impl PolicyCloseSessionSupport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::UnsupportedNotImplemented => "unsupported_not_implemented",
            Self::SupportedForSession => "supported_for_session",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IncomingElicitationSupport {
    Unknown,
    Unsupported,
    SupportedWithCorrelatedRoundTrip,
}
impl IncomingElicitationSupport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Unsupported => "unsupported",
            Self::SupportedWithCorrelatedRoundTrip => "supported_with_correlated_round_trip",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationCapabilities {
    pub queue: bool,
    pub steer: bool,
    pub resume: bool,
    pub permissions: bool,
    pub image_input: bool,
    pub agent_features: ConversationAgentFeatures,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImageAttachment {
    pub digest: String,
    pub mime_type: String,
    pub size: u64,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LinkedFile {
    pub path: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AttachmentBeginParams {
    pub conversation_id: String,
    pub request_id: String,
    pub digest: String,
    pub mime_type: String,
    pub size: u64,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AttachmentBeginResult {
    pub request_id: String,
    pub state: String,
    pub ticket: Option<String>,
    pub expires_at_ms: Option<u64>,
    pub digest: Option<String>,
    pub mime_type: Option<String>,
    pub size: Option<u64>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationMessage {
    pub execution_id: String,
    pub user_text: String,
    pub attachments: Vec<ImageAttachment>,
    pub files: Vec<LinkedFile>,
    pub status: ConversationMessageStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steering_target: Option<String>,
    pub parts: Vec<ConversationPart>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steering_offset: Option<u64>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationPending {
    pub execution_id: String,
    pub text: String,
    pub attachments: Vec<ImageAttachment>,
    pub files: Vec<LinkedFile>,
    pub mode: ConversationPendingMode,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationPermissionOption {
    pub id: String,
    pub label: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationPermission {
    pub execution_id: String,
    pub permission_id: String,
    pub tool_id: String,
    pub title: String,
    pub options: Vec<ConversationPermissionOption>,
    pub tool_name: String,
    pub arguments_json: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationTool {
    pub execution_id: String,
    pub tool_id: String,
    pub title: String,
    pub kind: String,
    pub status: String,
    pub details: String,
    pub input: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp: Option<ConversationMcpTool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_content: Option<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationMcpTool {
    pub server: String,
    pub tool: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationView {
    pub conversation_id: String,
    pub revision: String,
    pub messages: Vec<ConversationMessage>,
    pub pending: Vec<ConversationPending>,
    pub permissions: Vec<ConversationPermission>,
    pub tools: Vec<ConversationTool>,
    pub capabilities: ConversationCapabilities,
    pub lifecycle: ConversationLifecycle,
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interaction_view_error: Option<String>,
    pub queue_complete: bool,
    pub transcript_state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<ConversationRuntime>,
    pub title: Option<String>,
    pub questions: Vec<ConversationQuestion>,
    pub approval_mode: ApprovalMode,
    pub approval_modes: Vec<ApprovalModeChoice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_mode_change: Option<ApprovalModeChange>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationCreateParams {
    pub conversation_id: String,
    pub request_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_mode: Option<ApprovalMode>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationCreateResult {
    pub conversation_id: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationReadParams {
    pub conversation_id: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationSummary {
    pub conversation_id: String,
    pub title: Option<String>,
    pub preview: Option<String>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub running: bool,
    pub archived: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationListResult {
    pub conversations: Vec<ConversationSummary>,
    pub complete: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationSendParams {
    pub conversation_id: String,
    pub request_id: String,
    pub execution_id: String,
    pub text: String,
    pub attachments: Vec<ImageAttachment>,
    pub files: Vec<LinkedFile>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationRemoveParams {
    pub conversation_id: String,
    pub request_id: String,
    pub execution_id: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationPermissionSelectionState {
    Pending,
    Consumed,
    Unknown,
}
impl ConversationPermissionSelectionState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Consumed => "consumed",
            Self::Unknown => "unknown",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationPermissionAnswerErrorDetails {
    pub selection_state: ConversationPermissionSelectionState,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationAnswerParams {
    pub conversation_id: String,
    pub request_id: String,
    pub execution_id: String,
    pub permission_id: String,
    pub option_id: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationCancelParams {
    pub conversation_id: String,
    pub request_id: String,
    pub execution_id: String,
    pub permission_id: String,
    pub reason: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationCloseParams {
    pub conversation_id: String,
    pub request_id: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationArchiveParams {
    pub conversation_id: String,
    pub request_id: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationDeleteParams {
    pub conversation_id: String,
    pub request_id: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationReceipt {
    pub execution_id: String,
    pub disposition: ConversationDisposition,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationMutationResult {
    pub request_id: String,
    pub applied: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationReorderParams {
    pub conversation_id: String,
    pub request_id: String,
    pub execution_ids: Vec<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationReorderOutcome {
    Applied,
    Unchanged,
    QueueChanged,
    PriorityConflict,
}
impl ConversationReorderOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::Unchanged => "unchanged",
            Self::QueueChanged => "queue_changed",
            Self::PriorityConflict => "priority_conflict",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationReorderResult {
    pub request_id: String,
    pub outcome: ConversationReorderOutcome,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationRuntime {
    pub model: String,
    pub provider: String,
    pub workspace: String,
    pub agent: String,
    pub model_name: String,
    pub context_window_tokens: u64,
    pub reasoning: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationPart {
    pub offset: u64,
    pub kind: String,
    pub text: String,
    pub tool_id: String,
    pub notice_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationErrorCode {
    AgentNotConfigured,
    AgentUnsupported,
    ModelUnavailable,
    ApprovalModeUnavailable,
    ApprovalModeNotApplied,
    ApprovalModeUncertain,
    ApprovalRequestConflict,
    TurnRunning,
    ConversationsNotConfigured,
    UnknownMethod,
    InvalidRequest,
    ConversationNotFound,
    ConversationCapacity,
    ConversationClosed,
    ConversationConfigurationChanged,
    ConversationStateUnreadable,
    ConversationStorageUnavailable,
    TemporarilyUnavailable,
    AuditUnavailable,
    SubmissionConflict,
    SubmissionUnresolved,
    StalePermission,
    AgentStartupDeadline,
    AgentOperationFailed,
    ImageInputUnsupported,
    AttachmentNotFound,
    AttachmentUnavailable,
    AttachmentCapacity,
    AttachmentStorageUnavailable,
    AttachmentCleanupUnavailable,
    ConversationDeleted,
    ConversationErasureIncomplete,
}
impl ConversationErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AgentNotConfigured => "agent_not_configured",
            Self::AgentUnsupported => "agent_unsupported",
            Self::ModelUnavailable => "model_unavailable",
            Self::ApprovalModeUnavailable => "approval_mode_unavailable",
            Self::ApprovalModeNotApplied => "approval_mode_not_applied",
            Self::ApprovalModeUncertain => "approval_mode_uncertain",
            Self::ApprovalRequestConflict => "approval_request_conflict",
            Self::TurnRunning => "turn_running",
            Self::ConversationsNotConfigured => "conversations_not_configured",
            Self::UnknownMethod => "unknown_method",
            Self::InvalidRequest => "invalid_request",
            Self::ConversationNotFound => "conversation_not_found",
            Self::ConversationCapacity => "conversation_capacity",
            Self::ConversationClosed => "conversation_closed",
            Self::ConversationConfigurationChanged => "conversation_configuration_changed",
            Self::ConversationStateUnreadable => "conversation_state_unreadable",
            Self::ConversationStorageUnavailable => "conversation_storage_unavailable",
            Self::TemporarilyUnavailable => "temporarily_unavailable",
            Self::AuditUnavailable => "audit_unavailable",
            Self::SubmissionConflict => "submission_conflict",
            Self::SubmissionUnresolved => "submission_unresolved",
            Self::StalePermission => "stale_permission",
            Self::AgentStartupDeadline => "agent_startup_deadline",
            Self::AgentOperationFailed => "agent_operation_failed",
            Self::ImageInputUnsupported => "image_input_unsupported",
            Self::AttachmentNotFound => "attachment_not_found",
            Self::AttachmentUnavailable => "attachment_unavailable",
            Self::AttachmentCapacity => "attachment_capacity",
            Self::AttachmentStorageUnavailable => "attachment_storage_unavailable",
            Self::AttachmentCleanupUnavailable => "attachment_cleanup_unavailable",
            Self::ConversationDeleted => "conversation_deleted",
            Self::ConversationErasureIncomplete => "conversation_erasure_incomplete",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationAnswerOption {
    pub value: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationAsked {
    pub key: String,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    pub multi_select: bool,
    pub free_text: bool,
    pub required: bool,
    pub options: Vec<ConversationAnswerOption>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationQuestion {
    pub execution_id: String,
    pub question_id: String,
    pub message: String,
    pub questions: Vec<ConversationAsked>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationQuestionChoice {
    pub key: String,
    pub values: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub own_words: Option<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationAnswerQuestionParams {
    pub conversation_id: String,
    pub request_id: String,
    pub execution_id: String,
    pub question_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choices: Option<Vec<ConversationQuestionChoice>>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalMode {
    Ask,
    Auto,
    Full,
}
impl ApprovalMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Auto => "auto",
            Self::Full => "full",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalModeChoice {
    pub id: ApprovalMode,
    pub name: String,
    pub description: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentModelOption {
    pub model_id: String,
    pub display_name: String,
    pub max_context_window_tokens: u64,
    pub reasoning: bool,
    pub image_input: bool,
    pub approval_modes: Vec<ApprovalModeChoice>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentOption {
    pub agent: String,
    pub default_model: String,
    pub models: Vec<AgentModelOption>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentsListResult {
    pub agents: Vec<AgentOption>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationSetApprovalModeParams {
    pub conversation_id: String,
    pub request_id: String,
    pub mode: ApprovalMode,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationSetApprovalModeResult {
    pub request_id: String,
    pub mode: ApprovalMode,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalModeChange {
    pub request_id: String,
    pub requested_mode: ApprovalMode,
    pub status: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentInstallParams {
    pub agent: InstallableAgent,
    pub request_id: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentInstallOffer {
    pub agent: InstallableAgent,
    pub version: String,
    pub archive_bytes: u64,
    pub installed: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentInstallOptionsResult {
    pub agents: Vec<AgentInstallOffer>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentInstallResult {
    pub agent: InstallableAgent,
    pub version: String,
    pub downloaded: bool,
    pub cleanup_pending: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallableAgent {
    Claude,
    Codex,
    Opencode,
}
impl InstallableAgent {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Opencode => "opencode",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecordScope {
    pub receiver: String,
    pub origin: String,
    pub stream: String,
    pub incarnation: String,
    pub schema: String,
    pub access_epoch: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecordPageRequest {
    pub scope: RecordScope,
    pub after: String,
    pub target: String,
    pub max_records: u64,
    pub max_payload_bytes: u64,
    pub max_record_bytes: u64,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecordWireRecord {
    pub position: String,
    pub id: String,
    pub payload: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationRecordsHeadParams {
    pub conversation_id: String,
    pub access_epoch: String,
    pub receiver_id: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationRecordsHeadResult {
    pub scope: RecordScope,
    pub head: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationRecordsPageParams {
    pub conversation_id: String,
    pub access_epoch: String,
    pub request: RecordPageRequest,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationRecordsPageResult {
    pub request: RecordPageRequest,
    pub records: Vec<RecordWireRecord>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogueEntryKey {
    pub creation: String,
    pub id: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogueDescriptor {
    pub key: CatalogueEntryKey,
    pub revision: String,
    pub deleted: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CataloguePass {
    pub scope: RecordScope,
    pub completed: String,
    pub boundary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<CatalogueEntryKey>,
    pub generation: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogueManifestRequest {
    pub pass: CataloguePass,
    pub max_entries: u64,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationCatalogueHeadParams {
    pub receiver_id: String,
    pub access_epoch: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationCatalogueHeadResult {
    pub scope: RecordScope,
    pub head: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationCatalogueManifestParams {
    pub request: CatalogueManifestRequest,
    pub access_epoch: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationCatalogueManifestResult {
    pub request: CatalogueManifestRequest,
    pub entries: Vec<CatalogueDescriptor>,
    pub has_more: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationCatalogueResolveParams {
    pub pass: CataloguePass,
    pub descriptor: CatalogueDescriptor,
    pub max_payload_bytes: u64,
    pub access_epoch: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationCatalogueResolveResult {
    pub pass: CataloguePass,
    pub descriptor: CatalogueDescriptor,
    pub entry: CatalogueDescriptor,
    pub payload: String,
}
/// Published bound from the product schema.
pub const MAX_AUTH_CREDENTIAL_CHARACTERS: usize = 16384;
/// Published bound from the product schema.
pub const MAX_PRODUCT_CLIENT_ID_CHARACTERS: usize = 256;
/// Published bound from the product schema.
pub const MIN_AGENT_INSTALL_REQUEST_ID_CHARACTERS: usize = 1;
/// Published bound from the product schema.
pub const MAX_AGENT_INSTALL_REQUEST_ID_BYTES: usize = 256;
/// Published bound from the product schema.
pub const MAX_PHYSICAL_RECORD_PAYLOAD_BYTES: usize = 65546;
/// Published bound from the product schema.
pub const MAX_RECORD_PAGE_RECORDS: usize = 16;
/// Published bound from the product schema.
pub const MAX_RECORD_PAGE_PAYLOAD_BYTES: usize = 65546;
/// Published bound from the product schema.
pub const MAX_RECORD_RESPONSE_BYTES: usize = 131072;
/// Published passive read timing from the product schema, in milliseconds.
pub const PASSIVE_READ_TIMEOUT_MS: u64 = 10000;
/// Published passive read timing from the product schema, in milliseconds.
pub const PASSIVE_DELIVERY_TIMEOUT_MS: u64 = 30000;
/// Published passive read timing from the product schema, in milliseconds.
pub const PASSIVE_CLIENT_ALLOWANCE_MS: u64 = 5000;
/// Published passive read timing from the product schema, in milliseconds.
pub const PASSIVE_MIN_REQUEST_TIMEOUT_MS: u64 = 45000;
pub mod product_method {
    pub const SESSION_AUTHENTICATE: &str = "session.authenticate";
    pub const AUTH_SESSION: &str = "auth.session";
    pub const SERVER_HEALTH: &str = "server.health";
    pub const CREDENTIAL_ISSUE: &str = "credential.issue";
    pub const CREDENTIAL_LIST: &str = "credential.list";
    pub const CREDENTIAL_REVOKE: &str = "credential.revoke";
    pub const CONVERSATION_CREATE: &str = "conversation.create";
    pub const CONVERSATION_READ: &str = "conversation.read";
    pub const CONVERSATION_RECORDS_HEAD: &str = "conversation.recordsHead";
    pub const CONVERSATION_RECORDS_PAGE: &str = "conversation.recordsPage";
    pub const CONVERSATION_LIST: &str = "conversation.list";
    pub const CONVERSATION_SEND: &str = "conversation.send";
    pub const CONVERSATION_STEER: &str = "conversation.steer";
    pub const CONVERSATION_REMOVE: &str = "conversation.remove";
    pub const CONVERSATION_ANSWER: &str = "conversation.answer";
    pub const CONVERSATION_ANSWER_QUESTION: &str = "conversation.answerQuestion";
    pub const CONVERSATION_CANCEL: &str = "conversation.cancel";
    pub const CONVERSATION_CLOSE: &str = "conversation.close";
    pub const CONVERSATION_ARCHIVE: &str = "conversation.archive";
    pub const CONVERSATION_UNARCHIVE: &str = "conversation.unarchive";
    pub const CONVERSATION_DELETE: &str = "conversation.delete";
    pub const CONVERSATION_REORDER: &str = "conversation.reorder";
    pub const ATTACHMENT_BEGIN: &str = "attachment.begin";
    pub const AGENTS_LIST: &str = "agents.list";
    pub const CONVERSATION_SET_APPROVAL_MODE: &str = "conversation.setApprovalMode";
    pub const AGENTS_INSTALL_OPTIONS: &str = "agents.installOptions";
    pub const AGENTS_INSTALL: &str = "agents.install";
    pub const CONVERSATION_CATALOGUE_HEAD: &str = "conversation.catalogueHead";
    pub const CONVERSATION_CATALOGUE_MANIFEST: &str = "conversation.catalogueManifest";
    pub const CONVERSATION_CATALOGUE_RESOLVE: &str = "conversation.catalogueResolve";
}
pub mod product_event {
    pub const SESSION_CHALLENGE: &str = "session.challenge";
}
pub(crate) fn wire_shape_session_challenge(value: &Value) -> bool {
    value.as_object().is_some_and(|object| {
        object.get("minVersion").is_some_and(|field| {
            let _ = field;
            field
                .as_u64()
                .is_some_and(|number| number <= 9007199254740991)
        }) && object.get("maxVersion").is_some_and(|field| {
            let _ = field;
            field
                .as_u64()
                .is_some_and(|number| number <= 9007199254740991)
        }) && object.get("nonce").is_some_and(|field| {
            let _ = field;
            field
                .as_str()
                .is_some_and(|text| text.chars().count() >= 1 && text.chars().count() <= 256)
        }) && object.get("expiresAt").is_some_and(|field| {
            let _ = field;
            field
                .as_u64()
                .is_some_and(|number| number <= 9007199254740991)
        }) && object
            .keys()
            .all(|key| ["minVersion", "maxVersion", "nonce", "expiresAt"].contains(&key.as_str()))
    })
}
pub(crate) fn wire_shape_product_client_metadata(value: &Value) -> bool {
    value.as_object().is_some_and(|object| {
        object.get("id").is_some_and(|field| {
            let _ = field;
            field
                .as_str()
                .is_some_and(|text| text.chars().count() >= 1 && text.chars().count() <= 256)
        }) && object.keys().all(|key| ["id"].contains(&key.as_str()))
    })
}
pub(crate) fn wire_shape_session_authenticate_params(value: &Value) -> bool {
    value.as_object().is_some_and(|object| {
        object.get("minVersion").is_some_and(|field| {
            let _ = field;
            field
                .as_u64()
                .is_some_and(|number| number <= 9007199254740991)
        }) && object.get("maxVersion").is_some_and(|field| {
            let _ = field;
            field
                .as_u64()
                .is_some_and(|number| number <= 9007199254740991)
        }) && object.get("nonce").is_some_and(|field| {
            let _ = field;
            field
                .as_str()
                .is_some_and(|text| text.chars().count() >= 1 && text.chars().count() <= 256)
        }) && object.get("credential").is_some_and(|field| {
            let _ = field;
            field
                .as_str()
                .is_some_and(|text| text.chars().count() >= 1 && text.chars().count() <= 16384)
        }) && object.get("client").is_some_and(|field| {
            let _ = field;
            wire_shape_product_client_metadata(field)
        }) && object.keys().all(|key| {
            ["minVersion", "maxVersion", "nonce", "credential", "client"].contains(&key.as_str())
        })
    })
}
pub(crate) fn wire_shape_product_resource(value: &Value) -> bool {
    value.as_object().is_some_and(|object| {
        object.get("organizationId").is_some_and(|field| {
            let _ = field;
            field
                .as_str()
                .is_some_and(|text| text.chars().count() >= 1 && text.chars().count() <= 256)
        }) && object.get("id").is_some_and(|field| {
            let _ = field;
            field
                .as_str()
                .is_some_and(|text| text.chars().count() >= 1 && text.chars().count() <= 256)
        }) && object
            .keys()
            .all(|key| ["organizationId", "id"].contains(&key.as_str()))
    })
}
pub(crate) fn wire_shape_product_grant(value: &Value) -> bool {
    value.as_object().is_some_and(|object| {
        object.get("action").is_some_and(|field| {
            let _ = field;
            field
                .as_str()
                .is_some_and(|text| text.chars().count() >= 1 && text.chars().count() <= 256)
        }) && object.get("resource").is_some_and(|field| {
            let _ = field;
            wire_shape_product_resource(field)
        }) && object
            .keys()
            .all(|key| ["action", "resource"].contains(&key.as_str()))
    })
}
pub(crate) fn wire_shape_product_session_ready(value: &Value) -> bool {
    value.as_object().is_some_and(|object| {
        object.get("version").is_some_and(|field| {
            let _ = field;
            field.as_u64() == Some(1)
        }) && object.get("gatewayId").is_some_and(|field| {
            let _ = field;
            field
                .as_str()
                .is_some_and(|text| text.chars().count() >= 1 && text.chars().count() <= 256)
        }) && object.get("principalId").is_some_and(|field| {
            let _ = field;
            field
                .as_str()
                .is_some_and(|text| text.chars().count() >= 1 && text.chars().count() <= 256)
        }) && object.get("organizationId").is_some_and(|field| {
            let _ = field;
            field
                .as_str()
                .is_some_and(|text| text.chars().count() >= 1 && text.chars().count() <= 256)
        }) && object.get("membershipId").is_some_and(|field| {
            let _ = field;
            field
                .as_str()
                .is_some_and(|text| text.chars().count() >= 1 && text.chars().count() <= 256)
        }) && object.get("credentialId").is_some_and(|field| {
            let _ = field;
            field
                .as_str()
                .is_some_and(|text| text.chars().count() >= 1 && text.chars().count() <= 256)
        }) && object.get("audienceId").is_some_and(|field| {
            let _ = field;
            field
                .as_str()
                .is_some_and(|text| text.chars().count() >= 1 && text.chars().count() <= 256)
        }) && object.get("expiresAt").is_some_and(|field| {
            let _ = field;
            field.is_null()
                || field
                    .as_u64()
                    .is_some_and(|number| number <= 9007199254740991)
        }) && object.get("grants").is_some_and(|field| {
            let _ = field;
            field.as_array().is_some_and(|items| {
                items.len() <= 16 && items.iter().all(wire_shape_product_grant)
            })
        }) && object.get("methods").is_some_and(|field| {
            let _ = field;
            field.as_array().is_some_and(|items| {
                items.len() <= 29
                    && items.iter().all(|item| {
                        let _ = item;
                        item.is_string()
                    })
            })
        }) && object.keys().all(|key| {
            [
                "version",
                "gatewayId",
                "principalId",
                "organizationId",
                "membershipId",
                "credentialId",
                "audienceId",
                "expiresAt",
                "grants",
                "methods",
            ]
            .contains(&key.as_str())
        })
    })
}
pub const PRODUCT_HANDSHAKE_METHOD: &str = "session.authenticate";
pub const PRODUCT_READY_METHODS: &[&str] = &[
    "auth.session",
    "server.health",
    "credential.issue",
    "credential.list",
    "credential.revoke",
    "conversation.create",
    "conversation.read",
    "conversation.recordsHead",
    "conversation.recordsPage",
    "conversation.list",
    "conversation.send",
    "conversation.steer",
    "conversation.remove",
    "conversation.answer",
    "conversation.answerQuestion",
    "conversation.cancel",
    "conversation.close",
    "conversation.archive",
    "conversation.unarchive",
    "conversation.delete",
    "conversation.reorder",
    "attachment.begin",
    "agents.list",
    "conversation.setApprovalMode",
    "agents.installOptions",
    "agents.install",
    "conversation.catalogueHead",
    "conversation.catalogueManifest",
    "conversation.catalogueResolve",
];
pub const PRODUCT_VERSION: u64 = 1;
pub const PRODUCT_SESSION_PATH: &str = "/session";
