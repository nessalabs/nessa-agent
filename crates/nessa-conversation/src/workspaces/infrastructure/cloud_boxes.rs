//! Durable file-backed receipt store and provider adapter for persistent
//! cloud gateway VMs (`<namespace>/cloud-boxes/<uuid>.json`), using atomic
//! temp-file replacement inside a private directory
//! (`nessa_local_storage::PrivateDirectory`).

use crate::domain::VmLifetime;
use crate::workspaces::{
    application::{
        ActiveWorkChecker, BoxFuture, CloudBoxAudit, CloudBoxAuditEvent, CloudBoxProvider,
        CloudBoxStore, Provider, ProvisionedCloudBox, RestCause,
    },
    domain::{
        Admitted, CloudBoxEndpoint, CloudBoxError, CloudBoxId, CloudBoxRecord, CloudBoxUrl,
        CloudConnection, Failure, RefusalDisposition,
    },
};
use nessa_local_storage::{create_directory, OpenMode, PrivateDirectory};
use serde::Deserialize;
use std::{
    ffi::OsStr,
    fs,
    io::{Read, Write},
    path::PathBuf,
    sync::Arc,
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
            let history: Vec<CloudBoxRecord> = serde_json::from_slice(&bytes)
                .map_err(|error| CloudBoxError::ReceiptUnavailable(error.to_string()))?;
            histories.push(history);
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
        serde_json::from_slice(&bytes)
            .map_err(|error| CloudBoxError::ReceiptUnavailable(error.to_string()))
    }

    fn append(&self, box_id: &CloudBoxId, record: CloudBoxRecord) -> Result<(), CloudBoxError> {
        let mut history = self.read(box_id)?;
        history.push(record);
        CloudBoxRecord::validate_history(&history)?;

        let bytes = serde_json::to_vec_pretty(&history)
            .map_err(|error| CloudBoxError::ReceiptUnavailable(error.to_string()))?;
        if bytes.len() as u64 > MAX_HISTORY_BYTES {
            return Err(CloudBoxError::ReceiptUnavailable(
                "cloud box receipt history exceeded maximum size".into(),
            ));
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

#[derive(Deserialize)]
struct BootstrapCreateOutput {
    gateway_id: String,
    url: String,
    credential: String,
}

#[derive(Deserialize)]
struct BootstrapStatusOutput {
    gateway_id: String,
    url: String,
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
                if receipt.disposition == RefusalDisposition::RollbackNewVm {
                    let _ = self
                        .provider
                        .remove(&account, &receipt.vm, &receipt.admission)
                        .await;
                }
                return Err(CloudBoxError::AdmissionRefused(format!("{rejection:?}")));
            }

            let runtime = match self
                .provider
                .connect(&account, &spec, &key, &receipt.vm, &receipt.admission)
                .await
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ = self
                        .provider
                        .remove(&account, &receipt.vm, &receipt.admission)
                        .await;
                    return Err(map_provider_failure(error));
                }
            };

            let raw = match runtime.run(BOOTSTRAP_CREATE_CMD.to_owned(), None).await {
                Ok(raw) => raw,
                Err(error) => {
                    let _ = self
                        .provider
                        .remove(&account, &receipt.vm, &receipt.admission)
                        .await;
                    return Err(CloudBoxError::BootstrapFailed(error.to_string()));
                }
            };

            let parsed: BootstrapCreateOutput = match last_json_line(&raw).and_then(|line| {
                serde_json::from_str(line)
                    .map_err(|error| CloudBoxError::BootstrapFailed(error.to_string()))
            }) {
                Ok(parsed) => parsed,
                Err(error) => {
                    let _ = self
                        .provider
                        .remove(&account, &receipt.vm, &receipt.admission)
                        .await;
                    return Err(error);
                }
            };

            let gateway_id = parsed.gateway_id.trim().to_owned();
            let credential = Zeroizing::new(parsed.credential.trim().to_owned());
            let url = match CloudBoxUrl::new(parsed.url) {
                Ok(url) if !gateway_id.is_empty() && !credential.is_empty() => url,
                _ => {
                    let _ = self
                        .provider
                        .remove(&account, &receipt.vm, &receipt.admission)
                        .await;
                    return Err(CloudBoxError::BootstrapFailed(
                        "gateway bootstrap returned incomplete enrollment metadata".into(),
                    ));
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

/// Tracing-backed [`CloudBoxAudit`] emitting structured audit events on every
/// cloud box lifecycle transition (including refusal and rollback paths).
pub struct TracingCloudBoxAudit;

impl CloudBoxAudit for TracingCloudBoxAudit {
    fn record(&self, event: CloudBoxAuditEvent) {
        tracing::info!(
            target: "nessa.audit.cloud_boxes",
            box_id = event.box_id.as_str(),
            cloud = event.cloud.map(|c| c.as_str()).unwrap_or("unknown"),
            before = ?event.before,
            after = ?event.after,
            action = event.action,
            cause = %event.cause,
            initiator = %event.initiator,
            "cloud box lifecycle transition"
        );
    }
}

/// Default [`ActiveWorkChecker`] reporting no local work on the control-plane
/// gateway (remote gateways enforce their own idle checks when queried).
pub struct IdleActiveWorkChecker;

impl ActiveWorkChecker for IdleActiveWorkChecker {
    fn has_active_work<'a>(
        &'a self,
        _box_id: &'a CloudBoxId,
    ) -> BoxFuture<'a, Result<bool, CloudBoxError>> {
        Box::pin(async { Ok(false) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspaces::{
        application::{WorkspaceReceipt, WorkspaceRuntime},
        domain::{
            AdmissionRefusal, AdmissionRejection, BoatConnection, Cloud, CloudAccount,
            CloudBoxName, CloudBoxState, TemplateRelease, VmSpec,
        },
    };
    use async_trait::async_trait;
    use std::sync::Mutex;

    struct FakeRuntime {
        output: String,
    }

    #[async_trait]
    impl WorkspaceRuntime for FakeRuntime {
        async fn serve(
            &self,
            _copy: crate::workspaces::application::ServedCopy,
        ) -> Result<crate::workspaces::application::WorkspaceStream, Failure> {
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
            Ok(())
        }
    }

    #[test]
    fn round_trips_validated_history_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileCloudBoxStore::new(dir.path().join("cloud-boxes"));
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
    }

    #[tokio::test]
    async fn workspace_cloud_box_provider_bootstraps_and_rolls_back_on_refusal() {
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
    }
}
