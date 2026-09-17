//! What this machine answers, and what it admits it cannot answer.
//!
//! Every input is given to the probe as data, so none of these tests reads the
//! developer's own home directory, credentials, or keychain. The keychain is
//! only ever reached when the environment and the file have both said no, and
//! no test here drives it there, because its answer belongs to the host.
//!
//! What makes a credentials file a sign-in is Claude's own, and is tested
//! beside it in `claude.rs`. These tests are about the order the sources are
//! asked in and what an unanswered source does to the whole answer.

use super::*;
use std::path::Path;
use tempfile::TempDir;

/// A probe told exactly what composition resolved and nothing more.
fn probe(installed: bool, config: Option<&Path>, credential: Option<&str>) -> LocalAgentProbe {
    LocalAgentProbe {
        claude_installed: installed,
        environment_credential: credential.map(str::to_owned),
        claude_config_directory: config.map(Path::to_path_buf),
    }
}

#[test]
fn a_machine_with_no_agent_configured_at_all_is_a_real_no() {
    // Nothing to launch is an answer, not a failure to look: composition asked
    // the configuration and the configuration said there is no agent.
    assert_eq!(
        probe(false, None, None).installed(AgentId::Claude),
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
    assert_eq!(probe(true, None, None).installed(AgentId::Claude), Ok(true));
}

#[test]
fn nowhere_to_look_for_a_credentials_file_is_not_the_same_as_not_finding_one() {
    // No CLAUDE_CONFIG_DIR and no home directory: the question was never asked.
    assert_eq!(
        probe(false, None, None).claude_credentials_file(),
        Err(ProbeFailure::NothingToAsk)
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
        probe(false, Some(config.path()), None).claude_credentials_file(),
        Ok(true)
    );
    assert_eq!(
        probe(false, Some(config.path()), None).authenticated(AgentId::Claude),
        Ok(true)
    );
}

#[test]
fn an_api_key_in_the_environment_answers_before_anything_is_looked_at() {
    // A machine account signs in this way; no file and no keychain is consulted.
    assert_eq!(
        probe(false, None, Some("key")).authenticated(AgentId::Claude),
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
            probe(false, Some(config.path()), None).authenticated(AgentId::Claude),
            Ok(false)
        );
    }

    #[test]
    fn with_nowhere_to_look_at_all_the_answer_is_undetermined_rather_than_no() {
        assert_eq!(
            probe(false, None, None).authenticated(AgentId::Claude),
            Err(ProbeFailure::NothingToAsk)
        );
    }
}
