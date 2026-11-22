//! Boat VMs through the workspace SDK. Its machines make, admit, wake, stop
//! and roll back each thread's VM, and its connection carries everything
//! Nessa runs in one. Nessa keeps only its own record of which VM is whose
//! (the assignments) and what it runs there.
//!
//! ```text
//! validate: machines.prepare
//! create:   machines.create (key nessa-thread-<id>)  ─► receipt
//! connect:  machines.ready ─► attach ─► connection (+ relay socket)
//!           ─► config.json ─► runtime
//! stop:     machines.stop      remove: machines.roll_back
//! ```
use crate::workspaces::{
    application::{Provider, RestCause, WorkspaceReceipt, WorkspaceRuntime},
    domain::{
        AdmissionRefusal, AdmissionRejection, Admitted, BoatAccount, CloudAccount,
        EnvironmentIdentity, Failure, ObservedEnvironment, RefusalDisposition, VmSpec,
    },
};
use async_trait::async_trait;
use std::{collections::BTreeMap, io::Write, sync::Arc, time::Duration};
use workspace_sdk::{
    boat_client::{Api, ClientConfig},
    ports::SocketForward,
    providers::{
        boat_machines::{
            self as machines, BoatMachines, Budget, Created, Disposition, IMachineEvents,
            MachineEvent, MachineFailure, MachineSpec, StopCause,
        },
        SandboxAttachOptions, SandboxProvider, SupervisorDeployment,
    },
    AttachRequest, WorkspaceId,
};

/// How long making a thread's VM ready may take: waiting out a stop still
/// in progress, resuming it and waiting for it, with both checks. The rest
/// of [`CONNECT_BUDGET`] attaches the workspace, opens its connection and
/// writes its configuration.
const READY_BUDGET: Duration = Duration::from_secs(85);
/// The whole workspace connect budget (`agent-startup-budgets.json`,
/// 120 s), less a margin for the caller's own records.
const CONNECT_BUDGET: Duration = Duration::from_secs(115);
/// Boat's size for every thread's VM.
const MACHINE_TYPE: &str = "small";
/// Where each thread's workspace is made inside its VM.
const WORKSPACES: &str = "/home/user/.nessa/workspaces";
/// The workspace supervisor the image carries.
const VM_SUPERVISOR: &str = "/opt/nessa/bin/workspace-supervisor";
/// Writing the configuration is one short command.
const CONFIG_WRITE: Duration = Duration::from_secs(30);

pub struct BoatProvider {
    pub api_base: String,
    pub relay_socket: std::path::PathBuf,
    pub relay_program: std::path::PathBuf,
}
impl BoatProvider {
    fn machines(&self, account: &BoatAccount) -> Result<BoatMachines, Failure> {
        let token =
            std::str::from_utf8(account.token.as_slice()).map_err(|_| Failure::InvalidInput)?;
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut config = ClientConfig::new(token)
            .map_err(|_| Failure::InvalidInput)?
            .with_base_path(&self.api_base)
            .map_err(|_| Failure::InvalidInput)?
            .with_connect_timeout(Duration::from_secs(10))
            .with_request_timeout(Duration::from_secs(45));
        config.max_response_bytes = 4 * 1024 * 1024;
        let api = Api::new(config).map_err(|_| Failure::Provider)?;
        Ok(BoatMachines::for_account(api, Arc::new(MachineLog))
            .map_err(|_| Failure::Provider)?
            .with_budget(Budget::new(READY_BUDGET).map_err(|_| Failure::Provider)?))
    }
    /// The thread's workspace in the ready `vm`, and its connection.
    async fn attach(
        &self,
        machines: BoatMachines,
        thread: &str,
        vm: &str,
    ) -> Result<Arc<dyn WorkspaceRuntime>, Failure> {
        let provider = SandboxProvider::from_machines(machines).with_supervisor(
            SupervisorDeployment::Preinstalled {
                executable: VM_SUPERVISOR.into(),
            },
        );
        let request = AttachRequest::new(
            WorkspaceId::new(thread).map_err(|_| Failure::InvalidInput)?,
            &SandboxAttachOptions {
                root: WORKSPACES.into(),
                sandbox_id: vm.to_owned(),
            },
        )
        .map_err(|_| Failure::Provider)?;
        let backend = workspace_sdk::IProvider::attach(&provider, request)
            .await
            .map_err(|_| Failure::Provider)?;
        let root = backend.descriptor().root;
        let forward =
            SocketForward::new(self.relay_socket.clone()).map_err(|_| Failure::InvalidInput)?;
        let link = backend
            .connections()
            .ok_or(Failure::Provider)?
            .connection(Some(forward))
            .await
            .map_err(|_| Failure::Provider)?;
        write_config(link.as_ref(), &root, super::ssh::BOAT_DATA_DIR).await?;
        let access = super::ssh::VmLink::new(
            link,
            self.relay_socket.clone(),
            self.relay_program.clone(),
            root.clone(),
            vm,
            super::ssh::BOAT_DATA_DIR,
        )?;
        Ok(Arc::new(super::runtime::BoatRuntime {
            access,
            backend,
            cloud: crate::workspaces::domain::Cloud::Boat,
            workspace_root: root,
        }))
    }
}

