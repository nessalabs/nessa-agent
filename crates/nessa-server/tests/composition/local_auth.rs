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
fn metadata_an_earlier_build_kept_as_files_is_refused_with_what_moves_it() {
    let root = tempfile::tempdir().unwrap();
    // Nothing from an earlier build: conversations start.
    assert!(refuse_metadata_files(root.path()).is_ok());
    // Either directory, even empty — a move interrupted after its last file —
    // refuses, naming it and the script, until the move has finished.
    for name in ["metadata", "summaries"] {
        let directory = root.path().join(name);
        std::fs::create_dir(&directory).unwrap();
        let Err(RunError::Agent(message)) = refuse_metadata_files(root.path()) else {
            panic!("{name} was not refused");
        };
        assert!(
            message.contains(&directory.display().to_string()),
            "{message}"
        );
        assert!(
            message.contains("scripts/move-conversation-metadata.mjs"),
            "{message}"
        );
        std::fs::remove_dir(&directory).unwrap();
    }
    assert!(refuse_metadata_files(root.path()).is_ok());
}
