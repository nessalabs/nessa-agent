//! Where earlier builds placed each conversation, read so that one placed
//! away from this machine is refused rather than run here (ADR 1417).
//!
//! Earlier builds kept `places/<conversation id>.json` under the
//! conversations directory (`{"schemaVersion":1,"placed":"…"}`, `"here"` for
//! this machine), and before that `placements/<id>.json`
//! (`{"schemaVersion":1,"host":"…"}`, absent for here) and
//! `bindings/<id>.json` for a pool conversation's worker. This build writes
//! none of them; it reads them, and removes them with the conversation's
//! history.
//!
//! A record-scope dataset (ADR 202): a file this build cannot read refuses
//! that conversation's opening alone.
use crate::application::{ConversationPlaces, EnvironmentFuture, PlacementError};
use nessa_local_storage::{is_replaced_file, is_unsafe_file, open, sync_directory, OpenMode};
use nessa_protocol::conversation::domain::ConversationId;
use serde::Deserialize;
use std::{
    io::{self, Read},
    path::{Path, PathBuf},
};

/// The only shape earlier builds wrote.
const SCHEMA_VERSION: u32 = 1;
/// Most bytes a placement file may have.
const MAX_BYTES: u64 = 1024;
/// How a record names this machine.
const HERE: &str = "here";

/// A `places/` record. Its pool fields, when present, are read and ignored:
/// any record they appear in places the conversation elsewhere.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Record {
    schema_version: u32,
    placed: String,
    #[serde(default)]
    #[allow(dead_code, reason = "read so the record parses; never used")]
    worker: Option<String>,
    #[serde(default)]
    #[allow(dead_code, reason = "read so the record parses; never used")]
    context: Option<String>,
}

/// A `placements/` file.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Placement {
    schema_version: u32,
    host: String,
}

/// Earlier builds' place records.
pub struct FilePlaces {
    places: PathBuf,
    placements: PathBuf,
    bindings: PathBuf,
}

impl FilePlaces {
    /// The records kept under `conversations`, the conversations directory.
    /// Nothing is created: a directory that is not there holds no record.
    pub fn new(conversations: &Path) -> Self {
        Self {
            places: conversations.join("places"),
            placements: conversations.join("placements"),
            bindings: conversations.join("bindings"),
        }
    }

    fn file(directory: &Path, conversation: &ConversationId) -> PathBuf {
        directory.join(format!("{conversation}.json"))
    }
}

fn remove(directory: PathBuf, path: PathBuf) -> Result<(), PlacementError> {
    match std::fs::remove_file(&path) {
        Ok(()) => sync_directory(&directory).map_err(|_| PlacementError::Unavailable),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(PlacementError::Unavailable),
    }
}

/// The bytes of the file at `path`, at most [`MAX_BYTES`]; `None` when there
/// is none.
fn read_bytes(path: PathBuf) -> Result<Option<Vec<u8>>, PlacementError> {
    let file = loop {
        match open(&path, OpenMode::Read) {
            Ok(file) => break file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) if is_replaced_file(&error) => {}
            Err(error) if is_unsafe_file(&error) => return Err(PlacementError::Unreadable),
            Err(_) => return Err(PlacementError::Unavailable),
        }
    };
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| PlacementError::Unavailable)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(PlacementError::Unreadable);
    }
    Ok(Some(bytes))
}

/// `Ok` for a record that says here, [`PlacementError::Elsewhere`] for one
/// that names anywhere else.
fn record(bytes: &[u8]) -> Result<(), PlacementError> {
    let record: Record = serde_json::from_slice(bytes).map_err(|_| PlacementError::Unreadable)?;
    if record.schema_version != SCHEMA_VERSION || record.placed.is_empty() {
        return Err(PlacementError::Unreadable);
    }
    if record.placed == HERE {
        Ok(())
    } else {
        Err(PlacementError::Elsewhere)
    }
}

/// An earlier build's placement file: it was written only for a
/// conversation placed away from here.
fn earlier(placement: PathBuf) -> Result<(), PlacementError> {
    let Some(bytes) = read_bytes(placement)? else {
        return Ok(());
    };
    let placement: Placement =
        serde_json::from_slice(&bytes).map_err(|_| PlacementError::Unreadable)?;
    if placement.schema_version != SCHEMA_VERSION || placement.host.is_empty() {
        return Err(PlacementError::Unreadable);
    }
    Err(PlacementError::Elsewhere)
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, PlacementError> + Send + 'static,
) -> Result<T, PlacementError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| PlacementError::Unavailable)?
}

impl ConversationPlaces for FilePlaces {
    fn read<'a>(
        &'a self,
        conversation: &'a ConversationId,
    ) -> EnvironmentFuture<'a, Result<(), PlacementError>> {
        let path = Self::file(&self.places, conversation);
        let placement = Self::file(&self.placements, conversation);
        Box::pin(blocking(move || match read_bytes(path)? {
            Some(bytes) => record(&bytes),
            None => earlier(placement),
        }))
    }

    fn erase<'a>(
        &'a self,
        conversation: &'a ConversationId,
    ) -> EnvironmentFuture<'a, Result<(), PlacementError>> {
        let files = [&self.placements, &self.bindings, &self.places]
            .map(|directory| (directory.clone(), Self::file(directory, conversation)));
        Box::pin(blocking(move || {
            for (directory, path) in files {
                remove(directory, path)?;
            }
            Ok(())
        }))
    }
}

#[cfg(test)]
#[path = "../../tests/conversation/placements.rs"]
mod tests;
