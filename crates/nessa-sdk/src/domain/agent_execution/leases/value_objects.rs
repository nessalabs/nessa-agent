#![deny(missing_docs)]

use crate::domain::agent_execution::ExecutionError;
use std::num::NonZeroU64;

/// Identity of one lease, minted by whoever issues it when it is issued and
/// never reused. Portable: ASCII letters, digits, `_` and `-`, at most
/// [`Self::MAX_BYTES`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LeaseId(Box<str>);
impl LeaseId {
    /// Longest lease identity a record may carry.
    pub const MAX_BYTES: usize = 128;
    /// Accept `value` when it is a nonempty portable key of at most
    /// [`Self::MAX_BYTES`]; otherwise [`ExecutionError::InvalidLeaseId`].
    pub fn new(value: impl Into<String>) -> Result<Self, ExecutionError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > Self::MAX_BYTES
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        {
            return Err(ExecutionError::InvalidLeaseId);
        }
        Ok(Self(value.into_boxed_str()))
    }
    /// The identity as it was given.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Which issuance of a conversation's leases this is: the first lease a
/// conversation is given is revision one, and each lease issued or refused
/// after it takes the next. Renewing a lease does not change it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LeaseRevision(NonZeroU64);
impl LeaseRevision {
    /// The revision of a conversation's first lease.
    pub const FIRST: Self = Self(NonZeroU64::MIN);
    /// Accept a revision read back from a record; zero names no issuance.
    pub fn new(value: u64) -> Result<Self, ExecutionError> {
        NonZeroU64::new(value)
            .map(Self)
            .ok_or(ExecutionError::InvalidLeaseRevision)
    }
    /// The revision after this one, or `None` past the last representable one.
    pub fn next(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }
    /// The number.
    pub fn get(self) -> u64 {
        self.0.get()
    }
}

/// Where a lease's agent runs: the process that keeps the conversation,
/// running the agent as a child process, or a machine reached over SSH whose
/// `nessa env serve` runs the agent's harness next to its files.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum EnvironmentRef {
    /// This gateway's own process and machine.
    Here,
    /// The machine this OpenSSH destination reaches.
    Ssh(SshDestination),
}

/// An OpenSSH destination a person named for an environment, exactly as it is
/// handed to `ssh`: a `~/.ssh/config` alias, `host`, or `user@host`.
///
/// Built from an allowed alphabet rather than refusing known-bad input, so it
/// can never be read as an option or a second argument: ASCII letters, digits,
/// `.`, `_`, `-` and `@`, starting with a letter or digit, at most
/// [`Self::MAX_BYTES`]. A port, a jump host or an IPv6 literal is said in
/// `~/.ssh/config` under an alias, as for any other OpenSSH option.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SshDestination(Box<str>);
impl SshDestination {
    /// Longest destination accepted: a DNS name's bound.
    pub const MAX_BYTES: usize = 253;
    /// Accept `value` when it is in the alphabet above; otherwise
    /// [`ExecutionError::InvalidSshDestination`].
    pub fn new(value: impl Into<String>) -> Result<Self, ExecutionError> {
        let value = value.into();
        let starts_plainly = value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric());
        if !starts_plainly
            || value.len() > Self::MAX_BYTES
            || !value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'@')
            })
        {
            return Err(ExecutionError::InvalidSshDestination);
        }
        Ok(Self(value.into_boxed_str()))
    }
    /// The destination as it was given.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What the environment must enforce around the agent's commands. A profile
/// is declared by whoever can set it up and refused where it cannot be, never
/// silently weakened ("Sandboxes, honestly").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SandboxProfile {
    /// Whatever the harness does by default, configured by nothing Nessa sets.
    /// It encloses only what the harness itself encloses.
    HarnessDefault,
    /// Nothing encloses the work: it runs as the environment's account. What
    /// a command lease's environment holds, since a command has no harness to
    /// sandbox it.
    None,
}

/// The sandbox profiles a binding can set up, or an environment can enforce.
/// A set rather than one profile, so a request is answered by membership and
/// an empty set refuses every request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SandboxProfiles {
    harness_default: bool,
    none: bool,
}
impl SandboxProfiles {
    /// No profile at all: every request is refused.
    pub const NONE: Self = Self {
        harness_default: false,
        none: false,
    };
    /// Only the harness's own default. What every pinned binding declares today.
    pub const HARNESS_DEFAULT: Self = Self {
        harness_default: true,
        none: false,
    };
    /// Only [`SandboxProfile::None`]: what an environment declares for the
    /// commands it runs, which nothing encloses.
    pub const UNENCLOSED: Self = Self {
        harness_default: false,
        none: true,
    };
    /// Whether `profile` is in the set.
    pub fn contains(self, profile: SandboxProfile) -> bool {
        match profile {
            SandboxProfile::HarnessDefault => self.harness_default,
            SandboxProfile::None => self.none,
        }
    }
    /// The profiles both sets hold: what a binding can set up and an
    /// environment can enforce together.
    pub fn intersect(self, other: Self) -> Self {
        Self {
            harness_default: self.harness_default && other.harness_default,
            none: self.none && other.none,
        }
    }
    /// Grant `requested` when this set holds it, or refuse it with
    /// [`LeaseRefusal::SandboxUnavailable`]. What is granted is what is
    /// recorded, never what was asked (row L1).
    pub fn admit(self, requested: SandboxProfile) -> Result<SandboxProfile, LeaseRefusal> {
        if self.contains(requested) {
            Ok(requested)
        } else {
            Err(LeaseRefusal::SandboxUnavailable)
        }
    }
}

