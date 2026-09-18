//! What makes a file a sign-in, for any agent that writes one.
//!
//! Every input is handed in as a path to a temporary directory, so none of
//! these tests reads the developer's own home directory or credentials. Which
//! file an agent writes, and where, is that agent's own and is tested beside it.

use super::*;
use tempfile::TempDir;

/// A directory holding exactly the given credentials file contents, and the
/// path to that file.
fn config_with(contents: &[u8]) -> (TempDir, std::path::PathBuf) {
    let config = TempDir::new().unwrap();
    let path = config.path().join("credentials.json");
    std::fs::write(&path, contents).unwrap();
    (config, path)
}

#[test]
fn a_missing_credentials_file_is_a_real_no() {
    let config = TempDir::new().unwrap();
    assert_eq!(
        credentials_file(&config.path().join("credentials.json")),
        Ok(false)
    );
}

#[test]
fn an_empty_credentials_file_is_a_real_no() {
    let (_config, path) = config_with(b"");
    assert_eq!(credentials_file(&path), Ok(false));
}

#[test]
fn an_empty_json_object_is_a_real_no() {
    // What a crashed or half-finished write leaves behind. It has a byte count,
    // which used to be the whole test, but it carries no credential at all.
    let (_config, path) = config_with(b"{}");
    assert_eq!(credentials_file(&path), Ok(false));
}

#[test]
fn a_truncated_credentials_file_is_a_real_no() {
    // the agent could not sign in with this either, so neither can the probe
    // report it as a sign-in.
    let (_config, path) = config_with(b"{\"foo\": ");
    assert_eq!(credentials_file(&path), Ok(false));
    let (_stray, stray) = config_with(b"\0");
    assert_eq!(credentials_file(&stray), Ok(false));
}

#[test]
fn json_that_is_not_an_object_is_a_real_no() {
    // A credential is a set of named fields whatever its vendor calls them; none
    // of these could be one.
    for contents in [
        b"null".as_slice(),
        b"true".as_slice(),
        b"7".as_slice(),
        b"\"token\"".as_slice(),
        b"[{\"anything\": \"here\"}]".as_slice(),
    ] {
        let (_config, path) = config_with(contents);
        assert_eq!(
            credentials_file(&path),
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
    // its vendor renames it. An object with contents is as much as can honestly
    // be asserted, and it is enough to rule out the corrupted cases above.
    let (_config, path) = config_with(b"{\"anything\": \"here\"}");
    assert_eq!(credentials_file(&path), Ok(true));
}

#[test]
fn a_file_too_large_to_be_credentials_is_refused_unread_rather_than_called_a_no() {
    // Whatever wrote this does not get to choose how much memory the server
    // spends, and a file this probe declined to read is not a sign-in it ruled
    // out — the caller must be able to tell those apart.
    let mut oversized = vec![b' '; MAX_CREDENTIALS_BYTES as usize + 1];
    oversized[0] = b'{';
    let (_config, path) = config_with(&oversized);
    assert_eq!(credentials_file(&path), Err(ProbeFailure::Unanswered));
}
