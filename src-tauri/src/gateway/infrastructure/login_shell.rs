//! Asking the user's login shell what their `PATH` is.
//!
//! A login shell is the user's own code — `.zprofile`, `.bash_profile`, whatever
//! Homebrew, nvm or mise wrote into them — run by this host. So it is run the
//! way untrusted code is run: a clean environment with nothing secret in it, no
//! terminal, output read to a bound, a deadline, and an answer that is a
//! [`SearchPath`] or nothing.
//!
//! ```text
//! LoginShell::resolve ──spawn──▶ <login shell> -lc "printenv PATH"
//!        │                              │
//!        └──deadline, bounded read──────┘──▶ SearchPath::parse
//! ```
//! Arrows mean process control and the value coming back. A shell that is still
//! running when the deadline passes is killed with its process group and
//! reported as [`LoginShellError::TimedOut`]; registration carries on without it.
//!
//! What this module does not verify about itself: that a real `.zprofile` on a
//! real machine exports what the user expects. Its tests stand in their own
//! shell scripts for that, and the deadline path is covered by a script that
//! never returns — its descendants are killed with the process group, which the
//! test asserts only for the shell itself, because when the kernel reaps a
//! descendant of a dead leader is not ours to observe.
use super::super::application::{LoginShellError, LoginShellPath};
use super::super::domain::value_objects::SearchPath;
use std::time::Duration;

/// How long a registration will wait for a login shell before giving up on it.
///
/// Long enough for the version managers people actually have in their profiles;
/// short enough that a profile which waits for input costs one pause at startup
/// rather than a gateway that never registers.
const DEADLINE: Duration = Duration::from_secs(5);

/// The shell to ask when the account record names none that can be run.
const FALLBACK_SHELL: &str = "/bin/sh";

/// What the login shell is asked to run.
///
/// `printenv` rather than `echo $PATH`: every shell that a Mac ships or a user
/// installs exports `PATH` as one colon-separated string, but not all of them
/// interpolate it as one — fish holds it as a list and would print it with the
/// separators gone, which is a different path that happens to look like one.
const REPORT_PATH: &str = "/usr/bin/printenv PATH";

/// The most output to read before deciding this is not a shell reporting a path.
const OUTPUT_LIMIT: u64 = 64 * 1024;

#[cfg(unix)]
pub(super) use unix::LoginShell;

/// Every target that has no login shell to ask. Registration falls back to the
/// service's own path, which is what this reports.
#[cfg(not(unix))]
pub(super) struct LoginShell;
#[cfg(not(unix))]
impl LoginShell {
    pub(super) fn for_current_user() -> Self {
        Self
    }
}
#[cfg(not(unix))]
impl LoginShellPath for LoginShell {
    fn resolve(&self) -> Result<SearchPath, LoginShellError> {
        Err(LoginShellError::Unavailable(
            "login shells are read on Unix hosts only".into(),
        ))
    }
}

/// The path the shell reported, which is the last line it printed.
///
/// Profiles print things — a fortune, a version manager's notice, a warning
/// about a missing directory. The command asked for is the last thing to run,
/// so its answer is the last line, and everything above it is somebody else's.
fn reported_path(output: &str) -> &str {
    output
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
}

#[cfg(unix)]
mod unix {
    use super::{
        LoginShellError, LoginShellPath, SearchPath, DEADLINE, FALLBACK_SHELL, OUTPUT_LIMIT,
        REPORT_PATH,
    };
    use std::{
        ffi::{CStr, OsStr},
        io::Read,
        os::unix::{ffi::OsStrExt, process::CommandExt},
        path::{Path, PathBuf},
        process::{Command, Stdio},
        sync::mpsc,
        thread,
        time::Duration,
    };

    /// The account's login shell, run for its `PATH`.
    pub(in super::super) struct LoginShell {
        shell: PathBuf,
        deadline: Duration,
    }

    impl LoginShell {
        /// The shell this account logs in with, from the account record rather
        /// than from `SHELL`.
        ///
        /// `SHELL` is inherited: a Nessa started from a terminal running one
        /// shell under an account configured with another would resolve a
        /// different path than the same Nessa started from the Dock, and the
        /// resolved path is part of the service definition. The account record
        /// is the same on both launches.
        pub(in super::super) fn for_current_user() -> Self {
            Self {
                shell: account_shell(),
                deadline: DEADLINE,
            }
        }

