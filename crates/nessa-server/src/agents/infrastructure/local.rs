use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::agents::application::{AgentCredentialSource, AgentProbe, ProbeFailure};
use crate::agents::domain::AgentId;
use crate::agents::infrastructure::{claude, codex, credentials};

/// What this server would have to find on this machine to run an agent.
///
/// A command and the paths it is handed, rather than a runtime and an entry
/// script: an agent run through a shared runtime has one of each, and an agent
/// that is one self-contained executable taking its own subcommand has a
/// command and nothing else. The narrower pair could not describe the second
/// at all.
///
/// Composition owns *which* paths these are — it is what reads the launch
/// configuration — but not whether they are present, because that changes while
/// the server runs. Plain paths rather than the composition type that produced
/// them, so the dependency keeps pointing inward.
pub struct AgentLaunchFiles {
    /// The executable the launcher invokes.
    pub command: PathBuf,
    /// Every path the launcher hands that executable. Empty for an agent whose
    /// arguments name nothing on this machine.
    pub paths: Vec<PathBuf>,
    /// Exactly the environment the launcher starts that executable with.
    ///
    /// The launch clears the environment and names what it passes, so an agent
    /// asked about its own sign-in has to be asked under the same names or it
    /// answers about a different installation: the vendor's home directory
    /// decides which account it reads, and on Linux the session-bus variables
    /// decide whether a keyring can be opened at all. A probe that inherited
    /// this server's environment could find a sign-in the launch cannot use.
    pub environment: BTreeMap<OsString, OsString>,
}

/// How one agent answers for its own account store.
///
/// Given whatever launch this server resolved for that agent, because an agent
/// that answers for itself has to be asked as the copy of it that would really
/// run. What an absent launch means is the source's own to say.
type VendorStore = fn(Option<&AgentLaunchFiles>) -> Result<bool, ProbeFailure>;

/// Where one agent's sign-in could be on this machine, resolved once.
///
/// Held as data rather than asked for on each call because none of it changes
/// while the server runs: this process's environment is fixed at start, and so
/// is where an agent keeps its file. Whether the file is *there* is asked every
/// time, which is the part a person can change by signing in.
struct SignIn {
    /// The agent's own credentials file, when this host has somewhere to look.
    credentials: Option<PathBuf>,
    /// The agent's own account store, asked the agent's own way — a keychain
    /// item for one agent, the agent's own command-line status for another.
    /// `None` for an agent that keeps its sign-in nowhere but the two sources
    /// above: a source that does not exist for this agent is not a source that
    /// failed to answer.
    ///
    /// A keychain item is there to be read whether or not anything would be
    /// started; an agent that answers by running has nothing to ask.
    vendor_store: Option<VendorStore>,
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
    /// What each configured agent is made of. Composition resolves the
    /// paths, because composition is what owns the launch configuration;
    /// whether they are there is asked on every call, because a user can
    /// install an agent while the server is already running and expects the
    /// next answer to say so. An agent absent from this map is one this server
    /// has nothing to launch for.
    launch_files: HashMap<AgentId, AgentLaunchFiles>,
    /// Nessa-owned credential source shared with Claude provider construction.
    credentials: Arc<dyn AgentCredentialSource>,
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
    pub fn from_environment(
        launch_files: HashMap<AgentId, AgentLaunchFiles>,
        credentials: Arc<dyn AgentCredentialSource>,
    ) -> Self {
        Self {
            launch_files,
            credentials,
            sign_in: HashMap::from([
                (
                    AgentId::Claude,
                    SignIn {
                        credentials: claude::credentials_path(),
                        vendor_store: Some(|_| claude::keychain_sign_in()),
                    },
                ),
                (
                    AgentId::Codex,
                    SignIn {
                        credentials: codex::credentials_path(),
                        vendor_store: Some(codex_sign_in),
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
    /// Whether composition resolved a launch for this agent.
    fn configured(&self, agent: AgentId) -> bool {
        self.launch_files.contains_key(&agent)
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
    /// An agent with nothing to launch is not answered here. Whether this
    /// server is configured for it is [`Self::configured`]'s question and is
    /// asked first, so this is not reached for one — and if a second caller ever
    /// does reach it, it reports that it has nothing to go on rather than a no.
    /// `Ok(false)` would become "not installed", which is an instruction to
    /// install what may already be on the machine, and that is a policy the
    /// domain decides and this adapter must not.
    fn installed(&self, agent: AgentId) -> Result<bool, ProbeFailure> {
        let Some(files) = self.launch_files.get(&agent) else {
            return Err(ProbeFailure::NothingToAsk);
        };
        if !is_file(&files.command)? {
            return Ok(false);
        }
        // A path this server would hand the command has to be there for the
        // launch to work, but it is not required to be a regular file: an agent
        // given a directory to work in is given a directory.
        for path in &files.paths {
            if !exists(path)? {
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
        let mut unanswered = None;
        if agent == AgentId::Claude
            && holds(
                &mut unanswered,
                self.credentials
                    .read(agent)
                    .map(|credential| credential.is_some())
                    .map_err(|_| ProbeFailure::Unanswered),
            )
        {
            return Ok(true);
        }
        if holds(&mut unanswered, Self::credentials_file(sign_in)) {
            return Ok(true);
        }
        if let Some(store) = sign_in.vendor_store {
            if holds(&mut unanswered, store(self.launch_files.get(&agent))) {
                return Ok(true);
            }
        }
        unanswered.map_or(Ok(false), Err)
    }
}

/// Ask the Codex this server would launch whether it is signed in.
///
/// Its launch is a command and the adapter entry handed to it, which is exactly
/// what [`codex::sign_in_status`] needs; a launch shaped any other way is one
/// this function has nothing to ask, which is an unasked question rather than a
/// no.
fn codex_sign_in(files: Option<&AgentLaunchFiles>) -> Result<bool, ProbeFailure> {
    let files = files.ok_or(ProbeFailure::NothingToAsk)?;
    let [entry] = files.paths.as_slice() else {
        return Err(ProbeFailure::NothingToAsk);
    };
    // Codex reports "nothing is signed in" and its launcher reports "I could
    // not start" with the same exit status, so an adapter that is not on this
    // machine would come back as a signed-out account. Establish that there is
    // something here to run before running it, and leave the question unasked
    // when there is not.
    if !is_file(entry)? {
        return Err(ProbeFailure::NothingToAsk);
    }
    codex::sign_in_status(&files.command, entry, &files.environment)
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

/// Whether there is anything at all at `path`, right now.
///
/// The same three-way answer as [`is_file`], and for the same reason: only a
/// genuine not-found is a no.
fn exists(path: &Path) -> Result<bool, ProbeFailure> {
    match path.metadata() {
        Ok(_) => Ok(true),
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
