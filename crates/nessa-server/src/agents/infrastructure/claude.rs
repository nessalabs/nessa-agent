//! How Claude Code itself reports being installed and signed in.
//!
//! Claude Code writes its sign-in in places Anthropic chose, not places Nessa
//! chose: an environment variable the launcher passes through, a credentials
//! file under `CLAUDE_CONFIG_DIR`, and the login keychain on macOS. That is
//! Claude's own knowledge, so it lives in one named place rather than spread
//! through the generic host adapter. `local.rs` orchestrates — it decides the
//! order sources are asked in and what an unanswered source means — and calls
//! in here for every fact that is true of Claude specifically.
//!
//! Nothing here reads a secret. Every question is whether a credential exists.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
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

/// The most of a credentials file this probe will ever read.
///
/// The file holds one small JSON object. A path that leads to something far
/// larger is not a credentials file this probe can honestly judge, and reading
/// it would let whatever wrote it choose how much memory the server spends.
const MAX_CREDENTIALS_BYTES: u64 = 64 * 1024;

/// A non-empty credential in this process's environment, which is what a
/// machine account signs in with. Returned only to know that it is there.
pub(super) fn environment_credential() -> Option<String> {
    CREDENTIAL_VARIABLES.iter().find_map(|key| {
        std::env::var(key)
            .ok()
            .filter(|value| !value.trim().is_empty())
    })
}

/// Where Claude Code would write its credentials file on this machine.
///
/// `CLAUDE_CONFIG_DIR` when it is set, because that is what Claude Code obeys;
/// otherwise `~/.claude`, its default. A host with neither leaves nowhere to
/// look, which the caller reports as a question it could not ask.
pub(super) fn config_directory() -> Option<PathBuf> {
    std::env::var("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .ok()
        .or_else(|| {
            std::env::var(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                .ok()
                .map(|home| PathBuf::from(home).join(".claude"))
        })
}

/// Whether Claude Code has written a usable credentials file in `directory`.
///
/// Nessa does not define this file's format — Anthropic's own `claude` CLI
/// writes it — and there is no published schema to check against. So this
/// asserts only what is true of the file whatever its schema is, and guesses at
/// no field name: inventing a required key would reject genuinely valid
/// credentials the day that key is renamed, which is a worse failure than the
/// one being fixed here.
///
/// What can be said with certainty:
///
/// - The file must be small. Its size is read before its bytes, and anything
///   past [`MAX_CREDENTIALS_BYTES`] is refused unread — a source that did not
///   answer rather than a sign-in ruled out.
/// - It must parse as JSON. A truncated or corrupted write is a real no, for
///   the same reason an empty file always was: Claude Code could not sign in
///   with it either.
/// - It must be a JSON *object* with at least one key. A bare string, array,
///   number, `true`, `null`, or an `{}` left behind by a crashed process
///   carries no credential, so each is a real no.
///
/// Anything past that is Anthropic's business. A file that clears these checks
/// is reported as a sign-in without ever being interpreted.
pub(super) fn credentials_file(directory: &Path) -> Result<bool, ProbeFailure> {
    let path = directory.join(CREDENTIALS_FILE);
    match path.metadata() {
        Ok(file) if file.len() == 0 => Ok(false),
        Ok(file) if file.len() > MAX_CREDENTIALS_BYTES => Err(ProbeFailure::Unanswered),
        Ok(_) => match std::fs::read_to_string(&path) {
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
