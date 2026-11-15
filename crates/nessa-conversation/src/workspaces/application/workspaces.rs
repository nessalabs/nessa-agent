//! Allocation ordering. External effects and storage are injected; retries use
//! the saved VM, and observer cancellation does not cancel accepted provisioning.
use super::RuntimeSettings;
pub use crate::domain::RestCause;
use crate::domain::{ChannelId, SettingsScope};
use crate::workspaces::domain::{
    AdmissionRefusal, AdmissionRejection, Admitted, Assignment, BoatConnection, CleanupOutcome,
    Cloud, CloudAccount, CloudConnection, Failure, OpenFailure, RefusalDisposition, VmSpec,
};
use async_trait::async_trait;
use futures_util::FutureExt;
use nessa_protocol::conversation::domain::ConversationId;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};
use workspace_sdk::{NamedUse, Uses};

pub struct WorkspaceStream {
    pub files: Option<Arc<dyn crate::application::ArtifactChannels>>,
    pub mcp_relay:
        Option<Arc<dyn nessa_sdk::application::agent_execution::providers::HarnessMcpRelay>>,
    pub input: Box<dyn AsyncWrite + Send + Unpin>,
    pub output: Box<dyn AsyncRead + Send + Unpin>,
    pub owner: Box<dyn Send>,
}
/// Which copy of `nessa` in the VM serves its leases.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServedCopy {
    /// The one its image was made with.
    Image,
    /// One installed since, started by this command
    /// (`env_serve::install::serve_command`).
    Installed(String),
}
#[async_trait]
pub trait WorkspaceRuntime: Send + Sync {
    async fn serve(&self, copy: ServedCopy) -> Result<WorkspaceStream, Failure>;
    /// One command session with stdin kept open until cancellation or disconnect.
    async fn background(&self, _command: String) -> std::io::Result<WorkspaceStream> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "background sessions unavailable",
        ))
    }

    /// The lease protocol the image's own copy says it speaks, empty when it
    /// says none; `None` when the VM could not be asked.
    async fn image_protocol(&self) -> Option<String>;
    /// Run `command` in the VM with `input` as its standard input, and
    /// answer the end of what it printed.
    async fn run(&self, command: String, input: Option<std::fs::File>) -> std::io::Result<String>;
}
#[async_trait]
pub trait Settings: Send + Sync {
    async fn read(&self) -> Result<CloudConnection, Failure>;
    fn generation(&self) -> Result<u64, Failure>;
    async fn save(&self, generation: u64, connection: CloudConnection) -> Result<(), Failure>;
    async fn clear(&self) -> Result<(), Failure>;
}
#[async_trait]
pub trait Assignments: Send + Sync {
    async fn read(&self, thread: &str) -> Result<Option<Assignment>, Failure>;
    async fn save(&self, assignment: Assignment) -> Result<(), Failure>;
}
/// The provider retains an actual target even when post-create admission fails.
pub struct WorkspaceReceipt {
    pub vm: String,
    pub admission: Admitted,
    pub refusal: Option<AdmissionRejection>,
    pub disposition: RefusalDisposition,
}
#[async_trait]
pub trait Provider: Send + Sync {
    /// Check a connection about to be saved: its account, and that a VM
    /// made from `spec` could be.
    async fn validate(&self, account: &CloudAccount, spec: &VmSpec) -> Result<(), Failure>;
    async fn create(
        &self,
        account: &CloudAccount,
        spec: &VmSpec,
        thread: &str,
    ) -> Result<WorkspaceReceipt, Failure>;
    async fn connect(
        &self,
        account: &CloudAccount,
        spec: &VmSpec,
        thread: &str,
        vm: &str,
        admission: &Admitted,
    ) -> Result<Arc<dyn WorkspaceRuntime>, Failure>;
    /// Best effort rollback of a received creation receipt that could not be saved.
    /// Acceptance alone cannot be returned as confirmed cleanup.
    /// `admission` is what the VM was admitted under when it was made.
    async fn remove(
        &self,
        account: &CloudAccount,
        vm: &str,
        admission: &Admitted,
    ) -> Result<(), Failure>;
    /// Stop `vm`, keeping its disk: it costs nothing while stopped, and
    /// [`Provider::connect`] resumes it.
    async fn stop(&self, account: &CloudAccount, vm: &str, cause: RestCause)
        -> Result<(), Failure>;
}

