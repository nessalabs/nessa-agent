//! Generated from protocol/product/v1.json and manifest.json. Do not edit.
//! Bounds are validated at the transport boundary; these are payload types only.
//! Variant names are the schema's wire spellings, so a shared prefix is the wire's.
#![allow(dead_code, clippy::enum_variant_names)]
use crate::product_contract::generated::SessionCloseReason;
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
pub enum ProductSurfaceKind {
    #[serde(rename = "panel")]
    Panel,
    #[serde(rename = "web")]
    Web,
    #[serde(rename = "desktop")]
    Desktop,
    #[serde(rename = "cli")]
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
pub enum PrincipalKind {
    #[serde(rename = "human")]
    Human,
    #[serde(rename = "integration")]
    Integration,
    #[serde(rename = "agent")]
    Agent,
    #[serde(rename = "gateway")]
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
pub enum MembershipRole {
    #[serde(rename = "admin")]
    Admin,
    #[serde(rename = "member")]
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
pub enum MembershipState {
    #[serde(rename = "active")]
    Active,
    #[serde(rename = "disabled")]
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
pub enum ConversationMessageStatus {
    #[serde(rename = "queued")]
    Queued,
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "cancelled")]
    Cancelled,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "injected")]
    Injected,
    #[serde(rename = "unresolved")]
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
pub enum ConversationPendingMode {
    #[serde(rename = "queued")]
    Queued,
    #[serde(rename = "steering")]
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
pub enum ConversationDisposition {
    #[serde(rename = "queued")]
    Queued,
    #[serde(rename = "injected")]
    Injected,
    #[serde(rename = "settled")]
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
pub enum ConversationLifecyclePhase {
    #[serde(rename = "absent")]
    Absent,
    #[serde(rename = "starting")]
    Starting,
    #[serde(rename = "attached")]
    Attached,
    #[serde(rename = "failed")]
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
pub enum ConversationStartupFailureCode {
    #[serde(rename = "audit")]
    Audit,
    #[serde(rename = "provider")]
    Provider,
    #[serde(rename = "storage")]
    Storage,
    #[serde(rename = "cleanup")]
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
pub enum PermissionDenialSupport {
    #[serde(rename = "unknown")]
    Unknown,
    #[serde(rename = "unsupported")]
    Unsupported,
    #[serde(rename = "supported_for_offered_permission_reviews")]
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
pub enum NativeHookSuppressionSupport {
    #[serde(rename = "unknown")]
    Unknown,
    #[serde(rename = "unsupported")]
    Unsupported,
    #[serde(rename = "supported_for_user_configured_hooks")]
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
pub enum NativeHooksChoice {
    #[serde(rename = "off")]
    Off,
    #[serde(rename = "on")]
    On,
}
impl NativeHooksChoice {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::On => "on",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum NativeHooksAgent {
    #[serde(rename = "claude")]
    Claude,
    #[serde(rename = "codex")]
    Codex,
}
impl NativeHooksAgent {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum CompactionReportingSupport {
    #[serde(rename = "unsupported_not_implemented")]
    UnsupportedNotImplemented,
    #[serde(rename = "supported_with_invocation_correlation")]
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
pub enum ModelSwitchReportingSupport {
    #[serde(rename = "unsupported_not_implemented")]
    UnsupportedNotImplemented,
    #[serde(rename = "supported_after_validated_switch")]
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
pub enum PermissionDeferralSupport {
    #[serde(rename = "unsupported_not_implemented")]
    UnsupportedNotImplemented,
    #[serde(rename = "unsupported")]
    Unsupported,
    #[serde(rename = "supported_with_nonterminal_outcome")]
    SupportedWithNonterminalOutcome,
}
impl PermissionDeferralSupport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UnsupportedNotImplemented => "unsupported_not_implemented",
            Self::Unsupported => "unsupported",
            Self::SupportedWithNonterminalOutcome => "supported_with_nonterminal_outcome",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum ElicitationForwardingSupport {
    #[serde(rename = "unknown")]
    Unknown,
    #[serde(rename = "unsupported")]
    Unsupported,
    #[serde(rename = "supported_with_correlated_round_trip")]
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
pub enum PreToolPolicySupport {
    #[serde(rename = "unknown")]
    Unknown,
    #[serde(rename = "unsupported_not_implemented")]
    UnsupportedNotImplemented,
    #[serde(rename = "supported_at_permission_gate")]
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
pub enum PolicyEndTurnSupport {
    #[serde(rename = "unknown")]
    Unknown,
    #[serde(rename = "unsupported")]
    Unsupported,
    #[serde(rename = "supported_for_current_invocation")]
    SupportedForCurrentInvocation,
}
impl PolicyEndTurnSupport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Unsupported => "unsupported",
            Self::SupportedForCurrentInvocation => "supported_for_current_invocation",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum PolicyCloseSessionSupport {
    #[serde(rename = "unknown")]
    Unknown,
    #[serde(rename = "unsupported")]
    Unsupported,
    #[serde(rename = "supported_for_session")]
    SupportedForSession,
}
impl PolicyCloseSessionSupport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Unsupported => "unsupported",
            Self::SupportedForSession => "supported_for_session",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum IncomingElicitationSupport {
    #[serde(rename = "unknown")]
    Unknown,
    #[serde(rename = "unsupported")]
    Unsupported,
    #[serde(rename = "supported_with_correlated_round_trip")]
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
pub struct ArtifactReadParams {
    pub conversation_id: String,
    pub digest: String,
    pub mime_type: String,
    pub size: u64,
    pub offset: u64,
    pub max_bytes: u64,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactReadResult {
    pub offset: u64,
    pub data: String,
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
    pub required_work_failed: Option<bool>,
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
pub enum ConversationPermissionOptionEffect {
    #[serde(rename = "allow")]
    Allow,
    #[serde(rename = "deny")]
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
pub struct ConversationPermissionDeadline {
    pub wait_ms: u64,
    pub due_at_ms: u64,
    pub effect: ConversationPermissionOptionEffect,
    pub reason: String,
    pub stops_turn: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationPermission {
    pub review: String,
    pub execution_id: String,
    pub permission_id: String,
    pub tool_id: String,
    pub title: String,
    pub options: Vec<ConversationPermissionOption>,
    pub tool_name: String,
    pub origin: ConversationPermissionOrigin,
    pub ask: ConversationPermissionAsk,
    pub arguments_json: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline: Option<ConversationPermissionDeadline>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationReviewEntry {
    pub review: String,
    pub origin: String,
    pub tool_name: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationQuestionEntry {
    pub question: String,
    pub message: String,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_hooks: Option<NativeHooksChoice>,
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
    pub server_now_ms: u64,
    pub messages: Vec<ConversationMessage>,
    pub pending: Vec<ConversationPending>,
    pub permissions: Vec<ConversationReviewEntry>,
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
    pub artifacts: Vec<ConversationArtifact>,
    pub notes: Vec<ConversationNote>,
    pub unreadable: Vec<ConversationUnreadable>,
    pub unreadable_truncated: bool,
    #[serde(deserialize_with = "Option::deserialize")]
    pub title: Option<String>,
    pub questions: Vec<ConversationQuestionEntry>,
    pub approval_mode: ApprovalMode,
    pub approval_modes: Vec<ApprovalModeChoice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_mode_change: Option<ApprovalModeChange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_change: Option<ModelChange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_change_effect: Option<ModelChangeEffect>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationArtifact {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_id: Option<String>,
    pub lease: String,
    pub name: String,
    pub digest: String,
    pub mime_type: String,
    pub size: u64,
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
pub struct ConversationPeekResult {
    pub conversation_id: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub execution_id: Option<String>,
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
pub enum ConversationCommandOperation {
    #[serde(rename = "create")]
    Create,
    #[serde(rename = "submit")]
    Submit,
    #[serde(rename = "steer")]
    Steer,
    #[serde(rename = "stop")]
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
pub enum ConversationCommandStage {
    #[serde(rename = "accepted")]
    Accepted,
    #[serde(rename = "attempted")]
    Attempted,
    #[serde(rename = "ready")]
    Ready,
    #[serde(rename = "settled")]
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
pub enum ConversationCommandOutcome {
    #[serde(rename = "dispatched")]
    Dispatched,
    #[serde(rename = "withdrawn")]
    Withdrawn,
    #[serde(rename = "cancelled")]
    Cancelled,
    #[serde(rename = "already_final")]
    AlreadyFinal,
    #[serde(rename = "no_effect")]
    NoEffect,
    #[serde(rename = "refused_after_save")]
    RefusedAfterSave,
    #[serde(rename = "unavailable_after_save")]
    UnavailableAfterSave,
}
impl ConversationCommandOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dispatched => "dispatched",
            Self::Withdrawn => "withdrawn",
            Self::Cancelled => "cancelled",
            Self::AlreadyFinal => "already_final",
            Self::NoEffect => "no_effect",
            Self::RefusedAfterSave => "refused_after_save",
            Self::UnavailableAfterSave => "unavailable_after_save",
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
pub enum ConversationPermissionSelectionState {
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "consumed")]
    Consumed,
    #[serde(rename = "unknown")]
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
    pub review: String,
    pub permission_id: String,
    pub option_id: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationPermissionReviewParams {
    pub conversation_id: String,
    pub review: String,
    pub offset: u64,
    pub max_bytes: u64,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationQuestionReviewParams {
    pub conversation_id: String,
    pub question: String,
    pub offset: u64,
    pub max_bytes: u64,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationPermissionReviewResult {
    pub offset: u64,
    pub total_bytes: u64,
    pub data: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationQuestionReviewResult {
    pub offset: u64,
    pub total_bytes: u64,
    pub data: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationCancelParams {
    pub conversation_id: String,
    pub request_id: String,
    pub execution_id: String,
    pub review: String,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum ConversationCapability {
    #[serde(rename = "read")]
    Read,
    #[serde(rename = "comment")]
    Comment,
    #[serde(rename = "drive")]
    Drive,
}
impl ConversationCapability {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Comment => "comment",
            Self::Drive => "drive",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum ConversationToolPreset {
    #[serde(rename = "read_only_tools")]
    ReadOnlyTools,
    #[serde(rename = "no_shell")]
    NoShell,
    #[serde(rename = "ask_everything")]
    AskEverything,
    #[serde(rename = "all")]
    All,
}
impl ConversationToolPreset {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnlyTools => "read_only_tools",
            Self::NoShell => "no_shell",
            Self::AskEverything => "ask_everything",
            Self::All => "all",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationToolPolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<ConversationToolPreset>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deny: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_hosts: Option<Vec<String>>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationShareParams {
    pub conversation_id: String,
    pub request_id: String,
    pub credential_id: String,
    pub capabilities: Vec<ConversationCapability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<ConversationToolPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_policy_revision: Option<u64>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationUnshareParams {
    pub conversation_id: String,
    pub request_id: String,
    pub credential_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_policy_revision: Option<u64>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationSharesCursor {
    pub granted_at_ms: u64,
    pub credential_id: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationSharesParams {
    pub conversation_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<ConversationSharesCursor>,
    pub page_size: u64,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationShare {
    pub credential_id: String,
    pub capabilities: Vec<ConversationCapability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<ConversationToolPolicy>,
    pub policy_revision: u64,
    pub granted_at_ms: u64,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationSharesResult {
    pub items: Vec<ConversationShare>,
    pub change_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<ConversationSharesCursor>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationShareResult {
    pub request_id: String,
    pub applied: bool,
    pub policy_revision: u64,
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
pub enum ConversationReorderOutcome {
    #[serde(rename = "applied")]
    Applied,
    #[serde(rename = "unchanged")]
    Unchanged,
    #[serde(rename = "queue_changed")]
    QueueChanged,
    #[serde(rename = "priority_conflict")]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commands: Option<u64>,
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
pub enum ConversationPermissionAsk {
    #[serde(rename = "tool")]
    Tool,
    #[serde(rename = "message")]
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
pub enum ConversationPermissionOriginKind {
    #[serde(rename = "harness")]
    Harness,
    #[serde(rename = "app")]
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
pub enum McpServerKind {
    #[serde(rename = "stdio")]
    Stdio,
    #[serde(rename = "remote")]
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
pub enum McpAuthorizationPhase {
    #[serde(rename = "unauthenticated")]
    Unauthenticated,
    #[serde(rename = "consent_needed")]
    ConsentNeeded,
    #[serde(rename = "pending_consent")]
    PendingConsent,
    #[serde(rename = "ready")]
    Ready,
    #[serde(rename = "scope_required")]
    ScopeRequired,
    #[serde(rename = "authorization_incomplete")]
    AuthorizationIncomplete,
    #[serde(rename = "revoking")]
    Revoking,
    #[serde(rename = "revocation_incomplete")]
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
pub enum McpRemoteObservation {
    #[serde(rename = "acknowledged")]
    Acknowledged,
    #[serde(rename = "unsupported")]
    Unsupported,
    #[serde(rename = "unconfirmed")]
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
pub enum McpServerProblemCode {
    #[serde(rename = "too_many")]
    TooMany,
    #[serde(rename = "duplicate_name")]
    DuplicateName,
    #[serde(rename = "name")]
    Name,
    #[serde(rename = "command")]
    Command,
    #[serde(rename = "arguments")]
    Arguments,
    #[serde(rename = "environment_name")]
    EnvironmentName,
    #[serde(rename = "reserved_environment_name")]
    ReservedEnvironmentName,
    #[serde(rename = "environment_value")]
    EnvironmentValue,
    #[serde(rename = "environment_value_missing")]
    EnvironmentValueMissing,
    #[serde(rename = "environment_name_repeated")]
    EnvironmentNameRepeated,
    #[serde(rename = "url")]
    Url,
    #[serde(rename = "duplicate_server_id")]
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
pub enum McpServersInspectCut {
    #[serde(rename = "tools")]
    Tools,
    #[serde(rename = "ui")]
    Ui,
    #[serde(rename = "bytes")]
    Bytes,
    #[serde(rename = "stopping")]
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
pub enum McpServersErrorCode {
    #[serde(rename = "mcp_servers_not_configured")]
    McpServersNotConfigured,
    #[serde(rename = "mcp_servers_invalid")]
    McpServersInvalid,
    #[serde(rename = "mcp_servers_reserved_name")]
    McpServersReservedName,
    #[serde(rename = "mcp_servers_not_found")]
    McpServersNotFound,
    #[serde(rename = "mcp_servers_revision_conflict")]
    McpServersRevisionConflict,
    #[serde(rename = "mcp_servers_busy")]
    McpServersBusy,
    #[serde(rename = "mcp_servers_config_invalid")]
    McpServersConfigInvalid,
    #[serde(rename = "mcp_servers_config_too_large")]
    McpServersConfigTooLarge,
    #[serde(rename = "mcp_servers_storage_unavailable")]
    McpServersStorageUnavailable,
    #[serde(rename = "audit_unavailable")]
    AuditUnavailable,
    #[serde(rename = "mcp_servers_stopping")]
    McpServersStopping,
    #[serde(rename = "mcp_server_start_failed")]
    McpServerStartFailed,
    #[serde(rename = "mcp_server_timed_out")]
    McpServerTimedOut,
    #[serde(rename = "mcp_server_gone")]
    McpServerGone,
    #[serde(rename = "mcp_server_malformed")]
    McpServerMalformed,
    #[serde(rename = "mcp_server_remote_error")]
    McpServerRemoteError,
    #[serde(rename = "mcp_server_unreachable")]
    McpServerUnreachable,
    #[serde(rename = "mcp_server_unauthorized")]
    McpServerUnauthorized,
    #[serde(rename = "mcp_server_insufficient_scope")]
    McpServerInsufficientScope,
    #[serde(rename = "mcp_server_session_collision")]
    McpServerSessionCollision,
    #[serde(rename = "mcp_servers_authorization_held")]
    McpServersAuthorizationHeld,
    #[serde(rename = "mcp_servers_store_unavailable")]
    McpServersStoreUnavailable,
    #[serde(rename = "mcp_servers_registration_unsupported")]
    McpServersRegistrationUnsupported,
    #[serde(rename = "mcp_servers_discovery_failed")]
    McpServersDiscoveryFailed,
    #[serde(rename = "mcp_servers_authorization_incomplete")]
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
    pub question: String,
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
    pub question: String,
    pub question_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choices: Option<Vec<ConversationQuestionChoice>>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum ApprovalMode {
    #[serde(rename = "ask")]
    Ask,
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "full")]
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
pub struct AgentsSetNativeHooksParams {
    pub request_id: String,
    pub agent: NativeHooksAgent,
    pub enabled: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentsSetNativeHooksResult {
    pub request_id: String,
    pub agent: NativeHooksAgent,
    pub enabled: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentsListResult {
    pub agents: Vec<AgentOption>,
    pub environments: Vec<String>,
    pub command_hosts: Vec<String>,
    pub native_hooks: AgentsNativeHooks,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentsNativeHooks {
    pub claude: NativeHooksChoice,
    pub codex: NativeHooksChoice,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentsListed {
    pub subscription_id: ConversationSubscriptionId,
    pub catalog: AgentsListResult,
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
pub enum InstallableAgent {
    #[serde(rename = "claude")]
    Claude,
    #[serde(rename = "codex")]
    Codex,
    #[serde(rename = "opencode")]
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
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PhoneEndpointParams {}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PhoneOwnerSource {
    pub owner_id: String,
    pub organization_id: String,
    pub membership_id: String,
    pub audience: String,
    pub credential_id: String,
    pub resource_id: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PhoneConsent {
    pub class: String,
    pub grants: Vec<CredentialGrantDto>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PhoneDiscovery {
    pub source: PhoneOwnerSource,
    pub native_endpoint: String,
    pub gateway_key: Vec<u64>,
    pub consent: PhoneConsent,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PhoneOriginal {
    pub request_id: Vec<u64>,
    pub discovery: PhoneDiscovery,
    pub device_key: [u8; DeviceKey::LENGTH],
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PhoneAuthorizeParams {
    pub original: PhoneOriginal,
    pub signature: Vec<u64>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PhoneStatusParams {
    pub request_id: Vec<u64>,
    pub original_digest: Vec<u64>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PhoneAuthorizationStatus {
    pub original: PhoneOriginal,
    pub original_digest: Vec<u64>,
    pub invitation_id: [u8; InvitationId::LENGTH],
    pub consent_id: [u8; ConsentIntentId::LENGTH],
    pub generation: u64,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
    pub phase: PairingOwnerPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receiver: Option<PairingReceiver>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<PairingTerminal>,
    pub cleanup_pending: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activation_stopped: Option<PairingActivationStop>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum PairingEnrollee {
    #[serde(rename = "device")]
    Device,
    #[serde(rename = "gateway")]
    Gateway,
    #[serde(rename = "environment")]
    Environment,
}
impl PairingEnrollee {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Device => "device",
            Self::Gateway => "gateway",
            Self::Environment => "environment",
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
pub enum PairingOwnerPhase {
    #[serde(rename = "available")]
    Available,
    #[serde(rename = "claimed")]
    Claimed,
    #[serde(rename = "approved")]
    Approved,
    #[serde(rename = "staging")]
    Staging,
    #[serde(rename = "active")]
    Active,
    #[serde(rename = "terminal")]
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
pub enum PairingTerminalCause {
    #[serde(rename = "credential_revoked")]
    CredentialRevoked,
    #[serde(rename = "denied")]
    Denied,
    #[serde(rename = "cancelled")]
    Cancelled,
    #[serde(rename = "expired")]
    Expired,
    #[serde(rename = "restarted")]
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
pub enum PairingInitiatorKind {
    #[serde(rename = "local_operator")]
    LocalOperator,
    #[serde(rename = "principal")]
    Principal,
    #[serde(rename = "device")]
    Device,
    #[serde(rename = "system")]
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
pub enum PairingActivationStop {
    #[serde(rename = "retryable")]
    Retryable,
    #[serde(rename = "permanent")]
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
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PairingSetLinkingParams {
    pub enabled: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum PairingLinkingApply {
    #[serde(rename = "unchanged")]
    Unchanged,
    #[serde(rename = "restarting")]
    Restarting,
}
impl PairingLinkingApply {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unchanged => "unchanged",
            Self::Restarting => "restarting",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PairingSetLinkingResult {
    pub enabled: bool,
    pub applies: PairingLinkingApply,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum PairingErrorCode {
    #[serde(rename = "pairing_not_configured")]
    PairingNotConfigured,
    #[serde(rename = "pairing_not_found")]
    PairingNotFound,
    #[serde(rename = "pairing_slot_occupied")]
    PairingSlotOccupied,
    #[serde(rename = "pairing_capacity")]
    PairingCapacity,
    #[serde(rename = "pairing_conflict")]
    PairingConflict,
    #[serde(rename = "pairing_ineligible")]
    PairingIneligible,
    #[serde(rename = "pairing_busy")]
    PairingBusy,
    #[serde(rename = "pairing_unavailable")]
    PairingUnavailable,
    #[serde(rename = "pairing_address_in_use")]
    PairingAddressInUse,
    #[serde(rename = "pairing_audit_unavailable")]
    PairingAuditUnavailable,
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
            Self::PairingAddressInUse => "pairing_address_in_use",
            Self::PairingAuditUnavailable => "pairing_audit_unavailable",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PeerEnrollParams {
    pub address: String,
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay: Option<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PeerForgetParams {
    pub peer_key: [u8; DeviceKey::LENGTH],
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PeerRelayParams {
    pub peer_key: [u8; DeviceKey::LENGTH],
    #[serde(deserialize_with = "Option::deserialize")]
    pub relay: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum PeerPhase {
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "active")]
    Active,
    #[serde(rename = "revoked")]
    Revoked,
    #[serde(rename = "unreadable")]
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
    pub relay: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receiver_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync: Option<PeerSync>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum PeerSyncState {
    #[serde(rename = "waiting")]
    Waiting,
    #[serde(rename = "synced")]
    Synced,
    #[serde(rename = "syncing")]
    Syncing,
    #[serde(rename = "unreachable")]
    Unreachable,
    #[serde(rename = "away")]
    Away,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "quota")]
    Quota,
}
impl PeerSyncState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Waiting => "waiting",
            Self::Synced => "synced",
            Self::Syncing => "syncing",
            Self::Unreachable => "unreachable",
            Self::Away => "away",
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
pub enum PeerErrorCode {
    #[serde(rename = "peer_not_configured")]
    PeerNotConfigured,
    #[serde(rename = "peer_not_found")]
    PeerNotFound,
    #[serde(rename = "peer_exists")]
    PeerExists,
    #[serde(rename = "peer_capacity")]
    PeerCapacity,
    #[serde(rename = "peer_busy")]
    PeerBusy,
    #[serde(rename = "peer_unreachable")]
    PeerUnreachable,
    #[serde(rename = "peer_invitation_refused")]
    PeerInvitationRefused,
    #[serde(rename = "peer_wrong_invitation")]
    PeerWrongInvitation,
    #[serde(rename = "peer_own_gateway")]
    PeerOwnGateway,
    #[serde(rename = "peer_unavailable")]
    PeerUnavailable,
    #[serde(rename = "peer_audit_unavailable")]
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
pub enum ConversationSubscriptionEndReason {
    #[serde(rename = "lagging")]
    Lagging,
    #[serde(rename = "refused")]
    Refused,
    #[serde(rename = "source_closed")]
    SourceClosed,
    #[serde(rename = "too_large")]
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
    pub last_event_cursor: Option<ConversationEventCursor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_delivered: Option<ConversationViewCursor>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum ConversationSubscriptionErrorCode {
    #[serde(rename = "subscription_capacity")]
    SubscriptionCapacity,
    #[serde(rename = "subscription_duplicate")]
    SubscriptionDuplicate,
    #[serde(rename = "unknown_subscription")]
    UnknownSubscription,
    #[serde(rename = "cursor_ahead")]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum ConversationMessageDelivery {
    #[serde(rename = "record_only")]
    RecordOnly,
    #[serde(rename = "next_turn")]
    NextTurn,
}
impl ConversationMessageDelivery {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RecordOnly => "record_only",
            Self::NextTurn => "next_turn",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum ConversationMessageState {
    #[serde(rename = "recorded")]
    Recorded,
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "assigned")]
    Assigned,
    #[serde(rename = "retired")]
    Retired,
}
impl ConversationMessageState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Recorded => "recorded",
            Self::Pending => "pending",
            Self::Assigned => "assigned",
            Self::Retired => "retired",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum ConversationMessageRetiredReason {
    #[serde(rename = "expired")]
    Expired,
    #[serde(rename = "permission_lost")]
    PermissionLost,
}
impl ConversationMessageRetiredReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Expired => "expired",
            Self::PermissionLost => "permission_lost",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationUnreadable {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_execution_id: Option<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationNote {
    pub message_id: String,
    pub text: String,
    pub accepted_at_ms: u64,
    pub delivery: ConversationMessageDelivery,
    pub delivery_state: ConversationMessageState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retired_reason: Option<ConversationMessageRetiredReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_execution_id: Option<String>,
    pub author: ConversationNoteAuthor,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum ConversationNoteAuthorKind {
    #[serde(rename = "owner")]
    Owner,
    #[serde(rename = "grant")]
    Grant,
}
impl ConversationNoteAuthorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Grant => "grant",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationNoteAuthor {
    pub kind: ConversationNoteAuthorKind,
    pub surface: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationMessageParams {
    pub conversation_id: String,
    pub request_id: String,
    pub text: String,
    pub delivery: ConversationMessageDelivery,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationMessageReceipt {
    pub request_id: String,
    pub message_id: String,
    pub accepted_at_ms: u64,
    pub delivery_state: ConversationMessageState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retired_reason: Option<ConversationMessageRetiredReason>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationMessageStatusParams {
    pub conversation_id: String,
    pub request_id: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationMessageStatusResult {
    pub found: bool,
    pub request_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_state: Option<ConversationMessageState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retired_reason: Option<ConversationMessageRetiredReason>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum PermissionRuleEffect {
    #[serde(rename = "allow")]
    Allow,
    #[serde(rename = "deny")]
    Deny,
}
impl PermissionRuleEffect {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PermissionRule {
    pub id: String,
    pub revision: String,
    pub workspace: String,
    pub source: String,
    pub tool: String,
    pub effect: PermissionRuleEffect,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationPermissionRulesParams {
    pub request_id: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationPermissionRulesResult {
    pub request_id: String,
    pub rules: Vec<PermissionRule>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationRevokePermissionRuleParams {
    pub request_id: String,
    pub rule_id: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationRevokePermissionRuleResult {
    pub request_id: String,
    pub applied: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PermissionReviewNotice {
    pub conversation_id: String,
    pub execution_id: String,
    pub permission_id: String,
    pub tool: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PermissionReviewWaiting {
    pub reviews: Vec<PermissionReviewNotice>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum ConversationEventKind {
    #[serde(rename = "turn.accepted")]
    TurnAccepted,
    #[serde(rename = "turn.started")]
    TurnStarted,
    #[serde(rename = "turn.ended")]
    TurnEnded,
    #[serde(rename = "tool.updated")]
    ToolUpdated,
    #[serde(rename = "state.changed")]
    StateChanged,
}
impl ConversationEventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TurnAccepted => "turn.accepted",
            Self::TurnStarted => "turn.started",
            Self::TurnEnded => "turn.ended",
            Self::ToolUpdated => "tool.updated",
            Self::StateChanged => "state.changed",
        }
    }
}
pub type ConversationEventCursor = String;
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationEvent {
    pub id: ConversationEventCursor,
    pub cursor: ConversationEventCursor,
    pub kind: ConversationEventKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationSubscribeEventsParams {
    pub conversation_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<ConversationEventCursor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replay: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kinds: Option<Vec<ConversationEventKind>>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationEvents {
    pub subscription_id: ConversationSubscriptionId,
    pub cursor: ConversationEventCursor,
    pub events: Vec<ConversationEvent>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationSetModelParams {
    pub conversation_id: String,
    pub request_id: String,
    pub model: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationSetModelResult {
    pub request_id: String,
    pub model: String,
    pub approval_mode: ApprovalMode,
    pub effect: ModelChangeEffect,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum ModelChangeEffect {
    #[serde(rename = "subsequentResponses")]
    SubsequentResponses,
    #[serde(rename = "nextTurn")]
    NextTurn,
}
impl ModelChangeEffect {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SubsequentResponses => "subsequentResponses",
            Self::NextTurn => "nextTurn",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelChange {
    pub request_id: String,
    pub status: String,
    pub requested_model: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum WorkspaceArrangement {
    #[serde(rename = "columns")]
    Columns,
    #[serde(rename = "sidebar")]
    Sidebar,
}
impl WorkspaceArrangement {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Columns => "columns",
            Self::Sidebar => "sidebar",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum WorkspaceShowing {
    #[serde(rename = "panes")]
    Panes,
    #[serde(rename = "agents")]
    Agents,
    #[serde(rename = "widget")]
    Widget,
}
impl WorkspaceShowing {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Panes => "panes",
            Self::Agents => "agents",
            Self::Widget => "widget",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceWidget {
    pub plugin: String,
    pub id: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum WorkspacePaneKind {
    #[serde(rename = "conversation")]
    Conversation,
    #[serde(rename = "draft")]
    Draft,
    #[serde(rename = "widget")]
    Widget,
}
impl WorkspacePaneKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Conversation => "conversation",
            Self::Draft => "draft",
            Self::Widget => "widget",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspacePaneItem {
    pub kind: WorkspacePaneKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub widget: Option<WorkspaceWidget>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspacePane {
    pub height_percent: u64,
    pub focused: bool,
    pub item: WorkspacePaneItem,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceColumn {
    pub width_percent: u64,
    pub panes: Vec<WorkspacePane>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceWindow {
    pub arrangement: WorkspaceArrangement,
    pub sidebar: bool,
    pub session_list: bool,
    pub showing: WorkspaceShowing,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub widget: Option<WorkspaceWidget>,
    pub columns: Vec<WorkspaceColumn>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspacePublishParams {
    pub window_id: String,
    pub revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<WorkspaceWindow>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspacePublishResult {
    pub revision: u64,
    pub applied: bool,
}
pub const MAX_PEERS: usize = 64;
pub const MAX_CONNECTION_CONVERSATION_SUBSCRIPTIONS: usize = 8;
pub const MAX_CONNECTION_LIST_SUBSCRIPTIONS: usize = 2;
pub const SUBSCRIPTION_DELIVERY_TIMEOUT_MS: u64 = 10000;
pub const CONVERSATION_SUBSCRIPTION_ID_PATTERN: &str = "^[1-9][0-9]{0,19}$";
pub const MIN_CONVERSATION_SUBSCRIPTION_ID_CHARACTERS: usize = 1;
pub const MAX_CONVERSATION_SUBSCRIPTION_ID_CHARACTERS: usize = 20;
pub const MIN_VIEW_CURSOR_INCARNATION_CHARACTERS: usize = 1;
pub const MAX_VIEW_CURSOR_INCARNATION_CHARACTERS: usize = 128;
/// Published `transcriptState` values. A view carrying any other is outside the contract.
pub const TRANSCRIPT_STATES: &[&str] = &[
    "not_loaded",
    "partial",
    "complete_empty",
    "complete",
    "stale",
    "unknown",
];
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
pub const MIN_NATIVE_HOOKS_REQUEST_ID_CHARACTERS: usize = 1;
/// Published bound from the product schema.
pub const MAX_NATIVE_HOOKS_REQUEST_ID_BYTES: usize = 256;
/// Published bound from the product schema.
pub const MAX_PHYSICAL_RECORD_PAYLOAD_BYTES: usize = 65546;
/// Published bound from the product schema.
pub const MAX_RECORD_PAGE_RECORDS: usize = 16;
/// Published bound from the product schema.
pub const MAX_RECORD_PAGE_PAYLOAD_BYTES: usize = 65546;
/// Published bound from the product schema.
pub const MAX_RECORD_RESPONSE_BYTES: usize = 131072;
/// Published bound from the product schema.
pub const MAX_SHARE_PAGE: usize = 8;
/// Published bound from the product schema.
pub const MAX_REVIEW_RANGE_BYTES: usize = 32768;
/// Published bound from the product schema.
pub const MAX_LISTED_REVIEWS: usize = 144;
/// Published bound from the product schema.
pub const MAX_REVIEW_ENTRY_TOOL_NAME_BYTES: usize = 64;
/// Published bound from the product schema.
pub const MAX_QUESTION_ENTRY_MESSAGE_BYTES: usize = 48;
/// Published bound from the product schema.
pub const MAX_MCP_CONTEXT_BYTES: usize = 8192;
/// Published bound from the product schema.
pub const MIN_MCP_MESSAGE_CHARACTERS: usize = 1;
/// Published request-frame maximum from the product schema, in bytes.
pub const MAX_PAYLOAD_BYTES: i64 = 65536;
/// Published decimal shape from the product schema, including a view cursor position.
pub const DECIMAL_U64_PATTERN: &str = "^(0|[1-9][0-9]{0,19})$";
/// Published decimal width from the product schema, in characters.
pub const MAX_DECIMAL_U64_CHARACTERS: usize = 20;
/// Published passive read timing from the product schema, in milliseconds.
pub const PASSIVE_READ_TIMEOUT_MS: u64 = 10000;
/// Published passive read timing from the product schema, in milliseconds.
pub const PASSIVE_DELIVERY_TIMEOUT_MS: u64 = 30000;
/// Published passive read timing from the product schema, in milliseconds.
pub const PASSIVE_CLIENT_ALLOWANCE_MS: u64 = 5000;
/// Published passive read timing from the product schema, in milliseconds.
pub const PASSIVE_MIN_REQUEST_TIMEOUT_MS: u64 = 45000;
/// Event filter bounds published by the product schema.
pub const MIN_EVENT_SUBSCRIPTION_KINDS: usize = 1;
pub const MAX_EVENT_SUBSCRIPTION_KINDS: usize = 5;
pub const UNIQUE_EVENT_SUBSCRIPTION_KINDS: bool = true;
/// Optional event subscription fields whose published schema forbids explicit null.
pub const NON_NULL_EVENT_SUBSCRIPTION_OPTIONS: &[&str] = &["after", "replay", "kinds"];
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
    pub const CONVERSATION_MESSAGE: &str = "conversation.message";
    pub const CONVERSATION_MESSAGE_STATUS: &str = "conversation.messageStatus";
    pub const CONVERSATION_ANSWER: &str = "conversation.answer";
    pub const CONVERSATION_PERMISSION_REVIEW: &str = "conversation.permissionReview";
    pub const CONVERSATION_QUESTION_REVIEW: &str = "conversation.questionReview";
    pub const CONVERSATION_ANSWER_QUESTION: &str = "conversation.answerQuestion";
    pub const CONVERSATION_CANCEL: &str = "conversation.cancel";
    pub const CONVERSATION_CLOSE: &str = "conversation.close";
    pub const CONVERSATION_ARCHIVE: &str = "conversation.archive";
    pub const CONVERSATION_UNARCHIVE: &str = "conversation.unarchive";
    pub const CONVERSATION_DELETE: &str = "conversation.delete";
    pub const CONVERSATION_REORDER: &str = "conversation.reorder";
    pub const ARTIFACT_READ: &str = "artifact.read";
    pub const ATTACHMENT_BEGIN: &str = "attachment.begin";
    pub const AGENTS_LIST: &str = "agents.list";
    pub const AGENTS_SET_NATIVE_HOOKS: &str = "agents.setNativeHooks";
    pub const AGENTS_SUBSCRIBE: &str = "agents.subscribe";
    pub const AGENTS_UNSUBSCRIBE: &str = "agents.unsubscribe";
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
    pub const PHONE_ENDPOINT: &str = "phone.endpoint";
    pub const PHONE_AUTHORIZE: &str = "phone.authorize";
    pub const PHONE_STATUS: &str = "phone.status";
    pub const PAIRING_CREATE: &str = "pairing.create";
    pub const PAIRING_PENDING: &str = "pairing.pending";
    pub const PAIRING_STATUS: &str = "pairing.status";
    pub const PAIRING_APPROVE: &str = "pairing.approve";
    pub const PAIRING_DENY: &str = "pairing.deny";
    pub const PAIRING_CANCEL: &str = "pairing.cancel";
    pub const PAIRING_SET_LINKING: &str = "pairing.setLinking";
    pub const PEER_ENROLL: &str = "peer.enroll";
    pub const PEER_LIST: &str = "peer.list";
    pub const PEER_FORGET: &str = "peer.forget";
    pub const PEER_RELAY: &str = "peer.relay";
    pub const CONVERSATION_SUBSCRIBE: &str = "conversation.subscribe";
    pub const CONVERSATION_SUBSCRIBE_LIST: &str = "conversation.subscribeList";
    pub const CONVERSATION_UNSUBSCRIBE: &str = "conversation.unsubscribe";
    pub const CONVERSATION_SHARE: &str = "conversation.share";
    pub const CONVERSATION_UNSHARE: &str = "conversation.unshare";
    pub const CONVERSATION_SHARES: &str = "conversation.shares";
    pub const CONVERSATION_PERMISSION_RULES: &str = "conversation.permissionRules";
    pub const CONVERSATION_REVOKE_PERMISSION_RULE: &str = "conversation.revokePermissionRule";
    pub const WORKSPACE_PUBLISH: &str = "workspace.publish";
    pub const CONVERSATION_SUBSCRIBE_EVENTS: &str = "conversation.subscribeEvents";
    pub const CONVERSATION_PEEK: &str = "conversation.peek";
    pub const CONVERSATION_SET_MODEL: &str = "conversation.setModel";
}
pub mod product_event {
    pub const SESSION_CHALLENGE: &str = "session.challenge";
    pub const SESSION_TERMINATED: &str = "session.terminated";
    pub const CONVERSATION_VIEW: &str = "conversation.view";
    pub const CONVERSATION_LISTED: &str = "conversation.listed";
    pub const AGENTS_LISTED: &str = "agents.listed";
    pub const CONVERSATION_SUBSCRIPTION_ENDED: &str = "conversation.subscriptionEnded";
    pub const PERMISSION_REVIEW_WAITING: &str = "permission.reviewWaiting";
    pub const CONVERSATION_EVENTS: &str = "conversation.events";
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
                items.len() <= 77
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
    "conversation.message",
    "conversation.messageStatus",
    "conversation.answer",
    "conversation.permissionReview",
    "conversation.questionReview",
    "conversation.answerQuestion",
    "conversation.cancel",
    "conversation.close",
    "conversation.archive",
    "conversation.unarchive",
    "conversation.delete",
    "conversation.reorder",
    "artifact.read",
    "attachment.begin",
    "agents.list",
    "agents.setNativeHooks",
    "agents.subscribe",
    "agents.unsubscribe",
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
    "phone.endpoint",
    "phone.authorize",
    "phone.status",
    "pairing.create",
    "pairing.pending",
    "pairing.status",
    "pairing.approve",
    "pairing.deny",
    "pairing.cancel",
    "pairing.setLinking",
    "peer.enroll",
    "peer.list",
    "peer.forget",
    "peer.relay",
    "conversation.subscribe",
    "conversation.subscribeList",
    "conversation.unsubscribe",
    "conversation.share",
    "conversation.unshare",
    "conversation.shares",
    "conversation.permissionRules",
    "conversation.revokePermissionRule",
    "workspace.publish",
    "conversation.subscribeEvents",
    "conversation.peek",
    "conversation.setModel",
];
/// The grant Cedar is asked for before this method is dispatched.
///
/// Generated from `protocol/product/manifest.json`. Writing
/// to a conversation, including answering its question, uploading into it, and
/// an app's calls, asks for `conversation.write`. Reading its records or
/// catalogue asks for `conversation.read`. Running a configured server with
/// the gateway's authority, and enrolling a device, asks for
/// `credential.manage`; Auth asks again for the exact consent. `None`
/// means another owner admits the method: the handshake or `auth.session`.
pub fn action_for_method(method: &str) -> Option<&'static str> {
    match method {
        "server.health"
        | "agents.list"
        | "agents.subscribe"
        | "agents.unsubscribe"
        | "agents.installOptions"
        | "phone.endpoint" => Some("server.read"),
        "credential.issue"
        | "credential.list"
        | "credential.revoke"
        | "agents.setNativeHooks"
        | "mcpServers.list"
        | "mcpServers.save"
        | "mcpServers.remove"
        | "mcpServers.inspect"
        | "mcpServers.authorize"
        | "mcpServers.revoke"
        | "phone.authorize"
        | "phone.status"
        | "pairing.create"
        | "pairing.pending"
        | "pairing.status"
        | "pairing.approve"
        | "pairing.deny"
        | "pairing.cancel"
        | "pairing.setLinking"
        | "peer.enroll"
        | "peer.list"
        | "peer.forget"
        | "peer.relay"
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
        | "conversation.message"
        | "conversation.messageStatus"
        | "conversation.answer"
        | "conversation.permissionReview"
        | "conversation.questionReview"
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
        | "conversation.unsubscribe"
        | "conversation.permissionRules"
        | "conversation.revokePermissionRule"
        | "workspace.publish"
        | "conversation.subscribeEvents"
        | "conversation.peek"
        | "conversation.setModel" => Some("conversation.write"),
        "conversation.recordsHead"
        | "conversation.recordsPage"
        | "artifact.read"
        | "conversation.catalogueHead"
        | "conversation.catalogueManifest"
        | "conversation.catalogueResolve" => Some("conversation.read"),
        _ => None,
    }
}
pub const PRODUCT_VERSION: u64 = 1;
pub const PRODUCT_SESSION_PATH: &str = "/session";
