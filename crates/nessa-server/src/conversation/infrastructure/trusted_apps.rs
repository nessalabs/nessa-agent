//! The apps whose messages the person always allows, kept in one private
//! file: `<root>/trusted-apps.json`, `{"servers": ["<server>", ...]}`.
//!
//! A choice is written to a private temporary file, synced, and moved over
//! the file before it is reported kept; the set in memory changes only
//! after that. A file that cannot be read is no trust at all: every app
//! is asked about, never assumed allowed.
use crate::conversation::application::{TrustNotKept, TrustedApps};
use nessa_local_storage::{open, sync_directory, OpenMode, PrivateTempFile};
use std::{
    collections::BTreeSet,
    io::{Read, Write},
    path::PathBuf,
    sync::{Mutex, PoisonError},
};

/// The most apps one gateway remembers; past it a new one is not kept.
const MAX_TRUSTED_APPS: usize = 256;
/// The most bytes a server name may take, as configured names do.
const MAX_SERVER_BYTES: usize = 256;

pub struct DurableTrustedApps {
    file: PathBuf,
    servers: Mutex<BTreeSet<String>>,
}

impl DurableTrustedApps {
    /// The choices kept under `root`. The file is read once, here.
    pub fn new(root: PathBuf) -> Self {
        let file = root.join("trusted-apps.json");
        let servers = read(&file).unwrap_or_else(|| {
            if file.exists() {
                tracing::error!(
                    ?file,
                    "trusted apps file unreadable; every app is asked about"
                );
            }
            BTreeSet::new()
        });
        Self {
            file,
            servers: Mutex::new(servers),
        }
    }
}

fn read(file: &std::path::Path) -> Option<BTreeSet<String>> {
    let mut text = String::new();
    open(file, OpenMode::Read)
        .ok()?
        .read_to_string(&mut text)
        .ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value
        .get("servers")?
        .as_array()?
        .iter()
        .map(|server| server.as_str().map(str::to_owned))
        .collect()
}

impl TrustedApps for DurableTrustedApps {
    fn trusts(&self, server: &str) -> bool {
        self.servers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(server)
    }

    fn trust(&self, server: &str) -> Result<(), TrustNotKept> {
        if server.is_empty() || server.len() > MAX_SERVER_BYTES {
            return Err(TrustNotKept);
        }
        let mut servers = self.servers.lock().unwrap_or_else(PoisonError::into_inner);
        if servers.contains(server) {
            return Ok(());
        }
        if servers.len() >= MAX_TRUSTED_APPS {
            return Err(TrustNotKept);
        }
        let mut next = servers.clone();
        next.insert(server.to_owned());
        let bytes = serde_json::to_vec(&serde_json::json!({ "servers": next }))
            .map_err(|_| TrustNotKept)?;
        let directory = self.file.parent().ok_or(TrustNotKept)?;
        let written = PrivateTempFile::new_in(directory).and_then(|mut temporary| {
            temporary.as_file_mut().write_all(&bytes)?;
            temporary.as_file_mut().sync_all()?;
            temporary.persist(&self.file)?;
            sync_directory(directory)
        });
        if let Err(cause) = written {
            tracing::error!(%cause, "an app's trust was not kept");
            return Err(TrustNotKept);
        }
        *servers = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A private directory, as the conversations' root is.
    fn private_root() -> (tempfile::TempDir, PathBuf) {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("conversations");
        nessa_local_storage::create_directory(&root).unwrap();
        (parent, root)
    }

    #[test]
    fn a_trusted_app_is_remembered_across_a_restart() {
        let (_parent, root) = private_root();
        let apps = DurableTrustedApps::new(root.clone());
        assert!(!apps.trusts("huddle"));
        apps.trust("huddle").unwrap();
        assert!(apps.trusts("huddle"));
        assert!(!apps.trusts("charts"));
        let again = DurableTrustedApps::new(root);
        assert!(again.trusts("huddle"));
        assert!(!again.trusts("charts"));
    }

    #[test]
    fn an_unreadable_file_trusts_nothing() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("trusted-apps.json"), b"not json").unwrap();
        assert!(!DurableTrustedApps::new(root.path().to_owned()).trusts("huddle"));
    }
}
