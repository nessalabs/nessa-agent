//! Persistent cloud gateway VM orchestration service (ADR 1417, #1420).
//!
//! Coordinates durable receipt storage (ADR 1160), provider VM lifecycle
//! (`create`, `wake`, `sleep`, `delete`), active-work guards before sleeping
//! (G5), single-flight per-box locking (G3), and durable audit evidence on
//! every transition including refusal and cleanup paths.

use super::workspaces::Settings;
use crate::timer::clock;
use crate::workspaces::domain::{
    Admitted, Cloud, CloudBoxEndpoint, CloudBoxEnrollment, CloudBoxError, CloudBoxId, CloudBoxName,
    CloudBoxRecord, CloudBoxState, CloudBoxSummary, CloudBoxUrl, CloudConnection, Failure,
};
use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{Arc, Weak},
    time::UNIX_EPOCH,
};
use tokio::sync::Mutex;
use zeroize::Zeroizing;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Provisioned VM details returned by a provider after creating and
/// bootstrapping a headless gateway inside the VM.
#[derive(Debug)]
pub struct ProvisionedCloudBox {
    pub vm: String,
    pub image: String,
    pub admission: Admitted,
    pub endpoint: CloudBoxEndpoint,
    pub credential: Zeroizing<String>,
}

/// Process-boundary port for cloud VM provider lifecycle operations.
pub trait CloudBoxProvider: Send + Sync {
    fn create_box<'a>(
        &'a self,
        connection: &'a CloudConnection,
        box_id: &'a CloudBoxId,
    ) -> BoxFuture<'a, Result<ProvisionedCloudBox, CloudBoxError>>;

    fn wake_box<'a>(
        &'a self,
        connection: &'a CloudConnection,
        vm: &'a str,
        admission: &'a Admitted,
    ) -> BoxFuture<'a, Result<CloudBoxEndpoint, CloudBoxError>>;

    fn stop_box<'a>(
        &'a self,
        connection: &'a CloudConnection,
        vm: &'a str,
    ) -> BoxFuture<'a, Result<(), CloudBoxError>>;

    fn delete_box<'a>(
        &'a self,
        connection: &'a CloudConnection,
        vm: &'a str,
        admission: &'a Admitted,
    ) -> BoxFuture<'a, Result<(), CloudBoxError>>;
}

/// Process-boundary port for durable append-only cloud box receipt storage.
pub trait CloudBoxStore: Send + Sync {
    fn list(&self) -> Result<Vec<Vec<CloudBoxRecord>>, CloudBoxError>;
    fn read(&self, box_id: &CloudBoxId) -> Result<Vec<CloudBoxRecord>, CloudBoxError>;
    fn append(&self, box_id: &CloudBoxId, record: CloudBoxRecord) -> Result<(), CloudBoxError>;
}

/// Process-boundary guard checking whether a running cloud box currently has
/// active conversations or terminals before allowing it to sleep (G5).
pub trait ActiveWorkChecker: Send + Sync {
    fn has_active_work<'a>(
        &'a self,
        cloud: Cloud,
        connection: &'a CloudConnection,
        vm: &'a str,
        admission: &'a Admitted,
    ) -> BoxFuture<'a, Result<bool, CloudBoxError>>;
}

/// Consequential lifecycle audit event for persistent cloud boxes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudBoxAuditEvent {
    pub box_id: CloudBoxId,
    pub cloud: Option<Cloud>,
    pub before: Option<CloudBoxState>,
    pub after: Option<CloudBoxState>,
    pub action: &'static str,
    pub cause: String,
    pub initiator: String,
}

/// Durable audit sink for every cloud box transition, including refusal and
/// cleanup paths. Audit delivery failures are observable (`Err`).
pub trait CloudBoxAudit: Send + Sync {
    fn record(&self, event: CloudBoxAuditEvent) -> Result<(), CloudBoxError>;
}

/// One cloud provider's settings + box provider wired into [`CloudBoxes`].
pub struct CloudBoxBackend {
    pub cloud: Cloud,
    pub settings: Arc<dyn Settings>,
    pub provider: Arc<dyn CloudBoxProvider>,
}

/// Orchestration service for persistent cloud gateway VMs.
pub struct CloudBoxes {
    backends: HashMap<Cloud, CloudBoxBackend>,
    store: Arc<dyn CloudBoxStore>,
    active_work: Arc<dyn ActiveWorkChecker>,
    audit: Arc<dyn CloudBoxAudit>,
    locks: Mutex<HashMap<String, Weak<Mutex<()>>>>,
}

