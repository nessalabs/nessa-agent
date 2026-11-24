//! Saved logins (issue #1260): the agents' sign-ins the gateway keeps in its
//! vault, and which machines in its Devices list may be handed them when an
//! agent starts there.
//!
//! ```text
//! logins.status/save/remove ──▶ SavedLoginsService ──▶ SecretStore (vault entry login.<agent>)
//! logins.setDevice          ──▶ SavedLoginsService ──▶ SecretStore (vault entry login.devices)
//!                                     └──▶ LoginAudit (requested, then outcome; never a login)
//! a lease granted takesLogin ──▶ SavedLoginsService::login_for(environment, agent, lease)
//!   ──▶ LoginPolicyEvaluator (Cedar: may this environment receive logins?)
//!   ──▶ the vault's saved login for the agent ──▶ LoginAudit (sent, or withheld and why)
//! ```
//!
//! Codex may use a ChatGPT account instead of its saved key
//! (`chatgpt`, issue #1403): the lease is then handed that account's access
//! token, which only `ChatgptLogins` refreshes.
//!
//! Arrows are calls. The vault is the one place a login is kept; a machine
//! holds the one it is sent only for the harness it starts with it. The
//! design is in `docs/design/saved-logins.md`.
pub mod application;
pub mod chatgpt;
pub mod domain;
pub mod infrastructure;

#[cfg(test)]
#[path = "../../tests/logins/mod.rs"]
mod tests;