/// An agent a lease runs: the binding it is started with and its model.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AgentWork {
    agent: Box<str>,
    model: Box<str>,
}
impl AgentWork {
    /// Longest agent name a lease may carry.
    pub const MAX_AGENT_BYTES: usize = 64;
    /// Longest model name a lease may carry.
    pub const MAX_MODEL_BYTES: usize = 256;
    /// The agent named `agent` running `model`. Either blank is
    /// [`ExecutionError::EmptyValue`]; either past its bound
    /// [`ExecutionError::ValueTooLong`].
    pub fn new(agent: impl Into<String>, model: impl Into<String>) -> Result<Self, ExecutionError> {
        let agent = bounded(agent.into(), "lease agent", Self::MAX_AGENT_BYTES)?;
        let model = bounded(model.into(), "lease model", Self::MAX_MODEL_BYTES)?;
        Ok(Self { agent, model })
    }
    /// The binding's name.
    pub fn agent(&self) -> &str {
        &self.agent
    }
    /// The model's name.
    pub fn model(&self) -> &str {
        &self.model
    }
}

fn bounded(value: String, field: &'static str, max: usize) -> Result<Box<str>, ExecutionError> {
    if value.trim().is_empty() {
        return Err(ExecutionError::EmptyValue(field));
    }
    if value.len() > max {
        return Err(ExecutionError::ValueTooLong {
            field,
            max_bytes: max,
        });
    }
    Ok(value.into_boxed_str())
}

/// What a lease runs.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum LeaseWork {
    /// One conversation's agent, across its successive turns while the lease
    /// is live.
    Agent(AgentWork),
}

/// One bounded command a command lease runs: its argument vector, the
/// directory beneath the environment's workspace it runs in, and how long it
/// may take. Never a shell line: `argv[0]` is the program, and each argument
/// reaches it as given.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CommandWork {
    argv: Vec<Box<str>>,
    cwd: Option<Box<str>>,
    timeout_ms: u64,
}
impl CommandWork {
    /// Most arguments, the program included.
    pub const MAX_ARGS: usize = 256;
    /// Most bytes of all arguments together: with every record field around
    /// them, a lease record of the command stays within its stored bound
    /// however its arguments encode.
    pub const MAX_ARGV_BYTES: usize = 4 * 1024;
    /// Longest working directory.
    pub const MAX_CWD_BYTES: usize = 1024;
    /// Longest a command may run.
    pub const MAX_TIMEOUT_MS: u64 = 3_600_000;
    /// How long a command may run when its caller names no timeout.
    pub const DEFAULT_TIMEOUT_MS: u64 = 120_000;

