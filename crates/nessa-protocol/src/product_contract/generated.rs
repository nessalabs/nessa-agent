//! Pure product outcome values generated from protocol/product/v1.json. Do not edit.
use serde::{Deserialize, Serialize};
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
pub enum ConversationErrorCode {
    AgentNotConfigured,
    AgentUnsupported,
    SandboxUnavailable,
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
    McpAppUnknown,
    McpServerMismatch,
    McpToolNotForApp,
    McpSessionUnavailable,
    McpApprovalDenied,
    McpApprovalExpired,
    McpCancelled,
    McpRequestTooLarge,
    McpResultTooLarge,
    McpTimedOut,
    McpRemoteError,
    McpUnauthorized,
    McpUnreachable,
    McpInsufficientScope,
    ShareTargetNotPaired,
}
impl ConversationErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AgentNotConfigured => "agent_not_configured",
            Self::AgentUnsupported => "agent_unsupported",
            Self::SandboxUnavailable => "sandbox_unavailable",
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
            Self::ShareTargetNotPaired => "share_target_not_paired",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordReadErrorCode {
    InvalidRequest,
    Unauthorized,
    Forbidden,
    WrongOwner,
    WrongReceiver,
    StaleEpoch,
    Unverifiable,
    IdentityChanged,
    HistoryPruned,
    RecordTooLarge,
    ResponseTooLarge,
    TemporarilyUnavailable,
    ReadTimeout,
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
#[serde(rename_all = "snake_case")]
pub enum CatalogueReadErrorCode {
    InvalidRequest,
    Unauthorized,
    Forbidden,
    WrongOwner,
    WrongReceiver,
    StaleEpoch,
    Unverifiable,
    IdentityChanged,
    SourceUnavailable,
    OversizedEntry,
    ReadTimeout,
    ResponseTooLarge,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeWatchEndReason {
    Closed,
    NotificationFailed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeWatchErrorCode {
    InvalidRequest,
    Unauthorized,
    Forbidden,
    WrongOwner,
    WrongReceiver,
    StaleEpoch,
    Unverifiable,
    TemporarilyUnavailable,
    WatchDuplicate,
    WatchCapacity,
    WatchClosed,
    InvalidWatch,
}
impl ChangeWatchErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::Unauthorized => "unauthorized",
            Self::Forbidden => "forbidden",
            Self::WrongOwner => "wrong_owner",
            Self::WrongReceiver => "wrong_receiver",
            Self::StaleEpoch => "stale_epoch",
            Self::Unverifiable => "unverifiable",
            Self::TemporarilyUnavailable => "temporarily_unavailable",
            Self::WatchDuplicate => "watch_duplicate",
            Self::WatchCapacity => "watch_capacity",
            Self::WatchClosed => "watch_closed",
            Self::InvalidWatch => "invalid_watch",
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