/// What [`Workspaces::open`] hands out: the provider's runtime, holding the
/// thread's VM in use, as an opening, for as long as it is kept. Each
/// stream it serves, a connection or a background command, holds a use of
/// its own until it is dropped. A connection with no lease on it is let go
/// before a rest (`SshEnvironment::let_go`), which ends its use.
struct HeldRuntime {
    runtime: Arc<dyn WorkspaceRuntime>,
    opening: NamedUse,
}
#[async_trait]
impl WorkspaceRuntime for HeldRuntime {
    async fn serve(&self, copy: ServedCopy) -> Result<WorkspaceStream, Failure> {
        let used = self.opening.also("connection");
        let mut stream = self.runtime.serve(copy).await?;
        stream.owner = Box::new((stream.owner, used));
        Ok(stream)
    }
    async fn background(&self, command: String) -> std::io::Result<WorkspaceStream> {
        let used = self.opening.also("background command");
        let mut stream = self.runtime.background(command).await?;
        stream.owner = Box::new((stream.owner, used));
        Ok(stream)
    }
    async fn image_protocol(&self) -> Option<String> {
        self.runtime.image_protocol().await
    }
    async fn run(&self, command: String, input: Option<std::fs::File>) -> std::io::Result<String> {
        self.runtime.run(command, input).await
    }
}

/// Recovery evidence is independent of assignment publication and UI observers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecoveryEvent {
    AdmissionRefused {
        thread: String,
        vm: String,
        admission: Admitted,
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
    /// A thread's VM is about to be stopped, its disk kept, because its
    /// agent's lease ended for `cause`. Recorded before Boat is asked.
    StopRequested {
        thread: String,
        vm: String,
        cause: RestCause,
    },
    /// A thread's VM could not be opened: `cause` is what can be done about
    /// it, `failure` the kind of failure it came from.
    OpenRefused {
        thread: String,
        cause: OpenFailure,
        failure: &'static str,
    },
    /// What Boat answered that stop: accepted, the VM archives after it
    /// (`connect` waits that out); a `failure` (its kind) means it may
    /// still run, until the next rest or its own time limit.
    StopAnswered {
        thread: String,
        vm: String,
        cause: RestCause,
        failure: Option<&'static str>,
    },
}
pub trait RecoveryAudit: Send + Sync {
    fn record(&self, event: &RecoveryEvent) -> Result<(), Failure>;
}

/// What [`Workspaces::rest`] did: whether Boat accepted a stop of the VM
/// (it archives after), and whether a
/// record of that stop could not be kept. A failed record never holds the
/// stop back, and the stop's own answer never hides a failed record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rested {
    pub stopped: Result<bool, Failure>,
    pub audit_failed: bool,
}

/// Which cloud new thread VMs are made on. A thread whose VM exists stays
/// on the cloud it was made on.
#[async_trait]
pub trait CloudChoice: Send + Sync {
    async fn read(&self) -> Result<Cloud, Failure>;
    async fn save(&self, cloud: Cloud) -> Result<(), Failure>;
}
/// The one cloud a gateway that offers no choice makes VMs on.
struct OnlyCloud(Cloud);
#[async_trait]
impl CloudChoice for OnlyCloud {
    async fn read(&self) -> Result<Cloud, Failure> {
        Ok(self.0)
    }
    async fn save(&self, cloud: Cloud) -> Result<(), Failure> {
        if cloud == self.0 {
            Ok(())
        } else {
            Err(Failure::InvalidInput)
        }
    }
}

/// One cloud's parts: its saved connection, its VMs, and the receipts of
/// the VMs made on it. Each cloud keeps its own receipts, so a thread's VM
/// is found on the cloud it was made on.
#[derive(Clone)]
struct CloudParts {
    cloud: Cloud,
    settings: Arc<dyn Settings>,
    provider: Arc<dyn Provider>,
    assignments: Arc<dyn Assignments>,
}

