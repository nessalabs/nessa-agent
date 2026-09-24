//! Current-agent composition over substituted runtime and credential effects.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    },
    time::Duration,
};

use super::*;
use crate::{
    agent_install::{
        application::{
            ManagedExecutableUse, ManagedExecutableUseAdmissionFailure,
            ManagedExecutableUseFailure, ManagedExecutableUseGuard, ManagedLaunchSnapshot,
            Publication, PublicationLease, PublishFailure, RuntimeStore, StagedArchive,
            StoreFailure,
        },
        domain::{preferred_release, AgentName, ArchiveDigest, PinnedRelease},
        infrastructure::{host_platform, releases_for},
    },
    agents::{
        application::{
            AgentCredential, AgentCredentialFailure, AgentCredentialKind, ReadAgentReadiness,
        },
        domain::Readiness,
        infrastructure::LocalAgentProbe,
    },
    composition::local_auth::SystemClock,
    conversation::application::{
        ConversationAgentSource, ConversationAgents, ConversationCaller, ConversationDependencies,
        ConversationLimits, ConversationService, SubmissionMode, SubmittedMessage,
    },
    conversation::domain::ConversationId,
    conversation_test_support::{
        AcceptingAudit, AcceptingCreationAudit, MemoryRepository, Provider, ProviderFactory,
        RecordingFileLinkAudit, TestClock,
    },
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_sdk::{
    application::agent_execution::providers::{UserImageFuture, UserImageSource},
    domain::agent_execution::prompts::ImageReference,
    infrastructure::session_storage::InMemoryStorage,
};
use std::os::unix::fs::PermissionsExt;

#[derive(Clone)]
enum StoreAnswer {
    Missing,
    Ready(ManagedLaunchSnapshot),
    Failed(StoreFailure),
}

struct Store {
    answer: Mutex<StoreAnswer>,
    published_pin: Mutex<Option<(String, String, String)>>,
    reads: AtomicUsize,
}

impl Store {
    fn new(answer: StoreAnswer) -> Self {
        Self {
            answer: Mutex::new(answer),
            published_pin: Mutex::new(None),
            reads: AtomicUsize::new(0),
        }
    }

    fn answer(&self, answer: StoreAnswer) {
        *self.answer.lock().unwrap() = answer;
    }

    fn publish_current(&self, release: &PinnedRelease, snapshot: ManagedLaunchSnapshot) {
        *self.published_pin.lock().unwrap() = Some((
            release.version().as_str().to_owned(),
            release.archive_digest().as_str().to_owned(),
            release.launch().as_str().to_owned(),
        ));
        self.answer(StoreAnswer::Ready(snapshot));
    }

    fn publish_mismatched(&self, snapshot: ManagedLaunchSnapshot) {
        *self.published_pin.lock().unwrap() = Some((
            "not-the-current-version".into(),
            "sha256:not-the-current-digest".into(),
            "not-the-current-launch".into(),
        ));
        self.answer(StoreAnswer::Ready(snapshot));
    }
}

impl RuntimeStore for Store {
    fn installed(
        &self,
        _agent: &AgentName,
        _release: &PinnedRelease,
    ) -> Result<Option<PathBuf>, StoreFailure> {
        unreachable!("current-agent resolution retains a managed launch")
    }

    fn managed_launch(
        &self,
        _agent: &AgentName,
        release: &PinnedRelease,
    ) -> Result<Option<ManagedLaunchSnapshot>, StoreFailure> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        if let Some(expected) = self.published_pin.lock().unwrap().as_ref() {
            let requested = (
                release.version().as_str(),
                release.archive_digest().as_str(),
                release.launch().as_str(),
            );
            if requested
                != (
                    expected.0.as_str(),
                    expected.1.as_str(),
                    expected.2.as_str(),
                )
            {
                return Ok(None);
            }
        }
        match self.answer.lock().unwrap().clone() {
            StoreAnswer::Missing => Ok(None),
            StoreAnswer::Ready(snapshot) => Ok(Some(snapshot)),
            StoreAnswer::Failed(failure) => Err(failure),
        }
    }

    fn reclamation_lease(
        &self,
        _agent: &AgentName,
    ) -> Result<Box<dyn PublicationLease>, StoreFailure> {
        unreachable!("resolution does not reclaim runtimes")
    }

    fn stage(&self, _agent: &AgentName) -> Result<StagedArchive, StoreFailure> {
        unreachable!("resolution does not stage runtimes")
    }

    fn digest(&self, _staged: &mut StagedArchive) -> Result<ArchiveDigest, StoreFailure> {
        unreachable!("resolution does not hash runtimes")
    }

    fn publish(
        &self,
        _agent: &AgentName,
        _release: &PinnedRelease,
        _staged: &mut StagedArchive,
    ) -> Result<Publication, PublishFailure> {
        unreachable!("resolution does not publish runtimes")
    }

    fn discard(&self, _staged: StagedArchive) {
        unreachable!("resolution stages nothing to discard")
    }
}

