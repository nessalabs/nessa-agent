//! What this machine answers, and what it admits it cannot answer.
//!
//! Every input is given to the probe as data, so none of these tests reads the
//! developer's own home directory, credentials, or keychain. The keychain is
//! only ever reached when the environment and the file have both said no, and
//! no test here drives it there, because its answer belongs to the host.

use super::*;
use tempfile::TempDir;

/// A probe told exactly where to look and nothing more.
fn probe(runtime_root: Option<&Path>, config: Option<&Path>, key: Option<&str>) -> LocalAgentProbe {
    LocalAgentProbe {
        runtime_root: runtime_root.map(Path::to_path_buf),
        anthropic_api_key: key.map(str::to_owned),
        claude_config_directory: config.map(Path::to_path_buf),
    }
}

#[test]
fn a_machine_that_cannot_say_where_it_is_installed_says_so() {
    // Not "the agent is missing" — the probe could not find out either way.
    assert_eq!(
        probe(None, None, None).installed(AgentId::Claude),
        Err(ProbeFailure::NothingToAsk)
    );
}

#[test]
fn an_adapter_directory_beside_the_server_is_a_real_yes_and_its_absence_a_real_no() {
    let root = TempDir::new().unwrap();
    assert_eq!(
        probe(Some(root.path()), None, None).installed(AgentId::Claude),
        Ok(false)
    );
    std::fs::create_dir(root.path().join("claude-acp")).unwrap();
    assert_eq!(
        probe(Some(root.path()), None, None).installed(AgentId::Claude),
        Ok(true)
    );
}

#[test]
fn a_file_where_the_adapter_should_be_is_not_an_installed_adapter() {
    let root = TempDir::new().unwrap();
    std::fs::write(root.path().join("claude-acp"), b"not a directory").unwrap();
    assert_eq!(
        probe(Some(root.path()), None, None).installed(AgentId::Claude),
        Ok(false)
    );
}

#[test]
fn nowhere_to_look_for_a_credentials_file_is_not_the_same_as_not_finding_one() {
    // No CLAUDE_CONFIG_DIR and no home directory: the question was never asked.
    assert_eq!(
        probe(None, None, None).claude_credentials_file(),
        Err(ProbeFailure::NothingToAsk)
    );
}

#[test]
fn a_missing_or_empty_credentials_file_is_a_real_no() {
    let config = TempDir::new().unwrap();
    assert_eq!(
        probe(None, Some(config.path()), None).claude_credentials_file(),
        Ok(false)
    );
    std::fs::write(config.path().join(".credentials.json"), b"").unwrap();
    assert_eq!(
        probe(None, Some(config.path()), None).claude_credentials_file(),
        Ok(false)
    );
}

#[test]
fn a_credentials_file_with_contents_is_a_yes_without_being_read() {
    let config = TempDir::new().unwrap();
    std::fs::write(
        config.path().join(".credentials.json"),
        b"{\"token\":\"secret\"}",
    )
    .unwrap();
    assert_eq!(
        probe(None, Some(config.path()), None).claude_credentials_file(),
        Ok(true)
    );
    // And it settles the whole question, so nothing asks the keychain.
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
