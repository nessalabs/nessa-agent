//! Agent37 VMs through the workspace SDK. Its machines make, admit, wake,
//! stop and delete each thread's VM, and its connection, over Agent37's
//! authenticated SSH tunnel, carries everything Nessa runs in one. Nessa
//! keeps only its own record of which VM is whose (the assignments) and what
//! it runs there.
//!
//! ```text
//! validate: register the install's SSH key ─► machines.prepare
//! create:   machines.create (key nessa-thread-<id>) ─► receipt
//! connect:  machines.ready ─► attach ─► connection (+ relay socket)
//!           ─► config.json ─► runtime
//! stop:     machines.stop      remove: machines.delete
//! ```
use crate::workspaces::{
    application::{Provider, RestCause, WorkspaceReceipt, WorkspaceRuntime},
    domain::{
        AdmissionRefusal, AdmissionRejection, Admitted, Agent37Account, CloudAccount, Failure,
        RefusalDisposition, TemplateRelease, VmSpec,
    },
};
use async_trait::async_trait;
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use workspace_sdk::{
    infrastructure::FileStateRepository,
    ports::SocketForward,
    providers::{
        agent37::{
            self as sdk, Agent37AttachOptions, Agent37Config, Agent37Failure, Agent37Identity,
            Agent37Machines, ApiKey, MachineSpec,
        },
        SupervisorDeployment,
    },
    AttachRequest, ProviderId, WorkspaceId, WorkspaceSdk,
};

/// How long making a thread's VM ready may take: a start or wake, and the
/// wait for it to run. The rest of [`CONNECT_BUDGET`] attaches the
/// workspace, opens its connection and writes its configuration.
const READY_BUDGET: Duration = Duration::from_secs(85);
/// The whole workspace connect budget, as for Boat.
const CONNECT_BUDGET: Duration = Duration::from_secs(115);
/// Where each thread's workspace is made inside its VM.
const WORKSPACES: &str = "/home/node/.nessa/workspaces";
/// Nessa's data directory in an Agent37 VM: under the image user's home,
/// which is the instance's persistent disk.
const DATA_DIR: &str = "/home/node/.nessa";
/// The workspace supervisor the template carries.
const VM_SUPERVISOR: &str = "/opt/nessa/bin/workspace-supervisor";
/// The idle time Agent37 sleeps a VM after when its lifetime is unlimited:
/// the longest it allows. A VM it sleeps keeps its disk and wakes on use.
const LONGEST_IDLE: Duration = Duration::from_secs(86_400);

