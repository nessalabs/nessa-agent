//! Where each conversation runs: one small file per conversation,
//! `places/<conversation id>.json` under the conversations directory,
//! holding `{"schemaVersion":1,"placed":"…"}`, and for a `peer:any`
//! conversation the worker that holds it now and the provider context it
//! opened there: `"worker":"peer:…","context":"…"`, `context` absent until a
//! session opened one there. Here is written as `"here"`.
//!
//! Written whole and synced: before the conversation's record exists, and
//! again each time a pool conversation's worker or context changes; removed
//! with its history.
//!
//! Earlier builds kept the same facts in two files, read here as they were
//! written until the conversation's record is first written anew:
//! `placements/<id>.json` (`{"schemaVersion":1,"host":"…"}`, absent for a
//! conversation that runs here) and, for a `peer:any` conversation,
//! `bindings/<id>.json` (`{"schemaVersion":1,"worker":"peer:…","context":"…"}`).
//! For one release this build still writes them too, before the record: a
//! build put back then finds every conversation where it runs, never here
//! in place of its host. Once the record exists only its binding is still
//! read, and wins: this build writes it first, so it is never older than
//! the record, and a build that was put back moves a pool conversation
//! there alone. Coming back to this build then follows that move. The
//! next release stops writing them, and the one after stops reading them
//! once every conversation has a record. Erasing a conversation removes all
//! three, the earlier files first, so a failed erase never leaves them as
//! the only answer.
//!
//! A record-scope dataset (ADR 202): a file this build cannot read refuses
//! that conversation's opening alone. The metadata database is not touched,
//! so its schema and version stay as they are.
use crate::application::{
    ConversationPlaces, EnvironmentFuture, PlaceRecord, PlacementError, WorkerBinding,
};
use crate::domain::Place;
use nessa_local_storage::{
    create_directory, is_replaced_file, is_unsafe_file, open, sync_directory, OpenMode,
    PrivateTempFile,
};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::domain::agent_execution::leases::PeerEnvironment;
use nessa_sdk::domain::agent_execution::sessions::ExecutionSessionId;
use serde::{Deserialize, Serialize};
use std::{
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

/// The only shape this build writes or reads.
const SCHEMA_VERSION: u32 = 1;
/// Most bytes a placement file may have.
const MAX_BYTES: u64 = 1024;

/// One conversation's record, as this build writes it.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Record {
    schema_version: u32,
    placed: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    worker: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    context: Option<String>,
}

/// An earlier build's placement file.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Placement {
    schema_version: u32,
    host: String,
}

/// An earlier build's binding file.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Binding {
    schema_version: u32,
    worker: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    context: Option<String>,
}

/// The place records, in one private directory, and the two directories
/// earlier builds kept the same facts in.
pub struct FilePlaces {
    places: PathBuf,
    placements: PathBuf,
    bindings: PathBuf,
}

