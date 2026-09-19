//! What Claude Code's own sign-in conventions answer.
//!
//! Every input is handed in as a path to a temporary directory, so none of
//! these tests reads the developer's own home directory, credentials, or
//! keychain. The keychain is the host's to answer and is never driven here.
//! What makes a file a credentials file is shared and tested in
//! `credentials.rs`; what is tested here is what is Claude's own.

use super::*;

/// Waiting for a tool that will not finish, with an ordinary long-running
/// command standing in for a keychain that will not answer. The real keychain is
/// the host's and is never driven here; what is under test is the waiting.
#[cfg(unix)]
mod when_a_tool_will_not_finish {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::Instant;

    /// A process that would outlive any request, started the way the keychain
    /// lookup is started.
    fn a_long_running_process() -> std::process::Child {
        Command::new("/bin/sleep")
            .arg("300")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("/bin/sleep should be present on a Unix host")
    }

    #[test]
    fn the_wait_ends_at_the_deadline_rather_than_at_the_process() {
        let mut child = a_long_running_process();
        let began = Instant::now();
        let status = wait_or_kill(&mut child, Duration::from_millis(100));
        let waited = began.elapsed();
        assert_eq!(status, None, "a process that never answered has no status");
        assert!(
            waited < Duration::from_secs(5),
            "waited {waited:?} on a process that runs for five minutes"
        );
    }

    #[test]
    fn a_process_that_ran_out_of_time_is_killed_and_reaped() {
        // Killing without reaping leaves a zombie holding a process table entry
        // for as long as this server runs, which is the same unbounded cost in
        // a quieter form.
        let mut child = a_long_running_process();
        let id = child.id();
        assert_eq!(wait_or_kill(&mut child, Duration::from_millis(50)), None);
        // `kill -0` asks the kernel whether the process could be signalled,
        // without sending anything. It succeeds for a zombie too, so a no here
        // is the whole claim: the process was killed *and* reaped, and its
        // entry in the process table is gone rather than held for this
        // server's lifetime.
        let alive = Command::new("/bin/sh")
            .args(["-c", &format!("kill -0 {id}")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        assert!(!alive, "process {id} outlived the deadline it was given");
    }

    #[test]
    fn a_tool_that_answers_is_not_waited_on_any_longer_than_it_takes() {
        let mut child = Command::new("/bin/sh")
            .args(["-c", "exit 44"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let began = Instant::now();
        let status = wait_or_kill(&mut child, Duration::from_secs(5));
        assert_eq!(status.and_then(|status| status.code()), Some(44));
        assert!(
            began.elapsed() < Duration::from_secs(2),
            "an answer must not wait out the deadline"
        );
    }
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
