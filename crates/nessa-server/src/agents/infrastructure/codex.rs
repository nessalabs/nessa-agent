//! How Codex itself reports being signed in.
//!
//! Codex keeps its sign-in in places OpenAI chose, and in more than one of
//! them: an API key in the environment, a credentials file under `CODEX_HOME`,
//! and — where `cli_auth_credentials_store` says so — the operating system's own
//! credential store. So the file is not the whole answer, and an absent file is
//! not a signed-out machine.
//!
//! Which store is in use is Codex's own decision, made from its own
//! configuration, and reimplementing that decision here would be a copy of it
//! that goes stale. Codex is asked instead, through the same launch this server
//! would start it with.
//!
//! `local.rs` orchestrates: it decides the order sources are asked in and what
//! an unanswered source means. What makes a variable a credential and a file a
//! sign-in is the same for every agent and lives in `credentials.rs`. What is
//! true of Codex specifically is here.
//!
//! Nothing here reads a secret. Every question is whether a credential exists.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::agents::application::ProbeFailure;
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

/// What the adapter's own command-line passthrough is asked.
///
/// `cli` hands the rest to the Codex executable the adapter would start for a
/// session — the one `CODEX_PATH` names, or the one bundled beside the adapter —
/// so this question is answered by exactly the Codex that would run, with
/// exactly the configuration it would read. Asking a `codex` found on `PATH`
/// instead would answer about a different installation than the one Nessa
/// launches, which is the mistake this exists to avoid.
const SIGN_IN_QUERY: [&str; 3] = ["cli", "login", "status"];

/// Codex's own answer that nothing is signed in. Anything else non-zero is the
/// tool failing rather than reporting, and is not read as a no.
const NOT_SIGNED_IN: i32 = 1;

/// How long Codex gets to answer before this probe stops waiting for it.
///
/// A local answer takes a quarter of a second; five is not a budget any real
/// answer needs, it is the point past which waiting is no longer producing one.
/// The wait is bounded at all because the credential store behind this question
/// can be a locked keychain, and a locked keychain does not fail quickly — it
/// can sit on an unlock prompt nobody is looking at, and an onboarding request
/// must not sit there with it.
const SIGN_IN_DEADLINE: Duration = Duration::from_secs(5);

/// Whether Codex itself says this machine is signed in to it.
///
/// Asked of Codex rather than of the filesystem, because the filesystem holds
/// only one of the places a Codex login can be: with
/// `cli_auth_credentials_store` set to `keyring`, a perfectly good login leaves
/// no `auth.json` at all, and a probe that read only the file would tell that
/// person to sign in to an account they are already signed in to.
///
/// `command` and `entry` are the launch this server is configured to start
/// Codex with, so the answer is about the installation Nessa would really use.
///
/// Output is discarded and only the exit status read: zero is signed in,
/// [`NOT_SIGNED_IN`] is Codex saying no, and every other ending — a tool that
/// could not start, one that failed, one that never finished and was killed —
/// is reported as a question this machine did not answer rather than as a
/// sign-in ruled out. Nothing here handles a credential; the answer is a status
/// code.
pub(super) fn sign_in_status(command: &Path, entry: &Path) -> Result<bool, ProbeFailure> {
    let mut child = match Command::new(command)
        .arg(entry)
        .args(SIGN_IN_QUERY)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Err(ProbeFailure::NothingToAsk)
        }
        Err(_) => return Err(ProbeFailure::Unanswered),
    };
    match credentials::wait_or_kill(&mut child, SIGN_IN_DEADLINE) {
        Some(status) if status.success() => Ok(true),
        Some(status) if status.code() == Some(NOT_SIGNED_IN) => Ok(false),
        // A tool that failed, and a tool that never finished and was killed.
        Some(_) | None => Err(ProbeFailure::Unanswered),
    }
}

#[cfg(test)]
#[path = "../../../tests/agents/codex.rs"]
mod tests;
