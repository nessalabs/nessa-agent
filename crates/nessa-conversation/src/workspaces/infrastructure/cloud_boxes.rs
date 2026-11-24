//! Durable file-backed receipt store, durable audit log, and provider adapters
//! for persistent cloud gateway VMs (`<namespace>/cloud-boxes/<uuid>.json`),
//! using atomic temp-file replacement inside a private directory
//! (`nessa_local_storage::PrivateDirectory`).

use crate::domain::VmLifetime;
use crate::timer::clock;
use crate::workspaces::{
    application::{
        ActiveWorkChecker, BoxFuture, CloudBoxAudit, CloudBoxAuditEvent, CloudBoxProvider,
        CloudBoxStore, Provider, ProvisionedCloudBox, RestCause,
    },
    domain::{
        Admitted, Cloud, CloudBoxEndpoint, CloudBoxError, CloudBoxId, CloudBoxName, CloudBoxRecord,
        CloudBoxState, CloudBoxUrl, CloudConnection, EnvironmentIdentity, Failure,
        RefusalDisposition, TemplateRelease,
    },
};
use nessa_local_storage::{create_directory, OpenMode, PrivateDirectory};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    ffi::OsStr,
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::UNIX_EPOCH,
};
use zeroize::Zeroizing;

/// Maximum size of one cloud box receipt history file (64 KiB).
const MAX_HISTORY_BYTES: u64 = 64 * 1024;

/// Bootstrap command installed in gateway cloud images that starts headless
/// `nessa-server` and prints JSON enrollment metadata on stdout.
const BOOTSTRAP_CREATE_CMD: &str = "/opt/nessa/bin/nessa-gateway-bootstrap create";
/// Status command installed in gateway cloud images that ensures headless
/// `nessa-server` is running after wake and prints its current JSON endpoint.
const BOOTSTRAP_STATUS_CMD: &str = "/opt/nessa/bin/nessa-gateway-bootstrap status";
/// Active-work probe installed in gateway cloud images that reports whether
/// the running gateway currently has active turns or open terminals (G5).
const BOOTSTRAP_BUSY_CMD: &str = "/opt/nessa/bin/nessa-gateway-bootstrap busy";

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StoredState {
    Provisioning,
    Running,
    Sleeping,
    Refused,
    Deleted,
}

impl From<CloudBoxState> for StoredState {
    fn from(value: CloudBoxState) -> Self {
        match value {
            CloudBoxState::Provisioning => Self::Provisioning,
            CloudBoxState::Running => Self::Running,
            CloudBoxState::Sleeping => Self::Sleeping,
            CloudBoxState::Refused => Self::Refused,
            CloudBoxState::Deleted => Self::Deleted,
        }
    }
}