#[derive(Clone)]
enum CredentialAnswer {
    Missing,
    ApiKey(String),
    OAuth(String),
    Failed(AgentCredentialFailure),
}

struct Credentials {
    answer: Mutex<CredentialAnswer>,
    reads: AtomicUsize,
}

impl Credentials {
    fn new(answer: CredentialAnswer) -> Self {
        Self {
            answer: Mutex::new(answer),
            reads: AtomicUsize::new(0),
        }
    }

    fn answer(&self, answer: CredentialAnswer) {
        *self.answer.lock().unwrap() = answer;
    }
}

impl AgentCredentialSource for Credentials {
    fn read(&self, agent: AgentId) -> Result<Option<AgentCredential>, AgentCredentialFailure> {
        assert_eq!(agent, AgentId::Opencode);
        self.reads.fetch_add(1, Ordering::SeqCst);
        match self.answer.lock().unwrap().clone() {
            CredentialAnswer::Missing => Ok(None),
            CredentialAnswer::ApiKey(secret) => {
                AgentCredential::new(AgentCredentialKind::ApiKey, secret.into_bytes())
                    .map(Some)
                    .map_err(|_| AgentCredentialFailure::Invalid)
            }
            CredentialAnswer::OAuth(secret) => {
                AgentCredential::new(AgentCredentialKind::OAuthToken, secret.into_bytes())
                    .map(Some)
                    .map_err(|_| AgentCredentialFailure::Invalid)
            }
            CredentialAnswer::Failed(failure) => Err(failure),
        }
    }
}

struct NoImages;

impl UserImageSource for NoImages {
    fn read(&self, _image: ImageReference) -> UserImageFuture<'_> {
        unreachable!("resolver tests do not submit messages")
    }
}

fn fixed_agent() -> ConversationAgent {
    ConversationAgent {
        provider: Arc::new(Provider::new(Arc::new(ProviderFactory::default()))),
        execution_audit: Arc::new(AcceptingAudit),
        reserved_output_tokens: 4096,
        readiness: None,
    }
}

fn config(root: &Path, selected: AgentId) -> AgentsConfig {
    let workspace = root.join("workspace");
    nessa_local_storage::create_directory(&workspace).unwrap();
    serde_json::from_value(serde_json::json!({
        "catalog": concat!(env!("CARGO_MANIFEST_DIR"), "/../nessa-sdk/data/models.json"),
        "workspace": workspace,
        "selected": selected.name(),
        "runtimes": {
            "claude": {
                "command": root.join("claude"),
                "args": [],
                "model": "claude-sonnet-4-5-20250929",
                "toolsEnabled": false
            }
        }
    }))
    .unwrap()
}

fn resolver(
    root: &Path,
    store: Arc<dyn RuntimeStore>,
    credentials: Arc<dyn AgentCredentialSource>,
    fixed: HashMap<AgentId, ConversationAgent>,
) -> CurrentAgentResolver {
    CurrentAgentResolver::new(CurrentAgentResolverInput {
        fixed,
        fixed_probe: LocalAgentProbe::from_environment(HashMap::new(), credentials.clone()),
        config: config(root, AgentId::Opencode),
        managed_opencode: true,
        store,
        host: host_platform(),
        credentials,
        provider_directory: root.join("providers"),
        clock: Arc::new(SystemClock),
        images: Arc::new(NoImages),
    })
}

fn executable(root: &Path) -> ManagedLaunchSnapshot {
    let executable = root.join("opencode");
    std::fs::write(&executable, "fixture").unwrap();
    ManagedLaunchSnapshot::unmanaged(executable)
}

