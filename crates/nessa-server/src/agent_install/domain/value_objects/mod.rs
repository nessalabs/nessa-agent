//! Values describing one tested release of an agent's own runtime, and the
//! machine it would be installed on.
//!
//! The second half is not a stray: a release is chosen by comparing what it
//! needs against what the machine has, and both sides of that comparison are
//! rules that hold before any file exists.
mod agent_name;
mod device_names;
mod host_platform;
mod install_transition;
mod pinned_release;
pub use agent_name::{AgentName, NotAnAgentName};
pub use host_platform::{HostPlatform, Libc, ReleaseRequirements};
pub use install_transition::{
    InstallRequest, InstallRequestError, InstallTransition, InstallTransitionError,
    InstallTransitionKind, RollbackState, RuntimeArtifact,
};
pub use pinned_release::{
    ArchiveDigest, ArchivePath, ArchiveRejected, ArchiveUrl, PinRejected, PinnedRelease,
    ReleasePlatform, ReleaseVersion,
};
