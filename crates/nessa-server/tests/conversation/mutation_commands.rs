//! Submit and exact-turn Stop use the same record runtime as creation.
use super::*;
use crate::conversation::{
    application::{
        ConversationDependencies, ConversationLimits, ConversationRepository,
        ProviderSessionErasers, RequestedConversation,
    },
    domain::ConversationDeletion,
    infrastructure::LocalConversationStore,
};
use crate::conversation_test_support::{
    only, AcceptingCreationAudit, AcceptingDeletionAudit, AcceptingModeAudit, Provider,
    ProviderFactory, RecordingFileLinkAudit, TestClock, DELETION_BUDGETS,
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_sdk::application::agent_execution::{
    agents::{AgentError, AttachmentPhase},
    commands::CreationFailure,
    sessions::{SessionSnapshot, SessionStorage},
};
use nessa_sdk::infrastructure::session_storage::{RecordStorage, RuntimeMessageCommitClock};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::Ordering;
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
    nessa_local_storage::create_directory(root).unwrap();
    let storage = Arc::new(RecordStorage::new(root.join("records")).unwrap());
    let metadata = Arc::new(LocalConversationStore::open(&root.join("metadata.sqlite3")).unwrap());
    let service = ConversationService::new(
        ConversationDependencies {
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
            provider_sessions: ProviderSessionErasers::default(),
            deletion_budgets: DELETION_BUDGETS,
            message_commit_clock: Arc::new(RuntimeMessageCommitClock::new()),
            clock: Arc::new(TestClock),
            environment: crate::conversation::infrastructure::in_process_environment().into(),
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