/// The VM made from `vm` for `thread`, or, without a thread, the spec an
/// existing VM is checked and woken with: variables are only sent when one
/// is made.
fn spec(vm: &VmSpec, thread: Option<&str>) -> Result<MachineSpec, Failure> {
    let variables = thread
        .map(|thread| BTreeMap::from([("NESSA_THREAD_ID".to_owned(), thread.to_owned())]))
        .unwrap_or_default();
    MachineSpec::new(
        vm.image.clone(),
        vm.environment.clone(),
        MACHINE_TYPE,
        vm.auto_stop.seconds(),
        variables,
    )
    .map_err(|_| Failure::InvalidInput)
}

#[async_trait]
impl Provider for BoatProvider {
    async fn validate(&self, account: &CloudAccount, vm: &VmSpec) -> Result<(), Failure> {
        let account = boat(account)?;
        // Nessa admits only its own environment: a name it would refuse on
        // every creation is refused before anything is made for it.
        nessa_environment(&vm.environment)?;
        let spec = spec(vm, None)?;
        self.machines(account)?
            .prepare(&spec)
            .await
            .map_err(failure)
    }
    async fn create(
        &self,
        account: &CloudAccount,
        vm: &VmSpec,
        thread: &str,
    ) -> Result<WorkspaceReceipt, Failure> {
        let account = boat(account)?;
        nessa_environment(&vm.environment)?;
        let spec = spec(vm, Some(thread))?;
        let machines = self.machines(account)?;
        let created = machines
            .create(&spec, &format!("nessa-thread-{thread}"))
            .await
            .map_err(failure)?;
        // A new VM still wiping what its image carried is left refused
        // here: the workspace decides whether to stop it, and records it.
        let refusal = created.refusal().map(rejection_from);
        Ok(WorkspaceReceipt {
            vm: created.vm().into(),
            admission: Admitted::Boat(identity_from(created.admission())?),
            refusal,
            disposition: match created.disposition() {
                Disposition::RollbackNewVm => RefusalDisposition::RollbackNewVm,
                Disposition::RetainTarget => RefusalDisposition::RetainTarget,
            },
        })
    }
    async fn connect(
        &self,
        account: &CloudAccount,
        made: &VmSpec,
        thread: &str,
        vm: &str,
        admission: &Admitted,
    ) -> Result<Arc<dyn WorkspaceRuntime>, Failure> {
        let account = boat(account)?;
        let admission = boat_admission(admission)?;
        let deadline = crate::timer::clock::now() + CONNECT_BUDGET;
        let machines = self.machines(account)?;
        machines
            .ready(&spec(made, None)?, vm, &identity_to(admission)?)
            .await
            .map_err(failure)?;
        // What follows the VM being ready is bounded by what is left; a
        // ready VM left unconnected is stopped by its next rest or its
        // lifetime.
        tokio::time::timeout_at(deadline, self.attach(machines, thread, vm))
            .await
            .map_err(|_| Failure::Provider)?
    }
    async fn stop(
        &self,
        account: &CloudAccount,
        vm: &str,
        cause: RestCause,
    ) -> Result<(), Failure> {
        let account = boat(account)?;
        // Boat archives it: its disk is kept as a snapshot, nothing runs or
        // is billed, and `connect` resumes it.
        let cause = match cause {
            RestCause::Idle => StopCause::Idle,
            RestCause::Closed => StopCause::Closed,
            RestCause::Wiping => StopCause::NotAdmitted,
        };
        self.machines(account)?
            .stop(vm, cause)
            .await
            .map_err(failure)
    }
    async fn remove(
        &self,
        account: &CloudAccount,
        vm: &str,
        admission: &Admitted,
    ) -> Result<(), Failure> {
        let account = boat(account)?;
        let admission = boat_admission(admission)?;
        // Only a VM this creation made is ever removed; the workspace's
        // cleanup asks only for one. Acceptance alone is not confirmed
        // erasure, so this always ends uncertain.
        let created = Created::restore(
            vm.to_owned(),
            identity_to(admission)?,
            Disposition::RollbackNewVm,
        )
        .map_err(failure)?;
        self.machines(account)?
            .roll_back(&created)
            .await
            .map_err(failure)
    }
}

