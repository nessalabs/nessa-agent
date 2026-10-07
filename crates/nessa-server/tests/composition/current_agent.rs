//! Current-agent composition over substituted runtime and credential effects.

use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
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
        domain::{
            preferred_release, AgentName, ArchiveDigest, HostPlatform, PinnedRelease,
            ReleasePlatform,
        },
        infrastructure::{host_platform, releases_for},
    },
    agent_warm_up::infrastructure::{DurableWarmUpAudit, FileWarmUpRecords},
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
        ConversationLimits, ConversationService, ProviderSessionErasers, SubmissionMode,
        SubmittedMessage,
    },
    conversation_test_support::{
        AcceptingAudit, AcceptingCreationAudit, AcceptingDeletionAudit, MemoryRepository,
        MemorySummaries, Provider, ProviderFactory, RecordingFileLinkAudit, TestClock, Unlisted,
        DELETION_BUDGETS,
    },
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::{
    application::agent_execution::providers::{
        ExecutableUseError, ExecutableUseSnapshot, UserImageFuture, UserImageSource,
    },
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
                "model": "claude-sonnet-5",
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
    let config = config(root, AgentId::Opencode);
    let host = host_platform();
    resolver_from(
        root,
        ResolverTestInput {
            config,
            packaged: true,
            host,
            captured_environment: None,
            store,
            credentials,
            fixed,
        },
    )
}

struct ResolverTestInput {
    config: AgentsConfig,
    packaged: bool,
    host: HostPlatform,
    captured_environment: Option<OsString>,
    store: Arc<dyn RuntimeStore>,
    credentials: Arc<dyn AgentCredentialSource>,
    fixed: HashMap<AgentId, ConversationAgent>,
}

fn resolver_from(root: &Path, input: ResolverTestInput) -> CurrentAgentResolver {
    let ResolverTestInput {
        config,
        packaged,
        host,
        captured_environment,
        store,
        credentials,
        fixed,
    } = input;
    let opencode = EffectiveOpenCodeProfile::decide(&config, packaged, &host, captured_environment);
    CurrentAgentResolver::new(CurrentAgentResolverInput {
        managed_adapters: HashSet::new(),
        fixed,
        fixed_probe: LocalAgentProbe::from_environment(HashMap::new(), credentials.clone()),
        opencode,
        config,
        store,
        host,
        credentials,
        provider_directory: root.join("providers"),
        clock: Arc::new(SystemClock),
        images: Arc::new(NoImages),
        warm_up: CurrentOpenCodeWarmUp::new(
            Arc::new(FileWarmUpRecords::new(root.join("warm-up-records")).unwrap()),
            Arc::new(DurableWarmUpAudit::new(root.join("warm-up-audit")).unwrap()),
            Arc::new(SystemClock),
        ),
    })
}

fn executable(root: &Path) -> ManagedLaunchSnapshot {
    let executable = root.join("opencode");
    std::fs::write(&executable, "fixture").unwrap();
    ManagedLaunchSnapshot::unmanaged(executable)
}

fn explicit_opencode(
    config: &mut AgentsConfig,
    command: PathBuf,
    args: Vec<String>,
    model: &str,
    tools_enabled: bool,
    context_tokens: u32,
    output_tokens: u32,
) {
    config.runtimes.insert(
        AgentId::Opencode.name().into(),
        AgentRuntime {
            command: nessa_sdk::application::agent_execution::providers::ExecutableUseSnapshot::unmanaged(command),
            args,
            model: model.into(),
            tools_enabled,
            context_tokens,
            output_tokens,
        },
    );
}

#[tokio::test]
async fn packaged_explicit_policy_preserves_every_static_field_but_not_its_command() {
    let root = tempfile::tempdir().unwrap();
    let configured_path = root.path().join("configured-path-is-not-authority");
    let managed_path = root.path().join("managed-opencode");
    std::fs::write(&managed_path, "managed").unwrap();
    let mut config = config(root.path(), AgentId::Opencode);
    explicit_opencode(
        &mut config,
        configured_path.clone(),
        vec!["acp".into()],
        "opencode/big-pickle",
        true,
        72_000,
        3072,
    );
    let store = Arc::new(Store::new(StoreAnswer::Ready(
        ManagedLaunchSnapshot::unmanaged(managed_path.clone()),
    )));
    let credentials = Arc::new(Credentials::new(CredentialAnswer::ApiKey("secret".into())));
    let resolver = resolver_from(
        root.path(),
        ResolverTestInput {
            config: config.clone(),
            packaged: true,
            host: host_platform(),
            captured_environment: None,
            store,
            credentials,
            fixed: HashMap::new(),
        },
    );

    assert_eq!(resolver.configured(), HashSet::from([AgentId::Opencode]));
    assert_eq!(resolver.default_agent().unwrap(), AgentId::Opencode);
    let profile = resolver.opencode.configured().unwrap();
    let launch = profile
        .managed_runtime(ExecutableUseSnapshot::unmanaged(managed_path.clone()))
        .unwrap();
    assert_eq!(launch.command.executable(), managed_path);
    assert_eq!(launch.args, ["acp"]);
    assert_eq!(
        agent::image_limits(&config, Some(profile.validated().model())).unwrap(),
        None
    );
    let resolved = resolver.resolve(AgentId::Opencode).await.unwrap().unwrap();
    assert_eq!(
        resolved.provider.identity().model_id(),
        "opencode/big-pickle"
    );
    assert!(resolved.provider.capabilities().features().tool_use());
    assert_eq!(
        resolved
            .provider
            .capabilities()
            .limits()
            .max_context_window(),
        72_000
    );
    assert_eq!(resolved.provider.capabilities().limits().max_output(), 3072);
    assert_eq!(resolved.reserved_output_tokens, 3072);
    assert_eq!(
        config
            .runtime(AgentId::Opencode)
            .unwrap()
            .command
            .executable(),
        configured_path
    );
    assert_ne!(configured_path, managed_path);
}

