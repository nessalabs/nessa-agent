//! Asking the user's login shell what their `PATH` is.
//!
//! A login shell is the user's own code — `.zshrc`, `.zprofile`, `.bash_profile`,
//! whatever Homebrew, nvm, pnpm or mise wrote into them — run by this host. So it
//! is run the way untrusted code is run: a clean environment with nothing secret
//! in it, no terminal, no stdin, output read to a bound, one deadline over the
//! whole thing, and an answer that is a [`SearchPath`] or nothing.
//!
//! ```text
//! LoginShell::resolve ─▶ <shell> -i -l -c "<marker> printenv PATH <marker>"
//!          │ failed ──▶ <shell>    -l -c "<same>"
//!          │                              │
//!          └──one deadline, bounded read──┘──▶ between markers ──▶ SearchPath
//! ```
//! Arrows mean process control and the value coming back; each failed attempt is
//! reported before the next is tried, and the caller keeps the path its service
//! is already registered with if both fail.
//!
//! Two decisions are worth the detail:
//!
//! **What each shell is asked, and why bash is asked twice.** A shell only reads
//! the file the user's tools are in if it is started the way that file is for,
//! and the shells differ in how much one invocation can cover. Measured against
//! real shells, one marker per startup file:
//!
//! ```text
//!          │ bash                        │ zsh
//! ─────────┼─────────────────────────────┼──────────────────────────────────
//!    -l -c │ .bash_profile               │ .zshenv .zprofile .zlogin
//! -i -l -c │ .bash_profile               │ .zshenv .zprofile .zlogin .zshrc
//!    -i -c │ .bashrc                     │ .zshenv .zshrc
//! ```
//!
//! For zsh one invocation covers everything: `-i -l -c` is a superset of both
//! others, so zsh is asked once. `-l -c` remains as a fallback for a zsh that
//! cannot be interactive at all.
//!
//! For bash there is no such invocation. `.bash_profile` is what a macOS
//! terminal opens and where a login shell's `PATH` legitimately lives; `.bashrc`
//! is what a Linux terminal opens and where pnpm's installer and the standard
//! nvm setup write. Asking only one of them means a user whose tools are in the
//! other gets a *successful* probe carrying an incomplete `PATH` — nothing
//! fails, so nothing falls back and nothing is logged, and the agent simply
//! cannot find their tools. So bash is asked both ways and the answers are
//! combined: everything the login shell reported, then anything only the
//! interactive shell adds. Login first because that is the shell a terminal
//! opens on the platform this ships on, so where the two disagree about which
//! `node` comes first, the agent agrees with the user's terminal. Where
//! `.bash_profile` sources `.bashrc` — the common arrangement — the first
//! answer already contains the second and the combination changes nothing,
//! which is also why `-i` matters there: a sourced `.bashrc` opens with
//! `case $- in *i*) ;; *) return;; esac`, and only an interactive shell reaches
//! the `PATH` lines below it.
//!
//! **Markers, not the last line.** An interactive shell prints things: a motd, a
//! plugin banner, a prompt, a warning about a missing directory. The `PATH` is
//! written between two markers made of bytes from `/dev/urandom`, and only what
//! is between them is read — a profile cannot print a convincing answer because
//! it cannot know what to print, and noise around it does not matter.
//!
//! Two things an interactive profile makes likelier, and what is done about
//! them. It may background something — `ssh-agent`, `gpg-agent`, occasionally an
//! `exec tmux` — and a backgrounded process that inherits the shell's stdout
//! holds the pipe open after the shell is done, which the deadline then ends:
//! the answer was there and the attempt reports a timeout anyway, and the group
//! kill takes the agent with it. The login probe that follows usually does not
//! do this, which is the fallback earning its place. And `.zshrc` is where
//! working-directory-sensitive `PATH` logic lives (nvm's `.nvmrc` auto-use,
//! direnv, pyenv local), so the shell is run from the account's home rather than
//! from wherever this process happens to have been started: a packaged launch
//! starts at `/` and a developer's does not, and the resolved path is compared
//! for exact equality against the registered one. It is the same reason the
//! account record is trusted over `SHELL`.
//!
//! What this module does not verify about itself: that a real `.zprofile` on a
//! real machine exports what its owner expects. Its tests supply their own
//! shells and their own profiles, including a real zsh with a real `.zshrc` and
//! a real bash with a `.bashrc` its `.bash_profile` sources.
use super::super::application::{LoginShellError, LoginShellPath};
use super::super::domain::value_objects::SearchPath;

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

