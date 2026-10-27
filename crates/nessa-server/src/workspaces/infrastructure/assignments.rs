//! Private per-thread allocation receipts. Intent and receipt publication sync
//! before returning; unsafe files and mismatched identities are refused.
use crate::workspaces::{
    application::Assignments,
    domain::{Assignment, Failure},
};
use async_trait::async_trait;
use nessa_local_storage::{create_directory, OpenMode, PrivateDirectory};
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsStr,
    io::{Read, Write},
    path::PathBuf,
};

pub(crate) struct FileAssignments(pub(crate) PathBuf);
#[derive(Serialize, Deserialize)]
#[serde(
    remote = "Assignment",
    tag = "state",
    rename_all = "snake_case",
    deny_unknown_fields
)]
enum StoredAssignment {
    Provisioning {
        thread: String,
    },
    Assigned {
        thread: String,
        vm: String,
        image: String,
        environment: String,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(transparent)]
struct PersistedAssignment(#[serde(with = "StoredAssignment")] Assignment);
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct History {
    cause: Cause,
    transitions: Vec<PersistedAssignment>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Cause {
    ThreadLeaseRequested,
}
fn decode(bytes: &[u8], thread: &str) -> Result<History, Failure> {
    let history: History = serde_json::from_slice(bytes).map_err(|_| Failure::Storage)?;
    if history.transitions.is_empty()
        || history.transitions.len() > 2
        || history.transitions.iter().any(|a| a.0.thread() != thread)
        || !matches!(history.transitions[0].0, Assignment::Provisioning { .. })
        || (history.transitions.len() == 2
            && !matches!(history.transitions[1].0, Assignment::Assigned { .. }))
    {
        return Err(Failure::Storage);
    }
    Ok(history)
}
fn valid(thread: &str) -> bool {
    uuid::Uuid::parse_str(thread).is_ok_and(|id| id.to_string() == thread)
}
#[async_trait]
impl Assignments for FileAssignments {
    async fn read(&self, thread: &str) -> Result<Option<Assignment>, Failure> {
        if !valid(thread) {
            return Err(Failure::InvalidInput);
        }
        let root = self.0.clone();
        let thread = thread.to_owned();
        tokio::task::spawn_blocking(move || {
            create_directory(&root).map_err(|_| Failure::Storage)?;
            let directory = PrivateDirectory::open_root(&root).map_err(|_| Failure::Storage)?;
            let file = match directory.open_file(
                OsStr::new(&format!("{thread}.json")),
                OpenMode::ReadNonblocking,
            ) {
                Ok(file) => file,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(_) => return Err(Failure::Storage),
            };
            let mut bytes = Vec::new();
            file.take(8193)
                .read_to_end(&mut bytes)
                .map_err(|_| Failure::Storage)?;
            if bytes.len() > 8192 {
                return Err(Failure::Storage);
            }
            Ok(decode(&bytes, &thread)?
                .transitions
                .pop()
                .map(|value| value.0))
        })
        .await
        .map_err(|_| Failure::Storage)?
    }
    async fn save(&self, assignment: Assignment) -> Result<(), Failure> {
        if !valid(assignment.thread()) {
            return Err(Failure::InvalidInput);
        }
        let root = self.0.clone();
        tokio::task::spawn_blocking(move || {
            create_directory(&root).map_err(|_| Failure::Storage)?;
            let directory = PrivateDirectory::open_root(&root).map_err(|_| Failure::Storage)?;
            let name = format!("{}.json", assignment.thread());
            let mut history =
                match directory.open_file(OsStr::new(&name), OpenMode::ReadNonblocking) {
                    Ok(file) => {
                        let mut bytes = Vec::new();
                        file.take(8193)
                            .read_to_end(&mut bytes)
                            .map_err(|_| Failure::Storage)?;
                        if bytes.len() > 8192 {
                            return Err(Failure::Storage);
                        }
                        decode(&bytes, assignment.thread())?
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => History {
                        cause: Cause::ThreadLeaseRequested,
                        transitions: Vec::new(),
                    },
                    Err(_) => return Err(Failure::Storage),
                };
            if history.transitions.last().map(|value| &value.0) == Some(&assignment) {
                return Ok(());
            }
            match (history.transitions.as_slice(), &assignment) {
                ([], Assignment::Provisioning { .. })
                | (
                    [PersistedAssignment(Assignment::Provisioning { .. })],
                    Assignment::Assigned { .. },
                ) => {}
                _ => return Err(Failure::Storage),
            }
            history
                .transitions
                .push(PersistedAssignment(assignment.clone()));
            let temporary = directory.reserve_temp().map_err(|_| Failure::Storage)?;
            let bytes = serde_json::to_vec(&history).map_err(|_| Failure::Storage)?;
            temporary
                .as_file()
                .write_all(&bytes)
                .map_err(|_| Failure::Storage)?;
            temporary
                .as_file()
                .sync_all()
                .map_err(|_| Failure::Storage)?;
            temporary
                .replace(OsStr::new(&format!("{}.json", assignment.thread())))
                .map_err(|_| Failure::Storage)?;
            Ok(())
        })
        .await
        .map_err(|_| Failure::Storage)?
    }
}
