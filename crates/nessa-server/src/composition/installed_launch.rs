//! The launch for an agent Nessa installs rather than ships.
//!
//! The desktop bundles Claude and Codex, so their launch is a path inside the
//! bundle and is known before the app starts. Opencode is fetched onto the
//! machine that wants it, so its launch is only knowable by asking what is
//! actually installed — and the answer can change between two starts, because
//! somebody can install it while the app is closed, or a new build can pin a
//! version the installed one is not.
//!
//! So this is asked at every start rather than written down once. That is the
//! rule `agent_install` states for itself: the store answers from a record it
//! wrote, and answers "not installed" for anything that is not exactly the
//! artifact the current pin names. A launch kept from an earlier install would
//! keep working after the pin moved — superseded artifacts are left where they
//! are — and the agent Nessa tested would be quietly replaced by one it never
//! saw.
//!
//! ```text
//! releases_for(agent) ─▶ preferred_release(host) ─▶ RuntimeStore::installed
//!                                                        │
//!                                    Some(path) ─────────┴──── None
//!                                        │                      │
//!                                   a launch                 no launch,
//!                                                     and any older one is removed
//! ```
//!
//! The removal is the half that is easy to forget. Leaving a stale entry behind
//! is worse than never writing one: the picker would offer an agent whose
//! executable the current pin does not describe, and the person would be told
//! it is ready right up until it is not.
use std::path::PathBuf;

use crate::agent_install::application::RuntimeStore;
use crate::agent_install::domain::{preferred_release, AgentName, HostPlatform};
use crate::agent_install::infrastructure::{releases_for, PinFileError};
use crate::agents::domain::AgentId;

/// What this agent's runtime speaks ACP with, once it is on the machine.
///
/// Opencode ships as one executable and is asked for its ACP server by
/// subcommand, which is what the recorded contract fixtures were taken from.
const ACP_SUBCOMMAND: &str = "acp";

/// Why an installed launch could not be resolved.
///
/// Only a pin file this build compiled in can be wrong here, so this is a
/// programming fault rather than a machine's state — a store that cannot be
/// read is reported as "not installed" instead, since that is what it means to
/// whoever is choosing an agent.
#[derive(Debug)]
pub(super) enum LaunchUnknown {
    /// The compiled-in pin document does not describe this agent.
    Pins(PinFileError),
    /// The agent's own name is not one a pin file may carry.
    Name(String),
}

impl std::fmt::Display for LaunchUnknown {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pins(error) => write!(out, "pinned releases could not be read: {error}"),
            Self::Name(name) => write!(out, "{name} is not a name a pinned release may carry"),
        }
    }
}

/// Where `agent` is installed on `host`, if the pinned release for it is.
///
/// `Ok(None)` is the ordinary answer on a machine where nobody has run
/// `nessa install-agent`, and it is also the answer when a previous version is
/// installed but the current pin names another. Both mean the same thing to a
/// caller: there is no launch to write.
///
/// A store that fails to answer is read as "not installed" rather than
/// propagated. A gateway that refused to start because one optional agent's
/// record could not be read would take Claude and Codex down with it, and the
/// person would have no way to tell which of the three was at fault.
pub(super) fn installed_launch(
    agent: AgentId,
    host: &HostPlatform,
    store: &dyn RuntimeStore,
) -> Result<Option<PathBuf>, LaunchUnknown> {
    let name =
        AgentName::parse(agent.name()).map_err(|_| LaunchUnknown::Name(agent.name().into()))?;
    let releases = releases_for(&name).map_err(LaunchUnknown::Pins)?;
    let Some(release) = preferred_release(releases, host) else {
        return Ok(None);
    };
    Ok(store.installed(&name, &release).ok().flatten())
}

/// The arguments `agent`'s installed runtime is launched with.
///
/// Takes the agent rather than answering one way for all of them. `acp` is
/// Opencode's subcommand and nothing else's, and the only reason a wrong answer
/// has not been given yet is that no other agent reaches this. Naming each one
/// means the next agent to arrive has to say what it needs, rather than
/// inheriting a word that happens to be there.
pub(super) fn installed_arguments(agent: AgentId) -> Vec<String> {
    match agent {
        AgentId::Opencode => vec![ACP_SUBCOMMAND.to_string()],
        // Bundled today, so this is not reached. Answered rather than
        // wildcarded: a launch is the one place a wrong guess is silent.
        AgentId::Claude | AgentId::Codex => Vec::new(),
    }
}

#[cfg(test)]
#[path = "../../tests/composition/installed_launch.rs"]
mod tests;
