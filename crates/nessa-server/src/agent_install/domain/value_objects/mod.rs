//! Values describing one tested release of an agent's own runtime.
mod pinned_release;
pub use pinned_release::{
    ArchiveDigest, ArchivePath, ArchiveRejected, PinRejected, PinnedRelease, ReleasePlatform,
    ReleaseVersion,
};
