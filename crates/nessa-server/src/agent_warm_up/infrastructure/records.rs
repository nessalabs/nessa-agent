//! Completion records live in the data directory, because the point of the
//! record is to survive the process that wrote it.
use crate::agent_warm_up::application::{WarmUpError, WarmUpFuture, WarmUpRecords};
use crate::agent_warm_up::domain::RuntimeFingerprint;
use nessa_local_storage::{create_directory, open, sync_directory, OpenMode, PrivateTempFile};
use serde::{Deserialize, Serialize};
use std::{
    fmt::Display,
    io::{ErrorKind, Read, Write},
    path::{Path, PathBuf},
};

/// One completed warm-up, written as its own file so a partially written record
/// can never be mistaken for a complete one.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct CompletedWarmUp {
    provider: String,
    model: String,
    configuration: String,
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
    // The runtime's identity is long and can contain characters a filename
    // cannot, so the file is named by a bounded digest of it and carries the
    // identity inside. Equality is decided on the contents, so a digest
    // collision reads as a different runtime and warms again rather than
    // answering for the wrong one.
    fn path(&self, runtime: &RuntimeFingerprint) -> PathBuf {
        self.directory.join(format!("{}.json", digest(runtime)))
    }
}

// Not a security boundary and not a stable format: a short, stable-per-build
// name for a file whose contents are authoritative.
fn digest(runtime: &RuntimeFingerprint) -> String {
    let mut state: u64 = 0xcbf2_9ce4_8422_2325;
    for part in [runtime.provider(), runtime.model(), runtime.configuration()] {
        for byte in part.as_bytes() {
            state ^= u64::from(*byte);
            state = state.wrapping_mul(0x0000_0100_0000_01b3);
        }
        state ^= 0xff;
        state = state.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{state:016x}")
}

// Always false: the runtime is not known to be warm. Reported rather than
// swallowed, because a marker nobody can read would otherwise warm every boot
// with no explanation.
fn unreadable(path: &Path, error: &dyn Display) -> bool {
    tracing::warn!(path = %path.display(), %error, "ignoring unreadable warm-up record");
    false
}

impl WarmUpRecords for FileWarmUpRecords {
    fn completed(&self, runtime: &RuntimeFingerprint) -> WarmUpFuture<'_, bool> {
        let path = self.path(runtime);
        let runtime = runtime.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                // Every way of failing to read this marker means the same
                // thing: the runtime is not known to be warm. Warming again
                // costs one background launch, while returning a failure here
                // would stop the warm-up before it starts and cost the user
                // their first message on every boot until someone deletes the
                // file by hand. The unreadable file is reported, not swallowed.
                let mut file = match open(&path, OpenMode::Read) {
                    Ok(file) => file,
                    Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
                    Err(error) => return Ok(unreadable(&path, &error)),
                };
                let mut bytes = Vec::new();
                if let Err(error) = Read::by_ref(&mut file)
                    .take(MAX_RECORD_BYTES as u64 + 1)
                    .read_to_end(&mut bytes)
                {
                    return Ok(unreadable(&path, &error));
                }
                if bytes.len() > MAX_RECORD_BYTES {
                    return Ok(unreadable(&path, &"record exceeds 16 KiB"));
                }
                let Ok(stored) = serde_json::from_slice::<CompletedWarmUp>(&bytes) else {
                    return Ok(unreadable(&path, &"record is not a warm-up record"));
                };
                Ok(stored.provider == runtime.provider()
                    && stored.model == runtime.model()
                    && stored.configuration == runtime.configuration())
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
            provider: runtime.provider().to_owned(),
            model: runtime.model().to_owned(),
            configuration: runtime.configuration().to_owned(),
            completed_at_ms: observed_at_ms,
        };
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let failed = |error: &dyn Display| WarmUpError::Records(error.to_string());
                let mut file =
                    PrivateTempFile::new_in(&directory).map_err(|error| failed(&error))?;
                serde_json::to_writer(file.as_file_mut(), &record)
                    .map_err(|error| failed(&error))?;
                file.as_file_mut()
                    .write_all(b"\n")
                    .and_then(|()| file.as_file().sync_all())
                    .map_err(|error| failed(&error))?;
                // Rename over any previous record with this digest: one runtime
                // states one fact, so repeating it must not accumulate files.
                // Two runtimes whose digests collided would evict each other
                // and keep warming; neither ever gets a wrong answer.
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
