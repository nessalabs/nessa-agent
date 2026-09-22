use std::fmt;
use std::fs::File;
use std::path::{Path, PathBuf};

use crate::agent_install::domain::{
    AgentName, ArchiveDigest, InstallTransition, PinnedRelease, RuntimeArtifact,
};

/// Why an archive could not be fetched.
///
/// Typed rather than a message because the caller acts on the difference: a
/// user who is offline is told something different from one whose pin points at
/// a release that has been unpublished, and only the first is worth a retry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceFailure {
    /// Nothing answered, or the transfer came apart part-way.
    Unreachable(String),
    /// Something answered and said no. Carries the status so a 404 — the pin
    /// naming a release that no longer exists — is distinguishable from a 503.
    Refused(u16),
    /// The response kept coming past any size an agent runtime is. Not a
    /// rejection of the archive — nothing has been measured yet — but a refusal
    /// to keep filling a disk on the strength of a `Content-Length` nobody
    /// checked.
    TooLarge(u64),
    /// The bytes arrived and this machine could not keep them: a full disk, or
    /// a file that stopped being writable part-way. Its own variant because
    /// telling somebody the download could not be reached when their disk is
    /// full sends them to look at the wrong thing.
    NotStored(String),
}

impl fmt::Display for SourceFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreachable(detail) => write!(f, "could not reach the release archive: {detail}"),
            Self::Refused(status) => {
                write!(f, "the release archive was refused with status {status}")
            }
            Self::TooLarge(limit) => {
                write!(f, "the release archive is larger than {limit} bytes")
            }
            Self::NotStored(detail) => {
                write!(f, "could not store the release archive: {detail}")
            }
        }
    }
}

impl std::error::Error for SourceFailure {}

/// Why the machine could not hold, read, or unpack an installed runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreFailure {
    /// A directory could not be created, or a file could not be written.
    Unwritable(String),
    /// A file that should be there could not be read.
    Unreadable(String),
    /// The archive did not contain the file the release says is its executable.
    /// A pin that is wrong about its own contents, not a machine that failed.
    MissingExecutable(String),
    /// The archive is not a well-formed gzip tar.
    MalformedArchive(String),
}

impl fmt::Display for StoreFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unwritable(detail) => write!(f, "could not write the agent runtime: {detail}"),
            Self::Unreadable(detail) => write!(f, "could not read the agent runtime: {detail}"),
            Self::MissingExecutable(path) => {
                write!(f, "the release archive does not contain {path}")
            }
            Self::MalformedArchive(detail) => {
                write!(f, "the release archive could not be unpacked: {detail}")
            }
        }
    }
}

impl std::error::Error for StoreFailure {}

/// The durable audit sink did not acknowledge install evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditFailure(pub String);

impl fmt::Display for AuditFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for AuditFailure {}

/// Which state change a successful publication performed while holding the
/// agent's publication lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublicationChange {
    /// Another install had already published this exact artifact.
    Reused,
    /// No valid runtime record existed before this artifact was published.
    Installed,
    /// This artifact replaced the runtime named by the prior valid record.
    Replaced(RuntimeArtifact),
}

/// Keeps the store's per-agent publication authority until audit delivery has
/// completed. Implementations normally own the publication lock handle.
pub trait PublicationLease: Send {}

impl<T: Send> PublicationLease for T {}

/// A runtime publication and the state it actually changed under the store's
/// publication lock.
pub struct Publication {
    executable: PathBuf,
    change: PublicationChange,
    _lease: Box<dyn PublicationLease>,
}

impl fmt::Debug for Publication {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Publication")
            .field("executable", &self.executable)
            .field("change", &self.change)
            .finish_non_exhaustive()
    }
}

impl Publication {
    pub fn new(
        executable: PathBuf,
        change: PublicationChange,
        lease: Box<dyn PublicationLease>,
    ) -> Self {
        Self {
            executable,
            change,
            _lease: lease,
        }
    }

    pub fn executable(&self) -> &Path {
        &self.executable
    }

    pub fn change(&self) -> &PublicationChange {
        &self.change
    }
}

/// The state left after the store withdrew a publication that could not be
/// completed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RollbackChange {
    Restored(RuntimeArtifact),
    NoInstalledRuntime,
}

/// Every cleanup failure observed after publication failed. The original
/// publication failure remains separate on [`PublishFailure`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicationCleanupFailure {
    withdrawal: Option<StoreFailure>,
    restoration: Option<StoreFailure>,
    confirmation: Option<StoreFailure>,
}