#[cfg(unix)]
mod unix {
    use super::{LoginShellError, LoginShellPath, SearchPath};
    use std::{
        ffi::{CStr, OsStr, OsString},
        fs::File,
        io::{self, Read},
        os::unix::{ffi::OsStrExt, process::CommandExt},
        path::{Path, PathBuf},
        process::{Command, ExitStatus, Stdio},
        sync::mpsc::{self, Receiver, RecvTimeoutError},
        thread,
        time::{Duration, Instant},
    };

    /// How long one attempt may take, start to finish: the shell's output and
    /// the shell's exit, not one of them.
    ///
    /// Long enough for the version managers people actually have in their
    /// profiles; short enough that a profile which never returns costs a pause
    /// at startup rather than a gateway that never registers.
    const DEADLINE: Duration = Duration::from_secs(5);

    /// How long every attempt together may take.
    ///
    /// bash is asked twice and can then fall back, so a per-attempt deadline
    /// alone would let the ceiling grow with the number of probes — and this is
    /// time the panel spends waiting for `Gateway::wait_ready`, which has no
    /// deadline of its own: whatever is spent here, the window that asked for a
    /// credential simply waits. So the whole resolution gets one budget, each
    /// attempt gets what is left of it, and a shell with more probes buys more
    /// coverage rather than more waiting.
    ///
    /// The worst a caller can be kept waiting is this plus one [`REAP_GRACE`]
    /// for the last kill to be confirmed: 10 + 2 = 12 seconds, once per run of
    /// the app, and only for profiles that hang. That is below the 14 seconds
    /// two attempts could reach before this budget existed.
    const BUDGET: Duration = Duration::from_secs(10);

    /// How long to wait for a killed shell to be reaped before reporting the
    /// timeout anyway.
    ///
    /// `SIGKILL` cannot be caught or blocked, so this is a confirmation rather
    /// than a wait. Whether it arrives or not the child is owned by a thread
    /// that does nothing but reap it, so no zombie is left behind either way.
    const REAP_GRACE: Duration = Duration::from_secs(2);

    /// The shell to ask when the account record names none that can be run.
    const FALLBACK_SHELL: &str = "/bin/sh";

    /// The most output to read before deciding this is not a shell reporting a
    /// path.
    const OUTPUT_LIMIT: u64 = 64 * 1024;

    /// How much randomness a marker carries. A profile that cannot guess these
    /// bytes cannot put words in the shell's mouth.
    const MARKER_BYTES: usize = 16;

