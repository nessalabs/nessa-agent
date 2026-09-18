use std::fmt;
use std::path::{Path, PathBuf};

use crate::agent_install::domain::{ArchiveDigest, PinnedRelease, ReleaseVersion};

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
}

impl fmt::Display for SourceFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreachable(detail) => write!(f, "could not reach the release archive: {detail}"),
            Self::Refused(status) => {
                write!(f, "the release archive was refused with status {status}")
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

/// An agent runtime already present on this machine.
///
/// Both halves together: a version with no executable beside it is not an
/// installation, and an executable Nessa cannot name a version for is not one
/// it can say it tested.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledRecord {
    pub version: ReleaseVersion,
    pub executable: PathBuf,
}

/// Where release archives come from.
///
/// A port with one method so the install use case can be tested without a
/// network: every test in this context substitutes an archive that is already
/// on disk, or a failure, and none of them reaches the internet.
///
/// The archive is written to a path rather than returned as bytes because these
/// archives are large — a runtime is on the order of a hundred megabytes — and
/// holding one in memory to hash it and then again to unpack it is a cost with
/// nothing to show for it.
pub trait ArchiveSource: Send + Sync {
    /// Fetch `url` into `destination`, replacing anything already there.
    ///
    /// A failure leaves nothing at `destination` that a caller should treat as
    /// an archive; the caller is expected to discard the path either way.
    fn download(&self, url: &str, destination: &Path) -> Result<(), SourceFailure>;
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
    /// What is installed for `agent` right now.
    ///
    /// `None` is a real answer — nothing usable is installed — and is what
    /// makes a second install of the same pin free.
    ///
    /// The implementation reports `Some` only when the recorded version *and*
    /// the executable it names are both really there. Answering from the record
    /// alone would let a half-finished install, or one whose executable was
    /// since deleted, be reported as a runtime Nessa can launch. Keeping that
    /// check here rather than at the call site is what lets the use case above
    /// touch no files at all.
    fn installed(&self, agent: &str) -> Result<Option<InstalledRecord>, StoreFailure>;

    /// A private path this install may download an archive into.
    ///
    /// Owned by the store rather than chosen by the caller so that a partial
    /// download is always somewhere the store knows how to clean up, and never
    /// beside the executable that is still in use.
    fn scratch(&self, agent: &str) -> Result<PathBuf, StoreFailure>;

    /// The SHA-256 of a file already on disk.
    fn digest(&self, archive: &Path) -> Result<ArchiveDigest, StoreFailure>;

    /// Unpack the release's executable out of `archive` and make it the
    /// installed runtime for `agent`, returning where it now is.
    ///
    /// Called only after [`PinnedRelease::accept`] has passed, so an
    /// implementation may assume the archive is the pinned one. It may not
    /// assume anything about its *contents*: the executable named by the pin
    /// can still be absent, which is [`StoreFailure::MissingExecutable`].
    fn publish(
        &self,
        agent: &str,
        release: &PinnedRelease,
        archive: &Path,
    ) -> Result<PathBuf, StoreFailure>;

    /// Forget a scratch download. Never fails the install: a leftover file in a
    /// directory Nessa owns is survivable, and reporting it would turn a
    /// successful install into a failed one.
    fn discard(&self, scratch: &Path);
}
