//! The lease policy as Cedar decides it: the checked-in bundle validates,
//! each rule answers as the grants file means it, over the grant a peer
//! holds whether or not its entry was written, a refusal says why, and
//! anything the adapter cannot decide is refused.
use super::*;
use crate::env_serve::domain::{unwritten, AgentName, CommandGrant, PeerGrant};
use std::collections::BTreeSet;

const DIALED: Peer = Peer::Gateway { pin: [7; 44] };

fn policy() -> CedarLeasePolicy {
    CedarLeasePolicy::new().expect("the checked-in lease policy validates")
}

fn named(agents: &[&str]) -> Agents {
    Agents::Named(
        agents
            .iter()
            .map(|agent| AgentName::new(agent).unwrap())
            .collect::<BTreeSet<_>>(),
    )
}

fn commands() -> Option<CommandGrant> {
    Some(CommandGrant::new(60_000).unwrap())
}

/// The policy asked as admission asks it: over `entry`, or, with none, the
/// grant `domain::unwritten` says a peer with no entry holds.
fn ask(
    policy: &CedarLeasePolicy,
    peer: &Peer,
    entry: Option<&PeerGrant>,
    ask: LeaseAsk<'_>,
) -> LeaseDecision {
    let grant = entry.cloned().unwrap_or_else(|| unwritten(peer));
    policy.evaluate(&LeaseFacts {
        peer,
        written: entry.is_some(),
        grant: &grant,
        ask,
    })
}

fn allowed(decision: &LeaseDecision) -> bool {
    matches!(decision, LeaseDecision::Allow { .. })
}

#[test]
fn the_checked_in_bundle_validates() {
    policy();
}

/// SSH with no entry keeps what a host served over SSH always had: every
/// agent and commands, by the grant's own permits over what it holds.
#[test]
fn ssh_without_an_entry_is_allowed_every_agent_and_commands() {
    let policy = policy();
    assert_eq!(
        ask(&policy, &Peer::Ssh, None, LeaseAsk::Agent("codex")),
        LeaseDecision::Allow {
            policies: vec!["grant.every_agent".into()]
        }
    );
    assert_eq!(
        ask(&policy, &Peer::Ssh, None, LeaseAsk::Command),
        LeaseDecision::Allow {
            policies: vec!["grant.commands".into()]
        }
    );
}

/// A dialed gateway its owner never named holds nothing, so no permit
/// allows it anything, and the refusal says so.
#[test]
fn a_gateway_without_an_entry_is_not_permitted() {
    let policy = policy();
    for asked in [
        LeaseAsk::Agent("claude"),
        LeaseAsk::Command,
        LeaseAsk::Update,
    ] {
        assert_eq!(
            ask(&policy, &DIALED, None, asked),
            LeaseDecision::Deny(Denial::NotPermitted)
        );
    }
}

/// The bundle states no defaults: a peer with no entry is decided exactly
/// as one whose entry is the grant it holds, so a bundle edit cannot give
/// a peer with no entry a lease whose terms are another grant's.
#[test]
fn whether_an_entry_was_written_does_not_decide() {
    let policy = policy();
    for peer in [Peer::Ssh, DIALED] {
        let held = unwritten(&peer);
        for asked in [
            LeaseAsk::Agent("claude"),
            LeaseAsk::Command,
            LeaseAsk::Update,
        ] {
            assert_eq!(
                ask(&policy, &peer, None, asked),
                ask(&policy, &peer, Some(&held), asked),
                "{peer:?} {asked:?}"
            );
        }
    }
}

/// An entry admits the agents it names, or every agent, and no other; for
/// SSH as for a dialed gateway.
#[test]
fn an_entry_allows_its_named_agents_only() {
    let policy = policy();
    let some = PeerGrant::new(named(&["claude"]), None);
    let all = PeerGrant::new(Agents::All, None);
    for peer in [Peer::Ssh, DIALED] {
        assert_eq!(
            ask(&policy, &peer, Some(&some), LeaseAsk::Agent("claude")),
            LeaseDecision::Allow {
                policies: vec!["grant.named_agent".into()]
            }
        );
        assert_eq!(
            ask(&policy, &peer, Some(&some), LeaseAsk::Agent("codex")),
            LeaseDecision::Deny(Denial::NotPermitted)
        );
        assert!(allowed(&ask(
            &policy,
            &peer,
            Some(&all),
            LeaseAsk::Agent("codex")
        )));
        assert!(!allowed(&ask(
            &policy,
            &peer,
            Some(&PeerGrant::nothing()),
            LeaseAsk::Agent("claude")
        )));
    }
}

