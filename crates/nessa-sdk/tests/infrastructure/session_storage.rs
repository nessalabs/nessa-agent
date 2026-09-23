//! Storage substitution, durable snapshots, private files, and exclusive leases.
//! Tests use synthetic evidence; no provider or model is contacted.
//! `identifier_limits`, `messages`, and `retention` exercise restored admission;
//! `permission_choices` checks exact persisted choices and scalable decoding;
//! `provider_identity` checks validated resume metadata; `custom_storage` supplies
//! an unchecked adapter to prove application validation.
use nessa_local_storage as private;
mod benchmark;
mod custom_storage;
mod declined_reviews;
mod file_identity;
use file_identity::journal_path;
#[cfg(unix)]
use file_identity::lock_path;
mod journal;
mod journal_fixture;
use journal_fixture::{journal_bytes, snapshot_json};
mod error_limits;
mod evidence;
mod identifier_limits;
mod messages;
mod output_retention;
mod permission_choices;
mod provider_identity;
mod queue_history;
mod retention;
mod robustness;
mod scheduling;
mod settlement;
use nessa_sdk::application::agent_execution::agents::{
    AgentError, AgentStartupContext, AgentStartupPhase, AgentStartupStep, ProviderDiagnostic,
};
use nessa_sdk::application::agent_execution::executions::{
    ExecutionEvent, ExecutionRequest, ExecutionUpdate, SubmissionMode,
};

use nessa_sdk::application::agent_execution::hooks::{HookError, HookFailure};
use nessa_sdk::application::agent_execution::permissions::*;
use nessa_sdk::application::agent_execution::providers::{
    CloseOutcome, ImageInputRefusal, ProviderIdentity, UserImageError,
};
use nessa_sdk::application::agent_execution::sessions::storage::*;
use nessa_sdk::application::agent_execution::sessions::SessionManager;
use nessa_sdk::application::agent_execution::tools::ToolReviewInput;
use nessa_sdk::domain::agent_execution::executions::*;
use nessa_sdk::domain::agent_execution::permissions::*;
use nessa_sdk::domain::agent_execution::prompts::{
    ImageReference, LinkedFile, PromptText, UserMessage,
};
use nessa_sdk::domain::agent_execution::sessions::*;
use nessa_sdk::domain::agent_execution::tools::*;
use nessa_sdk::domain::common::value_objects::{ImageMediaType, Sha256Digest};
use nessa_sdk::infrastructure::session_storage::{InMemoryStorage, LocalFileStorage};
use std::sync::Arc;
use tokio::sync::Barrier;
use uuid::Uuid;

