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
}

/// The sandbox profiles a binding can set up, or an environment can enforce.
/// A set rather than one profile, so a request is answered by membership and
/// an empty set refuses every request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SandboxProfiles {
    harness_default: bool,
}
impl SandboxProfiles {
    /// No profile at all: every request is refused.
    pub const NONE: Self = Self {
        harness_default: false,
    };
    /// Only the harness's own default. What every pinned binding declares today.
    pub const HARNESS_DEFAULT: Self = Self {
        harness_default: true,
    };
    /// Whether `profile` is in the set.
    pub fn contains(self, profile: SandboxProfile) -> bool {
        match profile {
            SandboxProfile::HarnessDefault => self.harness_default,
        }
    }
    /// The profiles both sets hold: what a binding can set up and an
    /// environment can enforce together.
    pub fn intersect(self, other: Self) -> Self {
        Self {
            harness_default: self.harness_default && other.harness_default,
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
