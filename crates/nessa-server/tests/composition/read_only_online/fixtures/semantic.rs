//! Actual gateway Agent facts cross the native boundary into independent core processes.
use super::{gateway::HeadHold, private_write, Setup};
#[path = "semantic/storage.rs"]
mod storage;
use crate::conversation::application::{
    ConversationCaller, ConversationDependencies, ConversationError, ConversationLimits,
    ConversationService, ProviderSessionErasers, RequestedConversation, SubmissionMode,
    SubmittedMessage,
};
use crate::conversation::infrastructure::LocalConversationStore;
use crate::conversation_test_support::{
    only, AcceptingCreationAudit, AcceptingDeletionAudit, AcceptingModeAudit, MemorySummaries,
    Provider, ProviderFactory, RecordingFileLinkAudit, TestClock, Unlisted, DELETION_BUDGETS,
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_protocol::conversation::projection::retained_view;
use nessa_protocol::conversation::view::ConversationView;
use nessa_sdk::application::agent_execution::executions::ExecutionUpdate;
use nessa_sdk::application::agent_execution::sessions::{
    SessionStorage, SubmissionAcknowledgement,
};
use nessa_sdk::domain::agent_execution::executions::{
    ExecutionOutcome, InvocationStage, MessageChunk,
};
use nessa_sdk::domain::agent_execution::sessions::SessionId;
use nessa_sdk::infrastructure::session_storage::{
    RecordStorage, RuntimeMessageCommitClock, TranscriptCheckpoint, TranscriptFold,
};
use nessa_sync::replication::application::RecordSource;
use nessa_sync::replication::domain::{Id, PageRequest};
use nessa_sync::replication::infrastructure::MAX_PAGE_PAYLOAD;
use serde_json::json;
use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use storage::RecordingStorage;
use tokio::io::{stdin, AsyncBufReadExt, BufReader};
use tokio::sync::oneshot;
use uuid::Uuid;

pub(super) async fn serve(
    root: &Path,
    setup: &Setup,
    storage: Arc<RecordStorage>,
    metadata: Arc<LocalConversationStore>,
    heads: Arc<HeadHold>,
) {
    let producer = Arc::new(RecordingStorage::new(storage.clone()));
    let provider = Arc::new(ProviderFactory::default());
    provider.request_permission.store(1, Ordering::SeqCst);
    provider.answer_succeeds.store(true, Ordering::SeqCst);
    provider
        .execution_updates
        .lock()
        .unwrap()
        .push(ExecutionUpdate::Message(MessageChunk::text(
            "Committed before permission.",
        )));
    let (release, gate) = oneshot::channel();
    *provider.permission_gate.lock().unwrap() = Some(gate);
    let service = ConversationService::new(
        ConversationDependencies {
            agents: only(Arc::new(Provider::new(provider.clone()))),
            storage: producer.clone(),
            metadata,
            mode_audit: Arc::new(AcceptingModeAudit),
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            summaries: Arc::new(MemorySummaries::default()),
            listing: Arc::new(Unlisted),
            deletion_audit: Arc::new(AcceptingDeletionAudit),
            provider_sessions: ProviderSessionErasers::default(),
            deletion_budgets: DELETION_BUDGETS,
            message_commit_clock: Arc::new(RuntimeMessageCommitClock::new()),
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    let id = ConversationId::new(&setup.conversation).unwrap();
    let caller = |action: &str| ConversationCaller {
        organization_id: OrganizationId::new(&setup.organization).unwrap(),
        principal_id: PrincipalId::new(&setup.owner).unwrap(),
        surface_id: "semantic-acceptance".into(),
        action_id: action.into(),
    };
    service
        .create(
            id.clone(),
            caller("create"),
            RequestedConversation::default(),
        )
        .await
        .unwrap();
    service
        .submit(
            id.clone(),
            caller("first-input"),
            "first".into(),
            SubmittedMessage {
                text: "First accepted input".into(),
                ..SubmittedMessage::default()
            },
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let view = service
                .read(id.clone(), caller("wait-permission"))
                .await
                .unwrap();
            if !view.permissions.is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    service
        .submit(
            id.clone(),
            caller("second-input"),
            "second".into(),
            SubmittedMessage {
                text: "Second queued input".into(),
                ..SubmittedMessage::default()
            },
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    let prefix_view = service
        .read(id.clone(), caller("capture-prefix"))
        .await
        .unwrap();
    let checkpoint = capture(
        root,
        setup,
        &producer,
        &provider,
        &prefix_view,
        "prefix",
        None,
    )
    .await;
    let prefix = storage
        .read_committed(SessionId::new(&setup.conversation).unwrap())
        .await
        .unwrap()
        .unwrap();
    let snapshot = prefix.snapshot().unwrap();
    assert_eq!(snapshot.invocations.len(), 2);
    assert!(snapshot
        .invocations
        .iter()
        .all(|item| item.acknowledgement == SubmissionAcknowledgement::Acknowledged));
    assert!(snapshot
        .invocations
        .iter()
        .all(|item| item.result.is_none()));
    assert!(!snapshot.queue_history.is_empty());
    assert!(!snapshot.invocations[0].scheduling.is_empty());
    assert_eq!(provider.executions.lock().unwrap().as_slice(), &["first"]);
    println!("ONLINE_READY");
    io::stdout().flush().unwrap();
    let mut release = Some(release);
    let mut lines = BufReader::new(stdin()).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        match line.as_str() {
            "hold" => heads.hold_next(),
            "release" => heads.release(),
            "settle" => {
                service
                    .answer(
                        id.clone(),
                        caller("answer-permission"),
                        "first".into(),
                        "permission".into(),
                        "allow".into(),
                    )
                    .await
                    .unwrap();
                provider.request_permission.store(0, Ordering::SeqCst);
                release.take().unwrap().send(()).unwrap();
                tokio::time::timeout(Duration::from_secs(5), async {
                    loop {
                        if producer.confirmed().is_some_and(|confirmed| {
                            let snapshot = &confirmed.snapshot;
                            snapshot.invocations.len() == 2
                                && snapshot.invocations.iter().all(|item| {
                                    item.result == Some(Ok(ExecutionOutcome::Completed))
                                        && item.scheduling.last().is_some_and(|event| {
                                            event.stage == InvocationStage::Settled
                                        })
                                })
                        }) {
                            break;
                        }
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .unwrap();
                let terminal_view = service
                    .read(id.clone(), caller("capture-terminal"))
                    .await
                    .unwrap();
                capture(
                    root,
                    setup,
                    &producer,
                    &provider,
                    &terminal_view,
                    "terminal",
                    Some(&checkpoint),
                )
                .await;
                assert_eq!(
                    provider.executions.lock().unwrap().as_slice(),
                    &["first", "second"]
                );
            }
            "verify" => {
                assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);
                assert_eq!(
                    provider.executions.lock().unwrap().as_slice(),
                    &["first", "second"]
                );
            }
            "delete" => {
                // No provider eraser is installed: metadata still commits the
                // authoritative tombstone before reporting unfinished cleanup.
                assert!(matches!(
                    service.delete(id.clone(), caller("delete")).await,
                    Ok(true) | Err(ConversationError::DeletionIncomplete(_))
                ));
                assert!(matches!(
                    service.read(id.clone(), caller("read-deleted")).await,
                    Err(ConversationError::Deleted)
                ));
            }
            other => panic!("unknown semantic control {other}"),
        }
        println!("DONE {line}");
        io::stdout().flush().unwrap();
    }
}

async fn capture(
    root: &Path,
    setup: &Setup,
    producer: &RecordingStorage,
    provider: &ProviderFactory,
    gateway: &ConversationView,
    name: &str,
    prior: Option<&(u64, TranscriptCheckpoint)>,
) -> (u64, TranscriptCheckpoint) {
    let storage = producer.records();
    let confirmed = producer
        .confirmed()
        .expect("actual producer confirmed a save");
    let dispatches = provider.executions.lock().unwrap().clone();
    let opened = provider.open_calls.load(Ordering::SeqCst);
    let session = SessionId::new(&setup.conversation).unwrap();
    let committed = storage
        .read_committed(session.clone())
        .await
        .unwrap()
        .unwrap();
    let source = storage
        .record_source(&session, Id::new(&setup.gateway).unwrap())
        .await
        .unwrap()
        .unwrap();
    let scope = source.scope(
        Id::new(&setup.receiver).unwrap(),
        Id::new(setup.epoch.to_string()).unwrap(),
    );
    let read_scope = scope.clone();
    let (head, records) = tokio::task::spawn_blocking(move || {
        let mut source = source;
        let head = source.head(&read_scope).unwrap();
        assert!(head < 1000, "bounded two-input corpus");
        let mut records = Vec::new();
        while (records.len() as u64) < head {
            let page = source
                .page(&PageRequest {
                    scope: read_scope.clone(),
                    after: records.len() as u64,
                    target: head,
                    max_records: 64,
                    max_payload_bytes: MAX_PAGE_PAYLOAD,
                    max_record_bytes: MAX_PAGE_PAYLOAD,
                })
                .unwrap();
            assert!(!page.records.is_empty());
            records.extend(page.records);
        }
        (head, records)
    })
    .await
    .unwrap();
    let mut full = TranscriptFold::new(scope.clone()).unwrap();
    full.apply(&records).unwrap();
    full.observe_source_head(&scope, head).unwrap();
    // Independent actual write-side candidate, captured only after save success.
    assert_eq!(full.snapshot(), Some(&confirmed.snapshot));
    assert_eq!(full.applied(), confirmed.binding.base());
    // Separate cached incremental/full replay parity uses the same fold owner.
    assert_eq!(full.snapshot(), committed.snapshot());
    assert_eq!(full.applied(), committed.position());
    assert_eq!(full.downloaded(), committed.downloaded());
    assert_eq!(full.status(), committed.status());
    if let Some((applied, checkpoint)) = prior {
        let mut restored = TranscriptFold::restore(scope.clone(), *applied, checkpoint).unwrap();
        restored.apply(&records[*applied as usize..]).unwrap();
        restored.observe_source_head(&scope, head).unwrap();
        assert_eq!(restored.snapshot(), full.snapshot());
        assert_eq!(restored.applied(), full.applied());
        assert_eq!(restored.downloaded(), full.downloaded());
        assert_eq!(restored.fact_count(), full.fact_count());
        assert_eq!(restored.status(), full.status());
    }
    // Offline commands restore the checkpoint without an authenticated head
    // observation; retain the same content/progress with honest stale freshness.
    full.mark_stale();
    let view = retained_view(
        &ConversationId::new(&setup.conversation).unwrap(),
        full.snapshot(),
        full.status(),
        Uuid::new_v4(),
    );
    assert!(view.permissions.is_empty());
    let mut gateway_messages = serde_json::to_value(&gateway.messages).unwrap();
    let mut retained_messages = serde_json::to_value(&view.messages).unwrap();
    if name == "prefix" {
        assert_eq!(gateway.permissions.len(), 1);
        assert_eq!(gateway.pending.len(), 1);
        // Running/queued authority belongs to this Agent; the portable passive
        // receiver renders unresolved retained input. Content still agrees.
        assert_eq!(retained_messages[0]["status"], "unresolved");
        for messages in [&mut gateway_messages, &mut retained_messages] {
            for message in messages.as_array_mut().unwrap() {
                message.as_object_mut().unwrap().remove("status");
            }
        }
    } else {
        assert!(gateway.permissions.is_empty());
        assert!(gateway.pending.is_empty());
    }
    assert_eq!(gateway_messages, retained_messages);

    let snapshot = full.snapshot().unwrap();
    assert_eq!(
        snapshot.invocations[0].request.user_message.text_str(),
        "First accepted input"
    );
    assert_eq!(
        snapshot.invocations[1].request.user_message.text_str(),
        "Second queued input"
    );
    assert!(snapshot.invocations[0]
        .events
        .iter()
        .any(|event| matches!(event.update(), ExecutionUpdate::PermissionRequested { .. })));
    if name == "terminal" {
        assert!(snapshot
            .invocations
            .iter()
            .all(|item| item.provider_report.is_some()));
        assert!(snapshot
            .invocations
            .iter()
            .all(|item| item.result == Some(Ok(ExecutionOutcome::Completed))));
    }
    private_write(&root.join(format!("semantic-{name}.json")), &serde_json::to_vec(&json!({
        "progress": { "applied": full.applied().to_string(), "downloaded": full.downloaded().to_string(), "facts": full.fact_count().to_string() },
        "capturedHead": head.to_string(), "view": view,
    })).unwrap());
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), opened);
    assert_eq!(*provider.executions.lock().unwrap(), dispatches);
    (full.applied(), full.checkpoint().unwrap())
}
