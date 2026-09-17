use std::path::PathBuf;

use crate::agents::application::{AgentProbe, ProbeFailure};
use crate::agents::domain::AgentId;
use crate::agents::infrastructure::claude;

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
    /// Whether the agent this server would actually launch is configured and
    /// its executable and entry point are really there. Composition resolves
    /// this, because composition is what owns the launch configuration.
    claude_installed: bool,
    /// A non-empty credential in the environment, which is what a machine
    /// account signs in with. Held only to know that it is there.
    environment_credential: Option<String>,
    /// Where Claude Code would write a credentials file on this machine.
    claude_config_directory: Option<PathBuf>,
}

impl LocalAgentProbe {
    /// Read this host's environment once, in composition.
    ///
    /// Whether the agent is installed is not guessed from the filesystem around
    /// the running executable: that only ever matched the bundled desktop
    /// layout, and said nothing at all about a server started from a plain
    /// runtime config. Composition resolves the agent configuration that would
    /// really be launched and hands the answer in as `claude_installed`.
    ///
    /// Claude Code accepts a sign-in from three places, so three are resolved:
    /// a credential in the environment, a credentials file under
    /// `CLAUDE_CONFIG_DIR` (or `~/.claude`), and the login keychain on macOS.
    pub fn from_environment(claude_installed: bool) -> Self {
        Self {
            claude_installed,
            environment_credential: claude::environment_credential(),
            claude_config_directory: claude::config_directory(),
        }
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
    /// Already determined, in composition, from the configuration this server
    /// would launch. Nothing configured is a real no rather than a failure to
    /// look: the question was asked and the answer is that there is no agent.
    fn installed(&self, agent: AgentId) -> Result<bool, ProbeFailure> {
        match agent {
            AgentId::Claude => Ok(self.claude_installed),
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

#[cfg(test)]
#[path = "../../../tests/agents/probe.rs"]
mod tests;
