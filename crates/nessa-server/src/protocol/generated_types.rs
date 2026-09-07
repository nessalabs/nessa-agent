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
pub struct ClientInfo {
    /// Client application or instance identifier.
    pub id: String,
    /// Client software version.
    pub version: String,
    /// Runtime or host platform. Use other when none applies.
    pub platform: ClientPlatform,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum ClientPlatform {
    #[serde(rename = "node")]
    Node,
    #[serde(rename = "browser")]
    Browser,
    #[serde(rename = "macos")]
    Macos,
    #[serde(rename = "linux")]
    Linux,
    #[serde(rename = "windows")]
    Windows,
    #[serde(rename = "other")]
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum ClientRole {
    #[serde(rename = "surface")]
    Surface,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EchoParams {
    /// User draft text to echo back.
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EchoResult {
    /// Echo of EchoParams.text.
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GatewayError {
    /// Machine-readable error identifier.
    pub code: String,
    /// Server-provided explanation of the rejection.
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Optional structured context; its shape depends on the error code.
    pub details: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
pub struct HealthParams {}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HealthResult {
    /// Whether the health probe succeeded.
    pub ok: bool,
    /// Current runtime availability reported by the gateway.
    pub runtime_status: RuntimeStatus,
    /// Time since server startup in milliseconds.
    pub uptime_ms: i64,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum ShortcutAction {
    #[serde(rename = "panel.summon")]
    PanelSummon,
    #[serde(rename = "panel.newTab")]
    PanelNewTab,
    #[serde(rename = "panel.closeTab")]
    PanelCloseTab,
    #[serde(rename = "panel.activateTab")]
    PanelActivateTab,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShortcutArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Zero-based index among open tabs.
    pub index: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Preferred later: pin a binding to a conversation id when open.
    pub conversation_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShortcutBinding {
    /// Tauri-style accelerator, e.g. CmdOrCtrl+Shift+D.
    pub keys: String,
    /// Action requested when the binding fires.
    pub action: ShortcutAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Optional action-specific arguments.
    pub args: Option<ShortcutArgs>,
    /// Context in which the binding applies.
    pub scope: ShortcutScope,
    /// Surface on which the binding applies.
    pub surface: ShortcutSurface,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum ShortcutScope {
    #[serde(rename = "global")]
    Global,
    #[serde(rename = "focused")]
    Focused,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShortcutsDocument {
    /// Shortcut document format version.
    pub version: i64,
    /// Ordered keyboard bindings supplied by the server.
    pub bindings: Vec<ShortcutBinding>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum ShortcutSurface {
    #[serde(rename = "desktop")]
    Desktop,
    #[serde(rename = "browser")]
    Browser,
    #[serde(rename = "*")]
    Any,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SurfaceInfo {
    /// Kind of user-facing surface.
    pub kind: SurfaceKind,
    /// Identifier distinguishing instances of this surface.
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
