//! Allocation ordering. External effects and storage are injected; retries use
//! the saved VM, and observer cancellation does not cancel accepted provisioning.
use super::domain::{Assignment, BoatConnection, Failure};
use async_trait::async_trait;
use futures_util::FutureExt;
use nessa_protocol::conversation::domain::ConversationId;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::{
    collections::BTreeMap,
    sync::{Arc, Weak},
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::Mutex,
};

pub(crate) struct WorkspaceStream {
    pub(crate) files: Option<Arc<dyn crate::conversation::application::ArtifactChannels>>,
    pub(crate) mcp_relay:
        Option<Arc<dyn nessa_sdk::application::agent_execution::providers::HarnessMcpRelay>>,
    pub(crate) input: Box<dyn AsyncWrite + Send + Unpin>,
    pub(crate) output: Box<dyn AsyncRead + Send + Unpin>,
    pub(crate) owner: Box<dyn Send>,
}
#[async_trait]
pub(crate) trait WorkspaceRuntime: Send + Sync {
    async fn serve(&self) -> Result<WorkspaceStream, Failure>;
}
#[async_trait]
pub(crate) trait Settings: Send + Sync {
    async fn read(&self) -> Result<BoatConnection, Failure>;
    fn generation(&self) -> Result<u64, Failure>;
    async fn save(&self, generation: u64, connection: BoatConnection) -> Result<(), Failure>;
    async fn clear(&self) -> Result<(), Failure>;
}
#[async_trait]
pub(crate) trait Assignments: Send + Sync {
    async fn read(&self, thread: &str) -> Result<Option<Assignment>, Failure>;
    async fn save(&self, assignment: Assignment) -> Result<(), Failure>;
}
#[async_trait]
pub(crate) trait Provider: Send + Sync {
    async fn validate(&self, connection: &BoatConnection) -> Result<(), Failure>;
    async fn create(&self, connection: &BoatConnection, thread: &str) -> Result<String, Failure>;
    async fn connect(
        &self,
        connection: &BoatConnection,
        thread: &str,
        vm: &str,
    ) -> Result<Arc<dyn WorkspaceRuntime>, Failure>;
    /// Best effort rollback of a received creation receipt that could not be saved.
    /// Acceptance alone cannot be returned as confirmed cleanup.
    async fn remove(&self, connection: &BoatConnection, vm: &str) -> Result<(), Failure>;
}

/// Recovery evidence is independent of assignment publication and UI observers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RecoveryEvent {
    ReceiptSaveFailed {
        thread: String,
        vm: String,
    },
    RollbackObserved {
        thread: String,
        vm: String,
        confirmed: bool,
    },
}
pub(crate) trait RecoveryAudit: Send + Sync {
    fn record(&self, event: &RecoveryEvent) -> Result<(), Failure>;
}