impl PublicationCleanupFailure {
    pub fn new(
        withdrawal: Option<StoreFailure>,
        restoration: Option<StoreFailure>,
        confirmation: Option<StoreFailure>,
    ) -> Option<Self> {
        (withdrawal.is_some() || restoration.is_some() || confirmation.is_some()).then_some(Self {
            withdrawal,
            restoration,
            confirmation,
        })
    }

    pub fn withdrawal(&self) -> Option<&StoreFailure> {
        self.withdrawal.as_ref()
    }

    pub fn restoration(&self) -> Option<&StoreFailure> {
        self.restoration.as_ref()
    }

    pub fn confirmation(&self) -> Option<&StoreFailure> {
        self.confirmation.as_ref()
    }
}

impl fmt::Display for PublicationCleanupFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let failures = [
            self.withdrawal
                .as_ref()
                .map(|failure| ("withdrawal", failure)),
            self.restoration
                .as_ref()
                .map(|failure| ("restoration", failure)),
            self.confirmation
                .as_ref()
                .map(|failure| ("confirmation", failure)),
        ];
        let mut separator = "";
        for failure in failures.into_iter().flatten() {
            write!(formatter, "{separator}{}: {}", failure.0, failure.1)?;
            separator = "; ";
        }
        Ok(())
    }
}

/// What cleanup after a failed publication established while retaining the
/// publication lease.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublicationRecovery {
    /// Publication failed before it changed installed state.
    NotRequired,
    /// The prior installed state was durably restored.
    RolledBack(RollbackChange),
    /// Cleanup failed. `rollback` is present only when the installed state was
    /// nevertheless re-read and confirmed after that failure.
    Incomplete {
        rollback: Option<RollbackChange>,
        cleanup: PublicationCleanupFailure,
    },
}

/// A publication failure, including a rollback the store actually performed.
pub struct PublishFailure {
    failure: StoreFailure,
    recovery: PublicationRecovery,
    _lease: Box<dyn PublicationLease>,
}

impl PublishFailure {
    pub fn unchanged(failure: StoreFailure) -> Self {
        Self {
            failure,
            recovery: PublicationRecovery::NotRequired,
            _lease: Box::new(()),
        }
    }

    pub fn rolled_back(
        failure: StoreFailure,
        rollback: RollbackChange,
        lease: Box<dyn PublicationLease>,
    ) -> Self {
        Self {
            failure,
            recovery: PublicationRecovery::RolledBack(rollback),
            _lease: lease,
        }
    }

    pub fn incomplete(
        failure: StoreFailure,
        rollback: Option<RollbackChange>,
        cleanup: PublicationCleanupFailure,
        lease: Box<dyn PublicationLease>,
    ) -> Self {
        Self {
            failure,
            recovery: PublicationRecovery::Incomplete { rollback, cleanup },
            _lease: lease,
        }
    }

    pub fn failure(&self) -> &StoreFailure {
        &self.failure
    }

    pub fn recovery(&self) -> &PublicationRecovery {
        &self.recovery
    }
}

impl fmt::Debug for PublishFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PublishFailure")
            .field("failure", &self.failure)
            .field("recovery", &self.recovery)
            .finish_non_exhaustive()
    }
}

/// Durable evidence for agent-runtime installation transitions.
pub trait InstallAudit: Send + Sync {
    /// Commit one immutable transition before the install reports its outcome.
    fn record(&self, transition: InstallTransition) -> Result<(), AuditFailure>;
}

/// The private file one install downloads its archive into.
///
/// An open handle rather than a path, and that is the whole point of the type.
/// The order this context exists to guarantee is download, measure, accept,
/// unpack: a *path* can name a different file at each of those steps, so a
/// digest taken from one open and an unpack from another prove nothing about
/// each other. One handle, opened once, cannot come apart that way.
///
/// The store creates it exclusively, and where the platform allows it lets go
/// of the name at once. Where it does, what that buys is precise and worth
/// stating precisely: nothing holding only the *name* can reach the file — not
/// to truncate it between the hash and the unpack, not to replace it — and two
/// installs running at the same time stage into two files rather than over one
/// another. It is not unreachable in general: a process running as the same
/// user can still find the open descriptor, and a user who can do that can
/// equally write over the installed runtime afterwards. That is the limit of
/// what this can defend.
///
/// Where the platform does not allow it — today that is Windows, where a file
/// cannot be unlinked while it is open — the name survives, and with it the
/// substitution this type exists to prevent: a same-user process could rewrite
/// the staged file between the digest and the unpack, and the install would
/// then measure one thing and unpack another. Nothing reaches that path at
/// present, because no release is pinned for Windows and the use case refuses a
/// platform the pin does not cover before it stages anything. Pinning one is
/// what would make this real, so it is written down here rather than discovered
/// then.
///
/// The use case never reads or writes it — it only passes it along in order,
/// and hands it back to be discarded.
#[derive(Debug)]
pub struct StagedArchive {
    file: File,
    path: PathBuf,
}

