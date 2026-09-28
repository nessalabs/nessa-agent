//! What Codex's own sign-in conventions are.
//!
//! Nothing here reads the developer's own home directory or credentials. What
//! makes a file a sign-in is shared and tested in `credentials.rs`; what is
//! tested here is the part that is Codex's own, and that is a set of names
//! Nessa does not get to choose.

use super::*;

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

#[cfg(unix)]
#[test]
fn status_answers_are_returned_after_the_restricted_process_group_is_gone() {
    let root = tempfile::tempdir().unwrap();
    let script = root.path().join("status.sh");
    for (code, expected) in [
        (0, Ok(true)),
        (1, Ok(false)),
        (2, Err(ProbeFailure::Unanswered)),
    ] {
        std::fs::write(&script, format!("exit {code}\n")).unwrap();
        assert_eq!(
            sign_in_status(Path::new("/bin/sh"), &script, &BTreeMap::new()),
            expected
        );
    }
}

#[cfg(unix)]
#[test]
fn a_status_timeout_kills_and_reaps_its_root_before_reporting_an_unanswered_probe() {
    let mut child = Command::new("/bin/sleep")
        .arg("20")
        .process_group(0)
        .spawn()
        .unwrap();
    let group = child.id();
    assert_eq!(
        wait_status_group(&mut child, Duration::from_millis(20)),
        Ok(None)
    );
    assert!(child.try_wait().unwrap().is_some());
    assert_eq!(unsafe { libc::kill(-(group as i32), 0) }, -1);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
}

#[cfg(unix)]
#[test]
fn status_cleanup_removes_descendants_after_root_exit_and_after_timeout() {
    for wait_for_child in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let marker = root.path().join("child-ready");
        let script = if wait_for_child {
            "sleep 20 & echo $! > \"$1\"; wait"
        } else {
            "sleep 20 & echo $! > \"$1\"; exit 0"
        };
        let mut child = Command::new("/bin/sh")
            .args(["-c", script, "status-fixture"])
            .arg(&marker)
            .process_group(0)
            .spawn()
            .unwrap();
        let group = child.id();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !marker.exists() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        let answer = wait_status_group(&mut child, Duration::from_millis(30)).unwrap();
        assert_eq!(answer.is_some(), !wait_for_child);
        assert!(child.try_wait().unwrap().is_some());
        assert_eq!(unsafe { libc::kill(-(group as i32), 0) }, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }
}
