//! What Nessa knows about an agent runtime it did not write.
//!
//! One idea: a *pinned release*. Nessa does not track what an agent publishes,
//! and does not install whatever is newest. It names a version it has tested,
//! the archive that version is distributed as, how many bytes that archive is,
//! the digest it must have, and the files inside it that are installed — one of
//! which is the program Nessa launches. Anything that does not match that
//! description is not installed.
//!
//! The rules here are the ones that hold before any file exists: an agent name
//! can also be a directory name, a digest is sixty-four hex characters, a
//! version can also be a directory name, a path inside an archive does not
//! escape it, a release names exactly one program to launch, an archive is
//! fetched over https from a host that is named. Deciding *where* a runtime
//! goes and putting it there is infrastructure.
pub mod entities;
mod release_selection;
pub mod value_objects;
pub use entities::{InstallAttempt, InstallAttemptError, InstallEventAdmission};
pub use release_selection::preferred_release;
pub use value_objects::{
    AgentName, ArchiveDigest, ArchivePath, ArchiveRejected, ArchiveSize, ArchiveUrl, FileRole,
    HostPlatform, InstallEventIdentity, InstallEventSlot, InstallFailureEvidence,
    InstallFailureKind, InstallRequest, InstallRequestError, InstallTransition,
    InstallTransitionError, InstallTransitionFacts, InstallTransitionKind, Libc, NotAnAgentName,
    PinRejected, PinnedRelease, PublicationDeliveryError, PublicationOutcome,
    PublicationPreparation, PublicationSettlement, RecoveryFailureEvidence, RecoveryState,
    ReleaseContents, ReleaseFile, ReleasePlatform, ReleaseRequirements, ReleaseVersion,
    RollbackState, RuntimeArtifact,
};