impl FilePlaces {
    /// The place records kept under `conversations`, the conversations
    /// directory: `places/`, read before the earlier builds' `placements/`
    /// and `bindings/`, each created if absent.
    ///
    /// # Errors
    /// A directory could not be created privately.
    pub fn new(conversations: &Path) -> io::Result<Self> {
        let places = Self {
            places: conversations.join("places"),
            placements: conversations.join("placements"),
            bindings: conversations.join("bindings"),
        };
        for directory in [&places.places, &places.placements, &places.bindings] {
            create_directory(directory)?;
        }
        Ok(places)
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

/// An earlier build's file: `value` written whole, or the file removed.
fn earlier_file(
    (directory, path): (PathBuf, PathBuf),
    value: Option<&impl Serialize>,
) -> Result<(), PlacementError> {
    match value {
        Some(value) => write(directory, path, value),
        None => remove(directory, path),
    }
}

fn write(directory: PathBuf, path: PathBuf, value: &impl Serialize) -> Result<(), PlacementError> {
    let unavailable = |_| PlacementError::Unavailable;
    let mut file = PrivateTempFile::new_in(&directory).map_err(unavailable)?;
    serde_json::to_writer(file.as_file_mut(), value).map_err(|_| PlacementError::Unavailable)?;
    file.as_file_mut().write_all(b"\n").map_err(unavailable)?;
    file.as_file().sync_all().map_err(unavailable)?;
    file.persist(&path).map_err(unavailable)?;
    sync_directory(&directory).map_err(unavailable)
}

/// The bytes of the file at `path`, at most [`MAX_BYTES`]; `None` when there
/// is none.
///
/// A binding is replaced whole while others read it. A read that opens the
/// file a replacement is moving over holds a file with no name left, which
/// `open` refuses as replaced; opened again, the name is the replacement's.
/// Each retry follows a replacement that finished, so it is not counted.
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

fn worker_binding(worker: &str, context: Option<String>) -> Result<WorkerBinding, PlacementError> {
    let worker = PeerEnvironment::parse(worker).map_err(|_| PlacementError::Unreadable)?;
    let context = context
        .map(ExecutionSessionId::new)
        .transpose()
        .map_err(|_| PlacementError::Unreadable)?;
    Ok(WorkerBinding { worker, context })
}

fn record(bytes: &[u8]) -> Result<PlaceRecord, PlacementError> {
    let record: Record = serde_json::from_slice(bytes).map_err(|_| PlacementError::Unreadable)?;
    if record.schema_version != SCHEMA_VERSION {
        return Err(PlacementError::Unreadable);
    }
    let placed = Place::named(&record.placed).map_err(|_| PlacementError::Unreadable)?;
    let worker = match (record.worker, record.context) {
        (Some(worker), context) => Some(worker_binding(&worker, context)?),
        (None, None) => None,
        // A context is a worker's.
        (None, Some(_)) => return Err(PlacementError::Unreadable),
    };
    PlaceRecord::read(placed, worker)
}

/// Where an earlier build placed the conversation and, for the pool, which
/// worker held it: here when it kept no placement.
fn earlier(placement: PathBuf, binding: PathBuf) -> Result<PlaceRecord, PlacementError> {
    let Some(bytes) = read_bytes(placement)? else {
        return Ok(PlaceRecord::placed(Place::Here));
    };
    let placement: Placement =
        serde_json::from_slice(&bytes).map_err(|_| PlacementError::Unreadable)?;
    if placement.schema_version != SCHEMA_VERSION {
        return Err(PlacementError::Unreadable);
    }
    // Anywhere but here, which had no placement.
    let placed = match Place::named(&placement.host) {
        Ok(Place::Here) | Err(_) => return Err(PlacementError::Unreadable),
        Ok(place) => place,
    };
    // Only a pool conversation's binding was ever read.
    if placed != Place::Pool {
        return Ok(PlaceRecord::placed(placed));
    }
    Ok(earlier_binding(binding)?.map_or(PlaceRecord::placed(Place::Pool), PlaceRecord::in_pool))
}

/// The worker an earlier build's binding file names, if there is one.
fn earlier_binding(binding: PathBuf) -> Result<Option<WorkerBinding>, PlacementError> {
    let Some(bytes) = read_bytes(binding)? else {
        return Ok(None);
    };
    let binding: Binding =
        serde_json::from_slice(&bytes).map_err(|_| PlacementError::Unreadable)?;
    if binding.schema_version != SCHEMA_VERSION {
        return Err(PlacementError::Unreadable);
    }
    worker_binding(&binding.worker, binding.context).map(Some)
}

/// The record at `places/`, with the earlier binding winning for a pool
/// conversation: never older than the record, and newer once a build put
/// back moved the conversation.
fn current(bytes: &[u8], binding: PathBuf) -> Result<PlaceRecord, PlacementError> {
    let record = record(bytes)?;
    if *record.place() != Place::Pool {
        return Ok(record);
    }
    Ok(match earlier_binding(binding)? {
        Some(binding) => PlaceRecord::in_pool(binding),
        None => record,
    })
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, PlacementError> + Send + 'static,
) -> Result<T, PlacementError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| PlacementError::Unavailable)?
}

impl ConversationPlaces for FilePlaces {
    fn write<'a>(
        &'a self,
        conversation: &'a ConversationId,
        record: &'a PlaceRecord,
    ) -> EnvironmentFuture<'a, Result<(), PlacementError>> {
        let placed = record.place().name();
        let worker = record.worker().map(|binding| binding.worker.name());
        let context = record
            .worker()
            .and_then(|binding| binding.context.as_ref())
            .map(|context| context.as_str().to_owned());
        // What an earlier build reads, each file written when it has
        // something to say and removed when not: no placement for here, a
        // binding only once a worker holds the conversation.
        let placement = (*record.place() != Place::Here).then(|| Placement {
            schema_version: SCHEMA_VERSION,
            host: placed.clone(),
        });
        let binding = worker.clone().map(|worker| Binding {
            schema_version: SCHEMA_VERSION,
            worker,
            context: context.clone(),
        });
        let placements = (
            self.placements.clone(),
            Self::file(&self.placements, conversation),
        );
        let bindings = (
            self.bindings.clone(),
            Self::file(&self.bindings, conversation),
        );
        let directory = self.places.clone();
        let path = Self::file(&self.places, conversation);
        let written = Record {
            schema_version: SCHEMA_VERSION,
            placed,
            worker,
            context,
        };
        Box::pin(blocking(move || {
            earlier_file(placements, placement.as_ref())?;
            earlier_file(bindings, binding.as_ref())?;
            write(directory, path, &written)
        }))
    }

    fn read<'a>(
        &'a self,
        conversation: &'a ConversationId,
    ) -> EnvironmentFuture<'a, Result<PlaceRecord, PlacementError>> {
        let path = Self::file(&self.places, conversation);
        let placement = Self::file(&self.placements, conversation);
        let binding = Self::file(&self.bindings, conversation);
        Box::pin(blocking(move || match read_bytes(path)? {
            Some(bytes) => current(&bytes, binding),
            None => earlier(placement, binding),
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