    /// `argv` run in `cwd` within `timeout_ms`.
    ///
    /// `cwd` is relative to the environment's workspace, `None` for the
    /// workspace itself: a path of plain components, never absolute and never
    /// `.` or `..`, so it cannot name a directory outside the workspace by its
    /// spelling. An empty `argv`, or a blank program, is
    /// [`ExecutionError::EmptyValue`]; one past its bounds
    /// [`ExecutionError::TooManyValues`] or [`ExecutionError::ValueTooLong`];
    /// a control character other than newline or tab in an argument,
    /// [`ExecutionError::InvalidCommandArgument`]; a `cwd` that is not such a
    /// path, [`ExecutionError::InvalidPath`]; a timeout of zero or past
    /// [`Self::MAX_TIMEOUT_MS`] [`ExecutionError::InvalidCommandTimeout`].
    pub fn new(
        argv: Vec<String>,
        cwd: Option<String>,
        timeout_ms: u64,
    ) -> Result<Self, ExecutionError> {
        if argv.first().is_none_or(|program| program.trim().is_empty()) {
            return Err(ExecutionError::EmptyValue("command program"));
        }
        if argv.len() > Self::MAX_ARGS {
            return Err(ExecutionError::TooManyValues {
                field: "command arguments",
                max: Self::MAX_ARGS,
            });
        }
        if argv.iter().map(String::len).sum::<usize>() > Self::MAX_ARGV_BYTES {
            return Err(ExecutionError::ValueTooLong {
                field: "command arguments",
                max_bytes: Self::MAX_ARGV_BYTES,
            });
        }
        if argv.iter().any(|argument| {
            argument
                .chars()
                .any(|c| c.is_control() && c != '\n' && c != '\t')
        }) {
            return Err(ExecutionError::InvalidCommandArgument);
        }
        let cwd = match cwd {
            None => None,
            Some(cwd) if cwd.len() > Self::MAX_CWD_BYTES => {
                return Err(ExecutionError::ValueTooLong {
                    field: "command working directory",
                    max_bytes: Self::MAX_CWD_BYTES,
                })
            }
            Some(cwd) if !plain_relative(&cwd) => return Err(ExecutionError::InvalidPath),
            Some(cwd) => Some(cwd.into_boxed_str()),
        };
        if timeout_ms == 0 || timeout_ms > Self::MAX_TIMEOUT_MS {
            return Err(ExecutionError::InvalidCommandTimeout);
        }
        Ok(Self {
            argv: argv.into_iter().map(String::into_boxed_str).collect(),
            cwd,
            timeout_ms,
        })
    }
    /// The program and its arguments.
    pub fn argv(&self) -> &[Box<str>] {
        &self.argv
    }
    /// The program: the first argument.
    pub fn program(&self) -> &str {
        &self.argv[0]
    }
    /// The directory beneath the workspace, or `None` for the workspace.
    pub fn cwd(&self) -> Option<&str> {
        self.cwd.as_deref()
    }
    /// How long it may run, in milliseconds.
    pub fn timeout_ms(&self) -> u64 {
        self.timeout_ms
    }
}

/// Whether `path` is relative and made only of plain components separated
/// by single slashes: no empty, `.` or `..` component, no control character.
fn plain_relative(path: &str) -> bool {
    !path.is_empty()
        && path
            .split('/')
            .all(|part| !matches!(part, "" | "." | "..") && !part.chars().any(char::is_control))
}

/// What a command lease runs, where, and enclosed by what: its terms, as
/// granted. A command lease is a child of its conversation's agent lease and
/// lapses with its command's timeout or its parent's end.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CommandTerms {
    /// Where it runs.
    pub environment: EnvironmentRef,
    /// What it runs.
    pub command: CommandWork,
    /// The sandbox the environment enforces around it.
    pub sandbox: SandboxProfile,
}

/// How a command lease's command ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CommandExit {
    /// It exited by itself with this status.
    Exited {
        /// Its exit status.
        code: i32,
    },
    /// A signal ended it that its environment did not send.
    Signalled {
        /// The signal's number.
        signal: i32,
    },
    /// Its timeout passed and its environment stopped it.
    TimedOut,
    /// It was stopped before it finished: its caller went away, or its
    /// parent lease ended.
    Stopped {
        /// Why.
        cause: LeaseEndCause,
    },
    /// Its program could not be started; nothing ran.
    NotStarted,
    /// Its environment did not say how it ended: the connection to it was lost,
    /// or no answer came in time.
    Unanswered,
}

/// What a command printed, as kept in its lease's record: how much of each
/// stream was captured, how much of both together was dropped past the
/// capture bound, and the last bytes of each, as text. A capture drops from
/// both streams together and counts the drop once, so a stream's own total
/// is not known: what each carried is at least what was captured of it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CommandOutput {
    stdout_bytes: u64,
    stderr_bytes: u64,
    dropped_bytes: u64,
    stdout_tail: Box<str>,
    stderr_tail: Box<str>,
}
impl CommandOutput {
    /// Most bytes of each stream's tail a record keeps.
    pub const MAX_TAIL_BYTES: usize = 2 * 1024;

    /// `stdout_bytes` and `stderr_bytes` captured, `dropped_bytes` not
    /// captured past the bound, and the last of each stream. A tail is kept as
    /// at most [`Self::MAX_TAIL_BYTES`] of its end, cut at a character
    /// boundary, with every control character but newline and tab shown as
    /// U+FFFD, so a record of it stays within its bound however it encodes.
    pub fn new(
        stdout_bytes: u64,
        stderr_bytes: u64,
        dropped_bytes: u64,
        stdout_tail: &str,
        stderr_tail: &str,
    ) -> Self {
        Self {
            stdout_bytes,
            stderr_bytes,
            dropped_bytes,
            stdout_tail: tail(stdout_tail),
            stderr_tail: tail(stderr_tail),
        }
    }
    /// Bytes of its standard output captured.
    pub fn stdout_bytes(&self) -> u64 {
        self.stdout_bytes
    }
    /// Bytes of its standard error captured.
    pub fn stderr_bytes(&self) -> u64 {
        self.stderr_bytes
    }
    /// Bytes of both streams together not captured past the bound.
    pub fn dropped_bytes(&self) -> u64 {
        self.dropped_bytes
    }
    /// The last of its standard output.
    pub fn stdout_tail(&self) -> &str {
        &self.stdout_tail
    }
    /// The last of its standard error.
    pub fn stderr_tail(&self) -> &str {
        &self.stderr_tail
    }
}

