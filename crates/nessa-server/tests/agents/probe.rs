//! What this machine answers, and what it admits it cannot answer.
//!
//! Every input is given to the probe as data, so none of these tests reads the
//! developer's own home directory, credentials, or keychain. The keychain is
//! only ever reached when the environment and the file have both said no, and
//! no test here drives it there, because its answer belongs to the host.
//!
//! What makes a credentials file a sign-in is shared and tested in
//! `credentials.rs`; where each agent keeps one is tested beside that agent.
//! These tests are about the order the sources are asked in and what an
//! unanswered source does to the whole answer.

use super::*;
use std::path::Path;
use tempfile::TempDir;

/// A probe told exactly what composition resolved and nothing more.
///
/// Built for one agent at a time: the probe answers per agent, and a test that
/// filled in both would not say which agent's sources produced the answer.
fn agent_probe(
    agent: AgentId,
    launch_files: Option<AgentLaunchFiles>,
    credentials: Option<&Path>,
    credential: Option<&str>,
    keychain: Option<fn() -> Result<bool, ProbeFailure>>,
) -> LocalAgentProbe {
    LocalAgentProbe {
        launch_files: launch_files
            .into_iter()
            .map(|files| (agent, files))
            .collect(),
        sign_in: HashMap::from([(
            agent,
            SignIn {
                environment: credential.map(str::to_owned),
                credentials: credentials.map(Path::to_path_buf),
                keychain,
            },
        )]),
    }
}

/// The Claude probe, which is the one with three sources to fall through.
fn probe(
    launch_files: Option<AgentLaunchFiles>,
    config: Option<&Path>,
    credential: Option<&str>,
) -> LocalAgentProbe {
    agent_probe(
        AgentId::Claude,
        launch_files,
        config
            .map(|config| config.join(".credentials.json"))
            .as_deref(),
        credential,
        Some(claude::keychain_sign_in),
    )
}

/// The pair of paths composition would resolve for an agent rooted at `root`,
/// whether or not anything has been written there yet.
fn launch_files(root: &Path) -> Option<AgentLaunchFiles> {
    Some(AgentLaunchFiles {
        runtime: root.join("node"),
        entry: root.join("acp-entry.js"),
    })
}

/// Put both files in place, as installing the agent would.
fn install(root: &Path) {
    std::fs::write(root.join("node"), b"#!/bin/sh\n").unwrap();
    std::fs::write(root.join("acp-entry.js"), b"// entry\n").unwrap();
}

#[test]
fn a_machine_with_no_agent_configured_at_all_is_a_real_no() {
    // Nothing to launch is an answer, not a failure to look: composition asked
    // the configuration and the configuration said there is no agent.
    assert_eq!(
        probe(None, None, None).installed(AgentId::Claude),
        Ok(false)
    );
}

#[test]
fn a_configured_agent_is_installed_wherever_this_executable_happens_to_live() {
    // The old probe answered this by looking for a `claude-acp` directory beside
    // the running binary. That binary is the test runner, which has no such
    // sibling, and a plain server deployment has no such sibling either — yet
    // both can have a perfectly good agent configured. What composition
    // resolved is the answer; where this process happens to live is not.
    let sibling = std::env::current_exe()
        .ok()
        .and_then(|executable| executable.parent().map(|parent| parent.join("claude-acp")));
    assert!(
        sibling.is_some_and(|path| !path.is_dir()),
        "this test only means something where the old heuristic would have said no"
    );
    let root = TempDir::new().unwrap();
    install(root.path());
    assert_eq!(
        probe(launch_files(root.path()), None, None).installed(AgentId::Claude),
        Ok(true)
    );
}

#[test]
fn a_configured_agent_missing_its_files_is_not_installed() {
    let root = TempDir::new().unwrap();
    assert_eq!(
        probe(launch_files(root.path()), None, None).installed(AgentId::Claude),
        Ok(false)
    );
    // Half an install is not an install: the launcher needs both files.
    std::fs::write(root.path().join("node"), b"#!/bin/sh\n").unwrap();
    assert_eq!(
        probe(launch_files(root.path()), None, None).installed(AgentId::Claude),
        Ok(false)
    );
}

#[test]
fn installing_the_agent_while_the_server_runs_changes_the_answer() {
    // The regression this file exists for. Setup's "check again" is for the
    // user who was not ready when it opened, and installing the agent is the
    // most obvious way to become ready. One probe, built before the files
    // existed, asked twice — the second answer must reflect the machine as it
    // is now, not as it was when this process started.
    let root = TempDir::new().unwrap();
    let probe = probe(launch_files(root.path()), None, None);
    assert_eq!(probe.installed(AgentId::Claude), Ok(false));
    install(root.path());
    assert_eq!(probe.installed(AgentId::Claude), Ok(true));
}

