//! Allocation ordering. External effects and storage are injected; retries use
//! the saved VM, and observer cancellation does not cancel accepted provisioning.
use super::RuntimeSettings;
pub(crate) use crate::conversation::domain::RestCause;
use crate::conversation::domain::{ChannelId, SettingsScope};
use crate::workspaces::domain::{
    AdmissionRefusal, AdmissionRejection, Assignment, BoatAccount, BoatConnection, CleanupOutcome,
    EnvironmentIdentity, Failure, OpenFailure, RefusalDisposition, VmSpec,
};
use async_trait::async_trait;
use futures_util::FutureExt;
use nessa_protocol::conversation::domain::ConversationId;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};
use workspace_sdk::{NamedUse, Uses};

pub(crate) struct WorkspaceStream {
    pub(crate) files: Option<Arc<dyn crate::conversation::application::ArtifactChannels>>,
    pub(crate) mcp_relay:
        Option<Arc<dyn nessa_sdk::application::agent_execution::providers::HarnessMcpRelay>>,
    pub(crate) input: Box<dyn AsyncWrite + Send + Unpin>,
    pub(crate) output: Box<dyn AsyncRead + Send + Unpin>,
    pub(crate) owner: Box<dyn Send>,
}
/// Which copy of `nessa` in the VM serves its leases.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ServedCopy {
    /// The one its image was made with.
    Image,
    /// One installed since, started by this command
    /// (`env_serve::install::serve_command`).
    Installed(String),
}
#[async_trait]
pub(crate) trait WorkspaceRuntime: Send + Sync {
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
    /// Check a connection about to be saved: its account, and that a VM
    /// made from `spec` could be.
    async fn validate(&self, account: &BoatAccount, spec: &VmSpec) -> Result<(), Failure>;
    async fn create(
        &self,
        account: &BoatAccount,
        spec: &VmSpec,
        thread: &str,
    ) -> Result<WorkspaceReceipt, Failure>;
    async fn connect(
        &self,
        account: &BoatAccount,
        spec: &VmSpec,
        thread: &str,
        vm: &str,
        admission: &EnvironmentIdentity,
    ) -> Result<Arc<dyn WorkspaceRuntime>, Failure>;
    /// Best effort rollback of a received creation receipt that could not be saved.
    /// Acceptance alone cannot be returned as confirmed cleanup.
    /// `admission` is what the VM was admitted under when it was made.
    async fn remove(
        &self,
        account: &BoatAccount,
        vm: &str,
        admission: &EnvironmentIdentity,
    ) -> Result<(), Failure>;
    /// Stop `vm`, keeping its disk: it costs nothing while stopped, and
    /// [`Provider::connect`] resumes it.
    async fn stop(&self, account: &BoatAccount, vm: &str, cause: RestCause) -> Result<(), Failure>;
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
pub(crate) trait RecoveryAudit: Send + Sync {
    fn record(&self, event: &RecoveryEvent) -> Result<(), Failure>;
}

/// What [`Workspaces::rest`] did: whether Boat accepted a stop of the VM
/// (it archives after), and whether a
/// record of that stop could not be kept. A failed record never holds the
/// stop back, and the stop's own answer never hides a failed record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Rested {
    pub(crate) stopped: Result<bool, Failure>,
    pub(crate) audit_failed: bool,
}

