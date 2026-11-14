//! How Claude Code itself reports being signed in.
//!
//! Claude Code writes its sign-in in places Anthropic chose, not places Nessa
//! chose: a credentials file under `CLAUDE_CONFIG_DIR` and the login keychain
//! on macOS. That is
//! Claude's own knowledge, so it lives in one named place rather than spread
//! through the generic host adapter. `local.rs` orchestrates — it decides the
//! order sources are asked in and what an unanswered source means — and calls
//! in here for every fact that is true of Claude specifically. What makes a
//! file a sign-in lives in `credentials.rs`.
//!
//! Nothing here reads a secret. Every question is whether a credential exists.

#[cfg(target_os = "macos")]
use std::io::ErrorKind;
use std::path::PathBuf;
#[cfg(target_os = "macos")]
use std::process::{Command, Stdio};
#[cfg(target_os = "macos")]
use std::time::Duration;

use crate::agents::application::ProbeFailure;
use crate::agents::infrastructure::credentials;

/// Where Claude Code keeps its sign-in on macOS. Asked after, never read.
///
/// Gated with the one function that reads it: a host without a keychain has no
/// keychain to name.
#[cfg(target_os = "macos")]
const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";

/// What Claude Code names its credentials file inside its config directory.
const CREDENTIALS_FILE: &str = ".credentials.json";

/// Where Claude Code would write its credentials file on this machine.
///
/// `CLAUDE_CONFIG_DIR` when it is set, because that is what Claude Code obeys;
/// otherwise `~/.claude`, its default. A host with neither leaves nowhere to
/// look, which the caller reports as a question it could not ask.
pub(super) fn credentials_path() -> Option<PathBuf> {
    credentials::config_directory("CLAUDE_CONFIG_DIR", ".claude")
        .map(|directory| directory.join(CREDENTIALS_FILE))
}

/// `errSecItemNotFound`: the keychain looked and there is no such item. This is
/// the tool answering no, not the tool failing.
#[cfg(target_os = "macos")]
const KEYCHAIN_ITEM_NOT_FOUND: i32 = 44;

/// How long the keychain tool gets before this probe stops waiting for it.
///
/// A healthy keychain answers an attribute lookup in single-digit milliseconds,
/// so three seconds is not a budget any real answer needs — it is the point past
/// which waiting is no longer producing one. A locked keychain does not fail
/// quickly: `security` can sit there indefinitely, on a GUI unlock prompt nobody
/// is looking at, and an onboarding request must not sit there with it.
#[cfg(target_os = "macos")]
const KEYCHAIN_DEADLINE: Duration = Duration::from_secs(3);

/// Whether the login keychain holds Claude Code's sign-in.
///
/// Asked for the item's *attributes* and not its data: `find-generic-password`
/// without `-w` prints metadata and never the secret, so this answers "is there
/// a sign-in" without the server handling the token and without the keychain
/// prompting to release one.
///
/// Output is discarded and only the exit status read. A locked keychain, a
/// missing tool, or any other failure is reported as a failure rather than as
/// an absent sign-in, so that the caller can tell the two apart.
///
/// The tool is spawned rather than run to completion, and given
/// [`KEYCHAIN_DEADLINE`] to answer. A keychain that is locked — or waiting on an
/// unlock prompt nobody is looking at — is exactly the case where waiting for an
/// exit never ends, and it is reported as a sign-in this machine would not
/// confirm, never as one that is absent.
#[cfg(target_os = "macos")]
pub(super) fn keychain_sign_in() -> Result<bool, ProbeFailure> {
    let mut child = match Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", KEYCHAIN_SERVICE])
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
    match credentials::wait_or_kill(&mut child, KEYCHAIN_DEADLINE) {
        Some(status) if status.success() => Ok(true),
        Some(status) if status.code() == Some(KEYCHAIN_ITEM_NOT_FOUND) => Ok(false),
        // A tool that failed, and a tool that never finished and was killed.
        Some(_) | None => Err(ProbeFailure::Unanswered),
    }
}

/// A host with no keychain has already given its whole answer in the file and
/// the environment, so this is a real no rather than a failure to look.
#[cfg(not(target_os = "macos"))]
pub(super) fn keychain_sign_in() -> Result<bool, ProbeFailure> {
    Ok(false)
}
