//! What Codex's own sign-in conventions are.
//!
//! Nothing here reads the developer's own home directory or credentials. What
//! makes a file a sign-in is shared and tested in `credentials.rs`; what is
//! tested here is the part that is Codex's own, and that is a set of names
//! Nessa does not get to choose.

use super::*;

#[test]
fn both_variables_the_launcher_passes_through_count_as_a_sign_in() {
    // Codex reads CODEX_API_KEY first and falls back to OPENAI_API_KEY, and the
    // binding passes both through. A machine holding only the second one is
    // signed in and must not be sent to authenticate again.
    assert_eq!(
        CREDENTIAL_VARIABLES,
        ["CODEX_API_KEY", "OPENAI_API_KEY"],
        "the launcher passes both of these through; the probe must read both"
    );
}

#[test]
fn the_file_asked_about_is_the_one_codex_writes_a_chatgpt_login_into() {
    // Codex has no keychain item: an API key and the tokens from a ChatGPT
    // login both land in this one file, which is why asking about it is the
    // whole of the question this machine can answer about Codex on disk.
    assert_eq!(CREDENTIALS_FILE, "auth.json");
}

#[test]
fn the_path_is_taken_from_codexs_own_variable_before_its_default() {
    // Resolved from this process's environment, which the test does not change.
    // What is asserted is the shape of the answer: a path under whichever of
    // the two locations applies, ending in the file Codex writes.
    if let Some(path) = credentials_path() {
        assert!(path.ends_with(CREDENTIALS_FILE), "{}", path.display());
        let directory = path.parent().unwrap();
        match std::env::var("CODEX_HOME") {
            Ok(home) => assert_eq!(directory, std::path::Path::new(&home)),
            Err(_) => assert!(directory.ends_with(".codex"), "{}", directory.display()),
        }
    }
}