impl From<StoredState> for CloudBoxState {
    fn from(value: StoredState) -> Self {
        match value {
            StoredState::Provisioning => Self::Provisioning,
            StoredState::Running => Self::Running,
            StoredState::Sleeping => Self::Sleeping,
            StoredState::Refused => Self::Refused,
            StoredState::Deleted => Self::Deleted,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredBoatAdmitted {
    id: String,
    name: String,
    version_id: String,
    version: u32,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredAgent37Admitted {
    template: String,
    revision: Option<u64>,
    digest: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum StoredAdmitted {
    Boat(StoredBoatAdmitted),
    Agent37(StoredAgent37Admitted),
}

impl From<&Admitted> for StoredAdmitted {
    fn from(value: &Admitted) -> Self {
        match value {
            Admitted::Boat(v) => Self::Boat(StoredBoatAdmitted {
                id: v.id().into(),
                name: v.name().into(),
                version_id: v.version_id().into(),
                version: v.version(),
            }),
            Admitted::Agent37(v) => Self::Agent37(StoredAgent37Admitted {
                template: v.template().into(),
                revision: v.revision(),
                digest: v.digest().map(str::to_owned),
            }),
        }
    }
}

impl TryFrom<StoredAdmitted> for Admitted {
    type Error = CloudBoxError;
    fn try_from(value: StoredAdmitted) -> Result<Self, Self::Error> {
        match value {
            StoredAdmitted::Boat(b) => {
                EnvironmentIdentity::new(b.id, b.name, b.version_id, b.version).map(Admitted::Boat)
            }
            StoredAdmitted::Agent37(a) => {
                TemplateRelease::new(a.template, a.revision, a.digest).map(Admitted::Agent37)
            }
        }
        .map_err(|_| CloudBoxError::ReceiptUnavailable("invalid admission identity".into()))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum StoredCloudBoxRecord {
    Provisioning {
        box_id: String,
        name: String,
        cloud: String,
        created_at_ms: u64,
    },
    Active {
        box_id: String,
        name: String,
        cloud: String,
        vm: String,
        image: String,
        admission: StoredAdmitted,
        gateway_id: String,
        url: String,
        state: StoredState,
        created_at_ms: u64,
        updated_at_ms: u64,
    },
    Refused {
        box_id: String,
        name: String,
        cloud: String,
        reason: String,
        updated_at_ms: u64,
    },
    Deleted {
        box_id: String,
        vm: Option<String>,
        updated_at_ms: u64,
    },
}

impl From<&CloudBoxRecord> for StoredCloudBoxRecord {
    fn from(record: &CloudBoxRecord) -> Self {
        match record {
            CloudBoxRecord::Provisioning {
                box_id,
                name,
                cloud,
                created_at_ms,
            } => Self::Provisioning {
                box_id: box_id.as_str().to_owned(),
                name: name.as_str().to_owned(),
                cloud: cloud.as_str().to_owned(),
                created_at_ms: *created_at_ms,
            },
            CloudBoxRecord::Active {
                box_id,
                name,
                cloud,
                vm,
                image,
                admission,
                gateway_id,
                url,
                state,
                created_at_ms,
                updated_at_ms,
            } => Self::Active {
                box_id: box_id.as_str().to_owned(),
                name: name.as_str().to_owned(),
                cloud: cloud.as_str().to_owned(),
                vm: vm.clone(),
                image: image.clone(),
                admission: StoredAdmitted::from(admission),
                gateway_id: gateway_id.clone(),
                url: url.as_str().to_owned(),
                state: StoredState::from(*state),
                created_at_ms: *created_at_ms,
                updated_at_ms: *updated_at_ms,
            },
            CloudBoxRecord::Refused {
                box_id,
                name,
                cloud,
                reason,
                updated_at_ms,
            } => Self::Refused {
                box_id: box_id.as_str().to_owned(),
                name: name.as_str().to_owned(),
                cloud: cloud.as_str().to_owned(),
                reason: reason.clone(),
                updated_at_ms: *updated_at_ms,
            },
            CloudBoxRecord::Deleted {
                box_id,
                vm,
                updated_at_ms,
            } => Self::Deleted {
                box_id: box_id.as_str().to_owned(),
                vm: vm.clone(),
                updated_at_ms: *updated_at_ms,
            },
        }
    }
}

impl TryFrom<StoredCloudBoxRecord> for CloudBoxRecord {
    type Error = CloudBoxError;
    fn try_from(stored: StoredCloudBoxRecord) -> Result<Self, Self::Error> {
        let parse_cloud = |word: &str| {
            Cloud::parse(word).ok_or_else(|| {
                CloudBoxError::ReceiptUnavailable(format!("unknown cloud provider `{word}`"))
            })
        };
        match stored {
            StoredCloudBoxRecord::Provisioning {
                box_id,
                name,
                cloud,
                created_at_ms,
            } => Ok(Self::Provisioning {
                box_id: CloudBoxId::parse(box_id)?,
                name: CloudBoxName::new(name)?,
                cloud: parse_cloud(&cloud)?,
                created_at_ms,
            }),
            StoredCloudBoxRecord::Active {
                box_id,
                name,
                cloud,
                vm,
                image,
                admission,
                gateway_id,
                url,
                state,
                created_at_ms,
                updated_at_ms,
            } => Ok(Self::Active {
                box_id: CloudBoxId::parse(box_id)?,
                name: CloudBoxName::new(name)?,
                cloud: parse_cloud(&cloud)?,
                vm,
                image,
                admission: Admitted::try_from(admission)?,
                gateway_id,
                url: CloudBoxUrl::new(url)?,
                state: CloudBoxState::from(state),
                created_at_ms,
                updated_at_ms,
            }),
            StoredCloudBoxRecord::Refused {
                box_id,
                name,
                cloud,
                reason,
                updated_at_ms,
            } => Ok(Self::Refused {
                box_id: CloudBoxId::parse(box_id)?,
                name: CloudBoxName::new(name)?,
                cloud: parse_cloud(&cloud)?,
                reason,
                updated_at_ms,
            }),
            StoredCloudBoxRecord::Deleted {
                box_id,
                vm,
                updated_at_ms,
            } => Ok(Self::Deleted {
                box_id: CloudBoxId::parse(box_id)?,
                vm,
                updated_at_ms,
            }),
        }
    }
}

fn decode_history(
    bytes: &[u8],
    expected_id: &CloudBoxId,
) -> Result<Vec<CloudBoxRecord>, CloudBoxError> {
    let stored: Vec<StoredCloudBoxRecord> = serde_json::from_slice(bytes)
        .map_err(|error| CloudBoxError::ReceiptUnavailable(error.to_string()))?;
    let history: Vec<CloudBoxRecord> = stored
        .into_iter()
        .map(CloudBoxRecord::try_from)
        .collect::<Result<_, _>>()?;
    CloudBoxRecord::validate_history(&history)?;
    if history.first().map(CloudBoxRecord::box_id) != Some(expected_id) {
        return Err(CloudBoxError::ReceiptUnavailable(
            "receipt file box_id does not match filename".into(),
        ));
    }
    Ok(history)
}

fn encode_history(history: &[CloudBoxRecord]) -> Result<Vec<u8>, CloudBoxError> {
    let stored: Vec<StoredCloudBoxRecord> =
        history.iter().map(StoredCloudBoxRecord::from).collect();
    serde_json::to_vec_pretty(&stored)
        .map_err(|error| CloudBoxError::ReceiptUnavailable(error.to_string()))
}

/// File-backed [`CloudBoxStore`] storing one validated JSON history per box.
pub struct FileCloudBoxStore {
    root: PathBuf,
}

impl FileCloudBoxStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn file_name(box_id: &CloudBoxId) -> String {
        format!("{}.json", box_id.as_str())
    }
}

impl CloudBoxStore for FileCloudBoxStore {
    fn list(&self) -> Result<Vec<Vec<CloudBoxRecord>>, CloudBoxError> {
        if !self.root.exists() {
            return Ok(Vec::new());
        }
        let dir = PrivateDirectory::open_root(&self.root)
            .map_err(|error| CloudBoxError::ReceiptUnavailable(error.to_string()))?;
        let entries = fs::read_dir(&self.root)
            .map_err(|error| CloudBoxError::ReceiptUnavailable(error.to_string()))?;
        let mut histories = Vec::new();
        for entry in entries {
            let entry =
                entry.map_err(|error| CloudBoxError::ReceiptUnavailable(error.to_string()))?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Some(name) = path.file_name() else {
                continue;
            };
            let stem = path.file_stem().and_then(|s| s.to_str()).ok_or_else(|| {
                CloudBoxError::ReceiptUnavailable("invalid receipt filename".into())
            })?;
            let expected_id = CloudBoxId::parse(stem).map_err(|_| {
                CloudBoxError::ReceiptUnavailable("receipt filename is not a UUID".into())
            })?;
            let file = dir
                .open_file(name, OpenMode::ReadNonblocking)
                .map_err(|error| CloudBoxError::ReceiptUnavailable(error.to_string()))?;
            let mut bytes = Vec::new();
            file.take(MAX_HISTORY_BYTES + 1)
                .read_to_end(&mut bytes)
                .map_err(|error| CloudBoxError::ReceiptUnavailable(error.to_string()))?;
            if bytes.len() as u64 > MAX_HISTORY_BYTES {
                return Err(CloudBoxError::ReceiptUnavailable(format!(
                    "receipt file `{}` exceeded maximum size",
                    path.display()
                )));
            }
            histories.push(decode_history(&bytes, &expected_id)?);
        }
        Ok(histories)
    }

    fn read(&self, box_id: &CloudBoxId) -> Result<Vec<CloudBoxRecord>, CloudBoxError> {
        if !self.root.exists() {
            return Ok(Vec::new());
        }
        let dir = PrivateDirectory::open_root(&self.root)
            .map_err(|error| CloudBoxError::ReceiptUnavailable(error.to_string()))?;
        let name = Self::file_name(box_id);
        let file = match dir.open_file(OsStr::new(&name), OpenMode::ReadNonblocking) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(CloudBoxError::ReceiptUnavailable(error.to_string())),
        };
        let mut bytes = Vec::new();
        file.take(MAX_HISTORY_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| CloudBoxError::ReceiptUnavailable(error.to_string()))?;
        if bytes.len() as u64 > MAX_HISTORY_BYTES {
            return Err(CloudBoxError::ReceiptUnavailable(
                "cloud box receipt history exceeded maximum size".into(),
            ));
        }
        decode_history(&bytes, box_id)
    }

    fn append(&self, box_id: &CloudBoxId, record: CloudBoxRecord) -> Result<(), CloudBoxError> {
        let mut history = self.read(box_id)?;
        history.push(record);
        CloudBoxRecord::validate_history(&history)?;

        let mut bytes = encode_history(&history)?;
        if bytes.len() as u64 > MAX_HISTORY_BYTES {
            history = compact_history(&history);
            CloudBoxRecord::validate_history(&history)?;
            bytes = encode_history(&history)?;
            if bytes.len() as u64 > MAX_HISTORY_BYTES {
                return Err(CloudBoxError::ReceiptUnavailable(
                    "cloud box receipt history exceeded maximum size".into(),
                ));
            }
        }

        create_directory(&self.root)
            .map_err(|error| CloudBoxError::ReceiptUnavailable(error.to_string()))?;
        let dir = PrivateDirectory::open_root(&self.root)
            .map_err(|error| CloudBoxError::ReceiptUnavailable(error.to_string()))?;
        let temp = dir
            .reserve_temp()
            .map_err(|error| CloudBoxError::ReceiptUnavailable(error.to_string()))?;
        temp.as_file()
            .write_all(&bytes)
            .map_err(|error| CloudBoxError::ReceiptUnavailable(error.to_string()))?;
        temp.as_file()
            .sync_all()
            .map_err(|error| CloudBoxError::ReceiptUnavailable(error.to_string()))?;
        let name = Self::file_name(box_id);
        temp.replace(OsStr::new(&name))
            .map_err(|error| CloudBoxError::ReceiptUnavailable(error.to_string()))?;
        Ok(())
    }
}

/// Compact a validated receipt history so repeated wake/sleep/rename/URL
/// updates never exhaust `MAX_HISTORY_BYTES`, while preserving the full
/// ADR 1160 provenance chain:
/// 1. The initial `Provisioning` intent record (index 0).
/// 2. The first `Active` record that established `(cloud, vm, admission, gateway_id)`.
/// 3. The latest record (current `Active` state or terminal `Refused`/`Deleted`).
fn compact_history(history: &[CloudBoxRecord]) -> Vec<CloudBoxRecord> {
    if history.len() <= 3 {
        return history.to_vec();
    }
    let last_idx = history.len() - 1;
    let first_active_idx = history
        .iter()
        .position(|record| matches!(record, CloudBoxRecord::Active { .. }));
    let mut keep = vec![0usize];
    if let Some(active_idx) = first_active_idx {
        if active_idx != 0 && active_idx != last_idx {
            keep.push(active_idx);
        }
    }
    if last_idx != 0 {
        keep.push(last_idx);
    }
    keep.into_iter().map(|idx| history[idx].clone()).collect()
}

fn deserialize_zeroizing<'de, D>(deserializer: D) -> Result<Zeroizing<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    String::deserialize(deserializer).map(Zeroizing::new)
}

#[derive(Deserialize)]
struct BootstrapCreateOutput {
    gateway_id: String,
    url: String,
    #[serde(deserialize_with = "deserialize_zeroizing")]
    credential: Zeroizing<String>,
}

#[derive(Deserialize)]
struct BootstrapStatusOutput {
    gateway_id: String,
    url: String,
}

#[derive(Deserialize)]
struct BootstrapBusyOutput {
    busy: bool,
}

fn last_json_line(output: &str) -> Result<&str, CloudBoxError> {
    output
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .ok_or_else(|| {
            CloudBoxError::BootstrapFailed("gateway bootstrap produced empty output".into())
        })
}

fn map_provider_failure(failure: Failure) -> CloudBoxError {
    match failure {
        Failure::Locked => CloudBoxError::VaultLocked,
        Failure::NotConfigured => CloudBoxError::AccountDisconnected,
        Failure::AdmissionRejected(rejection) => {
            CloudBoxError::AdmissionRefused(format!("{rejection:?}"))
        }
        other => CloudBoxError::ProviderFailed(format!("{other:?}")),
    }
}

/// Provider-agnostic [`CloudBoxProvider`] adapter backed by an existing
/// workspace [`Provider`], bootstrapping headless `nessa-server` inside the
/// provisioned VM over its authenticated runtime connection.
pub struct WorkspaceCloudBoxProvider {
    provider: Arc<dyn Provider>,
    pinned_image: Option<&'static str>,
}

impl WorkspaceCloudBoxProvider {
    pub fn new(provider: Arc<dyn Provider>, pinned_image: Option<&'static str>) -> Self {
        Self {
            provider,
            pinned_image,
        }
    }

    async fn rollback_or_uncertain(
        &self,
        account: &crate::workspaces::domain::CloudAccount,
        vm: &str,
        admission: &Admitted,
        primary: CloudBoxError,
    ) -> CloudBoxError {
        match self.provider.remove(account, vm, admission).await {
            Ok(()) => primary,
            Err(cleanup_error) => CloudBoxError::CleanupUncertain(format!(
                "vm={vm}; cause={primary}; cleanup={cleanup_error:?}"
            )),
        }
    }
}

impl CloudBoxProvider for WorkspaceCloudBoxProvider {
    fn create_box<'a>(
        &'a self,
        connection: &'a CloudConnection,
        box_id: &'a CloudBoxId,
    ) -> BoxFuture<'a, Result<ProvisionedCloudBox, CloudBoxError>> {
        Box::pin(async move {
            let account = connection.account();
            let spec = connection.new_vm(self.pinned_image, VmLifetime::Unlimited);
            let key = format!("box-{}", box_id.as_str());
            let receipt = self
                .provider
                .create(&account, &spec, &key)
                .await
                .map_err(map_provider_failure)?;

            if let Some(rejection) = receipt.refusal {
                let primary = CloudBoxError::AdmissionRefused(format!("{rejection:?}"));
                if receipt.disposition == RefusalDisposition::RollbackNewVm {
                    return Err(self
                        .rollback_or_uncertain(&account, &receipt.vm, &receipt.admission, primary)
                        .await);
                }
                return Err(primary);
            }

            let runtime = match self
                .provider
                .connect(&account, &spec, &key, &receipt.vm, &receipt.admission)
                .await
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    return Err(self
                        .rollback_or_uncertain(
                            &account,
                            &receipt.vm,
                            &receipt.admission,
                            map_provider_failure(error),
                        )
                        .await);
                }
            };

            let raw = match runtime.run(BOOTSTRAP_CREATE_CMD.to_owned(), None).await {
                Ok(raw) => Zeroizing::new(raw),
                Err(error) => {
                    return Err(self
                        .rollback_or_uncertain(
                            &account,
                            &receipt.vm,
                            &receipt.admission,
                            CloudBoxError::BootstrapFailed(error.to_string()),
                        )
                        .await);
                }
            };

            let parsed: BootstrapCreateOutput = match last_json_line(&raw).and_then(|line| {
                serde_json::from_str(line)
                    .map_err(|error| CloudBoxError::BootstrapFailed(error.to_string()))
            }) {
                Ok(parsed) => parsed,
                Err(error) => {
                    return Err(self
                        .rollback_or_uncertain(&account, &receipt.vm, &receipt.admission, error)
                        .await);
                }
            };

            let gateway_id = parsed.gateway_id.trim().to_owned();
            let credential = Zeroizing::new(parsed.credential.trim().to_owned());
            let url = match CloudBoxUrl::new(parsed.url) {
                Ok(url) if !gateway_id.is_empty() && !credential.is_empty() => url,
                _ => {
                    return Err(self
                        .rollback_or_uncertain(
                            &account,
                            &receipt.vm,
                            &receipt.admission,
                            CloudBoxError::BootstrapFailed(
                                "gateway bootstrap returned incomplete enrollment metadata".into(),
                            ),
                        )
                        .await);
                }
            };

            Ok(ProvisionedCloudBox {
                vm: receipt.vm,
                image: spec.image,
                admission: receipt.admission,
                endpoint: CloudBoxEndpoint { gateway_id, url },
                credential,
            })
        })
    }

