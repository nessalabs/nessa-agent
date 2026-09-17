use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::agents::application::{AgentProbe, ProbeFailure};
use crate::agents::domain::AgentId;
use crate::agents::infrastructure::claude;

/// The two files this server would actually execute to run an agent.
///
/// Composition owns *which* paths these are — it is what reads the launch
/// configuration — but not whether they are present, because that changes while
/// the server runs. Plain paths rather than the composition type that produced
/// them, so the dependency keeps pointing inward.
pub struct AgentLaunchFiles {
    /// The runtime binary the launcher invokes.
    pub runtime: PathBuf,
    /// The entry point script it is handed.
    pub entry: PathBuf,
}

/// The machine this server is running on.
///
/// Everything it needs from the environment is resolved once, where
/// dependencies are chosen, and held as data. Nothing here reads a secret: the
/// questions are whether a credential exists, never what it is.
///
/// What is true of *Claude Code specifically* — the name of its keychain item,
/// where it writes a credentials file, which variables sign it in, what makes
/// one of those a real sign-in — belongs to `claude.rs`. This type owns the
/// order those sources are asked in and what an unanswered source means, which
/// would be the same for any agent.
pub struct LocalAgentProbe {
    /// The files the agent this server would launch is made of, when one is
    /// configured at all. Composition resolves the paths, because composition
    /// is what owns the launch configuration; whether they are there is asked
    /// on every call, because a user can install the agent while the server is
    /// already running and expects the next answer to say so.
    claude_launch_files: Option<AgentLaunchFiles>,
    /// A non-empty credential in the environment, which is what a machine
    /// account signs in with. Held only to know that it is there.
    environment_credential: Option<String>,
    /// Where Claude Code would write a credentials file on this machine.
    claude_config_directory: Option<PathBuf>,
}

impl LocalAgentProbe {
    /// Read this host's environment once, in composition.
    ///
    /// Where the agent lives is not guessed from the filesystem around the
    /// running executable: that only ever matched the bundled desktop layout,
    /// and said nothing at all about a server started from a plain runtime
    /// config. Composition resolves the agent configuration that would really
    /// be launched and hands in its paths as `claude_launch_files`. Their
    /// existence is not resolved here — see [`Self::claude_installed`].
    ///
    /// Claude Code accepts a sign-in from three places, so three are resolved:
    /// a credential in the environment, a credentials file under
    /// `CLAUDE_CONFIG_DIR` (or `~/.claude`), and the login keychain on macOS.
    pub fn from_environment(claude_launch_files: Option<AgentLaunchFiles>) -> Self {
        Self {
            claude_launch_files,
            environment_credential: claude::environment_credential(),
            claude_config_directory: claude::config_directory(),
        }
    }

    /// Whether the agent this server would launch is really on this machine.
    ///
    /// Asked of the filesystem on every call rather than once at construction.
    /// The whole point of setup's "check again" is that a user who was not
    /// ready when it opened can become ready without restarting the server, and
    /// installing the agent's runtime is the most obvious way to do exactly
    /// that; an answer frozen at process start can never report it. The reads
    /// are two metadata stats, and the caller already runs this on a blocking
    /// thread (see `entrypoint/http.rs`), so paying them per call is safe.
    ///
    /// No agent configured is a real no: the question was asked of the
    /// configuration, and the answer is that there is nothing to launch.
    fn claude_installed(&self) -> Result<bool, ProbeFailure> {
        let Some(files) = &self.claude_launch_files else {
            return Ok(false);
        };
        for path in [&files.runtime, &files.entry] {
            if !is_file(path)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Whether anything on this machine is signed in to Claude.
    ///
    /// A yes from any one source ends the search. A source that could not
    /// answer leaves the whole answer undetermined rather than no: a credential
    /// this probe was unable to look for is not a credential it ruled out.
    fn claude_authenticated(&self) -> Result<bool, ProbeFailure> {
        if self.environment_credential.is_some() {
            return Ok(true);
        }
        let mut unanswered = None;
        if holds(&mut unanswered, self.claude_credentials_file())
            || holds(&mut unanswered, claude::keychain_sign_in())
        {
            return Ok(true);
        }
        unanswered.map_or(Ok(false), Err)
    }

    /// Ask Claude's own credentials file, wherever this host puts it. Nowhere to
    /// look is a question that was never asked, not a sign-in ruled out.
    fn claude_credentials_file(&self) -> Result<bool, ProbeFailure> {
        let directory = self
            .claude_config_directory
            .as_deref()
            .ok_or(ProbeFailure::NothingToAsk)?;
        claude::credentials_file(directory)
    }
}

impl AgentProbe for LocalAgentProbe {
    fn installed(&self, agent: AgentId) -> Result<bool, ProbeFailure> {
        match agent {
            AgentId::Claude => self.claude_installed(),
        }
    }

    fn authenticated(&self, agent: AgentId) -> Result<bool, ProbeFailure> {
        match agent {
            AgentId::Claude => self.claude_authenticated(),
        }
    }
}

/// Whether there is really a file at `path`, right now.
///
/// Deliberately not [`Path::is_file`]: that collapses every error into `false`,
/// so a directory this server is not allowed to look inside reads exactly like
/// a missing agent. This module keeps "no" and "could not tell" apart
/// everywhere else — the same distinction `claude.rs` makes about a credentials
/// file — and setup acts on the difference, offering an install to someone
/// whose agent is merely unreadable. Only a genuine not-found is a no; anything
/// else leaves the answer undetermined.
fn is_file(path: &Path) -> Result<bool, ProbeFailure> {
    match path.metadata() {
        Ok(metadata) => Ok(metadata.is_file()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(_) => Err(ProbeFailure::Unanswered),
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

#[cfg(test)]
#[path = "../../../tests/agents/probe.rs"]
mod tests;
