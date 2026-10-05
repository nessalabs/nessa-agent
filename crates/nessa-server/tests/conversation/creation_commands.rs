//! Actual conversation creation uses real control and session record storage.
use super::super::LiveConversation;
use super::*;
use crate::{
    conversation::{
        application::{
            ConversationAgentFuture, ConversationAgentSource, ConversationAgents,
            ConversationDependencies, ConversationLimits, ConversationRepository,
            ProviderSessionErasers, RuntimeReadiness,
        },
        domain::{Conversation, ConversationDeletion},
        infrastructure::LocalConversationStore,
    },
    conversation_test_support::{
        only, AcceptingCreationAudit, AcceptingDeletionAudit, AcceptingModeAudit, Provider,
        ProviderFactory, RecordingFileLinkAudit, TestClock, DELETION_BUDGETS,
    },
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::agents::AgentId;
use nessa_protocol::conversation::domain::{ConversationApprovalMode, ConversationModelId};
use nessa_sdk::application::agent_execution::{
    agents::AgentError,
    commands::{
        CreationReceipt, CreationStage, CreationStorage, CreationStorageError,
        CreationStorageLease, CreationTaskFault,
    },
    sessions::{SessionSnapshot, SessionStorage, StorageError},
};
use nessa_sdk::infrastructure::session_storage::{RecordStorage, RuntimeMessageCommitClock};
use std::io::ErrorKind;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::{
    collections::HashSet,
    future::{poll_fn, Future},
    path::Path,
    pin::Pin,
    sync::atomic::{AtomicBool, Ordering},
    task::Poll,
    time::Duration,
};
use tokio::sync::{oneshot, Mutex as AsyncMutex, Notify};

fn caller(request: &str) -> ConversationCaller {
    ConversationCaller {
        organization_id: OrganizationId::new("org").unwrap(),
        principal_id: PrincipalId::new("person").unwrap(),
        surface_id: "panel".into(),
        action_id: request.into(),
    }
}
fn id() -> ConversationId {
    ConversationId::new(&uuid::Uuid::new_v4().to_string()).unwrap()
}
fn fixture(
    root: &Path,
    provider: Arc<ProviderFactory>,
) -> (
    ConversationService,
    Arc<RecordStorage>,
    Arc<LocalConversationStore>,
) {
    fixture_with_agents(root, only(Arc::new(Provider::new(provider))))
}
fn fixture_with_agents(
    root: &Path,
    agents: ConversationAgents,
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
            agents,
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

struct CreationReadiness {
    started: Notify,
    release: AsyncMutex<Option<oneshot::Receiver<()>>>,
    panics: bool,
}
async fn original_live(
    service: &ConversationService,
    target: &ConversationId,
) -> Arc<LiveConversation> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let slot = service
                .inner
                .conversations
                .lock()
                .await
                .get(target)
                .cloned()
                .unwrap();
            if let Some(Ok(live)) = slot.value.get() {
                break live.clone();
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap()
}
impl RuntimeReadiness for CreationReadiness {
    fn wait(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            self.started.notify_one();
            let release = self.release.lock().await.take().unwrap();
            release.await.unwrap();
            assert!(
                !self.panics,
                "original attachment readiness supervisor panic"
            );
        })
    }
}
struct CreationReadinessSource {
    original: ConversationAgents,
    readiness: Arc<CreationReadiness>,
}
impl ConversationAgentSource for CreationReadinessSource {
    fn resolve(&self, agent: AgentId) -> ConversationAgentFuture<'_> {
        Box::pin(async move {
            let mut configured = self.original.resolve(agent).await?;
            configured.readiness = Some(self.readiness.clone());
            Ok(Some(configured))
        })
    }
    fn resolve_for<'a>(
        &'a self,
        agent: AgentId,
        model: &'a str,
        mode: ConversationApprovalMode,
    ) -> ConversationAgentFuture<'a> {
        Box::pin(async move {
            let mut configured = self.original.resolve_for(agent, model, mode).await?;
            configured.readiness = Some(self.readiness.clone());
            Ok(Some(configured))
        })
    }
}