pub struct Agent37Provider {
    pub relay_socket: std::path::PathBuf,
    pub relay_program: std::path::PathBuf,
    /// Another API and tunnel, for tests; Agent37's own when `None`.
    pub endpoints: Option<(String, String)>,
}
impl Agent37Provider {
    fn config(&self, account: &Agent37Account) -> Result<Agent37Config, Failure> {
        let key = std::str::from_utf8(account.key.as_slice()).map_err(|_| Failure::InvalidInput)?;
        let config = Agent37Config::new(ApiKey::new(key).map_err(|_| Failure::InvalidInput)?);
        match &self.endpoints {
            None => Ok(config),
            Some((api, tunnel)) => config
                .with_api_base(api)
                .and_then(|config| config.with_tunnel(tunnel))
                .map_err(|_| Failure::InvalidInput),
        }
    }
    fn machines(&self, account: &Agent37Account) -> Result<Agent37Machines, Failure> {
        Ok(Agent37Machines::new(self.config(account)?)
            .map_err(|_| Failure::Provider)?
            .with_budget(READY_BUDGET))
    }
    /// The thread's workspace in the ready `vm`, and its connection.
    async fn attach(
        &self,
        account: &Agent37Account,
        thread: &str,
        vm: &str,
    ) -> Result<Arc<dyn WorkspaceRuntime>, Failure> {
        // An attachment for this connection only: Nessa's assignments are
        // the VM's record, so the SDK's is made here and dropped with it.
        let records = tempfile::tempdir().map_err(|_| Failure::Storage)?;
        let provider = sdk::Agent37Provider::new(
            self.config(account)?,
            identity(account)?,
            SupervisorDeployment::Preinstalled {
                executable: VM_SUPERVISOR.into(),
            },
        )
        .map_err(|_| Failure::Provider)?;
        let sdk = WorkspaceSdk::builder(Arc::new(
            FileStateRepository::new(records.path()).map_err(|_| Failure::Storage)?,
        ))
        .provider(Arc::new(provider))
        .map_err(|_| Failure::Provider)?
        .build()
        .map_err(|_| Failure::Provider)?;
        let request = AttachRequest::new(
            WorkspaceId::new(thread).map_err(|_| Failure::InvalidInput)?,
            &Agent37AttachOptions {
                instance_id: vm.to_owned(),
                root: format!("{WORKSPACES}/{thread}"),
            },
        )
        .map_err(|_| Failure::Provider)?;
        let workspace = sdk
            .attach(
                &ProviderId::new("agent37").map_err(|_| Failure::Provider)?,
                request,
            )
            .await
            .map_err(|_| Failure::Provider)?;
        let root = workspace.descriptor().root;
        let forward =
            SocketForward::new(self.relay_socket.clone()).map_err(|_| Failure::InvalidInput)?;
        let link = workspace
            .connection(Some(forward))
            .await
            .map_err(|_| Failure::Provider)?;
        super::boat::write_config(link.as_ref(), &root, DATA_DIR).await?;
        let access = super::ssh::VmLink::new(
            link,
            self.relay_socket.clone(),
            self.relay_program.clone(),
            root,
            vm,
            DATA_DIR,
        )?;
        Ok(Arc::new(super::runtime::BoatRuntime { access }))
    }
}

/// The Agent37 account a connection to Agent37 carries.
fn agent37(account: &CloudAccount) -> Result<&Agent37Account, Failure> {
    match account {
        CloudAccount::Agent37(account) => Ok(account),
        CloudAccount::Boat(_) => Err(Failure::InvalidInput),
    }
}
/// What an Agent37 VM was admitted under.
fn release(admission: &Admitted) -> Result<&TemplateRelease, Failure> {
    match admission {
        Admitted::Agent37(release) => Ok(release),
        Admitted::Boat(_) => Err(Failure::InvalidInput),
    }
}
fn identity(account: &Agent37Account) -> Result<Agent37Identity, Failure> {
    Agent37Identity::new(account.ssh_private.as_slice().to_vec(), &account.ssh_public)
        .map_err(|_| Failure::InvalidInput)
}
fn admission(release: &TemplateRelease) -> Result<sdk::Admission, Failure> {
    sdk::Admission::restore(
        release.template().into(),
        release.revision(),
        release.digest().into(),
    )
    .map_err(|_| Failure::InvalidInput)
}

/// The machines made from `vm` for `thread`, or, without a thread, the spec
/// an existing VM is checked with: variables are only sent when one is made.
fn spec(vm: &VmSpec, thread: Option<&str>) -> Result<MachineSpec, Failure> {
    let variables = thread
        .map(|thread| BTreeMap::from([("NESSA_THREAD_ID".to_owned(), thread.to_owned())]))
        .unwrap_or_default();
    // Agent37 sleeps a VM after a time with nothing moving through it,
    // within its own bounds; the thread's own rest still stops it first.
    let idle = vm
        .auto_stop
        .seconds()
        .map_or(LONGEST_IDLE, |seconds| {
            Duration::from_secs(u64::from(seconds))
        })
        .clamp(*sdk::IDLE.start(), *sdk::IDLE.end());
    MachineSpec::new(vm.image.clone(), idle, variables).map_err(|_| Failure::InvalidInput)
}