impl StagedArchive {
    /// Take ownership of a file the store has just created for this install.
    ///
    /// For store adapters. The file is expected to be private, empty, and
    /// reachable by no name anything else knows.
    pub fn new(file: File, path: PathBuf) -> Self {
        Self { file, path }
    }

    /// The handle every step reads and writes through.
    pub fn file_mut(&mut self) -> &mut File {
        &mut self.file
    }

    /// The name the file was created under.
    ///
    /// Not a name that necessarily still refers to it: a store is expected to
    /// release it as soon as the file exists. It is kept so that a store which
    /// cannot do that has something to remove, and so a diagnostic can say
    /// where the download was.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Where release archives come from.
///
/// A port with one method so the install use case can be tested without a
/// network: every test in this context substitutes an archive that is already
/// on disk, or a failure, and none of them reaches the internet.
///
/// The archive is written into a file rather than returned as bytes because
/// these archives are large — a runtime is on the order of a hundred megabytes —
/// and holding one in memory to hash it and then again to unpack it is a cost
/// with nothing to show for it.
pub trait ArchiveSource: Send + Sync {
    /// Fetch `url` into `staged`, which is empty when this is called.
    ///
    /// A failure may leave bytes in `staged`; the caller discards it either way
    /// and never asks for its digest.
    fn download(&self, url: &str, staged: &mut StagedArchive) -> Result<(), SourceFailure>;
}

/// Where installed runtimes live on this machine.
///
/// The filesystem side of installing, behind one port so that the use case
/// below owns the *order* — download, hash, accept, publish — and none of the
/// effects. The methods are deliberately shaped around that order rather than
/// as a general filesystem: there is no "write this file anywhere" here,
/// because the point of the type is that an agent runtime can only be put in
/// the one place Nessa manages.
pub trait RuntimeStore: Send + Sync {
    /// Where `release` is already installed for `agent`, if it really is.
    ///
    /// `None` is a real answer — this release is not installed — and is what
    /// makes a second install of the same pin free.
    ///
    /// Deliberately asked about one release rather than "what is installed":
    /// the only thing a caller can do with the answer is skip a download it
    /// would otherwise start, and a store that answered more generally would be
    /// handing out a launch path for a runtime nobody named.
    ///
    /// The implementation reports `Some` only when what it recorded agrees with
    /// `release` on both the version and the executable, and when that
    /// executable is really on the disk. Answering from the record alone would
    /// let a half-finished install, or one whose executable was since deleted,
    /// be reported as a runtime Nessa can launch. Keeping those checks here
    /// rather than at the call site is what lets the use case above touch no
    /// files.
    fn installed(
        &self,
        agent: &AgentName,
        release: &PinnedRelease,
    ) -> Result<Option<PathBuf>, StoreFailure>;

    /// Create a private file this install may download into.
    ///
    /// Owned by the store rather than chosen by the caller so that a partial
    /// download is always somewhere the store knows how to clean up, never
    /// beside the executable that is still in use, and never a name a second
    /// install would pick too.
    fn stage(&self, agent: &AgentName) -> Result<StagedArchive, StoreFailure>;

    /// The SHA-256 of what has been staged.
    fn digest(&self, staged: &mut StagedArchive) -> Result<ArchiveDigest, StoreFailure>;

    /// Unpack the release's executable out of `staged` and make it the
    /// installed runtime for `agent`, returning where it now is.
    ///
    /// Called only after [`PinnedRelease::accept`] has passed, and given the
    /// same open file that was measured, so an implementation may assume it is
    /// unpacking the pinned bytes. It may not assume anything about their
    /// *contents*: the executable named by the pin can still be absent, which
    /// is [`StoreFailure::MissingExecutable`].
    ///
    /// Durable on return: an executable this reports is one a machine that
    /// loses power immediately afterwards still has.
    fn publish(
        &self,
        agent: &AgentName,
        release: &PinnedRelease,
        staged: &mut StagedArchive,
    ) -> Result<Publication, PublishFailure>;

    /// Forget a staged download. Never fails the install: a leftover file in a
    /// directory Nessa owns is survivable, and reporting it would turn a
    /// successful install into a failed one.
    fn discard(&self, staged: StagedArchive);
}
