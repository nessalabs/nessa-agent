//! Allocation ordering. External effects and storage are injected; retries use
//! the saved VM, and observer cancellation does not cancel accepted provisioning.
use super::RuntimeSettings;
use crate::conversation::domain::{ChannelId, VmLifetimeScope};
use crate::workspaces::domain::{
    AdmissionRejection, Assignment, BoatConnection, CleanupOutcome, EnvironmentIdentity, Failure,
    RefusalDisposition,
};
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
/// The provider retains an actual target even when post-create admission fails.
pub(crate) struct WorkspaceReceipt {
    pub(crate) vm: String,
    pub(crate) admission: EnvironmentIdentity,
    pub(crate) refusal: Option<AdmissionRejection>,
    pub(crate) disposition: RefusalDisposition,
}
#[async_trait]
pub(crate) trait Provider: Send + Sync {
    async fn validate(&self, connection: &BoatConnection) -> Result<(), Failure>;
    async fn create(
        &self,
        connection: &BoatConnection,
        thread: &str,
    ) -> Result<WorkspaceReceipt, Failure>;
    async fn connect(
        &self,
        connection: &BoatConnection,
        thread: &str,
        vm: &str,
        admission: &EnvironmentIdentity,
    ) -> Result<Arc<dyn WorkspaceRuntime>, Failure>;
    /// Best effort rollback of a received creation receipt that could not be saved.
    /// Acceptance alone cannot be returned as confirmed cleanup.
    async fn remove(&self, connection: &BoatConnection, vm: &str) -> Result<(), Failure>;
}

