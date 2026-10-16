//! Block each actual observation phase, then prove source cancellation/deadline
//! releases the caller and its reader slot without waiting for the blocked port.
use super::*;
use crate::conversation::application::{
    ConversationDependencies, ConversationLimits, ProviderSessionErasers,
};
use crate::conversation::domain::Conversation;
use crate::conversation_test_support::{
    self as support, AcceptingCreationAudit, AcceptingDeletionAudit, AcceptingModeAudit,
    MemoryRepository, MemorySummaries, Provider, ProviderFactory, RecordingFileLinkAudit,
    TestClock, Unlisted, DELETION_BUDGETS,
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::conversation::{
    domain::{ConversationApprovalMode, ConversationModelId},
    projection::Projection,
    view::{ConversationCapabilities, ConversationPending, ConversationPendingMode},
};
use nessa_sdk::application::agent_execution::sessions::{
    CommittedEventReader, EventFuture, SessionStorage, SessionStorageLease, StorageError,
    StorageFuture,
};
use nessa_sdk::infrastructure::session_storage::RuntimeMessageCommitClock;
use nessa_sync::replication::domain::Id;
use std::sync::Mutex;
use tokio::sync::oneshot;

fn id() -> ConversationId {
    thread("00000000-0000-4000-8000-000000000001").unwrap()
}
fn caller() -> ConversationCaller {
    ConversationCaller {
        organization_id: OrganizationId::new("org").unwrap(),
        principal_id: PrincipalId::new("owner").unwrap(),
        surface_id: "panel".into(),
        action_id: "observe".into(),
    }
}

#[test]
fn default_stop_does_not_select_a_queued_turn_in_an_idle_view() {
    let mut view = Projection::new(
        id().to_string(),
        ConversationCapabilities::read_only(),
        None,
    )
    .read();
    view.pending.push(ConversationPending {
        execution_id: "future-turn".into(),
        text: "queued".into(),
        attachments: Vec::new(),
        files: Vec::new(),
        app: None,
        mode: ConversationPendingMode::Queued,
    });
    assert_eq!(active_execution(&view), None);
    assert_eq!(view.pending[0].execution_id, "future-turn");
}

#[tokio::test(start_paused = true)]
async fn source_admission_cancel_and_deadline_release_the_tool_reader_slot() {
    for cancel in [true, false] {
        let (service, _provider, metadata, _storage) =
            support::fixture(ConversationLimits::default());
        let (entered, read_entered) = oneshot::channel();
        let (release, read_release) = oneshot::channel();
        *metadata.metadata_load_gate.lock().unwrap() = Some((entered, read_release));
        let tools = ConversationTools::new();
        tools.attach(service.clone());
        let (stop, stopped) = watch::channel(false);
        let work = tools.call(
            &SessionId::new(&id().to_string()).unwrap(),
            "call".into(),
            "subscribe".into(),
            json!({"threadId":id().to_string(),"waitMs":30000}),
            stopped,
        );
        let work = tokio::spawn(work);
        read_entered.await.unwrap();
        assert_eq!(tools.readers.available_permits(), MAX_EVENT_READERS - 1);
        if cancel {
            stop.send_replace(true);
        } else {
            tokio::time::advance(Duration::from_secs(10)).await;
        }
        let result = tokio::time::timeout(Duration::from_secs(1), work)
            .await
            .expect("blocked source lookup must not retain the subscription")
            .unwrap();
        assert_eq!(
            result["structuredContent"]["error"],
            if cancel {
                "cancelled"
            } else {
                "event_source_preparing"
            }
        );
        assert_eq!(tools.readers.available_permits(), MAX_EVENT_READERS);
        // Existing supervised source admission retains its physical/audit owner;
        // release that owner independently before shutting the fixture down.
        let _ = release.send(());
        service.shutdown().await.unwrap();
    }
}

#[derive(Clone, Copy)]
enum Phase {
    Open,
    Reauthorize,
    Read,
}
struct Gate {
    entered: oneshot::Sender<()>,
    release: oneshot::Receiver<()>,
}
struct GatedStorage {
    phase: Phase,
    gate: Arc<Mutex<Option<Gate>>>,
    metadata: Arc<MemoryRepository>,
}
struct Reader {
    cursor: CommittedEventCursor,
    gate: Arc<Mutex<Option<Gate>>>,
    block: bool,
}
impl SessionStorage for GatedStorage {
    fn open(&self, _id: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
        Box::pin(async { Err(StorageError::CommittedReadUnavailable) })
    }
    fn events(
        &self,
        id: SessionId,
        _start: EventStart,
    ) -> EventFuture<'_, Box<dyn CommittedEventReader>> {
        Box::pin(async move {
            if matches!(self.phase, Phase::Open) {
                let gate = self.gate.lock().unwrap().take().unwrap();
                let _ = gate.entered.send(());
                let _ = gate.release.await;
            }
            if matches!(self.phase, Phase::Reauthorize) {
                let gate = self.gate.lock().unwrap().take().unwrap();
                *self.metadata.metadata_load_gate.lock().unwrap() =
                    Some((gate.entered, gate.release));
            }
            Ok(Box::new(Reader {
                cursor: CommittedEventCursor::new(id, Id::new("incarnation").unwrap(), 0, None),
                gate: self.gate.clone(),
                block: matches!(self.phase, Phase::Read),
            }) as Box<dyn CommittedEventReader>)
        })
    }
}
impl CommittedEventReader for Reader {
    fn read(&mut self, _limits: EventReadLimits) -> EventFuture<'_, EventRead> {
        let gate = if self.block {
            self.gate.lock().unwrap().take()
        } else {
            None
        };
        Box::pin(async move {
            if let Some(gate) = gate {
                let _ = gate.entered.send(());
                let _ = gate.release.await;
            }
            Ok(EventRead::Idle(self.cursor.clone()))
        })
    }
}
fn observing_service(
    metadata: Arc<MemoryRepository>,
    storage: Arc<dyn SessionStorage>,
) -> ConversationService {
    let provider = Arc::new(ProviderFactory::default());
    let summaries = Arc::new(MemorySummaries::default());
    let owner = caller();
    metadata.records.lock().unwrap().insert(
        id(),
        Conversation::new(
            id(),
            owner.organization_id,
            owner.principal_id,
            "panel".into(),
            "create".into(),
            1,
            AgentId::Claude,
            ConversationModelId::new("model").unwrap(),
            ConversationApprovalMode::Ask,
        )
        .unwrap(),
    );
    ConversationService::new(
        ConversationDependencies {
            access_policy: support::access_policy(),
            agents: support::only(Arc::new(Provider::new(provider))),
            storage,
            metadata,
            mode_audit: Arc::new(AcceptingModeAudit),
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            summaries,
            listing: Arc::new(Unlisted),
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
    .unwrap()
}

#[tokio::test(start_paused = true)]
async fn reader_open_reauthorization_and_read_obey_cancellation_and_setup_deadline() {
    for phase in [Phase::Open, Phase::Reauthorize, Phase::Read] {
        for cancel in [true, false] {
            let metadata = Arc::new(MemoryRepository::default());
            let (entered, read_entered) = oneshot::channel();
            let (release, read_release) = oneshot::channel();
            let storage = Arc::new(GatedStorage {
                phase,
                metadata: metadata.clone(),
                gate: Arc::new(Mutex::new(Some(Gate {
                    entered,
                    release: read_release,
                }))),
            });
            let service = observing_service(metadata, storage);
            let (stop, stopped) = watch::channel(false);
            let control = SubscriptionControl::new(stopped, 30000);
            let owned = service.clone();
            let work = tokio::spawn(async move {
                follow(
                    &owned,
                    id(),
                    caller(),
                    Follow {
                        thread_id: id().to_string(),
                        after: None,
                        replay: None,
                        wait_ms: Some(30000),
                        limit: None,
                    },
                    control,
                )
                .await
            });
            read_entered.await.unwrap();
            if cancel {
                stop.send_replace(true);
            } else {
                tokio::time::advance(Duration::from_secs(10)).await;
            }
            let result = tokio::time::timeout(Duration::from_secs(1), work)
                .await
                .expect("blocked observation phase must return at its bound")
                .unwrap();
            assert_eq!(
                result.unwrap_err(),
                if cancel {
                    "cancelled"
                } else {
                    "event_source_preparing"
                }
            );
            let _ = release.send(());
            service.shutdown().await.unwrap();
        }
    }
}