#[tokio::test]
async fn creation_waits_for_original_attachment_before_saving_ready() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let (release, gate) = oneshot::channel();
    *provider.open_gate.lock().unwrap() = Some(gate);
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let target = id();
    let task = tokio::spawn({
        let service = service.clone();
        let storage = storage.clone();
        let target = target.clone();
        async move {
            service
                .create_command(
                    storage,
                    target,
                    caller("request"),
                    RequestedConversation::default(),
                )
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(5), provider.opening.notified())
        .await
        .unwrap();
    let live = original_live(&service, &target).await;
    {
        let ordinary_waiter = live.join_attachment_owner();
        tokio::pin!(ordinary_waiter);
        poll_fn(|cx| {
            assert!(ordinary_waiter.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        assert!(!task.is_finished());
        assert_eq!(
            live.agent.attachment_status().phase(),
            AttachmentPhase::Starting
        );
        let db =
            nessa_local_database::rusqlite::Connection::open(root.join("records/records.sqlite3"))
                .unwrap();
        let count: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM event_records WHERE schema_id = 'nessa.creation'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);
        drop(db);
        release.send(()).unwrap();
        ordinary_waiter.await.unwrap();
    }
    let ready = task.await.unwrap().unwrap();
    assert_eq!(ready.stage(), CreationStage::Ready);
    assert_eq!(
        live.agent.attachment_status().phase(),
        AttachmentPhase::Attached
    );
    assert!(live
        .agent
        .session_manager()
        .snapshot()
        .await
        .unwrap()
        .provider_context
        .recorded()
        .is_some());
    drop(live);
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn dropping_an_attachment_join_waiter_keeps_the_original_running_owner() {
    let directory = tempfile::tempdir().unwrap();
    let provider = Arc::new(ProviderFactory::default());
    let (release, gate) = oneshot::channel();
    *provider.open_gate.lock().unwrap() = Some(gate);
    let (service, storage, metadata) = fixture(&directory.path().join("data"), provider.clone());
    let target = id();
    service
        .create(
            target.clone(),
            caller("request"),
            RequestedConversation::default(),
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), provider.opening.notified())
        .await
        .unwrap();
    let live = original_live(&service, &target).await;
    {
        let cancelled = live.join_attachment_owner();
        tokio::pin!(cancelled);
        poll_fn(|cx| {
            assert!(cancelled.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        // Dropping this future releases the wait's lock, not the original handle.
    }
    {
        let replacement = live.join_attachment_owner();
        tokio::pin!(replacement);
        poll_fn(|cx| {
            assert!(replacement.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        release.send(()).unwrap();
        replacement.await.unwrap();
    }
    assert_eq!(live.join_attachment_owner().await, Ok(()));
    assert_eq!(
        live.agent.attachment_status().phase(),
        AttachmentPhase::Attached
    );
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);
    drop(live);
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn competing_attachment_waiters_share_original_completion_and_fault() {
    for (panics, provider_refuses) in [(false, false), (false, true), (true, false)] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("data");
        let provider = Arc::new(ProviderFactory::default());
        if provider_refuses {
            *provider.open_failure.lock().unwrap() =
                Some(AgentError::Protocol("original attachment refused".into()));
        }
        let (release, gate) = oneshot::channel();
        let readiness = Arc::new(CreationReadiness {
            started: Notify::new(),
            release: AsyncMutex::new(Some(gate)),
            panics,
        });
        let original = only(Arc::new(Provider::new(provider.clone())));
        let agents = ConversationAgents::from_source(
            HashSet::from([AgentId::Claude]),
            AgentId::Claude,
            Arc::new(CreationReadinessSource {
                original,
                readiness: readiness.clone(),
            }),
        )
        .unwrap();
        let (service, storage, metadata) = fixture_with_agents(&root, agents);
        let target = id();
        let command = tokio::spawn({
            let service = service.clone();
            let storage = storage.clone();
            let target = target.clone();
            async move {
                service
                    .create_command(
                        storage,
                        target,
                        caller("request"),
                        RequestedConversation::default(),
                    )
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(5), readiness.started.notified())
            .await
            .unwrap();
        let live = original_live(&service, &target).await;
        {
            let ordinary_waiter = live.join_attachment_owner();
            tokio::pin!(ordinary_waiter);
            poll_fn(|cx| {
                assert!(ordinary_waiter.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
            assert!(!command.is_finished());
            assert_eq!(provider.open_calls.load(Ordering::SeqCst), 0);
            release.send(()).unwrap();
            assert_eq!(
                ordinary_waiter.await,
                if panics {
                    Err(CreationTaskFault::Panicked)
                } else {
                    Ok(())
                }
            );
        }
        // A later waiter observes the same retained result, including faults.
        assert_eq!(
            live.join_attachment_owner().await,
            if panics {
                Err(CreationTaskFault::Panicked)
            } else {
                Ok(())
            }
        );
        let outcome = command.await.unwrap();
        if panics {
            assert!(
                matches!(
                    &outcome,
                    Err(CreationFailure::TaskFault(CreationTaskFault::Panicked))
                ),
                "{outcome:?}"
            );
        } else if provider_refuses {
            assert!(
                matches!(
                    &outcome,
                    Err(CreationFailure::Target(ConversationError::Agent(
                        AgentError::Protocol(_)
                    )))
                ),
                "{outcome:?}"
            );
        } else {
            assert_eq!(outcome.unwrap().stage(), CreationStage::Ready);
            assert_eq!(
                live.agent.attachment_status().phase(),
                AttachmentPhase::Attached
            );
        }
        let opens = usize::from(!panics);
        assert_eq!(provider.open_calls.load(Ordering::SeqCst), opens);
        drop(live);
        retire(service, storage, metadata).await;
        let (service, storage, metadata) = fixture(&root, provider.clone());
        let saved = service
            .lookup_creation(
                storage.clone(),
                target.clone(),
                caller("request"),
                RequestedConversation::default(),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            saved.stage(),
            if panics || provider_refuses {
                CreationStage::Attempted
            } else {
                CreationStage::Ready
            }
        );
        let retry = service
            .create_command(
                storage.clone(),
                target.clone(),
                caller("request"),
                RequestedConversation::default(),
            )
            .await;
        if panics || provider_refuses {
            let Err(CreationFailure::Interrupted(original)) = retry else {
                panic!("unconfirmed original attachment was repeated: {retry:?}")
            };
            assert_eq!(original.binding().target(), &conversation_session(&target));
        } else {
            assert_eq!(retry.unwrap(), saved);
        }
        assert_eq!(provider.open_calls.load(Ordering::SeqCst), opens);
        retire(service, storage, metadata).await;
    }
}

#[tokio::test]
async fn creation_commits_the_original_attempt_before_provider_open() {
    let directory = tempfile::tempdir().unwrap();
    let provider = Arc::new(ProviderFactory::default());
    let (release, gate) = oneshot::channel();
    *provider.open_gate.lock().unwrap() = Some(gate);
    let (service, storage, metadata) = fixture(&directory.path().join("data"), provider.clone());
    let target = id();
    let task = tokio::spawn({
        let service = service.clone();
        let storage = storage.clone();
        let target = target.clone();
        async move {
            service
                .create_command(
                    storage,
                    target,
                    caller("first"),
                    RequestedConversation::default(),
                )
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(5), provider.opening.notified())
        .await
        .unwrap();
    // Read the committed SQLite rows through a separate SQL observer while the
    // original principal lease is still held; this observes durable event bytes,
    // rather than the coordinator's in-memory projection.
    let db = nessa_local_database::rusqlite::Connection::open(
        directory.path().join("data/records/records.sqlite3"),
    )
    .unwrap();
    let count: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM event_records WHERE schema_id = 'nessa.creation'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 2);
    drop(db);
    release.send(()).unwrap();
    let ready = task.await.unwrap().unwrap();
    assert_eq!(ready.stage(), CreationStage::Ready);
    assert_eq!(ready.binding().target(), &conversation_session(&target));
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn a_ready_creation_reopens_without_opening_the_provider_again() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let target = id();
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let first = service
        .create_command(
            storage.clone(),
            target.clone(),
            caller("request"),
            RequestedConversation::default(),
        )
        .await
        .unwrap();
    retire(service, storage, metadata).await;
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let retry = service
        .create_command(
            storage.clone(),
            target.clone(),
            caller("request"),
            RequestedConversation::default(),
        )
        .await
        .unwrap();
    assert_eq!(retry, first);
    assert_eq!(
        service
            .lookup_creation(
                storage.clone(),
                target,
                caller("request"),
                RequestedConversation::default()
            )
            .await
            .unwrap(),
        Some(first)
    );
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn a_creation_identity_refuses_changed_target_origin_or_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(&directory.path().join("data"), provider.clone());
    let target = id();
    service
        .create_command(
            storage.clone(),
            target.clone(),
            caller("request"),
            RequestedConversation::default(),
        )
        .await
        .unwrap();
    assert!(matches!(
        service
            .create_command(
                storage.clone(),
                id(),
                caller("request"),
                RequestedConversation::default()
            )
            .await,
        Err(CreationFailure::Conflict)
    ));
    let mut other = caller("request");
    other.surface_id = "other".into();
    assert!(matches!(
        service
            .create_command(
                storage.clone(),
                target.clone(),
                other,
                RequestedConversation::default()
            )
            .await,
        Err(CreationFailure::Conflict)
    ));
    assert!(matches!(
        service
            .create_command(
                storage.clone(),
                target,
                caller("request"),
                RequestedConversation {
                    model: Some("changed".into()),
                    ..Default::default()
                }
            )
            .await,
        Err(CreationFailure::Conflict)
    ));
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn an_interrupted_creation_preserves_its_target_without_reinitialization() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    *provider.open_failure.lock().unwrap() =
        Some(AgentError::Protocol("provider opening refused".into()));
    let target = id();
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let refused = service
        .create_command(
            storage.clone(),
            target.clone(),
            caller("request"),
            RequestedConversation::default(),
        )
        .await;
    assert!(
        matches!(
            &refused,
            Err(CreationFailure::Target(ConversationError::Agent(
                AgentError::Protocol(_)
            )))
        ),
        "original provider refusal must stay typed: {refused:?}"
    );
    retire(service, storage, metadata).await;
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let Err(CreationFailure::Interrupted(receipt)) = service
        .create_command(
            storage.clone(),
            target.clone(),
            caller("request"),
            RequestedConversation::default(),
        )
        .await
    else {
        panic!("original attempt must remain unconfirmed")
    };
    assert_eq!(receipt.binding().target(), &conversation_session(&target));
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn an_accepted_creation_can_make_its_first_attempt_after_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let target = id();
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let lease = storage
        .open_creation("person".into(), true)
        .await
        .unwrap()
        .unwrap();
    lease
        .save(CreationReceipt::accepted(
            binding(
                &target,
                &caller("request"),
                &RequestedConversation::default(),
            )
            .unwrap(),
        ))
        .await
        .unwrap();
    drop(lease);
    retire(service, storage, metadata).await;
    let (service, storage, metadata) = fixture(&root, provider.clone());
    assert_eq!(
        service
            .lookup_creation(
                storage.clone(),
                target.clone(),
                caller("request"),
                RequestedConversation::default()
            )
            .await
            .unwrap()
            .unwrap()
            .stage(),
        CreationStage::Accepted
    );
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        service
            .create_command(
                storage.clone(),
                target,
                caller("request"),
                RequestedConversation::default()
            )
            .await
            .unwrap()
            .stage(),
        CreationStage::Ready
    );
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn deletion_refuses_each_creation_state_and_read_only_lookup() {
    let directory = tempfile::tempdir().unwrap();
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(&directory.path().join("data"), provider.clone());
    for stage in [
        None,
        Some(CreationStage::Accepted),
        Some(CreationStage::Attempted),
        Some(CreationStage::Ready),
    ] {
        let target = id();
        let request = target.to_string();
        let caller = caller(&request);
        metadata
            .create(
                Conversation::new(
                    target.clone(),
                    caller.organization_id.clone(),
                    caller.principal_id.clone(),
                    caller.surface_id.clone(),
                    caller.action_id.clone(),
                    1,
                    AgentId::Claude,
                    ConversationModelId::new("test").unwrap(),
                    ConversationApprovalMode::Ask,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        if let Some(stage) = stage {
            let lease = storage
                .open_creation("person".into(), true)
                .await
                .unwrap()
                .unwrap();
            let accepted = CreationReceipt::accepted(
                binding(&target, &caller, &RequestedConversation::default()).unwrap(),
            );
            lease.save(accepted.clone()).await.unwrap();
            if stage != CreationStage::Accepted {
                let attempted = accepted.advance(CreationStage::Attempted).unwrap();
                lease.save(attempted.clone()).await.unwrap();
                if stage == CreationStage::Ready {
                    lease
                        .save(attempted.advance(CreationStage::Ready).unwrap())
                        .await
                        .unwrap();
                }
            }
            drop(lease);
        }
        metadata
            .record_deletion(
                &target,
                ConversationDeletion::new(
                    caller.organization_id.clone(),
                    caller.principal_id.clone(),
                    "panel".into(),
                    "delete".into(),
                    2,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert!(matches!(
            service
                .create_command(
                    storage.clone(),
                    target.clone(),
                    caller.clone(),
                    RequestedConversation::default()
                )
                .await,
            Err(CreationFailure::Target(ConversationError::Deleted))
        ));
        assert!(matches!(
            service
                .lookup_creation(
                    storage.clone(),
                    target,
                    caller,
                    RequestedConversation::default()
                )
                .await,
            Err(CreationFailure::Target(ConversationError::Deleted))
        ));
    }
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 0);
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn deletion_refuses_finished_creation_lookup_and_retry_after_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let target = id();
    let ready = service
        .create_command(
            storage.clone(),
            target.clone(),
            caller("request"),
            RequestedConversation::default(),
        )
        .await
        .unwrap();
    assert_eq!(ready.stage(), CreationStage::Ready);
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);
    let control_rows = creation_rows(&root);
    assert!(
        control_rows >= 3,
        "accepted, attempted and ready stay in the control stream"
    );
    metadata
        .record_deletion(
            &target,
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
    retire(service, storage, metadata).await;
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(&root, provider.clone());
    assert_eq!(creation_rows(&root), control_rows);
    assert!(matches!(
        service
            .lookup_creation(
                storage.clone(),
                target.clone(),
                caller("request"),
                RequestedConversation::default(),
            )
            .await,
        Err(CreationFailure::Target(ConversationError::Deleted))
    ));
    assert!(matches!(
        service
            .create_command(
                storage.clone(),
                target,
                caller("request"),
                RequestedConversation::default(),
            )
            .await,
        Err(CreationFailure::Target(ConversationError::Deleted))
    ));
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 0);
    assert_eq!(creation_rows(&root), control_rows);
    retire(service, storage, metadata).await;
}

fn creation_rows(root: &Path) -> i64 {
    let db = nessa_local_database::rusqlite::Connection::open(root.join("records/records.sqlite3"))
        .unwrap();
    db.query_row(
        "SELECT COUNT(*) FROM event_records WHERE schema_id = 'nessa.creation'",
        [],
        |row| row.get(0),
    )
    .unwrap()
}

#[tokio::test]
async fn caller_loss_keeps_the_original_creation_owner_until_completion() {
    let directory = tempfile::tempdir().unwrap();
    let provider = Arc::new(ProviderFactory::default());
    let (release, gate) = oneshot::channel();
    *provider.open_gate.lock().unwrap() = Some(gate);
    let (service, storage, metadata) = fixture(&directory.path().join("data"), provider.clone());
    let target = id();
    let task = tokio::spawn({
        let service = service.clone();
        let storage = storage.clone();
        let target = target.clone();
        async move {
            service
                .create_command(
                    storage,
                    target,
                    caller("request"),
                    RequestedConversation::default(),
                )
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(5), provider.opening.notified())
        .await
        .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    // The same host command still owns admission after its caller disappears.
    assert!(service.inner.admission.try_write().is_err());
    assert!(matches!(
        service
            .create_command(
                storage.clone(),
                target.clone(),
                caller("request"),
                RequestedConversation::default()
            )
            .await,
        Err(CreationFailure::Storage(CreationStorageError::Storage(
            StorageError::Busy
        )))
    ));
    release.send(()).unwrap();
    let receipt = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match service
                .lookup_creation(
                    storage.clone(),
                    target.clone(),
                    caller("request"),
                    RequestedConversation::default(),
                )
                .await
            {
                Ok(Some(receipt)) if receipt.stage() == CreationStage::Ready => break receipt,
                Err(CreationFailure::Storage(CreationStorageError::Storage(
                    StorageError::Busy,
                ))) => tokio::task::yield_now().await,
                outcome => panic!("unexpected original creation outcome: {outcome:?}"),
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(receipt.binding().target(), &conversation_session(&target));
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn deletion_during_initialization_refuses_the_returned_receipt() {
    let directory = tempfile::tempdir().unwrap();
    let provider = Arc::new(ProviderFactory::default());
    let (release, gate) = oneshot::channel();
    *provider.open_gate.lock().unwrap() = Some(gate);
    let (service, storage, metadata) = fixture(&directory.path().join("data"), provider.clone());
    let target = id();
    let task = tokio::spawn({
        let service = service.clone();
        let storage = storage.clone();
        let target = target.clone();
        async move {
            service
                .create_command(
                    storage,
                    target,
                    caller("request"),
                    RequestedConversation::default(),
                )
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(5), provider.opening.notified())
        .await
        .unwrap();
    metadata
        .record_deletion(
            &target,
            ConversationDeletion::new(
                OrganizationId::new("org").unwrap(),
                PrincipalId::new("person").unwrap(),
                "panel".into(),
                "delete".into(),
                1_700_000_000_124,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    release.send(()).unwrap();
    assert!(matches!(
        task.await.unwrap(),
        Err(CreationFailure::Target(ConversationError::Deleted))
    ));
    let lease = storage
        .open_creation("person".into(), false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        lease.load("request").await.unwrap().unwrap().stage(),
        CreationStage::Ready
    );
    drop(lease);
    assert!(matches!(
        service
            .lookup_creation(
                storage.clone(),
                target,
                caller("request"),
                RequestedConversation::default()
            )
            .await,
        Err(CreationFailure::Target(ConversationError::Deleted))
    ));
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);
    retire(service, storage, metadata).await;
}

struct LoseReply {
    storage: Arc<RecordStorage>,
    stage: CreationStage,
    remaining: Arc<AtomicBool>,
}
struct LoseReplyLease {
    original: Arc<dyn CreationStorageLease>,
    stage: CreationStage,
    remaining: Arc<AtomicBool>,
}
impl CreationStorage for LoseReply {
    fn open_creation(
        &self,
        principal: String,
        create: bool,
    ) -> CreationFuture<'_, Option<Arc<dyn CreationStorageLease>>> {
        Box::pin(async move {
            Ok(self
                .storage
                .open_creation(principal, create)
                .await?
                .map(|original| {
                    Arc::new(LoseReplyLease {
                        original,
                        stage: self.stage,
                        remaining: self.remaining.clone(),
                    }) as Arc<dyn CreationStorageLease>
                }))
        })
    }
}
impl CreationStorageLease for LoseReplyLease {
    fn load(&self, request: &str) -> CreationFuture<'_, Option<CreationReceipt>> {
        self.original.load(request)
    }
    fn save(&self, receipt: CreationReceipt) -> CreationFuture<'_, ()> {
        Box::pin(async move {
            let stage = receipt.stage();
            self.original.save(receipt).await?;
            if stage == self.stage && self.remaining.swap(false, Ordering::SeqCst) {
                return Err(StorageError::Io("lost original append answer".into()).into());
            }
            Ok(())
        })
    }
}

struct PanicSave {
    storage: Arc<RecordStorage>,
    stage: CreationStage,
    after_commit: bool,
}
struct PanicSaveLease {
    original: Arc<dyn CreationStorageLease>,
    stage: CreationStage,
    after_commit: bool,
}
impl CreationStorage for PanicSave {
    fn open_creation(
        &self,
        principal: String,
        create: bool,
    ) -> CreationFuture<'_, Option<Arc<dyn CreationStorageLease>>> {
        Box::pin(async move {
            Ok(self
                .storage
                .open_creation(principal, create)
                .await?
                .map(|original| {
                    Arc::new(PanicSaveLease {
                        original,
                        stage: self.stage,
                        after_commit: self.after_commit,
                    }) as Arc<dyn CreationStorageLease>
                }))
        })
    }
}
impl CreationStorageLease for PanicSaveLease {
    fn load(&self, request: &str) -> CreationFuture<'_, Option<CreationReceipt>> {
        self.original.load(request)
    }
    fn save(&self, receipt: CreationReceipt) -> CreationFuture<'_, ()> {
        Box::pin(async move {
            let inject = receipt.stage() == self.stage;
            assert!(!inject || self.after_commit, "panic before original save");
            self.original.save(receipt).await?;
            assert!(!inject, "panic after original save");
            Ok(())
        })
    }
}

#[tokio::test]
async fn a_panicked_creation_save_preserves_original_phase_and_provider_ownership() {
    for stage in [
        CreationStage::Accepted,
        CreationStage::Attempted,
        CreationStage::Ready,
    ] {
        for after_commit in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path().join("data");
            let provider = Arc::new(ProviderFactory::default());
            let target = id();
            let original = binding(
                &target,
                &caller("request"),
                &RequestedConversation::default(),
            )
            .unwrap();
            let (service, storage, metadata) = fixture(&root, provider.clone());
            let panicking = Arc::new(PanicSave {
                storage: storage.clone(),
                stage,
                after_commit,
            });
            assert!(matches!(
                service
                    .create_command(
                        panicking.clone(),
                        target.clone(),
                        caller("request"),
                        RequestedConversation::default()
                    )
                    .await,
                Err(CreationFailure::TaskFault(CreationTaskFault::Panicked))
            ));
            let original_opens = usize::from(stage == CreationStage::Ready);
            assert_eq!(provider.open_calls.load(Ordering::SeqCst), original_opens);
            drop(panicking);
            // The panic must relinquish the same admitted host operation and
            // original control reservation so actual shutdown and reopen finish.
            retire(service, storage, metadata).await;
            let (service, storage, metadata) = fixture(&root, provider.clone());
            let expected = match (stage, after_commit) {
                (CreationStage::Accepted, false) => None,
                (CreationStage::Accepted, true) | (CreationStage::Attempted, false) => {
                    Some(CreationStage::Accepted)
                }
                (CreationStage::Attempted, true) | (CreationStage::Ready, false) => {
                    Some(CreationStage::Attempted)
                }
                (CreationStage::Ready, true) => Some(CreationStage::Ready),
            };
            let saved = service
                .lookup_creation(
                    storage.clone(),
                    target.clone(),
                    caller("request"),
                    RequestedConversation::default(),
                )
                .await
                .unwrap();
            assert_eq!(saved.as_ref().map(CreationReceipt::stage), expected);
            if let Some(saved) = saved {
                assert_eq!(saved.binding(), &original);
            }
            assert_eq!(provider.open_calls.load(Ordering::SeqCst), original_opens);
            let retry = service
                .create_command(
                    storage.clone(),
                    target,
                    caller("request"),
                    RequestedConversation::default(),
                )
                .await;
            if expected == Some(CreationStage::Attempted) {
                match retry {
                    Err(CreationFailure::Interrupted(saved)) => {
                        assert_eq!(saved.binding(), &original);
                        assert_eq!(saved.stage(), CreationStage::Attempted);
                    }
                    other => panic!("attempted effect was repeated or lost: {other:?}"),
                }
                assert_eq!(provider.open_calls.load(Ordering::SeqCst), original_opens);
            } else {
                let ready = retry.unwrap();
                assert_eq!(ready.binding(), &original);
                assert_eq!(ready.stage(), CreationStage::Ready);
                assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);
            }
            retire(service, storage, metadata).await;
        }
    }
}

#[tokio::test]
async fn an_uncertain_creation_commit_cannot_authorize_initialization() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let target = id();
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let uncertain = Arc::new(LoseReply {
        storage: storage.clone(),
        stage: CreationStage::Attempted,
        remaining: Arc::new(AtomicBool::new(true)),
    });
    assert!(matches!(
        service
            .create_command(
                uncertain.clone(),
                target.clone(),
                caller("request"),
                RequestedConversation::default()
            )
            .await,
        Err(CreationFailure::Storage(CreationStorageError::Storage(
            StorageError::Io(_)
        )))
    ));
    drop(uncertain);
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 0);
    retire(service, storage, metadata).await;
    let (service, storage, metadata) = fixture(&root, provider.clone());
    assert!(matches!(
        service
            .create_command(
                storage.clone(),
                target,
                caller("request"),
                RequestedConversation::default()
            )
            .await,
        Err(CreationFailure::Interrupted(_))
    ));
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 0);
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn lost_ready_commit_answer_recovers_the_original_success_without_opening_again() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let target = id();
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let uncertain = Arc::new(LoseReply {
        storage: storage.clone(),
        stage: CreationStage::Ready,
        remaining: Arc::new(AtomicBool::new(true)),
    });
    assert!(matches!(
        service
            .create_command(
                uncertain.clone(),
                target.clone(),
                caller("request"),
                RequestedConversation::default()
            )
            .await,
        Err(CreationFailure::Storage(CreationStorageError::Storage(
            StorageError::Io(_)
        )))
    ));
    drop(uncertain);
    retire(service, storage, metadata).await;
    let (service, storage, metadata) = fixture(&root, provider.clone());
    assert_eq!(
        service
            .create_command(
                storage.clone(),
                target,
                caller("request"),
                RequestedConversation::default()
            )
            .await
            .unwrap()
            .stage(),
        CreationStage::Ready
    );
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn a_second_creation_request_cannot_reinitialize_the_original_target() {
    let directory = tempfile::tempdir().unwrap();
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(&directory.path().join("data"), provider.clone());
    let target = id();
    service
        .create_command(
            storage.clone(),
            target.clone(),
            caller("original"),
            RequestedConversation::default(),
        )
        .await
        .unwrap();
    assert!(matches!(
        service
            .create_command(
                storage.clone(),
                target,
                caller("another"),
                RequestedConversation::default()
            )
            .await,
        Err(CreationFailure::Target(ConversationError::RequestConflict))
    ));
    let lease = storage
        .open_creation("person".into(), false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        lease.load("another").await.unwrap().unwrap().stage(),
        CreationStage::Accepted
    );
    drop(lease);
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);
    retire(service, storage, metadata).await;
}

struct GateReady {
    storage: Arc<RecordStorage>,
    started: Arc<Notify>,
    gate: Arc<AsyncMutex<Option<oneshot::Receiver<()>>>>,
}
struct GateReadyLease {
    original: Arc<dyn CreationStorageLease>,
    started: Arc<Notify>,
    gate: Arc<AsyncMutex<Option<oneshot::Receiver<()>>>>,
}
impl CreationStorage for GateReady {
    fn open_creation(
        &self,
        principal: String,
        create: bool,
    ) -> CreationFuture<'_, Option<Arc<dyn CreationStorageLease>>> {
        Box::pin(async move {
            Ok(self
                .storage
                .open_creation(principal, create)
                .await?
                .map(|original| {
                    Arc::new(GateReadyLease {
                        original,
                        started: self.started.clone(),
                        gate: self.gate.clone(),
                    }) as Arc<dyn CreationStorageLease>
                }))
        })
    }
}
impl CreationStorageLease for GateReadyLease {
    fn load(&self, request: &str) -> CreationFuture<'_, Option<CreationReceipt>> {
        self.original.load(request)
    }
    fn save(&self, receipt: CreationReceipt) -> CreationFuture<'_, ()> {
        Box::pin(async move {
            if receipt.stage() == CreationStage::Ready {
                if let Some(gate) = self.gate.lock().await.take() {
                    self.started.notify_one();
                    gate.await.map_err(|_| StorageError::Closed)?;
                }
            }
            self.original.save(receipt).await
        })
    }
}

#[tokio::test]
async fn retirement_joins_the_original_creation_receipt_owner() {
    let directory = tempfile::tempdir().unwrap();
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(&directory.path().join("data"), provider.clone());
    let target = id();
    let started = Arc::new(Notify::new());
    let (release, gate) = oneshot::channel();
    let gated = Arc::new(GateReady {
        storage: storage.clone(),
        started: started.clone(),
        gate: Arc::new(AsyncMutex::new(Some(gate))),
    });
    let task = tokio::spawn({
        let service = service.clone();
        let target = target.clone();
        let gated = gated.clone();
        async move {
            service
                .create_command(
                    gated,
                    target,
                    caller("request"),
                    RequestedConversation::default(),
                )
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(5), started.notified())
        .await
        .unwrap();
    {
        // Observe the actual admission owner, not a cooperative shutdown yield.
        assert!(service.inner.admission.try_write().is_err());
        let shutdown = service.shutdown();
        tokio::pin!(shutdown);
        poll_fn(|cx| {
            assert!(shutdown.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        release.send(()).unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), task)
                .await
                .unwrap()
                .unwrap()
                .unwrap()
                .stage(),
            CreationStage::Ready
        );
        tokio::time::timeout(Duration::from_secs(5), shutdown)
            .await
            .unwrap()
            .unwrap();
    }
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);
    drop(gated);
    drop(service);
    drop(storage);
    drop(metadata);
}

struct ChildOwner(Child);
impl Drop for ChildOwner {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
#[ignore = "invoked only by the crash-window parent fixture"]
fn creation_crash_child() {
    let root = std::env::var_os("NESSA_CREATION_CRASH_ROOT").expect("child fixture root");
    let target = std::env::var("NESSA_CREATION_CRASH_TARGET").expect("child fixture target");
    let root = PathBuf::from(root);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let provider = Arc::new(ProviderFactory::default());
        let (service, storage, _metadata) = fixture(&root, provider.clone());
        let started = Arc::new(Notify::new());
        let (_release, gate) = oneshot::channel();
        let gated = Arc::new(GateReady {
            storage,
            started: started.clone(),
            gate: Arc::new(AsyncMutex::new(Some(gate))),
        });
        tokio::spawn(async move {
            service
                .create_command(
                    gated,
                    ConversationId::new(&target).unwrap(),
                    caller("crash-request"),
                    RequestedConversation::default(),
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(5), started.notified())
            .await
            .unwrap();
        assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);
        // This harness marker is written only after the actual initializer has
        // returned success and the original terminal append is parked on its gate.
        let marker = root.join("provider-opened.pending");
        std::fs::write(&marker, b"1").unwrap();
        // Publish only the complete observation: the parent must not see the
        // create/truncate window of a write to its polling path.
        std::fs::rename(marker, root.join("provider-opened")).unwrap();
        std::future::pending::<()>().await;
    });
}

#[tokio::test]
async fn a_crash_after_provider_open_reopens_the_original_interrupted_receipt() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let target = id();
    let child_log = directory.path().join("creation-crash-child.log");
    let child_output = std::fs::File::create(&child_log).unwrap();
    let child_error = child_output.try_clone().unwrap();
    let mut child = ChildOwner(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "conversation::application::service::creation::tests::creation_crash_child",
                "--ignored",
                "--nocapture",
            ])
            .env("NESSA_CREATION_CRASH_ROOT", &root)
            .env("NESSA_CREATION_CRASH_TARGET", target.to_string())
            .stdout(Stdio::from(child_output))
            .stderr(Stdio::from(child_error))
            .spawn()
            .unwrap(),
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match std::fs::read(root.join("provider-opened")) {
                Ok(bytes) => {
                    assert_eq!(bytes, b"1");
                    break;
                }
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => panic!("cannot observe original provider marker: {error}"),
            }
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "crash fixture exited before its actual provider open: {}",
                std::fs::read_to_string(&child_log).unwrap_or_default()
            );
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
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
    assert!(snapshot.provider_context.recorded().is_some());
    drop(saved);
    let observed = service
        .lookup_creation(
            storage.clone(),
            target.clone(),
            caller("crash-request"),
            RequestedConversation::default(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(observed.stage(), CreationStage::Attempted);
    assert_eq!(observed.binding().target(), &conversation_session(&target));
    assert!(matches!(
        service
            .create_command(
                storage.clone(),
                target,
                caller("crash-request"),
                RequestedConversation::default()
            )
            .await,
        Err(CreationFailure::Interrupted(_))
    ));
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 0);
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn a_ready_receipt_does_not_invent_missing_target_metadata() {
    let directory = tempfile::tempdir().unwrap();
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(&directory.path().join("data"), provider.clone());
    let target = id();
    let lease = storage
        .open_creation("person".into(), true)
        .await
        .unwrap()
        .unwrap();
    let accepted = CreationReceipt::accepted(
        binding(
            &target,
            &caller("request"),
            &RequestedConversation::default(),
        )
        .unwrap(),
    );
    lease.save(accepted.clone()).await.unwrap();
    let attempted = accepted.advance(CreationStage::Attempted).unwrap();
    lease.save(attempted.clone()).await.unwrap();
    lease
        .save(attempted.advance(CreationStage::Ready).unwrap())
        .await
        .unwrap();
    drop(lease);
    assert!(matches!(
        service
            .create_command(
                storage.clone(),
                target.clone(),
                caller("request"),
                RequestedConversation::default()
            )
            .await,
        Err(CreationFailure::Target(ConversationError::NotFound))
    ));
    assert!(matches!(
        service
            .lookup_creation(
                storage.clone(),
                target,
                caller("request"),
                RequestedConversation::default()
            )
            .await,
        Err(CreationFailure::Target(ConversationError::NotFound))
    ));
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 0);
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn close_before_attachment_keeps_creation_attempted_after_original_task_completion() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let (_release, gate) = oneshot::channel();
    let readiness = Arc::new(CreationReadiness {
        started: Notify::new(),
        release: AsyncMutex::new(Some(gate)),
        panics: false,
    });
    let agents = ConversationAgents::from_source(
        HashSet::from([AgentId::Claude]),
        AgentId::Claude,
        Arc::new(CreationReadinessSource {
            original: only(Arc::new(Provider::new(provider.clone()))),
            readiness: readiness.clone(),
        }),
    )
    .unwrap();
    let (service, storage, metadata) = fixture_with_agents(&root, agents);
    let target = id();
    let task = tokio::spawn({
        let service = service.clone();
        let storage = storage.clone();
        let target = target.clone();
        async move {
            service
                .create_command(
                    storage,
                    target,
                    caller("request"),
                    RequestedConversation::default(),
                )
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(5), readiness.started.notified())
        .await
        .unwrap();
    let live = original_live(&service, &target).await;
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 0);
    // Existing SDK close cancels the original authorization while host readiness
    // is held. The task can finish successfully without opening a provider.
    tokio::time::timeout(
        Duration::from_secs(5),
        live.agent
            .close(caller("close-before-attachment").actor().unwrap()),
    )
    .await
    .unwrap()
    .unwrap();
    tokio::time::timeout(Duration::from_secs(5), live.join_attachment_owner())
        .await
        .unwrap()
        .unwrap();
    let status = live.agent.attachment_status();
    assert_ne!(status.phase(), AttachmentPhase::Attached);
    assert!(status.failure().is_none());
    assert!(status.evidence_failure().is_none());
    let outcome = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            outcome,
            Err(CreationFailure::Target(ConversationError::Agent(
                AgentError::AttachmentUnavailable(_)
            )))
        ),
        "{outcome:?}"
    );
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 0);
    drop(live);
    retire(service, storage, metadata).await;
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let saved = service
        .lookup_creation(
            storage.clone(),
            target.clone(),
            caller("request"),
            RequestedConversation::default(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.stage(), CreationStage::Attempted);
    assert_eq!(saved.binding().target(), &conversation_session(&target));
    assert!(matches!(
        service
            .create_command(
                storage.clone(),
                target,
                caller("request"),
                RequestedConversation::default(),
            )
            .await,
        Err(CreationFailure::Interrupted(original)) if *original == saved
    ));
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 0);
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn read_only_creation_refuses_a_conflicting_original_binding() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let target = id();
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let ready = service
        .create_command(
            storage.clone(),
            target.clone(),
            caller("request"),
            RequestedConversation::default(),
        )
        .await
        .unwrap();
    retire(service, storage, metadata).await;
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let mut other_origin = caller("request");
    other_origin.surface_id = "other".into();
    for (requested_target, requested_caller, requested) in [
        (id(), caller("request"), RequestedConversation::default()),
        (
            target.clone(),
            other_origin,
            RequestedConversation::default(),
        ),
        (
            target.clone(),
            caller("request"),
            RequestedConversation {
                model: Some("changed".into()),
                ..Default::default()
            },
        ),
    ] {
        assert!(matches!(
            service
                .lookup_creation(
                    storage.clone(),
                    requested_target,
                    requested_caller,
                    requested
                )
                .await,
            Err(CreationFailure::Conflict)
        ));
    }
    assert_eq!(
        service
            .lookup_creation(
                storage.clone(),
                target,
                caller("request"),
                RequestedConversation::default(),
            )
            .await
            .unwrap(),
        Some(ready)
    );
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);
    let db = nessa_local_database::rusqlite::Connection::open(root.join("records/records.sqlite3"))
        .unwrap();
    let count: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM event_records WHERE schema_id = 'nessa.creation'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 3);
    drop(db);
    retire(service, storage, metadata).await;
}
