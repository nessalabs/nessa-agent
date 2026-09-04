//! Client API wire protocol — typed frames aligned with `protocol/` schemas.
//!
//! **SSOT:** `protocol/manifest.json` (names) + `protocol/schemas/v1/` (payloads).
//! Generated: `generated_catalog.rs`, `generated_types.rs` via `pnpm protocol:generate`.
//!
//! Hand-written here: frame encode/decode helpers and RPC routing. Handlers work
//! with generated structs, not raw `serde_json::Value`.
//!
//! ```text
//! WebSocket text
//!      │
//!      ▼
//! decode_client_request ──► ClientRequest
//!      │
//!      ▼
//! connect / health handlers
//!      │
//!      ▼
//! connect_success_message / health_check_message / …
//!      │
//!      ▼
//! OutgoingMessage::to_wire_text ──► WebSocket text
//! ```

mod decode;
mod encode;
mod frames;
mod generated_catalog;
mod generated_types;
pub mod defaults;

pub use decode::{decode_client_request, decode_error_response, ClientRequest, DecodeError};
pub use defaults::default_shortcuts;
pub use encode::{
    connect_challenge_message, connect_success_message, error_message, health_check_message,
    MAX_PAYLOAD_BYTES, PROTOCOL_VERSION,
};
pub use frames::{OutgoingMessage, ResponseFrame};
pub use generated_catalog::{event as wire_event, method as wire_method};
pub use generated_types::{
    AuthToken, ClientInfo, ClientRole, ConnectChallenge, ConnectParams, GatewayError, HealthParams,
    HealthResult, HelloOk, RuntimeStatus, Scope, ServerPolicy, ShortcutAction, ShortcutArgs,
    ShortcutBinding, ShortcutScope, ShortcutSurface, ShortcutsDocument, SurfaceInfo, SurfaceKind,
};
