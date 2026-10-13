//! The lease policy as Cedar decides it: the checked-in bundle validates,
//! each rule answers as the grants file means it, a refusal names its rule,
//! and anything the adapter cannot decide is refused.
use super::*;
use crate::env_serve::domain::{unwritten, AgentName, CommandGrant};
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

fn ask(
    policy: &CedarLeasePolicy,
    peer: &Peer,
    entry: Option<&PeerGrant>,
    ask: LeaseAsk<'_>,
) -> LeaseDecision {
    policy.evaluate(&LeaseFacts { peer, entry, ask })
}

fn allowed(decision: &LeaseDecision) -> bool {
    matches!(decision, LeaseDecision::Allow { .. })
}

#[test]
fn the_checked_in_bundle_validates() {
    policy();
}

/// SSH with no entry keeps what a host served over SSH always had: every
/// agent and commands, each by its named rule.
#[test]
fn ssh_without_an_entry_is_allowed_every_agent_and_commands() {
    let policy = policy();
    assert_eq!(
        ask(&policy, &Peer::Ssh, None, LeaseAsk::Agent("codex")),
        LeaseDecision::Allow {
            policies: vec!["ssh.unnamed.every_agent".into()]
        }
    );
    assert_eq!(
        ask(&policy, &Peer::Ssh, None, LeaseAsk::Command),
        LeaseDecision::Allow {
            policies: vec!["ssh.unnamed.commands".into()]
        }
    );
}

/// A dialed gateway its owner never named is granted nothing, and the
/// refusal names the rule.
#[test]
fn a_gateway_without_an_entry_is_denied_by_name() {
    let policy = policy();
    for asked in [LeaseAsk::Agent("claude"), LeaseAsk::Command] {
        assert_eq!(
            ask(&policy, &DIALED, None, asked),
            LeaseDecision::Deny {
                policies: vec!["unnamed.nothing".into()]
            }
        );
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
                policies: vec!["entry.named_agent".into()]
            }
        );
        assert_eq!(
            ask(&policy, &peer, Some(&some), LeaseAsk::Agent("codex")),
            LeaseDecision::Deny { policies: vec![] }
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
                policies: vec!["entry.commands".into()]
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

/// The no-entry rules answer as the grant a peer with no entry is shown as
/// would: the two statements of a peer's defaults cannot drift apart.
#[test]
fn the_no_entry_rules_answer_as_the_unwritten_grant() {
    let policy = policy();
    for peer in [Peer::Ssh, DIALED] {
        let shown = unwritten(&peer);
        for asked in [
            LeaseAsk::Agent("claude"),
            LeaseAsk::Agent("codex"),
            LeaseAsk::Command,
        ] {
            assert_eq!(
                allowed(&ask(&policy, &peer, None, asked)),
                allowed(&ask(&policy, &peer, Some(&shown), asked)),
                "{peer:?} {asked:?}"
            );
        }
    }
}

/// A bundle that does not parse, does not validate strictly, or holds a
/// policy without an `@id` is refused when it is built.
#[test]
fn a_malformed_bundle_fails_construction() {
    assert!(CedarLeasePolicy::from_sources("{", POLICIES).is_err());
    assert!(CedarLeasePolicy::from_sources(SCHEMA, "permit (").is_err());
    let unknown_attribute = r#"@id("x") permit (principal is NessaEnv::Peer, action == NessaEnv::Action::"command.run", resource) when { principal.nope };"#;
    assert!(CedarLeasePolicy::from_sources(SCHEMA, unknown_attribute).is_err());
    let unnamed = r#"permit (principal is NessaEnv::Peer, action == NessaEnv::Action::"command.run", resource is NessaEnv::Command);"#;
    assert!(CedarLeasePolicy::from_sources(SCHEMA, unnamed).is_err());
}

/// A rule that errors while it is evaluated refuses the lease, even where
/// Cedar would skip it and allow: an error is never a yes.
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
        LeaseDecision::Deny { policies: vec![] }
    );
}