#[test]
fn one_observation_reads_runtime_and_credential_once_and_keeps_failures_distinct() {
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::new(StoreAnswer::Missing));
    let credentials = Arc::new(Credentials::new(CredentialAnswer::ApiKey("secret".into())));
    let resolver = resolver(
        root.path(),
        store.clone(),
        credentials.clone(),
        HashMap::new(),
    );

    let missing = resolver.evidence(AgentId::Opencode).unwrap();
    assert_eq!(missing.installed, Ok(false));
    assert_eq!(missing.authenticated, Some(Ok(true)));
    assert_eq!(store.reads.load(Ordering::SeqCst), 1);
    assert_eq!(credentials.reads.load(Ordering::SeqCst), 1);

    store.answer(StoreAnswer::Failed(StoreFailure::Unreadable(
        "record".into(),
    )));
    credentials.answer(CredentialAnswer::Missing);
    let unknown = resolver.evidence(AgentId::Opencode).unwrap();
    assert_eq!(unknown.installed, Err(ProbeFailure::Unanswered));
    assert_eq!(unknown.authenticated, Some(Ok(false)));

    store.answer(StoreAnswer::Ready(executable(root.path())));
    credentials.answer(CredentialAnswer::OAuth("oauth".into()));
    let unsupported = resolver.evidence(AgentId::Opencode).unwrap();
    assert_eq!(unsupported.installed, Ok(true));
    assert_eq!(
        unsupported.authenticated,
        Some(Err(ProbeFailure::Unanswered))
    );

    credentials.answer(CredentialAnswer::Failed(
        AgentCredentialFailure::Unavailable,
    ));
    let unavailable = resolver.evidence(AgentId::Opencode).unwrap();
    assert_eq!(unavailable.installed, Ok(true));
    assert_eq!(
        unavailable.authenticated,
        Some(Err(ProbeFailure::Unanswered))
    );
}

#[tokio::test]
async fn cold_resolution_reobserves_after_readiness_in_both_directions() {
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::new(StoreAnswer::Missing));
    let credentials = Arc::new(Credentials::new(CredentialAnswer::ApiKey("secret".into())));
    let resolver = resolver(root.path(), store.clone(), credentials, HashMap::new());

    assert_eq!(
        resolver.evidence(AgentId::Opencode).unwrap().installed,
        Ok(false)
    );
    store.answer(StoreAnswer::Ready(executable(root.path())));
    assert!(resolver.resolve(AgentId::Opencode).await.unwrap().is_some());

    assert_eq!(
        resolver.evidence(AgentId::Opencode).unwrap().installed,
        Ok(true)
    );
    store.answer(StoreAnswer::Missing);
    assert!(resolver.resolve(AgentId::Opencode).await.unwrap().is_none());
    assert_eq!(store.reads.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn cold_resolution_refuses_pin_credential_and_model_contradictions() {
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::new(StoreAnswer::Missing));
    let credentials = Arc::new(Credentials::new(CredentialAnswer::ApiKey("secret".into())));
    let mut resolver = resolver(
        root.path(),
        store.clone(),
        credentials.clone(),
        HashMap::new(),
    );
    let current = current_release();
    store.publish_mismatched(executable(root.path()));
    assert!(resolver.resolve(AgentId::Opencode).await.unwrap().is_none());

    store.publish_current(&current, executable(root.path()));
    credentials.answer(CredentialAnswer::Missing);
    assert!(resolver.resolve(AgentId::Opencode).await.unwrap().is_none());
    credentials.answer(CredentialAnswer::OAuth("oauth".into()));
    assert!(resolver.resolve(AgentId::Opencode).await.is_err());
    credentials.answer(CredentialAnswer::Failed(AgentCredentialFailure::Invalid));
    assert!(resolver.resolve(AgentId::Opencode).await.is_err());

    credentials.answer(CredentialAnswer::ApiKey("secret".into()));
    let catalog = root.path().join("unsupported-models.json");
    std::fs::write(
        &catalog,
        serde_json::json!({"verifiedOn": "2026-09-24", "models": []}).to_string(),
    )
    .unwrap();
    resolver.config.catalog = catalog;
    assert!(resolver.evidence(AgentId::Opencode).is_none());
    assert!(resolver.resolve(AgentId::Opencode).await.is_err());
}

