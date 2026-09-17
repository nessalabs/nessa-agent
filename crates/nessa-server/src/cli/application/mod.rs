//! Auth token issuance and diagnostics through an injected gateway connection.
mod commands;
pub(crate) use commands::MAX_SAFE_TIMESTAMP;
pub use commands::{issue_token, BrowserToken, Gateway, GatewayIdentity, TokenRequest};
mod error;
pub use error::CliError;
