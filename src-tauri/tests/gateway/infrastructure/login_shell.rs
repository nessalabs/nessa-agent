//! The adapter against real shells: scripts that stand in for a user's profile,
//! including the profiles that misbehave.
use super::{unix::reported_path, LoginShell};
use crate::gateway::application::{LoginShellError, LoginShellPath};
use crate::gateway::domain::value_objects::SearchPathError;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// A directory of this test's own, named for the test using it.
fn temporary_directory(name: &str) -> PathBuf {
    let directory =
        std::env::temp_dir().join(format!("nessa-login-shell-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(&directory).unwrap();
    directory
}

/// A stand-in login shell: an executable script run in place of `/bin/zsh`.
fn shell_script(directory: &Path, body: &str) -> PathBuf {
    let script = directory.join("login-shell");
    fs::write(&script, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    script
}

fn resolve(body: &str, name: &str) -> Result<String, LoginShellError> {
    let directory = temporary_directory(name);
    let shell =
        LoginShell::with_shell(shell_script(&directory, body), Duration::from_millis(5_000));
    let resolved = shell.resolve().map(|path| path.as_str().to_owned());
    let _ = fs::remove_dir_all(&directory);
    resolved
}

#[test]
fn the_path_a_profile_exports_is_what_comes_back() {
    assert_eq!(
        resolve("echo /opt/homebrew/bin:/usr/bin:/bin", "exported"),
        Ok("/opt/homebrew/bin:/usr/bin:/bin".into())
    );
}

/// Profiles print things. nvm announces itself, a version manager warns about a
/// missing directory, someone left an `echo` in their `.zprofile`. The answer
/// is the command's, which ran last.
#[test]
fn a_chatty_profile_does_not_lose_the_answer() {
    assert_eq!(
        resolve(
            "echo 'nvm: default -> 20'\necho 'warning: /nope is missing'\necho /opt/homebrew/bin:/usr/bin\necho",
            "chatty"
        ),
        Ok("/opt/homebrew/bin:/usr/bin".into())
    );
}

#[test]
fn a_profile_that_prints_no_path_at_all_is_rejected() {
    assert_eq!(
        resolve("echo 'welcome back'", "no-path"),
        Err(LoginShellError::Rejected(SearchPathError::NoAbsoluteEntry))
    );
}

#[test]
fn a_shell_that_fails_is_unavailable_rather_than_believed() {
    assert!(matches!(
        resolve("echo /usr/bin:/bin\nexit 3", "failing"),
        Err(LoginShellError::Unavailable(_))
    ));
}

#[test]
fn a_shell_that_cannot_be_run_at_all_is_unavailable() {
    let shell = LoginShell::with_shell(
        PathBuf::from("/nonexistent/login-shell"),
        Duration::from_millis(5_000),
    );
    assert!(matches!(
        shell.resolve(),
        Err(LoginShellError::Unavailable(_))
    ));
}

/// The bound is on the output, not on the shell's good behaviour: a profile
/// that pours out megabytes is stopped rather than read.
#[test]
fn unbounded_output_does_not_become_a_path() {
    assert!(matches!(
        resolve(
            "i=0; while [ $i -lt 4000 ]; do echo /usr/local/bin/padding-directory-name; i=$((i+1)); done",
            "flood"
        ),
        Err(LoginShellError::Rejected(_)) | Err(LoginShellError::Unavailable(_))
    ));
}

/// The profile that this whole fallback exists for: one that never returns.
/// The deadline is the only thing that ends it, and the shell is gone
/// afterwards rather than left running for the life of the app.
#[test]
fn a_profile_that_never_returns_times_out_and_is_stopped() {
    let directory = temporary_directory("hanging");
    let script = shell_script(&directory, "sleep 600");
    let shell = LoginShell::with_shell(script, Duration::from_millis(250));
    let started = Instant::now();
    assert_eq!(shell.resolve(), Err(LoginShellError::TimedOut));
    // Bounded by the deadline, not by the profile: the generous ceiling is for
    // a loaded CI machine, and 600 seconds is what failing this looks like.
    assert!(started.elapsed() < Duration::from_secs(30));
    let _ = fs::remove_dir_all(&directory);
}

/// Nothing this process holds is handed to user-controlled code. The profile is
/// given a closed set of variables, so whatever the app was launched with —
/// tokens, credentials, launch context — cannot be read out of it.
#[test]
fn the_profile_is_given_a_closed_set_of_variables() {
    let directory = temporary_directory("environment");
    let dump = directory.join("environment");
    let shell = LoginShell::with_shell(
        shell_script(
            &directory,
            &format!("/usr/bin/env > {}\necho /usr/bin:/bin", dump.display()),
        ),
        Duration::from_millis(5_000),
    );
    assert!(shell.resolve().is_ok());
    let passed: Vec<String> = fs::read_to_string(&dump)
        .unwrap()
        .lines()
        .filter_map(|line| line.split_once('=').map(|(key, _)| key.to_owned()))
        .collect();
    assert!(passed.contains(&"PATH".to_string()), "{passed:?}");
    for key in &passed {
        assert!(
            ["PATH", "TERM", "HOME", "USER", "LOGNAME", "PWD", "SHLVL", "_"]
                .contains(&key.as_str()),
            "the login shell was given {key}"
        );
    }
    let _ = fs::remove_dir_all(&directory);
}

#[test]
fn the_reported_path_is_the_last_line_with_anything_on_it() {
    assert_eq!(reported_path("/usr/bin\n"), "/usr/bin");
    assert_eq!(reported_path("banner\n/usr/bin\n\n  \n"), "/usr/bin");
    assert_eq!(reported_path("/usr/bin"), "/usr/bin");
    assert_eq!(reported_path(""), "");
    assert_eq!(reported_path("\n \n"), "");
}