struct BlockingCredentials {
    entered: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    finished: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    release: Mutex<mpsc::Receiver<()>>,
    reads: AtomicUsize,
}

impl AgentCredentialSource for BlockingCredentials {
    fn read(&self, agent: AgentId) -> Result<Option<AgentCredential>, AgentCredentialFailure> {
        assert_eq!(agent, AgentId::Opencode);
        let read = self.reads.fetch_add(1, Ordering::SeqCst);
        if read == 0 {
            if let Some(entered) = self.entered.lock().unwrap().take() {
                entered.send(()).ok();
            }
            self.release.lock().unwrap().recv().unwrap();
            if let Some(finished) = self.finished.lock().unwrap().take() {
                finished.send(()).ok();
            }
        }
        AgentCredential::new(AgentCredentialKind::ApiKey, b"secret".to_vec())
            .map(Some)
            .map_err(|_| AgentCredentialFailure::Invalid)
    }
}

#[tokio::test(start_paused = true)]
async fn timed_out_and_cancelled_callers_do_not_release_or_multiply_blocking_work() {
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::new(StoreAnswer::Ready(executable(root.path()))));
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let credentials = Arc::new(BlockingCredentials {
        entered: Mutex::new(Some(entered_tx)),
        finished: Mutex::new(Some(finished_tx)),
        release: Mutex::new(release_rx),
        reads: AtomicUsize::new(0),
    });
    let resolver = Arc::new(resolver(
        root.path(),
        store,
        credentials.clone(),
        HashMap::from([(AgentId::Claude, fixed_agent())]),
    ));

    let first = tokio::spawn({
        let resolver = resolver.clone();
        async move { resolver.resolve(AgentId::Opencode).await }
    });
    entered_rx.await.unwrap();
    first.abort();
    match first.await {
        Err(error) => assert!(error.is_cancelled()),
        Ok(_) => panic!("aborted resolver unexpectedly completed"),
    }

    assert!(
        resolver.resolve(AgentId::Claude).await.unwrap().is_some(),
        "a fixed configured agent never waits for OpenCode effects"
    );

    let waiting = tokio::spawn({
        let resolver = resolver.clone();
        async move { resolver.resolve(AgentId::Opencode).await }
    });
    tokio::task::yield_now().await;
    tokio::time::advance(RESOLUTION_DEADLINE + Duration::from_secs(1)).await;
    assert!(waiting.await.unwrap().is_err());
    assert_eq!(credentials.reads.load(Ordering::SeqCst), 1);

    release_tx.send(()).unwrap();
    finished_rx.await.unwrap();
    let completed = resolver.slots.clone().acquire_owned().await.unwrap();
    drop(completed);
    assert!(resolver.resolve(AgentId::Opencode).await.unwrap().is_some());
    assert_eq!(credentials.reads.load(Ordering::SeqCst), 2);
}

#[test]
fn configured_set_and_default_share_the_deferred_agent_authority() {
    let root = tempfile::tempdir().unwrap();
    let resolver = resolver(
        root.path(),
        Arc::new(Store::new(StoreAnswer::Missing)),
        Arc::new(Credentials::new(CredentialAnswer::Missing)),
        HashMap::from([(AgentId::Claude, fixed_agent())]),
    );

    assert_eq!(
        resolver.configured(),
        HashSet::from([AgentId::Claude, AgentId::Opencode])
    );
    assert_eq!(resolver.default_agent().unwrap(), AgentId::Opencode);
}

#[derive(Default)]
struct UseAuthority {
    admissions: AtomicUsize,
    releases: Arc<AtomicUsize>,
}

impl ManagedExecutableUse for UseAuthority {
    fn admit(
        &self,
    ) -> Result<Box<dyn ManagedExecutableUseGuard>, ManagedExecutableUseAdmissionFailure> {
        self.admissions.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(UseGuard(self.releases.clone())))
    }
}

struct UseGuard(Arc<AtomicUsize>);

