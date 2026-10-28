//! Private per-thread allocation receipts. Intent and receipt publication sync
//! before returning; unsafe files and mismatched identities are refused.
use crate::workspaces::{
    application::Assignments,
    domain::{
        AdmissionRefusal, AdmissionRejection, Assignment, EnvironmentIdentity, Failure,
        RefusalDisposition,
    },
};
use async_trait::async_trait;
use nessa_local_storage::{create_directory, OpenMode, PrivateDirectory};
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsStr,
    io::{Read, Write},
    path::PathBuf,
};

const MAX_HISTORY_BYTES: usize = 8192;

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
        #[serde(with = "identity")]
        admission: EnvironmentIdentity,
    },
    Refused {
        thread: String,
        vm: String,
        image: String,
        #[serde(with = "identity")]
        admission: EnvironmentIdentity,
        #[serde(with = "rejection")]
        rejection: AdmissionRejection,
        #[serde(with = "StoredDisposition")]
        disposition: RefusalDisposition,
    },
    #[serde(skip)]
    Unverified {
        thread: String,
        vm: String,
        image: String,
        environment: String,
    },
}
#[derive(Serialize)]
#[serde(transparent)]
struct PersistedAssignment(#[serde(with = "StoredAssignment")] Assignment);
#[derive(Serialize, Deserialize)]
#[serde(remote = "AdmissionRefusal", rename_all = "snake_case")]
enum StoredRefusal {
    MissingEnvironment,
    WrongEnvironment,
    MissingVersion,
    WrongVersion,
    EnvironmentChanged,
    EnvironmentUnavailable,
    InvalidReceipt,
    ReceiptUnavailable,
    LifetimeNotEnforced,
}
#[derive(Serialize, Deserialize)]
#[serde(remote = "RefusalDisposition", rename_all = "snake_case")]
enum StoredDisposition {
    RollbackNewVm,
    RetainTarget,
}
mod rejection {
    use super::{AdmissionRefusal, AdmissionRejection, StoredRefusal};
    use crate::workspaces::domain::ObservedEnvironment;
    use serde::{Deserialize, Serialize};
    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Wire {
        #[serde(with = "StoredRefusal")]
        cause: AdmissionRefusal,
        observed: Option<Observed>,
    }
    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Observed {
        name: Option<String>,
        version: Option<u32>,
    }
    pub(super) fn serialize<S: serde::Serializer>(
        value: &AdmissionRejection,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        Wire {
            cause: value.cause(),
            observed: value.observed().map(|o| Observed {
                name: o.name.clone(),
                version: o.version,
            }),
        }
        .serialize(serializer)
    }
    pub(super) fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<AdmissionRejection, D::Error> {
        let wire = Wire::deserialize(deserializer)?;
        let observed = wire.observed.map(|o| ObservedEnvironment {
            name: o.name,
            version: o.version,
        });
        let rejection = AdmissionRejection::new(wire.cause, observed.clone());
        if rejection.cause() != wire.cause || rejection.observed() != observed.as_ref() {
            return Err(serde::de::Error::custom("invalid refusal evidence"));
        }
        Ok(rejection)
    }
}
mod identity {
    use super::EnvironmentIdentity;
    use serde::{Deserialize, Serialize};
    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Wire {
        id: String,
        name: String,
        version_id: String,
        version: u32,
    }
    pub(super) fn serialize<S: serde::Serializer>(
        value: &EnvironmentIdentity,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        Wire {
            id: value.id().into(),
            name: value.name().into(),
            version_id: value.version_id().into(),
            version: value.version(),
        }
        .serialize(serializer)
    }
    pub(super) fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<EnvironmentIdentity, D::Error> {
        let wire = Wire::deserialize(deserializer)?;
        EnvironmentIdentity::new(wire.id, wire.name, wire.version_id, wire.version)
            .map_err(|_| serde::de::Error::custom("invalid environment identity"))
    }
}
impl<'de> Deserialize<'de> for PersistedAssignment {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Current(#[serde(with = "StoredAssignment")] Assignment);
        #[derive(Deserialize)]
        #[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
        enum Legacy {
            Assigned {
                thread: String,
                vm: String,
                image: String,
                environment: String,
            },
        }
        let value = serde_json::Value::deserialize(deserializer)?;
        if let Ok(Current(current)) = serde_json::from_value(value.clone()) {
            return Ok(Self(current));
        }
        let Legacy::Assigned {
            thread,
            vm,
            image,
            environment,
        } = serde_json::from_value(value).map_err(serde::de::Error::custom)?;
        Ok(Self(Assignment::Unverified {
            thread,
            vm,
            image,
            environment,
        }))
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct History {
    cause: Cause,
    transitions: Vec<PersistedAssignment>,
}
impl History {
    fn validate(&self, thread: &str) -> Result<(), Failure> {
        let transitions = self.transitions.iter().map(|a| &a.0).collect::<Vec<_>>();
        Assignment::validate_history(thread, &transitions)
    }
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Cause {
    ThreadLeaseRequested,
}
fn decode(bytes: &[u8], thread: &str) -> Result<History, Failure> {
    let history: History = serde_json::from_slice(bytes).map_err(|_| Failure::Storage)?;
    history.validate(thread)?;
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
            file.take((MAX_HISTORY_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
                .map_err(|_| Failure::Storage)?;
            if bytes.len() > MAX_HISTORY_BYTES {
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
        if !valid(assignment.thread()) || matches!(assignment, Assignment::Unverified { .. }) {
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
                        file.take((MAX_HISTORY_BYTES + 1) as u64)
                            .read_to_end(&mut bytes)
                            .map_err(|_| Failure::Storage)?;
                        if bytes.len() > MAX_HISTORY_BYTES {
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
            history
                .transitions
                .push(PersistedAssignment(assignment.clone()));
            history.validate(assignment.thread())?;
            let bytes = serde_json::to_vec(&history).map_err(|_| Failure::Storage)?;
            if bytes.len() > MAX_HISTORY_BYTES {
                return Err(Failure::Storage);
            }
            let temporary = directory.reserve_temp().map_err(|_| Failure::Storage)?;
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
