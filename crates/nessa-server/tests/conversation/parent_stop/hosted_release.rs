//! Real hosted ACP cleanup retains executable-use ownership across remote End.
use super::*;
use crate::conversation::application::{GrantedTerms, LeaseHold, LeaseRelease};
use nessa_sdk::application::agent_execution::{
    agents::AgentFuture,
    executions::{ExecutionAudit, ExecutionAuditRecord},
    providers::{
        ExecutableUse, ExecutableUseAdmissionFailure, ExecutableUseError, ExecutableUseGuard,
        ExecutableUseSnapshot, HarnessCleanupFuture, HarnessControl, HarnessHost, HarnessLaunch,
        HarnessProcess,
    },
};
use nessa_sdk::application::dto::{ModalitiesDto, ModelMetadataDto};
use nessa_sdk::domain::{
    agent_execution::{
        leases::{
            EnvironmentRef, LeaseCleanup, LeaseId, LeaseRefusal, LeaseTerms, SandboxProfiles,
        },
        permissions::PermissionOfferPolicy,
    },
    common::value_objects::TokenLimits,
    model_metadata::entities::ModelMetadata,
};
use nessa_sdk::infrastructure::{
    acp::sessions::{AcpConfig, McpServerList, StandInSessions},
    claude_acp::sessions::ClaudeAcpProvider,
    harness_process::SupervisedHarness,
};
use std::{collections::BTreeMap, sync::atomic::AtomicUsize};

struct UseAuthority {
    path: PathBuf,
    held: Arc<AtomicUsize>,
    attempts: Arc<AtomicUsize>,
    allow: Arc<AtomicBool>,
}
struct UseGuard {
    held: Arc<AtomicUsize>,
    attempts: Arc<AtomicUsize>,
    allow: Arc<AtomicBool>,
}
impl ExecutableUse for UseAuthority {
    fn executable(&self) -> &Path {
        &self.path
    }
    fn admit(&self) -> Result<Box<dyn ExecutableUseGuard>, ExecutableUseAdmissionFailure> {
        self.held.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(UseGuard {
            held: self.held.clone(),
            attempts: self.attempts.clone(),
            allow: self.allow.clone(),
        }))
    }
}
impl ExecutableUseGuard for UseGuard {
    fn release(&mut self) -> Result<(), ExecutableUseError> {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        if !self.allow.load(Ordering::SeqCst) {
            return Err(ExecutableUseError::new(
                "generation release is not acknowledged",
            ));
        }
        assert_eq!(self.held.fetch_sub(1, Ordering::SeqCst), 1);
        Ok(())
    }
}
type SharedHarnessControl = Arc<tokio::sync::Mutex<Box<dyn HarnessControl>>>;