#[test]
fn a_directory_where_a_file_belongs_is_not_an_installed_agent() {
    let root = TempDir::new().unwrap();
    install(root.path());
    std::fs::remove_file(root.path().join("node")).unwrap();
    std::fs::create_dir(root.path().join("node")).unwrap();
    assert_eq!(
        probe(launch_files(root.path()), None, None).installed(AgentId::Claude),
        Ok(false)
    );
}

/// Making a path unreadable needs Unix permissions, and root ignores them.
#[cfg(unix)]
mod when_the_path_cannot_be_read {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn an_unreadable_path_leaves_the_answer_undetermined_rather_than_no() {
        // "I am not allowed to look" is not "the agent is not installed".
        // Reported as a failure so setup does not offer an install to a user
        // who already has one.
        let root = TempDir::new().unwrap();
        let sealed = root.path().join("sealed");
        std::fs::create_dir(&sealed).unwrap();
        install(&sealed);
        std::fs::set_permissions(&sealed, std::fs::Permissions::from_mode(0o000)).unwrap();
        // Root is refused nothing, so on such a host there is no failure to see.
        let enforced = std::fs::read_dir(&sealed).is_err();
        let answer = probe(launch_files(&sealed), None, None).installed(AgentId::Claude);
        // Restore before asserting so the temporary directory can be removed.
        std::fs::set_permissions(&sealed, std::fs::Permissions::from_mode(0o700)).unwrap();
        if enforced {
            assert_eq!(answer, Err(ProbeFailure::Unanswered));
        }
    }
}

#[test]
fn nowhere_to_look_for_a_credentials_file_is_not_the_same_as_not_finding_one() {
    // No CLAUDE_CONFIG_DIR and no home directory: the question was never asked,
    // and with the keychain the only source left, a host without one has
    // nothing to answer from.
    assert_eq!(
        agent_probe(AgentId::Claude, None, None, None, None).authenticated(AgentId::Claude),
        Err(ProbeFailure::NothingToAsk)
    );
}

#[test]
fn an_agent_this_server_knows_nothing_about_is_a_question_it_cannot_answer() {
    // Not a no: reporting "signed out" for an agent whose sources were never
    // resolved would send someone to sign in to something already signed in.
    let claude_only = agent_probe(AgentId::Claude, None, None, Some("key"), None);
    assert_eq!(
        claude_only.authenticated(AgentId::Codex),
        Err(ProbeFailure::NothingToAsk)
    );
    // Installation is different: the configuration was asked, and it says there
    // is nothing to launch.
    assert_eq!(claude_only.installed(AgentId::Codex), Ok(false));
}

#[test]
fn an_agent_with_no_keychain_is_answered_from_its_two_sources_alone() {
    // Codex keeps a ChatGPT login in the same file as an API key, so there is
    // no third source — and a source that does not exist for this agent is not
    // one that failed to answer.
    let config = TempDir::new().unwrap();
    let credentials = config.path().join("auth.json");
    assert_eq!(
        agent_probe(AgentId::Codex, None, Some(&credentials), None, None)
            .authenticated(AgentId::Codex),
        Ok(false)
    );
    std::fs::write(&credentials, b"{\"tokens\":{\"id\":\"x\"}}").unwrap();
    assert_eq!(
        agent_probe(AgentId::Codex, None, Some(&credentials), None, None)
            .authenticated(AgentId::Codex),
        Ok(true)
    );
}

#[test]
fn a_credentials_file_settles_the_question_before_the_keychain_is_asked() {
    let config = TempDir::new().unwrap();
    std::fs::write(
        config.path().join(".credentials.json"),
        b"{\"token\":\"secret\"}",
    )
    .unwrap();
    assert_eq!(
        probe(None, Some(config.path()), None).authenticated(AgentId::Claude),
        Ok(true)
    );
}

#[test]
fn an_api_key_in_the_environment_answers_before_anything_is_looked_at() {
    // A machine account signs in this way; no file and no keychain is consulted.
    assert_eq!(
        probe(None, None, Some("key")).authenticated(AgentId::Claude),
        Ok(true)
    );
}

/// On a host with a keychain the last source is the machine's own, and its
/// answer is not the test's to fix. These two cover the fall-through where it
/// can be run honestly.
#[cfg(not(target_os = "macos"))]
mod without_a_keychain {
    use super::*;

    #[test]
    fn a_host_with_no_keychain_answers_from_the_file_and_the_environment_alone() {
        let config = TempDir::new().unwrap();
        assert_eq!(
            probe(None, Some(config.path()), None).authenticated(AgentId::Claude),
            Ok(false)
        );
    }

    #[test]
    fn with_nowhere_to_look_at_all_the_answer_is_undetermined_rather_than_no() {
        assert_eq!(
            probe(None, None, None).authenticated(AgentId::Claude),
            Err(ProbeFailure::NothingToAsk)
        );
    }
}