/// The end of `text`, at most [`CommandOutput::MAX_TAIL_BYTES`] once its
/// control characters are replaced, starting at a character boundary.
fn tail(text: &str) -> Box<str> {
    let shown: String = text
        .chars()
        .map(|c| match c {
            '\n' | '\t' => c,
            c if c.is_control() => char::REPLACEMENT_CHARACTER,
            c => c,
        })
        .collect();
    let mut start = shown.len().saturating_sub(CommandOutput::MAX_TAIL_BYTES);
    while !shown.is_char_boundary(start) {
        start += 1;
    }
    shown[start..].into()
}

/// What a lease lets its environment reach beyond the agent itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LeaseGrants {
    /// Exactly what the harness opening already carries — its Nessa tools
    /// relay token and nothing held for it by digest — with no narrowing by
    /// the lease.
    Opening,
}

/// When a lease lapses without renewal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LeaseDeadline {
    /// Only when it is ended: the environment is the process that keeps the
    /// conversation, so there is nobody for it to outlive.
    UntilEnded,
    /// At this many milliseconds since the Unix epoch.
    At(u64),
}

/// Everything a lease grants, as it was granted.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LeaseTerms {
    /// Where the work runs.
    pub environment: EnvironmentRef,
    /// What runs.
    pub work: LeaseWork,
    /// The sandbox the environment enforces around it.
    pub sandbox: SandboxProfile,
    /// What else it may reach.
    pub grants: LeaseGrants,
    /// When it lapses without renewal.
    pub deadline: LeaseDeadline,
}

/// Why a lease ended. The first cause recorded is the lease's cause; a later
/// one joins it and is not recorded again (rows L5 and L6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LeaseEndCause {
    /// A person, or a system stop such as the gateway retiring, stopped the
    /// work while the conversation stays.
    Stopped,
    /// The conversation was closed or deleted.
    Closed,
    /// The grant it ran under was withdrawn.
    Revoked,
    /// Its deadline passed without renewal.
    Expired,
    /// The environment no longer holds it: the connection to it was lost, or
    /// it or the gateway started again.
    Lost,
}

/// What an environment reported about removing what a lease created.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LeaseCleanup {
    /// The harness process tree and its session state were released.
    /// `forced` says whether that took forced termination.
    Confirmed {
        /// Whether cleanup needed forced termination.
        forced: bool,
    },
    /// The environment holds nothing for the lease: none of what it is
    /// running was started under it. Asked about a lease issued before it
    /// last started, that is all it can say; whether something started then
    /// outlived it is not known.
    NotHeld,
}

/// Why a lease was not issued.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LeaseRefusal {
    /// The requested sandbox profile is not one both the binding can set up
    /// and the environment can enforce.
    SandboxUnavailable,
    /// The environment could not be reached, or did not answer before its
    /// deadline: the connection to it failed or ended.
    EnvironmentUnreachable,
    /// The environment runs another build than the one asking: its lease
    /// frames are not this build's, and nothing was sent to it.
    EnvironmentVersionMismatch,
    /// The environment is already serving another connection, and serves one
    /// at a time.
    EnvironmentBusy,
    /// The environment had no copy of this build, and runs on a system or
    /// processor this build cannot run on, so none was installed there.
    EnvironmentPlatformUnsupported,
    /// The environment had no copy of this build, and installing one failed:
    /// what arrived did not verify, did not run, or could not be stored.
    EnvironmentInstallFailed,
    /// The environment cannot run this agent: its binding cannot start its
    /// harness elsewhere, or the environment has no runtime configured for it.
    AgentUnavailable,
}

/// Why a command lease was not issued: the refusals every lease can meet, and
/// those only a command meets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CommandRefusal {
    /// The environment refused it as it refuses any lease: a sandbox profile
    /// it cannot enforce, unreachable, another build, or busy.
    Environment(LeaseRefusal),
    /// The caller holds no grant to run commands on the environment it named,
    /// or names an environment this gateway does not know.
    EnvironmentNotGranted,
    /// The caller's tool policy does not allow this command.
    CommandDenied,
    /// Past a budget: as many commands already run under the agent's lease as
    /// it may hold at once.
    BudgetExceeded,
    /// The environment runs no commands: it does not serve them, or it is
    /// this gateway's own machine, whose commands go through its shell tool.
    CommandsUnavailable,
}