#[derive(Clone)]
pub struct Workspaces {
    pub runtime_settings: Arc<dyn RuntimeSettings>,
    /// Counts each change of the Claude sign-in, for whoever must
    /// serve VMs again after one (`claude_sign_in_changed`).
    claude_sign_in_changes: tokio::sync::watch::Sender<u64>,
    clouds: Arc<Vec<CloudParts>>,
    choice: Arc<dyn CloudChoice>,
    recovery: Arc<dyn RecoveryAudit>,
    /// Who uses each thread's VM, by name, and the turn that orders
    /// opening it against resting it.
    uses: Arc<Uses<String>>,
    image: Option<String>,
}
impl Workspaces {
    pub fn new(
        settings: Arc<dyn Settings>,
        provider: Arc<dyn Provider>,
        assignments: Arc<dyn Assignments>,
        recovery: Arc<dyn RecoveryAudit>,
        runtime_settings: Arc<dyn RuntimeSettings>,
    ) -> Self {
        Self {
            runtime_settings,
            claude_sign_in_changes: tokio::sync::watch::Sender::new(0),
            image: None,
            clouds: Arc::new(vec![CloudParts {
                cloud: Cloud::Boat,
                settings,
                provider,
                assignments,
            }]),
            choice: Arc::new(OnlyCloud(Cloud::Boat)),
            recovery,
            uses: Arc::default(),
        }
    }
    /// The same workspaces, also on `cloud`, with new VMs made on whichever
    /// cloud `choice` names.
    pub fn with_cloud(
        mut self,
        cloud: Cloud,
        settings: Arc<dyn Settings>,
        provider: Arc<dyn Provider>,
        assignments: Arc<dyn Assignments>,
        choice: Arc<dyn CloudChoice>,
    ) -> Self {
        let mut clouds = (*self.clouds).clone();
        clouds.retain(|parts| parts.cloud != cloud);
        clouds.push(CloudParts {
            cloud,
            settings,
            provider,
            assignments,
        });
        self.clouds = Arc::new(clouds);
        self.choice = choice;
        self
    }
    fn parts(&self, cloud: Cloud) -> Result<&CloudParts, Failure> {
        self.clouds
            .iter()
            .find(|parts| parts.cloud == cloud)
            .ok_or(Failure::NotConfigured)
    }
    /// `cloud`'s saved connection, if this gateway offers that cloud.
    pub fn settings(&self, cloud: Cloud) -> Option<Arc<dyn Settings>> {
        self.parts(cloud).ok().map(|parts| parts.settings.clone())
    }
    /// The cloud new thread VMs are made on.
    pub async fn cloud(&self) -> Result<Cloud, Failure> {
        self.choice.read().await
    }
    /// Make new thread VMs on `cloud`, which must be connected.
    pub async fn choose(&self, cloud: Cloud) -> Result<(), Failure> {
        self.parts(cloud)?.settings.read().await?;
        self.choice.save(cloud).await
    }
    /// Whether the cloud new VMs are made on is connected.
    pub async fn connected(&self) -> bool {
        match self
            .cloud()
            .await
            .and_then(|cloud| self.parts(cloud).cloned())
        {
            Ok(parts) => parts.settings.read().await.is_ok(),
            Err(_) => false,
        }
    }
    /// The cloud thread `id`'s VM, or its creation, was recorded on, and
    /// that record. A thread recorded on two clouds is refused: which is
    /// its VM cannot be told.
    async fn recorded(&self, id: &str) -> Result<Option<(CloudParts, Assignment)>, Failure> {
        let mut found = None;
        for parts in self.clouds.iter() {
            if let Some(assignment) = parts.assignments.read(id).await? {
                if found.is_some() {
                    return Err(Failure::Storage);
                }
                found = Some((parts.clone(), assignment));
            }
        }
        Ok(found)
    }
    fn record_recovery(&self, event: &RecoveryEvent) -> Result<(), Failure> {
        catch_unwind(AssertUnwindSafe(|| self.recovery.record(event)))
            .unwrap_or(Err(Failure::Storage))
    }
    pub fn with_image(mut self, image: Option<&str>) -> Self {
        self.image = image.map(str::to_owned);
        self
    }
    pub async fn save_token(&self, token: String) -> Result<(), Failure> {
        let image = self.image.clone().ok_or(Failure::WorkerUnavailable)?;
        self.save(CloudConnection::Boat(BoatConnection::new(
            token,
            "nessa".into(),
            image,
        )?))
        .await
    }
    /// Forget `cloud`'s connection. The Claude sign-in stays in Saved
    /// logins: every other machine is handed it too. It is removed only
    /// from Settings › Saved logins.
    /// New VMs then go to Boat again if they were going to `cloud`.
    pub async fn disconnect(&self, cloud: Cloud) -> Result<(), Failure> {
        self.parts(cloud)?.settings.clear().await?;
        if cloud != Cloud::Boat && self.cloud().await? == cloud {
            self.choice.save(Cloud::Boat).await?;
        }
        Ok(())
    }
    /// How many times the Claude sign-in was said to have changed.
    #[cfg(any(test, feature = "test-support"))]
    pub fn claude_sign_in_changes(&self) -> u64 {
        *self.claude_sign_in_changes.borrow()
    }
    /// Told each time the Claude sign-in is saved or cleared.
    pub fn claude_sign_in_changed_watch(&self) -> tokio::sync::watch::Receiver<u64> {
        self.claude_sign_in_changes.subscribe()
    }
    /// Say the Claude sign-in changed: also when Settings › Saved logins
    /// saved or removed it.
    pub fn claude_sign_in_changed(&self) {
        self.claude_sign_in_changes
            .send_modify(|changes| *changes += 1);
    }
    pub async fn save(&self, connection: CloudConnection) -> Result<(), Failure> {
        let parts = self.parts(connection.cloud())?;
        let generation = parts.settings.generation()?;
        let spec = connection.new_vm(None, Default::default());
        parts
            .provider
            .validate(&connection.account(), &spec)
            .await?;
        parts.settings.save(generation, connection).await
    }
    pub async fn open(
        &self,
        thread: ConversationId,
        provision: bool,
        channel: Option<ChannelId>,
    ) -> Result<Arc<dyn WorkspaceRuntime>, Failure> {
        let this = self.clone();
        // The admitted task owns allocation through receipt persistence, and
        // the record of a failed opening, even if its initiating
        // conversation/open waiter disappears.
        tokio::spawn(async move {
            let id = thread.to_string();
            let opened = AssertUnwindSafe(this.allocate(thread, provision, channel))
                .catch_unwind()
                .await
                .unwrap_or(Err(Failure::ProvisioningUncertain));
            this.record_opening(id, opened)
        })
        .await
        .unwrap_or(Err(Failure::ProvisioningUncertain))
    }
    /// Record why an opening failed, beside the receipt's own records. The
    /// failure stays the answer; one whose record was not saved says so too.
    fn record_opening(
        &self,
        thread: String,
        opened: Result<Arc<dyn WorkspaceRuntime>, Failure>,
    ) -> Result<Arc<dyn WorkspaceRuntime>, Failure> {
        let error = match opened {
            Ok(runtime) => return Ok(runtime),
            Err(error) => error,
        };
        let recorded = self.record_recovery(&RecoveryEvent::OpenRefused {
            thread,
            cause: error.cause(),
            failure: error.kind(),
        });
        Err(match recorded {
            Ok(()) => error,
            Err(_) => Failure::OpeningUnrecorded(Box::new(error)),
        })
    }
    /// Stop `thread`'s VM, keeping its disk, unless something still uses
    /// it: a runtime [`Workspaces::open`] handed out, or a stream it served.
    /// Ordered with opening under the thread's lock, so an open that comes
    /// meanwhile either finds the VM stopped, and resumes it, or holds it in
    /// use and keeps it running. Each stop asked of Boat is recorded, with
    /// `cause`, before it is asked and once it answered.
    pub async fn rest(&self, thread: ConversationId, cause: RestCause) -> Rested {
        let this = self.clone();
        // Like an opening, a stop once asked is carried through by its own
        // task, whoever stops waiting for it.
        tokio::spawn(async move { this.rest_owned(thread, cause).await })
            .await
            .unwrap_or(Rested {
                stopped: Err(Failure::Provider),
                audit_failed: false,
            })
    }
    async fn rest_owned(&self, thread: ConversationId, cause: RestCause) -> Rested {
        let id = thread.to_string();
        let turn = self.uses.turn(&id).await;
        if turn.in_use() {
            return Rested {
                stopped: Ok(false),
                audit_failed: false,
            };
        }
        let (parts, account, vm) = match self.resting(&id).await {
            Ok(Some(found)) => found,
            Ok(None) => {
                // No VM to stop: nothing to keep for the thread either.
                turn.rested();
                return Rested {
                    stopped: Ok(false),
                    audit_failed: false,
                };
            }
            Err(error) => {
                return Rested {
                    stopped: Err(error),
                    audit_failed: false,
                }
            }
        };
        let rested = self
            .stop_recorded(&parts, &account, id.clone(), vm, cause)
            .await;
        if matches!(rested.stopped, Ok(true)) {
            turn.rested();
        }
        rested
    }
    /// Stops thread `id`'s `vm` for `cause`, recording the stop before it
    /// is asked and once it answered.
    async fn stop_recorded(
        &self,
        parts: &CloudParts,
        account: &CloudAccount,
        id: String,
        vm: String,
        cause: RestCause,
    ) -> Rested {
        let requested_failed = self
            .record_recovery(&RecoveryEvent::StopRequested {
                thread: id.clone(),
                vm: vm.clone(),
                cause,
            })
            .is_err();
        let stopped = AssertUnwindSafe(parts.provider.stop(account, &vm, cause))
            .catch_unwind()
            .await
            .unwrap_or(Err(Failure::Provider));
        let observed_failed = self
            .record_recovery(&RecoveryEvent::StopAnswered {
                thread: id,
                vm,
                cause,
                failure: stopped.as_ref().err().map(Failure::kind),
            })
            .is_err();
        Rested {
            stopped: stopped.map(|()| true),
            audit_failed: requested_failed || observed_failed,
        }
    }
    /// The cloud, account and VM a rest of thread `id` stops, if it has one.
    async fn resting(
        &self,
        id: &str,
    ) -> Result<Option<(CloudParts, CloudAccount, String)>, Failure> {
        let Some((
            parts,
            Assignment::Assigned {
                thread: assigned,
                vm,
                ..
            },
        )) = self.recorded(id).await?
        else {
            return Ok(None);
        };
        if assigned != id {
            return Ok(None);
        }
        let account = parts.settings.read().await?.account();
        Ok(Some((parts, account, vm)))
    }
    /// The names of what uses `thread`'s VM now.
    pub fn uses(&self, thread: &str) -> Vec<String> {
        self.uses.names(&thread.to_owned())
    }
    async fn allocate(
        &self,
        thread: ConversationId,
        provision: bool,
        channel: Option<ChannelId>,
    ) -> Result<Arc<dyn WorkspaceRuntime>, Failure> {
        let id = thread.to_string();
        let turn = self.uses.turn(&id).await;
        // Held from before the VM is reached until the runtime is gone: a
        // rest waiting for the turn sees it.
        let opening = turn.hold("opening");
        let runtime = self.allocate_held(&id, channel, provision).await?;
        Ok(Arc::new(HeldRuntime { runtime, opening }))
    }
    async fn allocate_held(
        &self,
        id: &str,
        channel: Option<ChannelId>,
        provision: bool,
    ) -> Result<Arc<dyn WorkspaceRuntime>, Failure> {
        let id = id.to_owned();
        let recorded = self.recorded(&id).await?;
        // A recorded thread stays on its cloud; a new one goes to the cloud
        // chosen now.
        let parts = match &recorded {
            Some((parts, _)) => parts.clone(),
            None => self.parts(self.cloud().await?)?.clone(),
        };
        let parts = &parts;
        let connection = parts.settings.read().await?;
        let scope = channel.map_or(SettingsScope::Global, SettingsScope::Channel);
        let lifetimes = self.runtime_settings.read(&scope).await?;
        if lifetimes.scope() != &scope {
            return Err(Failure::Storage);
        }
        let auto_stop = lifetimes.effective();
        let account = &connection.account();
        let (spec, vm, admission, disposition) = match recorded.map(|(_, assignment)| assignment) {
            Some(Assignment::Assigned {
                thread,
                vm,
                image,
                admission,
            }) if thread == id => {
                // Existing work retains its original image and environment. A
                // changed connection token may access it, but cannot relocate it.
                let spec = VmSpec::of_receipt(image, &admission, auto_stop);
                return self
                    .connect(
                        parts,
                        account,
                        &spec,
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
            // Accounting never makes or finishes a VM.
            Some(Assignment::Provisioning { .. }) if !provision => {
                return Err(Failure::ProvisioningUncertain)
            }
            None if !provision => return Err(Failure::Missing),
            recorded @ (None | Some(Assignment::Provisioning { .. })) => {
                // A new VM is made from the image this build pins, whatever
                // image was pinned when the account was connected: that one
                // may speak a lease protocol this build no longer does.
                let spec = connection.new_vm(self.image.as_deref(), auto_stop);
                // The intent is saved before Boat is asked. An intent already
                // saved is an earlier creation whose answer was lost: Boat
                // is asked again with the same key (the thread's), and
                // answers with the VM it made then while it remembers that
                // key.
                if recorded.is_none() {
                    parts
                        .assignments
                        .save(Assignment::Provisioning { thread: id.clone() })
                        .await?;
                }
                let created = parts.provider.create(account, &spec, &id).await?;
                let vm = created.vm;
                let mut refusal = created.refusal;
                // A new VM still wiping is clean once it restarts, and the
                // next wake is that restart: it is stopped, on the record,
                // and `connect` checks it in full once ready. A stop not
                // accepted, or not recorded, leaves it refused, so it is
                // rolled back. A VM Boat replayed may be in use elsewhere,
                // so its refusal always stands.
                if created.disposition == RefusalDisposition::RollbackNewVm
                    && refusal.as_ref().map(AdmissionRejection::cause)
                        == Some(AdmissionRefusal::WipePending)
                {
                    let rested = self
                        .stop_recorded(parts, account, id.clone(), vm.clone(), RestCause::Wiping)
                        .await;
                    if rested.stopped == Ok(true) && !rested.audit_failed {
                        refusal = None;
                    }
                }
                if let Some(rejection) = refusal {
                    return self
                        .refuse(
                            parts,
                            account,
                            &spec,
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
                    image: spec.image.clone(),
                    admission: created.admission.clone(),
                };
                let publication = AssertUnwindSafe(async { parts.assignments.save(receipt).await })
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
                        .cleanup(
                            parts,
                            account,
                            &id,
                            &vm,
                            &created.admission,
                            created.disposition,
                        )
                        .await;
                    return Err(Failure::ReceiptRecovery {
                        vm,
                        receipt_audit_failed,
                        cleanup,
                        cleanup_audit_failed,
                    });
                }
                (spec, vm, created.admission, created.disposition)
            }
            // A receipt naming another thread is never this one's.
            Some(Assignment::Assigned { .. }) => return Err(Failure::ProvisioningUncertain),
        };
        self.connect(parts, account, &spec, &id, &vm, &admission, disposition)
            .await
    }
    #[allow(clippy::too_many_arguments)]
    async fn connect(
        &self,
        parts: &CloudParts,
        account: &CloudAccount,
        spec: &VmSpec,
        thread: &str,
        vm: &str,
        admission: &Admitted,
        disposition: RefusalDisposition,
    ) -> Result<Arc<dyn WorkspaceRuntime>, Failure> {
        match parts
            .provider
            .connect(account, spec, thread, vm, admission)
            .await
        {
            Err(Failure::AdmissionRejected(rejection)) => {
                self.refuse(
                    parts,
                    account,
                    spec,
                    thread,
                    vm,
                    admission,
                    rejection,
                    disposition,
                )
                .await
            }
            result => result,
        }
    }
    #[allow(clippy::too_many_arguments)]
    async fn refuse(
        &self,
        parts: &CloudParts,
        account: &CloudAccount,
        spec: &VmSpec,
        thread: &str,
        vm: &str,
        admission: &Admitted,
        rejection: AdmissionRejection,
        disposition: RefusalDisposition,
    ) -> Result<Arc<dyn WorkspaceRuntime>, Failure> {
        let refusal = Assignment::Refused {
            thread: thread.into(),
            vm: vm.into(),
            image: spec.image.clone(),
            admission: admission.clone(),
            rejection: rejection.clone(),
            disposition,
        };
        let receipt_failed = AssertUnwindSafe(parts.assignments.save(refusal))
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
        let (cleanup, cleanup_audit_failed) = self
            .cleanup(parts, account, thread, vm, admission, disposition)
            .await;
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
        parts: &CloudParts,
        account: &CloudAccount,
        thread: &str,
        vm: &str,
        admission: &Admitted,
        disposition: RefusalDisposition,
    ) -> (CleanupOutcome, bool) {
        // Never delete an existing/replayed workspace or a target named by an
        // unrecognized acknowledgement. Only a pending new allocation rolls back.
        match disposition {
            RefusalDisposition::RetainTarget => (CleanupOutcome::Retained, false),
            RefusalDisposition::RollbackNewVm => {
                let confirmed = AssertUnwindSafe(parts.provider.remove(account, vm, admission))
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
