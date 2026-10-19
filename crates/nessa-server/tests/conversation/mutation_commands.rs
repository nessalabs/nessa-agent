//! Submit and exact-turn Stop use the same record runtime as creation.
use super::*;
use crate::conversation::{
    application::{
        ConversationAccessPolicyEvaluator, ConversationDependencies, ConversationLimits,
        ConversationRepository, ProviderSessionErasers, RequestedConversation,
    },
    domain::ConversationDeletion,
    infrastructure::LocalConversationStore,
};
use crate::conversation_test_support::{
    only, AcceptingCreationAudit, AcceptingDeletionAudit, AcceptingModeAudit, ObservedPolicy,
    Provider, ProviderFactory, RecordingFileLinkAudit, TestClock, DELETION_BUDGETS,
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_sdk::application::agent_execution::permissions::{
    PermissionRules, RememberError, RememberRequest, SettledRule,
};
use nessa_sdk::application::agent_execution::{
    agents::{AgentError, AttachmentPhase},
    commands::CreationFailure,
    sessions::{SessionSnapshot, SessionStorage},
};
use nessa_sdk::domain::agent_execution::executions::{ExecutionId, Initiation};
use nessa_sdk::infrastructure::session_storage::{RecordStorage, RuntimeMessageCommitClock};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::oneshot;

fn caller_on(request: &str, surface: &str) -> ConversationCaller {
    ConversationCaller {
        organization_id: OrganizationId::new("org").unwrap(),
        principal_id: PrincipalId::new("person").unwrap(),
        surface_id: surface.into(),
        action_id: request.into(),
    }
}
fn caller(request: &str) -> ConversationCaller {
    caller_on(request, "panel")
}
fn id() -> ConversationId {
    ConversationId::new(&uuid::Uuid::new_v4().to_string()).unwrap()
}
fn message(text: &str) -> SubmittedMessage {
    SubmittedMessage {
        text: text.into(),
        ..SubmittedMessage::default()
    }
}
fn image_message(media_type: &str, size: u64) -> SubmittedMessage {
    SubmittedMessage {
        text: "hello".into(),
        images: vec![crate::conversation::application::SubmittedImage {
            digest: "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                .into(),
            media_type: media_type.into(),
            size,
        }],
        ..SubmittedMessage::default()
    }
}
fn fixture(
    root: &Path,
    provider: Arc<ProviderFactory>,
) -> (
    ConversationService,
    Arc<RecordStorage>,
    Arc<LocalConversationStore>,
) {
    fixture_in(
        root,
        provider,
        crate::conversation::infrastructure::in_process_environment().into(),
    )
}

/// [`fixture`], with every environment its conversations may run in.
fn fixture_in(
    root: &Path,
    provider: Arc<ProviderFactory>,
    environment: crate::conversation::application::Environments,
) -> (
    ConversationService,
    Arc<RecordStorage>,
    Arc<LocalConversationStore>,
) {
    fixture_admitted_by(
        root,
        provider,
        environment,
        crate::conversation_test_support::access_policy(),
    )
}

/// [`fixture_in`], its service admitted by `policy`.
fn fixture_admitted_by(
    root: &Path,
    provider: Arc<ProviderFactory>,
    environment: crate::conversation::application::Environments,
    policy: Arc<dyn ConversationAccessPolicyEvaluator>,
) -> (
    ConversationService,
    Arc<RecordStorage>,
    Arc<LocalConversationStore>,
) {
    fixture_with_sessions(
        root,
        provider,
        environment,
        policy,
        ProviderSessionErasers::default(),
    )
}

fn fixture_with_sessions(
    root: &Path,
    provider: Arc<ProviderFactory>,
    environment: crate::conversation::application::Environments,
    policy: Arc<dyn ConversationAccessPolicyEvaluator>,
    provider_sessions: ProviderSessionErasers,
) -> (
    ConversationService,
    Arc<RecordStorage>,
    Arc<LocalConversationStore>,
) {
    nessa_local_storage::create_directory(root).unwrap();
    let storage = Arc::new(RecordStorage::new(root.join("records")).unwrap());
    let metadata = Arc::new(
        LocalConversationStore::open(&root.join("metadata.sqlite3"))
            .unwrap()
            .with_access_policy(crate::conversation_test_support::access_policy()),
    );
    let service = ConversationService::new(
        ConversationDependencies {
            access_policy: policy,
            agents: only(Arc::new(Provider::new(provider))),
            storage: storage.clone(),
            metadata: metadata.clone(),
            mode_audit: Arc::new(AcceptingModeAudit),
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            summaries: metadata.clone(),
            listing: metadata.clone(),
            deletion_audit: Arc::new(AcceptingDeletionAudit),
            provider_sessions,
            deletion_budgets: DELETION_BUDGETS,
            message_commit_clock: Arc::new(RuntimeMessageCommitClock::new()),
            clock: Arc::new(TestClock),
            environment,
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    (service, storage, metadata)
}
async fn retire(
    service: ConversationService,
    storage: Arc<RecordStorage>,
    metadata: Arc<LocalConversationStore>,
) {
    service.shutdown().await.unwrap();
    drop(service);
    storage.shutdown().await.unwrap();
    drop(storage);
    drop(metadata);
}
fn command_rows(root: &Path) -> i64 {
    let path = root.join("records/records.sqlite3");
    if !path.exists() {
        return 0;
    }
    let db = nessa_local_database::rusqlite::Connection::open(path).unwrap();
    db.busy_timeout(Duration::from_secs(5)).unwrap();
    db.query_row(
        "SELECT COUNT(*) FROM event_records WHERE schema_id = 'nessa.command'",
        [],
        |row| row.get(0),
    )
    .unwrap()
}
async fn opened(
    service: &ConversationService,
    provider: &ProviderFactory,
    target: &ConversationId,
) {
    service
        .create(
            target.clone(),
            caller("create"),
            RequestedConversation::default(),
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if provider.open_calls.load(Ordering::SeqCst) >= 1 {
                let live = service.resolve(target, &caller("create")).await.unwrap();
                if live.agent.attachment_status().phase() == AttachmentPhase::Attached {
                    break;
                }
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
async fn finish_turn(
    service: &ConversationService,
    provider: &ProviderFactory,
    target: &ConversationId,
    execution: &str,
) {
    send(service, target, execution, "finished").await;
    let turn = nessa_sdk::domain::agent_execution::executions::ExecutionId::new(execution).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let started = provider
                .executions
                .lock()
                .unwrap()
                .iter()
                .any(|id| id == execution);
            if started {
                let live = service.resolve(target, &caller("create")).await.unwrap();
                if super::turn_is_final(&live, &turn).await == Some(true) {
                    break;
                }
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
async fn send(service: &ConversationService, target: &ConversationId, execution: &str, text: &str) {
    service
        .submit(
            target.clone(),
            caller(execution),
            execution.into(),
            message(text),
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
}
/// Hang ceiling while a held turn is still reaching the provider.
///
/// The wait is `execution_started`, armed before submit. Five seconds expired
/// on a loaded Windows runner before that notify (#558).
const TURN_HOLD_HANG: Duration = Duration::from_secs(60);
async fn hold_turn(
    service: &ConversationService,
    provider: &ProviderFactory,
    target: &ConversationId,
    execution: &str,
) -> oneshot::Sender<()> {
    let (release, gate) = oneshot::channel();
    *provider.execution_gate.lock().unwrap() = Some(gate);
    let started = provider.execution_started.notified();
    let pending = {
        let service = service.clone();
        let target = target.clone();
        let execution = execution.to_owned();
        tokio::spawn(async move { send(&service, &target, &execution, "active").await })
    };
    if tokio::time::timeout(TURN_HOLD_HANG, started).await.is_err() {
        panic!(
            "held turn had not started after {TURN_HOLD_HANG:?}; submit finished={} executions={:?}",
            pending.is_finished(),
            provider.executions.lock().unwrap()
        );
    }
    match tokio::time::timeout(TURN_HOLD_HANG, pending).await {
        Ok(joined) => joined.expect("held turn submit panicked"),
        Err(_) => panic!(
            "held turn submit had not finished after {TURN_HOLD_HANG:?}; executions={:?}",
            provider.executions.lock().unwrap()
        ),
    }
    release
}
async fn tombstone(metadata: &LocalConversationStore, target: &ConversationId) {
    metadata
        .record_deletion(
            target,
            ConversationDeletion::new(
                OrganizationId::new("org").unwrap(),
                PrincipalId::new("person").unwrap(),
                "panel".into(),
                "delete".into(),
                1_700_000_000_200,
            )
            .unwrap(),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn submit_commits_the_attempt_before_enqueue() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let target = id();
    opened(&service, &provider, &target).await;
    let mode = service.inner.mode_changes.lock(&target).await;
    let task = {
        let service = service.clone();
        let storage = storage.clone();
        let target = target.clone();
        tokio::spawn(async move {
            service
                .submit_command(
                    storage,
                    target,
                    caller("submit"),
                    Initiation::owner(),
                    true,
                    "turn".into(),
                    message("hello"),
                    SubmissionMode::Queue,
                )
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if command_rows(&root) == 2 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(provider.executions.lock().unwrap().is_empty());
    drop(mode);
    let receipt = task.await.unwrap().unwrap();
    assert_eq!(receipt.progress.stage(), MutationStage::Settled);
    assert_eq!(
        receipt.progress.outcome(),
        Some(MutationOutcome::Dispatched)
    );
    tokio::time::timeout(
        Duration::from_secs(5),
        provider.execution_started.notified(),
    )
    .await
    .unwrap();
    assert_eq!(provider.executions.lock().unwrap().as_slice(), ["turn"]);
    assert_eq!(provider.close_calls.load(Ordering::SeqCst), 0);
    retire(service, storage, metadata).await;
}

/// Holds the first command opened here until `release`, telling `opening`.
struct HoldFirstOpen {
    storage: Arc<RecordStorage>,
    held: AtomicBool,
    opening: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}
impl nessa_sdk::application::agent_execution::commands::CreationStorage for HoldFirstOpen {
    fn open_creation(
        &self,
        principal: String,
        request: String,
        create: bool,
    ) -> nessa_sdk::application::agent_execution::commands::CreationFuture<
        '_,
        Option<Arc<dyn nessa_sdk::application::agent_execution::commands::CreationStorageLease>>,
    > {
        Box::pin(async move {
            if !self.held.swap(true, Ordering::SeqCst) {
                self.opening.notify_one();
                self.release.notified().await;
            }
            self.storage.open_creation(principal, request, create).await
        })
    }
}

/// A submit holds admission through its effect. Asking for it again there
/// queues the second read behind a retirement that waits on the first, so
/// shutdown stalled until it gave up waiting, and a running turn with it.
#[tokio::test]
async fn a_submit_admitted_before_retirement_does_not_hold_it_back() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let target = id();
    opened(&service, &provider, &target).await;
    let opening = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let held = Arc::new(HoldFirstOpen {
        storage: storage.clone(),
        held: AtomicBool::new(false),
        opening: opening.clone(),
        release: release.clone(),
    });
    let task = {
        let service = service.clone();
        let target = target.clone();
        tokio::spawn(async move {
            service
                .submit_command(
                    held,
                    target,
                    caller("submit"),
                    Initiation::owner(),
                    true,
                    "turn".into(),
                    message("hello"),
                    SubmissionMode::Queue,
                )
                .await
        })
    };
    // Admitted, and held before its effect.
    tokio::time::timeout(Duration::from_secs(5), opening.notified())
        .await
        .unwrap();
    let shutdown = {
        let service = service.clone();
        tokio::spawn(async move { service.shutdown().await })
    };
    // Retirement is fenced and waiting on the submit's admission.
    tokio::time::timeout(Duration::from_secs(5), async {
        while service.inner.retirement.get().is_none() || service.inner.admission.try_read().is_ok()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(5), shutdown)
        .await
        .expect("retirement is not held back by a submit it already admitted")
        .unwrap()
        .unwrap();
    // Fenced before its effect ran, the submit is refused with nothing taken.
    match task.await.unwrap() {
        Err(MutationFailure::Target(ConversationError::Unavailable)) => {}
        Err(other) => panic!("refused with {other:?}"),
        Ok(submitted) => panic!("dispatched: {:?}", submitted.progress.outcome()),
    }
    drop(service);
    storage.shutdown().await.unwrap();
    drop(storage);
    drop(metadata);
}

#[tokio::test]
async fn a_settled_submit_reopens_without_enqueueing_again() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let target = id();
    opened(&service, &provider, &target).await;
    let original = service
        .submit_command(
            storage.clone(),
            target.clone(),
            caller("submit"),
            Initiation::owner(),
            true,
            "turn".into(),
            message("hello"),
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    assert_eq!(
        original.progress.outcome(),
        Some(MutationOutcome::Dispatched)
    );
    let rows = command_rows(&root);
    retire(service, storage, metadata).await;
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let retry = service
        .submit_command(
            storage.clone(),
            target.clone(),
            caller("submit"),
            Initiation::owner(),
            true,
            "turn".into(),
            message("hello"),
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    assert_eq!(retry.progress, original.progress);
    assert!(provider.executions.lock().unwrap().is_empty());
    assert_eq!(command_rows(&root), rows);
    let looked = service
        .lookup_command(
            storage.clone(),
            target.clone(),
            caller("submit"),
            submit_lookup_binding(
                &target,
                &caller("submit"),
                "turn",
                &message("hello"),
                SubmissionMode::Queue,
            )
            .unwrap(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(looked, original.progress);
    assert_eq!(command_rows(&root), rows);
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn a_submit_identity_conflicts_with_changed_bytes_or_a_creation_request() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let target = id();
    service
        .create_command(
            storage.clone(),
            target.clone(),
            caller("shared"),
            RequestedConversation::default(),
        )
        .await
        .unwrap();
    assert!(matches!(
        service
            .submit_command(
                storage.clone(),
                target.clone(),
                caller("shared"),
                Initiation::owner(),
                true,
                "turn".into(),
                message("hello"),
                SubmissionMode::Queue,
            )
            .await,
        Err(MutationFailure::Conflict)
    ));
    assert!(provider.executions.lock().unwrap().is_empty());
    let other = id();
    opened(&service, &provider, &other).await;
    service
        .submit_command(
            storage.clone(),
            other.clone(),
            caller("submit"),
            Initiation::owner(),
            true,
            "turn".into(),
            message("hello"),
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    tokio::time::timeout(
        Duration::from_secs(5),
        provider.execution_started.notified(),
    )
    .await
    .unwrap();
    let queued = provider.executions.lock().unwrap().len();
    assert!(matches!(
        service
            .submit_command(
                storage.clone(),
                other.clone(),
                caller("submit"),
                Initiation::owner(),
                true,
                "turn".into(),
                message("different"),
                SubmissionMode::Queue,
            )
            .await,
        Err(MutationFailure::Conflict)
    ));
    assert!(matches!(
        service
            .submit_command(
                storage.clone(),
                id(),
                caller("submit"),
                Initiation::owner(),
                true,
                "other-turn".into(),
                message("hello"),
                SubmissionMode::Queue,
            )
            .await,
        Err(MutationFailure::Conflict)
    ));
    assert!(matches!(
        service
            .submit_command(
                storage.clone(),
                other.clone(),
                caller_on("submit", "phone"),
                Initiation::owner(),
                true,
                "turn".into(),
                message("hello"),
                SubmissionMode::Queue,
            )
            .await,
        Err(MutationFailure::Conflict)
    ));
    assert!(matches!(
        service
            .create_command(
                storage.clone(),
                other,
                caller("submit"),
                RequestedConversation::default(),
            )
            .await,
        Err(CreationFailure::Conflict)
    ));
    assert_eq!(provider.executions.lock().unwrap().len(), queued);
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 2);
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn a_changed_image_type_or_size_conflicts_with_the_saved_submit() {
    let directory = tempfile::tempdir().unwrap();
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(&directory.path().join("data"), provider.clone());
    let target = id();
    opened(&service, &provider, &target).await;
    let _ = service
        .submit_command(
            storage.clone(),
            target.clone(),
            caller("picture"),
            Initiation::owner(),
            true,
            "turn".into(),
            image_message("image/png", 4),
            SubmissionMode::Queue,
        )
        .await;
    for changed in [
        image_message("image/jpeg", 4),
        image_message("image/png", 5),
    ] {
        match service
            .submit_command(
                storage.clone(),
                target.clone(),
                caller("picture"),
                Initiation::owner(),
                true,
                "turn".into(),
                changed,
                SubmissionMode::Queue,
            )
            .await
        {
            Err(MutationFailure::Conflict) => {}
            Err(MutationFailure::Interrupted(_)) => panic!("changed image bytes were interrupted"),
            Err(MutationFailure::Target(error)) => panic!("changed image bytes target {error}"),
            Err(MutationFailure::Storage(_)) => panic!("changed image bytes storage"),
            Err(MutationFailure::TaskFault(_)) => panic!("changed image bytes fault"),
            Ok(_) => panic!("changed image bytes enqueued"),
        }
    }
    retire(service, storage, metadata).await;
}

#[test]
#[ignore = "invoked only by the submit crash parent"]
fn submit_crash_child() {
    let root = PathBuf::from(std::env::var_os("NESSA_MUTATION_ROOT").unwrap());
    let target = std::env::var("NESSA_MUTATION_TARGET").unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let provider = Arc::new(ProviderFactory::default());
        let (service, storage, _metadata) = fixture(&root, provider.clone());
        let target = ConversationId::new(&target).unwrap();
        opened(&service, &provider, &target).await;
        let _mode = service.inner.mode_changes.lock(&target).await;
        let storage_for_submit = storage.clone();
        let service_for_submit = service.clone();
        let target_for_submit = target.clone();
        tokio::spawn(async move {
            let _ = service_for_submit
                .submit_command(
                    storage_for_submit,
                    target_for_submit,
                    caller("submit"),
                    Initiation::owner(),
                    true,
                    "turn".into(),
                    message("hello"),
                    SubmissionMode::Queue,
                )
                .await;
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if command_rows(&root) == 2 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(provider.executions.lock().unwrap().is_empty());
        let marker = root.join("submit-attempted.pending");
        std::fs::write(&marker, b"1").unwrap();
        std::fs::rename(marker, root.join("submit-attempted")).unwrap();
        std::future::pending::<()>().await;
    });
}

#[tokio::test]
async fn an_interrupted_submit_does_not_enqueue_again() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let target = id();
    let log = directory.path().join("submit-crash.log");
    let output = std::fs::File::create(&log).unwrap();
    let mut child = ChildOwner(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "conversation::application::service::mutation::tests::submit_crash_child",
                "--ignored",
                "--nocapture",
            ])
            .env("NESSA_MUTATION_ROOT", &root)
            .env("NESSA_MUTATION_TARGET", target.to_string())
            .stdout(Stdio::from(output.try_clone().unwrap()))
            .stderr(Stdio::from(output))
            .spawn()
            .unwrap(),
    );
    wait_marker(&root, "submit-attempted", &mut child, &log).await;
    child.0.kill().unwrap();
    assert!(!child.0.wait().unwrap().success());
    drop(child);
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let saved = storage
        .open_existing(conversation_session(&target))
        .await
        .unwrap()
        .unwrap();
    let snapshot = SessionSnapshot::load_saved(saved.as_ref(), &conversation_session(&target))
        .await
        .unwrap()
        .unwrap();
    assert!(snapshot
        .invocations
        .iter()
        .all(|record| record.request.execution_id.as_str() != "turn"));
    drop(saved);
    assert!(matches!(
        service
            .submit_command(
                storage.clone(),
                target.clone(),
                caller("submit"),
                Initiation::owner(),
                true,
                "turn".into(),
                message("hello"),
                SubmissionMode::Queue,
            )
            .await,
        Err(MutationFailure::Interrupted(_))
    ));
    assert!(provider.executions.lock().unwrap().is_empty());
    assert_eq!(command_rows(&root), 2);
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn deletion_refuses_submit_lookup_after_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let target = id();
    opened(&service, &provider, &target).await;
    service
        .submit_command(
            storage.clone(),
            target.clone(),
            caller("submit"),
            Initiation::owner(),
            true,
            "turn".into(),
            message("hello"),
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    let rows = command_rows(&root);
    tombstone(&metadata, &target).await;
    retire(service, storage, metadata).await;
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let binding = submit_lookup_binding(
        &target,
        &caller("submit"),
        "turn",
        &message("hello"),
        SubmissionMode::Queue,
    )
    .unwrap();
    assert!(matches!(
        service
            .lookup_command(storage.clone(), target.clone(), caller("submit"), binding)
            .await,
        Err(MutationFailure::Target(ConversationError::Deleted))
    ));
    assert!(matches!(
        service
            .submit_command(
                storage.clone(),
                target,
                caller("submit"),
                Initiation::owner(),
                true,
                "turn".into(),
                message("hello"),
                SubmissionMode::Queue,
            )
            .await,
        Err(MutationFailure::Target(ConversationError::Deleted))
    ));
    assert!(provider.executions.lock().unwrap().is_empty());
    assert_eq!(command_rows(&root), rows);
    retire(service, storage, metadata).await;
}

/// A submit or a stop whose target the policy cannot decide is refused as
/// storage trouble before any stage: nothing is recorded or sent, rather
/// than a deleted conversation being let through as one it could not tell
/// about.
#[tokio::test]
async fn a_command_the_policy_cannot_decide_is_refused_before_any_stage() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let policy = Arc::new(ObservedPolicy::default());
    let (service, storage, metadata) = fixture_admitted_by(
        &root,
        provider.clone(),
        crate::conversation::infrastructure::in_process_environment().into(),
        policy.clone(),
    );
    let target = id();
    opened(&service, &provider, &target).await;
    let rows = command_rows(&root);
    policy.undecide();
    assert!(matches!(
        service
            .submit_command(
                storage.clone(),
                target.clone(),
                caller("submit"),
                Initiation::owner(),
                true,
                "turn".into(),
                message("hello"),
                SubmissionMode::Queue,
            )
            .await,
        Err(MutationFailure::Target(ConversationError::Metadata))
    ));
    assert!(matches!(
        service
            .stop_command(storage.clone(), target, caller("stop"), "turn".into())
            .await,
        Err(MutationFailure::Target(ConversationError::Metadata))
    ));
    assert!(provider.executions.lock().unwrap().is_empty());
    assert_eq!(command_rows(&root), rows);
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn stop_withdraws_only_the_named_queued_turn() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let target = id();
    opened(&service, &provider, &target).await;
    let release = hold_turn(&service, &provider, &target, "active").await;
    send(&service, &target, "queued", "later").await;
    let live = service.resolve(&target, &caller("create")).await.unwrap();
    assert!(live
        .agent
        .queued_ids()
        .await
        .iter()
        .any(|queued| queued.as_str() == "queued"));
    let receipt = service
        .stop_command(
            storage.clone(),
            target.clone(),
            caller("stop"),
            "queued".into(),
        )
        .await
        .unwrap();
    assert_eq!(receipt.outcome(), Some(MutationOutcome::Withdrawn));
    assert!(live
        .agent
        .queued_ids()
        .await
        .iter()
        .all(|queued| queued.as_str() != "queued"));
    assert_eq!(
        live.agent
            .active_execution_id()
            .as_ref()
            .map(|id| id.as_str()),
        Some("active")
    );
    assert_eq!(provider.cancel_calls.load(Ordering::SeqCst), 0);
    assert_eq!(provider.close_calls.load(Ordering::SeqCst), 0);
    drop(release);
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn stop_cancels_the_active_turn_without_closing_the_attachment() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    provider.turn_cancel.store(true, Ordering::SeqCst);
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let target = id();
    opened(&service, &provider, &target).await;
    let release_execution = hold_turn(&service, &provider, &target, "active").await;
    let (release_cancel, gate) = oneshot::channel();
    *provider.cancel_gate.lock().unwrap() = Some(gate);
    let task = {
        let service = service.clone();
        let storage = storage.clone();
        let target = target.clone();
        tokio::spawn(async move {
            service
                .stop_command(storage, target, caller("stop"), "active".into())
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(5), provider.cancel_started.notified())
        .await
        .unwrap();
    assert_eq!(command_rows(&root), 2);
    assert_eq!(provider.close_calls.load(Ordering::SeqCst), 0);
    assert_eq!(provider.cancel_turns.lock().unwrap().as_slice(), ["active"]);
    release_cancel.send(()).unwrap();
    let receipt = task.await.unwrap().unwrap();
    assert_eq!(receipt.stage(), MutationStage::Settled);
    assert_eq!(receipt.outcome(), Some(MutationOutcome::Cancelled));
    assert_eq!(provider.close_calls.load(Ordering::SeqCst), 0);
    assert_eq!(provider.cancel_calls.load(Ordering::SeqCst), 1);
    drop(release_execution);
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn a_settled_stop_reopens_without_cancelling_again() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    provider.turn_cancel.store(true, Ordering::SeqCst);
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let target = id();
    opened(&service, &provider, &target).await;
    let release = hold_turn(&service, &provider, &target, "active").await;
    let original = service
        .stop_command(
            storage.clone(),
            target.clone(),
            caller("stop"),
            "active".into(),
        )
        .await
        .unwrap();
    assert_eq!(original.outcome(), Some(MutationOutcome::Cancelled));
    assert_eq!(provider.cancel_calls.load(Ordering::SeqCst), 1);
    let rows = command_rows(&root);
    drop(release);
    retire(service, storage, metadata).await;
    let provider = Arc::new(ProviderFactory::default());
    provider.turn_cancel.store(true, Ordering::SeqCst);
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let retry = service
        .stop_command(
            storage.clone(),
            target.clone(),
            caller("stop"),
            "active".into(),
        )
        .await
        .unwrap();
    assert_eq!(retry, original);
    assert_eq!(provider.cancel_calls.load(Ordering::SeqCst), 0);
    assert_eq!(provider.close_calls.load(Ordering::SeqCst), 0);
    assert_eq!(command_rows(&root), rows);
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn a_stop_identity_conflicts_before_any_effect() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    provider.turn_cancel.store(true, Ordering::SeqCst);
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let target = id();
    opened(&service, &provider, &target).await;
    let release = hold_turn(&service, &provider, &target, "active").await;
    send(&service, &target, "queued", "later").await;
    service
        .stop_command(
            storage.clone(),
            target.clone(),
            caller("stop"),
            "queued".into(),
        )
        .await
        .unwrap();
    assert!(matches!(
        service
            .stop_command(
                storage.clone(),
                target.clone(),
                caller("stop"),
                "active".into(),
            )
            .await,
        Err(MutationFailure::Conflict)
    ));
    assert!(matches!(
        service
            .stop_command(storage.clone(), id(), caller("stop"), "queued".into(),)
            .await,
        Err(MutationFailure::Conflict)
    ));
    assert!(matches!(
        service
            .stop_command(
                storage.clone(),
                target.clone(),
                caller_on("stop", "phone"),
                "queued".into(),
            )
            .await,
        Err(MutationFailure::Conflict)
    ));
    assert!(matches!(
        service
            .create_command(
                storage.clone(),
                target,
                caller("stop"),
                RequestedConversation::default(),
            )
            .await,
        Err(CreationFailure::Conflict)
    ));
    assert_eq!(provider.cancel_calls.load(Ordering::SeqCst), 0);
    assert_eq!(provider.close_calls.load(Ordering::SeqCst), 0);
    drop(release);
    retire(service, storage, metadata).await;
}

#[test]
#[ignore = "invoked only by the stop crash parent"]
fn stop_crash_child() {
    let root = PathBuf::from(std::env::var_os("NESSA_MUTATION_ROOT").unwrap());
    let target = std::env::var("NESSA_MUTATION_TARGET").unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let provider = Arc::new(ProviderFactory::default());
        provider.turn_cancel.store(true, Ordering::SeqCst);
        let (release_cancel, gate) = oneshot::channel();
        *provider.cancel_gate.lock().unwrap() = Some(gate);
        let (service, storage, _metadata) = fixture(&root, provider.clone());
        let target = ConversationId::new(&target).unwrap();
        opened(&service, &provider, &target).await;
        let _release_execution = hold_turn(&service, &provider, &target, "active").await;
        tokio::spawn({
            let service = service.clone();
            let storage = storage.clone();
            let target = target.clone();
            async move {
                let _ = service
                    .stop_command(storage, target, caller("stop"), "active".into())
                    .await;
            }
        });
        tokio::time::timeout(Duration::from_secs(5), provider.cancel_started.notified())
            .await
            .unwrap();
        assert_eq!(command_rows(&root), 2);
        let marker = root.join("stop-attempted.pending");
        std::fs::write(&marker, b"1").unwrap();
        std::fs::rename(marker, root.join("stop-attempted")).unwrap();
        // Hold the cancel until this process is killed. Releasing the gate
        // lets the attempt settle before the signal arrives, and the parent
        // then sees a finished stop.
        let _held_cancel = release_cancel;
        std::future::pending::<()>().await;
    });
}

#[tokio::test]
async fn an_interrupted_stop_does_not_cancel_again() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let target = id();
    let log = directory.path().join("stop-crash.log");
    let output = std::fs::File::create(&log).unwrap();
    let mut child = ChildOwner(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "conversation::application::service::mutation::tests::stop_crash_child",
                "--ignored",
                "--nocapture",
            ])
            .env("NESSA_MUTATION_ROOT", &root)
            .env("NESSA_MUTATION_TARGET", target.to_string())
            .stdout(Stdio::from(output.try_clone().unwrap()))
            .stderr(Stdio::from(output))
            .spawn()
            .unwrap(),
    );
    wait_marker(&root, "stop-attempted", &mut child, &log).await;
    child.0.kill().unwrap();
    assert!(!child.0.wait().unwrap().success());
    drop(child);
    let provider = Arc::new(ProviderFactory::default());
    provider.turn_cancel.store(true, Ordering::SeqCst);
    let (service, storage, metadata) = fixture(&root, provider.clone());
    assert!(matches!(
        service
            .stop_command(storage.clone(), target, caller("stop"), "active".into())
            .await,
        Err(MutationFailure::Interrupted(_))
    ));
    assert_eq!(provider.cancel_calls.load(Ordering::SeqCst), 0);
    assert_eq!(provider.close_calls.load(Ordering::SeqCst), 0);
    assert_eq!(command_rows(&root), 2);
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn stop_of_a_finished_turn_sends_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    provider.turn_cancel.store(true, Ordering::SeqCst);
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let target = id();
    opened(&service, &provider, &target).await;
    finish_turn(&service, &provider, &target, "done").await;
    let receipt = service
        .stop_command(storage.clone(), target, caller("stop"), "done".into())
        .await
        .unwrap();
    assert_eq!(receipt.stage(), MutationStage::Settled);
    assert_eq!(receipt.outcome(), Some(MutationOutcome::AlreadyFinal));
    assert_eq!(provider.cancel_calls.load(Ordering::SeqCst), 0);
    assert_eq!(provider.close_calls.load(Ordering::SeqCst), 0);
    assert_eq!(command_rows(&root), 2);
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn stop_refuses_an_unsupported_active_turn_before_attempting() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let target = id();
    opened(&service, &provider, &target).await;
    let release = hold_turn(&service, &provider, &target, "active").await;
    assert!(matches!(
        service
            .stop_command(
                storage.clone(),
                target.clone(),
                caller("stop"),
                "active".into(),
            )
            .await,
        Err(MutationFailure::Target(ConversationError::Agent(
            AgentError::Unsupported(_)
        )))
    ));
    assert_eq!(command_rows(&root), 1);
    assert_eq!(provider.cancel_calls.load(Ordering::SeqCst), 0);
    provider.turn_cancel.store(true, Ordering::SeqCst);
    let receipt = service
        .stop_command(storage.clone(), target, caller("stop"), "active".into())
        .await
        .unwrap();
    assert_eq!(receipt.outcome(), Some(MutationOutcome::Cancelled));
    assert_eq!(provider.cancel_calls.load(Ordering::SeqCst), 1);
    assert_eq!(provider.close_calls.load(Ordering::SeqCst), 0);
    drop(release);
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn deletion_refuses_stop_lookup_and_retry_after_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    provider.turn_cancel.store(true, Ordering::SeqCst);
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let target = id();
    opened(&service, &provider, &target).await;
    finish_turn(&service, &provider, &target, "done").await;
    let original = service
        .stop_command(
            storage.clone(),
            target.clone(),
            caller("stop"),
            "done".into(),
        )
        .await
        .unwrap();
    assert_eq!(original.outcome(), Some(MutationOutcome::AlreadyFinal));
    let rows = command_rows(&root);
    tombstone(&metadata, &target).await;
    retire(service, storage, metadata).await;
    let provider = Arc::new(ProviderFactory::default());
    provider.turn_cancel.store(true, Ordering::SeqCst);
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let binding = stop_lookup_binding(&target, &caller("stop"), "done").unwrap();
    assert!(matches!(
        service
            .lookup_command(storage.clone(), target.clone(), caller("stop"), binding)
            .await,
        Err(MutationFailure::Target(ConversationError::Deleted))
    ));
    assert!(matches!(
        service
            .stop_command(storage.clone(), target, caller("stop"), "done".into())
            .await,
        Err(MutationFailure::Target(ConversationError::Deleted))
    ));
    assert_eq!(provider.cancel_calls.load(Ordering::SeqCst), 0);
    assert_eq!(provider.close_calls.load(Ordering::SeqCst), 0);
    assert_eq!(command_rows(&root), rows);
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn read_only_stop_lookup_writes_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let target = id();
    let binding = stop_lookup_binding(&target, &caller("stop"), "done").unwrap();
    assert!(matches!(
        service
            .lookup_command(
                storage.clone(),
                target.clone(),
                caller("stop"),
                binding.clone(),
            )
            .await,
        Err(MutationFailure::Target(ConversationError::NotFound))
    ));
    assert_eq!(command_rows(&root), 0);
    opened(&service, &provider, &target).await;
    assert!(service
        .lookup_command(
            storage.clone(),
            target.clone(),
            caller("stop"),
            binding.clone(),
        )
        .await
        .unwrap()
        .is_none());
    assert_eq!(command_rows(&root), 0);
    finish_turn(&service, &provider, &target, "done").await;
    let receipt = service
        .stop_command(
            storage.clone(),
            target.clone(),
            caller("stop"),
            "done".into(),
        )
        .await
        .unwrap();
    let rows = command_rows(&root);
    let looked = service
        .lookup_command(storage.clone(), target, caller("stop"), binding)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(looked, receipt);
    assert_eq!(command_rows(&root), rows);
    assert_eq!(provider.cancel_calls.load(Ordering::SeqCst), 0);
    assert_eq!(provider.close_calls.load(Ordering::SeqCst), 0);
    retire(service, storage, metadata).await;
}

struct ChildOwner(Child);
impl Drop for ChildOwner {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
async fn wait_marker(root: &Path, name: &str, child: &mut ChildOwner, log: &Path) {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            match std::fs::read(root.join(name)) {
                Ok(bytes) => {
                    assert_eq!(bytes, b"1");
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => panic!("cannot observe {name}: {error}"),
            }
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "crash fixture exited before {name}: {}",
                std::fs::read_to_string(log).unwrap_or_default()
            );
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

/// An environment that runs elsewhere and is never reached: what a
/// conversation placed on an SSH host refuses before it opens.
struct Elsewhere;

impl crate::conversation::application::Environment for Elsewhere {
    fn declaration(&self) -> crate::conversation::application::EnvironmentDeclaration {
        crate::conversation::application::EnvironmentDeclaration {
            environment: nessa_sdk::domain::agent_execution::leases::EnvironmentRef::Ssh(
                nessa_sdk::domain::agent_execution::leases::SshDestination::new("devbox").unwrap(),
            ),
            sandbox: nessa_sdk::domain::agent_execution::leases::SandboxProfiles::HARNESS_DEFAULT,
        }
    }
    fn open<'a>(
        &'a self,
        _lease: &'a nessa_sdk::domain::agent_execution::leases::LeaseId,
        _grant: &'a nessa_sdk::domain::agent_execution::leases::LeaseTerms,
        _binding: Arc<dyn nessa_sdk::application::agent_execution::providers::AgentProvider>,
    ) -> crate::conversation::application::EnvironmentFuture<
        'a,
        Result<
            crate::conversation::application::EnvironmentLease,
            nessa_sdk::domain::agent_execution::leases::LeaseRefusal,
        >,
    > {
        Box::pin(async {
            Err(nessa_sdk::domain::agent_execution::leases::LeaseRefusal::EnvironmentUnreachable)
        })
    }
    fn account<'a>(
        &'a self,
        _lease: &'a nessa_sdk::domain::agent_execution::leases::LeaseId,
    ) -> crate::conversation::application::EnvironmentFuture<
        'a,
        Option<nessa_sdk::domain::agent_execution::leases::LeaseCleanup>,
    > {
        Box::pin(async { None })
    }
}

/// The wire's submit refuses a file linked by path for a conversation on a
/// host before its command is staged, so the corrected request can be sent
/// again under the same identity.
#[tokio::test]
async fn a_submit_linking_a_file_is_refused_on_a_host_before_it_is_staged() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    nessa_local_storage::create_directory(&root).unwrap();
    let placements = Arc::new(
        crate::conversation::infrastructure::FilePlacements::new(root.join("placements")).unwrap(),
    );
    let environments = crate::conversation::application::Environments::new(
        crate::conversation::infrastructure::in_process_environment(),
        std::collections::BTreeMap::from([(
            "devbox".to_string(),
            Arc::new(Elsewhere) as Arc<dyn crate::conversation::application::Environment>,
        )]),
        placements,
    );
    let (service, storage, metadata) = fixture_in(&root, provider.clone(), environments);
    let target = id();
    service
        .inner
        .environment
        .place(&target, Some("devbox"))
        .await
        .unwrap();
    let mut linking = message("read this");
    linking.files = vec![crate::conversation::application::SubmittedFile {
        path: "/Users/me/notes.txt".into(),
    }];
    let refused = service
        .submit_command(
            storage.clone(),
            target,
            caller("submit"),
            Initiation::owner(),
            true,
            "turn".into(),
            linking,
            SubmissionMode::Queue,
        )
        .await;
    assert!(matches!(
        refused,
        Err(MutationFailure::Target(
            ConversationError::LinkedFileUnreachable
        ))
    ));
    assert_eq!(command_rows(&root), 0);
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 0);
    retire(service, storage, metadata).await;
}

/// An owner initiation is not enough. The membership flag is what arms the
/// remember offer, so a member whose turn is still saved as the owner's is
/// not offered a choice they cannot use.
#[tokio::test]
async fn an_owner_turn_offers_remember_only_when_the_membership_may() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let rules = Arc::new(RecordingRules::default());
    service.bind_permission_rules(rules.clone());
    let target = id();
    let conversation = target.to_string();
    opened(&service, &provider, &target).await;
    service
        .submit_command(
            storage.clone(),
            target.clone(),
            caller("submit"),
            Initiation::owner(),
            false,
            "hidden".into(),
            message("hello"),
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    assert!(!rules.offers_remember(&conversation, &ExecutionId::new("hidden").unwrap()));
    service
        .submit_command(
            storage.clone(),
            target,
            caller("again"),
            Initiation::owner(),
            true,
            "offered".into(),
            message("again"),
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    assert!(rules.offers_remember(&conversation, &ExecutionId::new("offered").unwrap()));
    retire(service, storage, metadata).await;
}

#[derive(Default)]
struct RecordingRules {
    armed: std::sync::Mutex<Vec<(String, String, bool)>>,
}

impl PermissionRules for RecordingRules {
    fn settle(
        &self,
        _: nessa_sdk::application::agent_execution::permissions::PermissionCaller,
        _: &str,
        _: &str,
        _: &str,
    ) -> Option<SettledRule> {
        None
    }
    fn offers_remember(&self, conversation_id: &str, execution: &ExecutionId) -> bool {
        self.armed
            .lock()
            .unwrap()
            .iter()
            .find(|(conversation, id, _)| {
                conversation == conversation_id && id == execution.as_str()
            })
            .is_some_and(|(_, _, offered)| *offered)
    }
    fn arm(&self, conversation_id: &str, execution: &ExecutionId, owner: bool) {
        self.armed.lock().unwrap().push((
            conversation_id.to_owned(),
            execution.as_str().to_owned(),
            owner,
        ));
    }
    fn remember(&self, _: RememberRequest<'_>) -> Result<SettledRule, RememberError> {
        Err(RememberError::Refused)
    }
}

#[path = "agent_tools.rs"]
mod agent_tool_tests;
/// The display can evict the Running parent after many Injected children.
/// Activity selection belongs to the live lifecycle even in that state.
#[tokio::test]
async fn active_execution_ignores_truncated_display_and_opens_no_provider() {
    let directory = tempfile::tempdir().unwrap();
    // The temporary locator's mode follows umask. Storage owns a private child.
    let root = directory.path().join("conversations");
    nessa_local_storage::create_directory(&root).unwrap();
    let provider = Arc::new(ProviderFactory::default());
    let (service, _, _) = fixture(&root, provider.clone());
    let target = id();
    opened(&service, &provider, &target).await;
    assert_eq!(
        service
            .active_execution(target.clone(), caller("peek"))
            .await
            .unwrap(),
        None
    );
    let release = hold_turn(&service, &provider, &target, "running-parent").await;
    let live = service.resolve(&target, &caller("read")).await.unwrap();
    {
        let mut projection = live.projection.lock().await;
        for index in 0..30 {
            projection.injected(&format!("steering-{index}"), "running-parent");
        }
        assert!(projection.view().truncated);
        assert!(!projection
            .view()
            .messages
            .iter()
            .any(|message| message.execution_id == "running-parent"));
    }
    let opens = provider.open_calls.load(Ordering::SeqCst);
    assert_eq!(
        service
            .active_execution(target.clone(), caller("peek"))
            .await
            .unwrap(),
        Some("running-parent".into())
    );
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), opens);
    let mut foreign = caller("foreign");
    foreign.principal_id = PrincipalId::new("other").unwrap();
    assert!(service.active_execution(target, foreign).await.is_err());
    let _ = release.send(());
    service.shutdown().await.unwrap();
}

#[path = "parent_stop.rs"]
mod parent_stop;
/// A turn leaving the queue is for a moment neither queued nor running: it
/// was selected and is being prepared. A stop that meets it then waits for it
/// to start, and stops it, instead of answering that the conversation was
/// not found. Found by the conversation chaos check.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_that_meets_a_turn_leaving_the_queue_stops_it() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    provider.turn_cancel.store(true, Ordering::SeqCst);
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let target = id();
    opened(&service, &provider, &target).await;
    let began = Arc::new(tokio::sync::Notify::new());
    let (prepare, prepare_gate) = oneshot::channel();
    *provider.prepare_gate.lock().unwrap() = Some((began.clone(), prepare_gate));
    let (release, execution_gate) = oneshot::channel::<()>();
    *provider.execution_gate.lock().unwrap() = Some(execution_gate);
    let preparing = began.notified();
    send(&service, &target, "moving", "now").await;
    tokio::time::timeout(Duration::from_secs(5), preparing)
        .await
        .expect("the turn is being prepared");
    let live = service.resolve(&target, &caller("create")).await.unwrap();
    assert!(live
        .agent
        .queued_ids()
        .await
        .iter()
        .all(|queued| queued.as_str() != "moving"));
    let stopping = {
        let service = service.clone();
        let storage = storage.clone();
        let target = target.clone();
        tokio::spawn(async move {
            service
                .stop_command(storage, target, caller("stop"), "moving".into())
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    prepare.send(()).unwrap();
    let receipt = tokio::time::timeout(Duration::from_secs(5), stopping)
        .await
        .unwrap()
        .unwrap();
    let outcome = receipt.as_ref().ok().and_then(|receipt| receipt.outcome());
    assert!(
        matches!(
            outcome,
            Some(MutationOutcome::Cancelled | MutationOutcome::AlreadyFinal)
        ),
        "{:?}",
        receipt.map(|receipt| receipt.outcome())
    );
    drop(release);
    drop(live);
    retire(service, storage, metadata).await;
}

/// A stop whose cancel finds the agent ending (a close tearing it down) while
/// its turn is still listed as running is answered by how the turn ends, not
/// by the cancel's refusal.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_whose_cancel_meets_the_agent_ending_is_answered_by_the_turns_end() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    provider.turn_cancel.store(true, Ordering::SeqCst);
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let target = id();
    opened(&service, &provider, &target).await;
    let (release, execution_gate) = oneshot::channel::<()>();
    *provider.execution_gate.lock().unwrap() = Some(execution_gate);
    *provider.cancel_failure.lock().unwrap() = Some(AgentError::AttachmentAuthorizationStale);
    provider.cancel_failure_ends.store(true, Ordering::SeqCst);
    let started = provider.execution_started.notified();
    send(&service, &target, "ending", "now").await;
    tokio::time::timeout(Duration::from_secs(5), started)
        .await
        .expect("the turn runs");
    let cancelling = provider.cancel_started.notified();
    let stopping = {
        let service = service.clone();
        let storage = storage.clone();
        let target = target.clone();
        tokio::spawn(async move {
            service
                .stop_command(storage, target, caller("stop"), "ending".into())
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(5), cancelling)
        .await
        .expect("the cancel reaches the agent");
    // The cancel has failed with the turn still running; then it ends.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!stopping.is_finished(), "the stop waits for the turn's end");
    release.send(()).unwrap();
    let receipt = tokio::time::timeout(Duration::from_secs(5), stopping)
        .await
        .unwrap()
        .unwrap();
    let outcome = receipt.as_ref().ok().and_then(|receipt| receipt.outcome());
    assert!(
        matches!(outcome, Some(MutationOutcome::AlreadyFinal)),
        "{:?}",
        receipt.map(|receipt| receipt.outcome())
    );
    retire(service, storage, metadata).await;
}

/// A stop whose cancel the provider refuses because the turn has not reached
/// it yet (the agent lists it running; the provider has not started it) asks
/// again and stops it, rather than answering with the refusal.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_whose_cancel_meets_the_turn_not_yet_in_the_provider_stops_it() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    provider.turn_cancel.store(true, Ordering::SeqCst);
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let target = id();
    opened(&service, &provider, &target).await;
    let (release, execution_gate) = oneshot::channel::<()>();
    *provider.execution_gate.lock().unwrap() = Some(execution_gate);
    // Refused once with the session usable, as the ACP worker refuses a
    // turn it has not started.
    *provider.cancel_failure.lock().unwrap() =
        Some(AgentError::Unsupported("turn is not active".into()));
    let started = provider.execution_started.notified();
    send(&service, &target, "early", "now").await;
    tokio::time::timeout(Duration::from_secs(5), started)
        .await
        .expect("the turn runs");
    let receipt = tokio::time::timeout(
        Duration::from_secs(5),
        service.stop_command(
            storage.clone(),
            target.clone(),
            caller("stop"),
            "early".into(),
        ),
    )
    .await
    .unwrap();
    let outcome = receipt.as_ref().ok().and_then(|receipt| receipt.outcome());
    assert!(
        matches!(outcome, Some(MutationOutcome::Cancelled)),
        "{:?}",
        receipt.map(|receipt| receipt.outcome())
    );
    assert_eq!(provider.cancel_calls.load(Ordering::SeqCst), 2);
    drop(release);
    retire(service, storage, metadata).await;
}

/// A stop that meets a close of its conversation, the turn still listed
/// running while the close takes its attachment away, is answered by how the
/// turn ends rather than refused as a provider that cannot cancel one turn.
/// Found by the conversation chaos check.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_that_meets_a_close_is_answered_by_the_turns_end() {
    use nessa_sdk::application::agent_execution::agents::AttachmentPhase;
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    provider.turn_cancel.store(true, Ordering::SeqCst);
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let target = id();
    opened(&service, &provider, &target).await;
    let release = hold_turn(&service, &provider, &target, "closing").await;
    let live = service.resolve(&target, &caller("create")).await.unwrap();
    let closing = {
        let service = service.clone();
        let target = target.clone();
        tokio::spawn(async move { service.close(target, caller("close")).await })
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        while matches!(
            live.agent.attachment_status().phase(),
            AttachmentPhase::Attached
        ) || live.agent.active_execution_id().is_none()
        {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("the close takes the attachment while the turn is listed running");
    let stopping = {
        let service = service.clone();
        let storage = storage.clone();
        let target = target.clone();
        tokio::spawn(async move {
            service
                .stop_command(storage, target, caller("stop"), "closing".into())
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(20)).await;
    drop(release);
    let receipt = tokio::time::timeout(Duration::from_secs(5), stopping)
        .await
        .unwrap()
        .unwrap();
    let outcome = receipt.as_ref().ok().and_then(|receipt| receipt.outcome());
    assert!(
        matches!(
            outcome,
            Some(MutationOutcome::Cancelled | MutationOutcome::AlreadyFinal)
        ) || matches!(
            receipt,
            Err(MutationFailure::Target(ConversationError::Agent(
                AgentError::Closed
            )))
        ),
        "{:?}",
        receipt.map(|receipt| receipt.outcome())
    );
    let _ = tokio::time::timeout(Duration::from_secs(5), closing).await;
    drop(live);
    retire(service, storage, metadata).await;
}

/// A stop whose cancel reaches the turn as it ends is answered by how the
/// turn ended, not by the cancel's refusal of a turn no longer running.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_whose_cancel_meets_the_turn_ending_is_answered_by_its_end() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    provider.turn_cancel.store(true, Ordering::SeqCst);
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let target = id();
    opened(&service, &provider, &target).await;
    let (release, execution_gate) = oneshot::channel::<()>();
    *provider.execution_gate.lock().unwrap() = Some(execution_gate);
    let (let_cancel, cancel_gate) = oneshot::channel::<()>();
    *provider.cancel_gate.lock().unwrap() = Some(cancel_gate);
    *provider.cancel_failure.lock().unwrap() = Some(AgentError::Closed);
    let started = provider.execution_started.notified();
    send(&service, &target, "ending", "now").await;
    tokio::time::timeout(Duration::from_secs(5), started)
        .await
        .expect("the turn runs");
    let cancelling = provider.cancel_started.notified();
    let stopping = {
        let service = service.clone();
        let storage = storage.clone();
        let target = target.clone();
        tokio::spawn(async move {
            service
                .stop_command(storage, target, caller("stop"), "ending".into())
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(5), cancelling)
        .await
        .expect("the cancel reaches the agent");
    release.send(()).unwrap();
    let live = service.resolve(&target, &caller("create")).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while live.agent.active_execution_id().is_some() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the turn ends");
    let_cancel.send(()).unwrap();
    let receipt = tokio::time::timeout(Duration::from_secs(5), stopping)
        .await
        .unwrap()
        .unwrap();
    let outcome = receipt.as_ref().ok().and_then(|receipt| receipt.outcome());
    assert!(
        matches!(
            outcome,
            Some(MutationOutcome::Cancelled | MutationOutcome::AlreadyFinal)
        ),
        "{:?}",
        receipt.map(|receipt| receipt.outcome())
    );
    drop(live);
    retire(service, storage, metadata).await;
}
