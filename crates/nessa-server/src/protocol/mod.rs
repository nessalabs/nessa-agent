//! Shared typed payloads, shortcuts, and wire frames generated from protocol schemas.
mod defaults;
mod encode;
mod frames;
mod generated_catalog;
mod generated_types;
pub use defaults::default_shortcuts;
pub use encode::{echo_message, error_message, health_check_message, MAX_PAYLOAD_BYTES};
pub use frames::{EventFrame, OutgoingMessage, RequestFrame, ResponseFrame};
pub use generated_catalog::{event as wire_event, method as wire_method};
pub use generated_types::{
    ClientInfo, ClientPlatform, ClientRole, EchoParams, EchoResult, GatewayError, HealthParams,
    HealthResult, RuntimeStatus, Scope, ShortcutAction, ShortcutArgs, ShortcutBinding,
    ShortcutScope, ShortcutSurface, ShortcutsDocument, SurfaceInfo, SurfaceKind,
};
