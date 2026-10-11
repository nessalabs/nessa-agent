//! `nessa env grant`, `nessa env ungrant` and `nessa env grants`: this
//! machine's owner says what each gateway leasing from it is granted, in
//! `<data>/environment/grants.json` (issue #706, part 1d).
//!
//! ```text
//! env grant / ungrant [--peer P] ──▶ the peer: P, or the gateway this
//!   machine's pairing approved (FilePairingEnvironment), or ssh
//!   ──▶ FileGrants::change: what the peer has now (its entry, or the
//!       defaults) ──▶ granted more / less ──▶ written, or the entry removed
//!       when it is the peer's defaults again ──▶ every entry printed
//! env grants ──▶ FileGrants::read ──▶ every entry printed
//! ```
//!
//! Arrows are steps, in order. Whoever can write `<data>` is this machine's
//! owner: nothing else authorizes a change. A serving process reads the
//! file again at every lease, so a change applies from the next one, and
//! the leases already running go on.
use super::env_serve_command::environment_directory;
use crate::{
    app::dependencies::RuntimeDependencies,
    cli::entrypoint::{GrantedAgents, Ungranted},
    core::RunError,
    env_serve::{
        domain::{granted_to, AgentName, Agents, AllAgents, CommandGrant, Peer, PeerGrant},
        infrastructure::{printed, Changed, FileGrants, FilePairingEnvironment},
    },
};
use nessa_protocol::agents::AgentId;
use nessa_sdk::domain::agent_execution::leases::CommandWork;
use std::{io::Write, path::Path};

/// `nessa env grant`: grant `peer` the `agents`, and commands of at most
/// `max_timeout_ms` each, beside what it has now.
///
/// # Errors
/// The peer or an agent is not one, the timeout is out of bounds, or the
/// grants could not be read or written.
pub(super) async fn grant(
    peer: Option<&str>,
    agents: Option<GrantedAgents>,
    max_timeout_ms: Option<u64>,
) -> Result<(), RunError> {
    let agents = agents.map(granted_agents).transpose()?;
    let commands = max_timeout_ms
        .map(|max| {
            CommandGrant::new(max).map_err(|_| {
                RunError::Agent(format!(
                    "--max-timeout is at most {}h",
                    CommandWork::MAX_TIMEOUT_MS / 3_600_000
                ))
            })
        })
        .transpose()?;
    let directory = environment_directory()?;
    let peer = peer_named(&directory, peer).await?;
    let grants = FileGrants::new(&directory);
    change(&grants, &peer, |current| {
        let grant = match agents {
            Some(agents) => current.with_agents(agents),
            None => current,
        };
        Ok(match commands {
            Some(commands) => grant.with_commands(commands),
            None => grant,
        })
    })?;
    print(&grants)
}

/// `nessa env ungrant`: take `what` back from `peer`.
///
/// # Errors
/// The peer or agent is not one, an agent is taken by name from a grant
/// of every agent, or the grants could not be read or written.
pub(super) async fn ungrant(peer: Option<&str>, what: Ungranted) -> Result<(), RunError> {
    let agent = match &what {
        Ungranted::Agent(name) => Some(agent_name(name)?),
        Ungranted::Commands | Ungranted::All => None,
    };
    let directory = environment_directory()?;
    let peer = peer_named(&directory, peer).await?;
    let grants = FileGrants::new(&directory);
    change(&grants, &peer, |current| match (what, agent) {
        (Ungranted::Agent(_), Some(agent)) => current.without_agent(&agent).map_err(|AllAgents| {
            RunError::Agent(format!(
                "{} is granted every agent: ungrant --all, then grant the agents it keeps",
                peer.name()
            ))
        }),
        (Ungranted::Commands, _) => Ok(current.without_commands()),
        (Ungranted::All | Ungranted::Agent(_), _) => Ok(PeerGrant::nothing()),
    })?;
    print(&grants)
}

/// `nessa env grants`: every entry, as JSON on standard output.
///
/// # Errors
/// The grants could not be read.
pub(super) fn grants() -> Result<(), RunError> {
    print(&FileGrants::new(&environment_directory()?))
}

/// Change `peer`'s grant with `change`, which is given what the peer has
/// now, its defaults when it has no entry. A grant equal to its defaults is
/// kept as no entry, so the defaults stay the only statement of it.
fn change(
    grants: &FileGrants,
    peer: &Peer,
    change: impl FnOnce(PeerGrant) -> Result<PeerGrant, RunError>,
) -> Result<(), RunError> {
    let defaults = granted_to(peer, None);
    grants
        .change(peer, |entry| {
            let changed = change(granted_to(peer, entry))?;
            Ok((changed != defaults).then_some(changed))
        })
        .map_err(|failure| match failure {
            Changed::Grants(error) => RunError::Agent(error.to_string()),
            Changed::Refused(refused) => refused,
        })
}

fn print(grants: &FileGrants) -> Result<(), RunError> {
    let all = grants
        .read()
        .map_err(|error| RunError::Agent(error.to_string()))?;
    let text = serde_json::to_string_pretty(&printed(&all))
        .map_err(|error| RunError::Agent(error.to_string()))?;
    writeln!(std::io::stdout(), "{text}")?;
    Ok(())
}

/// The peer `given` names, or the gateway this machine's pairing was
/// approved by, or `ssh` when nothing is paired here.
async fn peer_named(directory: &Path, given: Option<&str>) -> Result<Peer, RunError> {
    if let Some(name) = given {
        return Peer::parse(name).map_err(|_| {
            RunError::Agent(format!(
                "{name}: give --peer as ssh or a gateway's pin as env grants prints it"
            ))
        });
    }
    let approved =
        FilePairingEnvironment::approved_gateway(directory, RuntimeDependencies::default().clock)
            .await
            .map_err(RunError::Agent)?;
    Ok(approved.map_or(Peer::Ssh, |pin| Peer::Gateway { pin }))
}

fn granted_agents(agents: GrantedAgents) -> Result<Agents, RunError> {
    match agents {
        GrantedAgents::All => Ok(Agents::All),
        GrantedAgents::Named(names) => names
            .iter()
            .map(|name| match AgentId::parse(name) {
                Some(_) => agent_name(name),
                None => Err(RunError::Agent(format!(
                    "{name}: not an agent this build runs"
                ))),
            })
            .collect::<Result<_, _>>()
            .map(Agents::Named),
    }
}

fn agent_name(name: &str) -> Result<AgentName, RunError> {
    AgentName::new(name).map_err(|_| RunError::Agent(format!("{name:?} is not an agent's name")))
}