fn id(value: &str) -> SessionId {
    SessionId::new(value).unwrap()
}
fn snapshot(name: &str) -> SessionSnapshot {
    let execution_id = ExecutionId::new("execution").unwrap();
    SessionSnapshot {
        queue_history: Vec::new(),
        id: id(name),
        provider: ProviderIdentity::new("fixture", "model", "workspace").unwrap(),
        provider_context: ProviderContext::Recorded(
            ExecutionSessionId::new("native-session").unwrap(),
        ),
        invocations: vec![InvocationRecord {
            target_event_offset: None,
            provider_report: None,
            local_cancellation: None,
            local_outcome: None,
            cancellation: None,
            submission: SubmissionMode::Immediate,
            scheduling: Vec::new(),
            request: ExecutionRequest {
                execution_id: execution_id.clone(),
                user_message: UserMessage::text_only(PromptText::new("message").unwrap()),
                estimated_input_tokens: 12,
                reserved_output_tokens: 24,
            },
            actor: ActionContext::new("user", "test", "invoke").unwrap(),
            events: vec![ExecutionEvent::new(
                execution_id,
                ExecutionUpdate::Message(MessageChunk::text("exact α\n")),
            )],
            result: None,
        }],
    }
}
fn cancelled_request(
    request: PermissionRequest,
    reason: PermissionCancellationReason,
) -> PermissionRequest {
    let execution = request.execution_id().clone();
    let id = request.id().clone();
    let mut session =
        ExecutionSession::new(ExecutionSessionId::new("fixture-cancellation").unwrap());
    session.begin_execution(execution.clone()).unwrap();
    session
        .observe_tool(
            &execution,
            ToolCallUpdate::new(request.tool_id().clone(), None, None, None, None, None),
        )
        .unwrap();
    session.request_permission(request).unwrap();
    match reason.view() {
        PermissionCancellationReasonView::ExecutionFinished => {
            return session
                .finish_execution(&execution, Ok(ExecutionOutcome::Completed))
                .unwrap()
                .1
                .remove(0);
        }
        PermissionCancellationReasonView::ExecutionFailed => {
            return session
                .finish_execution(&execution, Err(reason))
                .unwrap()
                .1
                .remove(0);
        }
        _ => {}
    }
    if matches!(
        reason.view(),
        PermissionCancellationReasonView::SessionClosed
            | PermissionCancellationReasonView::SessionFailed
            | PermissionCancellationReasonView::EventConsumerDropped
            | PermissionCancellationReasonView::SessionHandlesDropped
    ) {
        return session.close(reason).unwrap().into_parts().1.remove(0);
    }
    session
        .cancel_permission(&execution, &id, reason)
        .unwrap()
        .unwrap()
}
fn assert_same(actual: &SessionSnapshot, expected: &SessionSnapshot) {
    assert_eq!(actual.id, expected.id);
    assert_eq!(actual.provider, expected.provider);
    assert_eq!(actual.provider_context, expected.provider_context);
    assert_eq!(actual.invocations.len(), expected.invocations.len());
    for (actual, expected) in actual.invocations.iter().zip(&expected.invocations) {
        assert_eq!(actual.submission, expected.submission);
        assert_eq!(actual.request.execution_id, expected.request.execution_id);
        assert_eq!(actual.request.user_message, expected.request.user_message);
        assert_eq!(
            actual.request.estimated_input_tokens,
            expected.request.estimated_input_tokens
        );
        assert_eq!(
            actual.request.reserved_output_tokens,
            expected.request.reserved_output_tokens
        );
        assert_eq!(actual.target_event_offset, expected.target_event_offset);
        assert_eq!(actual.actor, expected.actor);
        assert_eq!(actual.events, expected.events);
        assert_eq!(actual.scheduling, expected.scheduling);
        assert_eq!(actual.cancellation, expected.cancellation);
        assert_eq!(actual.result, expected.result);
    }
}
#[tokio::test]
async fn stores_substitute_with_exclusive_leases_and_isolated_snapshots() {
    let root = tempfile::tempdir().unwrap();
    private::create_directory(&root.path().join("private")).unwrap();
    let stores: Vec<Arc<dyn SessionStorage>> = vec![
        Arc::new(InMemoryStorage::new()),
        Arc::new(LocalFileStorage::new(root.path().join("private")).unwrap()),
    ];
    for storage in stores {
        let first = storage.open(id("first")).await.unwrap();
        assert!(first.load().await.unwrap().is_none());
        assert!(matches!(
            storage.open(id("first")).await,
            Err(StorageError::Busy)
        ));
        let independent = storage.open(id("second")).await.unwrap();
        assert!(independent.load().await.unwrap().is_none());
        first.save(snapshot("first")).await.unwrap();
        assert_eq!(
            first.save(snapshot("second")).await,
            Err(StorageError::IdentityMismatch)
        );
        assert_same(&first.load().await.unwrap().unwrap(), &snapshot("first"));
        drop(first);
        let reopened = storage.open(id("first")).await.unwrap();
        assert_same(&reopened.load().await.unwrap().unwrap(), &snapshot("first"));
        assert!(independent.load().await.unwrap().is_none());
    }
}
#[tokio::test]
async fn file_snapshots_survive_independent_storage_instances_and_replace_atomically() {
    let root = tempfile::tempdir().unwrap();
    private::create_directory(&root.path().join("private")).unwrap();
    let storage = LocalFileStorage::new(root.path().join("private")).unwrap();
    let store = storage.open(id("saved")).await.unwrap();
    assert!(matches!(
        LocalFileStorage::new(root.path().join("private"))
            .unwrap()
            .open(id("saved"))
            .await,
        Err(StorageError::Busy)
    ));
    let mut value = snapshot("saved");
    store.save(value.clone()).await.unwrap();
    value.invocations[0].result = Some(Ok(ExecutionOutcome::Completed));
    store.save(value.clone()).await.unwrap();
    drop(store);
    drop(storage);
    let store = LocalFileStorage::new(root.path().join("private"))
        .unwrap()
        .open(id("saved"))
        .await
        .unwrap();
    assert_same(&store.load().await.unwrap().unwrap(), &value);
    let files: Vec<_> = std::fs::read_dir(root.path().join("private"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(files.len(), 2, "only snapshot and stable lease file remain");
}
fn choices() -> PermissionOptions {
    let decisions = vec![
        PermissionDecision::new(PermissionEffect::Allow, PermissionScope::request()),
        PermissionDecision::new(
            PermissionEffect::Deny,
            PermissionScope::application(PermissionApplicationId::new("app").unwrap()),
        ),
        PermissionDecision::new(
            PermissionEffect::Allow,
            PermissionScope::session(
                PermissionApplicationId::new("app").unwrap(),
                PermissionSessionId::new("scope").unwrap(),
            ),
        ),
    ];
    let config = PermissionOfferPolicy::new(decisions.clone()).unwrap();
    PermissionOptions::new(
        decisions
            .into_iter()
            .enumerate()
            .map(|(index, decision)| {
                PermissionOption::new(
                    PermissionOptionId::new(format!("option-{index}")).unwrap(),
                    format!("Choice {index}"),
                    decision,
                )
                .unwrap()
            })
            .collect(),
        &config,
    )
    .unwrap()
}
#[tokio::test]
async fn snapshots_round_trip_tool_updates_reviews_and_every_cancellation_cause() {
    let root = tempfile::tempdir().unwrap();
    private::create_directory(&root.path().join("private")).unwrap();
    let storage = LocalFileStorage::new(root.path().join("private")).unwrap();
    let store = storage.open(id("evidence")).await.unwrap();
    let mut value = snapshot("evidence");
    let invocation = &mut value.invocations[0];
    let execution = invocation.request.execution_id.clone();
    let tool = ToolCallUpdate::new(
        ToolCallId::new("tool").unwrap(),
        Some("Edit".into()),
        Some(ToolKind::Edit),
        Some(ToolStatus::Running),
        Some(vec![FileLocation::new(
            FilePath::new("relative/file").unwrap(),
            Some(3),
        )]),
        Some(vec![
            ToolContent::text("output"),
            ToolContent::diff(
                FilePath::new("relative/file").unwrap(),
                Some("old".into()),
                "new",
            ),
            ToolContent::diff(FilePath::new("new-file").unwrap(), None, "created"),
            ToolContent::text(""),
            ToolContent::diff(
                FilePath::new("empty-file").unwrap(),
                Some(String::new()),
                "",
            ),
        ]),
    );
    invocation.events.push(ExecutionEvent::new(
        execution.clone(),
        ExecutionUpdate::Tool(tool.clone()),
    ));
    invocation.events.push(ExecutionEvent::new(
        execution.clone(),
        ExecutionUpdate::Message(MessageChunk::thought("thought")),
    ));
    let input = ToolReviewInput {
        name: "Write".into(),
        arguments_json: "{\"content\":\"exact\"}".into(),
    };
    invocation.events.push(ExecutionEvent::new(
        execution.clone(),
        ExecutionUpdate::PermissionRequested {
            id: PermissionId::new("review").unwrap(),
            tool_id: tool.id().clone(),
            observation: ToolObservation::default().with_update(tool.clone()),
            input: input.clone(),
            options: choices(),
        },
    ));
    let reasons = vec![
        PermissionCancellationReason::provider_withdrawal(),
        PermissionCancellationReason::session_closed(),
        PermissionCancellationReason::session_failed(),
        PermissionCancellationReason::execution_finished(),
        PermissionCancellationReason::execution_failed(),
        PermissionCancellationReason::deadline_exceeded(),
        PermissionCancellationReason::event_consumer_dropped(),
        PermissionCancellationReason::session_handles_dropped(),
        PermissionCancellationReason::custom(
            CustomPermissionCancellationReason::new("guard", "specific reason").unwrap(),
        ),
    ];
    for (index, reason) in reasons.into_iter().enumerate() {
        let request = PermissionRequest::new(
            PermissionId::new(format!("review-{index}")).unwrap(),
            execution.clone(),
            tool.id().clone(),
            choices(),
        );
        invocation.events.push(ExecutionEvent::new(
            execution.clone(),
            ExecutionUpdate::PermissionRequested {
                id: request.id().clone(),
                tool_id: tool.id().clone(),
                observation: ToolObservation::default().with_update(tool.clone()),
                input: input.clone(),
                options: choices(),
            },
        ));
        let origin = match reason.view() {
            PermissionCancellationReasonView::ProviderWithdrawal => CancellationOrigin::Provider,
            PermissionCancellationReasonView::SessionClosed
            | PermissionCancellationReasonView::Custom(_) => {
                CancellationOrigin::Client(invocation.actor.clone())
            }
            _ => CancellationOrigin::Runtime,
        };
        let request = cancelled_request(request, reason);
        let record = PermissionCancellation::from_record(
            value.provider_context.recorded().unwrap().clone(),
            request,
            input.clone(),
            origin,
        )
        .unwrap();
        invocation.events.push(ExecutionEvent::new(
            execution.clone(),
            ExecutionUpdate::PermissionCancelled(record),
        ));
    }
    for (kind, status) in [
        (ToolKind::Read, ToolStatus::Pending),
        (ToolKind::Search, ToolStatus::Completed),
        (ToolKind::Other, ToolStatus::Failed),
    ] {
        invocation.events.push(ExecutionEvent::new(
            execution.clone(),
            ExecutionUpdate::Tool(ToolCallUpdate::new(
                tool.id().clone(),
                None,
                Some(kind),
                Some(status),
                Some(vec![]),
                Some(vec![]),
            )),
        ));
    }
    invocation.events.push(ExecutionEvent::new(
        execution.clone(),
        ExecutionUpdate::Tool(ToolCallUpdate::new(
            tool.id().clone(),
            None,
            None,
            None,
            None,
            None,
        )),
    ));
    invocation.events.push(ExecutionEvent::new(
        execution,
        ExecutionUpdate::Finished(ExecutionOutcome::Cancelled),
    ));
    store.save(value.clone()).await.unwrap();
    assert_same(&store.load().await.unwrap().unwrap(), &value);
}
#[tokio::test]
async fn snapshots_preserve_all_settlement_errors_and_unresolved_attempts() {
    let root = tempfile::tempdir().unwrap();
    private::create_directory(&root.path().join("private")).unwrap();
    let storage = LocalFileStorage::new(root.path().join("private")).unwrap();
    let store = storage.open(id("errors")).await.unwrap();
    let mut value = snapshot("errors");
    let errors = vec![
        AgentError::Configuration("config".into()),
        AgentError::MultipleOperationFailures {
            first_error: Box::new(AgentError::AuditFailure),
            subsequent_error: Box::new(AgentError::MultipleOperationFailures {
                first_error: Box::new(AgentError::Transport("response write".into())),
                subsequent_error: Box::new(AgentError::Deadline),
            }),
        },
        AgentError::OperationAndCleanupFailure {
            operation_error: Box::new(AgentError::Backpressure),
            cleanup_error: Box::new(AgentError::AuditAndCleanupFailure),
        },
        AgentError::SubmissionConflict,
        AgentError::SubmissionUnresolved,
        AgentError::ExecutionObservation {
            error: Box::new(AgentError::CleanupUncertain),
            execution_result: None,
        },
        AgentError::ExecutionObservation {
            error: Box::new(AgentError::Transport("stream".into())),
            execution_result: Some(Box::new(Ok(ExecutionOutcome::Completed))),
        },
        AgentError::Unsupported("unsupported".into()),
        AgentError::InvalidInput("input".into()),
        AgentError::UserImage(UserImageError::Missing),
        AgentError::UserImage(UserImageError::Unavailable),
        AgentError::UserImage(UserImageError::Mismatch),
        AgentError::ImageInputRefused(ImageInputRefusal::NotOffered),
        AgentError::ImageInputRefused(ImageInputRefusal::AgentDoesNotAccept),
        AgentError::ImageInputRefused(ImageInputRefusal::MediaType(ImageMediaType::Png)),
        AgentError::ImageInputRefused(ImageInputRefusal::MediaType(ImageMediaType::Jpeg)),
        AgentError::ImageInputRefused(ImageInputRefusal::MediaType(ImageMediaType::Gif)),
        AgentError::ImageInputRefused(ImageInputRefusal::MediaType(ImageMediaType::Webp)),
        AgentError::ImageInputRefused(ImageInputRefusal::ImageTooLarge {
            size: 7,
            max_bytes: 6,
        }),
        AgentError::MessageTooLarge {
            encoded_bytes: u64::MAX,
            max_bytes: 16 * 1024 * 1024,
        },
        AgentError::Busy,
        AgentError::Closed,
        AgentError::StalePermission,
        AgentError::Protocol("protocol".into()),
        AgentError::Provider {
            code: -123,
            diagnostic: None,
        },
        AgentError::Transport("transport".into()),
        AgentError::Deadline,
        AgentError::Backpressure,
        AgentError::CleanupUncertain,
        AgentError::AuditFailure,
        AgentError::AuditAndCleanupFailure,
        AgentError::PermissionAnswerDeliveryAndAuditFailure {
            delivery_error: Box::new(AgentError::Transport("broken write".into())),
            cleanup_error: None,
        },
        AgentError::PermissionAnswerDeliveryAndAuditFailure {
            delivery_error: Box::new(AgentError::Deadline),
            cleanup_error: Some(Box::new(AgentError::CleanupUncertain)),
        },
        AgentError::BeforeInvocationHook(HookFailure {
            index: 2,
            error: HookError::Failed("hook".into()),
        }),
        AgentError::AfterInvocationHooks {
            failures: vec![HookFailure {
                index: 3,
                error: HookError::Panicked,
            }],
            execution_result: Box::new(Ok(ExecutionOutcome::Completed)),
        },
        AgentError::StorageAfterExecution {
            error: StorageError::Io("disk".into()),
            execution_result: Box::new(Err(AgentError::Storage(StorageError::Busy))),
        },
        AgentError::Storage(StorageError::Corrupt("record".into())),
        AgentError::Storage(StorageError::IdentityMismatch),
        AgentError::StorageInitialization {
            error: StorageError::Io("initial write".into()),
            cleanup_result: Box::new(Ok(CloseOutcome { forced: true })),
        },
        AgentError::StorageInitialization {
            error: StorageError::Busy,
            cleanup_result: Box::new(Err(AgentError::CleanupUncertain)),
        },
    ];
    for error in errors {
        let mut record = value.invocations[0].clone();
        record.result = Some(Err(error));
        value.invocations.push(record);
    }
    for outcome in [
        ExecutionOutcome::Completed,
        ExecutionOutcome::OutputLimit,
        ExecutionOutcome::RequestLimit,
        ExecutionOutcome::Refused,
        ExecutionOutcome::Cancelled,
    ] {
        let mut record = value.invocations[0].clone();
        record.result = Some(Ok(outcome));
        value.invocations.push(record);
    }
    for (index, record) in value.invocations.iter_mut().enumerate() {
        record.request.execution_id = ExecutionId::new(format!("error-case-{index}")).unwrap();
        record.events = record
            .events
            .iter()
            .map(|event| {
                ExecutionEvent::new(record.request.execution_id.clone(), event.update().clone())
            })
            .collect();
    }
    store.save(value.clone()).await.unwrap();
    assert_same(&store.load().await.unwrap().unwrap(), &value);
}
#[tokio::test]
async fn corrupt_or_mismatched_snapshots_fail_without_replacing_existing_evidence() {
    let root = tempfile::tempdir().unwrap();
    private::create_directory(&root.path().join("private")).unwrap();
    let storage = LocalFileStorage::new(root.path().join("private")).unwrap();
    let store = storage.open(id("saved")).await.unwrap();
    store.save(snapshot("saved")).await.unwrap();
    let path = journal_path(&root.path().join("private"), "saved");
    let original = std::fs::read(&path).unwrap();
    let mut json: serde_json::Value = snapshot_json(&original).unwrap();
    for mutate in [0, 1, 2, 3] {
        match mutate {
            0 => json["id"] = "other".into(),
            1 => {
                json = snapshot_json(&original).unwrap();
                json["invocations"][0]["execution_id"] = "".into();
            }
            2 => {
                json = snapshot_json(&original).unwrap();
                json["invocations"][0]["events"][0]["execution_id"] = "other".into();
            }
            _ => {
                json = snapshot_json(&original).unwrap();
                json["unexpected"] = true.into();
            }
        }
        std::fs::write(&path, journal_bytes(&json).unwrap()).unwrap();
        let error = store.load().await.unwrap_err();
        if mutate == 0 {
            assert_eq!(error, StorageError::IdentityMismatch);
        } else {
            assert!(matches!(error, StorageError::Corrupt(_)));
        }
    }
    std::fs::write(&path, b"{broken\n").unwrap();
    assert!(matches!(store.load().await, Err(StorageError::Corrupt(_))));
    assert_eq!(std::fs::read(path).unwrap(), b"{broken\n");
}
#[cfg(unix)]
#[tokio::test]
async fn local_storage_rejects_public_files_and_links_without_repairing_them() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let root = tempfile::tempdir().unwrap();
    private::create_directory(&root.path().join("private")).unwrap();
    let storage = LocalFileStorage::new(root.path().join("private")).unwrap();
    let store = storage.open(id("private")).await.unwrap();
    store.save(snapshot("private")).await.unwrap();
    let path = journal_path(&root.path().join("private"), "private");
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(matches!(store.load().await, Err(StorageError::Io(_))));
    assert!(matches!(
        store.save(snapshot("private")).await,
        Err(StorageError::Io(_))
    ));
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o644
    );
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::hard_link(&path, root.path().join("private").join("alias")).unwrap();
    assert!(matches!(store.load().await, Err(StorageError::Io(_))));
    symlink(&path, lock_path(&root.path().join("private"), "linked")).unwrap();
    assert!(matches!(
        storage.open(id("linked")).await,
        Err(StorageError::Io(_))
    ));
}
#[tokio::test]
async fn file_writer_lease_excludes_a_separate_process() {
    let root = tempfile::tempdir().unwrap();
    private::create_directory(&root.path().join("private")).unwrap();
    let storage = LocalFileStorage::new(root.path().join("private")).unwrap();
    let _lease = storage.open(id("cross-process")).await.unwrap();
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "infrastructure::session_storage::cross_process_lease_probe",
            "--ignored",
        ])
        .env("NESSA_STORAGE_LEASE_TEST_ROOT", root.path().join("private"))
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
#[ignore = "runs only as the child process of the lease test"]
#[tokio::test]
async fn cross_process_lease_probe() {
    let root = std::env::var_os("NESSA_STORAGE_LEASE_TEST_ROOT")
        .expect("parent supplies isolated test directory");
    let storage = LocalFileStorage::new(std::path::PathBuf::from(root)).unwrap();
    assert!(matches!(
        storage.open(id("cross-process")).await,
        Err(StorageError::Busy)
    ));
}

