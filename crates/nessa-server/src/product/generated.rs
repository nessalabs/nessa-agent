//! Generated from protocol/product/v1.json. Do not edit.
//! Bounds are validated at the transport boundary; these are payload types only.
#![allow(dead_code)]
use nessa_auth::application::dto::{
    CredentialGrantDto, CredentialMetadataDto, MembershipInputDto, PrincipalInputDto,
};
use serde::{Deserialize, Serialize};
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
pub enum SessionCloseReason {
    AuthenticationFailed,
    CredentialRevoked,
    CredentialExpired,
    AuthorizationLost,
    ProtocolIncompatible,
    HandshakeTimeout,
    TemporaryUnavailable,
    GatewayRestarting,
    GatewayOverloaded,
    ServerShutdown,
    TransportInterrupted,
}
impl SessionCloseReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AuthenticationFailed => "authentication_failed",
            Self::CredentialRevoked => "credential_revoked",
            Self::CredentialExpired => "credential_expired",
            Self::AuthorizationLost => "authorization_lost",
            Self::ProtocolIncompatible => "protocol_incompatible",
            Self::HandshakeTimeout => "handshake_timeout",
            Self::TemporaryUnavailable => "temporary_unavailable",
            Self::GatewayRestarting => "gateway_restarting",
            Self::GatewayOverloaded => "gateway_overloaded",
            Self::ServerShutdown => "server_shutdown",
            Self::TransportInterrupted => "transport_interrupted",
        }
    }
}
impl SessionCloseReason {
    pub fn web_socket_code(self) -> u16 {
        match self {
            Self::AuthenticationFailed => 4001,
            Self::CredentialRevoked => 4002,
            Self::CredentialExpired => 4003,
            Self::AuthorizationLost => 4004,
            Self::ProtocolIncompatible => 4005,
            Self::HandshakeTimeout => 4006,
            Self::TemporaryUnavailable => 4010,
            Self::GatewayRestarting => 1012,
            Self::GatewayOverloaded => 1013,
            Self::ServerShutdown => 1000,
            Self::TransportInterrupted => 1006,
        }
    }
    pub fn retryable(self) -> bool {
        match self {
            Self::AuthenticationFailed => false,
            Self::CredentialRevoked => false,
            Self::CredentialExpired => false,
            Self::AuthorizationLost => false,
            Self::ProtocolIncompatible => false,
            Self::HandshakeTimeout => true,
            Self::TemporaryUnavailable => true,
            Self::GatewayRestarting => true,
            Self::GatewayOverloaded => true,
            Self::ServerShutdown => false,
            Self::TransportInterrupted => true,
        }
    }
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
    UnsupportedNotImplemented,
    SupportedWithCorrelatedRoundTrip,
}
impl IncomingElicitationSupport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::UnsupportedNotImplemented => "unsupported_not_implemented",
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
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_view_error: Option<String>,
    pub queue_complete: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<ConversationRuntime>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationCreateParams {
    pub conversation_id: String,
    pub request_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
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
}
impl ConversationErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AgentNotConfigured => "agent_not_configured",
            Self::AgentUnsupported => "agent_unsupported",
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
        }
    }
}
