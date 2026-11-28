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
pub struct UploadedFileAttachment {
    pub digest: String,
    pub mime_type: String,
    pub size: u64,
    pub name: String,
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
pub struct AttachmentWriteParams {
    pub ticket: String,
    pub offset: u64,
    pub data: String,
    pub last: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AttachmentWriteResult {
    pub received: u64,
    #[serde(deserialize_with = "Option::deserialize")]
    pub digest: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub mime_type: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub size: Option<u64>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationUsageLimit {
    pub provider: String,
    pub recovery: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_resuming: Option<bool>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationMessage {
    pub execution_id: String,
    pub user_text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retried: Option<bool>,
    pub attachments: Vec<ImageAttachment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uploads: Option<Vec<UploadedFileAttachment>>,
    pub files: Vec<LinkedFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<ConversationMessageApp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<ConversationAgentSource>,
    pub status: ConversationMessageStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authentication_required: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_work_failed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_limit: Option<ConversationUsageLimit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steering_target: Option<String>,
    pub parts: Vec<ConversationPart>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steering_offset: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starts_at: Option<ConversationPartPosition>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationPending {
    pub execution_id: String,
    pub text: String,
    pub attachments: Vec<ImageAttachment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uploads: Option<Vec<UploadedFileAttachment>>,
    pub files: Vec<LinkedFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<ConversationMessageApp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<ConversationAgentSource>,
    pub mode: ConversationPendingMode,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationAgentSource {
    pub conversation_id: String,
    pub execution_id: String,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_task_id: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changes: Option<Vec<ConversationToolChange>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changes_cut: Option<bool>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationToolChange {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    pub after: String,
    pub cut: bool,
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
pub struct ConversationSetup {
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_id: Option<String>,
    pub steps: Vec<ConversationSetupStep>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationSetupStep {
    pub step: String,
    pub started_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at_ms: Option<u64>,
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
    pub earlier: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interaction_view_error: Option<String>,
    pub queue_complete: bool,
    pub transcript_state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<ConversationRuntime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup: Option<Vec<ConversationSetup>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease: Option<ConversationLease>,
    pub artifacts: Vec<ConversationArtifact>,
    pub notes: Vec<ConversationNote>,
    pub handoffs: Vec<ConversationHandoff>,
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
    pub usage_limit_auto_continue: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_change_effect: Option<ModelChangeEffect>,
    pub model: String,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<String>,
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
pub struct ConversationPartPosition {
    pub part: u64,
    pub byte: u64,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationPoint {
    pub execution_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<ConversationPartPosition>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationReadEarlierParams {
    pub conversation_id: String,
    pub before: ConversationPoint,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<ConversationPoint>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationEarlier {
    pub conversation_id: String,
    pub messages: Vec<ConversationMessage>,
    pub tools: Vec<ConversationTool>,
    pub handoffs: Vec<ConversationHandoff>,
    pub more: bool,
    pub truncated: bool,
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
pub struct OwnerAsk {
    pub ask_id: String,
    pub question: String,
    pub choices: Vec<OwnerAskChoice>,
    pub recommended: u64,
    pub reason: String,
    pub asked_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<OwnerAskAccess>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_pick: Option<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OwnerAskAccess {
    pub mode: String,
    pub grants: Vec<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OwnerAskChoice {
    pub label: String,
    pub consequence: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pros: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cons: Option<Vec<String>>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OwnerDecision {
    pub ask: OwnerAsk,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<OwnerDecisionOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_pick_paused: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_pick_due_at_ms: Option<u64>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum OwnerDecisionState {
    #[serde(rename = "decided")]
    Decided,
    #[serde(rename = "replaced")]
    Replaced,
}
impl OwnerDecisionState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Decided => "decided",
            Self::Replaced => "replaced",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OwnerDecisionOutcome {
    pub state: OwnerDecisionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choice: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto: Option<bool>,
    pub at_ms: u64,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask: Option<OwnerAsk>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationListResult {
    pub conversations: Vec<ConversationSummary>,
    pub complete: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared_conversation_ids: Option<Vec<String>>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uploads: Option<Vec<UploadedFileAttachment>>,
    pub files: Vec<LinkedFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spoken: Option<bool>,
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
    pub uploads: Option<Vec<UploadedFileAttachment>>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<String>,
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
pub struct ConversationRenameParams {
    pub conversation_id: String,
    pub request_id: String,
    pub title: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationDecisionsParams {
    pub conversation_id: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationPauseAutoPickParams {
    pub conversation_id: String,
    pub ask_id: String,
    pub paused: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationPauseAutoPickResult {
    pub applied: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationDecisionSettingsParams {}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecisionSettings {
    pub auto_pick: bool,
    pub auto_pick_after_seconds: u64,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationDecisionsResult {
    pub decisions: Vec<OwnerDecision>,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum ConversationNoticeKind {
    #[serde(rename = "review")]
    Review,
    #[serde(rename = "hook")]
    Hook,
    #[serde(rename = "deadline")]
    Deadline,
}
impl ConversationNoticeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Review => "review",
            Self::Hook => "hook",
            Self::Deadline => "deadline",
        }
    }
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
    pub notice_kind: Option<ConversationNoticeKind>,
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
pub struct AppsTrustedResult {
    pub servers: Vec<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppsUntrustParams {
    pub request_id: String,
    pub server: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppsUntrustResult {
    pub request_id: String,
    pub applied: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppPackageAsset {
    pub path: String,
    pub bytes: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppPackage {
    pub manifest: String,
    pub assets: Vec<AppPackageAsset>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppReview {
    pub app_id: String,
    pub package_version: String,
    pub entrypoint: String,
    pub asset_count: u64,
    pub digest: String,
    pub requested_capabilities: Vec<AppCapability>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum AppCapability {
    #[serde(rename = "list")]
    List,
    #[serde(rename = "read")]
    Read,
    #[serde(rename = "create")]
    Create,
    #[serde(rename = "subscribe")]
    Subscribe,
    #[serde(rename = "stop")]
    Stop,
    #[serde(rename = "send")]
    Send,
}
impl AppCapability {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Read => "read",
            Self::Create => "create",
            Self::Subscribe => "subscribe",
            Self::Stop => "stop",
            Self::Send => "send",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppsStarterParams {
    pub app_id: String,
    pub package_version: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppsPackageResult {
    pub package: AppPackage,
    pub review: AppReview,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppsReviewParams {
    pub package: AppPackage,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppsInstallParams {
    pub request_id: String,
    pub package: AppPackage,
    pub reviewed_digest: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstalledApp {
    pub app_id: String,
    pub package_version: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppsInstallResult {
    pub request_id: String,
    pub app: InstalledApp,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppsInstalledResult {
    pub apps: Vec<InstalledApp>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppsReadParams {
    pub app_id: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppsUninstallParams {
    pub request_id: String,
    pub app_id: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppsUninstallResult {
    pub request_id: String,
    pub app_id: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppsIssueCredentialParams {
    pub request_id: String,
    pub app_id: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppsIssueCredentialResult {
    pub request_id: String,
    pub app_id: String,
    pub credential_id: String,
    pub credential: String,
    pub expires_at: u64,
    pub capabilities: Vec<AppCapability>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppsRevokeCredentialsParams {
    pub request_id: String,
    pub app_id: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppsRevokeCredentialsResult {
    pub request_id: String,
    pub app_id: String,
    pub revoked: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum AppsErrorCode {
    #[serde(rename = "apps_not_configured")]
    AppsNotConfigured,
    #[serde(rename = "app_package_invalid")]
    AppPackageInvalid,
    #[serde(rename = "app_package_too_large")]
    AppPackageTooLarge,
    #[serde(rename = "app_review_stale")]
    AppReviewStale,
    #[serde(rename = "app_already_installed")]
    AppAlreadyInstalled,
    #[serde(rename = "apps_limit_reached")]
    AppsLimitReached,
    #[serde(rename = "app_not_installed")]
    AppNotInstalled,
    #[serde(rename = "apps_storage_unavailable")]
    AppsStorageUnavailable,
    #[serde(rename = "apps_audit_unavailable")]
    AppsAuditUnavailable,
    #[serde(rename = "app_requests_nothing")]
    AppRequestsNothing,
    #[serde(rename = "app_credential_conflict")]
    AppCredentialConflict,
    #[serde(rename = "app_credentials_unavailable")]
    AppCredentialsUnavailable,
}
impl AppsErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AppsNotConfigured => "apps_not_configured",
            Self::AppPackageInvalid => "app_package_invalid",
            Self::AppPackageTooLarge => "app_package_too_large",
            Self::AppReviewStale => "app_review_stale",
            Self::AppAlreadyInstalled => "app_already_installed",
            Self::AppsLimitReached => "apps_limit_reached",
            Self::AppNotInstalled => "app_not_installed",
            Self::AppsStorageUnavailable => "apps_storage_unavailable",
            Self::AppsAuditUnavailable => "apps_audit_unavailable",
            Self::AppRequestsNothing => "app_requests_nothing",
            Self::AppCredentialConflict => "app_credential_conflict",
            Self::AppCredentialsUnavailable => "app_credentials_unavailable",
        }
    }
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
pub struct ConversationSetUsageLimitAutoContinueParams {
    pub request_id: String,
    pub enabled: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationSetUsageLimitAutoContinueResult {
    pub request_id: String,
    pub enabled: bool,
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
    #[serde(rename = "ownerPhone")]
    OwnerPhone,
    #[serde(rename = "gateway")]
    Gateway,
}
impl PairingEnrollee {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Device => "device",
            Self::OwnerPhone => "ownerPhone",
            Self::Gateway => "gateway",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PairingCreateParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub same_person: Option<bool>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub same_person: Option<bool>,
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
pub enum PairingTunnelMode {
    #[serde(rename = "quick")]
    Quick,
    #[serde(rename = "named")]
    Named,
}
impl PairingTunnelMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Quick => "quick",
            Self::Named => "named",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PairingSetTunnelParams {
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<PairingTunnelMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PairingSetTunnelResult {
    pub enabled: bool,
    pub mode: PairingTunnelMode,
    pub applies: PairingLinkingApply,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum PairingTunnelStatus {
    #[serde(rename = "off")]
    Off,
    #[serde(rename = "starting")]
    Starting,
    #[serde(rename = "ready")]
    Ready,
    #[serde(rename = "failed")]
    Failed,
}
impl PairingTunnelStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Starting => "starting",
            Self::Ready => "ready",
            Self::Failed => "failed",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PairingTunnelResult {
    pub enabled: bool,
    pub mode: PairingTunnelMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    pub has_token: bool,
    pub ingress: String,
    pub status: PairingTunnelStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeviceMachine {
    pub hostname: String,
    pub os: String,
    pub os_version: String,
    pub arch: String,
    pub cpus: u64,
    pub cpu_permille: u64,
    pub memory_total_bytes: u64,
    pub memory_used_bytes: u64,
    pub disk_total_bytes: u64,
    pub disk_used_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disks: Option<Vec<DeviceDisk>>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeviceDisk {
    pub mount: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DevicesListResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub: Option<DeviceMachine>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listening: Option<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServerReachResult {
    #[serde(deserialize_with = "Option::deserialize")]
    pub tunnel: Option<String>,
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
    #[serde(rename = "pairing_linking_off")]
    PairingLinkingOff,
    #[serde(rename = "pairing_tunnel_incomplete")]
    PairingTunnelIncomplete,
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
            Self::PairingLinkingOff => "pairing_linking_off",
            Self::PairingTunnelIncomplete => "pairing_tunnel_incomplete",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GithubAccount {
    pub id: u64,
    pub login: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum GithubConnectState {
    #[serde(rename = "signed_out")]
    SignedOut,
    #[serde(rename = "connecting")]
    Connecting,
    #[serde(rename = "connected")]
    Connected,
}
impl GithubConnectState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SignedOut => "signed_out",
            Self::Connecting => "connecting",
            Self::Connected => "connected",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum GithubConnectEnd {
    #[serde(rename = "expired")]
    Expired,
    #[serde(rename = "denied")]
    Denied,
    #[serde(rename = "cancelled")]
    Cancelled,
    #[serde(rename = "rejected")]
    Rejected,
    #[serde(rename = "unreachable")]
    Unreachable,
    #[serde(rename = "store_unavailable")]
    StoreUnavailable,
    #[serde(rename = "audit_unavailable")]
    AuditUnavailable,
}
impl GithubConnectEnd {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Expired => "expired",
            Self::Denied => "denied",
            Self::Cancelled => "cancelled",
            Self::Rejected => "rejected",
            Self::Unreachable => "unreachable",
            Self::StoreUnavailable => "store_unavailable",
            Self::AuditUnavailable => "audit_unavailable",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GithubCode {
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in_seconds: u64,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GithubLiveEvents {
    pub enabled: bool,
    pub repositories: u64,
    pub gaps: u64,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GithubLiveEventsParams {
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accept_gaps: Option<bool>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GithubConnection {
    pub state: GithubConnectState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_events: Option<GithubLiveEvents>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<GithubAccount>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<GithubCode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<GithubConnectEnd>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unrecorded: Option<bool>,
    pub install_url: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum GithubErrorCode {
    #[serde(rename = "github_not_configured")]
    GithubNotConfigured,
    #[serde(rename = "github_unreachable")]
    GithubUnreachable,
    #[serde(rename = "github_rejected")]
    GithubRejected,
    #[serde(rename = "github_storage_unavailable")]
    GithubStorageUnavailable,
    #[serde(rename = "github_audit_unavailable")]
    GithubAuditUnavailable,
    #[serde(rename = "github_live_unavailable")]
    GithubLiveUnavailable,
    #[serde(rename = "github_live_capacity")]
    GithubLiveCapacity,
    #[serde(rename = "vault_locked")]
    VaultLocked,
}
impl GithubErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::GithubNotConfigured => "github_not_configured",
            Self::GithubUnreachable => "github_unreachable",
            Self::GithubRejected => "github_rejected",
            Self::GithubStorageUnavailable => "github_storage_unavailable",
            Self::GithubAuditUnavailable => "github_audit_unavailable",
            Self::GithubLiveUnavailable => "github_live_unavailable",
            Self::GithubLiveCapacity => "github_live_capacity",
            Self::VaultLocked => "vault_locked",
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
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PeerFollowParams {
    pub peer_key: [u8; DeviceKey::LENGTH],
    pub conversations: Vec<String>,
    pub keep_bytes: u64,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PeerFollowResult {
    pub peer_key: [u8; DeviceKey::LENGTH],
    pub conversations: Vec<String>,
    pub keep_bytes: u64,
    pub replica_reset: bool,
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
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GrantStreamCursor {
    pub incarnation: String,
    pub position: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GrantsSubscribeParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<GrantStreamCursor>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GrantsPageParams {
    pub subscription_id: ConversationSubscriptionId,
    pub snapshot_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<GrantStreamCursor>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GrantsUnsubscribeParams {
    pub subscription_id: ConversationSubscriptionId,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GrantFrameEntry {
    pub conversation_id: String,
    pub absent: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_revision: Option<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GrantsChanged {
    pub subscription_id: ConversationSubscriptionId,
    pub cursor: GrantStreamCursor,
    pub complete: bool,
    pub replaces: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_id: Option<String>,
    pub grants: Vec<GrantFrameEntry>,
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
    #[serde(rename = "incarnation_changed")]
    IncarnationChanged,
    #[serde(rename = "grant_log_floor")]
    GrantLogFloor,
}
impl ConversationSubscriptionErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SubscriptionCapacity => "subscription_capacity",
            Self::SubscriptionDuplicate => "subscription_duplicate",
            Self::UnknownSubscription => "unknown_subscription",
            Self::CursorAhead => "cursor_ahead",
            Self::IncarnationChanged => "incarnation_changed",
            Self::GrantLogFloor => "grant_log_floor",
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
    #[serde(rename = "agent")]
    Agent,
}
impl ConversationNoteAuthorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Grant => "grant",
            Self::Agent => "agent",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationNoteAuthor {
    pub kind: ConversationNoteAuthorKind,
    pub surface: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<ConversationAgentSource>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationHandoff {
    pub handoff_id: String,
    pub kind: String,
    pub at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_execution_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<ConversationHandoffAgent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<ConversationHandoffAgent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turns: Option<Vec<ConversationCarriedTurn>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub omitted_turns: Option<u64>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationHandoffAgent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    pub model: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationCarriedTurn {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<ConversationAgentSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub said: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
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
pub struct ConversationSwitchAgentParams {
    pub conversation_id: String,
    pub request_id: String,
    pub agent: String,
    pub model: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationForkParams {
    pub conversation_id: String,
    pub request_id: String,
    pub source_conversation_id: String,
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
    #[serde(rename = "terminal")]
    Terminal,
}
impl WorkspacePaneKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Conversation => "conversation",
            Self::Draft => "draft",
            Self::Widget => "widget",
            Self::Terminal => "terminal",
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
pub struct WorkspacePanel {
    pub open: bool,
    pub tabs: Vec<WorkspaceWidget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub front: Option<WorkspaceWidget>,
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
    pub panel: WorkspacePanel,
    pub columns: Vec<WorkspaceColumn>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum ChannelErrorCode {
    #[serde(rename = "invalid_request")]
    InvalidRequest,
    #[serde(rename = "forbidden")]
    Forbidden,
    #[serde(rename = "conversations_not_configured")]
    ConversationsNotConfigured,
    #[serde(rename = "channel_folder_not_found")]
    ChannelFolderNotFound,
    #[serde(rename = "channel_folder_not_a_directory")]
    ChannelFolderNotADirectory,
    #[serde(rename = "channel_folder_refused")]
    ChannelFolderRefused,
    #[serde(rename = "channel_resource_not_found")]
    ChannelResourceNotFound,
    #[serde(rename = "channel_limit_reached")]
    ChannelLimitReached,
    #[serde(rename = "channel_request_conflict")]
    ChannelRequestConflict,
    #[serde(rename = "channel_storage_unavailable")]
    ChannelStorageUnavailable,
    #[serde(rename = "temporarily_unavailable")]
    TemporarilyUnavailable,
}
impl ChannelErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::Forbidden => "forbidden",
            Self::ConversationsNotConfigured => "conversations_not_configured",
            Self::ChannelFolderNotFound => "channel_folder_not_found",
            Self::ChannelFolderNotADirectory => "channel_folder_not_a_directory",
            Self::ChannelFolderRefused => "channel_folder_refused",
            Self::ChannelResourceNotFound => "channel_resource_not_found",
            Self::ChannelLimitReached => "channel_limit_reached",
            Self::ChannelRequestConflict => "channel_request_conflict",
            Self::ChannelStorageUnavailable => "channel_storage_unavailable",
            Self::TemporarilyUnavailable => "temporarily_unavailable",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Channel {
    pub channel_id: String,
    pub name: String,
    pub folder: String,
    pub folder_made: bool,
    #[serde(deserialize_with = "Option::deserialize")]
    pub agent: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub model: Option<String>,
    pub approval_mode: ApprovalMode,
    pub created_at_ms: u64,
    #[serde(deserialize_with = "Option::deserialize")]
    pub coordinator_conversation_id: Option<String>,
    pub coordinator_on: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChannelCreateParams {
    pub request_id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    pub agent: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_mode: Option<ApprovalMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coordinator: Option<bool>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChannelCreateResult {
    pub channel: Channel,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChannelSetCoordinatorParams {
    pub request_id: String,
    pub channel_id: String,
    pub on: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChannelSetCoordinatorResult {
    pub channel: Channel,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChannelListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChannelListResult {
    pub channels: Vec<Channel>,
    pub complete: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum WorkspaceActionKind {
    #[serde(rename = "sidebar")]
    Sidebar,
    #[serde(rename = "session_list")]
    SessionList,
    #[serde(rename = "panel")]
    Panel,
    #[serde(rename = "panel_tab")]
    PanelTab,
    #[serde(rename = "show")]
    Show,
    #[serde(rename = "open_conversation")]
    OpenConversation,
    #[serde(rename = "focus_pane")]
    FocusPane,
    #[serde(rename = "close_pane")]
    ClosePane,
}
impl WorkspaceActionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sidebar => "sidebar",
            Self::SessionList => "session_list",
            Self::Panel => "panel",
            Self::PanelTab => "panel_tab",
            Self::Show => "show",
            Self::OpenConversation => "open_conversation",
            Self::FocusPane => "focus_pane",
            Self::ClosePane => "close_pane",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum WorkspaceArrangeKind {
    #[serde(rename = "sidebar")]
    Sidebar,
    #[serde(rename = "session_list")]
    SessionList,
    #[serde(rename = "panel")]
    Panel,
    #[serde(rename = "panel_tab")]
    PanelTab,
    #[serde(rename = "show")]
    Show,
    #[serde(rename = "open_conversation")]
    OpenConversation,
    #[serde(rename = "focus_pane")]
    FocusPane,
}
impl WorkspaceArrangeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sidebar => "sidebar",
            Self::SessionList => "session_list",
            Self::Panel => "panel",
            Self::PanelTab => "panel_tab",
            Self::Show => "show",
            Self::OpenConversation => "open_conversation",
            Self::FocusPane => "focus_pane",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum WorkspaceView {
    #[serde(rename = "panes")]
    Panes,
    #[serde(rename = "agents")]
    Agents,
}
impl WorkspaceView {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Panes => "panes",
            Self::Agents => "agents",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum WorkspaceOpenPlace {
    #[serde(rename = "beside")]
    Beside,
    #[serde(rename = "right")]
    Right,
    #[serde(rename = "below")]
    Below,
    #[serde(rename = "here")]
    Here,
}
impl WorkspaceOpenPlace {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Beside => "beside",
            Self::Right => "right",
            Self::Below => "below",
            Self::Here => "here",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceAction {
    pub kind: WorkspaceActionKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<WorkspaceView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub widget: Option<WorkspaceWidget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub place: Option<WorkspaceOpenPlace>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceArrangeAction {
    pub kind: WorkspaceArrangeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<WorkspaceView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub widget: Option<WorkspaceWidget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub place: Option<WorkspaceOpenPlace>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceCommand {
    pub window_id: String,
    pub command_id: u64,
    pub action: WorkspaceAction,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum WorkspaceAnswerOutcome {
    #[serde(rename = "applied")]
    Applied,
    #[serde(rename = "unchanged")]
    Unchanged,
    #[serde(rename = "refused")]
    Refused,
}
impl WorkspaceAnswerOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::Unchanged => "unchanged",
            Self::Refused => "refused",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum WorkspaceCommandRefusal {
    #[serde(rename = "conversation_not_listed")]
    ConversationNotListed,
    #[serde(rename = "not_on_screen")]
    NotOnScreen,
    #[serde(rename = "tab_not_found")]
    TabNotFound,
    #[serde(rename = "no_session_list")]
    NoSessionList,
    #[serde(rename = "no_chat_focused")]
    NoChatFocused,
    #[serde(rename = "no_room")]
    NoRoom,
    #[serde(rename = "covered")]
    Covered,
}
impl WorkspaceCommandRefusal {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ConversationNotListed => "conversation_not_listed",
            Self::NotOnScreen => "not_on_screen",
            Self::TabNotFound => "tab_not_found",
            Self::NoSessionList => "no_session_list",
            Self::NoChatFocused => "no_chat_focused",
            Self::NoRoom => "no_room",
            Self::Covered => "covered",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceAnswerParams {
    pub window_id: String,
    pub command_id: u64,
    pub outcome: WorkspaceAnswerOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<WorkspaceCommandRefusal>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceAnswerResult {
    pub settled: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceArrangeParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_id: Option<String>,
    pub action: WorkspaceArrangeAction,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceClosePaneParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_id: Option<String>,
    pub conversation_id: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceControlResult {
    pub window_id: String,
    pub revision: u64,
    pub changed: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum VaultState {
    #[serde(rename = "unconfigured")]
    Unconfigured,
    #[serde(rename = "locked")]
    Locked,
    #[serde(rename = "unlocked")]
    Unlocked,
}
impl VaultState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unconfigured => "unconfigured",
            Self::Locked => "locked",
            Self::Unlocked => "unlocked",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VaultStatus {
    pub state: VaultState,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum VaultPassphraseChangeState {
    #[serde(rename = "locked")]
    Locked,
}
impl VaultPassphraseChangeState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Locked => "locked",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VaultPassphraseChangeStatus {
    pub state: VaultPassphraseChangeState,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VaultPassphraseParams {
    pub passphrase: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum VaultErrorCode {
    #[serde(rename = "vault_unavailable")]
    VaultUnavailable,
    #[serde(rename = "vault_forbidden")]
    VaultForbidden,
    #[serde(rename = "vault_locked")]
    VaultLocked,
    #[serde(rename = "vault_passphrase_policy")]
    VaultPassphrasePolicy,
    #[serde(rename = "vault_wrong_passphrase")]
    VaultWrongPassphrase,
    #[serde(rename = "vault_busy")]
    VaultBusy,
    #[serde(rename = "vault_cooldown")]
    VaultCooldown,
    #[serde(rename = "vault_session_closed")]
    VaultSessionClosed,
    #[serde(rename = "vault_session_owned")]
    VaultSessionOwned,
    #[serde(rename = "vault_already_configured")]
    VaultAlreadyConfigured,
    #[serde(rename = "vault_capacity")]
    VaultCapacity,
    #[serde(rename = "vault_audit_unavailable")]
    VaultAuditUnavailable,
    #[serde(rename = "vault_migration_unavailable")]
    VaultMigrationUnavailable,
    #[serde(rename = "vault_change_unchanged")]
    VaultChangeUnchanged,
    #[serde(rename = "vault_change_unchanged_audit_unavailable")]
    VaultChangeUnchangedAuditUnavailable,
    #[serde(rename = "vault_change_not_confirmed")]
    VaultChangeNotConfirmed,
    #[serde(rename = "vault_change_not_confirmed_audit_unavailable")]
    VaultChangeNotConfirmedAuditUnavailable,
    #[serde(rename = "vault_changed_audit_unavailable")]
    VaultChangedAuditUnavailable,
}
impl VaultErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::VaultUnavailable => "vault_unavailable",
            Self::VaultForbidden => "vault_forbidden",
            Self::VaultLocked => "vault_locked",
            Self::VaultPassphrasePolicy => "vault_passphrase_policy",
            Self::VaultWrongPassphrase => "vault_wrong_passphrase",
            Self::VaultBusy => "vault_busy",
            Self::VaultCooldown => "vault_cooldown",
            Self::VaultSessionClosed => "vault_session_closed",
            Self::VaultSessionOwned => "vault_session_owned",
            Self::VaultAlreadyConfigured => "vault_already_configured",
            Self::VaultCapacity => "vault_capacity",
            Self::VaultAuditUnavailable => "vault_audit_unavailable",
            Self::VaultMigrationUnavailable => "vault_migration_unavailable",
            Self::VaultChangeUnchanged => "vault_change_unchanged",
            Self::VaultChangeUnchangedAuditUnavailable => {
                "vault_change_unchanged_audit_unavailable"
            }
            Self::VaultChangeNotConfirmed => "vault_change_not_confirmed",
            Self::VaultChangeNotConfirmedAuditUnavailable => {
                "vault_change_not_confirmed_audit_unavailable"
            }
            Self::VaultChangedAuditUnavailable => "vault_changed_audit_unavailable",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BoatSaveParams {
    pub token: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum BoatErrorCode {
    #[serde(rename = "vault_locked")]
    VaultLocked,
    #[serde(rename = "boat_not_configured")]
    BoatNotConfigured,
    #[serde(rename = "invalid_request")]
    InvalidRequest,
    #[serde(rename = "boat_storage_unavailable")]
    BoatStorageUnavailable,
    #[serde(rename = "boat_resource_missing")]
    BoatResourceMissing,
    #[serde(rename = "boat_unreachable")]
    BoatUnreachable,
    #[serde(rename = "boat_worker_unavailable")]
    BoatWorkerUnavailable,
    #[serde(rename = "boat_environment_blocks_credentials")]
    BoatEnvironmentBlocksCredentials,
    #[serde(rename = "boat_provisioning_uncertain")]
    BoatProvisioningUncertain,
    #[serde(rename = "boat_cleanup_uncertain")]
    BoatCleanupUncertain,
}
impl BoatErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::VaultLocked => "vault_locked",
            Self::BoatNotConfigured => "boat_not_configured",
            Self::InvalidRequest => "invalid_request",
            Self::BoatStorageUnavailable => "boat_storage_unavailable",
            Self::BoatResourceMissing => "boat_resource_missing",
            Self::BoatUnreachable => "boat_unreachable",
            Self::BoatWorkerUnavailable => "boat_worker_unavailable",
            Self::BoatEnvironmentBlocksCredentials => "boat_environment_blocks_credentials",
            Self::BoatProvisioningUncertain => "boat_provisioning_uncertain",
            Self::BoatCleanupUncertain => "boat_cleanup_uncertain",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BoatStatus {
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Agent37SaveParams {
    pub key: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum Agent37ErrorCode {
    #[serde(rename = "vault_locked")]
    VaultLocked,
    #[serde(rename = "agent37_not_configured")]
    Agent37NotConfigured,
    #[serde(rename = "invalid_request")]
    InvalidRequest,
    #[serde(rename = "agent37_storage_unavailable")]
    Agent37StorageUnavailable,
    #[serde(rename = "agent37_resource_missing")]
    Agent37ResourceMissing,
    #[serde(rename = "agent37_unreachable")]
    Agent37Unreachable,
    #[serde(rename = "agent37_unavailable")]
    Agent37Unavailable,
    #[serde(rename = "agent37_provisioning_uncertain")]
    Agent37ProvisioningUncertain,
    #[serde(rename = "agent37_cleanup_uncertain")]
    Agent37CleanupUncertain,
}
impl Agent37ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::VaultLocked => "vault_locked",
            Self::Agent37NotConfigured => "agent37_not_configured",
            Self::InvalidRequest => "invalid_request",
            Self::Agent37StorageUnavailable => "agent37_storage_unavailable",
            Self::Agent37ResourceMissing => "agent37_resource_missing",
            Self::Agent37Unreachable => "agent37_unreachable",
            Self::Agent37Unavailable => "agent37_unavailable",
            Self::Agent37ProvisioningUncertain => "agent37_provisioning_uncertain",
            Self::Agent37CleanupUncertain => "agent37_cleanup_uncertain",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Agent37Status {
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum Cloud {
    #[serde(rename = "boat")]
    Boat,
    #[serde(rename = "agent37")]
    Agent37,
}
impl Cloud {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Boat => "boat",
            Self::Agent37 => "agent37",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CloudChooseParams {
    pub cloud: Cloud,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum CloudErrorCode {
    #[serde(rename = "vault_locked")]
    VaultLocked,
    #[serde(rename = "cloud_not_configured")]
    CloudNotConfigured,
    #[serde(rename = "invalid_request")]
    InvalidRequest,
    #[serde(rename = "cloud_storage_unavailable")]
    CloudStorageUnavailable,
}
impl CloudErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::VaultLocked => "vault_locked",
            Self::CloudNotConfigured => "cloud_not_configured",
            Self::InvalidRequest => "invalid_request",
            Self::CloudStorageUnavailable => "cloud_storage_unavailable",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CloudChoice {
    pub cloud: Cloud,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum CloudBoxState {
    #[serde(rename = "provisioning")]
    Provisioning,
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "sleeping")]
    Sleeping,
    #[serde(rename = "refused")]
    Refused,
    #[serde(rename = "deleted")]
    Deleted,
}
impl CloudBoxState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Provisioning => "provisioning",
            Self::Running => "running",
            Self::Sleeping => "sleeping",
            Self::Refused => "refused",
            Self::Deleted => "deleted",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CloudBoxSummary {
    pub box_id: String,
    pub name: String,
    pub cloud: Cloud,
    pub vm: String,
    pub url: String,
    pub state: CloudBoxState,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CloudBoxCreateParams {
    pub box_id: String,
    pub name: String,
    pub cloud: Cloud,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CloudBoxIdParams {
    pub box_id: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CloudBoxRenameParams {
    pub box_id: String,
    pub name: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CloudBoxesListResult {
    pub boxes: Vec<CloudBoxSummary>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CloudBoxCreateResult {
    pub cloud_box: CloudBoxSummary,
    pub credential: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CloudBoxWakeResult {
    pub box_id: String,
    pub url: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CloudBoxResult {
    pub cloud_box: CloudBoxSummary,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CloudBoxDeleteResult {
    pub box_id: String,
    pub deleted: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum CloudBoxErrorCode {
    #[serde(rename = "invalid_request")]
    InvalidRequest,
    #[serde(rename = "vault_locked")]
    VaultLocked,
    #[serde(rename = "cloud_not_configured")]
    CloudNotConfigured,
    #[serde(rename = "cloud_box_not_found")]
    CloudBoxNotFound,
    #[serde(rename = "cloud_box_already_exists")]
    CloudBoxAlreadyExists,
    #[serde(rename = "cloud_box_not_active")]
    CloudBoxNotActive,
    #[serde(rename = "cloud_box_active_work")]
    CloudBoxActiveWork,
    #[serde(rename = "cloud_box_admission_refused")]
    CloudBoxAdmissionRefused,
    #[serde(rename = "cloud_box_bootstrap_failed")]
    CloudBoxBootstrapFailed,
    #[serde(rename = "cloud_box_provider_failed")]
    CloudBoxProviderFailed,
    #[serde(rename = "cloud_box_storage_unavailable")]
    CloudBoxStorageUnavailable,
}
impl CloudBoxErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::VaultLocked => "vault_locked",
            Self::CloudNotConfigured => "cloud_not_configured",
            Self::CloudBoxNotFound => "cloud_box_not_found",
            Self::CloudBoxAlreadyExists => "cloud_box_already_exists",
            Self::CloudBoxNotActive => "cloud_box_not_active",
            Self::CloudBoxActiveWork => "cloud_box_active_work",
            Self::CloudBoxAdmissionRefused => "cloud_box_admission_refused",
            Self::CloudBoxBootstrapFailed => "cloud_box_bootstrap_failed",
            Self::CloudBoxProviderFailed => "cloud_box_provider_failed",
            Self::CloudBoxStorageUnavailable => "cloud_box_storage_unavailable",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum VmLifetimeMode {
    #[serde(rename = "never")]
    Never,
    #[serde(rename = "after")]
    After,
}
impl VmLifetimeMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Never => "never",
            Self::After => "after",
        }
    }
}
pub type VmLifetimeSeconds = u64;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum BoatRuntimeSelection {
    #[serde(rename = "default")]
    Default,
    #[serde(rename = "never")]
    Never,
    #[serde(rename = "after")]
    After,
}
impl BoatRuntimeSelection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Never => "never",
            Self::After => "after",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationWebhookCursorParams {
    pub conversation_id: String,
    pub credential_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_source_head: Option<bool>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationWebhookResetParams {
    pub conversation_id: String,
    pub credential_id: String,
    pub request_id: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationWebhookCursorResult {
    pub scope: RecordScope,
    pub position: String,
    pub reset_required: bool,
    #[serde(deserialize_with = "Option::deserialize")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_source_head: Option<ConversationRecordsHeadResult>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationWebhookResetResult {
    pub request_id: String,
    pub receipt_id: String,
    pub scope: RecordScope,
    pub position: String,
    pub applied_at_ms: u64,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VoiceStatus {
    pub key_saved: bool,
    pub model: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VoiceSaveKeyParams {
    pub key: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VoiceStartCallParams {
    pub conversation_id: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum VoiceTransport {
    #[serde(rename = "webrtc")]
    Webrtc,
    #[serde(rename = "scripted")]
    Scripted,
}
impl VoiceTransport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Webrtc => "webrtc",
            Self::Scripted => "scripted",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum VoiceToolName {
    #[serde(rename = "send_to_agent")]
    SendToAgent,
    #[serde(rename = "stop_agent")]
    StopAgent,
}
impl VoiceToolName {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SendToAgent => "send_to_agent",
            Self::StopAgent => "stop_agent",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VoiceCall {
    pub transport: VoiceTransport,
    pub call_url: String,
    pub client_secret: String,
    pub expires_at: u64,
    pub model: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum VoiceErrorCode {
    #[serde(rename = "voice_not_configured")]
    VoiceNotConfigured,
    #[serde(rename = "voice_key_missing")]
    VoiceKeyMissing,
    #[serde(rename = "voice_key_rejected")]
    VoiceKeyRejected,
    #[serde(rename = "voice_unreachable")]
    VoiceUnreachable,
    #[serde(rename = "voice_storage_unavailable")]
    VoiceStorageUnavailable,
    #[serde(rename = "voice_audit_unavailable")]
    VoiceAuditUnavailable,
    #[serde(rename = "vault_locked")]
    VaultLocked,
}
impl VoiceErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::VoiceNotConfigured => "voice_not_configured",
            Self::VoiceKeyMissing => "voice_key_missing",
            Self::VoiceKeyRejected => "voice_key_rejected",
            Self::VoiceUnreachable => "voice_unreachable",
            Self::VoiceStorageUnavailable => "voice_storage_unavailable",
            Self::VoiceAuditUnavailable => "voice_audit_unavailable",
            Self::VaultLocked => "vault_locked",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum ReplyStyleRole {
    #[serde(rename = "coordinator")]
    Coordinator,
    #[serde(rename = "thread")]
    Thread,
}
impl ReplyStyleRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Coordinator => "coordinator",
            Self::Thread => "thread",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum ReplyStyleMode {
    #[serde(rename = "plain")]
    Plain,
    #[serde(rename = "off")]
    Off,
    #[serde(rename = "custom")]
    Custom,
}
impl ReplyStyleMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::Off => "off",
            Self::Custom => "custom",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReplyStyleChoice {
    pub mode: ReplyStyleMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReplyStyleRoles {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coordinator: Option<ReplyStyleChoice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<ReplyStyleChoice>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReplyStyleReadParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum ReplyStyleSelection {
    #[serde(rename = "default")]
    Default,
    #[serde(rename = "plain")]
    Plain,
    #[serde(rename = "off")]
    Off,
    #[serde(rename = "custom")]
    Custom,
}
impl ReplyStyleSelection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Plain => "plain",
            Self::Off => "off",
            Self::Custom => "custom",
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReplyStyleSetParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<String>,
    pub role: ReplyStyleRole,
    pub mode: ReplyStyleSelection,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReplyStyles {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<String>,
    pub nessa: ReplyStyleRoles,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<ReplyStyleRoles>,
    pub effective: ReplyStyleEffective,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReplyStyleEffective {
    pub coordinator: ReplyStyleChoice,
    pub thread: ReplyStyleChoice,
}
pub type TerminalId = String;
pub type TerminalViewId = String;
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalOpenParams {
    pub request_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    pub cols: u64,
    pub rows: u64,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalSummary {
    pub terminal_id: TerminalId,
    pub title: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    pub request_id: String,
    pub folder: String,
    pub shell: String,
    pub runs_on: String,
    pub started_at_ms: u64,
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_status: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program_status: Option<TerminalProgramStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resumed_at_ms: Option<u64>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalProgramStatus {
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub msg: Option<String>,
    pub updated_at_ms: u64,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalOpenResult {
    pub terminal: TerminalSummary,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalListResult {
    pub terminals: Vec<TerminalSummary>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalListed {
    pub terminals: Vec<TerminalSummary>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalViewParams {
    pub terminal_id: TerminalId,
    pub view_id: TerminalViewId,
    pub cols: u64,
    pub rows: u64,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalDetachParams {
    pub terminal_id: TerminalId,
    pub view_id: TerminalViewId,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalInputParams {
    pub terminal_id: TerminalId,
    pub data: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalHistoryParams {
    pub terminal_id: TerminalId,
    pub view_id: TerminalViewId,
    pub start: u64,
    pub count: u64,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalCloseParams {
    pub terminal_id: TerminalId,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalRenameParams {
    pub terminal_id: TerminalId,
    pub title: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalResumeRule {
    pub program: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    pub action: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalResumeSettings {
    pub enabled: bool,
    pub rules: Vec<TerminalResumeRule>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalSavedProgram {
    pub terminal_id: TerminalId,
    pub title: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    pub saved_at_ms: u64,
    pub comes_back: bool,
    pub open: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalResumeState {
    pub settings: TerminalResumeSettings,
    pub saved: Vec<TerminalSavedProgram>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalForgetSavedParams {
    pub terminal_id: TerminalId,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalOutput {
    pub terminal_id: TerminalId,
    pub view_id: TerminalViewId,
    pub message: String,
}
pub const MAX_PEERS: usize = 64;
pub const MAX_CONNECTION_CONVERSATION_SUBSCRIPTIONS: usize = 8;
pub const MAX_CONNECTION_LIST_SUBSCRIPTIONS: usize = 2;
pub const MAX_CONNECTION_GRANT_SUBSCRIPTIONS: usize = 1;
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
    pub const SERVER_REACH: &str = "server.reach";
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
    pub const CONVERSATION_RENAME: &str = "conversation.rename";
    pub const CONVERSATION_DECISIONS: &str = "conversation.decisions";
    pub const CONVERSATION_DECISION_SETTINGS: &str = "conversation.decisionSettings";
    pub const CONVERSATION_PAUSE_AUTO_PICK: &str = "conversation.pauseAutoPick";
    pub const CONVERSATION_SET_DECISION_SETTINGS: &str = "conversation.setDecisionSettings";
    pub const CONVERSATION_DELETE: &str = "conversation.delete";
    pub const CONVERSATION_REORDER: &str = "conversation.reorder";
    pub const ARTIFACT_READ: &str = "artifact.read";
    pub const ATTACHMENT_BEGIN: &str = "attachment.begin";
    pub const ATTACHMENT_WRITE: &str = "attachment.write";
    pub const AGENTS_LIST: &str = "agents.list";
    pub const AGENTS_SET_NATIVE_HOOKS: &str = "agents.setNativeHooks";
    pub const AGENTS_SUBSCRIBE: &str = "agents.subscribe";
    pub const AGENTS_UNSUBSCRIBE: &str = "agents.unsubscribe";
    pub const CONVERSATION_SET_APPROVAL_MODE: &str = "conversation.setApprovalMode";
    pub const CONVERSATION_SET_USAGE_LIMIT_AUTO_CONTINUE: &str =
        "conversation.setUsageLimitAutoContinue";
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
    pub const APPS_TRUSTED: &str = "apps.trusted";
    pub const APPS_UNTRUST: &str = "apps.untrust";
    pub const APPS_STARTER: &str = "apps.starter";
    pub const APPS_REVIEW: &str = "apps.review";
    pub const APPS_INSTALL: &str = "apps.install";
    pub const APPS_INSTALLED: &str = "apps.installed";
    pub const APPS_READ: &str = "apps.read";
    pub const APPS_UNINSTALL: &str = "apps.uninstall";
    pub const APPS_ISSUE_CREDENTIAL: &str = "apps.issueCredential";
    pub const APPS_REVOKE_CREDENTIALS: &str = "apps.revokeCredentials";
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
    pub const PAIRING_SET_TUNNEL: &str = "pairing.setTunnel";
    pub const PAIRING_TUNNEL: &str = "pairing.tunnel";
    pub const GITHUB_LIVE_EVENTS: &str = "github.liveEvents";
    pub const GITHUB_STATUS: &str = "github.status";
    pub const GITHUB_CONNECT: &str = "github.connect";
    pub const DEVICES_LIST: &str = "devices.list";
    pub const GITHUB_CANCEL: &str = "github.cancel";
    pub const GITHUB_DISCONNECT: &str = "github.disconnect";
    pub const PEER_ENROLL: &str = "peer.enroll";
    pub const PEER_LIST: &str = "peer.list";
    pub const PEER_FORGET: &str = "peer.forget";
    pub const PEER_RELAY: &str = "peer.relay";
    pub const PEER_FOLLOW: &str = "peer.follow";
    pub const GRANTS_SUBSCRIBE: &str = "grants.subscribe";
    pub const GRANTS_PAGE: &str = "grants.page";
    pub const GRANTS_UNSUBSCRIBE: &str = "grants.unsubscribe";
    pub const CONVERSATION_SUBSCRIBE: &str = "conversation.subscribe";
    pub const CONVERSATION_SUBSCRIBE_LIST: &str = "conversation.subscribeList";
    pub const CONVERSATION_UNSUBSCRIBE: &str = "conversation.unsubscribe";
    pub const CONVERSATION_SHARE: &str = "conversation.share";
    pub const CONVERSATION_UNSHARE: &str = "conversation.unshare";
    pub const CONVERSATION_SHARES: &str = "conversation.shares";
    pub const CONVERSATION_PERMISSION_RULES: &str = "conversation.permissionRules";
    pub const CONVERSATION_REVOKE_PERMISSION_RULE: &str = "conversation.revokePermissionRule";
    pub const WORKSPACE_PUBLISH: &str = "workspace.publish";
    pub const WORKSPACE_ANSWER: &str = "workspace.answer";
    pub const WORKSPACE_ARRANGE: &str = "workspace.arrange";
    pub const WORKSPACE_CLOSE_PANE: &str = "workspace.closePane";
    pub const CHANNEL_CREATE: &str = "channel.create";
    pub const CHANNEL_LIST: &str = "channel.list";
    pub const CHANNEL_SET_COORDINATOR: &str = "channel.setCoordinator";
    pub const REPLY_STYLE_READ: &str = "replyStyle.read";
    pub const REPLY_STYLE_SET: &str = "replyStyle.set";
    pub const CONVERSATION_SUBSCRIBE_EVENTS: &str = "conversation.subscribeEvents";
    pub const CONVERSATION_READ_EARLIER: &str = "conversation.readEarlier";
    pub const CONVERSATION_PEEK: &str = "conversation.peek";
    pub const CONVERSATION_SET_MODEL: &str = "conversation.setModel";
    pub const CONVERSATION_SWITCH_AGENT: &str = "conversation.switchAgent";
    pub const CONVERSATION_FORK: &str = "conversation.fork";
    pub const VAULT_STATUS: &str = "vault.status";
    pub const VAULT_SETUP: &str = "vault.setup";
    pub const VAULT_UNLOCK: &str = "vault.unlock";
    pub const VAULT_CHANGE_PASSPHRASE: &str = "vault.changePassphrase";
    pub const VAULT_LOCK: &str = "vault.lock";
    pub const BOAT_STATUS: &str = "boat.status";
    pub const BOAT_SAVE: &str = "boat.save";
    pub const BOAT_DISCONNECT: &str = "boat.disconnect";
    pub const AGENT37_STATUS: &str = "agent37.status";
    pub const AGENT37_SAVE: &str = "agent37.save";
    pub const AGENT37_DISCONNECT: &str = "agent37.disconnect";
    pub const CLOUD_READ: &str = "cloud.read";
    pub const CLOUD_CHOOSE: &str = "cloud.choose";
    pub const CLOUD_BOXES_LIST: &str = "cloud.boxes.list";
    pub const CLOUD_BOXES_CREATE: &str = "cloud.boxes.create";
    pub const CLOUD_BOXES_WAKE: &str = "cloud.boxes.wake";
    pub const CLOUD_BOXES_SLEEP: &str = "cloud.boxes.sleep";
    pub const CLOUD_BOXES_RENAME: &str = "cloud.boxes.rename";
    pub const CLOUD_BOXES_DELETE: &str = "cloud.boxes.delete";
    pub const CONVERSATION_WEBHOOK_CURSOR: &str = "conversation.webhook.cursor";
    pub const CONVERSATION_WEBHOOK_RESET: &str = "conversation.webhook.reset";
    pub const VOICE_STATUS: &str = "voice.status";
    pub const VOICE_SAVE_KEY: &str = "voice.saveKey";
    pub const VOICE_FORGET_KEY: &str = "voice.forgetKey";
    pub const VOICE_START_CALL: &str = "voice.startCall";
    pub const TERMINAL_OPEN: &str = "terminal.open";
    pub const TERMINAL_LIST: &str = "terminal.list";
    pub const TERMINAL_ATTACH: &str = "terminal.attach";
    pub const TERMINAL_FOCUS: &str = "terminal.focus";
    pub const TERMINAL_FIT: &str = "terminal.fit";
    pub const TERMINAL_DETACH: &str = "terminal.detach";
    pub const TERMINAL_INPUT: &str = "terminal.input";
    pub const TERMINAL_HISTORY: &str = "terminal.history";
    pub const TERMINAL_CLOSE: &str = "terminal.close";
    pub const TERMINAL_RENAME: &str = "terminal.rename";
    pub const TERMINAL_RESUME_SETTINGS: &str = "terminal.resumeSettings";
    pub const TERMINAL_SET_RESUME_SETTINGS: &str = "terminal.setResumeSettings";
    pub const TERMINAL_FORGET_SAVED: &str = "terminal.forgetSaved";
}
pub mod product_event {
    pub const SESSION_CHALLENGE: &str = "session.challenge";
    pub const SESSION_TERMINATED: &str = "session.terminated";
    pub const CONVERSATION_VIEW: &str = "conversation.view";
    pub const CONVERSATION_LISTED: &str = "conversation.listed";
    pub const AGENTS_LISTED: &str = "agents.listed";
    pub const CONVERSATION_SUBSCRIPTION_ENDED: &str = "conversation.subscriptionEnded";
    pub const GRANTS_CHANGED: &str = "grants.changed";
    pub const GRANTS_SUBSCRIPTION_ENDED: &str = "grants.subscriptionEnded";
    pub const PERMISSION_REVIEW_WAITING: &str = "permission.reviewWaiting";
    pub const WORKSPACE_COMMAND: &str = "workspace.command";
    pub const CONVERSATION_EVENTS: &str = "conversation.events";
    pub const TERMINAL_OUTPUT: &str = "terminal.output";
    pub const TERMINAL_LISTED: &str = "terminal.listed";
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
                items.len() <= 156
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
    "server.reach",
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
    "conversation.rename",
    "conversation.decisions",
    "conversation.decisionSettings",
    "conversation.pauseAutoPick",
    "conversation.setDecisionSettings",
    "conversation.delete",
    "conversation.reorder",
    "artifact.read",
    "attachment.begin",
    "attachment.write",
    "agents.list",
    "agents.setNativeHooks",
    "agents.subscribe",
    "agents.unsubscribe",
    "conversation.setApprovalMode",
    "conversation.setUsageLimitAutoContinue",
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
    "apps.trusted",
    "apps.untrust",
    "apps.starter",
    "apps.review",
    "apps.install",
    "apps.installed",
    "apps.read",
    "apps.uninstall",
    "apps.issueCredential",
    "apps.revokeCredentials",
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
    "pairing.setTunnel",
    "pairing.tunnel",
    "github.liveEvents",
    "github.status",
    "github.connect",
    "devices.list",
    "github.cancel",
    "github.disconnect",
    "peer.enroll",
    "peer.list",
    "peer.forget",
    "peer.relay",
    "peer.follow",
    "grants.subscribe",
    "grants.page",
    "grants.unsubscribe",
    "conversation.subscribe",
    "conversation.subscribeList",
    "conversation.unsubscribe",
    "conversation.share",
    "conversation.unshare",
    "conversation.shares",
    "conversation.permissionRules",
    "conversation.revokePermissionRule",
    "workspace.publish",
    "workspace.answer",
    "workspace.arrange",
    "workspace.closePane",
    "channel.create",
    "channel.list",
    "channel.setCoordinator",
    "replyStyle.read",
    "replyStyle.set",
    "conversation.subscribeEvents",
    "conversation.readEarlier",
    "conversation.peek",
    "conversation.setModel",
    "conversation.switchAgent",
    "conversation.fork",
    "vault.status",
    "vault.setup",
    "vault.unlock",
    "vault.changePassphrase",
    "vault.lock",
    "boat.status",
    "boat.save",
    "boat.disconnect",
    "agent37.status",
    "agent37.save",
    "agent37.disconnect",
    "cloud.read",
    "cloud.choose",
    "cloud.boxes.list",
    "cloud.boxes.create",
    "cloud.boxes.wake",
    "cloud.boxes.sleep",
    "cloud.boxes.rename",
    "cloud.boxes.delete",
    "conversation.webhook.cursor",
    "conversation.webhook.reset",
    "voice.status",
    "voice.saveKey",
    "voice.forgetKey",
    "voice.startCall",
    "terminal.open",
    "terminal.list",
    "terminal.attach",
    "terminal.focus",
    "terminal.fit",
    "terminal.detach",
    "terminal.input",
    "terminal.history",
    "terminal.close",
    "terminal.rename",
    "terminal.resumeSettings",
    "terminal.setResumeSettings",
    "terminal.forgetSaved",
];
/// The grant Cedar is asked for before this method is dispatched.
///
/// Generated from `protocol/product/manifest.json`. Writing
/// to a conversation, including answering its question, uploading into it, and
/// an app's calls, asks for `conversation.write`. Reading its records or
/// catalogue asks for `conversation.read`. Running a configured server with
/// the gateway's authority, and enrolling a device, asks for
/// `credential.manage`; Auth asks again for the exact consent. `None`
/// means another owner admits the method: the handshake or `auth.session`,
/// or the manifest has no such method.
pub fn action_for_method(method: &str) -> Option<&'static str> {
    ProductMethod::parse(method).and_then(ProductMethod::grant)
}
/// An admission lane: the capacity a method waits in on the gateway.
///
/// Generated from the `lane` of each method in
/// `protocol/product/manifest.json`. Each lane has its own capacity, so a
/// burst of one kind of request never takes another lane's place. How many
/// permits each lane holds is `OperationalLimits`'s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProductLane {
    Requests,
    Controls,
    RecordReads,
    AppCalls,
    Deletions,
    UploadBegins,
    ArtifactReads,
}

impl ProductLane {
    /// Every lane, in manifest order.
    pub const ALL: &'static [ProductLane] = &[
        ProductLane::Requests,
        ProductLane::Controls,
        ProductLane::RecordReads,
        ProductLane::AppCalls,
        ProductLane::Deletions,
        ProductLane::UploadBegins,
        ProductLane::ArtifactReads,
    ];

    /// The lane's manifest spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            ProductLane::Requests => "requests",
            ProductLane::Controls => "controls",
            ProductLane::RecordReads => "recordReads",
            ProductLane::AppCalls => "appCalls",
            ProductLane::Deletions => "deletions",
            ProductLane::UploadBegins => "uploadBegins",
            ProductLane::ArtifactReads => "artifactReads",
        }
    }
}

/// A method `protocol/product/manifest.json` declares.
///
/// Generated: the manifest is the only list of methods, and this enum is
/// how the gateway names one once a frame is read. A method with no route
/// in an exhaustive match does not compile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProductMethod {
    /// `session.authenticate`
    SessionAuthenticate,
    /// `auth.session`
    AuthSession,
    /// `server.health`
    ServerHealth,
    /// `server.reach`
    ServerReach,
    /// `credential.issue`
    CredentialIssue,
    /// `credential.list`
    CredentialList,
    /// `credential.revoke`
    CredentialRevoke,
    /// `conversation.create`
    ConversationCreate,
    /// `conversation.read`
    ConversationRead,
    /// `conversation.recordsHead`
    ConversationRecordsHead,
    /// `conversation.recordsPage`
    ConversationRecordsPage,
    /// `conversation.list`
    ConversationList,
    /// `conversation.observe`
    ConversationObserve,
    /// `conversation.send`
    ConversationSend,
    /// `conversation.steer`
    ConversationSteer,
    /// `conversation.remove`
    ConversationRemove,
    /// `conversation.stop`
    ConversationStop,
    /// `conversation.receipt`
    ConversationReceipt,
    /// `conversation.message`
    ConversationMessage,
    /// `conversation.messageStatus`
    ConversationMessageStatus,
    /// `conversation.answer`
    ConversationAnswer,
    /// `conversation.permissionReview`
    ConversationPermissionReview,
    /// `conversation.questionReview`
    ConversationQuestionReview,
    /// `conversation.answerQuestion`
    ConversationAnswerQuestion,
    /// `conversation.cancel`
    ConversationCancel,
    /// `conversation.close`
    ConversationClose,
    /// `conversation.archive`
    ConversationArchive,
    /// `conversation.unarchive`
    ConversationUnarchive,
    /// `conversation.rename`
    ConversationRename,
    /// `conversation.decisions`
    ConversationDecisions,
    /// `conversation.decisionSettings`
    ConversationDecisionSettings,
    /// `conversation.pauseAutoPick`
    ConversationPauseAutoPick,
    /// `conversation.setDecisionSettings`
    ConversationSetDecisionSettings,
    /// `conversation.delete`
    ConversationDelete,
    /// `conversation.reorder`
    ConversationReorder,
    /// `artifact.read`
    ArtifactRead,
    /// `attachment.begin`
    AttachmentBegin,
    /// `attachment.write`
    AttachmentWrite,
    /// `agents.list`
    AgentsList,
    /// `agents.setNativeHooks`
    AgentsSetNativeHooks,
    /// `agents.subscribe`
    AgentsSubscribe,
    /// `agents.unsubscribe`
    AgentsUnsubscribe,
    /// `conversation.setApprovalMode`
    ConversationSetApprovalMode,
    /// `conversation.setUsageLimitAutoContinue`
    ConversationSetUsageLimitAutoContinue,
    /// `agents.installOptions`
    AgentsInstallOptions,
    /// `agents.install`
    AgentsInstall,
    /// `conversation.catalogueHead`
    ConversationCatalogueHead,
    /// `conversation.catalogueManifest`
    ConversationCatalogueManifest,
    /// `conversation.catalogueResolve`
    ConversationCatalogueResolve,
    /// `mcp.callTool`
    McpCallTool,
    /// `mcp.readResource`
    McpReadResource,
    /// `mcp.releaseApp`
    McpReleaseApp,
    /// `mcp.sendMessage`
    McpSendMessage,
    /// `mcp.updateModelContext`
    McpUpdateModelContext,
    /// `mcpServers.list`
    McpServersList,
    /// `mcpServers.save`
    McpServersSave,
    /// `mcpServers.remove`
    McpServersRemove,
    /// `mcpServers.inspect`
    McpServersInspect,
    /// `mcpServers.authorize`
    McpServersAuthorize,
    /// `mcpServers.revoke`
    McpServersRevoke,
    /// `apps.trusted`
    AppsTrusted,
    /// `apps.untrust`
    AppsUntrust,
    /// `apps.starter`
    AppsStarter,
    /// `apps.review`
    AppsReview,
    /// `apps.install`
    AppsInstall,
    /// `apps.installed`
    AppsInstalled,
    /// `apps.read`
    AppsRead,
    /// `apps.uninstall`
    AppsUninstall,
    /// `apps.issueCredential`
    AppsIssueCredential,
    /// `apps.revokeCredentials`
    AppsRevokeCredentials,
    /// `phone.endpoint`
    PhoneEndpoint,
    /// `phone.authorize`
    PhoneAuthorize,
    /// `phone.status`
    PhoneStatus,
    /// `pairing.create`
    PairingCreate,
    /// `pairing.pending`
    PairingPending,
    /// `pairing.status`
    PairingStatus,
    /// `pairing.approve`
    PairingApprove,
    /// `pairing.deny`
    PairingDeny,
    /// `pairing.cancel`
    PairingCancel,
    /// `pairing.setLinking`
    PairingSetLinking,
    /// `pairing.setTunnel`
    PairingSetTunnel,
    /// `pairing.tunnel`
    PairingTunnel,
    /// `github.liveEvents`
    GithubLiveEvents,
    /// `github.status`
    GithubStatus,
    /// `github.connect`
    GithubConnect,
    /// `devices.list`
    DevicesList,
    /// `github.cancel`
    GithubCancel,
    /// `github.disconnect`
    GithubDisconnect,
    /// `peer.enroll`
    PeerEnroll,
    /// `peer.list`
    PeerList,
    /// `peer.forget`
    PeerForget,
    /// `peer.relay`
    PeerRelay,
    /// `peer.follow`
    PeerFollow,
    /// `grants.subscribe`
    GrantsSubscribe,
    /// `grants.page`
    GrantsPage,
    /// `grants.unsubscribe`
    GrantsUnsubscribe,
    /// `conversation.subscribe`
    ConversationSubscribe,
    /// `conversation.subscribeList`
    ConversationSubscribeList,
    /// `conversation.unsubscribe`
    ConversationUnsubscribe,
    /// `conversation.share`
    ConversationShare,
    /// `conversation.unshare`
    ConversationUnshare,
    /// `conversation.shares`
    ConversationShares,
    /// `conversation.permissionRules`
    ConversationPermissionRules,
    /// `conversation.revokePermissionRule`
    ConversationRevokePermissionRule,
    /// `workspace.publish`
    WorkspacePublish,
    /// `workspace.answer`
    WorkspaceAnswer,
    /// `workspace.arrange`
    WorkspaceArrange,
    /// `workspace.closePane`
    WorkspaceClosePane,
    /// `channel.create`
    ChannelCreate,
    /// `channel.list`
    ChannelList,
    /// `channel.setCoordinator`
    ChannelSetCoordinator,
    /// `replyStyle.read`
    ReplyStyleRead,
    /// `replyStyle.set`
    ReplyStyleSet,
    /// `conversation.subscribeEvents`
    ConversationSubscribeEvents,
    /// `conversation.readEarlier`
    ConversationReadEarlier,
    /// `conversation.peek`
    ConversationPeek,
    /// `conversation.setModel`
    ConversationSetModel,
    /// `conversation.switchAgent`
    ConversationSwitchAgent,
    /// `conversation.fork`
    ConversationFork,
    /// `vault.status`
    VaultStatus,
    /// `vault.setup`
    VaultSetup,
    /// `vault.unlock`
    VaultUnlock,
    /// `vault.changePassphrase`
    VaultChangePassphrase,
    /// `vault.lock`
    VaultLock,
    /// `boat.status`
    BoatStatus,
    /// `boat.save`
    BoatSave,
    /// `boat.disconnect`
    BoatDisconnect,
    /// `agent37.status`
    Agent37Status,
    /// `agent37.save`
    Agent37Save,
    /// `agent37.disconnect`
    Agent37Disconnect,
    /// `cloud.read`
    CloudRead,
    /// `cloud.choose`
    CloudChoose,
    /// `cloud.boxes.list`
    CloudBoxesList,
    /// `cloud.boxes.create`
    CloudBoxesCreate,
    /// `cloud.boxes.wake`
    CloudBoxesWake,
    /// `cloud.boxes.sleep`
    CloudBoxesSleep,
    /// `cloud.boxes.rename`
    CloudBoxesRename,
    /// `cloud.boxes.delete`
    CloudBoxesDelete,
    /// `conversation.webhook.cursor`
    ConversationWebhookCursor,
    /// `conversation.webhook.reset`
    ConversationWebhookReset,
    /// `voice.status`
    VoiceStatus,
    /// `voice.saveKey`
    VoiceSaveKey,
    /// `voice.forgetKey`
    VoiceForgetKey,
    /// `voice.startCall`
    VoiceStartCall,
    /// `terminal.open`
    TerminalOpen,
    /// `terminal.list`
    TerminalList,
    /// `terminal.attach`
    TerminalAttach,
    /// `terminal.focus`
    TerminalFocus,
    /// `terminal.fit`
    TerminalFit,
    /// `terminal.detach`
    TerminalDetach,
    /// `terminal.input`
    TerminalInput,
    /// `terminal.history`
    TerminalHistory,
    /// `terminal.close`
    TerminalClose,
    /// `terminal.rename`
    TerminalRename,
    /// `terminal.resumeSettings`
    TerminalResumeSettings,
    /// `terminal.setResumeSettings`
    TerminalSetResumeSettings,
    /// `terminal.forgetSaved`
    TerminalForgetSaved,
}

impl ProductMethod {
    /// Every method, in manifest order.
    pub const ALL: &'static [ProductMethod] = &[
        ProductMethod::SessionAuthenticate,
        ProductMethod::AuthSession,
        ProductMethod::ServerHealth,
        ProductMethod::ServerReach,
        ProductMethod::CredentialIssue,
        ProductMethod::CredentialList,
        ProductMethod::CredentialRevoke,
        ProductMethod::ConversationCreate,
        ProductMethod::ConversationRead,
        ProductMethod::ConversationRecordsHead,
        ProductMethod::ConversationRecordsPage,
        ProductMethod::ConversationList,
        ProductMethod::ConversationObserve,
        ProductMethod::ConversationSend,
        ProductMethod::ConversationSteer,
        ProductMethod::ConversationRemove,
        ProductMethod::ConversationStop,
        ProductMethod::ConversationReceipt,
        ProductMethod::ConversationMessage,
        ProductMethod::ConversationMessageStatus,
        ProductMethod::ConversationAnswer,
        ProductMethod::ConversationPermissionReview,
        ProductMethod::ConversationQuestionReview,
        ProductMethod::ConversationAnswerQuestion,
        ProductMethod::ConversationCancel,
        ProductMethod::ConversationClose,
        ProductMethod::ConversationArchive,
        ProductMethod::ConversationUnarchive,
        ProductMethod::ConversationRename,
        ProductMethod::ConversationDecisions,
        ProductMethod::ConversationDecisionSettings,
        ProductMethod::ConversationPauseAutoPick,
        ProductMethod::ConversationSetDecisionSettings,
        ProductMethod::ConversationDelete,
        ProductMethod::ConversationReorder,
        ProductMethod::ArtifactRead,
        ProductMethod::AttachmentBegin,
        ProductMethod::AttachmentWrite,
        ProductMethod::AgentsList,
        ProductMethod::AgentsSetNativeHooks,
        ProductMethod::AgentsSubscribe,
        ProductMethod::AgentsUnsubscribe,
        ProductMethod::ConversationSetApprovalMode,
        ProductMethod::ConversationSetUsageLimitAutoContinue,
        ProductMethod::AgentsInstallOptions,
        ProductMethod::AgentsInstall,
        ProductMethod::ConversationCatalogueHead,
        ProductMethod::ConversationCatalogueManifest,
        ProductMethod::ConversationCatalogueResolve,
        ProductMethod::McpCallTool,
        ProductMethod::McpReadResource,
        ProductMethod::McpReleaseApp,
        ProductMethod::McpSendMessage,
        ProductMethod::McpUpdateModelContext,
        ProductMethod::McpServersList,
        ProductMethod::McpServersSave,
        ProductMethod::McpServersRemove,
        ProductMethod::McpServersInspect,
        ProductMethod::McpServersAuthorize,
        ProductMethod::McpServersRevoke,
        ProductMethod::AppsTrusted,
        ProductMethod::AppsUntrust,
        ProductMethod::AppsStarter,
        ProductMethod::AppsReview,
        ProductMethod::AppsInstall,
        ProductMethod::AppsInstalled,
        ProductMethod::AppsRead,
        ProductMethod::AppsUninstall,
        ProductMethod::AppsIssueCredential,
        ProductMethod::AppsRevokeCredentials,
        ProductMethod::PhoneEndpoint,
        ProductMethod::PhoneAuthorize,
        ProductMethod::PhoneStatus,
        ProductMethod::PairingCreate,
        ProductMethod::PairingPending,
        ProductMethod::PairingStatus,
        ProductMethod::PairingApprove,
        ProductMethod::PairingDeny,
        ProductMethod::PairingCancel,
        ProductMethod::PairingSetLinking,
        ProductMethod::PairingSetTunnel,
        ProductMethod::PairingTunnel,
        ProductMethod::GithubLiveEvents,
        ProductMethod::GithubStatus,
        ProductMethod::GithubConnect,
        ProductMethod::DevicesList,
        ProductMethod::GithubCancel,
        ProductMethod::GithubDisconnect,
        ProductMethod::PeerEnroll,
        ProductMethod::PeerList,
        ProductMethod::PeerForget,
        ProductMethod::PeerRelay,
        ProductMethod::PeerFollow,
        ProductMethod::GrantsSubscribe,
        ProductMethod::GrantsPage,
        ProductMethod::GrantsUnsubscribe,
        ProductMethod::ConversationSubscribe,
        ProductMethod::ConversationSubscribeList,
        ProductMethod::ConversationUnsubscribe,
        ProductMethod::ConversationShare,
        ProductMethod::ConversationUnshare,
        ProductMethod::ConversationShares,
        ProductMethod::ConversationPermissionRules,
        ProductMethod::ConversationRevokePermissionRule,
        ProductMethod::WorkspacePublish,
        ProductMethod::WorkspaceAnswer,
        ProductMethod::WorkspaceArrange,
        ProductMethod::WorkspaceClosePane,
        ProductMethod::ChannelCreate,
        ProductMethod::ChannelList,
        ProductMethod::ChannelSetCoordinator,
        ProductMethod::ReplyStyleRead,
        ProductMethod::ReplyStyleSet,
        ProductMethod::ConversationSubscribeEvents,
        ProductMethod::ConversationReadEarlier,
        ProductMethod::ConversationPeek,
        ProductMethod::ConversationSetModel,
        ProductMethod::ConversationSwitchAgent,
        ProductMethod::ConversationFork,
        ProductMethod::VaultStatus,
        ProductMethod::VaultSetup,
        ProductMethod::VaultUnlock,
        ProductMethod::VaultChangePassphrase,
        ProductMethod::VaultLock,
        ProductMethod::BoatStatus,
        ProductMethod::BoatSave,
        ProductMethod::BoatDisconnect,
        ProductMethod::Agent37Status,
        ProductMethod::Agent37Save,
        ProductMethod::Agent37Disconnect,
        ProductMethod::CloudRead,
        ProductMethod::CloudChoose,
        ProductMethod::CloudBoxesList,
        ProductMethod::CloudBoxesCreate,
        ProductMethod::CloudBoxesWake,
        ProductMethod::CloudBoxesSleep,
        ProductMethod::CloudBoxesRename,
        ProductMethod::CloudBoxesDelete,
        ProductMethod::ConversationWebhookCursor,
        ProductMethod::ConversationWebhookReset,
        ProductMethod::VoiceStatus,
        ProductMethod::VoiceSaveKey,
        ProductMethod::VoiceForgetKey,
        ProductMethod::VoiceStartCall,
        ProductMethod::TerminalOpen,
        ProductMethod::TerminalList,
        ProductMethod::TerminalAttach,
        ProductMethod::TerminalFocus,
        ProductMethod::TerminalFit,
        ProductMethod::TerminalDetach,
        ProductMethod::TerminalInput,
        ProductMethod::TerminalHistory,
        ProductMethod::TerminalClose,
        ProductMethod::TerminalRename,
        ProductMethod::TerminalResumeSettings,
        ProductMethod::TerminalSetResumeSettings,
        ProductMethod::TerminalForgetSaved,
    ];

    /// The method a frame names, or `None` when the manifest has none by
    /// that name.
    pub fn parse(method: &str) -> Option<Self> {
        match method {
            "session.authenticate" => Some(ProductMethod::SessionAuthenticate),
            "auth.session" => Some(ProductMethod::AuthSession),
            "server.health" => Some(ProductMethod::ServerHealth),
            "server.reach" => Some(ProductMethod::ServerReach),
            "credential.issue" => Some(ProductMethod::CredentialIssue),
            "credential.list" => Some(ProductMethod::CredentialList),
            "credential.revoke" => Some(ProductMethod::CredentialRevoke),
            "conversation.create" => Some(ProductMethod::ConversationCreate),
            "conversation.read" => Some(ProductMethod::ConversationRead),
            "conversation.recordsHead" => Some(ProductMethod::ConversationRecordsHead),
            "conversation.recordsPage" => Some(ProductMethod::ConversationRecordsPage),
            "conversation.list" => Some(ProductMethod::ConversationList),
            "conversation.observe" => Some(ProductMethod::ConversationObserve),
            "conversation.send" => Some(ProductMethod::ConversationSend),
            "conversation.steer" => Some(ProductMethod::ConversationSteer),
            "conversation.remove" => Some(ProductMethod::ConversationRemove),
            "conversation.stop" => Some(ProductMethod::ConversationStop),
            "conversation.receipt" => Some(ProductMethod::ConversationReceipt),
            "conversation.message" => Some(ProductMethod::ConversationMessage),
            "conversation.messageStatus" => Some(ProductMethod::ConversationMessageStatus),
            "conversation.answer" => Some(ProductMethod::ConversationAnswer),
            "conversation.permissionReview" => Some(ProductMethod::ConversationPermissionReview),
            "conversation.questionReview" => Some(ProductMethod::ConversationQuestionReview),
            "conversation.answerQuestion" => Some(ProductMethod::ConversationAnswerQuestion),
            "conversation.cancel" => Some(ProductMethod::ConversationCancel),
            "conversation.close" => Some(ProductMethod::ConversationClose),
            "conversation.archive" => Some(ProductMethod::ConversationArchive),
            "conversation.unarchive" => Some(ProductMethod::ConversationUnarchive),
            "conversation.rename" => Some(ProductMethod::ConversationRename),
            "conversation.decisions" => Some(ProductMethod::ConversationDecisions),
            "conversation.decisionSettings" => Some(ProductMethod::ConversationDecisionSettings),
            "conversation.pauseAutoPick" => Some(ProductMethod::ConversationPauseAutoPick),
            "conversation.setDecisionSettings" => {
                Some(ProductMethod::ConversationSetDecisionSettings)
            }
            "conversation.delete" => Some(ProductMethod::ConversationDelete),
            "conversation.reorder" => Some(ProductMethod::ConversationReorder),
            "artifact.read" => Some(ProductMethod::ArtifactRead),
            "attachment.begin" => Some(ProductMethod::AttachmentBegin),
            "attachment.write" => Some(ProductMethod::AttachmentWrite),
            "agents.list" => Some(ProductMethod::AgentsList),
            "agents.setNativeHooks" => Some(ProductMethod::AgentsSetNativeHooks),
            "agents.subscribe" => Some(ProductMethod::AgentsSubscribe),
            "agents.unsubscribe" => Some(ProductMethod::AgentsUnsubscribe),
            "conversation.setApprovalMode" => Some(ProductMethod::ConversationSetApprovalMode),
            "conversation.setUsageLimitAutoContinue" => {
                Some(ProductMethod::ConversationSetUsageLimitAutoContinue)
            }
            "agents.installOptions" => Some(ProductMethod::AgentsInstallOptions),
            "agents.install" => Some(ProductMethod::AgentsInstall),
            "conversation.catalogueHead" => Some(ProductMethod::ConversationCatalogueHead),
            "conversation.catalogueManifest" => Some(ProductMethod::ConversationCatalogueManifest),
            "conversation.catalogueResolve" => Some(ProductMethod::ConversationCatalogueResolve),
            "mcp.callTool" => Some(ProductMethod::McpCallTool),
            "mcp.readResource" => Some(ProductMethod::McpReadResource),
            "mcp.releaseApp" => Some(ProductMethod::McpReleaseApp),
            "mcp.sendMessage" => Some(ProductMethod::McpSendMessage),
            "mcp.updateModelContext" => Some(ProductMethod::McpUpdateModelContext),
            "mcpServers.list" => Some(ProductMethod::McpServersList),
            "mcpServers.save" => Some(ProductMethod::McpServersSave),
            "mcpServers.remove" => Some(ProductMethod::McpServersRemove),
            "mcpServers.inspect" => Some(ProductMethod::McpServersInspect),
            "mcpServers.authorize" => Some(ProductMethod::McpServersAuthorize),
            "mcpServers.revoke" => Some(ProductMethod::McpServersRevoke),
            "apps.trusted" => Some(ProductMethod::AppsTrusted),
            "apps.untrust" => Some(ProductMethod::AppsUntrust),
            "apps.starter" => Some(ProductMethod::AppsStarter),
            "apps.review" => Some(ProductMethod::AppsReview),
            "apps.install" => Some(ProductMethod::AppsInstall),
            "apps.installed" => Some(ProductMethod::AppsInstalled),
            "apps.read" => Some(ProductMethod::AppsRead),
            "apps.uninstall" => Some(ProductMethod::AppsUninstall),
            "apps.issueCredential" => Some(ProductMethod::AppsIssueCredential),
            "apps.revokeCredentials" => Some(ProductMethod::AppsRevokeCredentials),
            "phone.endpoint" => Some(ProductMethod::PhoneEndpoint),
            "phone.authorize" => Some(ProductMethod::PhoneAuthorize),
            "phone.status" => Some(ProductMethod::PhoneStatus),
            "pairing.create" => Some(ProductMethod::PairingCreate),
            "pairing.pending" => Some(ProductMethod::PairingPending),
            "pairing.status" => Some(ProductMethod::PairingStatus),
            "pairing.approve" => Some(ProductMethod::PairingApprove),
            "pairing.deny" => Some(ProductMethod::PairingDeny),
            "pairing.cancel" => Some(ProductMethod::PairingCancel),
            "pairing.setLinking" => Some(ProductMethod::PairingSetLinking),
            "pairing.setTunnel" => Some(ProductMethod::PairingSetTunnel),
            "pairing.tunnel" => Some(ProductMethod::PairingTunnel),
            "github.liveEvents" => Some(ProductMethod::GithubLiveEvents),
            "github.status" => Some(ProductMethod::GithubStatus),
            "github.connect" => Some(ProductMethod::GithubConnect),
            "devices.list" => Some(ProductMethod::DevicesList),
            "github.cancel" => Some(ProductMethod::GithubCancel),
            "github.disconnect" => Some(ProductMethod::GithubDisconnect),
            "peer.enroll" => Some(ProductMethod::PeerEnroll),
            "peer.list" => Some(ProductMethod::PeerList),
            "peer.forget" => Some(ProductMethod::PeerForget),
            "peer.relay" => Some(ProductMethod::PeerRelay),
            "peer.follow" => Some(ProductMethod::PeerFollow),
            "grants.subscribe" => Some(ProductMethod::GrantsSubscribe),
            "grants.page" => Some(ProductMethod::GrantsPage),
            "grants.unsubscribe" => Some(ProductMethod::GrantsUnsubscribe),
            "conversation.subscribe" => Some(ProductMethod::ConversationSubscribe),
            "conversation.subscribeList" => Some(ProductMethod::ConversationSubscribeList),
            "conversation.unsubscribe" => Some(ProductMethod::ConversationUnsubscribe),
            "conversation.share" => Some(ProductMethod::ConversationShare),
            "conversation.unshare" => Some(ProductMethod::ConversationUnshare),
            "conversation.shares" => Some(ProductMethod::ConversationShares),
            "conversation.permissionRules" => Some(ProductMethod::ConversationPermissionRules),
            "conversation.revokePermissionRule" => {
                Some(ProductMethod::ConversationRevokePermissionRule)
            }
            "workspace.publish" => Some(ProductMethod::WorkspacePublish),
            "workspace.answer" => Some(ProductMethod::WorkspaceAnswer),
            "workspace.arrange" => Some(ProductMethod::WorkspaceArrange),
            "workspace.closePane" => Some(ProductMethod::WorkspaceClosePane),
            "channel.create" => Some(ProductMethod::ChannelCreate),
            "channel.list" => Some(ProductMethod::ChannelList),
            "channel.setCoordinator" => Some(ProductMethod::ChannelSetCoordinator),
            "replyStyle.read" => Some(ProductMethod::ReplyStyleRead),
            "replyStyle.set" => Some(ProductMethod::ReplyStyleSet),
            "conversation.subscribeEvents" => Some(ProductMethod::ConversationSubscribeEvents),
            "conversation.readEarlier" => Some(ProductMethod::ConversationReadEarlier),
            "conversation.peek" => Some(ProductMethod::ConversationPeek),
            "conversation.setModel" => Some(ProductMethod::ConversationSetModel),
            "conversation.switchAgent" => Some(ProductMethod::ConversationSwitchAgent),
            "conversation.fork" => Some(ProductMethod::ConversationFork),
            "vault.status" => Some(ProductMethod::VaultStatus),
            "vault.setup" => Some(ProductMethod::VaultSetup),
            "vault.unlock" => Some(ProductMethod::VaultUnlock),
            "vault.changePassphrase" => Some(ProductMethod::VaultChangePassphrase),
            "vault.lock" => Some(ProductMethod::VaultLock),
            "boat.status" => Some(ProductMethod::BoatStatus),
            "boat.save" => Some(ProductMethod::BoatSave),
            "boat.disconnect" => Some(ProductMethod::BoatDisconnect),
            "agent37.status" => Some(ProductMethod::Agent37Status),
            "agent37.save" => Some(ProductMethod::Agent37Save),
            "agent37.disconnect" => Some(ProductMethod::Agent37Disconnect),
            "cloud.read" => Some(ProductMethod::CloudRead),
            "cloud.choose" => Some(ProductMethod::CloudChoose),
            "cloud.boxes.list" => Some(ProductMethod::CloudBoxesList),
            "cloud.boxes.create" => Some(ProductMethod::CloudBoxesCreate),
            "cloud.boxes.wake" => Some(ProductMethod::CloudBoxesWake),
            "cloud.boxes.sleep" => Some(ProductMethod::CloudBoxesSleep),
            "cloud.boxes.rename" => Some(ProductMethod::CloudBoxesRename),
            "cloud.boxes.delete" => Some(ProductMethod::CloudBoxesDelete),
            "conversation.webhook.cursor" => Some(ProductMethod::ConversationWebhookCursor),
            "conversation.webhook.reset" => Some(ProductMethod::ConversationWebhookReset),
            "voice.status" => Some(ProductMethod::VoiceStatus),
            "voice.saveKey" => Some(ProductMethod::VoiceSaveKey),
            "voice.forgetKey" => Some(ProductMethod::VoiceForgetKey),
            "voice.startCall" => Some(ProductMethod::VoiceStartCall),
            "terminal.open" => Some(ProductMethod::TerminalOpen),
            "terminal.list" => Some(ProductMethod::TerminalList),
            "terminal.attach" => Some(ProductMethod::TerminalAttach),
            "terminal.focus" => Some(ProductMethod::TerminalFocus),
            "terminal.fit" => Some(ProductMethod::TerminalFit),
            "terminal.detach" => Some(ProductMethod::TerminalDetach),
            "terminal.input" => Some(ProductMethod::TerminalInput),
            "terminal.history" => Some(ProductMethod::TerminalHistory),
            "terminal.close" => Some(ProductMethod::TerminalClose),
            "terminal.rename" => Some(ProductMethod::TerminalRename),
            "terminal.resumeSettings" => Some(ProductMethod::TerminalResumeSettings),
            "terminal.setResumeSettings" => Some(ProductMethod::TerminalSetResumeSettings),
            "terminal.forgetSaved" => Some(ProductMethod::TerminalForgetSaved),
            _ => None,
        }
    }

    /// The method's wire name.
    pub const fn as_str(self) -> &'static str {
        match self {
            ProductMethod::SessionAuthenticate => "session.authenticate",
            ProductMethod::AuthSession => "auth.session",
            ProductMethod::ServerHealth => "server.health",
            ProductMethod::ServerReach => "server.reach",
            ProductMethod::CredentialIssue => "credential.issue",
            ProductMethod::CredentialList => "credential.list",
            ProductMethod::CredentialRevoke => "credential.revoke",
            ProductMethod::ConversationCreate => "conversation.create",
            ProductMethod::ConversationRead => "conversation.read",
            ProductMethod::ConversationRecordsHead => "conversation.recordsHead",
            ProductMethod::ConversationRecordsPage => "conversation.recordsPage",
            ProductMethod::ConversationList => "conversation.list",
            ProductMethod::ConversationObserve => "conversation.observe",
            ProductMethod::ConversationSend => "conversation.send",
            ProductMethod::ConversationSteer => "conversation.steer",
            ProductMethod::ConversationRemove => "conversation.remove",
            ProductMethod::ConversationStop => "conversation.stop",
            ProductMethod::ConversationReceipt => "conversation.receipt",
            ProductMethod::ConversationMessage => "conversation.message",
            ProductMethod::ConversationMessageStatus => "conversation.messageStatus",
            ProductMethod::ConversationAnswer => "conversation.answer",
            ProductMethod::ConversationPermissionReview => "conversation.permissionReview",
            ProductMethod::ConversationQuestionReview => "conversation.questionReview",
            ProductMethod::ConversationAnswerQuestion => "conversation.answerQuestion",
            ProductMethod::ConversationCancel => "conversation.cancel",
            ProductMethod::ConversationClose => "conversation.close",
            ProductMethod::ConversationArchive => "conversation.archive",
            ProductMethod::ConversationUnarchive => "conversation.unarchive",
            ProductMethod::ConversationRename => "conversation.rename",
            ProductMethod::ConversationDecisions => "conversation.decisions",
            ProductMethod::ConversationDecisionSettings => "conversation.decisionSettings",
            ProductMethod::ConversationPauseAutoPick => "conversation.pauseAutoPick",
            ProductMethod::ConversationSetDecisionSettings => "conversation.setDecisionSettings",
            ProductMethod::ConversationDelete => "conversation.delete",
            ProductMethod::ConversationReorder => "conversation.reorder",
            ProductMethod::ArtifactRead => "artifact.read",
            ProductMethod::AttachmentBegin => "attachment.begin",
            ProductMethod::AttachmentWrite => "attachment.write",
            ProductMethod::AgentsList => "agents.list",
            ProductMethod::AgentsSetNativeHooks => "agents.setNativeHooks",
            ProductMethod::AgentsSubscribe => "agents.subscribe",
            ProductMethod::AgentsUnsubscribe => "agents.unsubscribe",
            ProductMethod::ConversationSetApprovalMode => "conversation.setApprovalMode",
            ProductMethod::ConversationSetUsageLimitAutoContinue => {
                "conversation.setUsageLimitAutoContinue"
            }
            ProductMethod::AgentsInstallOptions => "agents.installOptions",
            ProductMethod::AgentsInstall => "agents.install",
            ProductMethod::ConversationCatalogueHead => "conversation.catalogueHead",
            ProductMethod::ConversationCatalogueManifest => "conversation.catalogueManifest",
            ProductMethod::ConversationCatalogueResolve => "conversation.catalogueResolve",
            ProductMethod::McpCallTool => "mcp.callTool",
            ProductMethod::McpReadResource => "mcp.readResource",
            ProductMethod::McpReleaseApp => "mcp.releaseApp",
            ProductMethod::McpSendMessage => "mcp.sendMessage",
            ProductMethod::McpUpdateModelContext => "mcp.updateModelContext",
            ProductMethod::McpServersList => "mcpServers.list",
            ProductMethod::McpServersSave => "mcpServers.save",
            ProductMethod::McpServersRemove => "mcpServers.remove",
            ProductMethod::McpServersInspect => "mcpServers.inspect",
            ProductMethod::McpServersAuthorize => "mcpServers.authorize",
            ProductMethod::McpServersRevoke => "mcpServers.revoke",
            ProductMethod::AppsTrusted => "apps.trusted",
            ProductMethod::AppsUntrust => "apps.untrust",
            ProductMethod::AppsStarter => "apps.starter",
            ProductMethod::AppsReview => "apps.review",
            ProductMethod::AppsInstall => "apps.install",
            ProductMethod::AppsInstalled => "apps.installed",
            ProductMethod::AppsRead => "apps.read",
            ProductMethod::AppsUninstall => "apps.uninstall",
            ProductMethod::AppsIssueCredential => "apps.issueCredential",
            ProductMethod::AppsRevokeCredentials => "apps.revokeCredentials",
            ProductMethod::PhoneEndpoint => "phone.endpoint",
            ProductMethod::PhoneAuthorize => "phone.authorize",
            ProductMethod::PhoneStatus => "phone.status",
            ProductMethod::PairingCreate => "pairing.create",
            ProductMethod::PairingPending => "pairing.pending",
            ProductMethod::PairingStatus => "pairing.status",
            ProductMethod::PairingApprove => "pairing.approve",
            ProductMethod::PairingDeny => "pairing.deny",
            ProductMethod::PairingCancel => "pairing.cancel",
            ProductMethod::PairingSetLinking => "pairing.setLinking",
            ProductMethod::PairingSetTunnel => "pairing.setTunnel",
            ProductMethod::PairingTunnel => "pairing.tunnel",
            ProductMethod::GithubLiveEvents => "github.liveEvents",
            ProductMethod::GithubStatus => "github.status",
            ProductMethod::GithubConnect => "github.connect",
            ProductMethod::DevicesList => "devices.list",
            ProductMethod::GithubCancel => "github.cancel",
            ProductMethod::GithubDisconnect => "github.disconnect",
            ProductMethod::PeerEnroll => "peer.enroll",
            ProductMethod::PeerList => "peer.list",
            ProductMethod::PeerForget => "peer.forget",
            ProductMethod::PeerRelay => "peer.relay",
            ProductMethod::PeerFollow => "peer.follow",
            ProductMethod::GrantsSubscribe => "grants.subscribe",
            ProductMethod::GrantsPage => "grants.page",
            ProductMethod::GrantsUnsubscribe => "grants.unsubscribe",
            ProductMethod::ConversationSubscribe => "conversation.subscribe",
            ProductMethod::ConversationSubscribeList => "conversation.subscribeList",
            ProductMethod::ConversationUnsubscribe => "conversation.unsubscribe",
            ProductMethod::ConversationShare => "conversation.share",
            ProductMethod::ConversationUnshare => "conversation.unshare",
            ProductMethod::ConversationShares => "conversation.shares",
            ProductMethod::ConversationPermissionRules => "conversation.permissionRules",
            ProductMethod::ConversationRevokePermissionRule => "conversation.revokePermissionRule",
            ProductMethod::WorkspacePublish => "workspace.publish",
            ProductMethod::WorkspaceAnswer => "workspace.answer",
            ProductMethod::WorkspaceArrange => "workspace.arrange",
            ProductMethod::WorkspaceClosePane => "workspace.closePane",
            ProductMethod::ChannelCreate => "channel.create",
            ProductMethod::ChannelList => "channel.list",
            ProductMethod::ChannelSetCoordinator => "channel.setCoordinator",
            ProductMethod::ReplyStyleRead => "replyStyle.read",
            ProductMethod::ReplyStyleSet => "replyStyle.set",
            ProductMethod::ConversationSubscribeEvents => "conversation.subscribeEvents",
            ProductMethod::ConversationReadEarlier => "conversation.readEarlier",
            ProductMethod::ConversationPeek => "conversation.peek",
            ProductMethod::ConversationSetModel => "conversation.setModel",
            ProductMethod::ConversationSwitchAgent => "conversation.switchAgent",
            ProductMethod::ConversationFork => "conversation.fork",
            ProductMethod::VaultStatus => "vault.status",
            ProductMethod::VaultSetup => "vault.setup",
            ProductMethod::VaultUnlock => "vault.unlock",
            ProductMethod::VaultChangePassphrase => "vault.changePassphrase",
            ProductMethod::VaultLock => "vault.lock",
            ProductMethod::BoatStatus => "boat.status",
            ProductMethod::BoatSave => "boat.save",
            ProductMethod::BoatDisconnect => "boat.disconnect",
            ProductMethod::Agent37Status => "agent37.status",
            ProductMethod::Agent37Save => "agent37.save",
            ProductMethod::Agent37Disconnect => "agent37.disconnect",
            ProductMethod::CloudRead => "cloud.read",
            ProductMethod::CloudChoose => "cloud.choose",
            ProductMethod::CloudBoxesList => "cloud.boxes.list",
            ProductMethod::CloudBoxesCreate => "cloud.boxes.create",
            ProductMethod::CloudBoxesWake => "cloud.boxes.wake",
            ProductMethod::CloudBoxesSleep => "cloud.boxes.sleep",
            ProductMethod::CloudBoxesRename => "cloud.boxes.rename",
            ProductMethod::CloudBoxesDelete => "cloud.boxes.delete",
            ProductMethod::ConversationWebhookCursor => "conversation.webhook.cursor",
            ProductMethod::ConversationWebhookReset => "conversation.webhook.reset",
            ProductMethod::VoiceStatus => "voice.status",
            ProductMethod::VoiceSaveKey => "voice.saveKey",
            ProductMethod::VoiceForgetKey => "voice.forgetKey",
            ProductMethod::VoiceStartCall => "voice.startCall",
            ProductMethod::TerminalOpen => "terminal.open",
            ProductMethod::TerminalList => "terminal.list",
            ProductMethod::TerminalAttach => "terminal.attach",
            ProductMethod::TerminalFocus => "terminal.focus",
            ProductMethod::TerminalFit => "terminal.fit",
            ProductMethod::TerminalDetach => "terminal.detach",
            ProductMethod::TerminalInput => "terminal.input",
            ProductMethod::TerminalHistory => "terminal.history",
            ProductMethod::TerminalClose => "terminal.close",
            ProductMethod::TerminalRename => "terminal.rename",
            ProductMethod::TerminalResumeSettings => "terminal.resumeSettings",
            ProductMethod::TerminalSetResumeSettings => "terminal.setResumeSettings",
            ProductMethod::TerminalForgetSaved => "terminal.forgetSaved",
        }
    }

    /// The grant Cedar is asked for; `None` when another owner admits the
    /// method (see [`action_for_method`]).
    pub const fn grant(self) -> Option<&'static str> {
        match self {
            ProductMethod::SessionAuthenticate => None,
            ProductMethod::AuthSession => None,
            ProductMethod::ServerHealth => Some("server.read"),
            ProductMethod::ServerReach => Some("server.read"),
            ProductMethod::CredentialIssue => Some("credential.manage"),
            ProductMethod::CredentialList => Some("credential.manage"),
            ProductMethod::CredentialRevoke => Some("credential.manage"),
            ProductMethod::ConversationCreate => Some("conversation.write"),
            ProductMethod::ConversationRead => Some("conversation.write"),
            ProductMethod::ConversationRecordsHead => Some("conversation.read"),
            ProductMethod::ConversationRecordsPage => Some("conversation.read"),
            ProductMethod::ConversationList => Some("conversation.write"),
            ProductMethod::ConversationObserve => Some("conversation.write"),
            ProductMethod::ConversationSend => Some("conversation.write"),
            ProductMethod::ConversationSteer => Some("conversation.write"),
            ProductMethod::ConversationRemove => Some("conversation.write"),
            ProductMethod::ConversationStop => Some("conversation.write"),
            ProductMethod::ConversationReceipt => Some("conversation.write"),
            ProductMethod::ConversationMessage => Some("conversation.write"),
            ProductMethod::ConversationMessageStatus => Some("conversation.write"),
            ProductMethod::ConversationAnswer => Some("conversation.write"),
            ProductMethod::ConversationPermissionReview => Some("conversation.write"),
            ProductMethod::ConversationQuestionReview => Some("conversation.write"),
            ProductMethod::ConversationAnswerQuestion => Some("conversation.write"),
            ProductMethod::ConversationCancel => Some("conversation.write"),
            ProductMethod::ConversationClose => Some("conversation.write"),
            ProductMethod::ConversationArchive => Some("conversation.write"),
            ProductMethod::ConversationUnarchive => Some("conversation.write"),
            ProductMethod::ConversationRename => Some("conversation.write"),
            ProductMethod::ConversationDecisions => Some("conversation.write"),
            ProductMethod::ConversationDecisionSettings => Some("conversation.write"),
            ProductMethod::ConversationPauseAutoPick => Some("conversation.write"),
            ProductMethod::ConversationSetDecisionSettings => Some("conversation.write"),
            ProductMethod::ConversationDelete => Some("conversation.write"),
            ProductMethod::ConversationReorder => Some("conversation.write"),
            ProductMethod::ArtifactRead => Some("conversation.read"),
            ProductMethod::AttachmentBegin => Some("conversation.write"),
            ProductMethod::AttachmentWrite => Some("conversation.write"),
            ProductMethod::AgentsList => Some("server.read"),
            ProductMethod::AgentsSetNativeHooks => Some("credential.manage"),
            ProductMethod::AgentsSubscribe => Some("server.read"),
            ProductMethod::AgentsUnsubscribe => Some("server.read"),
            ProductMethod::ConversationSetApprovalMode => Some("conversation.write"),
            ProductMethod::ConversationSetUsageLimitAutoContinue => Some("credential.manage"),
            ProductMethod::AgentsInstallOptions => Some("server.read"),
            ProductMethod::AgentsInstall => Some("conversation.write"),
            ProductMethod::ConversationCatalogueHead => Some("conversation.read"),
            ProductMethod::ConversationCatalogueManifest => Some("conversation.read"),
            ProductMethod::ConversationCatalogueResolve => Some("conversation.read"),
            ProductMethod::McpCallTool => Some("conversation.write"),
            ProductMethod::McpReadResource => Some("conversation.write"),
            ProductMethod::McpReleaseApp => Some("conversation.write"),
            ProductMethod::McpSendMessage => Some("conversation.write"),
            ProductMethod::McpUpdateModelContext => Some("conversation.write"),
            ProductMethod::McpServersList => Some("credential.manage"),
            ProductMethod::McpServersSave => Some("credential.manage"),
            ProductMethod::McpServersRemove => Some("credential.manage"),
            ProductMethod::McpServersInspect => Some("credential.manage"),
            ProductMethod::McpServersAuthorize => Some("credential.manage"),
            ProductMethod::McpServersRevoke => Some("credential.manage"),
            ProductMethod::AppsTrusted => Some("credential.manage"),
            ProductMethod::AppsUntrust => Some("credential.manage"),
            ProductMethod::AppsStarter => Some("credential.manage"),
            ProductMethod::AppsReview => Some("credential.manage"),
            ProductMethod::AppsInstall => Some("credential.manage"),
            ProductMethod::AppsInstalled => Some("credential.manage"),
            ProductMethod::AppsRead => Some("credential.manage"),
            ProductMethod::AppsUninstall => Some("credential.manage"),
            ProductMethod::AppsIssueCredential => Some("credential.manage"),
            ProductMethod::AppsRevokeCredentials => Some("credential.manage"),
            ProductMethod::PhoneEndpoint => Some("server.read"),
            ProductMethod::PhoneAuthorize => Some("credential.manage"),
            ProductMethod::PhoneStatus => Some("credential.manage"),
            ProductMethod::PairingCreate => Some("credential.manage"),
            ProductMethod::PairingPending => Some("credential.manage"),
            ProductMethod::PairingStatus => Some("credential.manage"),
            ProductMethod::PairingApprove => Some("credential.manage"),
            ProductMethod::PairingDeny => Some("credential.manage"),
            ProductMethod::PairingCancel => Some("credential.manage"),
            ProductMethod::PairingSetLinking => Some("credential.manage"),
            ProductMethod::PairingSetTunnel => Some("credential.manage"),
            ProductMethod::PairingTunnel => Some("credential.manage"),
            ProductMethod::GithubLiveEvents => Some("credential.manage"),
            ProductMethod::GithubStatus => Some("credential.manage"),
            ProductMethod::GithubConnect => Some("credential.manage"),
            ProductMethod::DevicesList => Some("server.read"),
            ProductMethod::GithubCancel => Some("credential.manage"),
            ProductMethod::GithubDisconnect => Some("credential.manage"),
            ProductMethod::PeerEnroll => Some("credential.manage"),
            ProductMethod::PeerList => Some("credential.manage"),
            ProductMethod::PeerForget => Some("credential.manage"),
            ProductMethod::PeerRelay => Some("credential.manage"),
            ProductMethod::PeerFollow => Some("credential.manage"),
            ProductMethod::GrantsSubscribe => Some("conversation.write"),
            ProductMethod::GrantsPage => Some("conversation.write"),
            ProductMethod::GrantsUnsubscribe => Some("conversation.write"),
            ProductMethod::ConversationSubscribe => Some("conversation.write"),
            ProductMethod::ConversationSubscribeList => Some("conversation.write"),
            ProductMethod::ConversationUnsubscribe => Some("conversation.write"),
            ProductMethod::ConversationShare => Some("credential.manage"),
            ProductMethod::ConversationUnshare => Some("credential.manage"),
            ProductMethod::ConversationShares => Some("credential.manage"),
            ProductMethod::ConversationPermissionRules => Some("conversation.write"),
            ProductMethod::ConversationRevokePermissionRule => Some("conversation.write"),
            ProductMethod::WorkspacePublish => Some("conversation.write"),
            ProductMethod::WorkspaceAnswer => Some("conversation.write"),
            ProductMethod::WorkspaceArrange => Some("conversation.write"),
            ProductMethod::WorkspaceClosePane => Some("conversation.write"),
            ProductMethod::ChannelCreate => Some("conversation.write"),
            ProductMethod::ChannelList => Some("conversation.write"),
            ProductMethod::ChannelSetCoordinator => Some("conversation.write"),
            ProductMethod::ReplyStyleRead => Some("conversation.write"),
            ProductMethod::ReplyStyleSet => Some("conversation.write"),
            ProductMethod::ConversationSubscribeEvents => Some("conversation.write"),
            ProductMethod::ConversationReadEarlier => Some("conversation.write"),
            ProductMethod::ConversationPeek => Some("conversation.write"),
            ProductMethod::ConversationSetModel => Some("conversation.write"),
            ProductMethod::ConversationSwitchAgent => Some("conversation.write"),
            ProductMethod::ConversationFork => Some("conversation.write"),
            ProductMethod::VaultStatus => Some("credential.manage"),
            ProductMethod::VaultSetup => Some("credential.manage"),
            ProductMethod::VaultUnlock => Some("credential.manage"),
            ProductMethod::VaultChangePassphrase => Some("credential.manage"),
            ProductMethod::VaultLock => Some("credential.manage"),
            ProductMethod::BoatStatus => Some("credential.manage"),
            ProductMethod::BoatSave => Some("credential.manage"),
            ProductMethod::BoatDisconnect => Some("credential.manage"),
            ProductMethod::Agent37Status => Some("credential.manage"),
            ProductMethod::Agent37Save => Some("credential.manage"),
            ProductMethod::Agent37Disconnect => Some("credential.manage"),
            ProductMethod::CloudRead => Some("credential.manage"),
            ProductMethod::CloudChoose => Some("credential.manage"),
            ProductMethod::CloudBoxesList => Some("credential.manage"),
            ProductMethod::CloudBoxesCreate => Some("credential.manage"),
            ProductMethod::CloudBoxesWake => Some("credential.manage"),
            ProductMethod::CloudBoxesSleep => Some("credential.manage"),
            ProductMethod::CloudBoxesRename => Some("credential.manage"),
            ProductMethod::CloudBoxesDelete => Some("credential.manage"),
            ProductMethod::ConversationWebhookCursor => Some("credential.manage"),
            ProductMethod::ConversationWebhookReset => Some("credential.manage"),
            ProductMethod::VoiceStatus => Some("credential.manage"),
            ProductMethod::VoiceSaveKey => Some("credential.manage"),
            ProductMethod::VoiceForgetKey => Some("credential.manage"),
            ProductMethod::VoiceStartCall => Some("conversation.write"),
            ProductMethod::TerminalOpen => Some("credential.manage"),
            ProductMethod::TerminalList => Some("credential.manage"),
            ProductMethod::TerminalAttach => Some("credential.manage"),
            ProductMethod::TerminalFocus => Some("credential.manage"),
            ProductMethod::TerminalFit => Some("credential.manage"),
            ProductMethod::TerminalDetach => Some("credential.manage"),
            ProductMethod::TerminalInput => Some("credential.manage"),
            ProductMethod::TerminalHistory => Some("credential.manage"),
            ProductMethod::TerminalClose => Some("credential.manage"),
            ProductMethod::TerminalRename => Some("credential.manage"),
            ProductMethod::TerminalResumeSettings => Some("credential.manage"),
            ProductMethod::TerminalSetResumeSettings => Some("credential.manage"),
            ProductMethod::TerminalForgetSaved => Some("credential.manage"),
        }
    }

    /// The admission lane the method waits in.
    pub const fn lane(self) -> ProductLane {
        match self {
            ProductMethod::SessionAuthenticate => ProductLane::Requests,
            ProductMethod::AuthSession => ProductLane::Requests,
            ProductMethod::ServerHealth => ProductLane::Requests,
            ProductMethod::ServerReach => ProductLane::Requests,
            ProductMethod::CredentialIssue => ProductLane::Requests,
            ProductMethod::CredentialList => ProductLane::Requests,
            ProductMethod::CredentialRevoke => ProductLane::Requests,
            ProductMethod::ConversationCreate => ProductLane::Requests,
            ProductMethod::ConversationRead => ProductLane::Requests,
            ProductMethod::ConversationRecordsHead => ProductLane::RecordReads,
            ProductMethod::ConversationRecordsPage => ProductLane::RecordReads,
            ProductMethod::ConversationList => ProductLane::Requests,
            ProductMethod::ConversationObserve => ProductLane::Requests,
            ProductMethod::ConversationSend => ProductLane::Requests,
            ProductMethod::ConversationSteer => ProductLane::Requests,
            ProductMethod::ConversationRemove => ProductLane::Controls,
            ProductMethod::ConversationStop => ProductLane::Controls,
            ProductMethod::ConversationReceipt => ProductLane::Requests,
            ProductMethod::ConversationMessage => ProductLane::Requests,
            ProductMethod::ConversationMessageStatus => ProductLane::Requests,
            ProductMethod::ConversationAnswer => ProductLane::Controls,
            ProductMethod::ConversationPermissionReview => ProductLane::Controls,
            ProductMethod::ConversationQuestionReview => ProductLane::Controls,
            ProductMethod::ConversationAnswerQuestion => ProductLane::Controls,
            ProductMethod::ConversationCancel => ProductLane::Controls,
            ProductMethod::ConversationClose => ProductLane::Controls,
            ProductMethod::ConversationArchive => ProductLane::Controls,
            ProductMethod::ConversationUnarchive => ProductLane::Controls,
            ProductMethod::ConversationRename => ProductLane::Controls,
            ProductMethod::ConversationDecisions => ProductLane::Requests,
            ProductMethod::ConversationDecisionSettings => ProductLane::Requests,
            ProductMethod::ConversationPauseAutoPick => ProductLane::Requests,
            ProductMethod::ConversationSetDecisionSettings => ProductLane::Requests,
            ProductMethod::ConversationDelete => ProductLane::Deletions,
            ProductMethod::ConversationReorder => ProductLane::Controls,
            ProductMethod::ArtifactRead => ProductLane::ArtifactReads,
            ProductMethod::AttachmentBegin => ProductLane::UploadBegins,
            ProductMethod::AttachmentWrite => ProductLane::Requests,
            ProductMethod::AgentsList => ProductLane::Requests,
            ProductMethod::AgentsSetNativeHooks => ProductLane::Requests,
            ProductMethod::AgentsSubscribe => ProductLane::Requests,
            ProductMethod::AgentsUnsubscribe => ProductLane::Requests,
            ProductMethod::ConversationSetApprovalMode => ProductLane::Requests,
            ProductMethod::ConversationSetUsageLimitAutoContinue => ProductLane::Requests,
            ProductMethod::AgentsInstallOptions => ProductLane::Requests,
            ProductMethod::AgentsInstall => ProductLane::Requests,
            ProductMethod::ConversationCatalogueHead => ProductLane::RecordReads,
            ProductMethod::ConversationCatalogueManifest => ProductLane::RecordReads,
            ProductMethod::ConversationCatalogueResolve => ProductLane::RecordReads,
            ProductMethod::McpCallTool => ProductLane::AppCalls,
            ProductMethod::McpReadResource => ProductLane::AppCalls,
            ProductMethod::McpReleaseApp => ProductLane::Controls,
            ProductMethod::McpSendMessage => ProductLane::AppCalls,
            ProductMethod::McpUpdateModelContext => ProductLane::AppCalls,
            ProductMethod::McpServersList => ProductLane::Requests,
            ProductMethod::McpServersSave => ProductLane::Requests,
            ProductMethod::McpServersRemove => ProductLane::Requests,
            ProductMethod::McpServersInspect => ProductLane::Requests,
            ProductMethod::McpServersAuthorize => ProductLane::Requests,
            ProductMethod::McpServersRevoke => ProductLane::Requests,
            ProductMethod::AppsTrusted => ProductLane::Requests,
            ProductMethod::AppsUntrust => ProductLane::Requests,
            ProductMethod::AppsStarter => ProductLane::Requests,
            ProductMethod::AppsReview => ProductLane::Requests,
            ProductMethod::AppsInstall => ProductLane::Requests,
            ProductMethod::AppsInstalled => ProductLane::Requests,
            ProductMethod::AppsRead => ProductLane::Requests,
            ProductMethod::AppsUninstall => ProductLane::Requests,
            ProductMethod::AppsIssueCredential => ProductLane::Requests,
            ProductMethod::AppsRevokeCredentials => ProductLane::Requests,
            ProductMethod::PhoneEndpoint => ProductLane::Requests,
            ProductMethod::PhoneAuthorize => ProductLane::Requests,
            ProductMethod::PhoneStatus => ProductLane::Requests,
            ProductMethod::PairingCreate => ProductLane::Requests,
            ProductMethod::PairingPending => ProductLane::Requests,
            ProductMethod::PairingStatus => ProductLane::Requests,
            ProductMethod::PairingApprove => ProductLane::Requests,
            ProductMethod::PairingDeny => ProductLane::Controls,
            ProductMethod::PairingCancel => ProductLane::Controls,
            ProductMethod::PairingSetLinking => ProductLane::Requests,
            ProductMethod::PairingSetTunnel => ProductLane::Requests,
            ProductMethod::PairingTunnel => ProductLane::Requests,
            ProductMethod::GithubLiveEvents => ProductLane::Requests,
            ProductMethod::GithubStatus => ProductLane::Requests,
            ProductMethod::GithubConnect => ProductLane::Requests,
            ProductMethod::DevicesList => ProductLane::Requests,
            ProductMethod::GithubCancel => ProductLane::Requests,
            ProductMethod::GithubDisconnect => ProductLane::Requests,
            ProductMethod::PeerEnroll => ProductLane::Requests,
            ProductMethod::PeerList => ProductLane::Requests,
            ProductMethod::PeerForget => ProductLane::Requests,
            ProductMethod::PeerRelay => ProductLane::Requests,
            ProductMethod::PeerFollow => ProductLane::Requests,
            ProductMethod::GrantsSubscribe => ProductLane::Requests,
            ProductMethod::GrantsPage => ProductLane::Requests,
            ProductMethod::GrantsUnsubscribe => ProductLane::Requests,
            ProductMethod::ConversationSubscribe => ProductLane::Requests,
            ProductMethod::ConversationSubscribeList => ProductLane::Requests,
            ProductMethod::ConversationUnsubscribe => ProductLane::Requests,
            ProductMethod::ConversationShare => ProductLane::Requests,
            ProductMethod::ConversationUnshare => ProductLane::Requests,
            ProductMethod::ConversationShares => ProductLane::Requests,
            ProductMethod::ConversationPermissionRules => ProductLane::Requests,
            ProductMethod::ConversationRevokePermissionRule => ProductLane::Requests,
            ProductMethod::WorkspacePublish => ProductLane::Requests,
            ProductMethod::WorkspaceAnswer => ProductLane::Requests,
            ProductMethod::WorkspaceArrange => ProductLane::Requests,
            ProductMethod::WorkspaceClosePane => ProductLane::Requests,
            ProductMethod::ChannelCreate => ProductLane::Requests,
            ProductMethod::ChannelList => ProductLane::Requests,
            ProductMethod::ChannelSetCoordinator => ProductLane::Requests,
            ProductMethod::ReplyStyleRead => ProductLane::Requests,
            ProductMethod::ReplyStyleSet => ProductLane::Requests,
            ProductMethod::ConversationSubscribeEvents => ProductLane::Requests,
            ProductMethod::ConversationReadEarlier => ProductLane::Requests,
            ProductMethod::ConversationPeek => ProductLane::Requests,
            ProductMethod::ConversationSetModel => ProductLane::Requests,
            ProductMethod::ConversationSwitchAgent => ProductLane::Requests,
            ProductMethod::ConversationFork => ProductLane::Requests,
            ProductMethod::VaultStatus => ProductLane::Requests,
            ProductMethod::VaultSetup => ProductLane::Requests,
            ProductMethod::VaultUnlock => ProductLane::Requests,
            ProductMethod::VaultChangePassphrase => ProductLane::Requests,
            ProductMethod::VaultLock => ProductLane::Requests,
            ProductMethod::BoatStatus => ProductLane::Requests,
            ProductMethod::BoatSave => ProductLane::Requests,
            ProductMethod::BoatDisconnect => ProductLane::Requests,
            ProductMethod::Agent37Status => ProductLane::Requests,
            ProductMethod::Agent37Save => ProductLane::Requests,
            ProductMethod::Agent37Disconnect => ProductLane::Requests,
            ProductMethod::CloudRead => ProductLane::Requests,
            ProductMethod::CloudChoose => ProductLane::Requests,
            ProductMethod::CloudBoxesList => ProductLane::Requests,
            ProductMethod::CloudBoxesCreate => ProductLane::Requests,
            ProductMethod::CloudBoxesWake => ProductLane::Requests,
            ProductMethod::CloudBoxesSleep => ProductLane::Requests,
            ProductMethod::CloudBoxesRename => ProductLane::Requests,
            ProductMethod::CloudBoxesDelete => ProductLane::Requests,
            ProductMethod::ConversationWebhookCursor => ProductLane::Requests,
            ProductMethod::ConversationWebhookReset => ProductLane::Requests,
            ProductMethod::VoiceStatus => ProductLane::Requests,
            ProductMethod::VoiceSaveKey => ProductLane::Requests,
            ProductMethod::VoiceForgetKey => ProductLane::Requests,
            ProductMethod::VoiceStartCall => ProductLane::Requests,
            ProductMethod::TerminalOpen => ProductLane::Requests,
            ProductMethod::TerminalList => ProductLane::Requests,
            ProductMethod::TerminalAttach => ProductLane::Requests,
            ProductMethod::TerminalFocus => ProductLane::Requests,
            ProductMethod::TerminalFit => ProductLane::Requests,
            ProductMethod::TerminalDetach => ProductLane::Requests,
            ProductMethod::TerminalInput => ProductLane::Requests,
            ProductMethod::TerminalHistory => ProductLane::Requests,
            ProductMethod::TerminalClose => ProductLane::Requests,
            ProductMethod::TerminalRename => ProductLane::Requests,
            ProductMethod::TerminalResumeSettings => ProductLane::Requests,
            ProductMethod::TerminalSetResumeSettings => ProductLane::Requests,
            ProductMethod::TerminalForgetSaved => ProductLane::Requests,
        }
    }

    /// Decode `params` into this method's declared params type. A method
    /// whose manifest params are `null` takes an empty object.
    pub fn decode_params(self, params: Value) -> Result<ProductParams, ParamsRefused> {
        Ok(match self {
            ProductMethod::SessionAuthenticate => ProductParams::SessionAuthenticate(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::AuthSession => match params {
                Value::Object(fields) if fields.is_empty() => ProductParams::AuthSession,
                _ => return Err(ParamsRefused),
            },
            ProductMethod::ServerHealth => match params {
                Value::Object(fields) if fields.is_empty() => ProductParams::ServerHealth,
                _ => return Err(ParamsRefused),
            },
            ProductMethod::ServerReach => match params {
                Value::Object(fields) if fields.is_empty() => ProductParams::ServerReach,
                _ => return Err(ParamsRefused),
            },
            ProductMethod::CredentialIssue => ProductParams::CredentialIssue(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::CredentialList => match params {
                Value::Object(fields) if fields.is_empty() => ProductParams::CredentialList,
                _ => return Err(ParamsRefused),
            },
            ProductMethod::CredentialRevoke => ProductParams::CredentialRevoke(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::ConversationCreate => ProductParams::ConversationCreate(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::ConversationRead => ProductParams::ConversationRead(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::ConversationRecordsHead => ProductParams::ConversationRecordsHead(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::ConversationRecordsPage => ProductParams::ConversationRecordsPage(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::ConversationList => ProductParams::ConversationList(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::ConversationObserve => ProductParams::ConversationObserve(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::ConversationSend => ProductParams::ConversationSend(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::ConversationSteer => ProductParams::ConversationSteer(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::ConversationRemove => ProductParams::ConversationRemove(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::ConversationStop => ProductParams::ConversationStop(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::ConversationReceipt => ProductParams::ConversationReceipt(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::ConversationMessage => ProductParams::ConversationMessage(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::ConversationMessageStatus => ProductParams::ConversationMessageStatus(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::ConversationAnswer => ProductParams::ConversationAnswer(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::ConversationPermissionReview => {
                ProductParams::ConversationPermissionReview(
                    serde_json::from_value(params).map_err(|_| ParamsRefused)?,
                )
            }
            ProductMethod::ConversationQuestionReview => ProductParams::ConversationQuestionReview(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::ConversationAnswerQuestion => ProductParams::ConversationAnswerQuestion(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::ConversationCancel => ProductParams::ConversationCancel(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::ConversationClose => ProductParams::ConversationClose(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::ConversationArchive => ProductParams::ConversationArchive(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::ConversationUnarchive => ProductParams::ConversationUnarchive(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::ConversationRename => ProductParams::ConversationRename(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::ConversationDecisions => ProductParams::ConversationDecisions(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::ConversationDecisionSettings => {
                ProductParams::ConversationDecisionSettings(
                    serde_json::from_value(params).map_err(|_| ParamsRefused)?,
                )
            }
            ProductMethod::ConversationPauseAutoPick => ProductParams::ConversationPauseAutoPick(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::ConversationSetDecisionSettings => {
                ProductParams::ConversationSetDecisionSettings(
                    serde_json::from_value(params).map_err(|_| ParamsRefused)?,
                )
            }
            ProductMethod::ConversationDelete => ProductParams::ConversationDelete(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::ConversationReorder => ProductParams::ConversationReorder(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::ArtifactRead => ProductParams::ArtifactRead(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::AttachmentBegin => ProductParams::AttachmentBegin(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::AttachmentWrite => ProductParams::AttachmentWrite(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::AgentsList => match params {
                Value::Object(fields) if fields.is_empty() => ProductParams::AgentsList,
                _ => return Err(ParamsRefused),
            },
            ProductMethod::AgentsSetNativeHooks => ProductParams::AgentsSetNativeHooks(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::AgentsSubscribe => match params {
                Value::Object(fields) if fields.is_empty() => ProductParams::AgentsSubscribe,
                _ => return Err(ParamsRefused),
            },
            ProductMethod::AgentsUnsubscribe => ProductParams::AgentsUnsubscribe(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::ConversationSetApprovalMode => {
                ProductParams::ConversationSetApprovalMode(
                    serde_json::from_value(params).map_err(|_| ParamsRefused)?,
                )
            }
            ProductMethod::ConversationSetUsageLimitAutoContinue => {
                ProductParams::ConversationSetUsageLimitAutoContinue(
                    serde_json::from_value(params).map_err(|_| ParamsRefused)?,
                )
            }
            ProductMethod::AgentsInstallOptions => match params {
                Value::Object(fields) if fields.is_empty() => ProductParams::AgentsInstallOptions,
                _ => return Err(ParamsRefused),
            },
            ProductMethod::AgentsInstall => ProductParams::AgentsInstall(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::ConversationCatalogueHead => ProductParams::ConversationCatalogueHead(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::ConversationCatalogueManifest => {
                ProductParams::ConversationCatalogueManifest(
                    serde_json::from_value(params).map_err(|_| ParamsRefused)?,
                )
            }
            ProductMethod::ConversationCatalogueResolve => {
                ProductParams::ConversationCatalogueResolve(
                    serde_json::from_value(params).map_err(|_| ParamsRefused)?,
                )
            }
            ProductMethod::McpCallTool => ProductParams::McpCallTool(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::McpReadResource => ProductParams::McpReadResource(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::McpReleaseApp => ProductParams::McpReleaseApp(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::McpSendMessage => ProductParams::McpSendMessage(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::McpUpdateModelContext => ProductParams::McpUpdateModelContext(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::McpServersList => match params {
                Value::Object(fields) if fields.is_empty() => ProductParams::McpServersList,
                _ => return Err(ParamsRefused),
            },
            ProductMethod::McpServersSave => ProductParams::McpServersSave(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::McpServersRemove => ProductParams::McpServersRemove(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::McpServersInspect => ProductParams::McpServersInspect(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::McpServersAuthorize => ProductParams::McpServersAuthorize(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::McpServersRevoke => ProductParams::McpServersRevoke(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::AppsTrusted => match params {
                Value::Object(fields) if fields.is_empty() => ProductParams::AppsTrusted,
                _ => return Err(ParamsRefused),
            },
            ProductMethod::AppsUntrust => ProductParams::AppsUntrust(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::AppsStarter => ProductParams::AppsStarter(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::AppsReview => ProductParams::AppsReview(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::AppsInstall => ProductParams::AppsInstall(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::AppsInstalled => match params {
                Value::Object(fields) if fields.is_empty() => ProductParams::AppsInstalled,
                _ => return Err(ParamsRefused),
            },
            ProductMethod::AppsRead => {
                ProductParams::AppsRead(serde_json::from_value(params).map_err(|_| ParamsRefused)?)
            }
            ProductMethod::AppsUninstall => ProductParams::AppsUninstall(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::AppsIssueCredential => ProductParams::AppsIssueCredential(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::AppsRevokeCredentials => ProductParams::AppsRevokeCredentials(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::PhoneEndpoint => ProductParams::PhoneEndpoint(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::PhoneAuthorize => ProductParams::PhoneAuthorize(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::PhoneStatus => ProductParams::PhoneStatus(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::PairingCreate => ProductParams::PairingCreate(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::PairingPending => match params {
                Value::Object(fields) if fields.is_empty() => ProductParams::PairingPending,
                _ => return Err(ParamsRefused),
            },
            ProductMethod::PairingStatus => ProductParams::PairingStatus(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::PairingApprove => ProductParams::PairingApprove(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::PairingDeny => ProductParams::PairingDeny(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::PairingCancel => ProductParams::PairingCancel(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::PairingSetLinking => ProductParams::PairingSetLinking(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::PairingSetTunnel => ProductParams::PairingSetTunnel(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::PairingTunnel => match params {
                Value::Object(fields) if fields.is_empty() => ProductParams::PairingTunnel,
                _ => return Err(ParamsRefused),
            },
            ProductMethod::GithubLiveEvents => ProductParams::GithubLiveEvents(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::GithubStatus => match params {
                Value::Object(fields) if fields.is_empty() => ProductParams::GithubStatus,
                _ => return Err(ParamsRefused),
            },
            ProductMethod::GithubConnect => match params {
                Value::Object(fields) if fields.is_empty() => ProductParams::GithubConnect,
                _ => return Err(ParamsRefused),
            },
            ProductMethod::DevicesList => match params {
                Value::Object(fields) if fields.is_empty() => ProductParams::DevicesList,
                _ => return Err(ParamsRefused),
            },
            ProductMethod::GithubCancel => match params {
                Value::Object(fields) if fields.is_empty() => ProductParams::GithubCancel,
                _ => return Err(ParamsRefused),
            },
            ProductMethod::GithubDisconnect => match params {
                Value::Object(fields) if fields.is_empty() => ProductParams::GithubDisconnect,
                _ => return Err(ParamsRefused),
            },
            ProductMethod::PeerEnroll => ProductParams::PeerEnroll(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::PeerList => match params {
                Value::Object(fields) if fields.is_empty() => ProductParams::PeerList,
                _ => return Err(ParamsRefused),
            },
            ProductMethod::PeerForget => ProductParams::PeerForget(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::PeerRelay => {
                ProductParams::PeerRelay(serde_json::from_value(params).map_err(|_| ParamsRefused)?)
            }
            ProductMethod::PeerFollow => ProductParams::PeerFollow(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::GrantsSubscribe => ProductParams::GrantsSubscribe(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::GrantsPage => ProductParams::GrantsPage(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::GrantsUnsubscribe => ProductParams::GrantsUnsubscribe(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::ConversationSubscribe => ProductParams::ConversationSubscribe(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::ConversationSubscribeList => ProductParams::ConversationSubscribeList(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::ConversationUnsubscribe => ProductParams::ConversationUnsubscribe(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::ConversationShare => ProductParams::ConversationShare(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::ConversationUnshare => ProductParams::ConversationUnshare(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::ConversationShares => ProductParams::ConversationShares(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::ConversationPermissionRules => {
                ProductParams::ConversationPermissionRules(
                    serde_json::from_value(params).map_err(|_| ParamsRefused)?,
                )
            }
            ProductMethod::ConversationRevokePermissionRule => {
                ProductParams::ConversationRevokePermissionRule(
                    serde_json::from_value(params).map_err(|_| ParamsRefused)?,
                )
            }
            ProductMethod::WorkspacePublish => ProductParams::WorkspacePublish(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::WorkspaceAnswer => ProductParams::WorkspaceAnswer(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::WorkspaceArrange => ProductParams::WorkspaceArrange(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::WorkspaceClosePane => ProductParams::WorkspaceClosePane(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::ChannelCreate => ProductParams::ChannelCreate(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::ChannelList => ProductParams::ChannelList(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::ChannelSetCoordinator => ProductParams::ChannelSetCoordinator(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::ReplyStyleRead => ProductParams::ReplyStyleRead(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::ReplyStyleSet => ProductParams::ReplyStyleSet(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::ConversationSubscribeEvents => {
                ProductParams::ConversationSubscribeEvents(
                    serde_json::from_value(params).map_err(|_| ParamsRefused)?,
                )
            }
            ProductMethod::ConversationReadEarlier => ProductParams::ConversationReadEarlier(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::ConversationPeek => ProductParams::ConversationPeek(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::ConversationSetModel => ProductParams::ConversationSetModel(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::ConversationSwitchAgent => ProductParams::ConversationSwitchAgent(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::ConversationFork => ProductParams::ConversationFork(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::VaultStatus => match params {
                Value::Object(fields) if fields.is_empty() => ProductParams::VaultStatus,
                _ => return Err(ParamsRefused),
            },
            ProductMethod::VaultSetup => ProductParams::VaultSetup(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::VaultUnlock => ProductParams::VaultUnlock(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::VaultChangePassphrase => ProductParams::VaultChangePassphrase(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::VaultLock => match params {
                Value::Object(fields) if fields.is_empty() => ProductParams::VaultLock,
                _ => return Err(ParamsRefused),
            },
            ProductMethod::BoatStatus => match params {
                Value::Object(fields) if fields.is_empty() => ProductParams::BoatStatus,
                _ => return Err(ParamsRefused),
            },
            ProductMethod::BoatSave => {
                ProductParams::BoatSave(serde_json::from_value(params).map_err(|_| ParamsRefused)?)
            }
            ProductMethod::BoatDisconnect => match params {
                Value::Object(fields) if fields.is_empty() => ProductParams::BoatDisconnect,
                _ => return Err(ParamsRefused),
            },
            ProductMethod::Agent37Status => match params {
                Value::Object(fields) if fields.is_empty() => ProductParams::Agent37Status,
                _ => return Err(ParamsRefused),
            },
            ProductMethod::Agent37Save => ProductParams::Agent37Save(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::Agent37Disconnect => match params {
                Value::Object(fields) if fields.is_empty() => ProductParams::Agent37Disconnect,
                _ => return Err(ParamsRefused),
            },
            ProductMethod::CloudRead => match params {
                Value::Object(fields) if fields.is_empty() => ProductParams::CloudRead,
                _ => return Err(ParamsRefused),
            },
            ProductMethod::CloudChoose => ProductParams::CloudChoose(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::CloudBoxesList => match params {
                Value::Object(fields) if fields.is_empty() => ProductParams::CloudBoxesList,
                _ => return Err(ParamsRefused),
            },
            ProductMethod::CloudBoxesCreate => ProductParams::CloudBoxesCreate(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::CloudBoxesWake => ProductParams::CloudBoxesWake(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::CloudBoxesSleep => ProductParams::CloudBoxesSleep(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::CloudBoxesRename => ProductParams::CloudBoxesRename(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::CloudBoxesDelete => ProductParams::CloudBoxesDelete(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::ConversationWebhookCursor => ProductParams::ConversationWebhookCursor(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::ConversationWebhookReset => ProductParams::ConversationWebhookReset(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::VoiceStatus => match params {
                Value::Object(fields) if fields.is_empty() => ProductParams::VoiceStatus,
                _ => return Err(ParamsRefused),
            },
            ProductMethod::VoiceSaveKey => ProductParams::VoiceSaveKey(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::VoiceForgetKey => match params {
                Value::Object(fields) if fields.is_empty() => ProductParams::VoiceForgetKey,
                _ => return Err(ParamsRefused),
            },
            ProductMethod::VoiceStartCall => ProductParams::VoiceStartCall(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::TerminalOpen => ProductParams::TerminalOpen(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::TerminalList => match params {
                Value::Object(fields) if fields.is_empty() => ProductParams::TerminalList,
                _ => return Err(ParamsRefused),
            },
            ProductMethod::TerminalAttach => ProductParams::TerminalAttach(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::TerminalFocus => ProductParams::TerminalFocus(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::TerminalFit => ProductParams::TerminalFit(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::TerminalDetach => ProductParams::TerminalDetach(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::TerminalInput => ProductParams::TerminalInput(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::TerminalHistory => ProductParams::TerminalHistory(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::TerminalClose => ProductParams::TerminalClose(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::TerminalRename => ProductParams::TerminalRename(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::TerminalResumeSettings => match params {
                Value::Object(fields) if fields.is_empty() => ProductParams::TerminalResumeSettings,
                _ => return Err(ParamsRefused),
            },
            ProductMethod::TerminalSetResumeSettings => ProductParams::TerminalSetResumeSettings(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
            ProductMethod::TerminalForgetSaved => ProductParams::TerminalForgetSaved(
                serde_json::from_value(params).map_err(|_| ParamsRefused)?,
            ),
        })
    }
}

/// Params that do not decode into their method's declared type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParamsRefused;

/// A method's params, decoded into the type the manifest declares for it.
pub enum ProductParams {
    SessionAuthenticate(SessionAuthenticateParams),
    AuthSession,
    ServerHealth,
    ServerReach,
    CredentialIssue(CredentialIssueParams),
    CredentialList,
    CredentialRevoke(CredentialRevokeParams),
    ConversationCreate(ConversationCreateParams),
    ConversationRead(ConversationReadParams),
    ConversationRecordsHead(ConversationRecordsHeadParams),
    ConversationRecordsPage(ConversationRecordsPageParams),
    ConversationList(ConversationListParams),
    ConversationObserve(ConversationObserveParams),
    ConversationSend(ConversationSendParams),
    ConversationSteer(ConversationSendParams),
    ConversationRemove(ConversationRemoveParams),
    ConversationStop(ConversationStopParams),
    ConversationReceipt(ConversationReceiptParams),
    ConversationMessage(ConversationMessageParams),
    ConversationMessageStatus(ConversationMessageStatusParams),
    ConversationAnswer(ConversationAnswerParams),
    ConversationPermissionReview(ConversationPermissionReviewParams),
    ConversationQuestionReview(ConversationQuestionReviewParams),
    ConversationAnswerQuestion(ConversationAnswerQuestionParams),
    ConversationCancel(ConversationCancelParams),
    ConversationClose(ConversationCloseParams),
    ConversationArchive(ConversationArchiveParams),
    ConversationUnarchive(ConversationArchiveParams),
    ConversationRename(ConversationRenameParams),
    ConversationDecisions(ConversationDecisionsParams),
    ConversationDecisionSettings(ConversationDecisionSettingsParams),
    ConversationPauseAutoPick(ConversationPauseAutoPickParams),
    ConversationSetDecisionSettings(DecisionSettings),
    ConversationDelete(ConversationDeleteParams),
    ConversationReorder(ConversationReorderParams),
    ArtifactRead(ArtifactReadParams),
    AttachmentBegin(AttachmentBeginParams),
    AttachmentWrite(AttachmentWriteParams),
    AgentsList,
    AgentsSetNativeHooks(AgentsSetNativeHooksParams),
    AgentsSubscribe,
    AgentsUnsubscribe(ConversationUnsubscribeParams),
    ConversationSetApprovalMode(ConversationSetApprovalModeParams),
    ConversationSetUsageLimitAutoContinue(ConversationSetUsageLimitAutoContinueParams),
    AgentsInstallOptions,
    AgentsInstall(AgentInstallParams),
    ConversationCatalogueHead(ConversationCatalogueHeadParams),
    ConversationCatalogueManifest(ConversationCatalogueManifestParams),
    ConversationCatalogueResolve(ConversationCatalogueResolveParams),
    McpCallTool(McpCallToolParams),
    McpReadResource(McpReadResourceParams),
    McpReleaseApp(McpReleaseAppParams),
    McpSendMessage(McpSendMessageParams),
    McpUpdateModelContext(McpUpdateModelContextParams),
    McpServersList,
    McpServersSave(McpServersSaveParams),
    McpServersRemove(McpServersRemoveParams),
    McpServersInspect(McpServersInspectParams),
    McpServersAuthorize(McpServersAuthorizeParams),
    McpServersRevoke(McpServersRevokeParams),
    AppsTrusted,
    AppsUntrust(AppsUntrustParams),
    AppsStarter(AppsStarterParams),
    AppsReview(AppsReviewParams),
    AppsInstall(AppsInstallParams),
    AppsInstalled,
    AppsRead(AppsReadParams),
    AppsUninstall(AppsUninstallParams),
    AppsIssueCredential(AppsIssueCredentialParams),
    AppsRevokeCredentials(AppsRevokeCredentialsParams),
    PhoneEndpoint(PhoneEndpointParams),
    PhoneAuthorize(PhoneAuthorizeParams),
    PhoneStatus(PhoneStatusParams),
    PairingCreate(PairingCreateParams),
    PairingPending,
    PairingStatus(PairingInvitationParams),
    PairingApprove(PairingApproveParams),
    PairingDeny(PairingInvitationParams),
    PairingCancel(PairingInvitationParams),
    PairingSetLinking(PairingSetLinkingParams),
    PairingSetTunnel(PairingSetTunnelParams),
    PairingTunnel,
    GithubLiveEvents(GithubLiveEventsParams),
    GithubStatus,
    GithubConnect,
    DevicesList,
    GithubCancel,
    GithubDisconnect,
    PeerEnroll(PeerEnrollParams),
    PeerList,
    PeerForget(PeerForgetParams),
    PeerRelay(PeerRelayParams),
    PeerFollow(PeerFollowParams),
    GrantsSubscribe(GrantsSubscribeParams),
    GrantsPage(GrantsPageParams),
    GrantsUnsubscribe(GrantsUnsubscribeParams),
    ConversationSubscribe(ConversationSubscribeParams),
    ConversationSubscribeList(ConversationSubscribeListParams),
    ConversationUnsubscribe(ConversationUnsubscribeParams),
    ConversationShare(ConversationShareParams),
    ConversationUnshare(ConversationUnshareParams),
    ConversationShares(ConversationSharesParams),
    ConversationPermissionRules(ConversationPermissionRulesParams),
    ConversationRevokePermissionRule(ConversationRevokePermissionRuleParams),
    WorkspacePublish(WorkspacePublishParams),
    WorkspaceAnswer(WorkspaceAnswerParams),
    WorkspaceArrange(WorkspaceArrangeParams),
    WorkspaceClosePane(WorkspaceClosePaneParams),
    ChannelCreate(ChannelCreateParams),
    ChannelList(ChannelListParams),
    ChannelSetCoordinator(ChannelSetCoordinatorParams),
    ReplyStyleRead(ReplyStyleReadParams),
    ReplyStyleSet(ReplyStyleSetParams),
    ConversationSubscribeEvents(ConversationSubscribeEventsParams),
    ConversationReadEarlier(ConversationReadEarlierParams),
    ConversationPeek(ConversationReadParams),
    ConversationSetModel(ConversationSetModelParams),
    ConversationSwitchAgent(ConversationSwitchAgentParams),
    ConversationFork(ConversationForkParams),
    VaultStatus,
    VaultSetup(VaultPassphraseParams),
    VaultUnlock(VaultPassphraseParams),
    VaultChangePassphrase(VaultPassphraseParams),
    VaultLock,
    BoatStatus,
    BoatSave(BoatSaveParams),
    BoatDisconnect,
    Agent37Status,
    Agent37Save(Agent37SaveParams),
    Agent37Disconnect,
    CloudRead,
    CloudChoose(CloudChooseParams),
    CloudBoxesList,
    CloudBoxesCreate(CloudBoxCreateParams),
    CloudBoxesWake(CloudBoxIdParams),
    CloudBoxesSleep(CloudBoxIdParams),
    CloudBoxesRename(CloudBoxRenameParams),
    CloudBoxesDelete(CloudBoxIdParams),
    ConversationWebhookCursor(ConversationWebhookCursorParams),
    ConversationWebhookReset(ConversationWebhookResetParams),
    VoiceStatus,
    VoiceSaveKey(VoiceSaveKeyParams),
    VoiceForgetKey,
    VoiceStartCall(VoiceStartCallParams),
    TerminalOpen(TerminalOpenParams),
    TerminalList,
    TerminalAttach(TerminalViewParams),
    TerminalFocus(TerminalViewParams),
    TerminalFit(TerminalViewParams),
    TerminalDetach(TerminalDetachParams),
    TerminalInput(TerminalInputParams),
    TerminalHistory(TerminalHistoryParams),
    TerminalClose(TerminalCloseParams),
    TerminalRename(TerminalRenameParams),
    TerminalResumeSettings,
    TerminalSetResumeSettings(TerminalResumeSettings),
    TerminalForgetSaved(TerminalForgetSavedParams),
}

impl ProductParams {
    /// The method these params belong to.
    pub const fn method(&self) -> ProductMethod {
        match self {
            ProductParams::SessionAuthenticate(_) => ProductMethod::SessionAuthenticate,
            ProductParams::AuthSession => ProductMethod::AuthSession,
            ProductParams::ServerHealth => ProductMethod::ServerHealth,
            ProductParams::ServerReach => ProductMethod::ServerReach,
            ProductParams::CredentialIssue(_) => ProductMethod::CredentialIssue,
            ProductParams::CredentialList => ProductMethod::CredentialList,
            ProductParams::CredentialRevoke(_) => ProductMethod::CredentialRevoke,
            ProductParams::ConversationCreate(_) => ProductMethod::ConversationCreate,
            ProductParams::ConversationRead(_) => ProductMethod::ConversationRead,
            ProductParams::ConversationRecordsHead(_) => ProductMethod::ConversationRecordsHead,
            ProductParams::ConversationRecordsPage(_) => ProductMethod::ConversationRecordsPage,
            ProductParams::ConversationList(_) => ProductMethod::ConversationList,
            ProductParams::ConversationObserve(_) => ProductMethod::ConversationObserve,
            ProductParams::ConversationSend(_) => ProductMethod::ConversationSend,
            ProductParams::ConversationSteer(_) => ProductMethod::ConversationSteer,
            ProductParams::ConversationRemove(_) => ProductMethod::ConversationRemove,
            ProductParams::ConversationStop(_) => ProductMethod::ConversationStop,
            ProductParams::ConversationReceipt(_) => ProductMethod::ConversationReceipt,
            ProductParams::ConversationMessage(_) => ProductMethod::ConversationMessage,
            ProductParams::ConversationMessageStatus(_) => ProductMethod::ConversationMessageStatus,
            ProductParams::ConversationAnswer(_) => ProductMethod::ConversationAnswer,
            ProductParams::ConversationPermissionReview(_) => {
                ProductMethod::ConversationPermissionReview
            }
            ProductParams::ConversationQuestionReview(_) => {
                ProductMethod::ConversationQuestionReview
            }
            ProductParams::ConversationAnswerQuestion(_) => {
                ProductMethod::ConversationAnswerQuestion
            }
            ProductParams::ConversationCancel(_) => ProductMethod::ConversationCancel,
            ProductParams::ConversationClose(_) => ProductMethod::ConversationClose,
            ProductParams::ConversationArchive(_) => ProductMethod::ConversationArchive,
            ProductParams::ConversationUnarchive(_) => ProductMethod::ConversationUnarchive,
            ProductParams::ConversationRename(_) => ProductMethod::ConversationRename,
            ProductParams::ConversationDecisions(_) => ProductMethod::ConversationDecisions,
            ProductParams::ConversationDecisionSettings(_) => {
                ProductMethod::ConversationDecisionSettings
            }
            ProductParams::ConversationPauseAutoPick(_) => ProductMethod::ConversationPauseAutoPick,
            ProductParams::ConversationSetDecisionSettings(_) => {
                ProductMethod::ConversationSetDecisionSettings
            }
            ProductParams::ConversationDelete(_) => ProductMethod::ConversationDelete,
            ProductParams::ConversationReorder(_) => ProductMethod::ConversationReorder,
            ProductParams::ArtifactRead(_) => ProductMethod::ArtifactRead,
            ProductParams::AttachmentBegin(_) => ProductMethod::AttachmentBegin,
            ProductParams::AttachmentWrite(_) => ProductMethod::AttachmentWrite,
            ProductParams::AgentsList => ProductMethod::AgentsList,
            ProductParams::AgentsSetNativeHooks(_) => ProductMethod::AgentsSetNativeHooks,
            ProductParams::AgentsSubscribe => ProductMethod::AgentsSubscribe,
            ProductParams::AgentsUnsubscribe(_) => ProductMethod::AgentsUnsubscribe,
            ProductParams::ConversationSetApprovalMode(_) => {
                ProductMethod::ConversationSetApprovalMode
            }
            ProductParams::ConversationSetUsageLimitAutoContinue(_) => {
                ProductMethod::ConversationSetUsageLimitAutoContinue
            }
            ProductParams::AgentsInstallOptions => ProductMethod::AgentsInstallOptions,
            ProductParams::AgentsInstall(_) => ProductMethod::AgentsInstall,
            ProductParams::ConversationCatalogueHead(_) => ProductMethod::ConversationCatalogueHead,
            ProductParams::ConversationCatalogueManifest(_) => {
                ProductMethod::ConversationCatalogueManifest
            }
            ProductParams::ConversationCatalogueResolve(_) => {
                ProductMethod::ConversationCatalogueResolve
            }
            ProductParams::McpCallTool(_) => ProductMethod::McpCallTool,
            ProductParams::McpReadResource(_) => ProductMethod::McpReadResource,
            ProductParams::McpReleaseApp(_) => ProductMethod::McpReleaseApp,
            ProductParams::McpSendMessage(_) => ProductMethod::McpSendMessage,
            ProductParams::McpUpdateModelContext(_) => ProductMethod::McpUpdateModelContext,
            ProductParams::McpServersList => ProductMethod::McpServersList,
            ProductParams::McpServersSave(_) => ProductMethod::McpServersSave,
            ProductParams::McpServersRemove(_) => ProductMethod::McpServersRemove,
            ProductParams::McpServersInspect(_) => ProductMethod::McpServersInspect,
            ProductParams::McpServersAuthorize(_) => ProductMethod::McpServersAuthorize,
            ProductParams::McpServersRevoke(_) => ProductMethod::McpServersRevoke,
            ProductParams::AppsTrusted => ProductMethod::AppsTrusted,
            ProductParams::AppsUntrust(_) => ProductMethod::AppsUntrust,
            ProductParams::AppsStarter(_) => ProductMethod::AppsStarter,
            ProductParams::AppsReview(_) => ProductMethod::AppsReview,
            ProductParams::AppsInstall(_) => ProductMethod::AppsInstall,
            ProductParams::AppsInstalled => ProductMethod::AppsInstalled,
            ProductParams::AppsRead(_) => ProductMethod::AppsRead,
            ProductParams::AppsUninstall(_) => ProductMethod::AppsUninstall,
            ProductParams::AppsIssueCredential(_) => ProductMethod::AppsIssueCredential,
            ProductParams::AppsRevokeCredentials(_) => ProductMethod::AppsRevokeCredentials,
            ProductParams::PhoneEndpoint(_) => ProductMethod::PhoneEndpoint,
            ProductParams::PhoneAuthorize(_) => ProductMethod::PhoneAuthorize,
            ProductParams::PhoneStatus(_) => ProductMethod::PhoneStatus,
            ProductParams::PairingCreate(_) => ProductMethod::PairingCreate,
            ProductParams::PairingPending => ProductMethod::PairingPending,
            ProductParams::PairingStatus(_) => ProductMethod::PairingStatus,
            ProductParams::PairingApprove(_) => ProductMethod::PairingApprove,
            ProductParams::PairingDeny(_) => ProductMethod::PairingDeny,
            ProductParams::PairingCancel(_) => ProductMethod::PairingCancel,
            ProductParams::PairingSetLinking(_) => ProductMethod::PairingSetLinking,
            ProductParams::PairingSetTunnel(_) => ProductMethod::PairingSetTunnel,
            ProductParams::PairingTunnel => ProductMethod::PairingTunnel,
            ProductParams::GithubLiveEvents(_) => ProductMethod::GithubLiveEvents,
            ProductParams::GithubStatus => ProductMethod::GithubStatus,
            ProductParams::GithubConnect => ProductMethod::GithubConnect,
            ProductParams::DevicesList => ProductMethod::DevicesList,
            ProductParams::GithubCancel => ProductMethod::GithubCancel,
            ProductParams::GithubDisconnect => ProductMethod::GithubDisconnect,
            ProductParams::PeerEnroll(_) => ProductMethod::PeerEnroll,
            ProductParams::PeerList => ProductMethod::PeerList,
            ProductParams::PeerForget(_) => ProductMethod::PeerForget,
            ProductParams::PeerRelay(_) => ProductMethod::PeerRelay,
            ProductParams::PeerFollow(_) => ProductMethod::PeerFollow,
            ProductParams::GrantsSubscribe(_) => ProductMethod::GrantsSubscribe,
            ProductParams::GrantsPage(_) => ProductMethod::GrantsPage,
            ProductParams::GrantsUnsubscribe(_) => ProductMethod::GrantsUnsubscribe,
            ProductParams::ConversationSubscribe(_) => ProductMethod::ConversationSubscribe,
            ProductParams::ConversationSubscribeList(_) => ProductMethod::ConversationSubscribeList,
            ProductParams::ConversationUnsubscribe(_) => ProductMethod::ConversationUnsubscribe,
            ProductParams::ConversationShare(_) => ProductMethod::ConversationShare,
            ProductParams::ConversationUnshare(_) => ProductMethod::ConversationUnshare,
            ProductParams::ConversationShares(_) => ProductMethod::ConversationShares,
            ProductParams::ConversationPermissionRules(_) => {
                ProductMethod::ConversationPermissionRules
            }
            ProductParams::ConversationRevokePermissionRule(_) => {
                ProductMethod::ConversationRevokePermissionRule
            }
            ProductParams::WorkspacePublish(_) => ProductMethod::WorkspacePublish,
            ProductParams::WorkspaceAnswer(_) => ProductMethod::WorkspaceAnswer,
            ProductParams::WorkspaceArrange(_) => ProductMethod::WorkspaceArrange,
            ProductParams::WorkspaceClosePane(_) => ProductMethod::WorkspaceClosePane,
            ProductParams::ChannelCreate(_) => ProductMethod::ChannelCreate,
            ProductParams::ChannelList(_) => ProductMethod::ChannelList,
            ProductParams::ChannelSetCoordinator(_) => ProductMethod::ChannelSetCoordinator,
            ProductParams::ReplyStyleRead(_) => ProductMethod::ReplyStyleRead,
            ProductParams::ReplyStyleSet(_) => ProductMethod::ReplyStyleSet,
            ProductParams::ConversationSubscribeEvents(_) => {
                ProductMethod::ConversationSubscribeEvents
            }
            ProductParams::ConversationReadEarlier(_) => ProductMethod::ConversationReadEarlier,
            ProductParams::ConversationPeek(_) => ProductMethod::ConversationPeek,
            ProductParams::ConversationSetModel(_) => ProductMethod::ConversationSetModel,
            ProductParams::ConversationSwitchAgent(_) => ProductMethod::ConversationSwitchAgent,
            ProductParams::ConversationFork(_) => ProductMethod::ConversationFork,
            ProductParams::VaultStatus => ProductMethod::VaultStatus,
            ProductParams::VaultSetup(_) => ProductMethod::VaultSetup,
            ProductParams::VaultUnlock(_) => ProductMethod::VaultUnlock,
            ProductParams::VaultChangePassphrase(_) => ProductMethod::VaultChangePassphrase,
            ProductParams::VaultLock => ProductMethod::VaultLock,
            ProductParams::BoatStatus => ProductMethod::BoatStatus,
            ProductParams::BoatSave(_) => ProductMethod::BoatSave,
            ProductParams::BoatDisconnect => ProductMethod::BoatDisconnect,
            ProductParams::Agent37Status => ProductMethod::Agent37Status,
            ProductParams::Agent37Save(_) => ProductMethod::Agent37Save,
            ProductParams::Agent37Disconnect => ProductMethod::Agent37Disconnect,
            ProductParams::CloudRead => ProductMethod::CloudRead,
            ProductParams::CloudChoose(_) => ProductMethod::CloudChoose,
            ProductParams::CloudBoxesList => ProductMethod::CloudBoxesList,
            ProductParams::CloudBoxesCreate(_) => ProductMethod::CloudBoxesCreate,
            ProductParams::CloudBoxesWake(_) => ProductMethod::CloudBoxesWake,
            ProductParams::CloudBoxesSleep(_) => ProductMethod::CloudBoxesSleep,
            ProductParams::CloudBoxesRename(_) => ProductMethod::CloudBoxesRename,
            ProductParams::CloudBoxesDelete(_) => ProductMethod::CloudBoxesDelete,
            ProductParams::ConversationWebhookCursor(_) => ProductMethod::ConversationWebhookCursor,
            ProductParams::ConversationWebhookReset(_) => ProductMethod::ConversationWebhookReset,
            ProductParams::VoiceStatus => ProductMethod::VoiceStatus,
            ProductParams::VoiceSaveKey(_) => ProductMethod::VoiceSaveKey,
            ProductParams::VoiceForgetKey => ProductMethod::VoiceForgetKey,
            ProductParams::VoiceStartCall(_) => ProductMethod::VoiceStartCall,
            ProductParams::TerminalOpen(_) => ProductMethod::TerminalOpen,
            ProductParams::TerminalList => ProductMethod::TerminalList,
            ProductParams::TerminalAttach(_) => ProductMethod::TerminalAttach,
            ProductParams::TerminalFocus(_) => ProductMethod::TerminalFocus,
            ProductParams::TerminalFit(_) => ProductMethod::TerminalFit,
            ProductParams::TerminalDetach(_) => ProductMethod::TerminalDetach,
            ProductParams::TerminalInput(_) => ProductMethod::TerminalInput,
            ProductParams::TerminalHistory(_) => ProductMethod::TerminalHistory,
            ProductParams::TerminalClose(_) => ProductMethod::TerminalClose,
            ProductParams::TerminalRename(_) => ProductMethod::TerminalRename,
            ProductParams::TerminalResumeSettings => ProductMethod::TerminalResumeSettings,
            ProductParams::TerminalSetResumeSettings(_) => ProductMethod::TerminalSetResumeSettings,
            ProductParams::TerminalForgetSaved(_) => ProductMethod::TerminalForgetSaved,
        }
    }
}
pub const PRODUCT_VERSION: u64 = 1;
pub const PRODUCT_SESSION_PATH: &str = "/session";
