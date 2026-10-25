//! Pure product outcome values generated from protocol/product/v1.json. Do not edit.
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum SessionCloseReason {
    #[serde(rename = "authentication_failed")]
    AuthenticationFailed,
    #[serde(rename = "credential_revoked")]
    CredentialRevoked,
    #[serde(rename = "credential_expired")]
    CredentialExpired,
    #[serde(rename = "authorization_lost")]
    AuthorizationLost,
    #[serde(rename = "protocol_incompatible")]
    ProtocolIncompatible,
    #[serde(rename = "handshake_timeout")]
    HandshakeTimeout,
    #[serde(rename = "temporary_unavailable")]
    TemporaryUnavailable,
    #[serde(rename = "gateway_restarting")]
    GatewayRestarting,
    #[serde(rename = "gateway_overloaded")]
    GatewayOverloaded,
    #[serde(rename = "server_shutdown")]
    ServerShutdown,
    #[serde(rename = "transport_interrupted")]
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
pub enum ConversationErrorCode {
    #[serde(rename = "agent_not_configured")]
    AgentNotConfigured,
    #[serde(rename = "agent_unsupported")]
    AgentUnsupported,
    #[serde(rename = "sandbox_unavailable")]
    SandboxUnavailable,
    #[serde(rename = "environment_not_configured")]
    EnvironmentNotConfigured,
    #[serde(rename = "environment_unavailable")]
    EnvironmentUnavailable,
    #[serde(rename = "environment_version_mismatch")]
    EnvironmentVersionMismatch,
    #[serde(rename = "environment_platform_unsupported")]
    EnvironmentPlatformUnsupported,
    #[serde(rename = "environment_install_failed")]
    EnvironmentInstallFailed,
    #[serde(rename = "linked_file_unreachable")]
    LinkedFileUnreachable,
    #[serde(rename = "model_unavailable")]
    ModelUnavailable,
    #[serde(rename = "approval_mode_unavailable")]
    ApprovalModeUnavailable,
    #[serde(rename = "approval_mode_not_applied")]
    ApprovalModeNotApplied,
    #[serde(rename = "approval_mode_uncertain")]
    ApprovalModeUncertain,
    #[serde(rename = "model_change_not_applied")]
    ModelChangeNotApplied,
    #[serde(rename = "model_change_uncertain")]
    ModelChangeUncertain,
    #[serde(rename = "approval_request_conflict")]
    ApprovalRequestConflict,
    #[serde(rename = "turn_running")]
    TurnRunning,
    #[serde(rename = "conversations_not_configured")]
    ConversationsNotConfigured,
    #[serde(rename = "unknown_method")]
    UnknownMethod,
    #[serde(rename = "invalid_request")]
    InvalidRequest,
    #[serde(rename = "conversation_not_found")]
    ConversationNotFound,
    #[serde(rename = "conversation_capacity")]
    ConversationCapacity,
    #[serde(rename = "conversation_closed")]
    ConversationClosed,
    #[serde(rename = "conversation_configuration_changed")]
    ConversationConfigurationChanged,
    #[serde(rename = "conversation_state_unreadable")]
    ConversationStateUnreadable,
    #[serde(rename = "conversation_storage_unavailable")]
    ConversationStorageUnavailable,
    #[serde(rename = "temporarily_unavailable")]
    TemporarilyUnavailable,
    #[serde(rename = "audit_unavailable")]
    AuditUnavailable,
    #[serde(rename = "submission_conflict")]
    SubmissionConflict,
    #[serde(rename = "submission_unresolved")]
    SubmissionUnresolved,
    #[serde(rename = "stale_permission")]
    StalePermission,
    #[serde(rename = "agent_startup_deadline")]
    AgentStartupDeadline,
    #[serde(rename = "agent_operation_failed")]
    AgentOperationFailed,
    #[serde(rename = "image_input_unsupported")]
    ImageInputUnsupported,
    #[serde(rename = "attachment_not_found")]
    AttachmentNotFound,
    #[serde(rename = "attachment_unavailable")]
    AttachmentUnavailable,
    #[serde(rename = "attachment_capacity")]
    AttachmentCapacity,
    #[serde(rename = "attachment_storage_unavailable")]
    AttachmentStorageUnavailable,
    #[serde(rename = "attachment_cleanup_unavailable")]
    AttachmentCleanupUnavailable,
    #[serde(rename = "conversation_deleted")]
    ConversationDeleted,
    #[serde(rename = "conversation_erasure_incomplete")]
    ConversationErasureIncomplete,
    #[serde(rename = "mcp_app_unknown")]
    McpAppUnknown,
    #[serde(rename = "mcp_server_mismatch")]
    McpServerMismatch,
    #[serde(rename = "mcp_tool_not_for_app")]
    McpToolNotForApp,
    #[serde(rename = "mcp_session_unavailable")]
    McpSessionUnavailable,
    #[serde(rename = "mcp_approval_denied")]
    McpApprovalDenied,
    #[serde(rename = "mcp_approval_expired")]
    McpApprovalExpired,
    #[serde(rename = "mcp_cancelled")]
    McpCancelled,
    #[serde(rename = "mcp_request_too_large")]
    McpRequestTooLarge,
    #[serde(rename = "mcp_result_too_large")]
    McpResultTooLarge,
    #[serde(rename = "mcp_timed_out")]
    McpTimedOut,
    #[serde(rename = "mcp_remote_error")]
    McpRemoteError,
    #[serde(rename = "mcp_unauthorized")]
    McpUnauthorized,
    #[serde(rename = "mcp_unreachable")]
    McpUnreachable,
    #[serde(rename = "mcp_insufficient_scope")]
    McpInsufficientScope,
    #[serde(rename = "capabilities_refused")]
    CapabilitiesRefused,
    #[serde(rename = "share_target_not_paired")]
    ShareTargetNotPaired,
    #[serde(rename = "share_grant_changed")]
    ShareGrantChanged,
    #[serde(rename = "idempotency_conflict")]
    IdempotencyConflict,
    #[serde(rename = "message_quota_exceeded")]
    MessageQuotaExceeded,
    #[serde(rename = "drive_policy_unenforceable")]
    DrivePolicyUnenforceable,
    #[serde(rename = "revoked")]
    Revoked,
    #[serde(rename = "revoked_after_save")]
    RevokedAfterSave,
    #[serde(rename = "unavailable_after_save")]
    UnavailableAfterSave,
    #[serde(rename = "channel_not_found")]
    ChannelNotFound,
}
impl ConversationErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AgentNotConfigured => "agent_not_configured",
            Self::AgentUnsupported => "agent_unsupported",
            Self::SandboxUnavailable => "sandbox_unavailable",
            Self::EnvironmentNotConfigured => "environment_not_configured",
            Self::EnvironmentUnavailable => "environment_unavailable",
            Self::EnvironmentVersionMismatch => "environment_version_mismatch",
            Self::EnvironmentPlatformUnsupported => "environment_platform_unsupported",
            Self::EnvironmentInstallFailed => "environment_install_failed",
            Self::LinkedFileUnreachable => "linked_file_unreachable",
            Self::ModelUnavailable => "model_unavailable",
            Self::ApprovalModeUnavailable => "approval_mode_unavailable",
            Self::ApprovalModeNotApplied => "approval_mode_not_applied",
            Self::ApprovalModeUncertain => "approval_mode_uncertain",
            Self::ModelChangeNotApplied => "model_change_not_applied",
            Self::ModelChangeUncertain => "model_change_uncertain",
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
            Self::McpAppUnknown => "mcp_app_unknown",
            Self::McpServerMismatch => "mcp_server_mismatch",
            Self::McpToolNotForApp => "mcp_tool_not_for_app",
            Self::McpSessionUnavailable => "mcp_session_unavailable",
            Self::McpApprovalDenied => "mcp_approval_denied",
            Self::McpApprovalExpired => "mcp_approval_expired",
            Self::McpCancelled => "mcp_cancelled",
            Self::McpRequestTooLarge => "mcp_request_too_large",
            Self::McpResultTooLarge => "mcp_result_too_large",
            Self::McpTimedOut => "mcp_timed_out",
            Self::McpRemoteError => "mcp_remote_error",
            Self::McpUnauthorized => "mcp_unauthorized",
            Self::McpUnreachable => "mcp_unreachable",
            Self::McpInsufficientScope => "mcp_insufficient_scope",
            Self::CapabilitiesRefused => "capabilities_refused",
            Self::ShareTargetNotPaired => "share_target_not_paired",
            Self::ShareGrantChanged => "share_grant_changed",
            Self::IdempotencyConflict => "idempotency_conflict",
            Self::MessageQuotaExceeded => "message_quota_exceeded",
            Self::DrivePolicyUnenforceable => "drive_policy_unenforceable",
            Self::Revoked => "revoked",
            Self::RevokedAfterSave => "revoked_after_save",
            Self::UnavailableAfterSave => "unavailable_after_save",
            Self::ChannelNotFound => "channel_not_found",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum RecordReadErrorCode {
    #[serde(rename = "invalid_request")]
    InvalidRequest,
    #[serde(rename = "unauthorized")]
    Unauthorized,
    #[serde(rename = "forbidden")]
    Forbidden,
    #[serde(rename = "wrong_owner")]
    WrongOwner,
    #[serde(rename = "wrong_receiver")]
    WrongReceiver,
    #[serde(rename = "stale_epoch")]
    StaleEpoch,
    #[serde(rename = "unverifiable")]
    Unverifiable,
    #[serde(rename = "identity_changed")]
    IdentityChanged,
    #[serde(rename = "history_pruned")]
    HistoryPruned,
    #[serde(rename = "record_too_large")]
    RecordTooLarge,
    #[serde(rename = "response_too_large")]
    ResponseTooLarge,
    #[serde(rename = "temporarily_unavailable")]
    TemporarilyUnavailable,
    #[serde(rename = "read_timeout")]
    ReadTimeout,
    #[serde(rename = "source_preparing")]
    SourcePreparing,
}
impl RecordReadErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::Unauthorized => "unauthorized",
            Self::Forbidden => "forbidden",
            Self::WrongOwner => "wrong_owner",
            Self::WrongReceiver => "wrong_receiver",
            Self::StaleEpoch => "stale_epoch",
            Self::Unverifiable => "unverifiable",
            Self::IdentityChanged => "identity_changed",
            Self::HistoryPruned => "history_pruned",
            Self::RecordTooLarge => "record_too_large",
            Self::ResponseTooLarge => "response_too_large",
            Self::TemporarilyUnavailable => "temporarily_unavailable",
            Self::ReadTimeout => "read_timeout",
            Self::SourcePreparing => "source_preparing",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum CatalogueReadErrorCode {
    #[serde(rename = "invalid_request")]
    InvalidRequest,
    #[serde(rename = "unauthorized")]
    Unauthorized,
    #[serde(rename = "forbidden")]
    Forbidden,
    #[serde(rename = "wrong_owner")]
    WrongOwner,
    #[serde(rename = "wrong_receiver")]
    WrongReceiver,
    #[serde(rename = "stale_epoch")]
    StaleEpoch,
    #[serde(rename = "unverifiable")]
    Unverifiable,
    #[serde(rename = "identity_changed")]
    IdentityChanged,
    #[serde(rename = "source_unavailable")]
    SourceUnavailable,
    #[serde(rename = "oversized_entry")]
    OversizedEntry,
    #[serde(rename = "read_timeout")]
    ReadTimeout,
    #[serde(rename = "response_too_large")]
    ResponseTooLarge,
    #[serde(rename = "server_busy")]
    ServerBusy,
}
impl CatalogueReadErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::Unauthorized => "unauthorized",
            Self::Forbidden => "forbidden",
            Self::WrongOwner => "wrong_owner",
            Self::WrongReceiver => "wrong_receiver",
            Self::StaleEpoch => "stale_epoch",
            Self::Unverifiable => "unverifiable",
            Self::IdentityChanged => "identity_changed",
            Self::SourceUnavailable => "source_unavailable",
            Self::OversizedEntry => "oversized_entry",
            Self::ReadTimeout => "read_timeout",
            Self::ResponseTooLarge => "response_too_large",
            Self::ServerBusy => "server_busy",
        }
    }
}
/// Published bound from the product schema.
pub const MAX_MCP_MESSAGE_BYTES: usize = 8192;
/// Published MCP App call timing from the product schema, in milliseconds.
pub const MCP_APP_REVIEW_DEADLINE_MS: u64 = 300000;
/// Published MCP App call timing from the product schema, in milliseconds.
pub const MCP_APP_CALL_TIMEOUT_MS: u64 = 60000;
/// Published MCP App call timing from the product schema, in milliseconds.
pub const MCP_APP_READ_TIMEOUT_MS: u64 = 10000;
/// Published mcpServers.inspect policy from the product schema, in milliseconds.
pub const MCP_SERVER_INSPECT_DEADLINE_MS: u64 = 30000;
/// Published mcpServers.inspect policy from the product schema.
pub const MCP_SERVER_INSPECT_MAX_TOOL_PAGES: usize = 8;
/// Published mcpServers.inspect policy from the product schema.
pub const MCP_SERVER_INSPECT_MAX_UI_READS: usize = 32;
/// Published mcpServers.inspect policy from the product schema.
pub const MCP_SERVER_INSPECT_MAX_CONCURRENT: usize = 2;
/// Published lifetime of an MCP App's resource ticket from the product schema, in milliseconds.
pub const MCP_RESOURCE_TICKET_MS: u64 = 60000;
