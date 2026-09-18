//! How Codex itself reports being signed in.
//!
//! Codex keeps its sign-in in places OpenAI chose: an API key in the
//! environment, and a credentials file under `CODEX_HOME`. It has no keychain
//! item — a ChatGPT login is written into that same file — so this machine is
//! asked two questions about Codex rather than three, and the third is not
//! reported as one that could not be asked.
//!
//! `local.rs` orchestrates: it decides the order sources are asked in and what
//! an unanswered source means. What makes a variable a credential and a file a
//! sign-in is the same for every agent and lives in `credentials.rs`. What is
//! true of Codex specifically is here.
//!
//! Nothing here reads a secret. Every question is whether a credential exists.

use std::path::PathBuf;

use crate::agents::infrastructure::credentials;

/// What Codex names its credentials file inside its own directory. It holds an
/// API key or the tokens from a ChatGPT login, and either one starts Codex.
const CREDENTIALS_FILE: &str = "auth.json";

/// The environment variables the launcher passes through to the agent as a
/// sign-in, in the order Codex itself prefers them.
const CREDENTIAL_VARIABLES: [&str; 2] = ["CODEX_API_KEY", "OPENAI_API_KEY"];

/// A non-empty credential in this process's environment, which is what a
/// machine account signs in with. Returned only to know that it is there.
pub(super) fn environment_credential() -> Option<String> {
    credentials::environment_credential(&CREDENTIAL_VARIABLES)
}

/// Where Codex would write its credentials file on this machine.
///
/// `CODEX_HOME` when it is set, because that is what Codex obeys; otherwise
/// `~/.codex`, its default. A host with neither leaves nowhere to look, which
/// the caller reports as a question it could not ask.
pub(super) fn credentials_path() -> Option<PathBuf> {
    credentials::config_directory("CODEX_HOME", ".codex").map(|home| home.join(CREDENTIALS_FILE))
}

#[cfg(test)]
#[path = "../../../tests/agents/codex.rs"]
mod tests;