    fn wake_box<'a>(
        &'a self,
        connection: &'a CloudConnection,
        vm: &'a str,
        admission: &'a Admitted,
    ) -> BoxFuture<'a, Result<CloudBoxEndpoint, CloudBoxError>> {
        Box::pin(async move {
            let account = connection.account();
            let spec = connection.new_vm(self.pinned_image, VmLifetime::Unlimited);
            let runtime = self
                .provider
                .connect(&account, &spec, "box-wake", vm, admission)
                .await
                .map_err(map_provider_failure)?;
            let raw = runtime
                .run(BOOTSTRAP_STATUS_CMD.to_owned(), None)
                .await
                .map_err(|error| CloudBoxError::BootstrapFailed(error.to_string()))?;
            let line = last_json_line(&raw)?;
            let parsed: BootstrapStatusOutput = serde_json::from_str(line)
                .map_err(|error| CloudBoxError::BootstrapFailed(error.to_string()))?;
            let gateway_id = parsed.gateway_id.trim().to_owned();
            if gateway_id.is_empty() {
                return Err(CloudBoxError::BootstrapFailed(
                    "gateway status returned an empty gateway_id".into(),
                ));
            }
            let url = CloudBoxUrl::new(parsed.url)?;
            Ok(CloudBoxEndpoint { gateway_id, url })
        })
    }

    fn stop_box<'a>(
        &'a self,
        connection: &'a CloudConnection,
        vm: &'a str,
    ) -> BoxFuture<'a, Result<(), CloudBoxError>> {
        Box::pin(async move {
            self.provider
                .stop(&connection.account(), vm, RestCause::Idle)
                .await
                .map_err(map_provider_failure)
        })
    }

    fn delete_box<'a>(
        &'a self,
        connection: &'a CloudConnection,
        vm: &'a str,
        admission: &'a Admitted,
    ) -> BoxFuture<'a, Result<(), CloudBoxError>> {
        Box::pin(async move {
            self.provider
                .remove(&connection.account(), vm, admission)
                .await
                .map_err(map_provider_failure)
        })
    }
}

