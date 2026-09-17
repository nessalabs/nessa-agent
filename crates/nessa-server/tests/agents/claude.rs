//! What Claude Code's own sign-in conventions answer, given a directory.
//!
//! Every input is handed in as a path to a temporary directory, so none of
//! these tests reads the developer's own home directory, credentials, or
//! keychain. The keychain is the host's to answer and is never driven here.

use super::*;
use tempfile::TempDir;

/// A config directory holding exactly the given credentials file contents.
fn config_with(contents: &[u8]) -> TempDir {
    let config = TempDir::new().unwrap();
    std::fs::write(config.path().join(CREDENTIALS_FILE), contents).unwrap();
    config
}

#[test]
fn a_missing_credentials_file_is_a_real_no() {
    let config = TempDir::new().unwrap();
    assert_eq!(credentials_file(config.path()), Ok(false));
}

#[test]
fn an_empty_credentials_file_is_a_real_no() {
    let config = config_with(b"");
    assert_eq!(credentials_file(config.path()), Ok(false));
}

#[test]
fn an_empty_json_object_is_a_real_no() {
    // What a crashed or half-finished write leaves behind. It has a byte count,
    // which used to be the whole test, but it carries no credential at all.
    let config = config_with(b"{}");
    assert_eq!(credentials_file(config.path()), Ok(false));
}

#[test]
fn a_truncated_credentials_file_is_a_real_no() {
    // Claude Code could not sign in with this either, so neither can the probe
    // report it as a sign-in.
    let config = config_with(b"{\"foo\": ");
    assert_eq!(credentials_file(config.path()), Ok(false));
    let stray = config_with(b"\0");
    assert_eq!(credentials_file(stray.path()), Ok(false));
}

#[test]
fn json_that_is_not_an_object_is_a_real_no() {
    // A credential is a set of named fields whatever Anthropic calls them; none
    // of these could be one.
    for contents in [
        b"null".as_slice(),
        b"true".as_slice(),
        b"7".as_slice(),
        b"\"token\"".as_slice(),
        b"[{\"anything\": \"here\"}]".as_slice(),
    ] {
        let config = config_with(contents);
        assert_eq!(
            credentials_file(config.path()),
            Ok(false),
            "{} is not a credentials object",
            String::from_utf8_lossy(contents)
        );
    }
}

#[test]
fn any_nonempty_json_object_is_a_yes_without_being_interpreted() {
    // The check is structural on purpose. This repository does not know the
    // real field names, so requiring one would reject valid credentials the day
    // Anthropic renames it. An object with contents is as much as can honestly
    // be asserted, and it is enough to rule out the corrupted cases above.
    let config = config_with(b"{\"anything\": \"here\"}");
    assert_eq!(credentials_file(config.path()), Ok(true));
}

#[test]
fn a_file_too_large_to_be_credentials_is_refused_unread_rather_than_called_a_no() {
    // Whatever wrote this does not get to choose how much memory the server
    // spends, and a file this probe declined to read is not a sign-in it ruled
    // out — the caller must be able to tell those apart.
    let mut oversized = vec![b' '; MAX_CREDENTIALS_BYTES as usize + 1];
    oversized[0] = b'{';
    let config = config_with(&oversized);
    assert_eq!(
        credentials_file(config.path()),
        Err(ProbeFailure::Unanswered)
    );
}

#[test]
fn both_variables_the_launcher_passes_through_count_as_a_sign_in() {
    // CLAUDE_CODE_OAUTH_TOKEN is passed straight through to the agent process
    // alongside ANTHROPIC_API_KEY, so a machine holding only that one is signed
    // in and must not be sent to authenticate again.
    assert_eq!(
        CREDENTIAL_VARIABLES,
        ["ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN"],
        "the launcher passes both of these through; the probe must read both"
    );
}
