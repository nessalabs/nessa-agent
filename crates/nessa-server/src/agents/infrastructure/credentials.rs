//! What a credential in the environment and a credentials file on disk are,
//! for any agent.
//!
//! Each agent decides *which* variables sign it in and *where* it writes its
//! file. What makes a variable a credential and a file a sign-in is the same
//! question whichever agent is asked, so it is answered once, here.
//!
//! Nothing here reads a secret. Every question is whether a credential exists.

use std::io::ErrorKind;
use std::path::Path;

use crate::agents::application::ProbeFailure;

/// The most of a credentials file this probe will ever read.
///
/// These files hold one small JSON object. A path that leads to something far
/// larger is not a credentials file this probe can honestly judge, and reading
/// it would let whatever wrote it choose how much memory the server spends.
const MAX_CREDENTIALS_BYTES: u64 = 64 * 1024;

/// The first of `variables` set to something that is not blank.
///
/// Returned only so the caller knows one is there. Any one of them on its own
/// starts the agent, so any one of them on its own is an answered yes; anything
/// this probe did not check is a machine reported as needing a sign-in it
/// already has.
pub(super) fn environment_credential(variables: &[&str]) -> Option<String> {
    variables.iter().find_map(|key| {
        std::env::var(key)
            .ok()
            .filter(|value| !value.trim().is_empty())
    })
}

/// Whether the agent that owns `path` has written a usable credentials file there.
///
/// Nessa does not define these files' formats — each vendor's own CLI writes
/// them — and there is no published schema to check against. So this asserts
/// only what is true of such a file whatever its schema is, and guesses at no
/// field name: inventing a required key would reject genuinely valid credentials
/// the day that key is renamed, which is a worse failure than the one being
/// fixed here.
///
/// What can be said with certainty:
///
/// - The file must be small. Its size is read before its bytes, and anything
///   past [`MAX_CREDENTIALS_BYTES`] is refused unread — a source that did not
///   answer rather than a sign-in ruled out.
/// - It must parse as JSON. A truncated or corrupted write is a real no, for
///   the same reason an empty file always was: the agent could not sign in with
///   it either.
/// - It must be a JSON *object* with at least one key. A bare string, array,
///   number, `true`, `null`, or an `{}` left behind by a crashed process
///   carries no credential, so each is a real no.
///
/// Anything past that is the vendor's business. A file that clears these checks
/// is reported as a sign-in without ever being interpreted.
pub(super) fn credentials_file(path: &Path) -> Result<bool, ProbeFailure> {
    match path.metadata() {
        Ok(file) if file.len() == 0 => Ok(false),
        Ok(file) if file.len() > MAX_CREDENTIALS_BYTES => Err(ProbeFailure::Unanswered),
        Ok(_) => match std::fs::read_to_string(path) {
            Ok(contents) => Ok(holds_a_credential(&contents)),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
            Err(_) => Err(ProbeFailure::Unanswered),
        },
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(_) => Err(ProbeFailure::Unanswered),
    }
}

/// Whether these bytes are a JSON object with something in it.
///
/// Parsed into an untyped [`serde_json::Value`] on purpose: a typed struct
/// would be a claim about a schema this repository does not know.
fn holds_a_credential(contents: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(contents)
        .ok()
        .and_then(|value| value.as_object().map(|fields| !fields.is_empty()))
        .unwrap_or(false)
}

/// The directory an agent keeps its own state in.
///
/// `variable` when the agent obeys one, because that is what the agent itself
/// obeys; otherwise `default` beneath this user's home. A host with neither
/// leaves nowhere to look, which the caller reports as a question it could not
/// ask rather than as a sign-in ruled out.
pub(super) fn config_directory(variable: &str, default: &str) -> Option<std::path::PathBuf> {
    std::env::var(variable)
        .map(std::path::PathBuf::from)
        .ok()
        .or_else(|| {
            std::env::var(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                .ok()
                .map(|home| std::path::PathBuf::from(home).join(default))
        })
}

#[cfg(test)]
#[path = "../../../tests/agents/credentials.rs"]
mod tests;
