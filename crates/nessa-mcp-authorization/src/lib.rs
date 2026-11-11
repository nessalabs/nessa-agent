//! One remote MCP server's consent, token generation, refresh and revoke
//! (ADR 392).
//!
//! ```text
//! mcpServers.authorize/revoke ──▶ AuthorizationOwner ──step──▶ domain::ServerAuth
//!                              ├──▶ AuthorizationRecords (non-secret file, private secret)
//!                              ├──▶ AuthorizationAudit (intent before a token effect)
//!                              ├──▶ OAuthHttp (HTTPS metadata, token, revoke)
//!                              ├──▶ LoopbackCallback (127.0.0.1, code never echoed)
//!                              └──bearer──▶ TransportAuthorization ──▶ HttpSession
//!
//! McpServerSettings::edit ──fence──▶ AuthorizationOwner
//!         before the new URL is in the live set
//! ```
//!
//! Arrows are calls. The domain chooses a transition and names the effects.
//! It does not open a socket, open the private store, or read the clock. The owner
//! performs one effect at a time and brings the result back as the next
//! command. A token is never written to a log or to `config.json`.
// The one clock rule nessa-server holds (clippy.toml), kept here since this
// code moved out of it.
#![cfg_attr(not(test), warn(clippy::disallowed_methods))]
pub mod application;
pub mod domain;
pub mod infrastructure;

#[cfg(test)]
#[path = "../tests/mod.rs"]
mod tests;