impl CloudBoxes {
    pub fn new(
        backends: Vec<CloudBoxBackend>,
        store: Arc<dyn CloudBoxStore>,
        active_work: Arc<dyn ActiveWorkChecker>,
        audit: Arc<dyn CloudBoxAudit>,
    ) -> Self {
        let map = backends
            .into_iter()
            .map(|backend| (backend.cloud, backend))
            .collect();
        Self {
            backends: map,
            store,
            active_work,
            audit,
            locks: Mutex::new(HashMap::new()),
        }
    }

    fn now_ms() -> u64 {
        clock::wall_now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|d| u64::try_from(d.as_millis()).ok())
            .unwrap_or(0)
    }

    async fn box_lock(&self, box_id: &CloudBoxId) -> Arc<Mutex<()>> {
        let mut map = self.locks.lock().await;
        map.retain(|_, weak| weak.strong_count() > 0);
        if let Some(existing) = map.get(box_id.as_str()).and_then(Weak::upgrade) {
            return existing;
        }
        let lock = Arc::new(Mutex::new(()));
        map.insert(box_id.as_str().to_owned(), Arc::downgrade(&lock));
        lock
    }

    fn backend(&self, cloud: Cloud) -> Result<&CloudBoxBackend, CloudBoxError> {
        self.backends
            .get(&cloud)
            .ok_or(CloudBoxError::AccountDisconnected)
    }

    async fn load_connection(
        &self,
        cloud: Cloud,
    ) -> Result<(&CloudBoxBackend, CloudConnection), CloudBoxError> {
        let backend = self.backend(cloud)?;
        let connection = backend
            .settings
            .read()
            .await
            .map_err(|failure| match failure {
                Failure::Locked => CloudBoxError::VaultLocked,
                Failure::NotConfigured => CloudBoxError::AccountDisconnected,
                other => CloudBoxError::ProviderFailed(format!("{other:?}")),
            })?;
        Ok((backend, connection))
    }

    fn latest_active(history: &[CloudBoxRecord]) -> Result<CloudBoxRecord, CloudBoxError> {
        match history.last().cloned() {
            Some(record @ CloudBoxRecord::Active { .. }) => Ok(record),
            Some(CloudBoxRecord::Refused { .. }) => Err(CloudBoxError::BoxRefused),
            _ => Err(CloudBoxError::NotActive),
        }
    }

    /// List all currently active (running or sleeping) cloud boxes.
    pub fn list(&self) -> Result<Vec<CloudBoxSummary>, CloudBoxError> {
        let histories = self.store.list()?;
        let mut summaries = Vec::new();
        for history in histories {
            CloudBoxRecord::validate_history(&history)?;
            if let Some(summary) = history.last().and_then(CloudBoxRecord::as_summary) {
                summaries.push(summary);
            }
        }
        summaries.sort_by_key(|s| s.created_at_ms);
        Ok(summaries)
    }

    /// Create and bootstrap a persistent cloud gateway VM (ADR 1160):
    /// persists `Provisioning` intent before calling the provider, records
    /// `Active` receipt on success (rolling back on receipt-save failure), and
    /// returns the enrollment credential once.
    pub async fn create(
        &self,
        box_id: CloudBoxId,
        name: CloudBoxName,
        cloud: Cloud,
        initiator: impl Into<String>,
    ) -> Result<CloudBoxEnrollment, CloudBoxError> {
        let initiator = initiator.into();
        let lock = self.box_lock(&box_id).await;
        let _guard = lock.lock().await;

        if !self.store.read(&box_id)?.is_empty() {
            return Err(CloudBoxError::AlreadyExists);
        }

        let (backend, connection) = self.load_connection(cloud).await?;
        let created_at_ms = Self::now_ms();

        // ADR 1160: persist durable intent BEFORE provider creation.
        self.store.append(
            &box_id,
            CloudBoxRecord::Provisioning {
                box_id: box_id.clone(),
                name: name.clone(),
                cloud,
                created_at_ms,
            },
        )?;
        self.audit.record(CloudBoxAuditEvent {
            box_id: box_id.clone(),
            cloud: Some(cloud),
            before: None,
            after: Some(CloudBoxState::Provisioning),
            action: "create_requested",
            cause: "user_request".into(),
            initiator: initiator.clone(),
        })?;

        let provisioned = match backend.provider.create_box(&connection, &box_id).await {
            Ok(provisioned) => provisioned,
            Err(error) => {
                let receipt_result = self.store.append(
                    &box_id,
                    CloudBoxRecord::Refused {
                        box_id: box_id.clone(),
                        name: name.clone(),
                        cloud,
                        reason: error.to_string(),
                        updated_at_ms: Self::now_ms(),
                    },
                );
                let cause = match &receipt_result {
                    Ok(()) => error.to_string(),
                    Err(receipt_err) => format!("{error}; refusal_receipt_error={receipt_err}"),
                };
                let after = if receipt_result.is_ok() {
                    CloudBoxState::Refused
                } else {
                    CloudBoxState::Provisioning
                };
                let _ = self.audit.record(CloudBoxAuditEvent {
                    box_id,
                    cloud: Some(cloud),
                    before: Some(CloudBoxState::Provisioning),
                    after: Some(after),
                    action: "create_refused",
                    cause,
                    initiator,
                });
                return Err(error);
            }
        };

        let updated_at_ms = Self::now_ms();
        let active_record = CloudBoxRecord::Active {
            box_id: box_id.clone(),
            name: name.clone(),
            cloud,
            vm: provisioned.vm.clone(),
            image: provisioned.image,
            admission: provisioned.admission.clone(),
            gateway_id: provisioned.endpoint.gateway_id.clone(),
            url: provisioned.endpoint.url.clone(),
            state: CloudBoxState::Running,
            created_at_ms,
            updated_at_ms,
        };

        if let Err(save_error) = self.store.append(&box_id, active_record.clone()) {
            // Fail closed: roll back the newly created VM if we cannot durably
            // record its receipt; surface CleanupUncertain if rollback also fails.
            let cleanup = backend
                .provider
                .delete_box(&connection, &provisioned.vm, &provisioned.admission)
                .await;
            let _ = self.audit.record(CloudBoxAuditEvent {
                box_id,
                cloud: Some(cloud),
                before: Some(CloudBoxState::Provisioning),
                after: None,
                action: "create_receipt_failed_rollback",
                cause: format!(
                    "vm={}; save={save_error}; cleanup={cleanup:?}",
                    provisioned.vm
                ),
                initiator,
            });
            return match cleanup {
                Ok(()) => Err(save_error),
                Err(cleanup_error) => Err(CloudBoxError::CleanupUncertain(format!(
                    "vm={}; save={save_error}; cleanup={cleanup_error}",
                    provisioned.vm
                ))),
            };
        }

        let summary = active_record
            .as_summary()
            .expect("Active record always projects to summary");
        self.audit.record(CloudBoxAuditEvent {
            box_id,
            cloud: Some(cloud),
            before: Some(CloudBoxState::Provisioning),
            after: Some(CloudBoxState::Running),
            action: "create_admitted",
            cause: "provisioned".into(),
            initiator,
        })?;

        Ok(CloudBoxEnrollment {
            summary,
            credential: provisioned.credential,
        })
    }

    /// Wake a sleeping (or verify a running) cloud box under its per-box lock
    /// (G3), verifying its `gateway_id` and persisting any rotated `wss://`
    /// URL (G8).
    pub async fn wake(
        &self,
        box_id: &CloudBoxId,
        initiator: impl Into<String>,
    ) -> Result<CloudBoxEndpoint, CloudBoxError> {
        let initiator = initiator.into();
        let lock = self.box_lock(box_id).await;
        let _guard = lock.lock().await;

        let history = self.store.read(box_id)?;
        if history.is_empty() {
            return Err(CloudBoxError::NotFound);
        }
        CloudBoxRecord::validate_history(&history)?;

        let CloudBoxRecord::Active {
            name,
            cloud,
            vm,
            image,
            admission,
            gateway_id,
            state,
            created_at_ms,
            ..
        } = Self::latest_active(&history)?
        else {
            unreachable!()
        };

        let (backend, connection) = self.load_connection(cloud).await?;
        self.audit.record(CloudBoxAuditEvent {
            box_id: box_id.clone(),
            cloud: Some(cloud),
            before: Some(state),
            after: Some(CloudBoxState::Running),
            action: "wake_requested",
            cause: "client_wake".into(),
            initiator: initiator.clone(),
        })?;

        let endpoint = match backend
            .provider
            .wake_box(&connection, &vm, &admission)
            .await
        {
            Ok(endpoint) => endpoint,
            Err(error) => {
                let _ = self.audit.record(CloudBoxAuditEvent {
                    box_id: box_id.clone(),
                    cloud: Some(cloud),
                    before: Some(state),
                    after: Some(state),
                    action: "wake_refused",
                    cause: error.to_string(),
                    initiator,
                });
                return Err(error);
            }
        };

        if endpoint.gateway_id != gateway_id {
            let error = CloudBoxError::BootstrapFailed(
                "woken VM reported a different gateway_id than its receipt".into(),
            );
            let _ = self.audit.record(CloudBoxAuditEvent {
                box_id: box_id.clone(),
                cloud: Some(cloud),
                before: Some(state),
                after: Some(state),
                action: "wake_identity_mismatch",
                cause: error.to_string(),
                initiator,
            });
            return Err(error);
        }

        let updated = CloudBoxRecord::Active {
            box_id: box_id.clone(),
            name,
            cloud,
            vm,
            image,
            admission,
            gateway_id,
            url: endpoint.url.clone(),
            state: CloudBoxState::Running,
            created_at_ms,
            updated_at_ms: Self::now_ms(),
        };
        if let Err(save_error) = self.store.append(box_id, updated) {
            let _ = self.audit.record(CloudBoxAuditEvent {
                box_id: box_id.clone(),
                cloud: Some(cloud),
                before: Some(state),
                after: Some(CloudBoxState::Running),
                action: "wake_receipt_failed",
                cause: save_error.to_string(),
                initiator,
            });
            return Err(save_error);
        }
        self.audit.record(CloudBoxAuditEvent {
            box_id: box_id.clone(),
            cloud: Some(cloud),
            before: Some(state),
            after: Some(CloudBoxState::Running),
            action: "wake_completed",
            cause: "running".into(),
            initiator,
        })?;

        Ok(endpoint)
    }

    /// Stop an idle cloud box VM while keeping its persistent disk (G5).
    /// Refuses if active work is in flight on the box; idempotent if already
    /// sleeping.
    pub async fn sleep(
        &self,
        box_id: &CloudBoxId,
        initiator: impl Into<String>,
    ) -> Result<CloudBoxSummary, CloudBoxError> {
        let initiator = initiator.into();
        let lock = self.box_lock(box_id).await;
        let _guard = lock.lock().await;

        let history = self.store.read(box_id)?;
        if history.is_empty() {
            return Err(CloudBoxError::NotFound);
        }
        CloudBoxRecord::validate_history(&history)?;

        let active = Self::latest_active(&history)?;
        let CloudBoxRecord::Active {
            name,
            cloud,
            vm,
            image,
            admission,
            gateway_id,
            url,
            state,
            created_at_ms,
            ..
        } = active.clone()
        else {
            unreachable!()
        };

        if state == CloudBoxState::Sleeping {
            return Ok(active
                .as_summary()
                .expect("Active record always projects to summary"));
        }

        let (backend, connection) = self.load_connection(cloud).await?;

        // G5: never sleep while conversations or terminals are active.
        let busy = match self
            .active_work
            .has_active_work(cloud, &connection, &vm, &admission)
            .await
        {
            Ok(busy) => busy,
            Err(error) => {
                let _ = self.audit.record(CloudBoxAuditEvent {
                    box_id: box_id.clone(),
                    cloud: Some(cloud),
                    before: Some(state),
                    after: Some(state),
                    action: "sleep_active_work_check_failed",
                    cause: error.to_string(),
                    initiator,
                });
                return Err(error);
            }
        };
        if busy {
            let _ = self.audit.record(CloudBoxAuditEvent {
                box_id: box_id.clone(),
                cloud: Some(cloud),
                before: Some(state),
                after: Some(state),
                action: "sleep_refused",
                cause: "active_work_in_flight".into(),
                initiator,
            });
            return Err(CloudBoxError::ActiveWorkInFlight);
        }

        if let Err(error) = backend.provider.stop_box(&connection, &vm).await {
            let _ = self.audit.record(CloudBoxAuditEvent {
                box_id: box_id.clone(),
                cloud: Some(cloud),
                before: Some(state),
                after: Some(state),
                action: "sleep_failed",
                cause: error.to_string(),
                initiator,
            });
            return Err(error);
        }

        let updated = CloudBoxRecord::Active {
            box_id: box_id.clone(),
            name,
            cloud,
            vm,
            image,
            admission,
            gateway_id,
            url,
            state: CloudBoxState::Sleeping,
            created_at_ms,
            updated_at_ms: Self::now_ms(),
        };
        let summary = updated
            .as_summary()
            .expect("Active record always projects to summary");
        if let Err(save_error) = self.store.append(box_id, updated) {
            let _ = self.audit.record(CloudBoxAuditEvent {
                box_id: box_id.clone(),
                cloud: Some(cloud),
                before: Some(state),
                after: Some(CloudBoxState::Sleeping),
                action: "sleep_receipt_failed",
                cause: save_error.to_string(),
                initiator,
            });
            return Err(save_error);
        }
        self.audit.record(CloudBoxAuditEvent {
            box_id: box_id.clone(),
            cloud: Some(cloud),
            before: Some(state),
            after: Some(CloudBoxState::Sleeping),
            action: "sleep_completed",
            cause: "idle_or_user_request".into(),
            initiator,
        })?;

        Ok(summary)
    }

    /// Update the display name of an active cloud box.
    pub async fn rename(
        &self,
        box_id: &CloudBoxId,
        new_name: CloudBoxName,
        initiator: impl Into<String>,
    ) -> Result<CloudBoxSummary, CloudBoxError> {
        let initiator = initiator.into();
        let lock = self.box_lock(box_id).await;
        let _guard = lock.lock().await;

        let history = self.store.read(box_id)?;
        if history.is_empty() {
            return Err(CloudBoxError::NotFound);
        }
        CloudBoxRecord::validate_history(&history)?;

        let CloudBoxRecord::Active {
            cloud,
            vm,
            image,
            admission,
            gateway_id,
            url,
            state,
            created_at_ms,
            ..
        } = Self::latest_active(&history)?
        else {
            unreachable!()
        };

        let updated = CloudBoxRecord::Active {
            box_id: box_id.clone(),
            name: new_name,
            cloud,
            vm,
            image,
            admission,
            gateway_id,
            url,
            state,
            created_at_ms,
            updated_at_ms: Self::now_ms(),
        };
        let summary = updated
            .as_summary()
            .expect("Active record always projects to summary");
        if let Err(save_error) = self.store.append(box_id, updated) {
            let _ = self.audit.record(CloudBoxAuditEvent {
                box_id: box_id.clone(),
                cloud: Some(cloud),
                before: Some(state),
                after: Some(state),
                action: "rename_receipt_failed",
                cause: save_error.to_string(),
                initiator,
            });
            return Err(save_error);
        }
        self.audit.record(CloudBoxAuditEvent {
            box_id: box_id.clone(),
            cloud: Some(cloud),
            before: Some(state),
            after: Some(state),
            action: "renamed",
            cause: "user_request".into(),
            initiator,
        })?;
        Ok(summary)
    }

    /// Permanently destroy a cloud box VM and mark its receipt history
    /// terminally `Deleted` (G11).
    pub async fn delete(
        &self,
        box_id: &CloudBoxId,
        initiator: impl Into<String>,
    ) -> Result<(), CloudBoxError> {
        let initiator = initiator.into();
        let lock = self.box_lock(box_id).await;
        let _guard = lock.lock().await;

        let history = self.store.read(box_id)?;
        if history.is_empty() {
            return Err(CloudBoxError::NotFound);
        }
        CloudBoxRecord::validate_history(&history)?;

        let CloudBoxRecord::Active {
            cloud,
            vm,
            admission,
            state,
            ..
        } = Self::latest_active(&history)?
        else {
            unreachable!()
        };

        let (backend, connection) = self.load_connection(cloud).await?;
        if let Err(error) = backend
            .provider
            .delete_box(&connection, &vm, &admission)
            .await
        {
            let _ = self.audit.record(CloudBoxAuditEvent {
                box_id: box_id.clone(),
                cloud: Some(cloud),
                before: Some(state),
                after: Some(state),
                action: "delete_failed",
                cause: error.to_string(),
                initiator,
            });
            return Err(error);
        }

        if let Err(save_error) = self.store.append(
            box_id,
            CloudBoxRecord::Deleted {
                box_id: box_id.clone(),
                vm: Some(vm),
                updated_at_ms: Self::now_ms(),
            },
        ) {
            let _ = self.audit.record(CloudBoxAuditEvent {
                box_id: box_id.clone(),
                cloud: Some(cloud),
                before: Some(state),
                after: Some(CloudBoxState::Deleted),
                action: "delete_receipt_failed",
                cause: save_error.to_string(),
                initiator,
            });
            return Err(save_error);
        }
        self.audit.record(CloudBoxAuditEvent {
            box_id: box_id.clone(),
            cloud: Some(cloud),
            before: Some(state),
            after: Some(CloudBoxState::Deleted),
            action: "deleted",
            cause: "user_request".into(),
            initiator,
        })?;
        Ok(())
    }

    /// Persist a rotated endpoint URL for an active cloud box (G8).
    pub async fn update_url(
        &self,
        box_id: &CloudBoxId,
        new_url: CloudBoxUrl,
        initiator: impl Into<String>,
    ) -> Result<CloudBoxSummary, CloudBoxError> {
        let initiator = initiator.into();
        let lock = self.box_lock(box_id).await;
        let _guard = lock.lock().await;

        let history = self.store.read(box_id)?;
        if history.is_empty() {
            return Err(CloudBoxError::NotFound);
        }
        CloudBoxRecord::validate_history(&history)?;

        let CloudBoxRecord::Active {
            name,
            cloud,
            vm,
            image,
            admission,
            gateway_id,
            state,
            created_at_ms,
            ..
        } = Self::latest_active(&history)?
        else {
            unreachable!()
        };

        let updated = CloudBoxRecord::Active {
            box_id: box_id.clone(),
            name,
            cloud,
            vm,
            image,
            admission,
            gateway_id,
            url: new_url,
            state,
            created_at_ms,
            updated_at_ms: Self::now_ms(),
        };
        let summary = updated
            .as_summary()
            .expect("Active record always projects to summary");
        if let Err(save_error) = self.store.append(box_id, updated) {
            let _ = self.audit.record(CloudBoxAuditEvent {
                box_id: box_id.clone(),
                cloud: Some(cloud),
                before: Some(state),
                after: Some(state),
                action: "endpoint_update_receipt_failed",
                cause: save_error.to_string(),
                initiator,
            });
            return Err(save_error);
        }
        self.audit.record(CloudBoxAuditEvent {
            box_id: box_id.clone(),
            cloud: Some(cloud),
            before: Some(state),
            after: Some(state),
            action: "endpoint_updated",
            cause: "tunnel_rotation".into(),
            initiator,
        })?;
        Ok(summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspaces::domain::{BoatConnection, TemplateRelease};
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex as StdMutex;

    struct FakeSettings(Option<CloudConnection>);
    #[async_trait]
    impl Settings for FakeSettings {
        async fn read(&self) -> Result<CloudConnection, Failure> {
            self.0.clone().ok_or(Failure::NotConfigured)
        }
        fn generation(&self) -> Result<u64, Failure> {
            Ok(1)
        }
        async fn save(
            &self,
            _generation: u64,
            _connection: CloudConnection,
        ) -> Result<(), Failure> {
            Ok(())
        }
        async fn clear(&self) -> Result<(), Failure> {
            Ok(())
        }
    }

    struct MemoryStore {
        rows: StdMutex<HashMap<String, Vec<CloudBoxRecord>>>,
        fail_active_save: AtomicBool,
    }
    impl MemoryStore {
        fn new() -> Self {
            Self {
                rows: StdMutex::new(HashMap::new()),
                fail_active_save: AtomicBool::new(false),
            }
        }
    }
    impl CloudBoxStore for MemoryStore {
        fn list(&self) -> Result<Vec<Vec<CloudBoxRecord>>, CloudBoxError> {
            Ok(self.rows.lock().unwrap().values().cloned().collect())
        }
        fn read(&self, box_id: &CloudBoxId) -> Result<Vec<CloudBoxRecord>, CloudBoxError> {
            Ok(self
                .rows
                .lock()
                .unwrap()
                .get(box_id.as_str())
                .cloned()
                .unwrap_or_default())
        }
        fn append(&self, box_id: &CloudBoxId, record: CloudBoxRecord) -> Result<(), CloudBoxError> {
            if matches!(record, CloudBoxRecord::Active { .. })
                && self.fail_active_save.load(Ordering::SeqCst)
            {
                return Err(CloudBoxError::ReceiptUnavailable("disk full".into()));
            }
            self.rows
                .lock()
                .unwrap()
                .entry(box_id.as_str().to_owned())
                .or_default()
                .push(record);
            Ok(())
        }
    }

    struct FakeActiveWork(AtomicBool);
    impl ActiveWorkChecker for FakeActiveWork {
        fn has_active_work<'a>(
            &'a self,
            _cloud: Cloud,
            _connection: &'a CloudConnection,
            _vm: &'a str,
            _admission: &'a Admitted,
        ) -> BoxFuture<'a, Result<bool, CloudBoxError>> {
            let busy = self.0.load(Ordering::SeqCst);
            Box::pin(async move { Ok(busy) })
        }
    }

    struct FakeAudit(StdMutex<Vec<CloudBoxAuditEvent>>);
    impl CloudBoxAudit for FakeAudit {
        fn record(&self, event: CloudBoxAuditEvent) -> Result<(), CloudBoxError> {
            self.0.lock().unwrap().push(event);
            Ok(())
        }
    }

    struct FakeProvider {
        wake_url: StdMutex<String>,
        wake_gateway_id: StdMutex<String>,
        deleted: StdMutex<Vec<String>>,
        fail_delete: AtomicBool,
    }
    impl FakeProvider {
        fn new() -> Self {
            Self {
                wake_url: StdMutex::new("wss://box-rotated.example.com".into()),
                wake_gateway_id: StdMutex::new("gw-box-1".into()),
                deleted: StdMutex::new(Vec::new()),
                fail_delete: AtomicBool::new(false),
            }
        }
    }
    impl CloudBoxProvider for FakeProvider {
        fn create_box<'a>(
            &'a self,
            _connection: &'a CloudConnection,
            _box_id: &'a CloudBoxId,
        ) -> BoxFuture<'a, Result<ProvisionedCloudBox, CloudBoxError>> {
            Box::pin(async move {
                Ok(ProvisionedCloudBox {
                    vm: "vm-box-1".into(),
                    image: "nessa-headless:v1".into(),
                    admission: Admitted::Agent37(
                        TemplateRelease::new("v1".into(), Some(1), None).unwrap(),
                    ),
                    endpoint: CloudBoxEndpoint {
                        gateway_id: "gw-box-1".into(),
                        url: CloudBoxUrl::new("wss://box-1.example.com").unwrap(),
                    },
                    credential: Zeroizing::new("secret-enroll-token".into()),
                })
            })
        }

        fn wake_box<'a>(
            &'a self,
            _connection: &'a CloudConnection,
            _vm: &'a str,
            _admission: &'a Admitted,
        ) -> BoxFuture<'a, Result<CloudBoxEndpoint, CloudBoxError>> {
            let url = self.wake_url.lock().unwrap().clone();
            let gw = self.wake_gateway_id.lock().unwrap().clone();
            Box::pin(async move {
                Ok(CloudBoxEndpoint {
                    gateway_id: gw,
                    url: CloudBoxUrl::new(url).unwrap(),
                })
            })
        }

        fn stop_box<'a>(
            &'a self,
            _connection: &'a CloudConnection,
            _vm: &'a str,
        ) -> BoxFuture<'a, Result<(), CloudBoxError>> {
            Box::pin(async { Ok(()) })
        }

        fn delete_box<'a>(
            &'a self,
            _connection: &'a CloudConnection,
            vm: &'a str,
            _admission: &'a Admitted,
        ) -> BoxFuture<'a, Result<(), CloudBoxError>> {
            self.deleted.lock().unwrap().push(vm.to_owned());
            let fail = self.fail_delete.load(Ordering::SeqCst);
            Box::pin(async move {
                if fail {
                    Err(CloudBoxError::ProviderFailed("delete timeout".into()))
                } else {
                    Ok(())
                }
            })
        }
    }

    fn connected_account() -> CloudConnection {
        CloudConnection::Boat(
            BoatConnection::new("key-123".into(), "env".into(), "img".into()).unwrap(),
        )
    }

    #[tokio::test]
    async fn full_lifecycle_create_sleep_guard_wake_rotate_rename_and_delete() {
        let store = Arc::new(MemoryStore::new());
        let provider = Arc::new(FakeProvider::new());
        let active_work = Arc::new(FakeActiveWork(AtomicBool::new(true)));
        let audit = Arc::new(FakeAudit(StdMutex::new(Vec::new())));
        let service = CloudBoxes::new(
            vec![CloudBoxBackend {
                cloud: Cloud::Agent37,
                settings: Arc::new(FakeSettings(Some(connected_account()))),
                provider: provider.clone(),
            }],
            store.clone(),
            active_work.clone(),
            audit.clone(),
        );

        let id = CloudBoxId::parse("11111111-2222-3333-4444-555555555555").unwrap();
        let enrolled = service
            .create(
                id.clone(),
                CloudBoxName::new("Cloud Box").unwrap(),
                Cloud::Agent37,
                "owner",
            )
            .await
            .unwrap();
        assert_eq!(enrolled.summary.state, CloudBoxState::Running);
        assert_eq!(enrolled.credential.as_str(), "secret-enroll-token");

        // G5: active work prevents sleep.
        assert_eq!(
            service.sleep(&id, "owner").await.unwrap_err(),
            CloudBoxError::ActiveWorkInFlight
        );

        // Once idle, sleep succeeds and is idempotent.
        active_work.0.store(false, Ordering::SeqCst);
        let sleeping = service.sleep(&id, "owner").await.unwrap();
        assert_eq!(sleeping.state, CloudBoxState::Sleeping);
        let still_sleeping = service.sleep(&id, "owner").await.unwrap();
        assert_eq!(still_sleeping.state, CloudBoxState::Sleeping);

        // G8: wake refreshes rotated wss:// URL.
        let endpoint = service.wake(&id, "owner").await.unwrap();
        assert_eq!(endpoint.url.as_str(), "wss://box-rotated.example.com/");
        assert_eq!(service.list().unwrap()[0].state, CloudBoxState::Running);

        // Identity mismatch on wake is refused.
        *provider.wake_gateway_id.lock().unwrap() = "gw-imposter".into();
        assert!(matches!(
            service.wake(&id, "owner").await.unwrap_err(),
            CloudBoxError::BootstrapFailed(_)
        ));

        // Rename and delete (G11).
        let renamed = service
            .rename(&id, CloudBoxName::new("Prod Box").unwrap(), "owner")
            .await
            .unwrap();
        assert_eq!(renamed.name.as_str(), "Prod Box");

        service.delete(&id, "owner").await.unwrap();
        assert!(service.list().unwrap().is_empty());
        assert_eq!(provider.deleted.lock().unwrap().as_slice(), ["vm-box-1"]);
    }

    #[tokio::test]
    async fn receipt_save_failure_rolls_back_and_surfaces_cleanup_uncertain_on_rollback_error() {
        let store = Arc::new(MemoryStore::new());
        store.fail_active_save.store(true, Ordering::SeqCst);
        let provider = Arc::new(FakeProvider::new());
        let service = CloudBoxes::new(
            vec![CloudBoxBackend {
                cloud: Cloud::Agent37,
                settings: Arc::new(FakeSettings(Some(connected_account()))),
                provider: provider.clone(),
            }],
            store.clone(),
            Arc::new(FakeActiveWork(AtomicBool::new(false))),
            Arc::new(FakeAudit(StdMutex::new(Vec::new()))),
        );

        let id1 = CloudBoxId::parse("22222222-3333-4444-5555-666666666666").unwrap();
        let err = service
            .create(
                id1,
                CloudBoxName::new("Box").unwrap(),
                Cloud::Agent37,
                "owner",
            )
            .await
            .unwrap_err();
        assert!(matches!(err, CloudBoxError::ReceiptUnavailable(_)));
        assert_eq!(provider.deleted.lock().unwrap().as_slice(), ["vm-box-1"]);

        // When rollback delete ALSO fails, CleanupUncertain is surfaced (M1).
        provider.fail_delete.store(true, Ordering::SeqCst);
        let id2 = CloudBoxId::parse("33333333-4444-5555-6666-777777777777").unwrap();
        let err2 = service
            .create(
                id2,
                CloudBoxName::new("Box 2").unwrap(),
                Cloud::Agent37,
                "owner",
            )
            .await
            .unwrap_err();
        assert!(matches!(err2, CloudBoxError::CleanupUncertain(_)));
    }
}
