//! Generated from protocol/product/v1.json and manifest.json. Do not edit.
//! Bounds are validated at the transport boundary; these are payload types only.
//! Variant names are the schema's wire spellings, so a shared prefix is the wire's.
#![allow(dead_code, clippy::enum_variant_names)]
use crate::product_contract::generated::{ChangeWatchEndReason, SessionCloseReason};
use nessa_auth::application::dto::{
    CredentialGrantDto, CredentialMetadataDto, MembershipInputDto, PrincipalInputDto,
};
use nessa_auth::domain::pairing::{ConsentIntentId, DeviceKey, InvitationId};
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProductSurfaceKind {
    Panel,
    Web,
    Desktop,
    Cli,
}
impl ProductSurfaceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Panel => "panel",
            Self::Web => "web",
            Self::Desktop => "desktop",
            Self::Cli => "cli",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProductSurface {
    pub kind: ProductSurfaceKind,
    pub instance: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionAuthenticateParams {
    pub min_version: u64,
    pub max_version: u64,
    pub nonce: String,
    pub credential: String,
    pub client: ProductClientMetadata,
    pub surface: ProductSurface,
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
    #[serde(deserialize_with = "Option::deserialize")]
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
    Gateway,
}
impl PrincipalKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::Integration => "integration",
            Self::Agent => "agent",
            Self::Gateway => "gateway",
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
    #[serde(deserialize_with = "Option::deserialize")]
    pub ticket: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub expires_at_ms: Option<u64>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub digest: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub mime_type: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub size: Option<u64>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationMessage {
    pub execution_id: String,
    pub user_text: String,
    pub attachments: Vec<ImageAttachment>,
    pub files: Vec<LinkedFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<ConversationMessageApp>,
    pub status: ConversationMessageStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authentication_required: Option<bool>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<ConversationMessageApp>,
    pub mode: ConversationPendingMode,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationMessageApp {
    pub execution_id: String,
    pub tool_id: String,
    pub server: String,
    pub tool: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationPermissionOption {
    pub id: String,
    pub label: String,
    pub effect: ConversationPermissionOptionEffect,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationPermissionOptionEffect {
    Allow,
    Deny,
}
impl ConversationPermissionOptionEffect {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
        }
    }
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
    pub origin: ConversationPermissionOrigin,
    pub ask: ConversationPermissionAsk,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments_json: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease: Option<ConversationLease>,
    #[serde(deserialize_with = "Option::deserialize")]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
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
    #[serde(deserialize_with = "Option::deserialize")]
    pub title: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
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
pub struct ConversationObserveCursor {
    pub incarnation: String,
    pub boundary: String,
    pub creation: String,
    pub id: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationObserveParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<ConversationObserveCursor>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationObserveResult {
    pub conversations: Vec<ConversationSummary>,
    pub complete: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<ConversationObserveCursor>,
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
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationStopParams {
    pub conversation_id: String,
    pub request_id: String,
    pub execution_id: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationCommandOperation {
    Create,
    Submit,
    Steer,
    Stop,
}
impl ConversationCommandOperation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Submit => "submit",
            Self::Steer => "steer",
            Self::Stop => "stop",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationCommandStage {
    Accepted,
    Attempted,
    Ready,
    Settled,
}
impl ConversationCommandStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Attempted => "attempted",
            Self::Ready => "ready",
            Self::Settled => "settled",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationCommandOutcome {
    Dispatched,
    Withdrawn,
    Cancelled,
    AlreadyFinal,
}
impl ConversationCommandOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dispatched => "dispatched",
            Self::Withdrawn => "withdrawn",
            Self::Cancelled => "cancelled",
            Self::AlreadyFinal => "already_final",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationCommandReceipt {
    pub request_id: String,
    pub stage: ConversationCommandStage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<ConversationCommandOutcome>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationReceiptParams {
    pub conversation_id: String,
    pub request_id: String,
    pub operation: ConversationCommandOperation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<ImageAttachment>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<LinkedFile>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_mode: Option<ApprovalMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationReceiptResult {
    pub found: bool,
    pub request_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<ConversationCommandStage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<ConversationCommandOutcome>,
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
pub struct ConversationShareParams {
    pub conversation_id: String,
    pub request_id: String,
    pub credential_id: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationSharesParams {
    pub conversation_id: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationShare {
    pub credential_id: String,
    pub role: String,
    pub granted_at_ms: u64,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationSharesResult {
    pub items: Vec<ConversationShare>,
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
pub struct ConversationLease {
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal: Option<String>,
    pub dropped_events: u64,
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
pub enum ConversationPermissionAsk {
    Tool,
    Message,
}
impl ConversationPermissionAsk {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tool => "tool",
            Self::Message => "message",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationPermissionOriginKind {
    Harness,
    App,
}
impl ConversationPermissionOriginKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Harness => "harness",
            Self::App => "app",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationPermissionOrigin {
    pub kind: ConversationPermissionOriginKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpAppReference {
    pub execution_id: String,
    pub tool_id: String,
    pub instance_id: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpReleaseAppParams {
    pub conversation_id: String,
    pub request_id: String,
    pub app: McpAppReference,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpSendMessageParams {
    pub conversation_id: String,
    pub request_id: String,
    pub app: McpAppReference,
    pub server: String,
    pub text: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpSendMessageResult {
    pub execution_id: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpUpdateModelContextParams {
    pub conversation_id: String,
    pub request_id: String,
    pub app: McpAppReference,
    pub server: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_content_json: Option<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpCallToolParams {
    pub conversation_id: String,
    pub request_id: String,
    pub app: McpAppReference,
    pub server: String,
    pub tool: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments_json: Option<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpCallToolResult {
    pub result_json: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpReadResourceParams {
    pub conversation_id: String,
    pub request_id: String,
    pub app: McpAppReference,
    pub server: String,
    pub uri: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpUiCsp {
    pub connect_domains: Vec<String>,
    pub resource_domains: Vec<String>,
    pub frame_domains: Vec<String>,
    pub base_uri_domains: Vec<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpUiPermissions {
    pub camera: bool,
    pub microphone: bool,
    pub geolocation: bool,
    pub clipboard_write: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpReadResourceResult {
    pub uri: String,
    pub mime_type: String,
    pub size: u64,
    pub sha256: String,
    pub ticket: String,
    pub expires_in_ms: u64,
    pub csp: McpUiCsp,
    pub permissions: McpUiPermissions,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefers_border: Option<bool>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpRemoteErrorDetails {
    pub code: i64,
    pub message: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum McpServerKind {
    Stdio,
    Remote,
}
impl McpServerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stdio => "stdio",
            Self::Remote => "remote",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpServerListEntry {
    pub kind: McpServerKind,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_names: Option<Vec<String>>,
    pub enabled: bool,
    pub managed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization: Option<McpServerAuthorization>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum McpAuthorizationPhase {
    Unauthenticated,
    ConsentNeeded,
    PendingConsent,
    Ready,
    ScopeRequired,
    AuthorizationIncomplete,
    Revoking,
    RevocationIncomplete,
}
impl McpAuthorizationPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unauthenticated => "unauthenticated",
            Self::ConsentNeeded => "consent_needed",
            Self::PendingConsent => "pending_consent",
            Self::Ready => "ready",
            Self::ScopeRequired => "scope_required",
            Self::AuthorizationIncomplete => "authorization_incomplete",
            Self::Revoking => "revoking",
            Self::RevocationIncomplete => "revocation_incomplete",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum McpRemoteObservation {
    Acknowledged,
    Unsupported,
    Unconfirmed,
}
impl McpRemoteObservation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Acknowledged => "acknowledged",
            Self::Unsupported => "unsupported",
            Self::Unconfirmed => "unconfirmed",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpServerAuthorization {
    pub phase: McpAuthorizationPhase,
    pub generation: u64,
    pub token_expired: bool,
    pub refresh_failing: bool,
    pub scope_required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_observation: Option<McpRemoteObservation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domains_digest: Option<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpServersAuthorizeParams {
    pub revision: String,
    pub id: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpServersAuthorizeResult {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consent_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<u64>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpServersRevokeParams {
    pub revision: String,
    pub id: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpServersRevokeResult {
    pub settled: bool,
    pub local_drained: bool,
    pub secret_deleted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_observation: Option<McpRemoteObservation>,
    pub evidence_acknowledged: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpServersAuthorizationHeldDetails {
    pub applied: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpServersListResult {
    pub revision: String,
    pub servers: Vec<McpServerListEntry>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpServerEnvEntry {
    pub name: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub value: Option<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpServerInput {
    pub kind: McpServerKind,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<Vec<McpServerEnvEntry>>,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpServersSaveParams {
    pub revision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_name: Option<String>,
    pub server: McpServerInput,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpServersRemoveParams {
    pub revision: String,
    pub name: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpServersWriteResult {
    pub revision: String,
    pub live: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum McpServerProblemCode {
    TooMany,
    DuplicateName,
    Name,
    Command,
    Arguments,
    EnvironmentName,
    ReservedEnvironmentName,
    EnvironmentValue,
    EnvironmentValueMissing,
    EnvironmentNameRepeated,
    Url,
    DuplicateServerId,
}
impl McpServerProblemCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TooMany => "too_many",
            Self::DuplicateName => "duplicate_name",
            Self::Name => "name",
            Self::Command => "command",
            Self::Arguments => "arguments",
            Self::EnvironmentName => "environment_name",
            Self::ReservedEnvironmentName => "reserved_environment_name",
            Self::EnvironmentValue => "environment_value",
            Self::EnvironmentValueMissing => "environment_value_missing",
            Self::EnvironmentNameRepeated => "environment_name_repeated",
            Self::Url => "url",
            Self::DuplicateServerId => "duplicate_server_id",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpServersInvalidDetails {
    pub problem: McpServerProblemCode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpServersRevisionConflictDetails {
    pub revision: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpServersAuditUnavailableDetails {
    pub applied: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<McpServersErrorCode>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpServersConfigTooLargeDetails {
    pub revision: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpServersStorageUnavailableDetails {
    pub applied: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpServersInspectParams {
    pub name: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum McpServersInspectCut {
    Tools,
    Ui,
    Bytes,
    Stopping,
}
impl McpServersInspectCut {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tools => "tools",
            Self::Ui => "ui",
            Self::Bytes => "bytes",
            Self::Stopping => "stopping",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpInspectedUi {
    pub uri: String,
    pub csp: McpUiCsp,
    pub permissions: McpUiPermissions,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpInspectedTool {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only_hint: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destructive_hint: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui: Option<McpInspectedUi>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpServersInspectResult {
    pub complete: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cut: Option<McpServersInspectCut>,
    pub tools: Vec<McpInspectedTool>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum McpServersErrorCode {
    McpServersNotConfigured,
    McpServersInvalid,
    McpServersReservedName,
    McpServersNotFound,
    McpServersRevisionConflict,
    McpServersBusy,
    McpServersConfigInvalid,
    McpServersConfigTooLarge,
    McpServersStorageUnavailable,
    AuditUnavailable,
    McpServersStopping,
    McpServerStartFailed,
    McpServerTimedOut,
    McpServerGone,
    McpServerMalformed,
    McpServerRemoteError,
    McpServerUnreachable,
    McpServerUnauthorized,
    McpServerInsufficientScope,
    McpServerSessionCollision,
    McpServersAuthorizationHeld,
    McpServersStoreUnavailable,
    McpServersRegistrationUnsupported,
    McpServersDiscoveryFailed,
    McpServersAuthorizationIncomplete,
}
impl McpServersErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::McpServersNotConfigured => "mcp_servers_not_configured",
            Self::McpServersInvalid => "mcp_servers_invalid",
            Self::McpServersReservedName => "mcp_servers_reserved_name",
            Self::McpServersNotFound => "mcp_servers_not_found",
            Self::McpServersRevisionConflict => "mcp_servers_revision_conflict",
            Self::McpServersBusy => "mcp_servers_busy",
            Self::McpServersConfigInvalid => "mcp_servers_config_invalid",
            Self::McpServersConfigTooLarge => "mcp_servers_config_too_large",
            Self::McpServersStorageUnavailable => "mcp_servers_storage_unavailable",
            Self::AuditUnavailable => "audit_unavailable",
            Self::McpServersStopping => "mcp_servers_stopping",
            Self::McpServerStartFailed => "mcp_server_start_failed",
            Self::McpServerTimedOut => "mcp_server_timed_out",
            Self::McpServerGone => "mcp_server_gone",
            Self::McpServerMalformed => "mcp_server_malformed",
            Self::McpServerRemoteError => "mcp_server_remote_error",
            Self::McpServerUnreachable => "mcp_server_unreachable",
            Self::McpServerUnauthorized => "mcp_server_unauthorized",
            Self::McpServerInsufficientScope => "mcp_server_insufficient_scope",
            Self::McpServerSessionCollision => "mcp_server_session_collision",
            Self::McpServersAuthorizationHeld => "mcp_servers_authorization_held",
            Self::McpServersStoreUnavailable => "mcp_servers_store_unavailable",
            Self::McpServersRegistrationUnsupported => "mcp_servers_registration_unsupported",
            Self::McpServersDiscoveryFailed => "mcp_servers_discovery_failed",
            Self::McpServersAuthorizationIncomplete => "mcp_servers_authorization_incomplete",
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
    pub environments: Vec<String>,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PairingEnrollee {
    Device,
    Gateway,
}
impl PairingEnrollee {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Device => "device",
            Self::Gateway => "gateway",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PairingCreateParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enrollee: Option<PairingEnrollee>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PairingInvitationParams {
    pub invitation_id: [u8; InvitationId::LENGTH],
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PairingApproveParams {
    pub invitation_id: [u8; InvitationId::LENGTH],
    pub device_key: [u8; DeviceKey::LENGTH],
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PairingOwnerPhase {
    Available,
    Claimed,
    Approved,
    Staging,
    Active,
    Terminal,
}
impl PairingOwnerPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Claimed => "claimed",
            Self::Approved => "approved",
            Self::Staging => "staging",
            Self::Active => "active",
            Self::Terminal => "terminal",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PairingTerminalCause {
    CredentialRevoked,
    Denied,
    Cancelled,
    Expired,
    Restarted,
}
impl PairingTerminalCause {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CredentialRevoked => "credential_revoked",
            Self::Denied => "denied",
            Self::Cancelled => "cancelled",
            Self::Expired => "expired",
            Self::Restarted => "restarted",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PairingInitiatorKind {
    LocalOperator,
    Principal,
    Device,
    System,
}
impl PairingInitiatorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LocalOperator => "local_operator",
            Self::Principal => "principal",
            Self::Device => "device",
            Self::System => "system",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PairingInitiator {
    pub kind: PairingInitiatorKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_key: Option<[u8; DeviceKey::LENGTH]>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PairingTerminal {
    pub cause: PairingTerminalCause,
    pub initiator: PairingInitiator,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PairingReceiver {
    pub receiver_id: String,
    pub access_epoch: u64,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PairingOwnerStatus {
    pub invitation_id: [u8; InvitationId::LENGTH],
    pub consent_id: [u8; ConsentIntentId::LENGTH],
    pub generation: u64,
    pub class: String,
    pub grant: CredentialGrantDto,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
    pub phase: PairingOwnerPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claimed_device_key: Option<[u8; DeviceKey::LENGTH]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receiver: Option<PairingReceiver>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<PairingTerminal>,
    pub cleanup_pending: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PairingActivationStop {
    Retryable,
    Permanent,
}
impl PairingActivationStop {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Retryable => "retryable",
            Self::Permanent => "permanent",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PairingApproveResult {
    pub status: PairingOwnerStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activation_stopped: Option<PairingActivationStop>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PairingCreateResult {
    pub code: String,
    pub status: PairingOwnerStatus,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PairingPendingResult {
    pub items: Vec<PairingOwnerStatus>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PairingErrorCode {
    PairingNotConfigured,
    PairingNotFound,
    PairingSlotOccupied,
    PairingCapacity,
    PairingConflict,
    PairingIneligible,
    PairingBusy,
    PairingUnavailable,
}
impl PairingErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PairingNotConfigured => "pairing_not_configured",
            Self::PairingNotFound => "pairing_not_found",
            Self::PairingSlotOccupied => "pairing_slot_occupied",
            Self::PairingCapacity => "pairing_capacity",
            Self::PairingConflict => "pairing_conflict",
            Self::PairingIneligible => "pairing_ineligible",
            Self::PairingBusy => "pairing_busy",
            Self::PairingUnavailable => "pairing_unavailable",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PeerEnrollParams {
    pub address: String,
    pub code: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PeerForgetParams {
    pub peer_key: [u8; DeviceKey::LENGTH],
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerPhase {
    Pending,
    Active,
    Revoked,
    Unreadable,
}
impl PeerPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Active => "active",
            Self::Revoked => "revoked",
            Self::Unreadable => "unreadable",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PeerGateway {
    pub peer_key: [u8; DeviceKey::LENGTH],
    pub phase: PeerPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receiver_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync: Option<PeerSync>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerSyncState {
    Waiting,
    Synced,
    Syncing,
    Unreachable,
    Failed,
    Quota,
}
impl PeerSyncState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Waiting => "waiting",
            Self::Synced => "synced",
            Self::Syncing => "syncing",
            Self::Unreachable => "unreachable",
            Self::Failed => "failed",
            Self::Quota => "quota",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PeerSync {
    pub state: PeerSyncState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_synced_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversations: Option<u64>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PeerListResult {
    pub items: Vec<PeerGateway>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerErrorCode {
    PeerNotConfigured,
    PeerNotFound,
    PeerExists,
    PeerCapacity,
    PeerBusy,
    PeerUnreachable,
    PeerInvitationRefused,
    PeerWrongInvitation,
    PeerOwnGateway,
    PeerUnavailable,
    PeerAuditUnavailable,
}
impl PeerErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PeerNotConfigured => "peer_not_configured",
            Self::PeerNotFound => "peer_not_found",
            Self::PeerExists => "peer_exists",
            Self::PeerCapacity => "peer_capacity",
            Self::PeerBusy => "peer_busy",
            Self::PeerUnreachable => "peer_unreachable",
            Self::PeerInvitationRefused => "peer_invitation_refused",
            Self::PeerWrongInvitation => "peer_wrong_invitation",
            Self::PeerOwnGateway => "peer_own_gateway",
            Self::PeerUnavailable => "peer_unavailable",
            Self::PeerAuditUnavailable => "peer_audit_unavailable",
        }
    }
}
pub type ChangeWatchId = String;
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationWatchCatalogueParams {
    pub receiver_id: String,
    pub access_epoch: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationWatchRecordsParams {
    pub conversation_id: String,
    pub receiver_id: String,
    pub access_epoch: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationWatchResult {
    pub watch_id: ChangeWatchId,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationUnwatchParams {
    pub watch_id: ChangeWatchId,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationUnwatchResult {
    pub watch_id: ChangeWatchId,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationChanged {
    pub watch_id: ChangeWatchId,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationWatchEnded {
    pub watch_id: ChangeWatchId,
    pub reason: ChangeWatchEndReason,
}
pub type ConversationSubscriptionId = String;
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationViewCursor {
    pub incarnation: String,
    pub position: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationSubscribeParams {
    pub conversation_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<ConversationViewCursor>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationSubscribeListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationSubscribeResult {
    pub subscription_id: ConversationSubscriptionId,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationUnsubscribeParams {
    pub subscription_id: ConversationSubscriptionId,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationViewed {
    pub subscription_id: ConversationSubscriptionId,
    pub cursor: ConversationViewCursor,
    pub view: ConversationView,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationListed {
    pub subscription_id: ConversationSubscriptionId,
    pub list: ConversationListResult,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationSubscriptionEndReason {
    Lagging,
    Refused,
    SourceClosed,
    TooLarge,
}
impl ConversationSubscriptionEndReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lagging => "lagging",
            Self::Refused => "refused",
            Self::SourceClosed => "source_closed",
            Self::TooLarge => "too_large",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationSubscriptionEnded {
    pub subscription_id: ConversationSubscriptionId,
    pub reason: ConversationSubscriptionEndReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_delivered: Option<ConversationViewCursor>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationSubscriptionErrorCode {
    SubscriptionCapacity,
    SubscriptionDuplicate,
    UnknownSubscription,
    CursorAhead,
}
impl ConversationSubscriptionErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SubscriptionCapacity => "subscription_capacity",
            Self::SubscriptionDuplicate => "subscription_duplicate",
            Self::UnknownSubscription => "unknown_subscription",
            Self::CursorAhead => "cursor_ahead",
        }
    }
}
pub const MAX_CHANGE_WATCH_ID_BYTES: usize = 57;
pub const CHANGE_WATCH_ID_PATTERN: &str =
    "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}-[1-9][0-9]{0,19}$";
pub const MAX_GLOBAL_CHANGE_WATCHES: usize = 64;
pub const MAX_PRINCIPAL_CHANGE_WATCHES: usize = 8;
pub const MAX_CONNECTION_RECORD_WATCHES: usize = 1;
pub const MAX_CONNECTION_CATALOGUE_WATCHES: usize = 1;
pub const MAX_CONNECTION_CHANGE_WATCHES: usize = 2;
pub const MAX_PEERS: usize = 64;
pub const MAX_CONNECTION_CONVERSATION_SUBSCRIPTIONS: usize = 8;
pub const MAX_CONNECTION_LIST_SUBSCRIPTIONS: usize = 1;
pub const SUBSCRIPTION_DELIVERY_TIMEOUT_MS: u64 = 10000;
/// Published bound from the product schema.
pub const MAX_AUTH_CREDENTIAL_CHARACTERS: usize = 16384;
/// Published bound from the product schema.
pub const MAX_PRODUCT_CLIENT_ID_CHARACTERS: usize = 256;
/// Published bound from the product schema.
pub const MAX_PRODUCT_SURFACE_INSTANCE_CHARACTERS: usize = 256;
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
/// Published bound from the product schema.
pub const MAX_MCP_CONTEXT_BYTES: usize = 8192;
/// Published bound from the product schema.
pub const MIN_MCP_MESSAGE_CHARACTERS: usize = 1;
/// Published request-frame maximum from the product schema, in bytes.
pub const MAX_PAYLOAD_BYTES: i64 = 65536;
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
    pub const CONVERSATION_OBSERVE: &str = "conversation.observe";
    pub const CONVERSATION_SEND: &str = "conversation.send";
    pub const CONVERSATION_STEER: &str = "conversation.steer";
    pub const CONVERSATION_REMOVE: &str = "conversation.remove";
    pub const CONVERSATION_STOP: &str = "conversation.stop";
    pub const CONVERSATION_RECEIPT: &str = "conversation.receipt";
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
    pub const MCP_CALL_TOOL: &str = "mcp.callTool";
    pub const MCP_READ_RESOURCE: &str = "mcp.readResource";
    pub const MCP_RELEASE_APP: &str = "mcp.releaseApp";
    pub const MCP_SEND_MESSAGE: &str = "mcp.sendMessage";
    pub const MCP_UPDATE_MODEL_CONTEXT: &str = "mcp.updateModelContext";
    pub const MCP_SERVERS_LIST: &str = "mcpServers.list";
    pub const MCP_SERVERS_SAVE: &str = "mcpServers.save";
    pub const MCP_SERVERS_REMOVE: &str = "mcpServers.remove";
    pub const MCP_SERVERS_INSPECT: &str = "mcpServers.inspect";
    pub const MCP_SERVERS_AUTHORIZE: &str = "mcpServers.authorize";
    pub const MCP_SERVERS_REVOKE: &str = "mcpServers.revoke";
    pub const PAIRING_CREATE: &str = "pairing.create";
    pub const PAIRING_PENDING: &str = "pairing.pending";
    pub const PAIRING_STATUS: &str = "pairing.status";
    pub const PAIRING_APPROVE: &str = "pairing.approve";
    pub const PAIRING_DENY: &str = "pairing.deny";
    pub const PAIRING_CANCEL: &str = "pairing.cancel";
    pub const PEER_ENROLL: &str = "peer.enroll";
    pub const PEER_LIST: &str = "peer.list";
    pub const PEER_FORGET: &str = "peer.forget";
    pub const CONVERSATION_WATCH_RECORDS: &str = "conversation.watchRecords";
    pub const CONVERSATION_WATCH_CATALOGUE: &str = "conversation.watchCatalogue";
    pub const CONVERSATION_UNWATCH: &str = "conversation.unwatch";
    pub const CONVERSATION_SUBSCRIBE: &str = "conversation.subscribe";
    pub const CONVERSATION_SUBSCRIBE_LIST: &str = "conversation.subscribeList";
    pub const CONVERSATION_UNSUBSCRIBE: &str = "conversation.unsubscribe";
    pub const CONVERSATION_SHARE: &str = "conversation.share";
    pub const CONVERSATION_UNSHARE: &str = "conversation.unshare";
    pub const CONVERSATION_SHARES: &str = "conversation.shares";
}
pub mod product_event {
    pub const SESSION_CHALLENGE: &str = "session.challenge";
    pub const CONVERSATION_CHANGED: &str = "conversation.changed";
    pub const CONVERSATION_WATCH_ENDED: &str = "conversation.watchEnded";
    pub const CONVERSATION_VIEW: &str = "conversation.view";
    pub const CONVERSATION_LISTED: &str = "conversation.listed";
    pub const CONVERSATION_SUBSCRIPTION_ENDED: &str = "conversation.subscriptionEnded";
}
pub fn wire_shape_session_challenge(value: &Value) -> bool {
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
pub fn wire_shape_product_client_metadata(value: &Value) -> bool {
    value.as_object().is_some_and(|object| {
        object.get("id").is_some_and(|field| {
            let _ = field;
            field
                .as_str()
                .is_some_and(|text| text.chars().count() >= 1 && text.chars().count() <= 256)
        }) && object.keys().all(|key| ["id"].contains(&key.as_str()))
    })
}
pub fn wire_shape_product_surface_kind(value: &Value) -> bool {
    value
        .as_str()
        .is_some_and(|text| ["panel", "web", "desktop", "cli"].contains(&text))
}
pub fn wire_shape_product_surface(value: &Value) -> bool {
    value.as_object().is_some_and(|object| {
        object.get("kind").is_some_and(|field| {
            let _ = field;
            wire_shape_product_surface_kind(field)
        }) && object.get("instance").is_some_and(|field| {
            let _ = field;
            field
                .as_str()
                .is_some_and(|text| text.chars().count() >= 1 && text.chars().count() <= 256)
        }) && object
            .keys()
            .all(|key| ["kind", "instance"].contains(&key.as_str()))
    })
}
pub fn wire_shape_session_authenticate_params(value: &Value) -> bool {
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
        }) && object.get("surface").is_some_and(|field| {
            let _ = field;
            wire_shape_product_surface(field)
        }) && object.keys().all(|key| {
            [
                "minVersion",
                "maxVersion",
                "nonce",
                "credential",
                "client",
                "surface",
            ]
            .contains(&key.as_str())
        })
    })
}
pub fn wire_shape_product_resource(value: &Value) -> bool {
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
pub fn wire_shape_product_grant(value: &Value) -> bool {
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
pub fn wire_shape_product_session_ready(value: &Value) -> bool {
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
                items.len() <= 61
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
    "conversation.observe",
    "conversation.send",
    "conversation.steer",
    "conversation.remove",
    "conversation.stop",
    "conversation.receipt",
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
    "mcp.callTool",
    "mcp.readResource",
    "mcp.releaseApp",
    "mcp.sendMessage",
    "mcp.updateModelContext",
    "mcpServers.list",
    "mcpServers.save",
    "mcpServers.remove",
    "mcpServers.inspect",
    "mcpServers.authorize",
    "mcpServers.revoke",
    "pairing.create",
    "pairing.pending",
    "pairing.status",
    "pairing.approve",
    "pairing.deny",
    "pairing.cancel",
    "peer.enroll",
    "peer.list",
    "peer.forget",
    "conversation.watchRecords",
    "conversation.watchCatalogue",
    "conversation.unwatch",
    "conversation.subscribe",
    "conversation.subscribeList",
    "conversation.unsubscribe",
    "conversation.share",
    "conversation.unshare",
    "conversation.shares",
];
/// The grant Cedar is asked for before this method is dispatched.
///
/// Generated from `protocol/product/manifest.json`. Writing
/// to a conversation, including answering its question, uploading into it, and
/// an app's calls, asks for `conversation.write`. Reading its records or
/// catalogue asks for `conversation.read`. Running a configured server with
/// the gateway's authority, and enrolling a device, asks for
/// `credential.manage`; Auth asks again for the exact consent. `None`
/// means another owner admits the method: the handshake, `auth.session`, or
/// a watch.
pub fn action_for_method(method: &str) -> Option<&'static str> {
    match method {
        "server.health" | "agents.list" | "agents.installOptions" => Some("server.read"),
        "credential.issue"
        | "credential.list"
        | "credential.revoke"
        | "mcpServers.list"
        | "mcpServers.save"
        | "mcpServers.remove"
        | "mcpServers.inspect"
        | "mcpServers.authorize"
        | "mcpServers.revoke"
        | "pairing.create"
        | "pairing.pending"
        | "pairing.status"
        | "pairing.approve"
        | "pairing.deny"
        | "pairing.cancel"
        | "peer.enroll"
        | "peer.list"
        | "peer.forget"
        | "conversation.share"
        | "conversation.unshare"
        | "conversation.shares" => Some("credential.manage"),
        "conversation.create"
        | "conversation.read"
        | "conversation.list"
        | "conversation.observe"
        | "conversation.send"
        | "conversation.steer"
        | "conversation.remove"
        | "conversation.stop"
        | "conversation.receipt"
        | "conversation.answer"
        | "conversation.answerQuestion"
        | "conversation.cancel"
        | "conversation.close"
        | "conversation.archive"
        | "conversation.unarchive"
        | "conversation.delete"
        | "conversation.reorder"
        | "attachment.begin"
        | "conversation.setApprovalMode"
        | "agents.install"
        | "mcp.callTool"
        | "mcp.readResource"
        | "mcp.releaseApp"
        | "mcp.sendMessage"
        | "mcp.updateModelContext"
        | "conversation.subscribe"
        | "conversation.subscribeList"
        | "conversation.unsubscribe" => Some("conversation.write"),
        "conversation.recordsHead"
        | "conversation.recordsPage"
        | "conversation.catalogueHead"
        | "conversation.catalogueManifest"
        | "conversation.catalogueResolve" => Some("conversation.read"),
        _ => None,
    }
}
pub const PRODUCT_VERSION: u64 = 1;
pub const PRODUCT_SESSION_PATH: &str = "/session";
