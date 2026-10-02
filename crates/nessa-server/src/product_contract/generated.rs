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
    pub(crate) fn from_web_socket_code(code: u16) -> Option<Self> {
        match code {
            4001 => Some(Self::AuthenticationFailed),
            4002 => Some(Self::CredentialRevoked),
            4003 => Some(Self::CredentialExpired),
            4004 => Some(Self::AuthorizationLost),
            4005 => Some(Self::ProtocolIncompatible),
            4006 => Some(Self::HandshakeTimeout),
            4010 => Some(Self::TemporaryUnavailable),
            1012 => Some(Self::GatewayRestarting),
            1013 => Some(Self::GatewayOverloaded),
            1000 => Some(Self::ServerShutdown),
            1006 => Some(Self::TransportInterrupted),
            _ => None,
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
/// Published MCP App call timing from the product schema, in milliseconds.
pub const MCP_APP_REVIEW_MS: u64 = 300000;
/// Published MCP App call timing from the product schema, in milliseconds.
pub const MCP_APP_CALL_MS: u64 = 60000;
/// Published lifetime of an MCP App's resource ticket from the product schema, in milliseconds.
pub const MCP_RESOURCE_TICKET_MS: u64 = 60000;
