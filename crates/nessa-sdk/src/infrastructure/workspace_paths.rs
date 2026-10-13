//! Where a review's target paths really are on this machine.
//!
//! A read or a search is auto-allowed by its deadline only when every target
//! path resolves inside the session workspace. Resolving a path reads the
//! filesystem, so it is reached through [`WorkspacePaths`]. Composition
//! supplies [`FilesystemPaths`] through `AcpConfig::workspace_paths`; a test
//! supplies one that answers without a disk, or one that never answers, to
//! check what a slow or missing filesystem means for a review.
#![deny(missing_docs)]

use std::path::{Path, PathBuf};

/// Resolves a path to the one it names after links and `..` are followed.
///
/// The ACP worker calls this off its own task and stops waiting after a short
/// bound, so an implementation may block. `None` means the path did not
/// resolve: it is missing, unreadable, or the filesystem refused.
pub trait WorkspacePaths: Send + Sync {
    /// The canonical form of `path`, or `None` when it does not resolve.
    fn resolve(&self, path: &Path) -> Option<PathBuf>;
}

/// A function is a resolver: what a test that needs one answer writes.
impl<F> WorkspacePaths for F
where
    F: Fn(&Path) -> Option<PathBuf> + Send + Sync,
{
    fn resolve(&self, path: &Path) -> Option<PathBuf> {
        self(path)
    }
}

/// Resolves nothing. A workspace on another host cannot be resolved on this
/// machine: a local file at the same path is not the host's file. Every read
/// there is the deny class until a host supplies its own resolver.
#[derive(Clone, Copy, Debug, Default)]
pub struct UnresolvedPaths;

impl WorkspacePaths for UnresolvedPaths {
    fn resolve(&self, _path: &Path) -> Option<PathBuf> {
        None
    }
}

/// This machine's filesystem, through [`std::fs::canonicalize`].
#[derive(Clone, Copy, Debug, Default)]
pub struct FilesystemPaths;

impl WorkspacePaths for FilesystemPaths {
    fn resolve(&self, path: &Path) -> Option<PathBuf> {
        path.canonicalize().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_filesystem_resolves_what_exists_and_nothing_else() {
        let root = tempfile::tempdir().unwrap();
        let canonical_root = root.path().canonicalize().unwrap();
        assert_eq!(
            FilesystemPaths.resolve(&root.path().join(".")),
            Some(canonical_root)
        );
        assert_eq!(FilesystemPaths.resolve(&root.path().join("missing")), None);
        assert_eq!(UnresolvedPaths.resolve(root.path()), None);
    }
}