/// Commands are allowed only by an entry granting them; agents do not
/// grant commands, nor commands agents.
#[test]
fn an_entry_allows_commands_only_when_it_grants_them() {
    let policy = policy();
    let with = PeerGrant::new(named(&[]), commands());
    let without = PeerGrant::new(Agents::All, None);
    for peer in [Peer::Ssh, DIALED] {
        assert_eq!(
            ask(&policy, &peer, Some(&with), LeaseAsk::Command),
            LeaseDecision::Allow {
                policies: vec!["grant.commands".into()]
            }
        );
        assert!(!allowed(&ask(
            &policy,
            &peer,
            Some(&without),
            LeaseAsk::Command
        )));
        assert!(!allowed(&ask(
            &policy,
            &peer,
            Some(&with),
            LeaseAsk::Agent("claude")
        )));
    }
}

/// A bundle that does not parse, does not validate strictly, or holds a
/// policy without an `@id` is refused when it is built.
/// A peer may replace this build with its gateway's only when it may run
/// commands here, which could replace it anyway: agents alone, whose
/// harnesses may be sandboxed, never let it.
#[test]
fn an_update_is_allowed_only_with_commands() {
    let policy = policy();
    for peer in [Peer::Ssh, DIALED] {
        assert_eq!(
            ask(
                &policy,
                &peer,
                Some(&PeerGrant::new(named(&[]), commands())),
                LeaseAsk::Update
            ),
            LeaseDecision::Allow {
                policies: vec!["grant.commands_update".into()]
            }
        );
        assert!(!allowed(&ask(
            &policy,
            &peer,
            Some(&PeerGrant::new(Agents::All, None)),
            LeaseAsk::Update
        )));
    }
    assert!(allowed(&ask(&policy, &Peer::Ssh, None, LeaseAsk::Update)));
}

#[test]
fn a_malformed_bundle_fails_construction() {
    assert!(CedarLeasePolicy::from_sources("{", POLICIES).is_err());
    assert!(CedarLeasePolicy::from_sources(SCHEMA, "permit (").is_err());
    let unknown_attribute = r#"@id("x") permit (principal is NessaEnv::Peer, action == NessaEnv::Action::"command.run", resource) when { principal.nope };"#;
    assert!(CedarLeasePolicy::from_sources(SCHEMA, unknown_attribute).is_err());
    let unnamed = r#"permit (principal is NessaEnv::Peer, action == NessaEnv::Action::"command.run", resource is NessaEnv::Command);"#;
    assert!(CedarLeasePolicy::from_sources(SCHEMA, unnamed).is_err());
}

/// A rule that errors while it is evaluated refuses the lease as
/// unavailable, even where Cedar would skip it and allow: an error is never
/// a yes, nor taken for a policy's no.
#[test]
fn an_evaluation_error_fails_closed() {
    let bundle = r#"
        @id("allow")
        permit (principal is NessaEnv::Peer, action == NessaEnv::Action::"command.run", resource is NessaEnv::Command);
        @id("overflows")
        forbid (principal is NessaEnv::Peer, action, resource)
        when { 9223372036854775807 + 1 > 0 };
    "#;
    let policy = CedarLeasePolicy::from_sources(SCHEMA, bundle).expect("validates");
    assert_eq!(
        ask(&policy, &Peer::Ssh, None, LeaseAsk::Command),
        LeaseDecision::Deny(Denial::Unavailable)
    );
}

/// A forbid that matches is named in the refusal.
#[test]
fn a_forbid_is_named_in_the_refusal() {
    let bundle = r#"
        @id("allow")
        permit (principal is NessaEnv::Peer, action == NessaEnv::Action::"command.run", resource is NessaEnv::Command);
        @id("no.commands")
        forbid (principal is NessaEnv::Peer, action == NessaEnv::Action::"command.run", resource);
    "#;
    let policy = CedarLeasePolicy::from_sources(SCHEMA, bundle).expect("validates");
    assert_eq!(
        ask(&policy, &Peer::Ssh, None, LeaseAsk::Command),
        LeaseDecision::Deny(Denial::Forbidden {
            policies: vec!["no.commands".into()]
        })
    );
}