#[derive(Default)]
struct HostState {
    control: Mutex<Option<SharedHarnessControl>>,
    physical: Mutex<Option<CloseOutcome>>,
    launches: AtomicUsize,
    stops: AtomicUsize,
    ends: AtomicUsize,
}
struct ActualHost {
    workspace: PathBuf,
    state: Arc<HostState>,
}
struct MissingStopAck(Arc<HostState>);
impl HarnessControl for MissingStopAck {
    fn cleanup(&mut self, _grace: Duration, _kill: Duration) -> HarnessCleanupFuture<'_> {
        self.0.stops.fetch_add(1, Ordering::SeqCst);
        let physical = *self.0.physical.lock().unwrap();
        Box::pin(async move { physical.ok_or(AgentError::CleanupUncertain) })
    }
}
impl HarnessHost for ActualHost {
    fn workspace(&self) -> &Path {
        &self.workspace
    }
    fn start(&self, launch: HarnessLaunch) -> Result<HarnessProcess, AgentError> {
        self.state.launches.fetch_add(1, Ordering::SeqCst);
        let mut command = tokio::process::Command::new("/usr/bin/python3");
        command.arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../nessa-sdk/tests/infrastructure/acp/contracts/fixtures/claude_acp_test_handler.py")).arg("questions-disabled").current_dir(&self.workspace).env_clear().envs(launch.environment);
        let process = SupervisedHarness::start(command)?;
        *self.state.control.lock().unwrap() =
            Some(Arc::new(tokio::sync::Mutex::new(process.control)));
        Ok(HarnessProcess {
            input: process.input,
            output: process.output,
            control: Box::new(MissingStopAck(self.state.clone())),
        })
    }
}
struct RemoteEnvironment(Arc<ActualHost>);
struct RemoteHold(Arc<ActualHost>);
impl LeaseHold for RemoteHold {
    fn model_provider(
        &self,
        provider: Arc<dyn AgentProvider>,
    ) -> Result<Arc<dyn AgentProvider>, AgentError> {
        provider.on_host(self.0.clone())
    }
    fn ends(
        &self,
    ) -> Option<EnvironmentFuture<'static, nessa_sdk::domain::agent_execution::leases::LeaseEndCause>>
    {
        None
    }
    fn end(
        &self,
        _: nessa_sdk::domain::agent_execution::leases::LeaseEndCause,
    ) -> EnvironmentFuture<'_, LeaseRelease> {
        Box::pin(async move {
            self.0.state.ends.fetch_add(1, Ordering::SeqCst);
            let control = self.0.state.control.lock().unwrap().clone().unwrap();
            let physical = control
                .lock()
                .await
                .cleanup(Duration::from_millis(10), Duration::from_secs(1))
                .await
                .unwrap();
            *self.0.state.physical.lock().unwrap() = Some(physical);
            LeaseRelease::Released(LeaseCleanup::Confirmed {
                forced: physical.forced,
            })
        })
    }
}
impl Environment for RemoteEnvironment {
    fn declaration(&self) -> EnvironmentDeclaration {
        EnvironmentDeclaration {
            environment: EnvironmentRef::Here,
            sandbox: SandboxProfiles::HARNESS_DEFAULT,
        }
    }
    fn open<'a>(
        &'a self,
        _: &'a LeaseId,
        grant: &'a LeaseTerms,
        provider: Arc<dyn AgentProvider>,
    ) -> EnvironmentFuture<'a, Result<EnvironmentLease, LeaseRefusal>> {
        Box::pin(async move {
            Ok(EnvironmentLease {
                provider: provider.on_host(self.0.clone()).unwrap(),
                hold: Arc::new(RemoteHold(self.0.clone())),
                granted: GrantedTerms {
                    sandbox: grant.sandbox,
                    timeout_ms: None,
                    lease_ms: None,
                },
                workspace: None,
            })
        })
    }
    fn account<'a>(&'a self, _: &'a LeaseId) -> EnvironmentFuture<'a, Option<LeaseCleanup>> {
        let physical = *self.0.state.physical.lock().unwrap();
        Box::pin(async move {
            physical.map(|outcome| LeaseCleanup::Confirmed {
                forced: outcome.forced,
            })
        })
    }
}
struct AcceptAudit;
impl ExecutionAudit for AcceptAudit {
    fn record(&self, _: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}
fn provider(workspace: &Path, executable: ExecutableUseSnapshot) -> ClaudeAcpProvider {
    let config = AcpConfig {
        executable,
        arguments: Vec::new(),
        staged_runtime_arguments: Vec::new(),
        environment: BTreeMap::new(),
        runtime_environment: BTreeMap::new(),
        credential_environment: BTreeMap::new(),
        workspace: workspace.to_owned(),
        tools_enabled: false,
        mcp_servers: McpServerList::none(),
        stand_ins: StandInSessions::none(),
        permissions: PermissionOfferPolicy::once_only(),
        refusal:
            nessa_sdk::application::agent_execution::permissions::ConfiguredRefusal::continue_turn(),
        permission_deadlines:
            nessa_sdk::domain::agent_execution::permissions::PermissionDeadlinePolicy::standard(),
        launch_timeout: Duration::from_secs(5),
        startup_timeout: Duration::from_secs(5),
        execution_timeout: None,
        shutdown_grace: Duration::from_millis(10),
        kill_timeout: Duration::from_secs(1),
        event_capacity: 16,
        max_frame_bytes: 1024 * 1024,
        max_incoming_frame_bytes: 1024 * 1024,
        images: None,
        clock: Arc::new(nessa_sdk::infrastructure::clock::RuntimeClock::new()),
        workspace_paths: Arc::new(nessa_sdk::infrastructure::workspace_paths::FilesystemPaths),
        permission_rules: Arc::new(
            nessa_sdk::application::agent_execution::permissions::IdlePermissionRules,
        ),
        review_notices: Arc::new(
            nessa_sdk::application::agent_execution::permissions::IdleReviewNotices,
        ),
    };
    let text = ModalitiesDto {
        text: true,
        image: false,
        audio: false,
    };
    let model = ModelMetadata::try_from(ModelMetadataDto {
        provider: "anthropic".into(),
        model_id: "test".into(),
        display_name: "Fixture".into(),
        input: text,
        image_input: None,
        output: text,
        tool_use: true,
        reasoning: None,
        fast_mode: false,
        max_context_window_tokens: 10000,
        max_output_tokens: 5000,
        knowledge_cutoff: "2026-01".into(),
        documentation_url: "https://example.com".into(),
    })
    .unwrap();
    ClaudeAcpProvider::new(
        config,
        &model,
        TokenLimits::new(9000, 100).unwrap(),
        Arc::new(AcceptAudit),
    )
    .unwrap()
}
#[tokio::test]
async fn remote_end_cannot_acknowledge_an_unattempted_hosted_executable_guard() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let state = Arc::new(HostState::default());
    let host = Arc::new(ActualHost {
        workspace: directory.path().to_owned(),
        state: state.clone(),
    });
    let environment: Arc<dyn Environment> = Arc::new(RemoteEnvironment(host));
    let (mut service, storage, metadata) = fixture_in(
        &root,
        Arc::new(ProviderFactory::default()),
        environment.into(),
    );
    let held = Arc::new(AtomicUsize::new(0));
    let attempts = Arc::new(AtomicUsize::new(0));
    let allow = Arc::new(AtomicBool::new(false));
    let executable = ExecutableUseSnapshot::new(
        PathBuf::from("/usr/bin/python3"),
        Arc::new(UseAuthority {
            path: PathBuf::from("/usr/bin/python3"),
            held: held.clone(),
            attempts: attempts.clone(),
            allow: allow.clone(),
        }),
    )
    .unwrap();
    Arc::get_mut(&mut service.inner).unwrap().agents =
        only(Arc::new(provider(directory.path(), executable)));
    let coordinator = bind_with_capacity(&service, &storage, &root, true, 1).await;
    let target = id();
    service
        .create(
            target.clone(),
            caller("create"),
            RequestedConversation::default(),
        )
        .await
        .unwrap();
    let live = service.resolve(&target, &caller("inspect")).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while live.agent.attachment_status().phase() != AttachmentPhase::Attached {
            let phase = live.agent.attachment_status().phase();
            assert!(
                !matches!(phase, AttachmentPhase::Failed(_)),
                "hosted ACP fixture failed to attach: {phase:?}"
            );
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(held.load(Ordering::SeqCst), 1);
    let opening = Arc::downgrade(
        service
            .inner
            .conversations
            .lock()
            .await
            .get(&target)
            .unwrap(),
    );
    let lifetime = coordinator
        .active_lifetime_for_session(&conversation_session(&target))
        .unwrap();
    let command = || CloseCommand {
        lifetime: lifetime.clone(),
        cause: LifetimeCause::HostClose,
        initiator: Initiator::Host(
            HostActor::new("person", "panel", "hosted-guard-close").unwrap(),
        ),
        external_attachment: false,
        timeout: None,
    };
    let result = tokio::time::timeout(Duration::from_secs(5), coordinator.end_lifetime(command()))
        .await
        .unwrap();
    assert_eq!(state.ends.load(Ordering::SeqCst), 1);
    assert!(
        state.physical.lock().unwrap().is_some(),
        "actual supervised harness cleanup answered remote End"
    );
    assert_eq!(
        held.load(Ordering::SeqCst),
        1,
        "real managed generation remains admitted"
    );
    assert!(live.agent.may_hold_provider_resources());
    assert!(
        attempts.load(Ordering::SeqCst) > 0,
        "the SDK owner attempted its own guard release after actual End"
    );
    let graph = nessa_sdk::domain::agent_execution::subagents::OwnershipGraph::restore(
        SqliteOwnershipStore::open(&root.join("ownership.sqlite3"))
            .unwrap()
            .read()
            .await
            .unwrap(),
    );
    assert_eq!(
        graph.physical(&lifetime, &lifetime),
        Some(nessa_sdk::domain::agent_execution::subagents::PhysicalFact::Failed),
        "physical End cannot release the SDK guard's graph accounting"
    );
    assert!(
        result.is_err(),
        "remote End cannot acknowledge the independent SDK guard: {result:?}"
    );
    assert!(opening.upgrade().is_some());
    assert!(service.owns_unreleased_resources().await);
    allow.store(true, Ordering::SeqCst);
    tokio::time::timeout(Duration::from_secs(5), coordinator.end_lifetime(command()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(held.load(Ordering::SeqCst), 0);
    assert!(attempts.load(Ordering::SeqCst) > 0);
    assert!(!live.agent.may_hold_provider_resources());
    assert!(opening.upgrade().is_none());
    assert_eq!(state.ends.load(Ordering::SeqCst), 1);
    assert_eq!(state.launches.load(Ordering::SeqCst), 1);
    drop(live);
    retire(service, storage, metadata).await;
}
