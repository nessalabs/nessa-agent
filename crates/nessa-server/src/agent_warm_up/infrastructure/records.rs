//! Completion records live in the data directory, because the point of the
//! record is to survive the process that wrote it.
use crate::agent_warm_up::application::{WarmUpError, WarmUpFuture, WarmUpRecords};
use crate::agent_warm_up::domain::RuntimeFingerprint;
use nessa_local_storage::{create_directory, open, sync_directory, OpenMode, PrivateTempFile};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::PathBuf,
};

/// One completed warm-up, written as its own file so a partially written record
/// can never be mistaken for a complete one.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct CompletedWarmUp {
    executable: String,
    entry: String,
    model: String,
    completed_at_ms: u64,
}

const MAX_RECORD_BYTES: usize = 16_384;

/// Records completed warm-ups as files beneath one directory.
pub struct FileWarmUpRecords {
    directory: PathBuf,
}

impl FileWarmUpRecords {
    /// Open, creating the directory when it does not exist.
    ///
    /// # Errors
    /// Fails when the directory cannot be created.
    pub fn new(directory: PathBuf) -> Result<Self, WarmUpError> {
        create_directory(&directory).map_err(|error| WarmUpError::Records(error.to_string()))?;
        Ok(Self { directory })
    }
    // The runtime's identity is long and contains path separators, so it cannot
    // be a filename. The file is named by a bounded hex digest of that identity
    // and still carries the identity inside, which is what equality is decided
    // on: a digest collision would read as a different runtime and warm again.
    fn path(&self, runtime: &RuntimeFingerprint) -> PathBuf {
        self.directory.join(format!("{}.json", digest(runtime)))
    }
}

// Not a security boundary and not a stable format: a short, stable-per-build
// name for a file whose contents are authoritative.
fn digest(runtime: &RuntimeFingerprint) -> String {
    let mut state: u64 = 0xcbf2_9ce4_8422_2325;
    for part in [runtime.executable(), runtime.entry(), runtime.model()] {
        for byte in part.as_bytes() {
            state ^= u64::from(*byte);
            state = state.wrapping_mul(0x0000_0100_0000_01b3);
        }
        state ^= 0xff;
        state = state.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{state:016x}")
}

impl WarmUpRecords for FileWarmUpRecords {
    fn completed(&self, runtime: &RuntimeFingerprint) -> WarmUpFuture<'_, bool> {
        let path = self.path(runtime);
        let runtime = runtime.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let mut file = match open(&path, OpenMode::Read) {
                    Ok(file) => file,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
                    Err(error) => return Err(WarmUpError::Records(error.to_string())),
                };
                let mut bytes = Vec::new();
                Read::by_ref(&mut file)
                    .take(MAX_RECORD_BYTES as u64 + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|error| WarmUpError::Records(error.to_string()))?;
                if bytes.len() > MAX_RECORD_BYTES {
                    return Err(WarmUpError::Records("record exceeds 16 KiB".into()));
                }
                // A record that cannot be read is treated as absent rather than
                // as a failure: warming again is safe, and refusing to warm
                // because of a damaged marker would be worse than the scan.
                let Ok(stored) = serde_json::from_slice::<CompletedWarmUp>(&bytes) else {
                    return Ok(false);
                };
                Ok(stored.executable == runtime.executable()
                    && stored.entry == runtime.entry()
                    && stored.model == runtime.model())
            })
            .await
            .map_err(|error| WarmUpError::Records(error.to_string()))?
        })
    }

    fn record_completed(
        &self,
        runtime: RuntimeFingerprint,
        observed_at_ms: u64,
    ) -> WarmUpFuture<'_, ()> {
        let destination = self.path(&runtime);
        let directory = self.directory.clone();
        let record = CompletedWarmUp {
            executable: runtime.executable().to_owned(),
            entry: runtime.entry().to_owned(),
            model: runtime.model().to_owned(),
            completed_at_ms: observed_at_ms,
        };
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let failed =
                    |error: &dyn std::fmt::Display| WarmUpError::Records(error.to_string());
                let mut file =
                    PrivateTempFile::new_in(&directory).map_err(|error| failed(&error))?;
                serde_json::to_writer(file.as_file_mut(), &record)
                    .map_err(|error| failed(&error))?;
                file.as_file_mut()
                    .write_all(b"\n")
                    .and_then(|()| file.as_file().sync_all())
                    .map_err(|error| failed(&error))?;
                // Rename over any previous record for the same runtime: this
                // states one fact, so repeating it must not accumulate files.
                file.persist(&destination).map_err(|error| failed(&error))?;
                sync_directory(&directory).map_err(|error| failed(&error))
            })
            .await
            .map_err(|error| WarmUpError::Records(error.to_string()))?
        })
    }
}

#[cfg(test)]
#[path = "../../../tests/agent_warm_up/records.rs"]
mod tests;
