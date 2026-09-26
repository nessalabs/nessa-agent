//! Whether the gateway's conversation data is still on disk, for a refused
//! retirement to say why (ADR 221).
use crate::desktop_runtime::application::ConversationData;
use std::{io::ErrorKind, path::PathBuf};

/// The conversation root this gateway opened at startup.
pub(crate) struct ConversationDirectory(PathBuf);

impl ConversationDirectory {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self(root)
    }
}

impl ConversationData for ConversationDirectory {
    /// Missing only when the filesystem says the root is not there. Any other
    /// answer, including one it cannot give, is not proof that the data is gone.
    fn missing(&self) -> bool {
        matches!(
            std::fs::symlink_metadata(&self.0),
            Err(error) if error.kind() == ErrorKind::NotFound
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_absent_root_is_missing() {
        let namespace = tempfile::tempdir().unwrap();
        let root = namespace.path().join("conversations");
        std::fs::create_dir(&root).unwrap();
        let data = ConversationDirectory::new(root.clone());
        assert!(!data.missing());
        std::fs::remove_dir(&root).unwrap();
        assert!(data.missing());
    }
}
