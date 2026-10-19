//! Block each actual observation phase, then prove source cancellation/deadline
//! releases the caller and its reader slot without waiting for the blocked port.
use super::*;
use crate::conversation::application::{
    ConversationDependencies, ConversationLimits, ProviderSessionErasers, MAX_AGENT_TOOL_ADMISSIONS,
};
use crate::conversation::domain::Conversation;
use crate::conversation_test_support::{
    self as support, AcceptingCreationAudit, AcceptingDeletionAudit, AcceptingModeAudit,
    MemoryRepository, MemorySummaries, Provider, ProviderFactory, RecordingFileLinkAudit,
    TestClock, Unlisted, DELETION_BUDGETS,
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::conversation::domain::{ConversationApprovalMode, ConversationModelId};
use nessa_sdk::application::agent_execution::sessions::{
    CommittedEvent, CommittedEventPage, CommittedEventReader, EventFuture, EventReadError,
    SessionChange, SessionStorage, SessionStorageLease, StorageError, StorageFuture,
};
use nessa_sdk::domain::agent_execution::executions::{ExecutionId, ExecutionOutcome};
use nessa_sdk::infrastructure::session_storage::RuntimeMessageCommitClock;
use nessa_sync::replication::domain::Id;
use std::{collections::VecDeque, sync::Mutex};
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
            &SessionId::new(id().to_string()).unwrap(),
            "call".into(),
            "subscribe".into(),
            json!({"threadId":id().to_string(),"waitMs":30000}),
            stopped,
        );
        let work = tokio::spawn(work);
        read_entered.await.unwrap();
        assert_eq!(tools.readers.held(&id()), 1);
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
            result.result["structuredContent"]["error"],
            if cancel {
                "cancelled"
            } else {
                "event_source_preparing"
            }
        );
        assert_eq!(tools.readers.held(&id()), 0);
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
            let control = SubscriptionControl::new(stopped, 30000, None);
            let owned = service.clone();
            let work = tokio::spawn(async move {
                follow(
                    &owned,
                    id(),
                    caller(),
                    Follow {
                        thread_id: id().to_string(),
                        execution_id: None,
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

#[tokio::test(start_paused = true)]
async fn repeated_cancelled_observers_cannot_exceed_detached_source_capacity() {
    let (service, _provider, metadata, _storage) = support::fixture(ConversationLimits::default());
    let tools = ConversationTools::new();
    tools.attach(service.clone());
    let mut releases = Vec::new();
    for attempt in 0..MAX_AGENT_TOOL_ADMISSIONS {
        let (entered, read_entered) = oneshot::channel();
        let (release, read_release) = oneshot::channel();
        *metadata.metadata_load_gate.lock().unwrap() = Some((entered, read_release));
        let (stop, stopped) = watch::channel(false);
        let work = tokio::spawn(tools.call(
            &SessionId::new(id().to_string()).unwrap(),
            format!("call-{attempt}"),
            "subscribe".into(),
            json!({"threadId":id().to_string(),"waitMs":30000}),
            stopped,
        ));
        read_entered.await.unwrap();
        stop.send_replace(true);
        let result = tokio::time::timeout(Duration::from_secs(1), work)
            .await
            .expect("source cancellation must release the response")
            .unwrap();
        assert_eq!(result.result["structuredContent"]["error"], "cancelled");
        assert_eq!(
            tools.readers.held(&id()),
            0,
            "the observation cap must not mask retained source owners"
        );
        releases.push(release);
    }
    let (entered, mut read_entered) = oneshot::channel();
    let (release_other, read_release) = oneshot::channel();
    *metadata.metadata_load_gate.lock().unwrap() = Some((entered, read_release));
    let (_stop, stopped) = watch::channel(false);
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        tools.call(
            &SessionId::new(id().to_string()).unwrap(),
            "over-capacity".into(),
            "subscribe".into(),
            json!({"threadId":id().to_string()}),
            stopped,
        ),
    )
    .await
    .expect("physical source capacity refuses without waiting");
    assert_eq!(
        result.result["structuredContent"]["error"],
        error_code(&ConversationError::Unavailable).as_str()
    );
    assert!(
        matches!(
            read_entered.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ),
        "capacity refusal must precede source metadata I/O"
    );
    // Another agent is not counted against this one's admissions: its call
    // reaches the metadata read the full source was refused before.
    let other = thread("00000000-0000-4000-8000-000000000002").unwrap();
    let (stop_other, stopped) = watch::channel(false);
    let work = tokio::spawn(tools.call(
        &SessionId::new(other.to_string()).unwrap(),
        "other-source".into(),
        "subscribe".into(),
        json!({"threadId":other.to_string(),"waitMs":30000}),
        stopped,
    ));
    tokio::time::timeout(Duration::from_secs(1), read_entered)
        .await
        .expect("another source is admitted while this one is full")
        .unwrap();
    assert_eq!(tools.readers.held(&other), 1);
    stop_other.send_replace(true);
    let result = tokio::time::timeout(Duration::from_secs(1), work)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.result["structuredContent"]["error"], "cancelled");
    drop(release_other);
    drop(metadata.metadata_load_gate.lock().unwrap().take());
    let owned = service.clone();
    let mut shutdown = tokio::spawn(async move { owned.shutdown().await });
    assert!(
        tokio::time::timeout(Duration::from_millis(1), &mut shutdown)
            .await
            .is_err(),
        "the detached owners must still retain service admission during shutdown"
    );
    for release in releases {
        let _ = release.send(());
    }
    tokio::time::timeout(Duration::from_secs(1), shutdown)
        .await
        .expect("shutdown drains after physical source work releases")
        .unwrap()
        .unwrap();
}

#[derive(Clone, Copy)]
enum DeliveryPhase {
    Page,
    Capacity,
    Idle,
    ReadDeadline,
    AuthorizeDeadline,
    SleepDeadline,
    FinalAuthorize,
}
struct DeliveryStorage {
    phase: DeliveryPhase,
    gate: Arc<Mutex<Option<Gate>>>,
    metadata: Arc<MemoryRepository>,
}
struct DeliveryReader {
    phase: DeliveryPhase,
    cursor: CommittedEventCursor,
    gate: Arc<Mutex<Option<Gate>>>,
    metadata: Arc<MemoryRepository>,
    first: bool,
}
impl SessionStorage for DeliveryStorage {
    fn open(&self, _: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
        Box::pin(async { Err(StorageError::CommittedReadUnavailable) })
    }
    fn events(
        &self,
        id: SessionId,
        _: EventStart,
    ) -> EventFuture<'_, Box<dyn CommittedEventReader>> {
        Box::pin(async move {
            Ok(Box::new(DeliveryReader {
                phase: self.phase,
                cursor: CommittedEventCursor::new(id, Id::new("incarnation").unwrap(), 1, None),
                gate: self.gate.clone(),
                metadata: self.metadata.clone(),
                first: true,
            }) as Box<dyn CommittedEventReader>)
        })
    }
}
impl CommittedEventReader for DeliveryReader {
    fn read(&mut self, _: EventReadLimits) -> EventFuture<'_, EventRead> {
        let first = std::mem::replace(&mut self.first, false);
        Box::pin(async move {
            let block_read = matches!(
                self.phase,
                DeliveryPhase::Page | DeliveryPhase::Idle | DeliveryPhase::Capacity
            ) || (!first && matches!(self.phase, DeliveryPhase::ReadDeadline));
            if block_read {
                let gate = self.gate.lock().unwrap().take();
                if let Some(gate) = gate {
                    let _ = gate.entered.send(());
                    let _ = gate.release.await;
                }
            } else if first && !matches!(self.phase, DeliveryPhase::ReadDeadline) {
                let gate = self.gate.lock().unwrap().take();
                if let Some(gate) = gate {
                    if matches!(self.phase, DeliveryPhase::SleepDeadline) {
                        let _ = gate.entered.send(());
                    } else if matches!(
                        self.phase,
                        DeliveryPhase::AuthorizeDeadline | DeliveryPhase::FinalAuthorize
                    ) {
                        *self.metadata.metadata_load_gate.lock().unwrap() =
                            Some((gate.entered, gate.release));
                    } else {
                        *self.gate.lock().unwrap() = Some(gate);
                    }
                }
            }
            if matches!(self.phase, DeliveryPhase::Capacity) {
                return Err(
                    nessa_sdk::application::agent_execution::sessions::EventReadError::Capacity,
                );
            }
            if matches!(
                self.phase,
                DeliveryPhase::Page | DeliveryPhase::FinalAuthorize
            ) {
                use nessa_sdk::application::agent_execution::sessions::{
                    CommittedEvent, CommittedEventPage, SessionChange,
                };
                use nessa_sdk::domain::agent_execution::executions::{
                    ExecutionId, ExecutionOutcome,
                };
                let next = CommittedEventCursor::new(
                    self.cursor.session().clone(),
                    Id::new("incarnation").unwrap(),
                    1,
                    Some(1),
                );
                let event = CommittedEvent::change(
                    next.clone(),
                    SessionChange::LocalSettlement {
                        execution_id: ExecutionId::new("finished").unwrap(),
                        // A settlement update, not a turn's end: these pages
                        // are about cursors, not the reply an end looks up.
                        before: Some(Ok(ExecutionOutcome::Completed)),
                        after: Ok(ExecutionOutcome::Completed),
                        local_outcome: None,
                    },
                );
                Ok(EventRead::Page(
                    CommittedEventPage::new(vec![event], next, self.cursor.clone()).unwrap(),
                ))
            } else {
                Ok(EventRead::Idle(self.cursor.clone()))
            }
        })
    }
}
fn delete_target(metadata: &MemoryRepository) {
    use crate::conversation::domain::ConversationDeletion;
    let mut records = metadata.records.lock().unwrap();
    let deleted = records
        .get(&id())
        .unwrap()
        .clone()
        .deleted(
            ConversationDeletion::new(
                caller().organization_id,
                caller().principal_id,
                "panel".into(),
                "delete".into(),
                2,
            )
            .unwrap(),
        )
        .unwrap();
    records.insert(id(), deleted);
}
fn delivery_call(
    service: ConversationService,
    phase: DeliveryPhase,
    stop: watch::Receiver<bool>,
) -> tokio::task::JoinHandle<Result<Value, String>> {
    tokio::spawn(async move {
        let wait = if matches!(
            phase,
            DeliveryPhase::Page
                | DeliveryPhase::Idle
                | DeliveryPhase::Capacity
                | DeliveryPhase::FinalAuthorize
        ) {
            0
        } else {
            1000
        };
        follow(
            &service,
            id(),
            caller(),
            Follow {
                thread_id: id().to_string(),
                execution_id: None,
                after: None,
                replay: None,
                wait_ms: Some(wait),
                limit: None,
            },
            SubscriptionControl::new(stop, wait, None),
        )
        .await
        .map(|(value, _)| value)
    })
}

