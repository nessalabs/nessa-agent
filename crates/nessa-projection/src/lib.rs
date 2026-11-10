//! The one bounded fold of a conversation's committed SDK records into the
//! view the gateway answers `conversation.read` with and a device shows
//! offline.
//!
//! ```text
//! projection --> records --> nessa-protocol (view shapes)
//!            --> tool_uis (port: a tool's declared UI)
//!            --> nessa-sdk (committed records)
//! ```
//! Arrows are compile-time dependencies.
//!
//! The view's shapes are the protocol's, so a client can read a view without
//! the SDK. Reading SDK records into them is here, so the protocol does not
//! depend on the SDK.
pub mod projection;
pub mod records;
pub mod tool_uis;
mod view_of;

pub use projection::*;
pub use tool_uis::{McpToolUis, NoMcpToolUis};
pub use view_of::{IntoView, ViewOf};
