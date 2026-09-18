use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::agents::application::{AgentProbe, ProbeFailure};
use crate::agents::domain::AgentId;
use crate::agents::infrastructure::{claude, codex, credentials};

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

/// Where one agent's sign-in could be on this machine, resolved once.
///
/// Held as data rather than asked for on each call because none of it changes
/// while the server runs: this process's environment is fixed at start, and so
/// is where an agent keeps its file. Whether the file is *there* is asked every
/// time, which is the part a person can change by signing in.
struct SignIn {
    /// A non-empty credential in this process's environment, which is what a
    /// machine account signs in with. Held only to know that it is there.
    environment: Option<String>,
    /// The agent's own credentials file, when this host has somewhere to look.
    credentials: Option<PathBuf>,
    /// The login keychain, for an agent that keeps a sign-in there too. `None`
    /// for an agent that does not: a source that does not exist for this agent
    /// is not a source that failed to answer.
    keychain: Option<fn() -> Result<bool, ProbeFailure>>,
}

/// The machine this server is running on.
///
/// Everything it needs from the environment is resolved once, where
/// dependencies are chosen, and held as data. Nothing here reads a secret: the
/// questions are whether a credential exists, never what it is.
///
/// What is true of one agent specifically — the name of its keychain item,
/// where it writes a credentials file, which variables sign it in — belongs to
/// that agent's own module. This type owns the order those sources are asked in
/// and what an unanswered source means, which is the same for any agent.
pub struct LocalAgentProbe {
    /// The files each configured agent is made of. Composition resolves the
    /// paths, because composition is what owns the launch configuration;
    /// whether they are there is asked on every call, because a user can
    /// install an agent while the server is already running and expects the
    /// next answer to say so. An agent absent from this map is one this server
    /// has nothing to launch for.
    launch_files: HashMap<AgentId, AgentLaunchFiles>,
    /// Where each agent's sign-in could be.
    sign_in: HashMap<AgentId, SignIn>,
}

impl LocalAgentProbe {
    /// Read this host's environment once, in composition.
    ///
    /// Where each agent lives is not guessed from the filesystem around the
    /// running executable: that only ever matched the bundled desktop layout,
    /// and said nothing at all about a server started from a plain runtime
    /// config. Composition resolves the agent configurations that would really
    /// be launched and hands in their paths. Their existence is not resolved
    /// here — see [`Self::installed`].
    pub fn from_environment(launch_files: HashMap<AgentId, AgentLaunchFiles>) -> Self {
        Self {
            launch_files,
            sign_in: HashMap::from([
                (
                    AgentId::Claude,
                    SignIn {
                        environment: claude::environment_credential(),
                        credentials: claude::credentials_path(),
                        keychain: Some(claude::keychain_sign_in),
                    },
                ),
                (
                    AgentId::Codex,
                    SignIn {
                        environment: codex::environment_credential(),
                        credentials: codex::credentials_path(),
                        keychain: None,
                    },
                ),
            ]),
        }
    }

    /// Ask this agent's credentials file, wherever its vendor puts it. Nowhere
    /// to look is a question that was never asked, not a sign-in ruled out.
    fn credentials_file(sign_in: &SignIn) -> Result<bool, ProbeFailure> {
        let path = sign_in
            .credentials
            .as_deref()
            .ok_or(ProbeFailure::NothingToAsk)?;
        credentials::credentials_file(path)
    }
}

impl AgentProbe for LocalAgentProbe {
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
    /// No configuration for this agent is a real no: the question was asked of
    /// the configuration, and the answer is that there is nothing to launch.
    fn installed(&self, agent: AgentId) -> Result<bool, ProbeFailure> {
        let Some(files) = self.launch_files.get(&agent) else {
            return Ok(false);
        };
        for path in [&files.runtime, &files.entry] {
            if !is_file(path)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Whether anything on this machine is signed in to the agent.
    ///
    /// A yes from any one source ends the search. A source that could not
    /// answer leaves the whole answer undetermined rather than no: a credential
    /// this probe was unable to look for is not a credential it ruled out. An
    /// agent this server knows nothing about has no sources at all, which is
    /// the same kind of unanswered question as a directory it cannot read.
    fn authenticated(&self, agent: AgentId) -> Result<bool, ProbeFailure> {
        let sign_in = self.sign_in.get(&agent).ok_or(ProbeFailure::NothingToAsk)?;
        if sign_in.environment.is_some() {
            return Ok(true);
        }
        let mut unanswered = None;
        if holds(&mut unanswered, Self::credentials_file(sign_in)) {
            return Ok(true);
        }
        if let Some(keychain) = sign_in.keychain {
            if holds(&mut unanswered, keychain()) {
                return Ok(true);
            }
        }
        unanswered.map_or(Ok(false), Err)
    }
}

/// Whether there is really a file at `path`, right now.
///
/// Deliberately not [`Path::is_file`]: that collapses every error into `false`,
/// so a directory this server is not allowed to look inside reads exactly like
/// a missing agent. This module keeps "no" and "could not tell" apart
/// everywhere else — the same distinction `credentials.rs` makes about a
/// credentials file — and setup acts on the difference, offering an install to
/// someone whose agent is merely unreadable. Only a genuine not-found is a no;
/// anything else leaves the answer undetermined.
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
