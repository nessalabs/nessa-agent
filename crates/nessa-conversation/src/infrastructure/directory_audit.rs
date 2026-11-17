//! Append-only directory audit records. A failed write is the sink being down.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

use crate::application::{AuditUnavailable, DirectoryAudit, DirectoryAuditRecord};

/// One JSON line per record, created when the first record is written.
pub struct FileDirectoryAudit {
    path: PathBuf,
}

impl FileDirectoryAudit {
    /// Records are appended to `path`.
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl DirectoryAudit for FileDirectoryAudit {
    fn record(&self, record: &DirectoryAuditRecord) -> Result<(), AuditUnavailable> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|_| AuditUnavailable)?;
        }
        let mut line = serde_json::to_string(record).map_err(|_| AuditUnavailable)?;
        line.push('\n');
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|_| AuditUnavailable)?;
        file.write_all(line.as_bytes())
            .map_err(|_| AuditUnavailable)?;
        file.sync_all().map_err(|_| AuditUnavailable)?;
        Ok(())
    }
}