/// The Boat account a connection to Boat carries.
fn boat(account: &CloudAccount) -> Result<&BoatAccount, Failure> {
    match account {
        CloudAccount::Boat(account) => Ok(account),
        CloudAccount::Agent37(_) => Err(Failure::InvalidInput),
    }
}
/// What a Boat VM was admitted under.
fn boat_admission(admission: &Admitted) -> Result<&EnvironmentIdentity, Failure> {
    match admission {
        Admitted::Boat(identity) => Ok(identity),
        Admitted::Agent37(_) => Err(Failure::InvalidInput),
    }
}

/// Nessa's VMs are made only in the environment its domain admits.
fn nessa_environment(name: &str) -> Result<(), Failure> {
    if EnvironmentIdentity::admits(name) {
        Ok(())
    } else {
        Err(Failure::EnvironmentUnavailable)
    }
}

/// What every VM's `nessa` is configured with, as data: the agents the
/// image carries and how each is started. Only the workspace is the
/// thread's own, added by [`vm_config`].
const VM_CONFIG_TEMPLATE: &str = include_str!("vm_config.json");

/// The VM's `config.json` for a thread whose workspace is `root`.
pub fn vm_config(root: &str) -> Result<serde_json::Value, Failure> {
    let mut config: serde_json::Value =
        serde_json::from_str(VM_CONFIG_TEMPLATE).map_err(|_| Failure::Provider)?;
    config
        .pointer_mut("/agents")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or(Failure::Provider)?
        .insert("workspace".into(), root.into());
    Ok(config)
}

/// The VM's `nessa` configuration, `config.json` in its `data_dir`: its
/// agents run in the thread's workspace `root`. Written through the connection's standard input, so
/// nothing of it is in a command line, and only its account can read it.
pub(super) async fn write_config(
    link: &dyn workspace_sdk::ports::IConnection,
    root: &str,
    data_dir: &str,
) -> Result<(), Failure> {
    let config = vm_config(root)?;
    let mut input = tempfile::tempfile().map_err(|_| Failure::Storage)?;
    input
        .write_all(config.to_string().as_bytes())
        .and_then(|()| std::io::Seek::rewind(&mut input))
        .map_err(|_| Failure::Storage)?;
    let written = link
        .run(
            &format!(
                "umask 077 && mkdir -p {data_dir} && chmod 700 {data_dir} \
                 && cat > {data_dir}/config.json.new && chmod 600 {data_dir}/config.json.new \
                 && mv -f {data_dir}/config.json.new {data_dir}/config.json"
            ),
            Some(input),
            CONFIG_WRITE,
        )
        .await
        .map_err(|_| Failure::Provider)?;
    if written.end != workspace_sdk::ports::ProcessEnd::Exited(0) {
        // The VM's own words: logged a line at a time, each escaped.
        crate::infrastructure::ssh_environment::log_stderr(
            "boat configuration",
            std::io::Cursor::new(written.stderr.into_bytes()),
        );
        return Err(Failure::Provider);
    }
    Ok(())
}

