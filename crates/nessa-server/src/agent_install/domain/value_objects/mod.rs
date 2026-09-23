//! Values describing one tested release of an agent's own runtime, and the
//! machine it would be installed on.
//!
//! The second half is not a stray: a release is chosen by comparing what it
//! needs against what the machine has, and both sides of that comparison are
//! rules that hold before any file exists.
//!
//! ```text
//! pinned_release ──▶ release_contents   what one release installs
//!                └─▶ host_platform      what a machine provides
//! ```
//! Arrows mean "is made of". Nothing here reads a file, a clock or a network.
mod agent_name;
mod device_names;
mod host_platform;
mod install_transition;
mod pinned_release;
mod release_contents;
pub use agent_name::{AgentName, NotAnAgentName};
pub use host_platform::{HostPlatform, Libc, ReleaseRequirements};
pub use install_transition::{
    InstallFailureEvidence, InstallFailureKind, InstallRequest, InstallRequestError,
    InstallTransition, InstallTransitionError, InstallTransitionKind, RecoveryFailureEvidence,
    RecoveryState, RollbackState, RuntimeArtifact,
};
pub use pinned_release::{
    ArchiveDigest, ArchiveRejected, ArchiveSize, ArchiveUrl, PinRejected, PinnedRelease,
    ReleasePlatform, ReleaseVersion,
};
pub use release_contents::{ArchivePath, FileRole, ReleaseContents, ReleaseFile};
