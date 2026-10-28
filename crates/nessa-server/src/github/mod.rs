//! The person's GitHub sign-in through the Nessa GitHub App, kept in the
//! gateway for source-authorized repository operations.
//!
//! ```text
//! github.status/connect/cancel/disconnect ──▶ GithubConnections
//!                  ├──▶ GithubApi (device code, token poll, /user)
//!                  ├──▶ GithubTokenStore (one private file, never config.json)
//!                  ├──▶ GithubAudit (requested, then outcome; no token)
//!                  └──▶ Timer (poll interval and the code's deadline)
//! agent read tool ──▶ GithubConnections ──▶ GithubApi (bounded repo GET)
//! ```
//!
//! Arrows are calls. Sign-in is the device flow: the gateway asks GitHub for
//! a code, the window shows it and opens GitHub, and the gateway polls until
//! the person approves, refuses, cancels or the code expires. The gateway
//! owns it rather than the window because the agents run where the gateway
//! runs, which a remote host makes a different machine. The states and the
//! orderings between them are in `docs/design/github-connect.md`.
pub mod application;
pub mod domain;
pub mod infrastructure;

#[cfg(test)]
#[path = "../../tests/github/mod.rs"]
mod tests;