/// What the SDK's machines tell Nessa's logs. Stops Nessa asked for are
/// recorded by its own recovery audit; one the SDK made itself, putting
/// back a VM it woke and could not use, is logged here.
struct MachineLog;
impl IMachineEvents for MachineLog {
    fn record(&self, event: MachineEvent) {
        match event {
            MachineEvent::ReceiptRejected(diagnostic) => tracing::warn!(
                target: "nessa_server::workspaces::admission",
                stage = diagnostic.stage.name(),
                scope = diagnostic.scope,
                field = diagnostic.field,
                problem = diagnostic.problem,
                observed_type = diagnostic.observed_type,
                "Boat receipt rejected"
            ),
            MachineEvent::CredentialWipePending { startup_phase } => tracing::warn!(
                target: "nessa_server::workspaces::admission",
                startup_phase,
                "Boat VM still wiping inherited credentials"
            ),
            MachineEvent::StopAnswered {
                vm, cause, failure, ..
            } if !matches!(cause, StopCause::Idle | StopCause::Closed) => tracing::warn!(
                target: "nessa_server::workspaces::admission",
                vm,
                cause = cause.as_str(),
                failure,
                "Boat VM stopped after it could not be used"
            ),
            _ => {}
        }
    }
}

fn failure(failure: MachineFailure) -> Failure {
    match failure {
        MachineFailure::InvalidInput => Failure::InvalidInput,
        MachineFailure::Missing => Failure::Missing,
        MachineFailure::EnvironmentUnavailable => Failure::EnvironmentUnavailable,
        MachineFailure::AdmissionRejected(rejection) => {
            Failure::AdmissionRejected(rejection_from(&rejection))
        }
        MachineFailure::ProvisioningUncertain => Failure::ProvisioningUncertain,
        MachineFailure::CleanupUncertain => Failure::CleanupUncertain,
        MachineFailure::Provider => Failure::Provider,
    }
}

fn rejection_from(rejection: &machines::AdmissionRejection) -> AdmissionRejection {
    use machines::AdmissionRefusal as Sdk;
    let cause = match rejection.cause() {
        Sdk::MissingEnvironment => AdmissionRefusal::MissingEnvironment,
        Sdk::WrongEnvironment => AdmissionRefusal::WrongEnvironment,
        Sdk::MissingVersion => AdmissionRefusal::MissingVersion,
        Sdk::WrongVersion => AdmissionRefusal::WrongVersion,
        Sdk::EnvironmentChanged => AdmissionRefusal::EnvironmentChanged,
        Sdk::EnvironmentUnavailable => AdmissionRefusal::EnvironmentUnavailable,
        Sdk::InvalidReceipt => AdmissionRefusal::InvalidReceipt,
        Sdk::ReceiptUnavailable => AdmissionRefusal::ReceiptUnavailable,
        Sdk::LifetimeNotEnforced => AdmissionRefusal::LifetimeNotEnforced,
        Sdk::WipePending => AdmissionRefusal::WipePending,
    };
    AdmissionRejection::new(
        cause,
        rejection.observed().map(|observed| ObservedEnvironment {
            name: observed.name.clone(),
            version: observed.version,
        }),
    )
}

fn identity_from(admitted: &machines::EnvironmentIdentity) -> Result<EnvironmentIdentity, Failure> {
    EnvironmentIdentity::new(
        admitted.id().into(),
        admitted.name().into(),
        admitted.version_id().into(),
        admitted.version(),
    )
}

fn identity_to(admitted: &EnvironmentIdentity) -> Result<machines::EnvironmentIdentity, Failure> {
    machines::EnvironmentIdentity::new(
        admitted.id().into(),
        admitted.name().into(),
        admitted.version_id().into(),
        admitted.version(),
    )
    .map_err(|_| Failure::InvalidInput)
}
