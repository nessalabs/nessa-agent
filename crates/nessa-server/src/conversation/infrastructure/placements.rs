//! Where each conversation placed on an SSH host, a paired environment or
//! the pool (`peer:any`) runs: one small file per
//! such conversation, `placements/<conversation id>.json` under the
//! conversations directory, holding `{"schemaVersion":1,"host":"…"}`. A
//! conversation that runs here has none.
//!
//! Which worker holds a `peer:any` conversation now is its binding
//! ([`FileBindings`]): `bindings/<conversation id>.json`, holding
//! `{"schemaVersion":1,"worker":"peer:…","context":"…"}`, `context` absent
//! until a session opened one there. Written whole and synced, replaced
//! when the conversation moves or opens new context, removed with its
//! history. A binding this build cannot read refuses that conversation's
//! opening alone, as a placement does.
//!
//! A record-scope dataset (ADR 202): a file this build cannot read refuses
//! that conversation's opening alone. Written whole and synced before the
//! conversation's record exists; never changed after; removed with the
//! conversation's history. The metadata database is not touched, so its
//! schema and version stay as they are.
use crate::conversation::application::{
    ConversationBindings, ConversationPlacements, EnvironmentFuture, PlacementError, WorkerBinding,
    POOL,
};
use nessa_local_storage::{
    create_directory, is_replaced_file, is_unsafe_file, open, sync_directory, OpenMode,
    PrivateTempFile,
};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::domain::agent_execution::leases::{EnvironmentRef, PeerEnvironment};
use nessa_sdk::domain::agent_execution::sessions::ExecutionSessionId;
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

fn read(path: PathBuf) -> Result<Option<String>, PlacementError> {
    let Some(bytes) = read_bytes(path)? else {
        return Ok(None);
    };
    let placement: Placement =
        serde_json::from_slice(&bytes).map_err(|_| PlacementError::Unreadable)?;
    // An SSH destination, a paired environment's `peer:` name, or the
    // pool's; never here, which has no placement.
    let host = EnvironmentRef::named(&placement.host);
    if placement.schema_version != SCHEMA_VERSION
        || !(placement.host == POOL
            || matches!(
                host,
                Ok(EnvironmentRef::Ssh(_) | EnvironmentRef::Peer(_) | EnvironmentRef::Boat)
            ))
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
            Some(host) => write(
                directory,
                path,
                &Placement {
                    schema_version: SCHEMA_VERSION,
                    host,
                },
            ),
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

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Binding {
    schema_version: u32,
    worker: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    context: Option<String>,
}

fn read_binding(path: PathBuf) -> Result<Option<WorkerBinding>, PlacementError> {
    let Some(bytes) = read_bytes(path)? else {
        return Ok(None);
    };
    let binding: Binding =
        serde_json::from_slice(&bytes).map_err(|_| PlacementError::Unreadable)?;
    if binding.schema_version != SCHEMA_VERSION {
        return Err(PlacementError::Unreadable);
    }
    let worker = PeerEnvironment::parse(&binding.worker).map_err(|_| PlacementError::Unreadable)?;
    let context = binding
        .context
        .map(ExecutionSessionId::new)
        .transpose()
        .map_err(|_| PlacementError::Unreadable)?;
    Ok(Some(WorkerBinding { worker, context }))
}

/// Which worker holds each `peer:any` conversation, in one private
/// directory.
pub(crate) struct FileBindings {
    directory: PathBuf,
}

impl FileBindings {
    /// The bindings kept in `directory`, created if absent.
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

impl ConversationBindings for FileBindings {
    fn bind<'a>(
        &'a self,
        conversation: &'a ConversationId,
        binding: &'a WorkerBinding,
    ) -> EnvironmentFuture<'a, Result<(), PlacementError>> {
        let directory = self.directory.clone();
        let path = self.path(conversation);
        let binding = Binding {
            schema_version: SCHEMA_VERSION,
            worker: binding.worker.name(),
            context: binding
                .context
                .as_ref()
                .map(|context| context.as_str().to_owned()),
        };
        Box::pin(blocking(move || write(directory, path, &binding)))
    }

    fn binding<'a>(
        &'a self,
        conversation: &'a ConversationId,
    ) -> EnvironmentFuture<'a, Result<Option<WorkerBinding>, PlacementError>> {
        let path = self.path(conversation);
        Box::pin(blocking(move || read_binding(path)))
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
