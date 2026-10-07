//! Authorization evidence on disk. A record names the server, the action,
//! whether it was the intent or the outcome, the generation, the resource
//! and the phase. It never contains a token.
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use async_trait::async_trait;

use crate::mcp_authorization::application::{AuditFailure, AuthAuditRecord, AuthorizationAudit};

pub struct FileAuthorizationAudit {
    path: PathBuf,
    lock: Mutex<()>,
}

impl FileAuthorizationAudit {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            lock: Mutex::new(()),
        }
    }
}

#[async_trait]
impl AuthorizationAudit for FileAuthorizationAudit {
    async fn record(&self, record: &AuthAuditRecord) -> Result<(), AuditFailure> {
        let line = serde_json::json!({
            "server": record.server.to_string(),
            "action": record.action,
            "intent": record.intent,
            "generation": record.generation,
            "resource": record.resource,
            "phase": record.phase,
        });
        let bytes = serde_json::to_vec(&line).map_err(|_| AuditFailure)?;
        let path = self.path.clone();
        let _guard = self.lock.lock().map_err(|_| AuditFailure)?;
        append(&path, &bytes)
    }
}

fn append(path: &Path, line: &[u8]) -> Result<(), AuditFailure> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|_| AuditFailure)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|_| AuditFailure)?;
    file.write_all(line).map_err(|_| AuditFailure)?;
    file.write_all(b"\n").map_err(|_| AuditFailure)?;
    file.sync_all().map_err(|_| AuditFailure)
}
