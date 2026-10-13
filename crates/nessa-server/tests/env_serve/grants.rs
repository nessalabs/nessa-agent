//! What a peer is granted, and the terms of a lease admitted under it: a
//! command's timeout narrowed, never widened, and a lease's longest.
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

/// A peer with no entry is shown as, and changed from, everything for SSH
/// and nothing for a dialed gateway. Whether it may lease is the lease
/// policy's (`lease_policy.rs`), held to the same answer there.
#[test]
fn a_peer_without_an_entry_is_shown_as_its_defaults() {
    assert_eq!(unwritten(&Peer::Ssh), PeerGrant::everything());
    assert_eq!(
        unwritten(&Peer::Gateway { pin: pin(1) }),
        PeerGrant::nothing()
    );
}

/// A harness lease lasts at most the grant's `maxLeaseMs`, or until ended.
#[test]
fn a_lease_lasts_at_most_its_grants_max_lease() {
    assert_eq!(lease_ms(&PeerGrant::everything()), None);
    let limited =
        PeerGrant::new(named(&["claude"]), None).with_max_lease(MaxLease::new(3_600_000).unwrap());
    assert_eq!(lease_ms(&limited), Some(3_600_000));
}

/// A command runs for the timeout asked or the grant's longest when that is
/// shorter: narrowed, never widened; a grant without commands has no terms.
#[test]
fn a_command_timeout_is_narrowed_and_never_widened() {
    let grant = PeerGrant::new(named(&[]), Some(CommandGrant::new(60_000).unwrap()));
    assert_eq!(command_timeout(&grant, 120_000), Some(60_000));
    assert_eq!(command_timeout(&grant, 60_000), Some(60_000));
    assert_eq!(command_timeout(&grant, 5_000), Some(5_000));
    assert_eq!(command_timeout(&PeerGrant::nothing(), 5_000), None);
    assert_eq!(
        command_timeout(&PeerGrant::everything(), CommandWork::MAX_TIMEOUT_MS),
        Some(CommandWork::MAX_TIMEOUT_MS)
    );
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
