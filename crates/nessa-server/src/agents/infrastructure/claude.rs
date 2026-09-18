//! How Claude Code itself reports being signed in.
//!
//! Claude Code writes its sign-in in places Anthropic chose, not places Nessa
//! chose: an environment variable the launcher passes through, a credentials
//! file under `CLAUDE_CONFIG_DIR`, and the login keychain on macOS. That is
//! Claude's own knowledge, so it lives in one named place rather than spread
//! through the generic host adapter. `local.rs` orchestrates — it decides the
//! order sources are asked in and what an unanswered source means — and calls
//! in here for every fact that is true of Claude specifically. What makes a
//! variable a credential and a file a sign-in is the same for every agent and
//! lives in `credentials.rs`.
//!
//! Nothing here reads a secret. Every question is whether a credential exists.

#[cfg(target_os = "macos")]
use std::io::ErrorKind;
use std::path::PathBuf;
#[cfg(target_os = "macos")]
use std::process::{Command, Stdio};
// The waiting helper below belongs to the keychain, which only macOS has. It is
// compiled on other Unix hosts under `cfg(test)` alone, because that is where it
// can be driven honestly with an ordinary long-running command; Windows has
// neither, and compiling it there would be dead code under `-D warnings`.
#[cfg(any(target_os = "macos", all(unix, test)))]
use std::process::{Child, ExitStatus};
#[cfg(any(target_os = "macos", all(unix, test)))]
use std::time::{Duration, Instant};

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

/// The environment variables the launcher passes through to the agent as a
/// sign-in. Either one on its own starts Claude Code, so either one on its own
/// is an answered yes here; anything this probe did not check is a machine
/// reported as needing a sign-in it already has.
const CREDENTIAL_VARIABLES: [&str; 2] = ["ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN"];

/// A non-empty credential in this process's environment, which is what a
/// machine account signs in with. Returned only to know that it is there.
pub(super) fn environment_credential() -> Option<String> {
    credentials::environment_credential(&CREDENTIAL_VARIABLES)
}

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

/// How often the waiting thread looks to see whether the tool has finished.
///
/// The standard library has no wait-with-deadline, so the wait is a poll. This
/// thread exists only to wait, so the cost is a wakeup every 20ms — short enough
/// that a healthy answer is still returned promptly, long enough that a wait to
/// the full deadline costs a few dozen wakeups rather than a spin.
#[cfg(any(target_os = "macos", all(unix, test)))]
const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Wait for `child` for at most `limit`, and kill it if that runs out.
///
/// `Command::status()` waits forever, which makes the caller's cost whatever the
/// child's cost happens to be. This puts a ceiling on it, and — the part that
/// matters — the ceiling is real: on expiry the child is killed *and reaped*, so
/// the process is gone rather than merely stopped being waited for. Abandoning
/// the wait instead would leave a request that looks bounded and a machine that
/// is not.
///
/// `None` means the child never answered within `limit` and was killed. Every
/// caller reports that as a question this machine did not answer.
#[cfg(any(target_os = "macos", all(unix, test)))]
fn wait_or_kill(child: &mut Child, limit: Duration) -> Option<ExitStatus> {
    let deadline = Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            // Still running, or this machine will not say: both are waited out,
            // and both end at the kill below rather than in an unbounded loop.
            Ok(None) => {}
            Err(_) => break,
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        std::thread::sleep(PROCESS_POLL_INTERVAL.min(left));
    }
    // Kill *and* wait. Killing alone leaves a zombie holding a process table
    // entry for as long as this server runs.
    let _ = child.kill();
    let _ = child.wait();
    None
}

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
    match wait_or_kill(&mut child, KEYCHAIN_DEADLINE) {
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

#[cfg(test)]
#[path = "../../../tests/agents/claude.rs"]
mod tests;
