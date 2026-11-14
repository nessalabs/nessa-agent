//! Auth token issuance, product calls, and diagnostics through an injected gateway.
//!
//! ```text
//! call::invoke ──▶ surface::prepare ──▶ Gateway::call
//! mcp serve ────────┘
//! ```
//! Arrows are calls. `prepare` checks the method against the manifest catalog
//! and its schema. It does not decide permission: `Gateway::call` reaches the
//! gateway, and Cedar there is the only authorization.
mod call;
mod commands;
mod surface;
pub use call::{invoke, refusal_body, CallFailure};
pub use commands::{
    issue_token, BrowserToken, Gateway, GatewayCallError, GatewayIdentity, TokenRequest,
};
pub use surface::{prepare, tools, Prepared};
mod error;
pub use error::CliError;
