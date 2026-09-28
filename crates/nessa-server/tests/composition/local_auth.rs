use super::*;

/// A configuration naming both agents, with no file on disk for either.
///
/// Nothing here needs to exist: these are the paths the probe would stat, and
/// this is the code that decides which agents it gets to stat at all.
fn both_agents() -> AgentsConfig {
    let runtimes: serde_json::Map<String, serde_json::Value> = [AgentId::Claude, AgentId::Opencode]
        .into_iter()
        .map(|agent| {
            (
                agent.name().to_owned(),
                serde_json::json!({
                    "command": format!("/{}", agent.name()),
                    "args": [],
                    "model": "configured-model",
                    "toolsEnabled": true,
                }),
            )
        })
        .collect();
    serde_json::from_value(serde_json::json!({
        "catalog": "/catalog.json",
        "workspace": "/workspace",
        "selected": "claude",
        "runtimes": runtimes,
    }))
    .unwrap()
}

/// An agent the gateway could not start, with the agent already installed, is
/// not one the probe is asked about.
///
/// `providers` leaves an agent out for either of two reasons and only one of
/// them is a fact about this installation. A command nobody has installed yet
/// is a command somebody can install, and the probe re-stats it on every ask so
/// that setup keeps offering the button. A provider that could not be built
/// with everything already on disk failed on something no install re-asks, and
/// letting the probe stat that command would have it answer `ready` for an
/// agent no conversation can be opened on.
#[test]
fn an_agent_this_run_cannot_start_is_not_one_readiness_answers_for() {
    let config = both_agents();

    let all = launch_files(Some(&config), &HashSet::new());
    assert_eq!(all.len(), 2);
    assert_eq!(
        all[&AgentId::Opencode].command,
        Path::new("/opencode"),
        "an agent nobody has installed is still the probe's to answer for"
    );

    let narrowed = launch_files(Some(&config), &HashSet::from([AgentId::Opencode]));
    assert!(!narrowed.contains_key(&AgentId::Opencode));
    assert!(
        narrowed.contains_key(&AgentId::Claude),
        "and the agents that did build are untouched"
    );
}

/// No agents configured is no agents to answer for, which is not the same
/// answer as an agent that is configured and unstartable — it is setup having
/// nothing to list.
#[test]
fn a_gateway_configured_with_no_agents_answers_for_none() {
    assert!(launch_files(None, &HashSet::new()).is_empty());
}

#[test]
fn agent_catalog_uses_binding_choices_for_each_catalog_model() {
    let catalog = Path::new(env!("CARGO_MANIFEST_DIR")).join("../nessa-sdk/data/models.json");
    let config: AgentsConfig = serde_json::from_value(serde_json::json!({
        "catalog": catalog,
        "workspace": "/workspace",
        "selected": "claude",
        "runtimes": {
            "claude": {"command": "/claude", "model": "claude-sonnet-5", "toolsEnabled": true},
            "codex": {"command": "/codex", "model": "gpt-6-astra", "toolsEnabled": true}
        }
    }))
    .unwrap();
    let offered = agent_catalog(
        &config,
        &HashSet::from([AgentId::Claude, AgentId::Codex]),
        None,
    )
    .unwrap();
    assert_eq!(offered.agents.len(), 2);
    for agent in &offered.agents {
        let selected = agent
            .models
            .iter()
            .find(|model| model.model_id == agent.default_model)
            .unwrap();
        assert_eq!(selected.approval_modes.len(), 3);
        assert_eq!(selected.approval_modes[0].id, WireApprovalMode::Ask);
        assert_eq!(selected.approval_modes[1].id, WireApprovalMode::Auto);
        assert_eq!(selected.approval_modes[2].id, WireApprovalMode::Full);
    }
    assert_eq!(offered.agents[0].agent, "claude");
    assert_eq!(offered.agents[1].agent, "codex");
    for agent in &offered.agents {
        for model in &agent.models {
            let ids: Vec<_> = model
                .approval_modes
                .iter()
                .map(|choice| choice.id)
                .collect();
            let expected = match model.model_id.as_str() {
                "claude-haiku-4-5-20251001" => vec![WireApprovalMode::Ask, WireApprovalMode::Full],
                _ => vec![
                    WireApprovalMode::Ask,
                    WireApprovalMode::Auto,
                    WireApprovalMode::Full,
                ],
            };
            assert_eq!(ids, expected, "{} {}", agent.agent, model.model_id);
        }
    }
}
