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
        AdmissionRefusal, AdmissionRejection, BoatAccount, EnvironmentIdentity, Failure,
        ObservedEnvironment, RefusalDisposition, VmSpec,
    },
};
use async_trait::async_trait;
use std::{collections::BTreeMap, io::Write, sync::Arc, time::Duration};
use workspace_sdk::{
    boat_client::{Api, ClientConfig},
    infrastructure::FileStateRepository,
    ports::SocketForward,
    providers::{
        boat_machines::{
            self as machines, BoatMachines, Budget, Created, Disposition, IMachineEvents,
            MachineEvent, MachineFailure, MachineSpec, StopCause,
        },
        SandboxAttachOptions, SandboxProvider, SupervisorDeployment,
    },
    AttachRequest, ProviderId, WorkspaceId, WorkspaceSdk,
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
/// The VM's own `nessa` configuration, private to its account.
const VM_CONFIG: &str = "/home/user/.nessa/config.json";
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
        // An attachment for this connection only: Nessa's assignments are
        // the VM's record, so the SDK's is made here and dropped with it.
        let records = tempfile::tempdir().map_err(|_| Failure::Storage)?;
        let provider = SandboxProvider::from_machines(machines).with_supervisor(
            SupervisorDeployment::Preinstalled {
                executable: VM_SUPERVISOR.into(),
            },
        );
        let sdk = WorkspaceSdk::builder(Arc::new(
            FileStateRepository::new(records.path()).map_err(|_| Failure::Storage)?,
        ))
        .provider(Arc::new(provider))
        .map_err(|_| Failure::Provider)?
        .build()
        .map_err(|_| Failure::Provider)?;
        let request = AttachRequest::new(
            WorkspaceId::new(thread).map_err(|_| Failure::InvalidInput)?,
            &SandboxAttachOptions {
                root: WORKSPACES.into(),
                sandbox_id: vm.to_owned(),
            },
        )
        .map_err(|_| Failure::Provider)?;
        let workspace = sdk
            .attach(
                &ProviderId::new("boat").map_err(|_| Failure::Provider)?,
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
        write_config(link.as_ref(), &root).await?;
        let access = super::ssh::VmLink::new(
            link,
            self.relay_socket.clone(),
            self.relay_program.clone(),
            root,
            vm,
        )?;
        Ok(Arc::new(super::runtime::BoatRuntime { access }))
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
    async fn validate(&self, account: &BoatAccount, vm: &VmSpec) -> Result<(), Failure> {
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
        account: &BoatAccount,
        vm: &VmSpec,
        thread: &str,
    ) -> Result<WorkspaceReceipt, Failure> {
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
            admission: identity_from(created.admission())?,
            refusal,
            disposition: match created.disposition() {
                Disposition::RollbackNewVm => RefusalDisposition::RollbackNewVm,
                Disposition::RetainTarget => RefusalDisposition::RetainTarget,
            },
        })
    }
    async fn connect(
        &self,
        account: &BoatAccount,
        made: &VmSpec,
        thread: &str,
        vm: &str,
        admission: &EnvironmentIdentity,
    ) -> Result<Arc<dyn WorkspaceRuntime>, Failure> {
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
    async fn stop(&self, account: &BoatAccount, vm: &str, cause: RestCause) -> Result<(), Failure> {
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
        account: &BoatAccount,
        vm: &str,
        admission: &EnvironmentIdentity,
    ) -> Result<(), Failure> {
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

/// Nessa's VMs are made only in the environment its domain admits.
fn nessa_environment(name: &str) -> Result<(), Failure> {
    if EnvironmentIdentity::admits(name) {
        Ok(())
    } else {
        Err(Failure::EnvironmentUnavailable)
    }
}

/// The VM's `nessa` configuration: its agents run in the thread's
/// workspace `root`. Written through the connection's standard input, so
/// nothing of it is in a command line, and only its account can read it.
async fn write_config(
    link: &dyn workspace_sdk::ports::IConnection,
    root: &str,
) -> Result<(), Failure> {
    let config = serde_json::json!({"envServe":{"commands":true,"artifactsIn":true},"agents":{"catalog":"/opt/nessa/models.json","workspace":root,"selected":"codex","runtimes":{
        "codex":{"command":"/opt/nessa/bin/node","args":["/opt/nessa/harnesses/node_modules/@agentclientprotocol/codex-acp/dist/index.js"],"model":"gpt-5.6-terra","toolsEnabled":true},
        "claude":{"command":"/opt/nessa/bin/node","args":["/opt/nessa/harnesses/node_modules/@agentclientprotocol/claude-agent-acp/dist/index.js"],"model":"claude-sonnet-5","toolsEnabled":true}
    }}});
    let mut input = tempfile::tempfile().map_err(|_| Failure::Storage)?;
    input
        .write_all(config.to_string().as_bytes())
        .and_then(|()| std::io::Seek::rewind(&mut input))
        .map_err(|_| Failure::Storage)?;
    let written = link
        .run(
            &format!(
                "umask 077 && mkdir -p /home/user/.nessa && chmod 700 /home/user/.nessa \
                 && cat > {VM_CONFIG}.new && chmod 600 {VM_CONFIG}.new \
                 && mv -f {VM_CONFIG}.new {VM_CONFIG}"
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
