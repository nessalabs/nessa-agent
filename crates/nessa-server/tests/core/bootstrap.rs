//! What a command line this build cannot run ends as.
//!
//! Parsing is the one step every invocation passes through before anything is
//! dispatched, so its failure is the one any invocation can end in. These cover
//! which failure that is and what it says, which is what the desktop host reads
//! back as an exit code.
use super::*;

/// Words to arguments, since a command line is strings and nothing here is
/// testing how a `Vec` is built.
fn arguments(words: [&str; 3]) -> Vec<String> {
    words
        .iter()
        .filter(|word| !word.is_empty())
        .map(|word| (*word).to_string())
        .collect()
}

/// A mistyped command used to be reported as an authentication failure whatever
/// it was trying to do. A usage problem is its own fact and says only what was
/// wrong.
#[test]
fn a_command_line_this_binary_cannot_run_is_a_usage_failure_not_an_authentication_one() {
    for words in [
        ["auth", "token", "--cloud"],
        ["install-agent", "", ""],
        ["nonsense", "", ""],
    ] {
        let args = arguments(words);
        let error = parsed(&args).expect_err("the command line is not one nessa can run");
        assert!(matches!(error, RunError::Usage(_)), "{words:?}: {error:?}");
        let reported = error.to_string();
        assert!(
            !reported.contains("authentication setup failed"),
            "{words:?}: {reported}"
        );
        // Nothing was dispatched, so the message is the parser's own, carried
        // out whole rather than summarized by the variant carrying it.
        assert_eq!(reported, parse(&args).expect_err("rejected"), "{words:?}");
    }

    // And the one that says what to do instead still says it.
    let error = parsed(&arguments(["auth", "token", "--cloud"])).unwrap_err();
    assert!(error.to_string().contains("use --local"), "{error}");
}

/// A command line this build *can* run is handed back untouched, so the
/// mapping above is a mapping of failures and not a refusal of everything.
#[test]
fn a_command_this_binary_can_run_is_not_a_failure() {
    assert!(matches!(
        parsed(&arguments(["--help", "", ""])),
        Ok(Command::Help)
    ));
}