impl ManagedExecutableUseGuard for UseGuard {
    fn release(&mut self) -> Result<(), ManagedExecutableUseFailure> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

fn current_release() -> PinnedRelease {
    let agent = AgentName::parse(AgentId::Opencode.name()).unwrap();
    preferred_release(releases_for(&agent).unwrap(), &host_platform())
        .expect("test host has a current OpenCode pin")
}

fn fixture_wrapper(root: &Path, name: &str) -> PathBuf {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(
        "../nessa-sdk/tests/infrastructure/acp/contracts/fixtures/opencode_acp_test_handler.py",
    );
    let wrapper = root.join(name);
    let source = format!(
        r#"#!/usr/bin/env python3
import json, os, pathlib, sys
root = pathlib.Path.cwd()
launch = root / "launch-{name}.json"
pending_launch = root / f".launch-{name}-{{os.getpid()}}.json"
pending_launch.write_text(json.dumps({{
    "executable": str(pathlib.Path(sys.argv[0]).resolve()),
    "arguments": sys.argv[1:],
    "pid": os.getpid(),
    "credentialPresent": bool(os.environ.get("OPENCODE_API_KEY")),
}}))
os.replace(pending_launch, launch)
os.environ["NESSA_EXPECTED_OPENCODE_DATA_HOME"] = os.environ["XDG_DATA_HOME"]
os.environ["NESSA_REFUSED_OPENCODE_HOME"] = str(root / "refused-home")
os.environ["NESSA_EXPECTED_OPENCODE_MODEL"] = "opencode/minimax-m3"
os.execv(sys.executable, [sys.executable, {fixture:?}, "default"])
"#,
        name = name,
        fixture = fixture,
    );
    std::fs::write(&wrapper, source).unwrap();
    let mut permissions = std::fs::metadata(&wrapper).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&wrapper, permissions).unwrap();
    wrapper
}

fn caller(action: &str) -> ConversationCaller {
    ConversationCaller {
        organization_id: OrganizationId::new("org").unwrap(),
        principal_id: PrincipalId::new("person").unwrap(),
        surface_id: "panel".into(),
        action_id: action.into(),
    }
}

fn conversation_id() -> ConversationId {
    ConversationId::new(&uuid::Uuid::new_v4().to_string()).unwrap()
}

fn assert_process(pid: i32, alive: bool) {
    assert_eq!(unsafe { libc::kill(pid, 0) } == 0, alive);
}

async fn launched(path: &Path) -> serde_json::Value {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match std::fs::read(path) {
                Ok(bytes) => break serde_json::from_slice(&bytes).unwrap(),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Err(error) => panic!("could not read launch marker: {error}"),
            }
        }
    })
    .await
    .expect("managed ACP fixture launched")
}

