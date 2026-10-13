//! Test scopes use the existing private-directory owner on each supported platform.
use nessa_local_storage::create_directory;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

pub(crate) struct PrivateFixture {
    _lifetime: TempDir,
    root: PathBuf,
}
impl PrivateFixture {
    pub(crate) fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        // Match Auth's platform fixture: Windows validates ordinary drive paths
        // and creates protected DACLs; Unix resolves the system temp alias first.
        #[cfg(windows)]
        let root = temporary.path().join("root");
        #[cfg(not(windows))]
        let root = temporary.path().canonicalize().unwrap().join("root");
        create_directory(&root).unwrap();
        Self {
            _lifetime: temporary,
            root,
        }
    }
    pub(crate) fn path(&self) -> &Path {
        &self.root
    }
}