    /// How a shell is asked. Which of these a shell gets, and what is done with
    /// the answers, is [`Plan`].
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub(super) enum Probe {
        /// `-i -l -c`: the login files, read by a shell that also considers
        /// itself interactive. What a terminal opens on this platform.
        InteractiveLogin,
        /// `-i -c`: interactive and not a login shell, which is the only way
        /// bash reads `~/.bashrc`.
        Interactive,
        /// `-l -c`: the login files, nothing more. What every shell here
        /// understands, and the fallback when interactive attempts fail.
        Login,
    }
    impl Probe {
        fn arguments(self) -> &'static [&'static str] {
            match self {
                Self::InteractiveLogin => &["-i", "-l", "-c"],
                Self::Interactive => &["-i", "-c"],
                Self::Login => &["-l", "-c"],
            }
        }
        fn describe(self) -> &'static str {
            match self {
                Self::InteractiveLogin => "interactive login shell",
                Self::Interactive => "interactive shell",
                Self::Login => "login shell",
            }
        }
    }

    /// What to ask one shell, and what to do with what comes back.
    ///
    /// `combined` is asked in order and every answer contributes, earliest
    /// first; `fallback` is asked only if none of them answered at all. The
    /// module documentation says why bash has two of the first kind and the
    /// others have one.
    pub(super) struct Plan {
        combined: &'static [Probe],
        fallback: &'static [Probe],
    }

    const BASH: Plan = Plan {
        combined: &[Probe::InteractiveLogin, Probe::Interactive],
        fallback: &[Probe::Login],
    };
    const INTERACTIVE_LOGIN: Plan = Plan {
        combined: &[Probe::InteractiveLogin],
        fallback: &[Probe::Login],
    };
    const LOGIN: Plan = Plan {
        combined: &[Probe::Login],
        fallback: &[],
    };

    /// The account's login shell, run for its `PATH`.
    pub(in super::super) struct LoginShell {
        shell: PathBuf,
        /// What the shell is given. Built once, from this process's
        /// environment, by [`LoginShell::for_current_user`]; nothing else of
        /// this process reaches the user's profile.
        environment: Vec<(&'static str, OsString)>,
        /// What one attempt may take.
        deadline: Duration,
        /// What every attempt together may take. See [`BUDGET`].
        budget: Duration,
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
                environment: carried_environment(),
                deadline: DEADLINE,
                budget: BUDGET,
            }
        }

        /// A shell of the test's choosing, with the environment and deadline it
        /// wants — including the environment this host really builds.
        ///
        /// The budget is the deadline twice over, so a test that sets a short
        /// deadline still gets every attempt its shell is worth.
        #[cfg(test)]
        pub(in super::super) fn probing(
            shell: PathBuf,
            environment: Vec<(&'static str, OsString)>,
            deadline: Duration,
        ) -> Self {
            Self {
                shell,
                environment,
                deadline,
                budget: deadline * 2,
            }
        }

        /// The same, for a test about the budget rather than about a shell.
        #[cfg(test)]
        pub(in super::super) fn probing_within(
            shell: PathBuf,
            environment: Vec<(&'static str, OsString)>,
            deadline: Duration,
            budget: Duration,
        ) -> Self {
            Self {
                shell,
                environment,
                deadline,
                budget,
            }
        }

        /// What this shell is worth being asked.
        ///
        /// bash needs two questions because no single invocation of it reads
        /// both the file a login shell uses and the one an interactive shell
        /// uses; zsh needs one because its interactive login shell reads
        /// everything. Any other shell is asked the one way every shell here
        /// understands: a `-i` that a shell does not take, or takes badly
        /// without a terminal, would cost an attempt to learn nothing. The
        /// module documentation has the measurements behind all three.
        fn plan(&self) -> &'static Plan {
            match self.shell.file_name().and_then(OsStr::to_str) {
                Some("bash") => &BASH,
                Some("zsh") => &INTERACTIVE_LOGIN,
                _ => &LOGIN,
            }
        }

        /// One attempt: run the shell, read what is between the markers, and
        /// put it through the value object.
        ///
        /// `until` is when the budget for the whole resolution runs out; an
        /// attempt gets the shorter of its own deadline and what is left.
        fn ask(&self, probe: Probe, until: Instant) -> Result<SearchPath, LoginShellError> {
            let marker = Marker::random()?;
            let output = self.report(probe, &marker, until)?;
            let reported = between(&output, &marker.begin, &marker.end).ok_or_else(|| {
                LoginShellError::Unavailable(format!("{} printed no marked PATH", probe.describe()))
            })?;
            SearchPath::parse(reported).map_err(LoginShellError::Rejected)
        }

        /// What the shell printed, or why nothing usable came back.
        ///
        /// One deadline covers the whole attempt. Output arriving is not proof
        /// the shell is finished with — a profile can close its stdout, or fill
        /// the output limit, and then never return — so the exit is waited for
        /// under the same clock, and whichever runs out first kills the shell's
        /// process group and reaps it.
        fn report(
            &self,
            probe: Probe,
            marker: &Marker,
            until: Instant,
        ) -> Result<String, LoginShellError> {
            let unavailable = |error: io::Error| LoginShellError::Unavailable(error.to_string());
            let mut child = Command::new(&self.shell)
                .args(probe.arguments())
                .arg(marker.command())
                // Nothing of this process's environment reaches the user's
                // profile: not the surface credential, not a provider token,
                // not the launch context. The shell gets what a shell needs to
                // find the tools its own profile calls, and no more.
                .env_clear()
                .env("PATH", SearchPath::system().as_str())
                .env("TERM", "dumb")
                .envs(self.environment.iter().cloned())
                // No terminal and no input: an interactive profile that asks a
                // question reads end-of-file and carries on instead of waiting
                // for an answer that is never coming.
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                // From the account's home, not from wherever this process was
                // started: a profile that decides the path from the working
                // directory would otherwise answer differently for a packaged
                // launch and a developer's, and this answer is part of the
                // service definition.
                .current_dir(self.working_directory())
                // Its own process group, so a profile that leaves something
                // running is stopped with the shell rather than outliving it.
                .process_group(0)
                .spawn()
                .map_err(unavailable)?;
            let deadline = (Instant::now() + self.deadline).min(until);
            let group = child.id() as i32;
            let mut output = child
                .stdout
                .take()
                .ok_or_else(|| LoginShellError::Unavailable("no shell output".into()))?;

            let (send_read, read) = mpsc::channel();
            thread::spawn(move || {
                let mut bytes = Vec::new();
                let collected = output
                    .by_ref()
                    .take(OUTPUT_LIMIT)
                    .read_to_end(&mut bytes)
                    .map(|_| bytes);
                // Let go of the pipe as soon as reading is over, so a shell
                // still pouring out more than the limit finds nowhere to write.
                drop(output);
                // A receiver that has already given up on the deadline is the
                // expected end of this thread, not a failure of it.
                let _ = send_read.send(collected);
            });
            // The child belongs to this thread from here on. Whatever the
            // deadline does, something is always waiting to reap it.
            let (send_exit, exited) = mpsc::channel();
            thread::spawn(move || {
                let _ = send_exit.send(child.wait());
            });

            let collected = match receive(&read, deadline) {
                Ok(collected) => collected,
                Err(waited) => return Err(self.stop(group, &exited, waited)),
            };
            let bytes = match collected {
                Ok(bytes) => bytes,
                Err(error) => {
                    // A read that failed leaves a shell nobody is reading.
                    let _ = self.stop(group, &exited, RecvTimeoutError::Timeout);
                    return Err(unavailable(error));
                }
            };
            let status = match receive(&exited, deadline) {
                Ok(status) => status.map_err(unavailable)?,
                Err(waited) => return Err(self.stop(group, &exited, waited)),
            };
            if !status.success() {
                return Err(LoginShellError::Unavailable(format!(
                    "{} exited with {status}",
                    probe.describe()
                )));
            }
            String::from_utf8(bytes)
                .map_err(|_| LoginShellError::Unavailable("login shell output is not UTF-8".into()))
        }

        /// Where the shell is run from: the account's home if it has one that
        /// exists, and the root otherwise, which every host has.
        fn working_directory(&self) -> PathBuf {
            self.environment
                .iter()
                .find(|(key, _)| *key == "HOME")
                .map(|(_, home)| PathBuf::from(home))
                .filter(|home| home.is_dir())
                .unwrap_or_else(|| PathBuf::from("/"))
        }

        /// Kills the shell's process group and waits for the reap to be
        /// confirmed, then says why the attempt ended.
        fn stop(
            &self,
            group: i32,
            exited: &Receiver<io::Result<ExitStatus>>,
            waited: RecvTimeoutError,
        ) -> LoginShellError {
            // A shell that has already been reaped must not be signalled: its
            // pid is free from that moment, and a group kill aimed at a
            // recycled one would be aimed at somebody else's process group.
            // The read can time out or fail while the shell itself is long
            // gone, so this is asked rather than assumed.
            if exited.try_recv().is_err() {
                kill_group(group);
                let _ = exited.recv_timeout(REAP_GRACE);
            }
            match waited {
                RecvTimeoutError::Timeout => LoginShellError::TimedOut,
                // Nothing sends on these channels except threads that send
                // exactly once, so a closed one is a thread that did not get
                // that far — which is not the same thing as a slow profile.
                RecvTimeoutError::Disconnected => {
                    LoginShellError::Unavailable("login shell ended without an answer".into())
                }
            }
        }
    }

    impl LoginShellPath for LoginShell {
        /// Asks this shell everything its plan says it is worth asking, and
        /// combines what comes back.
        ///
        /// Every combined answer contributes, earliest first, because for bash
        /// the two invocations read different files and a user's tools may be in
        /// either. Only if none of them answered at all does the fallback run,
        /// and then the first answer is the answer. Each failure says which
        /// question it was; the whole thing is bounded by one [`BUDGET`], so a
        /// shell that is asked more is not a shell that waits longer.
        fn resolve(&self) -> Result<SearchPath, LoginShellError> {
            let until = Instant::now() + self.budget;
            let plan = self.plan();
            let mut combined: Option<SearchPath> = None;
            let mut last = None;
            for probe in plan.combined {
                if Instant::now() >= until {
                    report_exhausted(*probe);
                    break;
                }
                match self.ask(*probe, until) {
                    Ok(path) => {
                        combined = Some(match combined {
                            Some(reported) => reported.followed_by(&path),
                            None => path,
                        })
                    }
                    Err(error) => {
                        report_failure(*probe, &error);
                        last = Some(error);
                    }
                }
            }
            if let Some(combined) = combined {
                return Ok(combined);
            }
            for probe in plan.fallback {
                if Instant::now() >= until {
                    report_exhausted(*probe);
                    break;
                }
                match self.ask(*probe, until) {
                    Ok(path) => return Ok(path),
                    Err(error) => {
                        report_failure(*probe, &error);
                        last = Some(error);
                    }
                }
            }
            // Nothing to report happens only when the budget was gone before
            // the first question, which is the same thing as running out of it.
            Err(last.unwrap_or(LoginShellError::TimedOut))
        }
    }

    /// Says which question did not work and why, because a fallback nobody can
    /// see is indistinguishable from a tool the user never installed.
    fn report_failure(probe: Probe, error: &LoginShellError) {
        eprintln!(
            "[nessa] Reading the {} for the agent's PATH did not work ({error})",
            probe.describe()
        );
    }

    /// Says which question was never asked, for the same reason.
    fn report_exhausted(probe: Probe) {
        eprintln!(
            "[nessa] No time left to read the {} for the agent's PATH",
            probe.describe()
        );
    }

    /// What `channel` delivers before `deadline`.
    fn receive<T>(channel: &Receiver<T>, deadline: Instant) -> Result<T, RecvTimeoutError> {
        channel.recv_timeout(deadline.saturating_duration_since(Instant::now()))
    }

    /// The text between the first marker and the end marker that follows it.
    ///
    /// Searched from the end, because a shell that echoes what it was asked to
    /// run — a profile with `set -v`, a plugin that traces commands — prints the
    /// markers before it prints the answer. The last pair is the answer.
    pub(super) fn between<'a>(output: &'a str, begin: &str, end: &str) -> Option<&'a str> {
        let opened = output.rfind(begin)? + begin.len();
        let rest = &output[opened..];
        Some(rest[..rest.find(end)?].trim())
    }

    /// The pair of markers one attempt writes its answer between.
    pub(super) struct Marker {
        begin: String,
        end: String,
    }
    impl Marker {
        /// A fresh pair, from the system's randomness.
        ///
        /// Hex, so the marker is its own shell quoting: there is nothing in it a
        /// shell could read as syntax.
        fn random() -> Result<Self, LoginShellError> {
            let mut bytes = [0u8; MARKER_BYTES];
            File::open("/dev/urandom")
                .and_then(|mut source| source.read_exact(&mut bytes))
                .map_err(|error| {
                    LoginShellError::Unavailable(format!("no marker for the login shell: {error}"))
                })?;
            let token: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
            Ok(Self {
                begin: format!("nessa-path-{token}-begin"),
                end: format!("nessa-path-{token}-end"),
            })
        }

        /// What the shell is asked to run.
        ///
        /// `printenv` rather than `echo $PATH`: every shell exports `PATH` as one
        /// colon-separated string, but not all of them interpolate it as one —
        /// fish holds it as a list and would print it with the separators gone,
        /// which is a different path that happens to look like one.
        fn command(&self) -> String {
            format!(
                "/usr/bin/printf %s {}; /usr/bin/printenv PATH; /usr/bin/printf %s {}",
                self.begin, self.end
            )
        }
    }

    /// The variables a login shell is given.
    ///
    /// A closed set: what a shell needs to find the tools its own profile calls,
    /// and nothing else this process happens to be holding.
    pub(super) fn carried_environment() -> Vec<(&'static str, OsString)> {
        ["HOME", "USER", "LOGNAME"]
            .into_iter()
            .filter_map(|key| std::env::var_os(key).map(|value| (key, value)))
            .collect()
    }

    /// Kills the whole process group the shell was given.
    fn kill_group(group: i32) {
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
