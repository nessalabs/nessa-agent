//! Test-process composition for the desktop VM background step. No Boat HTTP,
//! SSH account or credential is constructed. Only this ignored test installs
//! the fake provider; released binaries cannot select it.
use crate::conversation::{
    application::{
        ChannelStore, ConversationError, ConversationLimits, Environment, EnvironmentDeclaration,
        EnvironmentFuture, EnvironmentLease, ThreadEnvironments,
    },
    infrastructure::{
        channel_cloud, in_process_environment,
        ssh_environment::{Build, BuildSource, HostInstaller},
    },
};
use crate::workspaces::{
    application::*,
    domain::*,
    infrastructure::{FileAssignments, FileCloudChoice},
};
use crate::{cli::entrypoint::LocalProvisioning, composition::CompositionRoot, core::Launch};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::{
    application::agent_execution::providers::AgentProvider,
    domain::agent_execution::leases::{
        EnvironmentRef, LeaseCleanup, LeaseId, LeaseRefusal, LeaseTerms,
    },
};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io,
    io::Write,
    path::PathBuf,
    process::Stdio,
    sync::{Arc, OnceLock},
    time::Duration,
};
use tokio::process::Command;

struct Fixture {
    root: PathBuf,
    cli: PathBuf,
    idle: Duration,
}
static FIXTURE: OnceLock<Fixture> = OnceLock::new();
fn event(kind: &str, thread: &str) {
    record(serde_json::json!({"kind":kind,"thread":thread}));
}
fn record(event: serde_json::Value) {
    let Some(fixture) = FIXTURE.get() else { return };
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(fixture.root.join("events.jsonl"))
        .unwrap();
    let mut record = serde_json::to_vec(&event).unwrap();
    record.push(b'\n');
    file.write_all(&record).unwrap();
}
fn folder(thread: &str) -> PathBuf {
    FIXTURE.get().unwrap().root.join(thread)
}

#[test]
#[ignore = "long-lived gateway owned by the desktop VM stress driver"]
fn serve() {
    // As bootstrap does: terminals are prepared before any async runtime
    // starts, since pty-runtime forks to stage its helper image.
    crate::terminal::prepare();
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap()
        .block_on(serve_gateway());
}

async fn serve_gateway() {
    let idle = std::env::var("NESSA_VM_STRESS_IDLE_SECONDS")
        .unwrap()
        .parse::<u64>()
        .unwrap();
    assert!((1..=60).contains(&idle));
    let fixture = Fixture {
        idle: Duration::from_secs(idle),
        root: std::env::var_os("NESSA_VM_STRESS_ROOT")
            .expect("fixture directory")
            .into(),
        cli: std::env::var_os("NESSA_VM_STRESS_CLI")
            .expect("real helper binary")
            .into(),
    };
    std::fs::create_dir_all(&fixture.root).unwrap();
    assert!(FIXTURE.set(fixture).is_ok());
    // The driver reads this gateway's log as it reads the real one's, so it
    // logs the same lines under the same filter.
    crate::core::logging::init();
    CompositionRoot::serve(LocalProvisioning::Manual, &Launch::Standalone)
        .await
        .unwrap();
}

pub(super) fn service(default: Arc<Workspaces>) -> Arc<Workspaces> {
    let Some(fixture) = FIXTURE.get() else {
        return default;
    };
    Arc::new(
        Workspaces::new(
            Arc::new(Config),
            Arc::new(FakeProvider(Cloud::Boat)),
            Arc::new(FileAssignments(fixture.root.join("assignments"))),
            Arc::new(Audit),
            default.runtime_settings.clone(),
        )
        // A connected Agent37 beside Boat: the driver picks the cloud for
        // new VMs in Settings, and reads which one made each VM from the
        // events.
        .with_cloud(
            Cloud::Agent37,
            Arc::new(Agent37Config),
            Arc::new(FakeProvider(Cloud::Agent37)),
            Arc::new(FileAssignments(fixture.root.join("agent37-assignments"))),
            Arc::new(FileCloudChoice(fixture.root.join("cloud"))),
        ),
    )
}
pub(super) fn environments(
    default: Arc<dyn ThreadEnvironments>,
    service: Arc<Workspaces>,
    channels: Arc<dyn ChannelStore>,
) -> Arc<dyn ThreadEnvironments> {
    if FIXTURE.get().is_none() {
        return default;
    }
    Arc::new(FakeEnvironments(service, channels))
}
pub(super) fn limits(default: ConversationLimits) -> ConversationLimits {
    FIXTURE.get().map_or(default, |fixture| ConversationLimits {
        workspace_idle: fixture.idle,
        ..default
    })
}