/// Durable file-backed [`CloudBoxAudit`] appending synced JSONL records inside
/// a private directory (`<namespace>/cloud-boxes-audit/events.jsonl`).
pub struct FileCloudBoxAudit {
    root: PathBuf,
    write_lock: Mutex<()>,
}

impl FileCloudBoxAudit {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            write_lock: Mutex::new(()),
        }
    }
}

#[derive(Serialize)]
struct StoredAuditLine<'a> {
    at_ms: u64,
    box_id: &'a str,
    cloud: Option<&'a str>,
    before: Option<StoredState>,
    after: Option<StoredState>,
    action: &'a str,
    cause: &'a str,
    initiator: &'a str,
}

impl CloudBoxAudit for FileCloudBoxAudit {
    fn record(&self, event: CloudBoxAuditEvent) -> Result<(), CloudBoxError> {
        let _guard = self
            .write_lock
            .lock()
            .map_err(|_| CloudBoxError::ReceiptUnavailable("audit lock poisoned".into()))?;
        create_directory(&self.root)
            .map_err(|error| CloudBoxError::ReceiptUnavailable(error.to_string()))?;
        let dir = PrivateDirectory::open_root(&self.root)
            .map_err(|error| CloudBoxError::ReceiptUnavailable(error.to_string()))?;
        let at_ms = clock::wall_now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|d| u64::try_from(d.as_millis()).ok())
            .unwrap_or(0);
        let line = StoredAuditLine {
            at_ms,
            box_id: event.box_id.as_str(),
            cloud: event.cloud.map(Cloud::as_str),
            before: event.before.map(StoredState::from),
            after: event.after.map(StoredState::from),
            action: event.action,
            cause: &event.cause,
            initiator: &event.initiator,
        };
        let mut encoded = serde_json::to_vec(&line)
            .map_err(|error| CloudBoxError::ReceiptUnavailable(error.to_string()))?;
        encoded.push(b'\n');
        let mut file = dir
            .open_file(OsStr::new("events.jsonl"), OpenMode::OpenOrCreate)
            .map_err(|error| CloudBoxError::ReceiptUnavailable(error.to_string()))?;
        file.seek(SeekFrom::End(0))
            .map_err(|error| CloudBoxError::ReceiptUnavailable(error.to_string()))?;
        file.write_all(&encoded)
            .map_err(|error| CloudBoxError::ReceiptUnavailable(error.to_string()))?;
        file.sync_all()
            .map_err(|error| CloudBoxError::ReceiptUnavailable(error.to_string()))?;
        Ok(())
    }
}