#[tokio::test]
async fn invalid_packaged_policy_is_unconfigured_without_runtime_or_credential_effects() {
    let cases = [
        ("missing-model", true, 72_000, 3072, vec!["acp"]),
        ("opencode/big-pickle", true, 1024, 2048, vec!["acp"]),
        ("opencode/big-pickle", false, 72_000, 3072, vec!["acp"]),
        ("opencode/big-pickle", true, 72_000, 3072, vec!["serve"]),
    ];
    for (model, tools, context, output, args) in cases {
        let root = tempfile::tempdir().unwrap();
        let mut config = config(root.path(), AgentId::Opencode);
        explicit_opencode(
            &mut config,
            root.path().join("ignored"),
            args.into_iter().map(str::to_owned).collect(),
            model,
            tools,
            context,
            output,
        );
        let store = Arc::new(Store::new(StoreAnswer::Missing));
        let credentials = Arc::new(Credentials::new(CredentialAnswer::ApiKey("secret".into())));
        let resolver = resolver_from(
            root.path(),
            ResolverTestInput {
                config,
                packaged: true,
                host: host_platform(),
                captured_environment: None,
                store: store.clone(),
                credentials: credentials.clone(),
                fixed: HashMap::new(),
            },
        );

        assert!(resolver.evidence(AgentId::Opencode).is_none());
        assert!(resolver.resolve(AgentId::Opencode).await.unwrap().is_none());
        assert!(resolver.default_agent().is_err());
        assert_eq!(store.reads.load(Ordering::SeqCst), 0);
        assert_eq!(credentials.reads.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn unsupported_packaged_host_is_unconfigured_without_effects_while_supported_missing_is_not_installed(
) {
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::new(StoreAnswer::Missing));
    let credentials = Arc::new(Credentials::new(CredentialAnswer::Missing));
    let unsupported = resolver_from(
        root.path(),
        ResolverTestInput {
            config: config(root.path(), AgentId::Opencode),
            packaged: true,
            host: HostPlatform::new(
                ReleasePlatform::new("plan9", "sparc64").unwrap(),
                None,
                false,
            ),
            captured_environment: None,
            store: store.clone(),
            credentials: credentials.clone(),
            fixed: HashMap::new(),
        },
    );
    assert!(unsupported.evidence(AgentId::Opencode).is_none());
    assert!(unsupported
        .resolve(AgentId::Opencode)
        .await
        .unwrap()
        .is_none());
    assert_eq!(store.reads.load(Ordering::SeqCst), 0);
    assert_eq!(credentials.reads.load(Ordering::SeqCst), 0);

    let supported = resolver(
        root.path(),
        store.clone(),
        credentials.clone(),
        HashMap::new(),
    );
    assert_eq!(
        ReadAgentReadiness { probe: &supported }.execute(AgentId::Opencode),
        Readiness::NotInstalled
    );
    assert_eq!(store.reads.load(Ordering::SeqCst), 1);
    assert_eq!(credentials.reads.load(Ordering::SeqCst), 1);
}

#[test]
fn credential_authority_is_scoped_store_when_packaged_and_captured_environment_when_standalone() {
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::new(StoreAnswer::Ready(executable(root.path()))));
    let missing = Arc::new(Credentials::new(CredentialAnswer::Missing));
    let packaged = resolver_from(
        root.path(),
        ResolverTestInput {
            config: config(root.path(), AgentId::Opencode),
            packaged: true,
            host: host_platform(),
            captured_environment: Some(OsString::from("ambient-must-be-ignored")),
            store: store.clone(),
            credentials: missing.clone(),
            fixed: HashMap::new(),
        },
    );
    assert_eq!(
        ReadAgentReadiness { probe: &packaged }.execute(AgentId::Opencode),
        Readiness::NeedsAuthentication
    );
    assert_eq!(missing.reads.load(Ordering::SeqCst), 1);

    let explicit = root.path().join("standalone");
    std::fs::write(&explicit, "standalone").unwrap();
    let mut config = config(root.path(), AgentId::Opencode);
    explicit_opencode(
        &mut config,
        explicit,
        vec!["acp".into()],
        "opencode/big-pickle",
        true,
        72_000,
        3072,
    );
    let keychain = Arc::new(Credentials::new(CredentialAnswer::ApiKey(
        "must-not-fallback".into(),
    )));
    let standalone = resolver_from(
        root.path(),
        ResolverTestInput {
            config: config.clone(),
            packaged: false,
            host: host_platform(),
            captured_environment: None,
            store: store.clone(),
            credentials: keychain.clone(),
            fixed: HashMap::new(),
        },
    );
    assert_eq!(
        ReadAgentReadiness { probe: &standalone }.execute(AgentId::Opencode),
        Readiness::NeedsAuthentication
    );
    assert_eq!(keychain.reads.load(Ordering::SeqCst), 0);

    let invalid = resolver_from(
        root.path(),
        ResolverTestInput {
            config,
            packaged: false,
            host: host_platform(),
            captured_environment: Some(OsString::from("  \t")),
            store,
            credentials: keychain.clone(),
            fixed: HashMap::new(),
        },
    );
    assert_eq!(
        ReadAgentReadiness { probe: &invalid }.execute(AgentId::Opencode),
        Readiness::AuthenticationUnknown
    );
    assert_eq!(keychain.reads.load(Ordering::SeqCst), 0);
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

#[test]
fn opencode_is_registered_to_delete_sessions_only_when_configured() {
    use crate::conversation::application::ProviderSessionErasers;
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::new(StoreAnswer::Missing));
    let credentials = Arc::new(Credentials::new(CredentialAnswer::Missing));
    let configured = Arc::new(resolver(
        root.path(),
        store.clone(),
        credentials.clone(),
        HashMap::new(),
    ));
    let mut erasers = ProviderSessionErasers::default();
    configured.register_session_eraser(&mut erasers);
    assert!(erasers.handles(AgentId::Opencode));
    // Standalone, with no OpenCode launch configured: nothing to register.
    let unconfigured = Arc::new(resolver_from(
        root.path(),
        ResolverTestInput {
            config: config(root.path(), AgentId::Claude),
            packaged: false,
            host: host_platform(),
            captured_environment: None,
            store,
            credentials,
            fixed: HashMap::from([(AgentId::Claude, fixed_agent())]),
        },
    ));
    let mut erasers = ProviderSessionErasers::default();
    unconfigured.register_session_eraser(&mut erasers);
    assert!(!erasers.handles(AgentId::Opencode));
}

#[tokio::test]
async fn opencode_is_asked_to_delete_a_session_by_its_current_generation() {
    use crate::conversation::application::{ConversationError, ProviderSessionEraser};
    use nessa_sdk::domain::agent_execution::sessions::ExecutionSessionId;
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::new(StoreAnswer::Missing));
    let credentials = Arc::new(Credentials::new(CredentialAnswer::ApiKey("secret".into())));
    let resolver = Arc::new(resolver(
        root.path(),
        store.clone(),
        credentials,
        HashMap::new(),
    ));
    let eraser = CurrentAgentEraser::new(resolver, AgentId::Opencode);
    let session = || ExecutionSessionId::new("provider-session").unwrap();
    // Not installed now: a known agent not built, so the deletion waits.
    assert!(matches!(
        eraser.erase(session()).await,
        Err(ConversationError::AgentNotConfigured)
    ));
    assert_eq!(store.reads.load(Ordering::SeqCst), 1);
    // Installed since: observed afresh, and that generation's binding is
    // launched to ask — here a file that is not a program, so the launch fails.
    store.answer(StoreAnswer::Ready(executable(root.path())));
    assert!(matches!(
        eraser.erase(session()).await,
        Err(ConversationError::Agent(_))
    ));
    assert_eq!(store.reads.load(Ordering::SeqCst), 2);
    eraser.settled().await;
}