#[tokio::test(start_paused = true)]
async fn delivery_reauthorizes_page_idle_and_cached_deadline_after_target_deletion() {
    for phase in [
        DeliveryPhase::Page,
        DeliveryPhase::Idle,
        DeliveryPhase::Capacity,
        DeliveryPhase::ReadDeadline,
        DeliveryPhase::AuthorizeDeadline,
        DeliveryPhase::SleepDeadline,
    ] {
        for deleted in [false, true] {
            let metadata = Arc::new(MemoryRepository::default());
            let (entered, read_entered) = oneshot::channel();
            let (release, read_release) = oneshot::channel();
            let service = observing_service(
                metadata.clone(),
                Arc::new(DeliveryStorage {
                    phase,
                    metadata: metadata.clone(),
                    gate: Arc::new(Mutex::new(Some(Gate {
                        entered,
                        release: read_release,
                    }))),
                }),
            );
            let (_stop, stopped) = watch::channel(false);
            let work = delivery_call(service.clone(), phase, stopped);
            read_entered.await.unwrap();
            if deleted {
                delete_target(&metadata);
            }
            if matches!(
                phase,
                DeliveryPhase::Page | DeliveryPhase::Idle | DeliveryPhase::Capacity
            ) {
                release.send(()).unwrap();
            } else {
                tokio::time::advance(Duration::from_secs(1)).await;
                drop(release);
            }
            let result = work.await.unwrap();
            if matches!(phase, DeliveryPhase::Capacity) {
                // Busy read workers are retried, not refused: the retry
                // reauthorizes, and a source that never frees ends at the
                // preparation bound.
                assert_eq!(
                    result.unwrap_err(),
                    if deleted {
                        "conversation_deleted"
                    } else {
                        "event_source_preparing"
                    }
                );
            } else if deleted {
                assert_eq!(result.unwrap_err(), "conversation_deleted");
            } else {
                let value = result.unwrap();
                assert!(value["cursor"].is_string());
                if matches!(phase, DeliveryPhase::Page) {
                    assert_eq!(value["events"][0]["executionId"], "finished");
                } else {
                    assert_eq!(value["events"], json!([]));
                }
            }
            service.shutdown().await.unwrap();
        }
    }
}