/// Recovery evidence is independent of assignment publication and UI observers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RecoveryEvent {
    AdmissionRefused {
        thread: String,
        vm: String,
        admission: EnvironmentIdentity,
        rejection: AdmissionRejection,
        disposition: RefusalDisposition,
    },
    ReceiptSaveFailed {
        thread: String,
        vm: String,
        disposition: RefusalDisposition,
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
    pub(crate) runtime_settings: Arc<dyn RuntimeSettings>,
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
        runtime_settings: Arc<dyn RuntimeSettings>,
    ) -> Self {
        Self {
            settings,
            runtime_settings,
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
        channel: Option<ChannelId>,
    ) -> Result<Arc<dyn WorkspaceRuntime>, Failure> {
        let this = self.clone();
        // The admitted task owns allocation through receipt persistence, even if
        // its initiating conversation/open waiter disappears.
        tokio::spawn(async move { this.allocate(thread, provision, channel).await })
            .await
            .map_err(|_| Failure::ProvisioningUncertain)?
    }
    async fn allocate(
        &self,
        thread: ConversationId,
        provision: bool,
        channel: Option<ChannelId>,
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
        let mut connection = self.settings.read().await?;
        let scope = channel.map_or(VmLifetimeScope::Global, VmLifetimeScope::Channel);
        let lifetimes = self.runtime_settings.read(&scope).await?;
        if lifetimes.scope() != &scope {
            return Err(Failure::Storage);
        }
        connection.auto_stop = lifetimes.effective();
        let vm = match self.assignments.read(&id).await? {
            Some(Assignment::Assigned {
                thread,
                vm,
                image,
                admission,
            }) if thread == id => {
                // Existing work retains its original image and environment. A
                // changed connection token may access it, but cannot relocate it.
                let connection = BoatConnection {
                    image,
                    environment: admission.name().into(),
                    ..connection
                };
                return self
                    .connect(
                        &connection,
                        &id,
                        &vm,
                        &admission,
                        RefusalDisposition::RetainTarget,
                    )
                    .await;
            }
            Some(Assignment::Refused {
                vm,
                rejection,
                disposition,
                ..
            }) => {
                return Err(Failure::RefusedWorkspace {
                    vm,
                    rejection,
                    disposition,
                })
            }
            Some(Assignment::Unverified { vm, .. }) => {
                return Err(Failure::UnverifiedWorkspace { vm })
            }
            Some(_) => return Err(Failure::ProvisioningUncertain),
            None if !provision => return Err(Failure::Missing),
            None => {
                self.assignments
                    .save(Assignment::Provisioning { thread: id.clone() })
                    .await?;
                let created = self.provider.create(&connection, &id).await?;
                let vm = created.vm;
                if let Some(rejection) = created.refusal {
                    return self
                        .refuse(
                            &connection,
                            &id,
                            &vm,
                            &created.admission,
                            rejection,
                            created.disposition,
                        )
                        .await;
                }
                let receipt = Assignment::Assigned {
                    thread: id.clone(),
                    vm: vm.clone(),
                    image: connection.image.clone(),
                    admission: created.admission.clone(),
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
                            disposition: created.disposition,
                        })
                        .is_err();
                    let (cleanup, cleanup_audit_failed) = self
                        .cleanup(&connection, &id, &vm, created.disposition)
                        .await;
                    return Err(Failure::ReceiptRecovery {
                        vm,
                        receipt_audit_failed,
                        cleanup,
                        cleanup_audit_failed,
                    });
                }
                (vm, created.admission, created.disposition)
            }
        };
        self.connect(&connection, &id, &vm.0, &vm.1, vm.2).await
    }
    async fn connect(
        &self,
        connection: &BoatConnection,
        thread: &str,
        vm: &str,
        admission: &EnvironmentIdentity,
        disposition: RefusalDisposition,
    ) -> Result<Arc<dyn WorkspaceRuntime>, Failure> {
        match self
            .provider
            .connect(connection, thread, vm, admission)
            .await
        {
            Err(Failure::AdmissionRejected(rejection)) => {
                self.refuse(connection, thread, vm, admission, rejection, disposition)
                    .await
            }
            result => result,
        }
    }
    async fn refuse(
        &self,
        connection: &BoatConnection,
        thread: &str,
        vm: &str,
        admission: &EnvironmentIdentity,
        rejection: AdmissionRejection,
        disposition: RefusalDisposition,
    ) -> Result<Arc<dyn WorkspaceRuntime>, Failure> {
        let refusal = Assignment::Refused {
            thread: thread.into(),
            vm: vm.into(),
            image: connection.image.clone(),
            admission: admission.clone(),
            rejection: rejection.clone(),
            disposition,
        };
        let receipt_failed = AssertUnwindSafe(self.assignments.save(refusal))
            .catch_unwind()
            .await
            .unwrap_or(Err(Failure::Storage))
            .is_err();
        let audit_failed = self
            .record_recovery(&RecoveryEvent::AdmissionRefused {
                thread: thread.into(),
                vm: vm.into(),
                admission: admission.clone(),
                rejection: rejection.clone(),
                disposition,
            })
            .is_err();
        let (cleanup, cleanup_audit_failed) =
            self.cleanup(connection, thread, vm, disposition).await;
        Err(Failure::AdmissionRecovery {
            vm: vm.into(),
            rejection: rejection.clone(),
            receipt_failed,
            audit_failed,
            cleanup,
            cleanup_audit_failed,
        })
    }
    async fn cleanup(
        &self,
        connection: &BoatConnection,
        thread: &str,
        vm: &str,
        disposition: RefusalDisposition,
    ) -> (CleanupOutcome, bool) {
        // Never delete an existing/replayed workspace or a target named by an
        // unrecognized acknowledgement. Only a pending new allocation rolls back.
        match disposition {
            RefusalDisposition::RetainTarget => (CleanupOutcome::Retained, false),
            RefusalDisposition::RollbackNewVm => {
                let confirmed = AssertUnwindSafe(self.provider.remove(connection, vm))
                    .catch_unwind()
                    .await
                    .unwrap_or(Err(Failure::CleanupUncertain))
                    .is_ok();
                let audit_failed = self
                    .record_recovery(&RecoveryEvent::RollbackObserved {
                        thread: thread.into(),
                        vm: vm.into(),
                        confirmed,
                    })
                    .is_err();
                (
                    if confirmed {
                        CleanupOutcome::Confirmed
                    } else {
                        CleanupOutcome::Unconfirmed
                    },
                    audit_failed,
                )
            }
        }
    }
}