#[tokio::test]
async fn cold_resolution_refuses_pin_and_credential_contradictions_without_redeciding_policy() {
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
    assert!(resolver.evidence(AgentId::Opencode).is_some());
    assert!(resolver.resolve(AgentId::Opencode).await.unwrap().is_some());
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

fn fixture_wrapper(root: &Path, name: &str, expected_model: &str) -> PathBuf {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(
        "../nessa-sdk/tests/infrastructure/acp/contracts/fixtures/opencode_acp_test_handler.py",
    );
    let wrapper = root.join(name);
    let source = format!(
        r#"#!/usr/bin/env python3
import json, os, pathlib, sys
root = pathlib.Path.cwd()
launch = root / "launch-{name}.json"
launch_history = root / f"launch-{name}-{{os.getpid()}}.json"
pending_launch = root / f".launch-{name}-{{os.getpid()}}.json"
record = json.dumps({{
    "executable": str(pathlib.Path(sys.argv[0]).resolve()),
    "arguments": sys.argv[1:],
    "pid": os.getpid(),
    "credentialPresent": bool(os.environ.get("OPENCODE_API_KEY")),
}})
expected_key = root / "expected-opencode-key"
if expected_key.exists():
    assert os.environ.get("OPENCODE_API_KEY") == expected_key.read_text()
pending_launch.write_text(record)
os.replace(pending_launch, launch)
launch_history.write_text(record)
os.environ["NESSA_REFUSED_OPENCODE_DATA_HOME"] = str(root / "refused-data")
os.environ["NESSA_REFUSED_OPENCODE_HOME"] = str(root / "refused-home")
os.environ["NESSA_EXPECTED_OPENCODE_MODEL"] = {expected_model:?}
os.execv(sys.executable, [sys.executable, {fixture:?}, "default"])
"#,
        name = name,
        fixture = fixture,
        expected_model = expected_model,
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

fn live_launch(
    directory: &Path,
    prefix: &str,
    minimum_launches: usize,
) -> Option<serde_json::Value> {
    let launches = std::fs::read_dir(directory)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.file_name().to_string_lossy().starts_with(prefix)
                && entry
                    .path()
                    .extension()
                    .is_some_and(|value| value == "json")
        })
        .filter_map(|entry| std::fs::read(entry.path()).ok())
        .filter_map(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .collect::<Vec<_>>();
    if launches.len() < minimum_launches {
        return None;
    }
    launches.into_iter().find(|launch| {
        let pid = i32::try_from(launch["pid"].as_i64().unwrap()).unwrap();
        unsafe { libc::kill(pid, 0) == 0 }
    })
}

/// The wrapper's launch file is the handshake. A thread waits for it on the
/// wall clock and wakes this task once; a runtime timer would lose to the
/// stall that kept the task from being polled.
async fn launched(path: &Path, minimum_launches: usize) -> serde_json::Value {
    let prefix = format!("{}-", path.file_stem().unwrap().to_string_lossy());
    let directory = path.parent().unwrap().to_path_buf();
    let (tx, rx) = tokio::sync::oneshot::channel();
    thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(30);
        let found = loop {
            if let Some(launch) = live_launch(&directory, &prefix, minimum_launches) {
                break Some(launch);
            }
            if Instant::now() >= deadline {
                break None;
            }
            thread::sleep(Duration::from_millis(10));
        };
        let _ = tx.send(found);
    });
    rx.await
        .expect("launch watcher stayed up")
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
            mode_audit: Arc::new(crate::conversation_test_support::AcceptingModeAudit),

            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            deletion_audit: Arc::new(AcceptingDeletionAudit),
            attachments: None,
            summaries: Arc::new(MemorySummaries::default()),
            listing: Arc::new(Unlisted),
            provider_sessions: ProviderSessionErasers::default(),
            deletion_budgets: DELETION_BUDGETS,
            message_commit_clock: Arc::new(
                nessa_sdk::infrastructure::session_storage::RuntimeMessageCommitClock::new(),
            ),
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
    let first_path = fixture_wrapper(root.path(), "opencode-first", "opencode/minimax-m3");
    store.publish_current(
        &release,
        ManagedLaunchSnapshot::new(first_path.clone(), first_authority.clone()),
    );
    assert_eq!(readiness.execute(AgentId::Opencode), Readiness::Ready);
    let packaged = resolver.resolve(AgentId::Opencode).await.unwrap().unwrap();
    assert_eq!(
        packaged.provider.identity().model_id(),
        "opencode/minimax-m3"
    );
    assert!(packaged.provider.capabilities().features().tool_use());
    assert_eq!(
        packaged
            .provider
            .capabilities()
            .limits()
            .max_context_window(),
        100_000
    );
    assert_eq!(packaged.provider.capabilities().limits().max_output(), 4096);
    assert_eq!(packaged.reserved_output_tokens, 4096);

    let service = service(root.path(), resolver);

    let first = conversation_id();
    service
        .create(
            first.clone(),
            caller("create-first"),
            crate::conversation::application::RequestedConversation::default(),
        )
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
    let first_launch = launched(&root.path().join("workspace/launch-opencode-first.json"), 2).await;
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
    assert_eq!(first_authority.admissions.load(Ordering::SeqCst), 2);

    let second_authority = Arc::new(UseAuthority::default());
    let second_path = fixture_wrapper(root.path(), "opencode-second", "opencode/minimax-m3");
    store.publish_current(
        &release,
        ManagedLaunchSnapshot::new(second_path.clone(), second_authority.clone()),
    );
    let reads_before_reopen = store.reads.load(Ordering::SeqCst);
    service
        .create(
            first.clone(),
            caller("reopen-first"),
            crate::conversation::application::RequestedConversation::default(),
        )
        .await
        .unwrap();
    assert_eq!(store.reads.load(Ordering::SeqCst), reads_before_reopen);
    assert_process(first_pid, true);

    let second = conversation_id();
    service
        .create(
            second.clone(),
            caller("create-second"),
            crate::conversation::application::RequestedConversation::default(),
        )
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
    let second_launch = launched(
        &root.path().join("workspace/launch-opencode-second.json"),
        2,
    )
    .await;
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
    assert_eq!(second_authority.admissions.load(Ordering::SeqCst), 2);

    service.close(first, caller("close-first")).await.unwrap();
    service.close(second, caller("close-second")).await.unwrap();
    assert_process(first_pid, false);
    assert_process(second_pid, false);
    assert_eq!(first_authority.releases.load(Ordering::SeqCst), 2);
    assert_eq!(second_authority.releases.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn proactive_current_warm_up_opens_and_closes_without_a_conversation_or_prompt() {
    let root = tempfile::tempdir().unwrap();
    let authority = Arc::new(UseAuthority::default());
    let store = Arc::new(Store::new(StoreAnswer::Missing));
    let release = current_release();
    let path = fixture_wrapper(root.path(), "opencode-startup", "opencode/minimax-m3");
    store.publish_current(
        &release,
        ManagedLaunchSnapshot::new(path, authority.clone()),
    );
    let resolver = Arc::new(resolver(
        root.path(),
        store,
        Arc::new(Credentials::new(CredentialAnswer::ApiKey(
            "synthetic-non-secret".into(),
        ))),
        HashMap::new(),
    ));

    resolver.start_warm_up();
    // ADR 221: from the moment it is scheduled, before any launch, the warm-up
    // may hold an agent process, so a refused retirement cannot miss it.
    assert!(resolver.warm_up_may_hold_resources());
    let launch = launched(
        &root.path().join("workspace/launch-opencode-startup.json"),
        1,
    )
    .await;
    let pid = i32::try_from(launch["pid"].as_i64().unwrap()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while authority.releases.load(Ordering::SeqCst) != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("startup preparation closes its session and managed use");
    assert_process(pid, false);
    tokio::time::timeout(Duration::from_secs(5), async {
        while resolver.warm_up_may_hold_resources() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("a released warm-up holds nothing");
    assert_eq!(authority.admissions.load(Ordering::SeqCst), 1);
    // No ConversationService or submission exists in this test. The fixture's
    // launch therefore covers initialize, session/new/configuration and close;
    // there is no path that can send session/prompt.
}

#[tokio::test]
async fn missing_key_skips_startup_warm_up_and_a_later_key_recovers_on_the_cold_slot() {
    let root = tempfile::tempdir().unwrap();
    let authority = Arc::new(UseAuthority::default());
    let store = Arc::new(Store::new(StoreAnswer::Missing));
    store.publish_current(
        &current_release(),
        ManagedLaunchSnapshot::new(
            fixture_wrapper(root.path(), "opencode-key-recovery", "opencode/minimax-m3"),
            authority.clone(),
        ),
    );
    let credentials = Arc::new(Credentials::new(CredentialAnswer::Missing));
    let resolver = Arc::new(resolver(
        root.path(),
        store,
        credentials.clone(),
        HashMap::new(),
    ));
    resolver.start_warm_up();
    for _ in 0..64 {
        tokio::task::yield_now().await;
    }
    assert_eq!(authority.admissions.load(Ordering::SeqCst), 0);

    credentials.answer(CredentialAnswer::ApiKey("saved-after-startup".into()));
    let service = service(root.path(), resolver);
    let conversation = conversation_id();
    service
        .create(
            conversation.clone(),
            caller("create-after-save"),
            crate::conversation::application::RequestedConversation::default(),
        )
        .await
        .unwrap();
    service
        .submit(
            conversation.clone(),
            caller("submit-after-save"),
            "hello".into(),
            SubmittedMessage {
                text: "hello".into(),
                ..SubmittedMessage::default()
            },
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    let launch = launched(
        &root
            .path()
            .join("workspace/launch-opencode-key-recovery.json"),
        2,
    )
    .await;
    assert_eq!(launch["credentialPresent"], true);
    assert_eq!(authority.admissions.load(Ordering::SeqCst), 2);
    service
        .close(conversation, caller("close-after-save"))
        .await
        .unwrap();
    assert_eq!(authority.releases.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn rotated_key_shares_runtime_preparation_but_the_conversation_uses_fresh_credentials() {
    let root = tempfile::tempdir().unwrap();
    let authority = Arc::new(UseAuthority::default());
    let store = Arc::new(Store::new(StoreAnswer::Missing));
    store.publish_current(
        &current_release(),
        ManagedLaunchSnapshot::new(
            fixture_wrapper(root.path(), "opencode-key-rotation", "opencode/minimax-m3"),
            authority.clone(),
        ),
    );
    let credentials = Arc::new(Credentials::new(CredentialAnswer::ApiKey(
        "first-synthetic-key".into(),
    )));
    let resolver = Arc::new(resolver(
        root.path(),
        store,
        credentials.clone(),
        HashMap::new(),
    ));
    let expectation = root.path().join("workspace/expected-opencode-key");
    std::fs::write(&expectation, "first-synthetic-key").unwrap();
    let prepared = resolver.resolve(AgentId::Opencode).await.unwrap().unwrap();
    prepared.readiness.unwrap().wait().await;

    credentials.answer(CredentialAnswer::ApiKey("second-synthetic-key".into()));
    std::fs::write(&expectation, "second-synthetic-key").unwrap();
    let service = service(root.path(), resolver);
    let conversation = conversation_id();
    service
        .create(
            conversation.clone(),
            caller("create-after-rotation"),
            crate::conversation::application::RequestedConversation::default(),
        )
        .await
        .unwrap();
    service
        .submit(
            conversation.clone(),
            caller("submit-after-rotation"),
            "rotation".into(),
            SubmittedMessage {
                text: "hello".into(),
                ..SubmittedMessage::default()
            },
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    let launch = launched(
        &root
            .path()
            .join("workspace/launch-opencode-key-rotation.json"),
        2,
    )
    .await;
    assert_eq!(launch["credentialPresent"], true);
    assert!(!launch.to_string().contains("synthetic-key"));
    assert_eq!(authority.admissions.load(Ordering::SeqCst), 2);
    service
        .close(conversation, caller("close-after-rotation"))
        .await
        .unwrap();
    assert_eq!(authority.releases.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn standalone_explicit_profile_launches_with_captured_environment_and_no_managed_effects() {
    let root = tempfile::tempdir().unwrap();
    let wrapper = fixture_wrapper(root.path(), "opencode-standalone", "opencode/big-pickle");
    let mut config = config(root.path(), AgentId::Opencode);
    explicit_opencode(
        &mut config,
        wrapper.clone(),
        vec!["acp".into(), "--explicit-profile".into()],
        "opencode/big-pickle",
        true,
        72_000,
        3072,
    );
    let store = Arc::new(Store::new(StoreAnswer::Failed(StoreFailure::Unreadable(
        "must not be read".into(),
    ))));
    let credentials = Arc::new(Credentials::new(CredentialAnswer::Failed(
        AgentCredentialFailure::Unavailable,
    )));
    let resolver = Arc::new(resolver_from(
        root.path(),
        ResolverTestInput {
            config,
            packaged: false,
            host: host_platform(),
            captured_environment: Some(OsString::from("captured-private-value")),
            store: store.clone(),
            credentials: credentials.clone(),
            fixed: HashMap::new(),
        },
    ));
    assert_eq!(
        ReadAgentReadiness {
            probe: resolver.as_ref()
        }
        .execute(AgentId::Opencode),
        Readiness::Ready
    );
    let resolved = resolver.resolve(AgentId::Opencode).await.unwrap().unwrap();
    assert_eq!(
        resolved.provider.identity().model_id(),
        "opencode/big-pickle"
    );
    assert!(resolved.provider.capabilities().features().tool_use());
    assert_eq!(
        resolved
            .provider
            .capabilities()
            .limits()
            .max_context_window(),
        72_000
    );
    assert_eq!(resolved.provider.capabilities().limits().max_output(), 3072);
    assert_eq!(resolved.reserved_output_tokens, 3072);
    assert_eq!(store.reads.load(Ordering::SeqCst), 0);
    assert_eq!(credentials.reads.load(Ordering::SeqCst), 0);

    let service = service(root.path(), resolver);
    let conversation = conversation_id();
    service
        .create(
            conversation.clone(),
            caller("create-standalone"),
            crate::conversation::application::RequestedConversation::default(),
        )
        .await
        .unwrap();
    service
        .submit(
            conversation.clone(),
            caller("submit-standalone"),
            "standalone-execution".into(),
            SubmittedMessage {
                text: "hello".into(),
                ..SubmittedMessage::default()
            },
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    let launch = launched(
        &root
            .path()
            .join("workspace/launch-opencode-standalone.json"),
        2,
    )
    .await;
    assert_eq!(
        launch["executable"],
        wrapper.canonicalize().unwrap().to_string_lossy().as_ref()
    );
    assert_eq!(
        launch["arguments"],
        serde_json::json!(["acp", "--explicit-profile"])
    );
    assert_eq!(launch["credentialPresent"], true);
    assert!(!launch.to_string().contains("captured-private-value"));
    let pid = i32::try_from(launch["pid"].as_i64().unwrap()).unwrap();
    assert_process(pid, true);
    service
        .close(conversation, caller("close-standalone"))
        .await
        .unwrap();
    assert_process(pid, false);
    assert_eq!(store.reads.load(Ordering::SeqCst), 0);
    assert_eq!(credentials.reads.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn stopping_during_native_resolution_prevents_provider_launch_and_preserves_work_ownership() {
    let root = tempfile::tempdir().unwrap();
    let authority = Arc::new(UseAuthority::default());
    let store = Arc::new(Store::new(StoreAnswer::Ready(ManagedLaunchSnapshot::new(
        fixture_wrapper(root.path(), "opencode-stopped", "opencode/minimax-m3"),
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
                .create(
                    conversation_id(),
                    caller("create-stopped"),
                    crate::conversation::application::RequestedConversation::default(),
                )
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

#[tokio::test]
async fn managed_adapter_observes_installation_and_removal_without_restart() {
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::new(StoreAnswer::Missing));
    let credentials = Arc::new(ClaudeCredentials);
    let mut source = resolver(root.path(), store.clone(), credentials, HashMap::new());
    source.managed_adapters.insert(AgentId::Claude);
    source.config.selected = Some("claude".into());
    std::fs::write(root.path().join("claude"), "adapter").unwrap();
    assert_eq!(source.default_agent().unwrap(), AgentId::Claude);
    assert!(!source.evidence(AgentId::Claude).unwrap().installed.unwrap());
    assert!(source.resolve(AgentId::Claude).await.unwrap().is_none());
    store.answer(StoreAnswer::Ready(executable(root.path())));
    assert!(source.evidence(AgentId::Claude).unwrap().installed.unwrap());
    let provider = source.resolve(AgentId::Claude).await.unwrap().unwrap();
    assert_eq!(provider.provider.identity().model_id(), "claude-sonnet-5");
    store.answer(StoreAnswer::Missing);
    assert!(source.resolve(AgentId::Claude).await.unwrap().is_none());
    // The already-created provider retains the generation it observed.
    assert_eq!(provider.provider.identity().model_id(), "claude-sonnet-5");
}

#[tokio::test]
async fn managed_adapter_store_failures_are_unknown_not_missing() {
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::new(StoreAnswer::Missing));
    let credentials = Arc::new(Credentials::new(CredentialAnswer::Missing));
    let mut source = resolver(root.path(), store.clone(), credentials, HashMap::new());
    source.managed_adapters.insert(AgentId::Claude);
    store.answer(StoreAnswer::Failed(StoreFailure::Unreadable(
        "unreadable".into(),
    )));
    assert!(source.evidence(AgentId::Claude).unwrap().installed.is_err());
    assert!(matches!(
        source.resolve(AgentId::Claude).await,
        Err(ConversationError::Unavailable)
    ));
}

struct ClaudeCredentials;
impl AgentCredentialSource for ClaudeCredentials {
    fn read(&self, agent: AgentId) -> Result<Option<AgentCredential>, AgentCredentialFailure> {
        assert_eq!(agent, AgentId::Claude);
        Ok(Some(
            AgentCredential::new(AgentCredentialKind::ApiKey, b"test-key".to_vec()).unwrap(),
        ))
    }
}

struct ProbeGuard {
    calls: Arc<AtomicUsize>,
    fail: bool,
}
impl ExecutableUseGuard for ProbeGuard {
    fn release(&mut self) -> Result<(), ExecutableUseError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail {
            self.fail = false;
            Err(ExecutableUseError::new("disk unavailable"))
        } else {
            Ok(())
        }
    }
}

#[test]
fn managed_probe_retries_the_same_release_before_observing_another_runtime() {
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::new(StoreAnswer::Missing));
    let mut source = resolver(
        root.path(),
        store.clone(),
        Arc::new(ClaudeCredentials),
        HashMap::new(),
    );
    source.managed_adapters.insert(AgentId::Claude);
    let calls = Arc::new(AtomicUsize::new(0));
    source.probe_cleanup.lock().unwrap().insert(
        AgentId::Claude,
        ProbeCleanup::Release(Box::new(ProbeGuard {
            calls: calls.clone(),
            fail: true,
        })),
    );
    assert!(source.managed_evidence(AgentId::Claude).installed.is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        source.managed_evidence(AgentId::Claude).installed,
        Ok(false)
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(source.probe_cleanup.lock().unwrap().is_empty());
}

#[test]
fn unknown_probe_cleanup_retains_one_owner_without_releasing_or_admitting_again() {
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::new(StoreAnswer::Missing));
    let source = resolver(
        root.path(),
        store.clone(),
        Arc::new(ClaudeCredentials),
        HashMap::new(),
    );
    let calls = Arc::new(AtomicUsize::new(0));
    source.probe_cleanup.lock().unwrap().insert(
        AgentId::Codex,
        ProbeCleanup::UnknownProcess {
            _guard: Box::new(ProbeGuard {
                calls: calls.clone(),
                fail: false,
            }),
        },
    );
    for _ in 0..3 {
        assert!(source.managed_evidence(AgentId::Codex).installed.is_err());
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(store.reads.load(Ordering::SeqCst), 0);
    assert_eq!(source.probe_cleanup.lock().unwrap().len(), 1);
}

struct ProbeAdmission {
    admissions: Arc<AtomicUsize>,
    releases: Arc<AtomicUsize>,
    partial: Option<bool>,
    fail_release: bool,
}
struct ProbeRelease {
    releases: Arc<AtomicUsize>,
    fail: bool,
}
impl ManagedExecutableUseGuard for ProbeRelease {
    fn release(&mut self) -> Result<(), ManagedExecutableUseFailure> {
        self.releases.fetch_add(1, Ordering::SeqCst);
        if std::mem::take(&mut self.fail) {
            Err(ManagedExecutableUseFailure::new("release write failed"))
        } else {
            Ok(())
        }
    }
}
impl ManagedExecutableUse for ProbeAdmission {
    fn admit(
        &self,
    ) -> Result<Box<dyn ManagedExecutableUseGuard>, ManagedExecutableUseAdmissionFailure> {
        self.admissions.fetch_add(1, Ordering::SeqCst);
        let guard = Box::new(ProbeRelease {
            releases: self.releases.clone(),
            fail: self.fail_release,
        });
        let failure = ManagedExecutableUseFailure::new("admission write failed");
        match self.partial {
            Some(true) => {
                Err(ManagedExecutableUseAdmissionFailure::with_confirmed_generation(failure, guard))
            }
            Some(false) => {
                Err(ManagedExecutableUseAdmissionFailure::with_uncertain_generation(failure, guard))
            }
            None => Ok(guard),
        }
    }
}

#[test]
fn managed_probe_failure_transitions_retain_the_admitted_owner_until_its_release() {
    // Both partial admission kinds, successful observation with failed release,
    // and uncertain process cleanup enter through the production observation path.
    for (partial, unknown) in [
        (Some(true), false),
        (Some(false), false),
        (None, false),
        (None, true),
    ] {
        let root = tempfile::tempdir().unwrap();
        let admissions = Arc::new(AtomicUsize::new(0));
        let releases = Arc::new(AtomicUsize::new(0));
        let store = Arc::new(Store::new(StoreAnswer::Ready(ManagedLaunchSnapshot::new(
            root.path().join("native"),
            Arc::new(ProbeAdmission {
                admissions: admissions.clone(),
                releases: releases.clone(),
                partial,
                fail_release: !unknown,
            }),
        ))));
        let source = resolver(
            root.path(),
            store.clone(),
            Arc::new(ClaudeCredentials),
            HashMap::new(),
        );
        let observed = AtomicUsize::new(0);
        let evidence = source.managed_evidence_with(AgentId::Claude, |_| {
            observed.fetch_add(1, Ordering::SeqCst);
            Some(AgentProbeEvidence {
                installed: Ok(true),
                authenticated: Some(if unknown {
                    Err(ProbeFailure::CleanupUnconfirmed)
                } else {
                    Ok(true)
                }),
            })
        });
        assert_eq!(admissions.load(Ordering::SeqCst), 1);
        assert_eq!(
            observed.load(Ordering::SeqCst),
            usize::from(partial.is_none())
        );
        assert_eq!(source.probe_cleanup.lock().unwrap().len(), 1);
        if unknown {
            assert_eq!(
                evidence.authenticated,
                Some(Err(ProbeFailure::CleanupUnconfirmed))
            );
            assert_eq!(releases.load(Ordering::SeqCst), 0);
            for _ in 0..2 {
                assert!(source
                    .managed_evidence_with(AgentId::Claude, |_| panic!(
                        "uncertain child must block another observation"
                    ))
                    .installed
                    .is_err());
            }
            assert_eq!(admissions.load(Ordering::SeqCst), 1);
            assert_eq!(releases.load(Ordering::SeqCst), 0);
        } else {
            assert!(evidence.installed.is_err());
            assert_eq!(releases.load(Ordering::SeqCst), 1);
            store.answer(StoreAnswer::Missing);
            assert_eq!(
                source.managed_evidence(AgentId::Claude).installed,
                Ok(false)
            );
            assert_eq!(releases.load(Ordering::SeqCst), 2);
            assert_eq!(admissions.load(Ordering::SeqCst), 1);
            assert!(source.probe_cleanup.lock().unwrap().is_empty());
        }
    }
}

#[tokio::test]
async fn managed_model_and_mode_refusals_match_fixed_provider_semantics() {
    for (agent, model) in [
        (AgentId::Claude, "claude-sonnet-5"),
        (AgentId::Codex, "gpt-6-astra"),
    ] {
        let root = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::new(StoreAnswer::Ready(executable(root.path()))));
        let mut source = resolver(
            root.path(),
            store.clone(),
            Arc::new(ClaudeCredentials),
            HashMap::new(),
        );
        let mut runtime = source.config.runtime(AgentId::Claude).unwrap().clone();
        runtime.model = model.into();
        runtime.tools_enabled = agent == AgentId::Codex;
        std::fs::write(runtime.command.executable(), "adapter fixture").unwrap();
        source.config.runtimes.insert(agent.name().into(), runtime);
        source.managed_adapters.insert(agent);
        assert!(source
            .resolve_for(agent, model, ConversationApprovalMode::Ask)
            .await
            .unwrap()
            .is_some());
        assert!(matches!(
            source
                .resolve_for(agent, "not-a-model", ConversationApprovalMode::Ask)
                .await,
            Err(ConversationError::ModelUnavailable)
        ));
        let reads = store.reads.load(Ordering::SeqCst);
        assert!(matches!(
            source
                .resolve_for(agent, "not-a-model", ConversationApprovalMode::Auto)
                .await,
            Err(ConversationError::ApprovalModeUnavailable)
        ));
        assert_eq!(
            store.reads.load(Ordering::SeqCst),
            reads,
            "unsupported presets are refused before managed-store lookup"
        );
    }
}
