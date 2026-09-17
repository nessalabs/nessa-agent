use std::io::ErrorKind;
use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::process::{Command, Stdio};

use crate::agents::application::{AgentProbe, ProbeFailure};
use crate::agents::domain::AgentId;

/// Where Claude Code keeps its sign-in on macOS. Asked after, never read.
const CLAUDE_KEYCHAIN_SERVICE: &str = "Claude Code-credentials";

/// The machine this server is running on.
///
/// Everything it needs from the environment is resolved once, where
/// dependencies are chosen, and held as data. Nothing here reads a secret: the
/// questions are whether a credential exists, never what it is.
pub struct LocalAgentProbe {
    /// The directory the agent adapters are installed in, if it can be found.
    runtime_root: Option<PathBuf>,
    /// A non-empty API key in the environment, which is what a machine account
    /// signs in with. Held only to know that it is there.
    anthropic_api_key: Option<String>,
    /// Where Claude Code would write a credentials file on this machine.
    claude_config_directory: Option<PathBuf>,
}

impl LocalAgentProbe {
    /// Read this host's environment once, in composition.
    ///
    /// The runtime root is derived from the running executable rather than from
    /// configuration: the adapters sit beside this binary, because the desktop
    /// app ships `nessa` and the adapters into one runtime directory and
    /// launches this server from it. Asking about the server that would
    /// actually start the agent is the only answer worth anything.
    ///
    /// Claude Code accepts a sign-in from three places, so three are resolved:
    /// an API key in the environment, a credentials file under
    /// `CLAUDE_CONFIG_DIR` (or `~/.claude`), and the login keychain on macOS.
    pub fn from_environment() -> Self {
        Self {
            runtime_root: std::env::current_exe()
                .ok()
                .and_then(|executable| executable.parent().map(Path::to_path_buf)),
            anthropic_api_key: std::env::var("ANTHROPIC_API_KEY")
                .ok()
                .filter(|key| !key.trim().is_empty()),
            claude_config_directory: std::env::var("CLAUDE_CONFIG_DIR")
                .map(PathBuf::from)
                .ok()
                .or_else(|| {
                    std::env::var(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                        .ok()
                        .map(|home| PathBuf::from(home).join(".claude"))
                }),
        }
    }

    /// Whether anything on this machine is signed in to Claude.
    ///
    /// A yes from any one source ends the search. A source that could not
    /// answer leaves the whole answer undetermined rather than no: a credential
    /// this probe was unable to look for is not a credential it ruled out.
    fn claude_authenticated(&self) -> Result<bool, ProbeFailure> {
        if self.anthropic_api_key.is_some() {
            return Ok(true);
        }
        let mut unanswered = None;
        if holds(&mut unanswered, self.claude_credentials_file())
            || holds(&mut unanswered, keychain_holds(CLAUDE_KEYCHAIN_SERVICE))
        {
            return Ok(true);
        }
        unanswered.map_or(Ok(false), Err)
    }

    /// Whether Claude Code has written a credentials file here.
    ///
    /// Only the file's size is looked at. An empty file is a real no; a file
    /// that exists with contents is a real yes; anything the filesystem refuses
    /// to describe is neither.
    fn claude_credentials_file(&self) -> Result<bool, ProbeFailure> {
        let directory = self
            .claude_config_directory
            .as_deref()
            .ok_or(ProbeFailure::NothingToAsk)?;
        match directory.join(".credentials.json").metadata() {
            Ok(file) => Ok(file.len() > 0),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
            Err(_) => Err(ProbeFailure::Unanswered),
        }
    }
}

impl AgentProbe for LocalAgentProbe {
    fn installed(&self, agent: AgentId) -> Result<bool, ProbeFailure> {
        let directory = match agent {
            AgentId::Claude => "claude-acp",
        };
        let root = self
            .runtime_root
            .as_deref()
            .ok_or(ProbeFailure::NothingToAsk)?;
        match root.join(directory).metadata() {
            Ok(entry) => Ok(entry.is_dir()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
            Err(_) => Err(ProbeFailure::Unanswered),
        }
    }

    fn authenticated(&self, agent: AgentId) -> Result<bool, ProbeFailure> {
        match agent {
            AgentId::Claude => self.claude_authenticated(),
        }
    }
}

/// Fold one source's answer into the search.
///
/// Yes ends it. A source that could not answer is remembered, so that a final
/// no is never claimed on behalf of a source that never spoke.
fn holds(unanswered: &mut Option<ProbeFailure>, source: Result<bool, ProbeFailure>) -> bool {
    match source {
        Ok(found) => found,
        Err(failure) => {
            *unanswered = Some(failure);
            false
        }
    }
}

/// `errSecItemNotFound`: the keychain looked and there is no such item. This is
/// the tool answering no, not the tool failing.
#[cfg(target_os = "macos")]
const KEYCHAIN_ITEM_NOT_FOUND: i32 = 44;

/// Whether the login keychain holds a generic password for `service`.
///
/// Asked for the item's *attributes* and not its data: `find-generic-password`
/// without `-w` prints metadata and never the secret, so this answers "is there
/// a sign-in" without the server handling the token and without the keychain
/// prompting to release one.
///
/// Output is discarded and only the exit status read. A locked keychain, a
/// missing tool, or any other failure is reported as a failure rather than as
/// an absent sign-in, so that the caller can tell the two apart.
#[cfg(target_os = "macos")]
fn keychain_holds(service: &str) -> Result<bool, ProbeFailure> {
    match Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", service])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(status) if status.success() => Ok(true),
        Ok(status) if status.code() == Some(KEYCHAIN_ITEM_NOT_FOUND) => Ok(false),
        Ok(_) => Err(ProbeFailure::Unanswered),
        Err(error) if error.kind() == ErrorKind::NotFound => Err(ProbeFailure::NothingToAsk),
        Err(_) => Err(ProbeFailure::Unanswered),
    }
}

/// A host with no keychain has already given its whole answer in the file and
/// the environment, so this is a real no rather than a failure to look.
#[cfg(not(target_os = "macos"))]
fn keychain_holds(_service: &str) -> Result<bool, ProbeFailure> {
    Ok(false)
}

#[cfg(test)]
#[path = "../../../tests/agents/probe.rs"]
mod tests;
