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
    process::Command,
    time::{Duration, Instant},
};

/// Long enough that a loaded machine does not fail a test about something else.
const PATIENT: Duration = Duration::from_millis(5_000);
/// Short enough that a test about the deadline is over quickly. The profiles it
/// is applied to sleep for ten minutes, so an attempt that ends at all ended
/// because of the deadline; what makes that reliable is the warm-up in
/// [`write_shell`], not the size of this number.
const BRIEF: Duration = Duration::from_millis(1_000);

/// A directory of this test's own, named for the test using it.
fn temporary_directory(name: &str) -> PathBuf {
    let directory =
        std::env::temp_dir().join(format!("nessa-login-shell-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(&directory).unwrap();
    directory
}

/// Writes a stand-in shell, and runs it once before any test depends on how
/// quickly it starts.
///
/// That warm-up is the point. macOS evaluates policy for an executable the first
/// time it runs, and those evaluations serialise: measured on this machine,
/// twelve freshly written scripts exec'd at once had only five of them past
/// their first line after two seconds, while the same files re-run were all past
/// it immediately. A fixture that writes a new executable and then races a
/// deadline is measuring first-exec policy, not the deadline — which is how
/// these tests came to fail four full-suite runs out of four while passing
/// whenever they ran alone. It is the same cost issue #74 is about.
///
/// Every stand-in takes `--warm` and does nothing, and does so *before* it
/// records its pid, so a warm-up cannot leave behind a pid the timed run never
/// wrote.
fn write_shell(directory: &Path, body: &str) -> PathBuf {
    let script = directory.join("login-shell");
    fs::write(
        &script,
        format!("#!/bin/sh\n[ \"$1\" = \"--warm\" ] && exit 0\n{body}\n"),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    let warmed = Command::new(&script)
        .arg("--warm")
        .status()
        .expect("the stand-in shell runs");
    assert!(
        warmed.success(),
        "the stand-in shell failed its warm-up run"
    );
    script
}

/// A stand-in login shell: a script that records that it started, then runs the
/// command it is given, `$3`, the way the shell it stands in for would.
///
/// `prelude` is what its profile does between those — print a banner, narrow the
/// path, close its output, refuse to return.
fn shell_script(directory: &Path, prelude: &str) -> PathBuf {
    // The pid goes to a path decided here, so what the test reads afterwards
    // does not depend on how the shell was invoked or what it has on its own
    // `PATH` — which is deliberately almost nothing.
    write_shell(
        directory,
        &format!(
            "echo \"$$\" > '{}'\n{prelude}\neval \"$3\"",
            directory.join("pid").display()
        ),
    )
}

/// The pid the stand-in shell wrote down for itself, which it does before its
/// profile runs.
///
/// A missing one is not a stricter test, it is a broken one: it means the shell
/// was killed before it started, so what the deadline ended was a process that
/// had not yet begun doing the thing the test is about.
fn shell_pid(directory: &Path) -> i32 {
    fs::read_to_string(directory.join("pid"))
        .expect("the stand-in shell started and recorded its pid")
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
    let script = write_shell(&directory, "echo 'welcome back'");
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

/// Where a real shell lives, or `None` with the reason it is not being tested.
///
/// macOS ships `/bin/zsh` and `/bin/bash` and logs users into one of them, so on
/// the platform this ships on a missing shell is a broken assumption rather than
/// a machine to be tolerant of — and skipping there would let the only tests
/// that drive a shell which distinguishes interactive files disappear into a
/// green run, since a passing test's output is captured and a skip looks exactly
/// like a pass from outside.
fn real_shell(name: &str) -> Option<PathBuf> {
    let found = ["/bin", "/usr/bin", "/usr/local/bin", "/opt/homebrew/bin"]
        .into_iter()
        .map(|directory| PathBuf::from(directory).join(name))
        .find(|shell| shell.is_file());
    if found.is_none() {
        assert!(
            !cfg!(target_os = "macos") || !["zsh", "bash"].contains(&name),
            "macOS ships {name}; a machine without it is not one to skip this on"
        );
        eprintln!("no {name} on this machine; the profile test for it did not run");
    }
    found
}

/// The finding this probe exists for, against the real thing: `zsh -l -c` reads
/// `.zprofile` and never `.zshrc`, and `.zshrc` is where pnpm's installer and
/// the standard nvm setup put themselves. A user with a tool there has it in
/// every terminal, so the agent has to have it too.
#[test]
fn a_real_zsh_reports_the_tools_its_zshrc_adds() {
    let Some(zsh) = real_shell("zsh") else {
        return;
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

/// bash is not zsh, and the reason `-i` earns its place for it is different.
/// `bash -i -l -c` does not read `.bashrc` — it reads the login files, exactly
/// as `bash -l -c` does. What it does reach is a `.bashrc` that `.bash_profile`
/// sources, because such a file almost always opens with an interactivity guard
/// and only an interactive shell gets past it to the `PATH` lines below. This
/// fixture is that arrangement, and it fails without the `-i`.
#[test]
fn a_real_bash_gets_past_the_interactivity_guard_in_a_sourced_bashrc() {
    let Some(bash) = real_shell("bash") else {
        return;
    };
    let home = temporary_directory("bashrc");
    let tools = home.join("tools");
    fs::create_dir_all(&tools).unwrap();
    fs::write(
        home.join(".bashrc"),
        format!(
            "case $- in *i*) ;; *) return;; esac\nexport PATH=\"{}:$PATH\"\n",
            tools.display()
        ),
    )
    .unwrap();
    fs::write(home.join(".bash_profile"), ". \"$HOME/.bashrc\"\n").unwrap();

    let resolved =
        LoginShell::probing(bash, vec![("HOME", home.clone().into_os_string())], PATIENT)
            .resolve()
            .expect("bash reports a path");
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

/// The gap that leaves, written down as a test rather than only as a sentence:
/// a `.bashrc` nothing sources is not reached, because an interactive *login*
/// shell does not read it. A user in that position does not have those tools in
/// their macOS terminal either. If this ever starts passing — a new probe, a
/// different host — the documentation above it is what needs revisiting.
#[test]
fn a_bashrc_that_nothing_sources_is_knowingly_not_reached() {
    let Some(bash) = real_shell("bash") else {
        return;
    };
    let home = temporary_directory("bashrc-only");
    let tools = home.join("tools");
    fs::create_dir_all(&tools).unwrap();
    fs::write(
        home.join(".bashrc"),
        format!("export PATH=\"{}:$PATH\"\n", tools.display()),
    )
    .unwrap();

    let resolved =
        LoginShell::probing(bash, vec![("HOME", home.clone().into_os_string())], PATIENT)
            .resolve()
            .expect("bash reports a path");
    assert!(
        !resolved
            .as_str()
            .split(':')
            .any(|entry| Path::new(entry) == tools),
        "an unsourced .bashrc is now reached; the module's account of bash needs revisiting"
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
