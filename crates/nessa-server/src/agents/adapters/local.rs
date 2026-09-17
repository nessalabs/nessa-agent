use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::agents::application::AgentProbe;
use crate::agents::domain::AgentId;

/// Where Claude Code keeps its sign-in on macOS. Asked after, never read.
const CLAUDE_KEYCHAIN_SERVICE: &str = "Claude Code-credentials";

/// The machine this server is running on.
pub struct LocalAgentProbe;

impl LocalAgentProbe {
    /// The directory the agent adapters are installed in.
    ///
    /// They sit beside this binary: the desktop app ships `nessa` and the
    /// adapters into one runtime directory and launches this server from it.
    /// Deriving it from the running executable rather than from configuration
    /// means the answer is about the server that will actually start the
    /// agent, which is the only server whose answer is worth anything.
    fn runtime_root() -> Option<PathBuf> {
        std::env::current_exe()
            .ok()?
            .parent()
            .map(std::path::Path::to_path_buf)
    }

    /// Whether anything on this machine is signed in to Claude.
    ///
    /// Three places, because Claude Code accepts three. An API key in the
    /// environment is what a machine account uses; a credentials file under
    /// `CLAUDE_CONFIG_DIR` (or `~/.claude`) is what it writes where no keychain
    /// is available; and on macOS the sign-in is a keychain item.
    fn claude_authenticated() -> bool {
        if std::env::var("ANTHROPIC_API_KEY").is_ok_and(|key| !key.trim().is_empty()) {
            return true;
        }
        let configured = std::env::var("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .ok()
            .or_else(|| {
                std::env::var(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                    .ok()
                    .map(|home| PathBuf::from(home).join(".claude"))
            });
        if configured.is_some_and(|directory| {
            directory
                .join(".credentials.json")
                .metadata()
                .is_ok_and(|file| file.len() > 0)
        }) {
            return true;
        }
        keychain_holds(CLAUDE_KEYCHAIN_SERVICE)
    }
}

impl AgentProbe for LocalAgentProbe {
    fn installed(&self, agent: AgentId) -> bool {
        let directory = match agent {
            AgentId::Claude => "claude-acp",
        };
        LocalAgentProbe::runtime_root().is_some_and(|root| root.join(directory).is_dir())
    }

    fn authenticated(&self, agent: AgentId) -> bool {
        match agent {
            AgentId::Claude => LocalAgentProbe::claude_authenticated(),
        }
    }
}

/// Whether the login keychain holds a generic password for `service`.
///
/// Asked for the item's *attributes* and not its data: `find-generic-password`
/// without `-w` prints metadata and never the secret, so this answers "is there
/// a sign-in" without the server handling the token and without the keychain
/// prompting to release one.
///
/// Output is discarded and only the exit status read. A missing binary, a
/// locked keychain or any other failure reads as absent, which is the safe
/// direction: the cost is offering to sign in again, where the alternative is
/// offering an agent that cannot run.
#[cfg(target_os = "macos")]
fn keychain_holds(service: &str) -> bool {
    Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", service])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Hosts with no keychain answer no, and fall back to the file and the
/// environment above.
#[cfg(not(target_os = "macos"))]
fn keychain_holds(_service: &str) -> bool {
    let _ = (Command::new("true"), Stdio::null());
    false
}