fn service(root: &Path, resolver: Arc<CurrentAgentResolver>) -> ConversationService {
    let agents = ConversationAgents::from_source(
        resolver.configured(),
        resolver.default_agent().unwrap(),
        resolver,
    )
    .unwrap();
    ConversationService::new(
        ConversationDependencies {
            agents,
            storage: Arc::new(InMemoryStorage::new()),
            metadata: Arc::new(MemoryRepository::default()),
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        Some(root.join("workspace").to_string_lossy().into_owned()),
    )
    .unwrap()
}

#[tokio::test]
async fn install_refresh_launches_the_managed_fixture_and_live_generation_stays_owned() {
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::new(StoreAnswer::Missing));
    let credentials = Arc::new(Credentials::new(CredentialAnswer::ApiKey(
        "synthetic-non-secret".into(),
    )));
    let resolver = Arc::new(resolver(
        root.path(),
        store.clone(),
        credentials,
        HashMap::new(),
    ));
    let readiness = ReadAgentReadiness {
        probe: resolver.as_ref(),
    };
    assert_eq!(
        readiness.execute(AgentId::Opencode),
        Readiness::NotInstalled
    );

    let release = current_release();
    let first_authority = Arc::new(UseAuthority::default());
    let first_path = fixture_wrapper(root.path(), "opencode-first");
    store.publish_current(
        &release,
        ManagedLaunchSnapshot::new(first_path.clone(), first_authority.clone()),
    );
    assert_eq!(readiness.execute(AgentId::Opencode), Readiness::Ready);

    let service = service(root.path(), resolver);

    let first = conversation_id();
    service
        .create(first.clone(), caller("create-first"), None)
        .await
        .unwrap();
    service
        .submit(
            first.clone(),
            caller("submit-first"),
            "first-execution".into(),
            SubmittedMessage {
                text: "hello".into(),
                ..SubmittedMessage::default()
            },
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    let first_launch = launched(&root.path().join("workspace/launch-opencode-first.json")).await;
    assert_eq!(
        first_launch["executable"],
        first_path
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .as_ref()
    );
    assert_eq!(first_launch["arguments"], serde_json::json!(["acp"]));
    assert_eq!(first_launch["credentialPresent"], true);
    let first_pid = i32::try_from(first_launch["pid"].as_i64().unwrap()).unwrap();
    assert_process(first_pid, true);
    assert_eq!(first_authority.admissions.load(Ordering::SeqCst), 1);

    let second_authority = Arc::new(UseAuthority::default());
    let second_path = fixture_wrapper(root.path(), "opencode-second");
    store.publish_current(
        &release,
        ManagedLaunchSnapshot::new(second_path.clone(), second_authority.clone()),
    );
    let reads_before_reopen = store.reads.load(Ordering::SeqCst);
    service
        .create(first.clone(), caller("reopen-first"), None)
        .await
        .unwrap();
    assert_eq!(store.reads.load(Ordering::SeqCst), reads_before_reopen);
    assert_process(first_pid, true);

    let second = conversation_id();
    service
        .create(second.clone(), caller("create-second"), None)
        .await
        .unwrap();
    service
        .submit(
            second.clone(),
            caller("submit-second"),
            "second-execution".into(),
            SubmittedMessage {
                text: "hello again".into(),
                ..SubmittedMessage::default()
            },
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    let second_launch = launched(&root.path().join("workspace/launch-opencode-second.json")).await;
    assert_eq!(
        second_launch["executable"],
        second_path
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .as_ref()
    );
    assert_eq!(second_launch["arguments"], serde_json::json!(["acp"]));
    assert_eq!(second_launch["credentialPresent"], true);
    let second_pid = i32::try_from(second_launch["pid"].as_i64().unwrap()).unwrap();
    assert_process(second_pid, true);
    assert_eq!(second_authority.admissions.load(Ordering::SeqCst), 1);

    service.close(first, caller("close-first")).await.unwrap();
    service.close(second, caller("close-second")).await.unwrap();
    assert_process(first_pid, false);
    assert_process(second_pid, false);
    assert_eq!(first_authority.releases.load(Ordering::SeqCst), 1);
    assert_eq!(second_authority.releases.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn stopping_during_native_resolution_prevents_provider_launch_and_preserves_work_ownership() {
    let root = tempfile::tempdir().unwrap();
    let authority = Arc::new(UseAuthority::default());
    let store = Arc::new(Store::new(StoreAnswer::Ready(ManagedLaunchSnapshot::new(
        fixture_wrapper(root.path(), "opencode-stopped"),
        authority.clone(),
    ))));
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let credentials = Arc::new(BlockingCredentials {
        entered: Mutex::new(Some(entered_tx)),
        finished: Mutex::new(Some(finished_tx)),
        release: Mutex::new(release_rx),
        reads: AtomicUsize::new(0),
    });
    let resolver = Arc::new(resolver(
        root.path(),
        store,
        credentials.clone(),
        HashMap::new(),
    ));
    let service = service(root.path(), resolver.clone());
    let opening = tokio::spawn({
        let service = service.clone();
        async move {
            service
                .create(conversation_id(), caller("create-stopped"), None)
                .await
        }
    });
    entered_rx.await.unwrap();

    service.stop_active_agents().await.unwrap();
    assert!(opening.await.unwrap().is_err());
    assert_eq!(credentials.reads.load(Ordering::SeqCst), 1);
    assert_eq!(authority.admissions.load(Ordering::SeqCst), 0);
    assert!(!root
        .path()
        .join("workspace/launch-opencode-stopped.json")
        .exists());
    assert_eq!(
        resolver.evidence(AgentId::Opencode).unwrap().installed,
        Err(ProbeFailure::Unanswered),
        "native work retains the only observation permit after caller loss"
    );

    release_tx.send(()).unwrap();
    finished_rx.await.unwrap();
    let completed = resolver.slots.clone().acquire_owned().await.unwrap();
    drop(completed);
    assert_eq!(
        resolver.evidence(AgentId::Opencode).unwrap().installed,
        Ok(true)
    );
}
