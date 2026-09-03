//! Generated from `protocol/schemas/v1/*.json` — do not edit by hand.
//!
//! **Source of truth:** `protocol/schemas/v1/` (payload shapes).
//! Regenerate: `pnpm protocol:generate`
//!
//! Envelope helpers (`RequestFrame` encode/decode) stay hand-written in
//! `frames.rs`; this file is the payload/type catalog only.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthToken {
    pub token: String,
    /// Echo of connect.challenge nonce — binds this connect RPC to the open socket.
    pub nonce: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientInfo {
    pub id: String,
    pub version: String,
    pub platform: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum ClientRole {
    #[serde(rename = "surface")]
    Surface,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectChallenge {
    pub nonce: String,
    pub protocol: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectParams {
    pub min_protocol: i64,
    pub max_protocol: i64,
    pub role: ClientRole,
    pub surface: SurfaceInfo,
    pub client: ClientInfo,
    pub auth: AuthToken,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GatewayError {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
pub struct HealthParams {}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HealthResult {
    pub ok: bool,
    pub runtime_status: RuntimeStatus,
    pub uptime_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HelloOk {
    pub protocol: i64,
    pub scopes: Vec<Scope>,
    pub server_version: String,
    pub runtime_status: RuntimeStatus,
    pub policy: ServerPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum RuntimeStatus {
    #[serde(rename = "ready")]
    Ready,
    #[serde(rename = "starting")]
    Starting,
    #[serde(rename = "unavailable")]
    Unavailable,
    #[serde(rename = "error")]
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum Scope {
    #[serde(rename = "server.read")]
    ServerRead,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServerPolicy {
    pub max_payload_bytes: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SurfaceInfo {
    pub kind: SurfaceKind,
    pub instance: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum SurfaceKind {
    #[serde(rename = "panel")]
    Panel,
    #[serde(rename = "web")]
    Web,
    #[serde(rename = "desktop")]
    Desktop,
    #[serde(rename = "cli")]
    Cli,
}
