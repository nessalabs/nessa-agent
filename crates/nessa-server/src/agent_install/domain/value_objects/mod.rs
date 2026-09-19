//! Values describing one tested release of an agent's own runtime.
mod agent_name;
mod device_names;
mod host_platform;
mod pinned_release;
pub use agent_name::{AgentName, NotAnAgentName};
pub use host_platform::{HostPlatform, Libc, ReleaseRequirements};
pub use pinned_release::{
    ArchiveDigest, ArchivePath, ArchiveRejected, ArchiveUrl, PinRejected, PinnedRelease,
    ReleasePlatform, ReleaseVersion,
};