#[derive(Clone)]
pub(crate) struct Workspaces {
    pub(crate) settings: Arc<dyn Settings>,
    pub(crate) runtime_settings: Arc<dyn RuntimeSettings>,
    /// Counts each change of the Claude sign-in, for whoever must
    /// serve VMs again after one (`claude_sign_in_changed`).
    claude_sign_in_changes: tokio::sync::watch::Sender<u64>,
    provider: Arc<dyn Provider>,
    assignments: Arc<dyn Assignments>,
    recovery: Arc<dyn RecoveryAudit>,
    /// Who uses each thread's VM, by name, and the turn that orders
    /// opening it against resting it.
    uses: Arc<Uses<String>>,
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
            claude_sign_in_changes: tokio::sync::watch::Sender::new(0),
            image: None,
            provider,
            assignments,
            recovery,
            uses: Arc::default(),
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
    /// Forget the Boat connection. The Claude sign-in stays in Saved
    /// logins: every other machine is handed it too. It is removed only
    /// from Settings › Saved logins.
    pub(crate) async fn disconnect(&self) -> Result<(), Failure> {
        self.settings.clear().await
    }
    /// How many times the Claude sign-in was said to have changed.
    #[cfg(test)]
    pub(crate) fn claude_sign_in_changes(&self) -> u64 {
        *self.claude_sign_in_changes.borrow()
    }
    /// Told each time the Claude sign-in is saved or cleared.
    pub(crate) fn claude_sign_in_changed_watch(&self) -> tokio::sync::watch::Receiver<u64> {
        self.claude_sign_in_changes.subscribe()
    }
    /// Say the Claude sign-in changed: also when Settings › Saved logins
    /// saved or removed it.
    pub(crate) fn claude_sign_in_changed(&self) {
        self.claude_sign_in_changes
            .send_modify(|changes| *changes += 1);
    }
    pub(crate) async fn save(&self, connection: BoatConnection) -> Result<(), Failure> {
        let generation = self.settings.generation()?;
        let spec = connection.new_vm(None, Default::default());
        self.provider.validate(&connection.account, &spec).await?;
        self.settings.save(generation, connection).await
    }
    pub(crate) async fn open(
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
    pub(crate) async fn rest(&self, thread: ConversationId, cause: RestCause) -> Rested {
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
        let (account, vm) = match self.resting(&id).await {
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
        let rested = self.stop_recorded(&account, id.clone(), vm, cause).await;
        if matches!(rested.stopped, Ok(true)) {
            turn.rested();
        }
        rested
    }
    /// Stops thread `id`'s `vm` for `cause`, recording the stop before it
    /// is asked and once it answered.
    async fn stop_recorded(
        &self,
        account: &BoatAccount,
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
        let stopped = AssertUnwindSafe(self.provider.stop(account, &vm, cause))
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
    /// The account and VM a rest of thread `id` stops, if it has one.
    async fn resting(&self, id: &str) -> Result<Option<(BoatAccount, String)>, Failure> {
        let Some(Assignment::Assigned {
            thread: assigned,
            vm,
            ..
        }) = self.assignments.read(id).await?
        else {
            return Ok(None);
        };
        if assigned != id {
            return Ok(None);
        }
        Ok(Some((self.settings.read().await?.account, vm)))
    }
    /// The names of what uses `thread`'s VM now.
    pub(crate) fn uses(&self, thread: &str) -> Vec<String> {
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
        let connection = self.settings.read().await?;
        let scope = channel.map_or(SettingsScope::Global, SettingsScope::Channel);
        let lifetimes = self.runtime_settings.read(&scope).await?;
        if lifetimes.scope() != &scope {
            return Err(Failure::Storage);
        }
        let auto_stop = lifetimes.effective();
        let account = &connection.account;
        let (spec, vm, admission, disposition) = match self.assignments.read(&id).await? {
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
            Some(_) => return Err(Failure::ProvisioningUncertain),
            None if !provision => return Err(Failure::Missing),
            None => {
                // A new VM is made from the image this build pins, whatever
                // image was pinned when the account was connected: that one
                // may speak a lease protocol this build no longer does.
                let spec = connection.new_vm(self.image.as_deref(), auto_stop);
                self.assignments
                    .save(Assignment::Provisioning { thread: id.clone() })
                    .await?;
                let created = self.provider.create(account, &spec, &id).await?;
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
                        .stop_recorded(account, id.clone(), vm.clone(), RestCause::Wiping)
                        .await;
                    if rested.stopped == Ok(true) && !rested.audit_failed {
                        refusal = None;
                    }
                }
                if let Some(rejection) = refusal {
                    return self
                        .refuse(
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
                        .cleanup(account, &id, &vm, &created.admission, created.disposition)
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
        };
        self.connect(account, &spec, &id, &vm, &admission, disposition)
            .await
    }
    async fn connect(
        &self,
        account: &BoatAccount,
        spec: &VmSpec,
        thread: &str,
        vm: &str,
        admission: &EnvironmentIdentity,
        disposition: RefusalDisposition,
    ) -> Result<Arc<dyn WorkspaceRuntime>, Failure> {
        match self
            .provider
            .connect(account, spec, thread, vm, admission)
            .await
        {
            Err(Failure::AdmissionRejected(rejection)) => {
                self.refuse(account, spec, thread, vm, admission, rejection, disposition)
                    .await
            }
            result => result,
        }
    }
    #[allow(clippy::too_many_arguments)]
    async fn refuse(
        &self,
        account: &BoatAccount,
        spec: &VmSpec,
        thread: &str,
        vm: &str,
        admission: &EnvironmentIdentity,
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
        let (cleanup, cleanup_audit_failed) = self
            .cleanup(account, thread, vm, admission, disposition)
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
        account: &BoatAccount,
        thread: &str,
        vm: &str,
        admission: &EnvironmentIdentity,
        disposition: RefusalDisposition,
    ) -> (CleanupOutcome, bool) {
        // Never delete an existing/replayed workspace or a target named by an
        // unrecognized acknowledgement. Only a pending new allocation rolls back.
        match disposition {
            RefusalDisposition::RetainTarget => (CleanupOutcome::Retained, false),
            RefusalDisposition::RollbackNewVm => {
                let confirmed = AssertUnwindSafe(self.provider.remove(account, vm, admission))
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