pub(super) fn cli() -> Option<PathBuf> {
    FIXTURE.get().map(|fixture| fixture.cli.clone())
}
pub(super) fn installer(default: HostInstaller) -> HostInstaller {
    let Some(fixture) = FIXTURE.get() else {
        return default;
    };
    default.with_build_source(Arc::new(CliBuild(fixture.cli.clone())))
}

struct CliBuild(PathBuf);
impl BuildSource for CliBuild {
    fn open(&self) -> io::Result<Build> {
        Ok(Build {
            file: File::open(&self.0)?,
            digest: format!("{:x}", Sha256::digest(std::fs::read(&self.0)?)),
        })
    }
}
struct Config;
#[async_trait::async_trait]
impl Settings for Config {
    async fn read(&self) -> Result<CloudConnection, Failure> {
        BoatConnection::new(
            "fixture-token".into(),
            "nessa".into(),
            "fixture-image".into(),
        )
        .map(CloudConnection::Boat)
    }
    fn generation(&self) -> Result<u64, Failure> {
        Ok(1)
    }
    async fn save(&self, _: u64, _: CloudConnection) -> Result<(), Failure> {
        unreachable!()
    }
    async fn clear(&self) -> Result<(), Failure> {
        unreachable!()
    }
}
struct Agent37Config;
#[async_trait::async_trait]
impl Settings for Agent37Config {
    async fn read(&self) -> Result<CloudConnection, Failure> {
        Agent37Connection::new(
            "sk_live_fixture".into(),
            b"fixture-private".to_vec(),
            "ssh-ed25519 AAAAfixture".into(),
            "nessa-runtime@1".into(),
        )
        .map(CloudConnection::Agent37)
    }
    fn generation(&self) -> Result<u64, Failure> {
        Ok(1)
    }
    async fn save(&self, _: u64, _: CloudConnection) -> Result<(), Failure> {
        unreachable!()
    }
    async fn clear(&self) -> Result<(), Failure> {
        unreachable!()
    }
}
struct Audit;
impl RecoveryAudit for Audit {
    fn record(&self, event: &RecoveryEvent) -> Result<(), Failure> {
        // The stress driver reads each failed opening's cause from here, as
        // the gateway's environment audit would keep it.
        if let RecoveryEvent::OpenRefused {
            thread,
            cause,
            failure,
        } = event
        {
            record(serde_json::json!({
                "kind": "open_refused",
                "thread": thread,
                "cause": cause.as_str(),
                "failure": failure,
            }));
        }
        Ok(())
    }
}
/// One cloud's VMs, which are folders beside each other: each says which
/// cloud made it in the events.
struct FakeProvider(Cloud);
#[async_trait::async_trait]
impl Provider for FakeProvider {
    async fn validate(&self, _: &CloudAccount, _: &VmSpec) -> Result<(), Failure> {
        Ok(())
    }
    async fn create(
        &self,
        _: &CloudAccount,
        _: &VmSpec,
        thread: &str,
    ) -> Result<WorkspaceReceipt, Failure> {
        // Like Boat, a creation is keyed by its thread: asking again for a
        // VM that already exists answers with that VM and makes nothing new.
        // Each cloud keeps its own marker, so a thread asked of the wrong
        // cloud is made there anew and the events show it.
        let made = folder(thread).join(format!(".{}-created", self.0.as_str()));
        if made.exists() {
            record(
                serde_json::json!({"kind":"create_replayed","thread":thread,"cloud":self.0.as_str()}),
            );
        } else {
            std::fs::create_dir_all(folder(thread)).map_err(|_| Failure::Storage)?;
            std::fs::write(&made, b"").map_err(|_| Failure::Storage)?;
            record(serde_json::json!({"kind":"created","thread":thread,"cloud":self.0.as_str()}));
        }
        // The driver makes Boat's answer get lost after the VM was made by
        // leaving this file beside the threads' folders.
        let root = &FIXTURE.get().unwrap().root;
        if std::fs::remove_file(root.join(format!("lose-next-create-{thread}"))).is_ok() {
            event("create_lost", thread);
            return Err(Failure::Provider);
        }
        Ok(WorkspaceReceipt {
            vm: thread.into(),
            admission: match self.0 {
                Cloud::Boat => Admitted::Boat(
                    EnvironmentIdentity::new(
                        "11111111-1111-4111-8111-111111111111".into(),
                        "nessa".into(),
                        "22222222-2222-4222-8222-222222222222".into(),
                        2,
                    )
                    .unwrap(),
                ),
                Cloud::Agent37 => Admitted::Agent37(
                    TemplateRelease::new(
                        "nessa-runtime@1".into(),
                        Some(1),
                        Some(format!("sha256:{}", "a".repeat(64))),
                    )
                    .unwrap(),
                ),
            },
            refusal: None,
            disposition: RefusalDisposition::RetainTarget,
        })
    }
    async fn connect(
        &self,
        _: &CloudAccount,
        _: &VmSpec,
        thread: &str,
        _: &str,
        _: &Admitted,
    ) -> Result<Arc<dyn WorkspaceRuntime>, Failure> {
        // The driver makes Boat fail a thread's next opening by leaving
        // this file in its folder.
        if std::fs::remove_file(folder(thread).join("fail-next-open")).is_ok() {
            return Err(Failure::Provider);
        }
        record(serde_json::json!({"kind":"connected","thread":thread,"cloud":self.0.as_str()}));
        Ok(Arc::new(FakeVm(thread.into())))
    }
    async fn remove(&self, _: &CloudAccount, _: &str, _: &Admitted) -> Result<(), Failure> {
        unreachable!()
    }
    async fn stop(&self, _: &CloudAccount, vm: &str, _: RestCause) -> Result<(), Failure> {
        record(serde_json::json!({"kind":"rested","thread":vm,"cloud":self.0.as_str()}));
        Ok(())
    }
}
struct FakeVm(String);
fn shell(thread: &str, program: &str) -> Command {
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", program])
        .env("HOME", folder(thread))
        .current_dir(folder(thread));
    command
}
#[async_trait::async_trait]
impl WorkspaceRuntime for FakeVm {
    async fn serve(&self, _: ServedCopy) -> Result<WorkspaceStream, Failure> {
        unreachable!("the scripted agent uses an in-process test lease")
    }
    async fn image_protocol(&self) -> Option<String> {
        Some(crate::env::LEASE_PROTOCOL.into())
    }
    async fn run(&self, program: String, input: Option<File>) -> io::Result<String> {
        let output = shell(&self.0, &program)
            .stdin(input.map_or_else(Stdio::null, Stdio::from))
            .output()
            .await?;
        if !output.status.success() {
            return Err(io::Error::other(
                String::from_utf8_lossy(&output.stderr).into_owned(),
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
    async fn background(&self, program: String) -> io::Result<WorkspaceStream> {
        event("background", &self.0);
        let mut child = shell(&self.0, &program)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        Ok(WorkspaceStream {
            input: Box::new(child.stdin.take().unwrap()),
            output: Box::new(child.stdout.take().unwrap()),
            owner: Box::new(child),
            files: None,
            mcp_relay: None,
        })
    }
}
struct FakeEnvironments(Arc<Workspaces>, Arc<dyn ChannelStore>);
impl ThreadEnvironments for FakeEnvironments {
    fn environment(&self, id: &ConversationId) -> Result<Arc<dyn Environment>, ConversationError> {
        Ok(Arc::new(FakeLease(
            self.0.clone(),
            id.clone(),
            self.1.clone(),
        )))
    }
    fn available(&self) -> EnvironmentFuture<'_, bool> {
        Box::pin(async { true })
    }
    fn rest<'a>(&'a self, id: &'a ConversationId, cause: RestCause) -> EnvironmentFuture<'a, bool> {
        Box::pin(async move {
            self.0
                .rest(id.clone(), cause)
                .await
                .stopped
                .unwrap_or(false)
        })
    }
}
struct FakeLease(Arc<Workspaces>, ConversationId, Arc<dyn ChannelStore>);
impl Environment for FakeLease {
    fn declaration(&self) -> EnvironmentDeclaration {
        let mut d = in_process_environment().declaration();
        d.environment = EnvironmentRef::Boat;
        d
    }
    fn open<'a>(
        &'a self,
        lease: &'a LeaseId,
        grant: &'a LeaseTerms,
        binding: Arc<dyn AgentProvider>,
    ) -> EnvironmentFuture<'a, Result<EnvironmentLease, LeaseRefusal>> {
        Box::pin(async move {
            // The channel's cloud, read as the real gateway reads it.
            let (channel, cloud) = channel_cloud(self.2.as_ref(), &self.1).await?;
            self.0
                .open(self.1.clone(), true, channel, cloud)
                .await
                .map_err(|_| LeaseRefusal::EnvironmentUnreachable)?;
            let mut opened = in_process_environment().open(lease, grant, binding).await?;
            opened.workspace = Some(folder(&self.1.to_string()).to_str().unwrap().into());
            Ok(opened)
        })
    }
    fn account<'a>(&'a self, _: &'a LeaseId) -> EnvironmentFuture<'a, Option<LeaseCleanup>> {
        Box::pin(async { Some(LeaseCleanup::NotHeld) })
    }
}