#[async_trait]
impl Provider for Agent37Provider {
    async fn validate(&self, account: &CloudAccount, vm: &VmSpec) -> Result<(), Failure> {
        let account = agent37(account)?;
        let machines = self.machines(account)?;
        // One key for the install, registered for the whole workspace;
        // registering it again is accepted.
        machines
            .register(&identity(account)?, "nessa")
            .await
            .map_err(failure)?;
        machines.prepare(&spec(vm, None)?).await.map_err(failure)
    }
    async fn create(
        &self,
        account: &CloudAccount,
        vm: &VmSpec,
        thread: &str,
    ) -> Result<WorkspaceReceipt, Failure> {
        let account = agent37(account)?;
        let created = self
            .machines(account)?
            .create(&spec(vm, Some(thread))?, &format!("nessa-thread-{thread}"))
            .await
            .map_err(failure)?;
        let admitted = created.admission();
        Ok(WorkspaceReceipt {
            vm: created.instance().into(),
            admission: Admitted::Agent37(TemplateRelease::new(
                admitted.template().into(),
                admitted.revision(),
                admitted.digest().into(),
            )?),
            refusal: None,
            // Found under the thread's own key or made now, it is the
            // thread's alone: either may be deleted if refused.
            disposition: RefusalDisposition::RollbackNewVm,
        })
    }
    async fn connect(
        &self,
        account: &CloudAccount,
        _made: &VmSpec,
        thread: &str,
        vm: &str,
        admitted: &Admitted,
    ) -> Result<Arc<dyn WorkspaceRuntime>, Failure> {
        let account = agent37(account)?;
        let deadline = crate::timer::clock::now() + CONNECT_BUDGET;
        self.machines(account)?
            .ready(vm, &admission(release(admitted)?)?)
            .await
            .map_err(failure)?;
        // What follows the VM being ready is bounded by what is left; a
        // ready VM left unconnected sleeps once idle.
        tokio::time::timeout_at(deadline, self.attach(account, thread, vm))
            .await
            .map_err(|_| Failure::Provider)?
    }
    async fn stop(
        &self,
        account: &CloudAccount,
        vm: &str,
        _cause: RestCause,
    ) -> Result<(), Failure> {
        // Agent37 keeps its disk; `connect` starts it again.
        self.machines(agent37(account)?)?
            .stop(vm)
            .await
            .map_err(failure)
    }
    async fn remove(
        &self,
        account: &CloudAccount,
        vm: &str,
        _admission: &Admitted,
    ) -> Result<(), Failure> {
        // Agent37 answers a delete once it is done, and the same for an
        // instance already gone.
        self.machines(agent37(account)?)?
            .delete(vm)
            .await
            .map_err(|_| Failure::CleanupUncertain)
    }
}

fn failure(failure: Agent37Failure) -> Failure {
    match failure {
        Agent37Failure::InvalidInput => Failure::InvalidInput,
        Agent37Failure::Unauthorized | Agent37Failure::Funds => Failure::EnvironmentUnavailable,
        Agent37Failure::Missing => Failure::Missing,
        Agent37Failure::AdmissionRejected(cause) => {
            Failure::AdmissionRejected(AdmissionRejection::new(refusal(cause), None))
        }
        Agent37Failure::ProvisioningUncertain | Agent37Failure::Ambiguous => {
            Failure::ProvisioningUncertain
        }
        Agent37Failure::Timeout | Agent37Failure::Provider => Failure::Provider,
    }
}

fn refusal(cause: sdk::AdmissionRefusal) -> AdmissionRefusal {
    use sdk::AdmissionRefusal as Sdk;
    match cause {
        Sdk::WrongTemplate => AdmissionRefusal::WrongEnvironment,
        Sdk::MissingDigest => AdmissionRefusal::MissingVersion,
        Sdk::ImageChanged => AdmissionRefusal::EnvironmentChanged,
        Sdk::WrongOwner => AdmissionRefusal::InvalidReceipt,
        Sdk::Failed => AdmissionRefusal::EnvironmentUnavailable,
    }
}