#[tokio::test]
async fn rejected_snapshot_correlation_keeps_the_previous_record_in_both_stores() {
    let root = tempfile::tempdir().unwrap();
    private::create_directory(&root.path().join("private")).unwrap();
    let stores: Vec<Arc<dyn SessionStorage>> = vec![
        Arc::new(InMemoryStorage::new()),
        Arc::new(LocalFileStorage::new(root.path().join("private")).unwrap()),
    ];
    for storage in stores {
        let store = storage.open(id("saved")).await.unwrap();
        let original = snapshot("saved");
        store.save(original.clone()).await.unwrap();
        let mut invalid = original.clone();
        invalid.invocations[0].events.push(ExecutionEvent::new(
            ExecutionId::new("different").unwrap(),
            ExecutionUpdate::Finished(ExecutionOutcome::Completed),
        ));
        assert!(matches!(
            store.save(invalid).await,
            Err(StorageError::Corrupt(_))
        ));
        assert_same(&store.load().await.unwrap().unwrap(), &original);
    }
}

#[tokio::test]
async fn file_write_failure_is_visible_and_does_not_change_the_existing_snapshot() {
    let root = tempfile::tempdir().unwrap();
    private::create_directory(&root.path().join("private")).unwrap();
    let directory = root.path().join("private").join("storage");
    let storage = LocalFileStorage::new(&directory).unwrap();
    let store = storage.open(id("saved")).await.unwrap();
    store.save(snapshot("saved")).await.unwrap();
    // Replace the snapshot path, not the leased directory: Windows correctly
    // prevents renaming a directory while its lease handle is open.
    let snapshot_path = journal_path(&directory, "saved");
    let preserved = directory.join("preserved.jsonl");
    std::fs::rename(&snapshot_path, &preserved).unwrap();
    private::create_directory(&snapshot_path).unwrap();
    assert!(matches!(
        store.save(snapshot("saved")).await,
        Err(StorageError::Io(_))
    ));
    assert!(matches!(store.load().await, Err(StorageError::Io(_))));
    std::fs::remove_dir(&snapshot_path).unwrap();
    std::fs::rename(&preserved, &snapshot_path).unwrap();
    assert_same(&store.load().await.unwrap().unwrap(), &snapshot("saved"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn simultaneous_opens_have_one_winner_and_release_without_losing_evidence() {
    let root = tempfile::tempdir().unwrap();
    private::create_directory(&root.path().join("private")).unwrap();
    let backends: Vec<Arc<dyn SessionStorage>> = vec![
        Arc::new(InMemoryStorage::new()),
        Arc::new(LocalFileStorage::new(root.path().join("private")).unwrap()),
    ];
    for backend in backends {
        let barrier = Arc::new(Barrier::new(8));
        let mut tasks = Vec::new();
        for _ in 0..8 {
            let backend = backend.clone();
            let barrier = barrier.clone();
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                backend.open(id("contended")).await
            }));
        }
        // Retain the winning lease while collecting every result: a later open
        // must not succeed just because an earlier task has returned.
        let mut winner = None;
        let mut rejected = 0;
        for task in tasks {
            match task.await.unwrap() {
                Ok(lease) => {
                    assert!(winner.is_none(), "multiple concurrent owners");
                    winner = Some(lease);
                }
                Err(StorageError::Busy) => rejected += 1,
                Err(error) => panic!("unexpected acquisition failure: {error}"),
            }
        }
        assert_eq!(rejected, 7);
        let winner = winner.unwrap();
        winner.save(snapshot("contended")).await.unwrap();
        let independent = backend.open(id("independent")).await.unwrap();
        assert!(independent.load().await.unwrap().is_none());
        drop(winner);
        let reopened = backend.open(id("contended")).await.unwrap();
        assert_same(
            &reopened.load().await.unwrap().unwrap(),
            &snapshot("contended"),
        );
        // The lease must keep its resources alive after the factory is dropped.
        drop(backend);
        assert_same(
            &reopened.load().await.unwrap().unwrap(),
            &snapshot("contended"),
        );
    }
}

#[tokio::test]
async fn optional_session_ids_generate_independent_keys_and_can_be_reopened() {
    let root = tempfile::tempdir().unwrap();
    private::create_directory(&root.path().join("private")).unwrap();
    let stores: Vec<Arc<dyn SessionStorage>> = vec![
        Arc::new(InMemoryStorage::new()),
        Arc::new(LocalFileStorage::new(root.path().join("private")).unwrap()),
    ];
    for storage in stores {
        let first = SessionManager::open(None, storage.clone()).await.unwrap();
        let second = SessionManager::open(None, storage.clone()).await.unwrap();
        let key = first.id().clone();
        assert_eq!(Uuid::parse_str(key.as_str()).unwrap().get_version_num(), 4);
        assert_ne!(first.id(), second.id());
        assert!(matches!(
            SessionManager::open(Some(key.clone()), storage.clone()).await,
            Err(StorageError::Busy)
        ));
        drop(first);
        let reopened = SessionManager::open(Some(key.clone()), storage.clone())
            .await
            .unwrap();
        assert_eq!(reopened.id(), &key);
        let named = SessionManager::open(Some(id("named-session")), storage)
            .await
            .unwrap();
        assert_eq!(named.id().as_str(), "named-session");
    }
}
