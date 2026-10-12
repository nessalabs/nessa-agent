//! `nessa env grant`, `nessa env ungrant` and `nessa env grants`: this
//! machine's owner says what each gateway leasing from it is granted, in
//! `<data>/environment/grants.json` (issue #706, part 1d).
//!
//! ```text
//! env grant / ungrant [--peer P] ──▶ the peer: P, or the gateway this
//!   machine's pairing approved (FilePairingEnvironment), or ssh
//!   ──▶ change_grant(FileGrants, FileGrantAudit, SystemClock, this account)
//!       ──▶ what the peer has now (its entry, or the defaults) ──▶ granted
//!       more / less ──▶ its intent audited in grants-audit.jsonl ──▶
//!       written, or the entry removed when it is the peer's defaults again
//!       ──▶ its outcome audited
//!   ──▶ every entry printed
//! env grants ──▶ FileGrants::read ──▶ every entry printed
//! ```
//!
//! Arrows are steps, in order. Whoever can write `<data>` is this machine's
//! owner: nothing else authorizes a change. A serving process reads the
//! file again at every lease, so a change applies from the next one, and
//! the leases already running go on.
use super::env_serve_command::environment_directory;
use super::install_command::local_account_id;
use super::local_auth::SystemClock;
use crate::{
    app::dependencies::RuntimeDependencies,
    cli::entrypoint::{GrantedAgents, Ungranted},
    core::RunError,
    env_serve::{
        application::{change_grant, ChangeFailed},
        domain::{AgentName, Agents, CommandGrant, GrantOperation, Peer},
        infrastructure::{
            parse_peer, peer_name, printed, FileGrantAudit, FileGrants, FilePairingEnvironment,
        },
    },
};
use nessa_protocol::agents::AgentId;
use nessa_sdk::domain::agent_execution::leases::CommandWork;
use std::{io::Write, path::Path};

/// `nessa env grant`: grant `peer` the `agents`, and commands of at most
/// `max_timeout_ms` each, beside the agents it has now. `max_timeout_ms`
/// replaces the peer's longest command run, shorter or longer.
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
    change(&directory, peer, GrantOperation::Grant { agents, commands })
}

/// `nessa env ungrant`: take `what` back from `peer`.
///
/// # Errors
/// The peer or agent is not one, an agent is taken by name from a grant
/// of every agent, or the grants could not be read or written.
pub(super) async fn ungrant(peer: Option<&str>, what: Ungranted) -> Result<(), RunError> {
    let operation = match what {
        Ungranted::Agent(name) => GrantOperation::UngrantAgent(agent_name(&name)?),
        Ungranted::Commands => GrantOperation::UngrantCommands,
        Ungranted::All => GrantOperation::UngrantAll,
    };
    let directory = environment_directory()?;
    let peer = peer_named(&directory, peer).await?;
    change(&directory, peer, operation)
}

/// `nessa env grants`: every entry, as JSON on standard output.
///
/// # Errors
/// The grants could not be read.
pub(super) fn grants() -> Result<(), RunError> {
    print(&FileGrants::new(&environment_directory()?))
}

/// Make `operation` to `peer`'s grant, audited as this account's, then
/// print every entry.
fn change(directory: &Path, peer: Peer, operation: GrantOperation) -> Result<(), RunError> {
    let grants = FileGrants::new(directory);
    change_grant(
        &grants,
        &FileGrantAudit::new(directory),
        &SystemClock,
        &local_account_id(),
        peer,
        operation,
    )
    .map_err(|failure| {
        RunError::Agent(match failure {
            ChangeFailed::AllAgents => format!(
                "{} is granted every agent: ungrant --all, then grant the agents it keeps",
                peer_name(&peer)
            ),
            ChangeFailed::Store(reason) => reason,
            ChangeFailed::Audit(reason) => {
                format!("the change was not made, since it could not be audited: {reason}")
            }
            ChangeFailed::NotDurable(reason) => {
                format!("the change is written but not yet durable: {reason}")
            }
            ChangeFailed::Unconfirmed(reason) => {
                format!("the change is in force, but its confirmation was not recorded: {reason}")
            }
        })
    })?;
    print(&grants)
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
        return parse_peer(name).map_err(|_| {
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
