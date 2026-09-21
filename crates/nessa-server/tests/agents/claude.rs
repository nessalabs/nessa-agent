//! What Claude Code's own sign-in conventions answer.
//!
//! Every input is handed in as a path to a temporary directory, so none of
//! these tests reads the developer's own home directory, credentials, or
//! keychain. The keychain is the host's to answer and is never driven here.
//! What makes a file a credentials file, and how long a vendor's own tool is
//! waited on, are shared and tested in `credentials.rs`; what is tested here is
//! what is Claude's own.

use super::*;

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
