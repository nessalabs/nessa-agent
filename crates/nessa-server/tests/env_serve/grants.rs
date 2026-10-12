//! What a peer is granted, and the one decision made from it: whether a
//! lease is admitted, and a command's timeout narrowed, never widened.
use super::*;

fn named(agents: &[&str]) -> Agents {
    Agents::Named(
        agents
            .iter()
            .map(|agent| AgentName::new(agent).unwrap())
            .collect(),
    )
}

fn pin(byte: u8) -> [u8; 44] {
    [byte; 44]
}

/// Without an entry, SSH keeps what a host served over SSH always had and
/// a dialed gateway is granted nothing; an entry is what it says.
#[test]
fn a_peer_without_an_entry_has_its_defaults() {
    assert_eq!(granted_to(&Peer::Ssh, None), PeerGrant::everything());
    assert_eq!(
        granted_to(&Peer::Gateway { pin: pin(1) }, None),
        PeerGrant::nothing()
    );
    let entry = PeerGrant::new(named(&["claude"]), None);
    assert_eq!(granted_to(&Peer::Ssh, Some(entry.clone())), entry);
    assert_eq!(
        granted_to(&Peer::Gateway { pin: pin(1) }, Some(entry.clone())),
        entry
    );
}

/// An agent is admitted only when its grant names it or grants every agent.
#[test]
fn an_agent_is_admitted_only_when_granted() {
    let some = PeerGrant::new(named(&["claude"]), None);
    assert_eq!(admit_agent(&some, "claude"), Ok(None));
    assert_eq!(admit_agent(&some, "codex"), Err(NotGranted));
    assert_eq!(
        admit_agent(&PeerGrant::nothing(), "claude"),
        Err(NotGranted)
    );
    assert_eq!(admit_agent(&PeerGrant::everything(), "codex"), Ok(None));
}

/// A command is admitted only with commands granted, for the timeout asked
/// or the grant's longest when that is shorter: narrowed, never widened.
#[test]
fn a_command_timeout_is_narrowed_and_never_widened() {
    let grant = PeerGrant::new(named(&[]), Some(CommandGrant::new(60_000).unwrap()));
    assert_eq!(admit_command(&grant, 120_000), Ok(60_000));
    assert_eq!(admit_command(&grant, 60_000), Ok(60_000));
    assert_eq!(admit_command(&grant, 5_000), Ok(5_000));
    assert_eq!(admit_command(&PeerGrant::nothing(), 5_000), Err(NotGranted));
    assert_eq!(
        admit_command(&PeerGrant::everything(), CommandWork::MAX_TIMEOUT_MS),
        Ok(CommandWork::MAX_TIMEOUT_MS)
    );
    // Commands do not grant agents.
    assert_eq!(admit_agent(&grant, "claude"), Err(NotGranted));
}

/// A grant holds only what it can: a command timeout from one millisecond
/// to the longest a command runs, and agent names that are names.
#[test]
fn a_grant_holds_only_bounded_values() {
    assert_eq!(CommandGrant::new(0), Err(InvalidGrant::MaxTimeout));
    assert_eq!(
        CommandGrant::new(CommandWork::MAX_TIMEOUT_MS + 1),
        Err(InvalidGrant::MaxTimeout)
    );
    assert!(CommandGrant::new(1).is_ok());
    for refused in ["", " ", "*", &"a".repeat(AgentWork::MAX_AGENT_BYTES + 1)] {
        assert_eq!(AgentName::new(refused), Err(InvalidGrant::AgentName));
    }
}

/// Granting adds to a grant and ungranting takes from it, each a new
/// value; every agent stays every agent, and none is taken from it by name.
#[test]
fn granting_adds_and_ungranting_takes_away() {
    let grant = PeerGrant::nothing()
        .with_agents(named(&["claude"]))
        .with_agents(named(&["codex"]))
        .with_commands(CommandGrant::new(1_000).unwrap());
    assert_eq!(grant.agents(), &named(&["claude", "codex"]));
    assert_eq!(grant.commands(), Some(CommandGrant::new(1_000).unwrap()));
    let fewer = grant
        .clone()
        .without_agent(&AgentName::new("claude").unwrap())
        .unwrap()
        .without_commands();
    assert_eq!(fewer, PeerGrant::new(named(&["codex"]), None));
    let all = grant
        .with_agents(Agents::All)
        .with_agents(named(&["claude"]));
    assert_eq!(all.agents(), &Agents::All);
    assert_eq!(
        all.without_agent(&AgentName::new("claude").unwrap()),
        Err(AllAgents)
    );
}
