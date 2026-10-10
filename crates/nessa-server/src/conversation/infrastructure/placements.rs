//! Where each conversation placed on an SSH host runs: one small file per
//! such conversation, `placements/<conversation id>.json` under the
//! conversations directory, holding `{"schemaVersion":1,"host":"…"}`. A
//! conversation that runs here has none.
//!
//! A record-scope dataset (ADR 202): a file this build cannot read refuses
//! that conversation's opening alone. Written whole and synced before the
//! conversation's record exists; never changed after; removed with the
//! conversation's history. The metadata database is not touched, so its
//! schema and version stay as they are.
use crate::conversation::application::{ConversationPlacements, EnvironmentFuture, PlacementError};
use nessa_local_storage::{
    create_directory, is_unsafe_file, open, sync_directory, OpenMode, PrivateTempFile,
};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::domain::agent_execution::leases::SshDestination;
use serde::{Deserialize, Serialize};
use std::{
    io::{self, Read, Write},
    path::PathBuf,
};

/// The only shape this build writes or reads.
const SCHEMA_VERSION: u32 = 1;
/// Most bytes a placement file may have.
const MAX_BYTES: u64 = 1024;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Placement {
    schema_version: u32,
    host: String,
}

/// The placement files, in one private directory.
pub(crate) struct FilePlacements {
    directory: PathBuf,
}

impl FilePlacements {
    /// The placements kept in `directory`, created if absent.
    ///
    /// # Errors
    /// The directory could not be created privately.
    pub(crate) fn new(directory: PathBuf) -> io::Result<Self> {
        create_directory(&directory)?;
        Ok(Self { directory })
    }

    fn path(&self, conversation: &ConversationId) -> PathBuf {
        self.directory.join(format!("{conversation}.json"))
    }
}

fn remove(directory: PathBuf, path: PathBuf) -> Result<(), PlacementError> {
    match std::fs::remove_file(&path) {
        Ok(()) => sync_directory(&directory).map_err(|_| PlacementError::Unavailable),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(PlacementError::Unavailable),
    }
}

fn write(directory: PathBuf, path: PathBuf, host: String) -> Result<(), PlacementError> {
    let unavailable = |_| PlacementError::Unavailable;
    let mut file = PrivateTempFile::new_in(&directory).map_err(unavailable)?;
    serde_json::to_writer(
        file.as_file_mut(),
        &Placement {
            schema_version: SCHEMA_VERSION,
            host,
        },
    )
    .map_err(|_| PlacementError::Unavailable)?;
    file.as_file_mut().write_all(b"\n").map_err(unavailable)?;
    file.as_file().sync_all().map_err(unavailable)?;
    file.persist(&path).map_err(unavailable)?;
    sync_directory(&directory).map_err(unavailable)
}

fn read(path: PathBuf) -> Result<Option<String>, PlacementError> {
    let file = match open(&path, OpenMode::Read) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) if is_unsafe_file(&error) => return Err(PlacementError::Unreadable),
        Err(_) => return Err(PlacementError::Unavailable),
    };
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| PlacementError::Unavailable)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(PlacementError::Unreadable);
    }
    let placement: Placement =
        serde_json::from_slice(&bytes).map_err(|_| PlacementError::Unreadable)?;
    if placement.schema_version != SCHEMA_VERSION || SshDestination::new(&*placement.host).is_err()
    {
        return Err(PlacementError::Unreadable);
    }
    Ok(Some(placement.host))
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, PlacementError> + Send + 'static,
) -> Result<T, PlacementError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| PlacementError::Unavailable)?
}

impl ConversationPlacements for FilePlacements {
    fn place<'a>(
        &'a self,
        conversation: &'a ConversationId,
        host: Option<&'a str>,
    ) -> EnvironmentFuture<'a, Result<(), PlacementError>> {
        let directory = self.directory.clone();
        let path = self.path(conversation);
        let host = host.map(str::to_owned);
        Box::pin(blocking(move || match host {
            Some(host) => write(directory, path, host),
            None => remove(directory, path),
        }))
    }

    fn placement<'a>(
        &'a self,
        conversation: &'a ConversationId,
    ) -> EnvironmentFuture<'a, Result<Option<String>, PlacementError>> {
        let path = self.path(conversation);
        Box::pin(blocking(move || read(path)))
    }

    fn erase<'a>(
        &'a self,
        conversation: &'a ConversationId,
    ) -> EnvironmentFuture<'a, Result<(), PlacementError>> {
        let directory = self.directory.clone();
        let path = self.path(conversation);
        Box::pin(blocking(move || remove(directory, path)))
    }
}

#[cfg(test)]
#[path = "../../../tests/conversation/placements.rs"]
mod tests;
