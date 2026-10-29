//! Auth token issuance and diagnostics through an injected gateway connection.
mod commands;
pub use commands::{issue_token, BrowserToken, Gateway, GatewayIdentity, TokenRequest};
mod error;
pub use error::CliError;