/// Real [`ActiveWorkChecker`] enforcing G5 by querying the running cloud box's
/// gateway bootstrap status (`BOOTSTRAP_BUSY_CMD`) over its authenticated
/// runtime connection. Fails closed (`Err`) if active work cannot be verified.
pub struct ProviderActiveWorkChecker {
    providers: HashMap<Cloud, (Arc<dyn Provider>, Option<&'static str>)>,
}

impl ProviderActiveWorkChecker {
    pub fn new(entries: Vec<(Cloud, Arc<dyn Provider>, Option<&'static str>)>) -> Self {
        let providers = entries
            .into_iter()
            .map(|(cloud, provider, pinned)| (cloud, (provider, pinned)))
            .collect();
        Self { providers }
    }
}

impl ActiveWorkChecker for ProviderActiveWorkChecker {
    fn has_active_work<'a>(
        &'a self,
        cloud: Cloud,
        connection: &'a CloudConnection,
        vm: &'a str,
        admission: &'a Admitted,
    ) -> BoxFuture<'a, Result<bool, CloudBoxError>> {
        Box::pin(async move {
            let Some((provider, pinned)) = self.providers.get(&cloud) else {
                return Err(CloudBoxError::AccountDisconnected);
            };
            let spec = connection.new_vm(*pinned, VmLifetime::Unlimited);
            let runtime = provider
                .connect(&connection.account(), &spec, "box-busy", vm, admission)
                .await
                .map_err(map_provider_failure)?;
            let raw = runtime
                .run(BOOTSTRAP_BUSY_CMD.to_owned(), None)
                .await
                .map_err(|error| CloudBoxError::BootstrapFailed(error.to_string()))?;
            let line = last_json_line(&raw)?;
            let parsed: BootstrapBusyOutput = serde_json::from_str(line)
                .map_err(|error| CloudBoxError::BootstrapFailed(error.to_string()))?;
            Ok(parsed.busy)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspaces::{
        application::{ServedCopy, WorkspaceReceipt, WorkspaceRuntime, WorkspaceStream},
        domain::{
            AdmissionRefusal, AdmissionRejection, BoatConnection, Cloud, CloudAccount,
            CloudBoxName, CloudBoxState, TemplateRelease, VmSpec,
        },
    };
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct FakeRuntime {
        output: String,
    }

    #[async_trait]
    impl WorkspaceRuntime for FakeRuntime {
        async fn serve(&self, _copy: ServedCopy) -> Result<WorkspaceStream, Failure> {
            Err(Failure::Provider)
        }
        async fn image_protocol(&self) -> Option<String> {
            None
        }
        async fn run(
            &self,
            _command: String,
            _input: Option<std::fs::File>,
        ) -> std::io::Result<String> {
            Ok(self.output.clone())
        }
    }

    struct FakeWorkspaceProvider {
        runtime_output: Mutex<String>,
        refusal: Mutex<Option<AdmissionRejection>>,
        removed: Mutex<Vec<String>>,
        fail_remove: AtomicBool,
    }

    #[async_trait]
    impl Provider for FakeWorkspaceProvider {
        async fn validate(&self, _account: &CloudAccount, _spec: &VmSpec) -> Result<(), Failure> {
            Ok(())
        }
        async fn create(
            &self,
            _account: &CloudAccount,
            _spec: &VmSpec,
            _thread: &str,
        ) -> Result<WorkspaceReceipt, Failure> {
            Ok(WorkspaceReceipt {
                vm: "vm-99".into(),
                admission: Admitted::Agent37(
                    TemplateRelease::new("v1".into(), Some(1), None).unwrap(),
                ),
                refusal: self.refusal.lock().unwrap().clone(),
                disposition: RefusalDisposition::RollbackNewVm,
            })
        }
        async fn connect(
            &self,
            _account: &CloudAccount,
            _spec: &VmSpec,
            _thread: &str,
            _vm: &str,
            _admission: &Admitted,
        ) -> Result<Arc<dyn WorkspaceRuntime>, Failure> {
            Ok(Arc::new(FakeRuntime {
                output: self.runtime_output.lock().unwrap().clone(),
            }))
        }
        async fn stop(
            &self,
            _account: &CloudAccount,
            _vm: &str,
            _cause: RestCause,
        ) -> Result<(), Failure> {
            Ok(())
        }
        async fn remove(
            &self,
            _account: &CloudAccount,
            vm: &str,
            _admission: &Admitted,
        ) -> Result<(), Failure> {
            self.removed.lock().unwrap().push(vm.to_owned());
            if self.fail_remove.load(Ordering::SeqCst) {
                Err(Failure::Provider)
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn round_trips_validated_history_atomically_and_persists_audit_lines() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileCloudBoxStore::new(dir.path().join("cloud-boxes"));
        let audit = FileCloudBoxAudit::new(dir.path().join("cloud-boxes-audit"));
        let id = CloudBoxId::parse("11111111-2222-3333-4444-555555555555").unwrap();
        let name = CloudBoxName::new("Cloud Box").unwrap();

        assert!(store.list().unwrap().is_empty());
        store
            .append(
                &id,
                CloudBoxRecord::Provisioning {
                    box_id: id.clone(),
                    name: name.clone(),
                    cloud: Cloud::Agent37,
                    created_at_ms: 100,
                },
            )
            .unwrap();
        store
            .append(
                &id,
                CloudBoxRecord::Active {
                    box_id: id.clone(),
                    name,
                    cloud: Cloud::Agent37,
                    vm: "vm-1".into(),
                    image: "img-1".into(),
                    admission: Admitted::Agent37(
                        TemplateRelease::new("v1".into(), Some(1), None).unwrap(),
                    ),
                    gateway_id: "gw-1".into(),
                    url: CloudBoxUrl::new("wss://box.example.com").unwrap(),
                    state: CloudBoxState::Running,
                    created_at_ms: 100,
                    updated_at_ms: 200,
                },
            )
            .unwrap();

        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].len(), 2);
        assert_eq!(
            listed[0].last().unwrap().as_summary().unwrap().gateway_id,
            "gw-1"
        );

        audit
            .record(CloudBoxAuditEvent {
                box_id: id,
                cloud: Some(Cloud::Agent37),
                before: Some(CloudBoxState::Provisioning),
                after: Some(CloudBoxState::Running),
                action: "create_admitted",
                cause: "provisioned".into(),
                initiator: "owner".into(),
            })
            .unwrap();
        let logged = fs::read_to_string(dir.path().join("cloud-boxes-audit/events.jsonl")).unwrap();
        assert!(logged.contains("\"action\":\"create_admitted\""));
    }

    #[tokio::test]
    async fn workspace_cloud_box_provider_bootstraps_rolls_back_and_surfaces_cleanup_uncertain() {
        let connection = CloudConnection::Boat(
            BoatConnection::new("tok".into(), "env".into(), "img".into()).unwrap(),
        );
        let id = CloudBoxId::parse("11111111-2222-3333-4444-555555555555").unwrap();
        let inner = Arc::new(FakeWorkspaceProvider {
            runtime_output: Mutex::new(
                r#"{"gateway_id":"gw-99","url":"wss://gw99.example.com","credential":"tok-99"}"#
                    .into(),
            ),
            refusal: Mutex::new(None),
            removed: Mutex::new(Vec::new()),
            fail_remove: AtomicBool::new(false),
        });
        let adapter = WorkspaceCloudBoxProvider::new(inner.clone(), None);
        let provisioned = adapter.create_box(&connection, &id).await.unwrap();
        assert_eq!(provisioned.vm, "vm-99");
        assert_eq!(provisioned.endpoint.gateway_id, "gw-99");
        assert_eq!(provisioned.credential.as_str(), "tok-99");

        // Wake returns refreshed endpoint.
        *inner.runtime_output.lock().unwrap() =
            r#"{"gateway_id":"gw-99","url":"wss://gw99-new.example.com"}"#.into();
        let woken = adapter
            .wake_box(&connection, "vm-99", &provisioned.admission)
            .await
            .unwrap();
        assert_eq!(woken.url.as_str(), "wss://gw99-new.example.com/");

        // Active work checker reads busy JSON.
        let checker = ProviderActiveWorkChecker::new(vec![(Cloud::Boat, inner.clone(), None)]);
        *inner.runtime_output.lock().unwrap() = r#"{"busy":true}"#.into();
        assert!(checker
            .has_active_work(Cloud::Boat, &connection, "vm-99", &provisioned.admission)
            .await
            .unwrap());

        // Refusal triggers rollback.
        *inner.refusal.lock().unwrap() = Some(AdmissionRejection::new(
            AdmissionRefusal::WrongVersion,
            None,
        ));
        assert!(matches!(
            adapter.create_box(&connection, &id).await.unwrap_err(),
            CloudBoxError::AdmissionRefused(_)
        ));
        assert_eq!(inner.removed.lock().unwrap().as_slice(), ["vm-99"]);

        // Failed rollback surfaces CleanupUncertain (M1).
        inner.fail_remove.store(true, Ordering::SeqCst);
        assert!(matches!(
            adapter.create_box(&connection, &id).await.unwrap_err(),
            CloudBoxError::CleanupUncertain(_)
        ));
    }

    #[test]
    fn file_store_compacts_repeated_active_transitions_without_exhausting_budget() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileCloudBoxStore::new(dir.path().join("cloud-boxes"));
        let id = CloudBoxId::parse("22222222-3333-4444-5555-666666666666").unwrap();
        let admission =
            Admitted::Agent37(TemplateRelease::new("v2026.1".into(), None, None).unwrap());

        store
            .append(
                &id,
                CloudBoxRecord::Provisioning {
                    box_id: id.clone(),
                    name: CloudBoxName::new("Long Lived Box").unwrap(),
                    cloud: Cloud::Agent37,
                    created_at_ms: 100,
                },
            )
            .unwrap();

        // Simulate 300 wake/sleep/rename cycles (~90+ KiB uncompacted, well above 64 KiB).
        for step in 0..300u64 {
            let state = if step % 2 == 0 {
                CloudBoxState::Running
            } else {
                CloudBoxState::Sleeping
            };
            store
                .append(
                    &id,
                    CloudBoxRecord::Active {
                        box_id: id.clone(),
                        name: CloudBoxName::new(format!("Box #{step}")).unwrap(),
                        cloud: Cloud::Agent37,
                        vm: "vm-persistent".into(),
                        admission: admission.clone(),
                        gateway_id: "gw-persistent".into(),
                        url: CloudBoxUrl::new(format!("wss://box-{step}.example.com")).unwrap(),
                        state,
                        image: "nessa-gateway:v1".into(),
                        created_at_ms: 100,
                        updated_at_ms: 200 + step,
                    },
                )
                .unwrap();
        }

        let history = store.read(&id).unwrap();
        assert!(
            history.len() <= 301 && history.len() >= 3,
            "compacted history should retain provenance chain"
        );
        assert!(matches!(history[0], CloudBoxRecord::Provisioning { .. }));
        assert!(matches!(history[1], CloudBoxRecord::Active { .. }));
        let latest = history.last().unwrap().as_summary().unwrap();
        assert_eq!(latest.name.as_str(), "Box #299");
        assert_eq!(latest.state, CloudBoxState::Sleeping);
        assert_eq!(latest.vm, "vm-persistent");
        assert_eq!(latest.gateway_id, "gw-persistent");
        CloudBoxRecord::validate_history(&history).unwrap();

        let file_len = fs::metadata(
            dir.path()
                .join("cloud-boxes")
                .join(FileCloudBoxStore::file_name(&id)),
        )
        .unwrap()
        .len();
        assert!(file_len <= MAX_HISTORY_BYTES);
    }
}