        #[cfg(test)]
        pub(in super::super) fn with_shell(shell: PathBuf, deadline: Duration) -> Self {
            Self { shell, deadline }
        }

        /// What the shell printed, or why nothing usable came back.
        fn report(&self) -> Result<String, LoginShellError> {
            let unavailable =
                |error: std::io::Error| LoginShellError::Unavailable(error.to_string());
            let mut child = Command::new(&self.shell)
                .arg("-lc")
                .arg(REPORT_PATH)
                // Nothing of this process's environment reaches the user's
                // profile: not the surface credential, not a provider token,
                // not the launch context. The shell gets what a shell needs to
                // find the tools its own profile calls, and no more.
                .env_clear()
                .env("PATH", SearchPath::system().as_str())
                .env("TERM", "dumb")
                .envs(
                    ["HOME", "USER", "LOGNAME"]
                        .into_iter()
                        .filter_map(|key| std::env::var_os(key).map(|value| (key, value))),
                )
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                // Its own process group, so a profile that leaves something
                // running is stopped with the shell rather than outliving it.
                .process_group(0)
                .spawn()
                .map_err(unavailable)?;
            let group = child.id() as i32;
            let mut output = child
                .stdout
                .take()
                .ok_or_else(|| LoginShellError::Unavailable("no shell output".into()))?;
            let (reader, reported) = mpsc::channel();
            thread::spawn(move || {
                let mut bytes = Vec::new();
                let read = output
                    .by_ref()
                    .take(OUTPUT_LIMIT)
                    .read_to_end(&mut bytes)
                    .map(|_| bytes);
                // Dropping the pipe here is what ends a shell that is still
                // pouring out more than the limit: its next write has nowhere
                // to go, so the wait below returns instead of blocking on a
                // reader that has stopped reading.
                drop(output);
                // A receiver that has already given up on the deadline is the
                // expected end of this thread, not a failure of it.
                let _ = reader.send(read);
            });
            // The read ends when the shell and everything holding its output
            // are done with it, so a profile that backgrounds a process holding
            // the pipe open is caught by this deadline rather than by the exit.
            let Ok(read) = reported.recv_timeout(self.deadline) else {
                stop(group);
                // Reaped here, so the deadline cannot leave a zombie behind.
                let _ = child.wait();
                return Err(LoginShellError::TimedOut);
            };
            let bytes = read.map_err(unavailable)?;
            let status = child.wait().map_err(unavailable)?;
            if !status.success() {
                return Err(LoginShellError::Unavailable(format!(
                    "login shell exited with {status}"
                )));
            }
            String::from_utf8(bytes)
                .map_err(|_| LoginShellError::Unavailable("login shell output is not UTF-8".into()))
        }
    }

    impl LoginShellPath for LoginShell {
        fn resolve(&self) -> Result<SearchPath, LoginShellError> {
            SearchPath::parse(super::reported_path(&self.report()?))
                .map_err(LoginShellError::Rejected)
        }
    }

    /// Kills the whole process group the shell was given.
    fn stop(group: i32) {
        // SAFETY: `group` is this process's own child, made a group leader when
        // it was spawned, and a negative pid signals that group alone.
        unsafe { libc::kill(-group, libc::SIGKILL) };
    }

    /// The login shell recorded for this account, or [`FALLBACK_SHELL`].
    fn account_shell() -> PathBuf {
        // SAFETY: the returned record is owned by libc and is read, not kept,
        // before any other call that could replace it. Registration is the one
        // caller and holds the service lock while it runs.
        let entry = unsafe { libc::getpwuid(libc::geteuid()) };
        if !entry.is_null() {
            let shell = unsafe { (*entry).pw_shell };
            if !shell.is_null() {
                let shell = Path::new(OsStr::from_bytes(
                    unsafe { CStr::from_ptr(shell) }.to_bytes(),
                ));
                if shell.is_absolute() {
                    return shell.to_path_buf();
                }
            }
        }
        PathBuf::from(FALLBACK_SHELL)
    }
}

#[cfg(all(test, unix))]
#[path = "../../../tests/gateway/infrastructure/login_shell.rs"]
mod tests;
