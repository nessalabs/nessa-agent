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

#[test]
fn an_unknown_name_is_never_resolved_to_some_other_agent() {
    for name in ["", "Claude", "codex-acp", "gpt", " codex"] {
        assert_eq!(AgentId::parse(name), None, "{name:?}");
    }
}