#[derive(Clone)]
pub(crate) struct Workspaces {
    pub(crate) settings: Arc<dyn Settings>,
    provider: Arc<dyn Provider>,
    assignments: Arc<dyn Assignments>,
    recovery: Arc<dyn RecoveryAudit>,
    locks: Arc<Mutex<BTreeMap<String, Weak<Mutex<()>>>>>,
    image: Option<String>,
}
impl Workspaces {
    pub(crate) fn new(
        settings: Arc<dyn Settings>,
        provider: Arc<dyn Provider>,
        assignments: Arc<dyn Assignments>,
        recovery: Arc<dyn RecoveryAudit>,
    ) -> Self {
        Self {
            settings,
            image: None,
            provider,
            assignments,
            recovery,
            locks: Arc::default(),
        }
    }
    fn record_recovery(&self, event: &RecoveryEvent) -> Result<(), Failure> {
        catch_unwind(AssertUnwindSafe(|| self.recovery.record(event)))
            .unwrap_or(Err(Failure::Storage))
    }
    pub(crate) fn with_image(mut self, image: Option<&str>) -> Self {
        self.image = image.map(str::to_owned);
        self
    }
    pub(crate) async fn save_token(&self, token: String) -> Result<(), Failure> {
        let image = self.image.clone().ok_or(Failure::WorkerUnavailable)?;
        self.save(BoatConnection::new(token, "nessa".into(), image)?)
            .await
    }
    pub(crate) async fn save(&self, connection: BoatConnection) -> Result<(), Failure> {
        let generation = self.settings.generation()?;
        self.provider.validate(&connection).await?;
        self.settings.save(generation, connection).await
    }
    pub(crate) async fn open(
        &self,
        thread: ConversationId,
        provision: bool,
    ) -> Result<Arc<dyn WorkspaceRuntime>, Failure> {
        let this = self.clone();
        // The admitted task owns allocation through receipt persistence, even if
        // its initiating conversation/open waiter disappears.
        tokio::spawn(async move { this.allocate(thread, provision).await })
            .await
            .map_err(|_| Failure::ProvisioningUncertain)?
    }
    async fn allocate(
        &self,
        thread: ConversationId,
        provision: bool,
    ) -> Result<Arc<dyn WorkspaceRuntime>, Failure> {
        let id = thread.to_string();
        let lock = {
            let mut locks = self.locks.lock().await;
            locks.retain(|_, lock| lock.strong_count() != 0);
            let lock = locks
                .get(&id)
                .and_then(Weak::upgrade)
                .unwrap_or_else(|| Arc::new(Mutex::new(())));
            locks.insert(id.clone(), Arc::downgrade(&lock));
            lock
        };
        let _guard = lock.lock().await;
        let connection = self.settings.read().await?;
        let vm = match self.assignments.read(&id).await? {
            Some(Assignment::Assigned {
                thread,
                vm,
                image,
                environment,
            }) if thread == id => {
                // Existing work retains its original image and environment. A
                // changed connection token may access it, but cannot relocate it.
                let connection = BoatConnection {
                    image,
                    environment,
                    ..connection
                };
                return self.provider.connect(&connection, &id, &vm).await;
            }
            Some(_) => return Err(Failure::ProvisioningUncertain),
            None if !provision => return Err(Failure::Missing),
            None => {
                self.assignments
                    .save(Assignment::Provisioning { thread: id.clone() })
                    .await?;
                let vm = self.provider.create(&connection, &id).await?;
                let receipt = Assignment::Assigned {
                    thread: id.clone(),
                    vm: vm.clone(),
                    image: connection.image.clone(),
                    environment: connection.environment.clone(),
                };
                let publication = AssertUnwindSafe(async { self.assignments.save(receipt).await })
                    .catch_unwind()
                    .await
                    .unwrap_or(Err(Failure::Storage));
                if publication.is_err() {
                    // Preserve the external target before attempting rollback.
                    // Audit failure never prevents cleanup; neither a dropped
                    // observer nor a failing port can discard its outcome.
                    let receipt_audit_failed = self
                        .record_recovery(&RecoveryEvent::ReceiptSaveFailed {
                            thread: id.clone(),
                            vm: vm.clone(),
                        })
                        .is_err();
                    let cleanup =
                        AssertUnwindSafe(async { self.provider.remove(&connection, &vm).await })
                            .catch_unwind()
                            .await
                            .unwrap_or(Err(Failure::CleanupUncertain));
                    let cleanup_unconfirmed = cleanup.is_err();
                    let cleanup_audit_failed = self
                        .record_recovery(&RecoveryEvent::RollbackObserved {
                            thread: id.clone(),
                            vm: vm.clone(),
                            confirmed: !cleanup_unconfirmed,
                        })
                        .is_err();
                    return Err(Failure::ReceiptRecovery {
                        vm,
                        receipt_audit_failed,
                        cleanup_unconfirmed,
                        cleanup_audit_failed,
                    });
                }
                vm
            }
        };
        self.provider.connect(&connection, &id, &vm).await
    }
}
