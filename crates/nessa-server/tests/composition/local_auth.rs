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
    nessa_local_storage::create_directory(&root.path().join("private")).unwrap();
    let root_path = root.path().join("private");
    let root = &root_path;
    // Nothing from an earlier build: conversations start, over a database.
    assert!(metadata_store(root).is_ok());
    std::fs::remove_file(root.join("metadata.sqlite3")).unwrap();
    // Either directory, even empty — a move interrupted after its last file —
    // refuses, naming it and the script, until the move has finished.
    for name in ["metadata", "summaries"] {
        let directory = root.join(name);
        std::fs::create_dir(&directory).unwrap();
        let Err(RunError::Agent(message)) = metadata_store(root) else {
            panic!("{name} was not refused");
        };
        // Refused before any database is made beside the files.
        assert!(!root.join("metadata.sqlite3").exists(), "{name}");
        assert!(
            message.contains(&directory.display().to_string()),
            "{message}"
        );
        // Naming the directory to move, so the script moves this one.
        assert!(
            message.contains(&format!(
                "scripts/move-conversation-metadata.mjs {}",
                root.display()
            )),
            "{message}"
        );
        std::fs::remove_dir(&directory).unwrap();
    }
    assert!(metadata_store(root).is_ok());
}

#[cfg(unix)]
#[test]
fn a_root_whose_contents_cannot_be_looked_at_is_refused_not_taken_as_empty() {
    use std::os::unix::fs::PermissionsExt;
    // Root reads through any mode, so there is nothing to refuse it.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("private");
    nessa_local_storage::create_directory(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o000)).unwrap();
    let refused = metadata_store(&root);
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let Err(RunError::Agent(message)) = refused else {
        panic!("a root nothing could be looked at in was taken as empty");
    };
    assert!(message.contains("could not tell whether"), "{message}");
    assert!(!root.join("metadata.sqlite3").exists());
}
