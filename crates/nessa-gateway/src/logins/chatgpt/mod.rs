//! ChatGPT accounts Codex uses on other machines (issue #1403): signed in
//! once on the gateway's computer through OpenAI's token sharing for
//! open-source apps, kept in the vault, refreshed only here, and handed to a
//! lease as a short-lived access token.
//!
//! ```text
//! logins.chatgptSignIn ──▶ ChatgptLogins::begin ──▶ CallbackListener (127.0.0.1)
//!   browser ──▶ OpenAI ──▶ callback ──▶ ChatgptAuthority::exchange, identity
//!   ──▶ SecretStore (vault entry login.codex.chatgpt) ──▶ LoginAudit
//! a lease ──▶ SavedLoginsService::hand ──▶ ChatgptLogins::access
//!   ──▶ ChatgptAuthority::refresh when under 50 minutes are left ──▶ saved first
//! ```
//!
//! Arrows are calls. The design, with its orderings, is
//! `docs/design/chatgpt-sign-in.md`.
pub mod application;
pub mod domain;
pub mod infrastructure;
pub mod ports;
pub mod vault_entry;

#[cfg(test)]
#[path = "../../../tests/logins/chatgpt.rs"]
mod tests;
