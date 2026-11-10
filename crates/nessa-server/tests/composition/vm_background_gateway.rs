//! Test-process composition for the desktop VM background step. No Boat HTTP,
//! SSH account or credential is constructed. Only this ignored test installs
//! the fake provider; released binaries cannot select it.
use crate::conversation::{
    application::{
        ConversationError, ConversationLimits, Environment, EnvironmentDeclaration,
        EnvironmentFuture, EnvironmentLease, ThreadEnvironments, WorkspaceRest,
    },
    infrastructure::{
        in_process_environment,
        ssh_environment::{Build, BuildSource, HostInstaller},
    },
};
use crate::workspaces::{application::*, domain::*, infrastructure::FileAssignments};
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
    let Some(fixture) = FIXTURE.get() else { return };
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(fixture.root.join("events.jsonl"))
        .unwrap();
    let mut record = serde_json::to_vec(&serde_json::json!({"kind":kind,"thread":thread})).unwrap();
    record.push(b'\n');
    file.write_all(&record).unwrap();
}
fn folder(thread: &str) -> PathBuf {
    FIXTURE.get().unwrap().root.join(thread)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "long-lived gateway owned by the desktop VM stress driver"]
async fn serve() {
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
    CompositionRoot::serve(LocalProvisioning::Manual, &Launch::Standalone)
        .await
        .unwrap();
}

pub(super) fn service(default: Arc<Workspaces>) -> Arc<Workspaces> {
    let Some(fixture) = FIXTURE.get() else {
        return default;
    };
    Arc::new(Workspaces::new(
        Arc::new(Config),
        Arc::new(FakeProvider),
        Arc::new(FileAssignments(fixture.root.join("assignments"))),
        Arc::new(Audit),
        default.runtime_settings.clone(),
    ))
}
pub(super) fn environments(
    default: Arc<dyn ThreadEnvironments>,
    service: Arc<Workspaces>,
) -> Arc<dyn ThreadEnvironments> {
    if FIXTURE.get().is_none() {
        return default;
    }
    Arc::new(FakeEnvironments(service))
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
    async fn read(&self) -> Result<BoatConnection, Failure> {
        BoatConnection::new(
            "fixture-token".into(),
            "nessa".into(),
            "fixture-image".into(),
        )
    }
    fn generation(&self) -> Result<u64, Failure> {
        Ok(1)
    }
    async fn save(&self, _: u64, _: BoatConnection) -> Result<(), Failure> {
        unreachable!()
    }
    async fn clear(&self) -> Result<(), Failure> {
        unreachable!()
    }
}
struct Audit;
impl RecoveryAudit for Audit {
    fn record(&self, _: &RecoveryEvent) -> Result<(), Failure> {
        Ok(())
    }
}
struct FakeProvider;
#[async_trait::async_trait]
impl Provider for FakeProvider {
    async fn validate(&self, _: &BoatConnection) -> Result<(), Failure> {
        Ok(())
    }
    async fn create(&self, _: &BoatConnection, thread: &str) -> Result<WorkspaceReceipt, Failure> {
        std::fs::create_dir_all(folder(thread)).map_err(|_| Failure::Storage)?;
        event("created", thread);
        Ok(WorkspaceReceipt {
            vm: thread.into(),
            admission: EnvironmentIdentity::new(
                "11111111-1111-4111-8111-111111111111".into(),
                "nessa".into(),
                "22222222-2222-4222-8222-222222222222".into(),
                2,
            )
            .unwrap(),
            refusal: None,
            disposition: RefusalDisposition::RetainTarget,
        })
    }
    async fn connect(
        &self,
        _: &BoatConnection,
        thread: &str,
        _: &str,
        _: &EnvironmentIdentity,
    ) -> Result<Arc<dyn WorkspaceRuntime>, Failure> {
        Ok(Arc::new(FakeVm(thread.into())))
    }
    async fn remove(&self, _: &BoatConnection, _: &str) -> Result<(), Failure> {
        unreachable!()
    }
    async fn stop(&self, _: &BoatConnection, vm: &str) -> Result<(), Failure> {
        event("rested", vm);
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
struct FakeEnvironments(Arc<Workspaces>);
impl ThreadEnvironments for FakeEnvironments {
    fn environment(&self, id: &ConversationId) -> Result<Arc<dyn Environment>, ConversationError> {
        Ok(Arc::new(FakeLease(self.0.clone(), id.clone())))
    }
    fn available(&self) -> EnvironmentFuture<'_, bool> {
        Box::pin(async { true })
    }
    fn rest<'a>(
        &'a self,
        id: &'a ConversationId,
        cause: WorkspaceRest,
    ) -> EnvironmentFuture<'a, bool> {
        Box::pin(async move {
            self.0
                .rest(
                    id.clone(),
                    match cause {
                        WorkspaceRest::Idle => RestCause::Idle,
                        WorkspaceRest::Closed => RestCause::Closed,
                    },
                )
                .await
                .stopped
                .unwrap_or(false)
        })
    }
}
struct FakeLease(Arc<Workspaces>, ConversationId);
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
            self.0
                .open(self.1.clone(), true, None)
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