#[tokio::test(start_paused = true)]
async fn blocked_delivery_authorization_cancels_or_times_out_without_cursor() {
    for cancel in [true, false] {
        let metadata = Arc::new(MemoryRepository::default());
        let (entered, read_entered) = oneshot::channel();
        let (_release, read_release) = oneshot::channel();
        let service = observing_service(
            metadata.clone(),
            Arc::new(DeliveryStorage {
                phase: DeliveryPhase::FinalAuthorize,
                metadata: metadata.clone(),
                gate: Arc::new(Mutex::new(Some(Gate {
                    entered,
                    release: read_release,
                }))),
            }),
        );
        let (stop, stopped) = watch::channel(false);
        let work = delivery_call(service.clone(), DeliveryPhase::FinalAuthorize, stopped);
        read_entered.await.unwrap();
        if cancel {
            stop.send_replace(true);
        } else {
            tokio::time::advance(Duration::from_secs(10)).await;
        }
        assert_eq!(
            work.await.unwrap().unwrap_err(),
            if cancel {
                "cancelled"
            } else {
                "temporarily_unavailable"
            }
        );
        service.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn already_observed_cancellation_refuses_delivery_before_polling_authorization() {
    let (stop, mut stopped) = watch::channel(false);
    stop.send_replace(true);
    assert!(*stopped.borrow_and_update());
    let mut control = SubscriptionControl::new(stopped, 0, None);
    let mut polled = false;
    let result = control
        .step_until(
            async {
                polled = true;
            },
            Instant::now() + Duration::from_secs(10),
        )
        .await;
    assert!(matches!(result, Err(SubscriptionEnd::Cancelled)));
    assert!(!polled);
}

/// Deliberately ignores EventStart so the actual follower must correlate output.
struct ScriptedStorage {
    reads: Mutex<Option<VecDeque<EventRead>>>,
    opened: Mutex<Option<EventStart>>,
}
struct ScriptedReader(VecDeque<EventRead>);
impl SessionStorage for ScriptedStorage {
    fn open(&self, _: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
        Box::pin(async { Err(StorageError::CommittedReadUnavailable) })
    }
    fn events(
        &self,
        _: SessionId,
        start: EventStart,
    ) -> EventFuture<'_, Box<dyn CommittedEventReader>> {
        Box::pin(async move {
            *self.opened.lock().unwrap() = Some(start);
            let reads = self.reads.lock().unwrap().take().unwrap();
            Ok(Box::new(ScriptedReader(reads)) as Box<dyn CommittedEventReader>)
        })
    }
}
impl CommittedEventReader for ScriptedReader {
    fn read(&mut self, _: EventReadLimits) -> EventFuture<'_, EventRead> {
        let read = self.0.pop_front().ok_or(EventReadError::Closed);
        Box::pin(async move { read })
    }
}
fn progress_cursor(incarnation: &str, position: u64, index: Option<u64>) -> CommittedEventCursor {
    CommittedEventCursor::new(
        SessionId::new(id().to_string()).unwrap(),
        Id::new(incarnation).unwrap(),
        position,
        index,
    )
}
fn progress_page(incarnation: &str, position: u64, index: u64) -> EventRead {
    let next = progress_cursor(incarnation, position, Some(index));
    let event = CommittedEvent::change(
        next.clone(),
        SessionChange::LocalSettlement {
            execution_id: ExecutionId::new("trailing").unwrap(),
            // A settlement update, not a turn's end: these pages are about
            // cursors, not the reply an end looks up.
            before: Some(Ok(ExecutionOutcome::Completed)),
            after: Ok(ExecutionOutcome::Completed),
            local_outcome: None,
        },
    );
    EventRead::Page(
        CommittedEventPage::new(
            vec![event],
            next,
            progress_cursor(incarnation, position, None),
        )
        .unwrap(),
    )
}
async fn scripted_follow(
    start: EventStart,
    reads: Vec<EventRead>,
    wait: u64,
) -> Result<Value, String> {
    let metadata = Arc::new(MemoryRepository::default());
    let storage = Arc::new(ScriptedStorage {
        reads: Mutex::new(Some(reads.into())),
        opened: Mutex::new(None),
    });
    let service = observing_service(metadata, storage.clone());
    let (_stop, stopped) = watch::channel(false);
    let result = follow(
        &service,
        id(),
        caller(),
        Follow {
            thread_id: id().to_string(),
            execution_id: None,
            after: match &start {
                EventStart::After(cursor) => Some(cursor.encode()),
                _ => None,
            },
            replay: if matches!(start, EventStart::Beginning) {
                Some(true)
            } else {
                None
            },
            wait_ms: Some(wait),
            limit: None,
        },
        SubscriptionControl::new(stopped, wait, None),
    )
    .await;
    assert_eq!(storage.opened.lock().unwrap().as_ref(), Some(&start));
    service.shutdown().await.unwrap();
    result.map(|(value, _)| value)
}

