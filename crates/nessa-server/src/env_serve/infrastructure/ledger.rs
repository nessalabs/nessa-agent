//! The environment's own audit of its leases: one JSON line per entry,
//! appended and synced before the answer it records is sent, in
//! `environment/leases.jsonl` under the host's data directory. It is also
//! the cleanup evidence `Account` answers with, read back line by line.
//!
//! Audit evidence, never migrated (ADR 202): a line this build cannot read
//! is skipped when accounting, and a lease whose end cannot be read is
//! answered as uncertain, never as released.
use crate::env_serve::application::{LeaseLedger, LedgerEntry};
use nessa_protocol::lease::Cleanup;
use serde::Deserialize;
use std::{
    fs::File,
    io::{self, BufRead, BufReader, Read, Seek, Write},
    path::PathBuf,
    sync::Mutex,
};

/// Most bytes accounting reads of the ledger, from its end: a lease recorded
/// only before them is not found, and answered as not held — all that can
/// be said of a lease that old.
const MAX_READ_BYTES: u64 = 64 * 1024 * 1024;

/// `leases.jsonl`, appended under a lock of its own.
pub(crate) struct FileLedger {
    path: PathBuf,
    append: Mutex<File>,
}

impl FileLedger {
    /// Open, or create, the ledger at `path`, private to this user.
    ///
    /// # Errors
    /// It could not be opened privately.
    pub(crate) fn open(path: PathBuf) -> io::Result<Self> {
        let file = nessa_local_storage::open(&path, nessa_local_storage::OpenMode::OpenOrCreate)?;
        Ok(Self {
            path,
            append: Mutex::new(file),
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Line {
    kind: String,
    #[serde(default)]
    lease: Option<String>,
    #[serde(default)]
    cleanup: Option<Cleanup>,
}

impl LeaseLedger for FileLedger {
    fn record(&self, entry: &LedgerEntry) -> io::Result<()> {
        let mut line = serde_json::to_vec(entry).map_err(io::Error::other)?;
        line.push(b'\n');
        let mut file = self
            .append
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        file.seek(io::SeekFrom::End(0))?;
        file.write_all(&line)?;
        file.sync_data()
    }

    fn accounted(&self, lease: &str) -> io::Result<Cleanup> {
        let mut file = nessa_local_storage::open(&self.path, nessa_local_storage::OpenMode::Read)?;
        let length = file.metadata()?.len();
        let start = length.saturating_sub(MAX_READ_BYTES);
        file.seek(io::SeekFrom::Start(start))?;
        let mut lines = BufReader::new(file.take(MAX_READ_BYTES)).lines();
        if start > 0 {
            // Begun mid-line: that line is not one this read can trust.
            let _ = lines.next();
        }
        let mut granted = false;
        let mut ended = None;
        for line in lines {
            let Ok(line) = line else { break };
            let Ok(line) = serde_json::from_str::<Line>(&line) else {
                continue;
            };
            if line.lease.as_deref() != Some(lease) {
                continue;
            }
            match line.kind.as_str() {
                // A grant after an end is a lease not yet ended again.
                "granted" | "command_granted" => {
                    granted = true;
                    ended = None;
                }
                "ended" => ended = line.cleanup,
                _ => {}
            }
        }
        Ok(match (ended, granted) {
            (Some(cleanup), _) => cleanup,
            (None, true) => Cleanup::Uncertain,
            (None, false) => Cleanup::NotHeld,
        })
    }
}

#[cfg(test)]
#[path = "../../../tests/env_serve/ledger.rs"]
mod tests;
