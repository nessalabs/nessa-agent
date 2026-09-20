//! The adapter against real shells: stand-ins that run what they are given the
//! way a shell does, a real zsh with a real profile, and the profiles that
//! misbehave — the ones that never return, and the ones that never stop talking.
use super::unix::{between, carried_environment, LoginShell};
use crate::gateway::application::{LoginShellError, LoginShellPath};
use crate::gateway::domain::value_objects::SearchPathError;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// Long enough that a loaded machine does not fail a test about something else.
const PATIENT: Duration = Duration::from_millis(5_000);
/// Short enough that a test about the deadline is over quickly, and long enough
/// that the shell it is applied to has certainly started — the profiles it is
/// applied to sleep for ten minutes, so there is no reading of this as anything
/// but the deadline. Half a second was not enough: with the rest of this file
/// running beside it, including a real zsh, a shell can take longer than that
/// to draw its first breath, and killing one before it has written its pid made
/// these tests fail for a reason that was not the one they are about.
const BRIEF: Duration = Duration::from_millis(2_000);

/// A directory of this test's own, named for the test using it.
fn temporary_directory(name: &str) -> PathBuf {
    let directory =
        std::env::temp_dir().join(format!("nessa-login-shell-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(&directory).unwrap();
    directory
}

/// A stand-in login shell: a script that runs the command it is given, `$3`,
/// the way the shell it stands in for would.
///
/// `prelude` is what its profile does first — print a banner, narrow the path,
/// close its output, refuse to return.
fn shell_script(directory: &Path, prelude: &str) -> PathBuf {
    let script = directory.join("login-shell");
    // The pid goes to a path decided here, so what the test reads afterwards
    // does not depend on how the shell was invoked or what it has on its own
    // `PATH` — which is deliberately almost nothing.
    fs::write(
        &script,
        format!(
            "#!/bin/sh\necho \"$$\" > '{}'\n{prelude}\neval \"$3\"\n",
            directory.join("pid").display()
        ),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    script
}

/// The pid the stand-in shell wrote down for itself.
fn shell_pid(directory: &Path) -> i32 {
    fs::read_to_string(directory.join("pid"))
        .expect("the stand-in shell records its pid")
        .trim()
        .parse()
        .expect("a pid")
}

/// Whether that process is still there. Gone means killed *and* reaped: a
/// zombie would still answer this.
fn still_running(pid: i32) -> bool {
    // SAFETY: signal 0 asks about delivery without delivering anything.
    unsafe { libc::kill(pid, 0) == 0 }
}

fn resolve_with(
    directory: &Path,
    prelude: &str,
    deadline: Duration,
) -> Result<String, LoginShellError> {
    LoginShell::probing(shell_script(directory, prelude), vec![], deadline)
        .resolve()
        .map(|path| path.as_str().to_owned())
}

fn resolve(prelude: &str, name: &str) -> Result<String, LoginShellError> {
    let directory = temporary_directory(name);
    let resolved = resolve_with(&directory, prelude, PATIENT);
    let _ = fs::remove_dir_all(&directory);
    resolved
}

#[test]
fn the_path_a_profile_exports_is_what_comes_back() {
    assert_eq!(
        resolve(
            "PATH=/opt/homebrew/bin:/usr/bin:/bin; export PATH",
            "exported"
        ),
        Ok("/opt/homebrew/bin:/usr/bin:/bin".into())
    );
}

/// Profiles print things. nvm announces itself, a version manager warns about a
/// missing directory, an interactive shell draws a prompt. None of it is between
/// the markers, so none of it is the answer.
#[test]
fn a_chatty_profile_does_not_lose_the_answer() {
    assert_eq!(
        resolve(
            "echo 'nvm: default -> 20'\nprintf 'user@host %% '\nPATH=/opt/homebrew/bin:/usr/bin; export PATH",
            "chatty"
        ),
        Ok("/opt/homebrew/bin:/usr/bin".into())
    );
}

/// A profile cannot answer for the shell: it does not know what the markers are
/// this time, so what it prints is noise around the answer, not the answer.
#[test]
fn a_profile_cannot_forge_the_answer() {
    assert_eq!(
        resolve(
            "echo nessa-path-begin/tmp/forged nessa-path-end\nPATH=/usr/bin:/bin; export PATH",
            "forged"
        ),
        Ok("/usr/bin:/bin".into())
    );
}

#[test]
fn a_shell_that_answers_with_nothing_usable_is_rejected() {
    assert_eq!(
        resolve("PATH=relative-only; export PATH", "unusable"),
        Err(LoginShellError::Rejected(SearchPathError::NoAbsoluteEntry))
    );
}

#[test]
fn a_shell_that_never_answers_in_the_form_asked_is_unavailable() {
    let directory = temporary_directory("unmarked");
    let script = directory.join("login-shell");
    fs::write(&script, "#!/bin/sh\necho 'welcome back'\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(matches!(
        LoginShell::probing(script, vec![], PATIENT).resolve(),
        Err(LoginShellError::Unavailable(_))
    ));
    let _ = fs::remove_dir_all(&directory);
}

#[test]
fn a_shell_that_fails_is_unavailable_rather_than_believed() {
    assert!(matches!(
        resolve(
            "PATH=/usr/bin:/bin; export PATH\ntrap 'exit 3' 0",
            "failing"
        ),
        Err(LoginShellError::Unavailable(_))
    ));
}

#[test]
fn a_shell_that_cannot_be_run_at_all_is_unavailable() {
    assert!(matches!(
        LoginShell::probing(PathBuf::from("/nonexistent/login-shell"), vec![], PATIENT).resolve(),
        Err(LoginShellError::Unavailable(_))
    ));
}

/// The profile this fallback exists for: one that never returns. The deadline is
/// the only thing that ends it.
#[test]
fn a_profile_that_never_returns_times_out_and_is_stopped() {
    let directory = temporary_directory("hanging");
    let started = Instant::now();
    assert_eq!(
        resolve_with(&directory, "sleep 600", BRIEF),
        Err(LoginShellError::TimedOut)
    );
    assert!(started.elapsed() < Duration::from_secs(30));
    assert!(!still_running(shell_pid(&directory)));
    let _ = fs::remove_dir_all(&directory);
}

/// Output arriving is not the shell being finished with. A profile that closes
/// its own stdout and then hangs ends the read immediately, and used to hand the
/// attempt to an unbounded wait — a registration that never came back.
#[test]
fn a_profile_that_closes_its_output_and_hangs_still_times_out() {
    let directory = temporary_directory("closed-output");
    let started = Instant::now();
    assert_eq!(
        resolve_with(&directory, "exec 1>&-\nsleep 600", BRIEF),
        Err(LoginShellError::TimedOut)
    );
    assert!(started.elapsed() < Duration::from_secs(30));
    assert!(!still_running(shell_pid(&directory)));
    let _ = fs::remove_dir_all(&directory);
}

/// The same for a profile that fills the output limit and then stops writing:
/// the read is over, nothing makes the shell notice, and only the deadline does.
#[test]
fn a_profile_that_fills_the_output_limit_and_hangs_still_times_out() {
    let directory = temporary_directory("flood");
    let started = Instant::now();
    assert_eq!(
        resolve_with(
            &directory,
            "/usr/bin/head -c 131072 /dev/zero | /usr/bin/tr '\\0' 'a'\nsleep 600",
            BRIEF
        ),
        Err(LoginShellError::TimedOut)
    );
    assert!(started.elapsed() < Duration::from_secs(30));
    assert!(!still_running(shell_pid(&directory)));
    let _ = fs::remove_dir_all(&directory);
}

/// Nothing this process holds is handed to user-controlled code. The profile is
/// given a closed set of variables — the one this host really builds — so
/// whatever the app was launched with cannot be read out of it.
#[test]
fn the_profile_is_given_a_closed_set_of_variables() {
    let directory = temporary_directory("environment");
    let dump = directory.join("environment");
    let shell = LoginShell::probing(
        shell_script(&directory, &format!("/usr/bin/env > {}", dump.display())),
        carried_environment(),
        PATIENT,
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

/// The finding this probe exists for, against the real thing: `zsh -l -c` reads
/// `.zprofile` and never `.zshrc`, and `.zshrc` is where pnpm's installer and
/// the standard nvm setup put themselves. A user with a tool there has it in
/// every terminal, so the agent has to have it too.
#[test]
fn a_real_zsh_reports_the_tools_its_zshrc_adds() {
    let Some(zsh) = ["/bin/zsh", "/usr/bin/zsh", "/usr/local/bin/zsh"]
        .into_iter()
        .map(PathBuf::from)
        .find(|shell| shell.is_file())
    else {
        // On the platform this ships on there is no such thing as no zsh: macOS
        // has shipped `/bin/zsh` for years and logs users into it. Skipping
        // there would let the one test that proves the interactive probe
        // disappear into a green run, because a passing test's output is
        // captured — a skip and a pass look identical from the outside.
        #[cfg(target_os = "macos")]
        panic!("macOS ships /bin/zsh; a machine without it is not one to skip this on");
        #[cfg(not(target_os = "macos"))]
        {
            eprintln!("no zsh on this machine; the interactive-profile test did not run");
            return;
        }
    };
    let home = temporary_directory("zshrc");
    let tools = home.join("tools");
    fs::create_dir_all(&tools).unwrap();
    // Only `.zshrc`: a login shell that is not interactive never reads it.
    fs::write(
        home.join(".zshrc"),
        format!("export PATH=\"{}:$PATH\"\n", tools.display()),
    )
    .unwrap();

    let resolved = LoginShell::probing(zsh, vec![("HOME", home.clone().into_os_string())], PATIENT)
        .resolve()
        .expect("zsh reports a path");
    assert!(
        resolved
            .as_str()
            .split(':')
            .any(|entry| Path::new(entry) == tools),
        "{} is not on {}",
        tools.display(),
        resolved.as_str()
    );
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn the_answer_is_what_lies_between_the_markers() {
    assert_eq!(between("<b>/usr/bin<e>", "<b>", "<e>"), Some("/usr/bin"));
    // `printenv` ends its line; the value does not include it.
    assert_eq!(between("<b>/usr/bin\n<e>", "<b>", "<e>"), Some("/usr/bin"));
    assert_eq!(
        between("banner\n<b>/usr/bin<e>\nmore", "<b>", "<e>"),
        Some("/usr/bin")
    );
    // A shell that echoed the command before running it prints the markers
    // twice; the answer is the last pair, not the echo.
    assert_eq!(
        between("<b>echo<e>\n<b>/usr/bin<e>", "<b>", "<e>"),
        Some("/usr/bin")
    );
    assert_eq!(between("<b>no end here", "<b>", "<e>"), None);
    assert_eq!(between("nothing at all", "<b>", "<e>"), None);
    assert_eq!(between("<b><e>", "<b>", "<e>"), Some(""));
}