#[tokio::test(start_paused = true)]
async fn follow_correlates_after_with_page_and_idle_before_returning_any_cursor() {
    let after = progress_cursor("incarnation", 2, Some(1));
    for read in [
        progress_page("replacement", 3, 1),
        progress_page("incarnation", 1, 1),
        progress_page("incarnation", 2, 1),
        EventRead::Idle(progress_cursor("replacement", 3, None)),
        EventRead::Idle(progress_cursor("incarnation", 1, None)),
    ] {
        let result = scripted_follow(EventStart::After(after.clone()), vec![read], 0).await;
        assert_eq!(result.unwrap_err(), "event_cursor_invalid");
    }
    let outer = progress_cursor("incarnation", 2, None);
    assert_eq!(
        scripted_follow(
            EventStart::After(outer),
            vec![progress_page("incarnation", 2, 1)],
            0
        )
        .await
        .unwrap_err(),
        "event_cursor_invalid"
    );
    for read in [
        progress_page("incarnation", 2, 2),
        progress_page("incarnation", 3, 1),
        EventRead::Idle(after.clone()),
    ] {
        let result = scripted_follow(EventStart::After(after.clone()), vec![read], 0)
            .await
            .unwrap();
        assert!(result["cursor"].is_string());
    }
}

#[tokio::test(start_paused = true)]
async fn follow_retains_now_idle_progress_through_preparing_and_valid_trailing_events() {
    for start in [EventStart::Now, EventStart::Beginning] {
        for read in [
            progress_page("replacement", 3, 1),
            progress_page("incarnation", 1, 1),
            progress_page("incarnation", 2, 1),
            EventRead::Idle(progress_cursor("replacement", 3, None)),
            EventRead::Idle(progress_cursor("incarnation", 1, None)),
        ] {
            let result = scripted_follow(
                start.clone(),
                vec![
                    EventRead::Idle(progress_cursor("incarnation", 2, None)),
                    EventRead::Preparing,
                    read,
                ],
                1000,
            )
            .await;
            assert_eq!(result.unwrap_err(), "event_cursor_invalid");
        }
        let value = scripted_follow(
            start,
            vec![
                EventRead::Idle(progress_cursor("incarnation", 2, None)),
                EventRead::Preparing,
                EventRead::Idle(progress_cursor("incarnation", 2, None)),
                progress_page("incarnation", 3, 1),
            ],
            1000,
        )
        .await
        .unwrap();
        assert_eq!(value["events"][0]["executionId"], "trailing");
        assert_eq!(
            value["cursor"],
            progress_cursor("incarnation", 3, Some(1)).encode()
        );
    }
    let first = scripted_follow(EventStart::Now, vec![progress_page("captured", 9, 1)], 0)
        .await
        .unwrap();
    assert_eq!(
        first["cursor"],
        progress_cursor("captured", 9, Some(1)).encode()
    );
    assert_eq!(first["events"][0]["executionId"], "trailing");
}

/// A turn's end whose history cannot be read now is refused, so the caller
/// asks again from its cursor, never told the turn ended without a reply.
#[tokio::test(start_paused = true)]
async fn an_ended_turn_whose_history_cannot_be_read_is_refused_not_answered_without_a_reply() {
    let next = progress_cursor("incarnation", 1, Some(1));
    let ended = CommittedEvent::change(
        next.clone(),
        SessionChange::LocalSettlement {
            execution_id: ExecutionId::new("ended").unwrap(),
            before: None,
            after: Ok(ExecutionOutcome::Completed),
            local_outcome: None,
        },
    );
    let page = EventRead::Page(
        CommittedEventPage::new(vec![ended], next, progress_cursor("incarnation", 1, None))
            .unwrap(),
    );
    assert_eq!(
        scripted_follow(EventStart::Beginning, vec![page], 0)
            .await
            .unwrap_err(),
        "temporarily_unavailable"
    );
}
