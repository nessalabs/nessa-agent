//! The name an agent is known by outside the server.

use super::*;

#[test]
fn every_agent_has_a_name_that_resolves_back_to_it() {
    for agent in AgentId::ALL {
        assert_eq!(AgentId::parse(agent.name()), Some(*agent), "{agent:?}");
    }
}

#[test]
fn no_two_agents_answer_to_the_same_name() {
    // A conversation record, a configuration and the wire all name agents with
    // these strings, so a collision would silently run one agent's stored work
    // on another.
    let mut names: Vec<&str> = AgentId::ALL.iter().map(|agent| agent.name()).collect();
    names.sort_unstable();
    let total = names.len();
    names.dedup();
    assert_eq!(names.len(), total, "two agents share a name");
}

/// The agents this build can drive, named rather than derived from the listing
/// itself.
///
/// An agent missing from here is one setup stops offering and one a stored
/// conversation can no longer be resumed on, and both of those are too quiet a
/// change to make by editing a single constant — the server would simply go on
/// working, about fewer agents.
#[test]
fn nessa_drives_claude_codex_and_opencode() {
    assert_eq!(
        AgentId::ALL
            .iter()
            .map(|agent| agent.name())
            .collect::<Vec<_>>(),
        ["claude", "codex", "opencode"]
    );
}

/// Whether an agent needs an account is a fact about the agent, so it is the
/// same on every host and every agent has to say which it is.
#[test]
fn only_the_agent_that_reaches_a_model_without_an_account_says_it_needs_none() {
    assert!(AgentId::Claude.needs_sign_in());
    assert!(AgentId::Codex.needs_sign_in());
    assert!(!AgentId::Opencode.needs_sign_in());
}

#[test]
fn an_unknown_name_is_never_resolved_to_some_other_agent() {
    for name in ["", "Claude", "codex-acp", "gpt", " codex"] {
        assert_eq!(AgentId::parse(name), None, "{name:?}");
    }
}
