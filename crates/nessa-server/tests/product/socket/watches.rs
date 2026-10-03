use super::*;
use crate::conversation::application::{
    CatalogueChangeWatch, CatalogueWatchError, CatalogueWatchState, ReceiverReadScope,
    WatchCatalogue, WatchRecords,
};
use crate::conversation::infrastructure::NessaRecordWatches;
use crate::product::{
    change_watch::ProductWatchPermit, generated::MAX_GLOBAL_CHANGE_WATCHES, WatchTaskFault,
};
use crate::product_contract::generated::ChangeWatchErrorCode;
use futures_util::poll;
use nessa_sdk::application::agent_execution::sessions::{ChangeWatchError, CommittedChangeWatch};
use nessa_sdk::domain::agent_execution::sessions::ExecutionSessionId;
use nessa_sync::replication::domain::Id;
use std::{
    io::Error as WriteError,
    sync::Condvar,
    task::{Context, Poll, Wake, Waker},
};
use tempfile::TempDir;
use tokio::{
    sync::{oneshot::Receiver as ReleaseReceiver, Notify},
    task::JoinHandle,
};

struct CountingRecords {
    actual: NessaRecordWatches,
    installed: AtomicU64,
}
impl WatchRecords for CountingRecords {
    fn watch(
        &self,
        conversation: &ConversationId,
    ) -> Result<CommittedChangeWatch, ChangeWatchError> {
        self.installed.fetch_add(1, Ordering::SeqCst);
        self.actual.watch(conversation)
    }
}

struct CountingReceiver {
    actual: RecordBinding,
    admitted: AtomicU64,
}
impl ReceiverAuthority for CountingReceiver {
    fn resolve<'a>(
        &'a self,
        credential: &'a CredentialId,
    ) -> Pin<Box<dyn Future<Output = Result<Option<ReceiverBinding>, ReadRefusal>> + Send + 'a>>
    {
        self.admitted.fetch_add(1, Ordering::SeqCst);
        self.actual.resolve(credential)
    }
}

struct WatchFixture {
    state: ProductRouteState,
    session: AuthenticatedSession,
    storage: Arc<RecordStorage>,
    id: ConversationId,
    records: Arc<CountingRecords>,
    receiver: Arc<CountingReceiver>,
    authority: Arc<Authority>,
    reads: Arc<RecordAdmissionSpy>,
    _directory: TempDir,
}

impl WatchFixture {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let private = directory.path().join("conversations");
        nessa_local_storage::create_directory(&private).unwrap();
        let metadata =
            Arc::new(LocalConversationStore::open(&private.join("metadata.sqlite3")).unwrap());
        let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
        metadata
            .create(
                Conversation::new(
                    id.clone(),
                    OrganizationId::new("organization").unwrap(),
                    PrincipalId::new("principal").unwrap(),
                    "panel".into(),
                    "create".into(),
                    1,
                    AgentId::Claude,
                    ConversationModelId::new("model").unwrap(),
                    ConversationApprovalMode::Ask,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let storage = Arc::new(RecordStorage::new(directory.path().join("records")).unwrap());
        storage.initialize().await.unwrap();
        let records = Arc::new(CountingRecords {
            actual: NessaRecordWatches::new(storage.clone()),
            installed: AtomicU64::new(0),
        });
        let receiver = Arc::new(CountingReceiver {
            actual: RecordBinding,
            admitted: AtomicU64::new(0),
        });
        let reads = Arc::new(RecordAdmissionSpy(AtomicU64::new(0)));
        let (state, authority) = fixture(MembershipRole::Member);
        {
            let mut snapshot = authority.snapshot.lock().unwrap();
            let organization = OrganizationId::new("organization").unwrap();
            snapshot.credential = Credential::new(
                CredentialId::new("credential").unwrap(),
                PrincipalId::new("principal").unwrap(),
                organization.clone(),
                AudienceId::new("gateway").unwrap(),
                100,
                200,
                ["conversation.read", "conversation.write", "server.read"]
                    .into_iter()
                    .map(|action| {
                        Grant::new(
                            Action::new(action).unwrap(),
                            Resource::new(
                                organization.clone(),
                                ResourceId::new("gateway-resource").unwrap(),
                            ),
                        )
                    })
                    .collect(),
            )
            .unwrap();
        }
        let state = state
            .with_passive_read(receiver.clone(), metadata.clone())
            .with_change_watches(records.clone(), metadata)
            .with_record_source(reads.clone());
        let session = authenticate(&state).await;
        Self {
            state,
            session,
            storage,
            id,
            records,
            receiver,
            authority,
            reads,
            _directory: directory,
        }
    }

    fn watch(&self, peer: &TestPeer, request: &str) {
        self.send(peer, request, "conversation.watchRecords", json!({"conversationId": self.id.to_string(), "receiverId": "receiver", "accessEpoch": "3"}));
    }

    fn send(&self, peer: &TestPeer, id: &str, method: &str, params: Value) {
        let frame = RequestFrame::new(id, method, &params).unwrap();
        peer.input
            .send(Ok(Message::Text(
                serde_json::to_string(&frame).unwrap().into(),
            )))
            .unwrap();
    }

    /// One complete durable save: the opening save first, then a provider
    /// context change on every later call, so each call is a real commit.
    async fn commit(&self) {
        let id = SessionId::new(self.id.to_string()).unwrap();
        let writer = self.storage.open(id.clone()).await.unwrap();
        let provider = ProviderIdentity::new("fixture", "model", "workspace").unwrap();
        let loaded = writer.load().await.unwrap();
        let (before, changes) = match loaded.snapshot() {
            None => (
                ProviderContext::Absent,
                vec![SessionChange::Opened {
                    id: id.clone(),
                    provider: provider.clone(),
                    context: ProviderContext::Absent,
                }],
            ),
            Some(snapshot) => {
                let after = ProviderContext::Recorded(
                    ExecutionSessionId::new(Uuid::new_v4().to_string()).unwrap(),
                );
                (
                    snapshot.provider_context.clone(),
                    vec![SessionChange::ProviderContext {
                        before: snapshot.provider_context.clone(),
                        after,
                    }],
                )
            }
        };
        let context = match changes.last() {
            Some(SessionChange::ProviderContext { after, .. }) => after.clone(),
            _ => before,
        };
        writer
            .save_changes(
                loaded.binding().clone(),
                SessionSnapshot {
                    id,
                    provider,
                    provider_context: context,
                    invocations: Vec::new(),
                    queue_history: Vec::new(),
                },
                vec![SessionSaveUnit::new(changes).unwrap()],
            )
            .await
            .unwrap();
    }

    async fn catalogue_commit(&self, creation: u64) -> ConversationId {
        let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
        self.state
            .passive_read
            .as_ref()
            .unwrap()
            .1
            .create(
                Conversation::new(
                    id.clone(),
                    OrganizationId::new("organization").unwrap(),
                    PrincipalId::new("principal").unwrap(),
                    format!("panel-{creation}"),
                    format!("create-{id}"),
                    creation,
                    AgentId::Claude,
                    ConversationModelId::new("model").unwrap(),
                    ConversationApprovalMode::Ask,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        id
    }

    async fn finish(&self, peer: TestPeer, socket: JoinHandle<()>) {
        drop(peer);
        socket.await.unwrap();
        assert_eq!(self.reads.0.load(Ordering::SeqCst), 0);
        assert_eq!(self.state.record_reads.available_permits(), 4);
        assert_eq!(
            self.state.change_watches.available_permits(),
            MAX_GLOBAL_CHANGE_WATCHES
        );
        self.state.drain_watches().await.unwrap();
        self.storage.shutdown().await.unwrap();
    }
}

fn text(message: Message) -> Value {
    let Message::Text(text) = message else {
        panic!("product text expected")
    };
    serde_json::from_str(&text).unwrap()
}

#[tokio::test]
async fn real_commit_waits_for_physical_ack_then_emits_only_opaque_watch_identity() {
    let fixture = WatchFixture::new().await;
    let (release, gate) = tokio::sync::oneshot::channel();
    let (socket, mut peer) = test_socket(Some(gate));
    let socket = tokio::spawn(run_authenticated(
        socket,
        fixture.state.clone(),
        fixture.session.clone(),
    ));
    fixture.watch(&peer, "install");
    peer.writing.recv().await.unwrap(); // Real success acknowledgement has started physical flush.
    assert_eq!(fixture.records.installed.load(Ordering::SeqCst), 1);
    fixture.commit().await;
    assert!(peer.output.try_recv().is_err());
    release.send(()).unwrap();
    let acknowledgement = text(peer.message().await);
    assert_eq!(acknowledgement["id"], "install");
    assert_eq!(acknowledgement["ok"], true);
    let watch = acknowledgement["payload"]["watchId"].as_str().unwrap();
    let notice = text(peer.message().await);
    assert_eq!(notice["event"], "conversation.changed");
    assert_eq!(notice["payload"], json!({"watchId": watch}));
    assert_eq!(notice["stateVersion"], 0);
    fixture.finish(peer, socket).await;
}

#[tokio::test]
async fn repeat_unwatch_is_cleanup_without_source_or_receiver_read_and_no_redundant_event() {
    let fixture = WatchFixture::new().await;
    let (socket, mut peer) = test_socket(None);
    let socket = tokio::spawn(run_authenticated(
        socket,
        fixture.state.clone(),
        fixture.session.clone(),
    ));
    fixture.watch(&peer, "install");
    let installed = text(peer.message().await);
    assert_eq!(installed["ok"], true);
    let watch = installed["payload"]["watchId"].as_str().unwrap().to_owned();
    let admissions = fixture.receiver.admitted.load(Ordering::SeqCst);
    for request in ["remove", "repeat-remove"] {
        fixture.send(
            &peer,
            request,
            "conversation.unwatch",
            json!({"watchId": watch}),
        );
        let response = text(peer.message().await);
        assert_eq!(response["id"], request);
        assert_eq!(response["payload"], json!({"watchId": watch}));
    }
    assert_eq!(fixture.receiver.admitted.load(Ordering::SeqCst), admissions);
    assert_eq!(fixture.records.installed.load(Ordering::SeqCst), 1);
    fixture.commit().await;
    fixture.send(&peer, "health-after-removal", "server.health", json!({}));
    let response = text(peer.message().await);
    assert_eq!(response["id"], "health-after-removal");
    assert_eq!(response["ok"], true);
    assert!(peer.output.try_recv().is_err());
    fixture.finish(peer, socket).await;
}

struct HeldReceiverWork {
    released: Mutex<bool>,
    wake: Condvar,
    entered: Notify,
    completed: AtomicU64,
}
impl HeldReceiverWork {
    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.wake.notify_one();
    }
}
struct ReleaseHeld(Arc<HeldReceiverWork>);
impl Drop for ReleaseHeld {
    fn drop(&mut self) {
        self.0.release();
    }
}
struct HoldFirstReceiver {
    actual: RecordBinding,
    first: AtomicBool,
    work: Arc<HeldReceiverWork>,
}
impl ReceiverAuthority for HoldFirstReceiver {
    fn resolve<'a>(
        &'a self,
        credential: &'a CredentialId,
    ) -> Pin<Box<dyn Future<Output = Result<Option<ReceiverBinding>, ReadRefusal>> + Send + 'a>>
    {
        if !self.first.swap(false, Ordering::SeqCst) {
            return self.actual.resolve(credential);
        }
        let work = self.work.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                work.entered.notify_one();
                let mut released = work.released.lock().unwrap();
                while !*released {
                    released = work.wake.wait(released).unwrap();
                }
                work.completed.fetch_add(1, Ordering::SeqCst);
            })
            .await
            .map_err(|_| ReadRefusal::Unverifiable)?;
            self.actual.resolve(credential).await
        })
    }
}

#[tokio::test]
async fn watch_admission_worker_loss_retains_original_owner_while_other_read_and_control_reap() {
    let mut fixture = WatchFixture::new().await;
    fixture.commit().await;
    let work = Arc::new(HeldReceiverWork {
        released: Mutex::new(false),
        wake: Condvar::new(),
        entered: Notify::new(),
        completed: AtomicU64::new(0),
    });
    let _release = ReleaseHeld(work.clone());
    let metadata = fixture.state.passive_read.as_ref().unwrap().1.clone();
    fixture.state = fixture.state.clone().with_passive_read(
        Arc::new(HoldFirstReceiver {
            actual: RecordBinding,
            first: AtomicBool::new(true),
            work: work.clone(),
        }),
        metadata,
    );
    let reader = Arc::new(NessaRecordReadSource::new(
        fixture.storage.clone(),
        Id::new("gateway-resource").unwrap(),
        Handle::current(),
    ));
    fixture.state = fixture.state.clone().with_record_source(reader.clone());
    let (socket, mut peer) = test_socket(None);
    let socket = tokio::spawn(run_authenticated(
        socket,
        fixture.state.clone(),
        fixture.session.clone(),
    ));
    fixture.watch(&peer, "held-watch");
    work.entered.notified().await;
    assert_eq!(
        fixture.state.change_watches.available_permits(),
        MAX_GLOBAL_CHANGE_WATCHES - 1
    );
    fixture.watch(&peer, "duplicate-while-watch-held");
    let duplicate = text(peer.message().await);
    assert_eq!(duplicate["id"], "duplicate-while-watch-held");
    assert_eq!(duplicate["error"]["code"], "watch_duplicate");
    fixture.send(
        &peer,
        "same-kind-while-watch-held",
        "conversation.watchRecords",
        json!({"conversationId": fixture.id.to_string(), "receiverId": "other-receiver", "accessEpoch": "3"}),
    );
    let same_kind = text(peer.message().await);
    assert_eq!(same_kind["id"], "same-kind-while-watch-held");
    assert_eq!(same_kind["error"]["code"], "watch_capacity");
    assert_eq!(fixture.records.installed.load(Ordering::SeqCst), 0);
    assert_eq!(work.completed.load(Ordering::SeqCst), 0);
    assert_eq!(
        fixture.state.change_watches.available_permits(),
        MAX_GLOBAL_CHANGE_WATCHES - 1
    );
    fixture.send(&peer, "head-while-watch-held", "conversation.recordsHead", json!({"conversationId": fixture.id.to_string(), "receiverId": "receiver", "accessEpoch": "3"}));
    let head = text(peer.message().await);
    assert_eq!(head["id"], "head-while-watch-held");
    assert_eq!(head["ok"], true);
    fixture.send(
        &peer,
        "control-while-watch-held",
        "conversation.close",
        json!({"conversationId": fixture.id.to_string(), "requestId": "stop"}),
    );
    let control = text(peer.message().await);
    assert_eq!(control["id"], "control-while-watch-held");
    // Current policy admitted and the actual control dispatcher answered. This
    // fixture has no Agent/service, so it proves dispatch progress, not a stopped Agent.
    assert_eq!(control["error"]["code"], "conversations_not_configured");
    assert_eq!(work.completed.load(Ordering::SeqCst), 0);
    drop(peer);
    socket.await.unwrap();
    let mut drain = Box::pin(fixture.state.drain_watches());
    assert!(matches!(poll!(&mut drain), Poll::Pending));
    assert_eq!(fixture.records.installed.load(Ordering::SeqCst), 0);
    assert_eq!(
        fixture.state.change_watches.available_permits(),
        MAX_GLOBAL_CHANGE_WATCHES - 1
    );
    work.release();
    assert_eq!(drain.await, Ok(()));
    assert_eq!(work.completed.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.records.installed.load(Ordering::SeqCst), 0); // Lost interest never installs late.
    assert_eq!(
        fixture.state.change_watches.available_permits(),
        MAX_GLOBAL_CHANGE_WATCHES
    );
    reader.shutdown().await.unwrap();
    fixture.storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn actual_shared_gateway_capacity_counts_both_source_kinds_until_connection_drop() {
    let fixture = WatchFixture::new().await;
    let mut connections = Vec::new();
    for index in 0..MAX_GLOBAL_CHANGE_WATCHES / 2 {
        let (socket, mut peer) = test_socket(None);
        let socket = tokio::spawn(run_authenticated(
            socket,
            fixture.state.clone(),
            fixture.session.clone(),
        ));
        let records = format!("records-{index}");
        fixture.watch(&peer, &records);
        let acknowledged = text(peer.message().await);
        assert_eq!(acknowledged["id"], records);
        assert_eq!(acknowledged["ok"], true);
        let catalogue = format!("catalogue-{index}");
        fixture.send(
            &peer,
            &catalogue,
            "conversation.watchCatalogue",
            json!({"receiverId": "receiver", "accessEpoch": "3"}),
        );
        let acknowledged = text(peer.message().await);
        assert_eq!(acknowledged["id"], catalogue);
        assert_eq!(acknowledged["ok"], true);
        connections.push((peer, socket));
    }
    assert_eq!(fixture.state.change_watches.available_permits(), 0);
    assert_eq!(fixture.state.record_reads.available_permits(), 4);
    assert_eq!(
        fixture.records.installed.load(Ordering::SeqCst),
        (MAX_GLOBAL_CHANGE_WATCHES / 2) as u64
    );
    let (socket, mut peer) = test_socket(None);
    let socket = tokio::spawn(run_authenticated(
        socket,
        fixture.state.clone(),
        fixture.session.clone(),
    ));
    fixture.watch(&peer, "capacity-refusal");
    let refused = text(peer.message().await);
    assert_eq!(refused["error"]["code"], "watch_capacity");
    assert_eq!(
        fixture.records.installed.load(Ordering::SeqCst),
        (MAX_GLOBAL_CHANGE_WATCHES / 2) as u64
    );
    let (removed, completion) = connections.pop().unwrap();
    drop(removed);
    completion.await.unwrap();
    assert_eq!(fixture.state.change_watches.available_permits(), 2);
    fixture.watch(&peer, "after-original-drop");
    let accepted = text(peer.message().await);
    assert_eq!(accepted["id"], "after-original-drop");
    assert_eq!(accepted["ok"], true);
    assert_eq!(fixture.state.change_watches.available_permits(), 1);
    assert_eq!(
        fixture.records.installed.load(Ordering::SeqCst),
        (MAX_GLOBAL_CHANGE_WATCHES / 2 + 1) as u64
    );
    for (peer, socket) in connections {
        drop(peer);
        socket.await.unwrap();
    }
    fixture.finish(peer, socket).await;
}

#[tokio::test]
async fn duplicate_and_foreign_removal_refuse_without_displacing_actual_interests() {
    let fixture = WatchFixture::new().await;
    let (socket, mut first) = test_socket(None);
    let first_socket = tokio::spawn(run_authenticated(
        socket,
        fixture.state.clone(),
        fixture.session.clone(),
    ));
    let (socket, mut second) = test_socket(None);
    let second_socket = tokio::spawn(run_authenticated(
        socket,
        fixture.state.clone(),
        fixture.session.clone(),
    ));
    fixture.watch(&first, "first");
    let first_id = text(first.message().await)["payload"]["watchId"]
        .as_str()
        .unwrap()
        .to_owned();
    fixture.watch(&second, "second");
    let second_id = text(second.message().await)["payload"]["watchId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(first_id, second_id);
    fixture.watch(&first, "duplicate");
    let duplicate = text(first.message().await);
    assert_eq!(duplicate["error"]["code"], "watch_duplicate");
    fixture.send(
        &first,
        "foreign-removal",
        "conversation.unwatch",
        json!({"watchId": second_id}),
    );
    let refused = text(first.message().await);
    assert_eq!(refused["error"]["code"], "invalid_watch");
    assert_eq!(fixture.records.installed.load(Ordering::SeqCst), 2);
    fixture.commit().await;
    let first_notice = text(first.message().await);
    let second_notice = text(second.message().await);
    assert_eq!(first_notice["payload"], json!({"watchId": first_id}));
    assert_eq!(second_notice["payload"], json!({"watchId": second_id}));
    drop(second);
    second_socket.await.unwrap();
    fixture.finish(first, first_socket).await;
}

#[tokio::test]
async fn actual_source_admission_close_emits_terminal_without_claiming_current_or_authority_loss() {
    let fixture = WatchFixture::new().await;
    let (socket, mut peer) = test_socket(None);
    let socket = tokio::spawn(run_authenticated(
        socket,
        fixture.state.clone(),
        fixture.session.clone(),
    ));
    fixture.watch(&peer, "source-close");
    let watch_id = text(peer.message().await)["payload"]["watchId"]
        .as_str()
        .unwrap()
        .to_owned();
    fixture.storage.shutdown().await.unwrap();
    let ended = text(peer.message().await);
    assert_eq!(ended["type"], "event");
    assert_eq!(ended["event"], "conversation.watchEnded");
    assert_eq!(
        ended["payload"],
        json!({"watchId": watch_id, "reason": "closed"})
    );
    // Existing terminal identity remains valid cleanup, without a second ended event.
    fixture.send(
        &peer,
        "cleanup-closed",
        "conversation.unwatch",
        json!({"watchId": watch_id}),
    );
    let cleaned = text(peer.message().await);
    assert_eq!(cleaned["id"], "cleanup-closed");
    assert_eq!(cleaned["ok"], true);
    let (fresh_socket, mut fresh) = test_socket(None);
    let fresh_socket = tokio::spawn(run_authenticated(
        fresh_socket,
        fixture.state.clone(),
        fixture.session.clone(),
    ));
    fixture.watch(&fresh, "closed-source-refusal");
    let refused = text(fresh.message().await);
    assert_eq!(refused["id"], "closed-source-refusal");
    assert_eq!(refused["error"]["code"], "watch_closed");
    assert!(peer.output.try_recv().is_err());
    drop(peer);
    socket.await.unwrap();
    fixture.finish(fresh, fresh_socket).await;
}

struct PanicAfterReceiver(HoldFirstReceiver);
impl ReceiverAuthority for PanicAfterReceiver {
    fn resolve<'a>(
        &'a self,
        credential: &'a CredentialId,
    ) -> Pin<Box<dyn Future<Output = Result<Option<ReceiverBinding>, ReadRefusal>> + Send + 'a>>
    {
        Box::pin(async move {
            self.0.resolve(credential).await?;
            panic!("injected authority task unwind after actual worker completion");
        })
    }
}

#[tokio::test]
async fn lost_socket_observer_cannot_erase_fault_after_original_authority_worker_join() {
    let mut fixture = WatchFixture::new().await;
    let work = Arc::new(HeldReceiverWork {
        released: Mutex::new(false),
        wake: Condvar::new(),
        entered: Notify::new(),
        completed: AtomicU64::new(0),
    });
    let _release = ReleaseHeld(work.clone());
    let metadata = fixture.state.passive_read.as_ref().unwrap().1.clone();
    fixture.state = fixture.state.clone().with_passive_read(
        Arc::new(PanicAfterReceiver(HoldFirstReceiver {
            actual: RecordBinding,
            first: AtomicBool::new(true),
            work: work.clone(),
        })),
        metadata,
    );
    let (socket, peer) = test_socket(None);
    let socket = tokio::spawn(run_authenticated(
        socket,
        fixture.state.clone(),
        fixture.session.clone(),
    ));
    fixture.watch(&peer, "lost-observer-panic");
    work.entered.notified().await;
    drop(peer);
    socket.await.unwrap();
    let mut drain = Box::pin(fixture.state.drain_watches());
    assert!(matches!(poll!(&mut drain), Poll::Pending));
    assert_eq!(
        fixture.state.change_watches.available_permits(),
        MAX_GLOBAL_CHANGE_WATCHES - 1
    );
    assert_eq!(work.completed.load(Ordering::SeqCst), 0);
    work.release();
    assert_eq!(drain.await, Err(WatchTaskFault::Panic));
    assert_eq!(work.completed.load(Ordering::SeqCst), 1);
    assert_eq!(
        fixture.state.change_watches.available_permits(),
        MAX_GLOBAL_CHANGE_WATCHES
    );
    assert_eq!(fixture.records.installed.load(Ordering::SeqCst), 0);
    fixture.storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn unwatch_ack_retains_original_authority_target_until_actual_join() {
    let mut fixture = WatchFixture::new().await;
    let work = Arc::new(HeldReceiverWork {
        released: Mutex::new(false),
        wake: Condvar::new(),
        entered: Notify::new(),
        completed: AtomicU64::new(0),
    });
    let _release = ReleaseHeld(work.clone());
    let authority = Arc::new(HoldFirstReceiver {
        actual: RecordBinding,
        first: AtomicBool::new(false),
        work: work.clone(),
    });
    let metadata = fixture.state.passive_read.as_ref().unwrap().1.clone();
    fixture.state = fixture
        .state
        .clone()
        .with_passive_read(authority.clone(), metadata);
    let (socket, mut peer) = test_socket(None);
    let socket = tokio::spawn(run_authenticated(
        socket,
        fixture.state.clone(),
        fixture.session.clone(),
    ));
    fixture.watch(&peer, "original-held-notice");
    let acknowledged = text(peer.message().await);
    assert_eq!(acknowledged["ok"], true);
    let original = acknowledged["payload"]["watchId"]
        .as_str()
        .unwrap()
        .to_owned();
    authority.first.store(true, Ordering::SeqCst);
    fixture.commit().await;
    work.entered.notified().await;
    fixture.send(
        &peer,
        "remove-held-notice",
        "conversation.unwatch",
        json!({"watchId": original}),
    );
    let removal = text(peer.message().await);
    assert_eq!(removal["id"], "remove-held-notice");
    assert_eq!(removal["payload"], json!({"watchId": original}));
    fixture.watch(&peer, "replacement-before-original-join");
    let refused = text(peer.message().await);
    assert_eq!(refused["id"], "replacement-before-original-join");
    assert_eq!(refused["error"]["code"], "watch_duplicate");
    assert_eq!(fixture.records.installed.load(Ordering::SeqCst), 1);
    assert_eq!(work.completed.load(Ordering::SeqCst), 0);
    assert_eq!(
        fixture.state.change_watches.available_permits(),
        MAX_GLOBAL_CHANGE_WATCHES - 1
    );
    work.release();
    timeout(Duration::from_secs(1), async {
        while fixture.state.change_watches.available_permits() != MAX_GLOBAL_CHANGE_WATCHES {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(work.completed.load(Ordering::SeqCst), 1);
    assert!(peer.output.try_recv().is_err());
    fixture.watch(&peer, "replacement-after-original-join");
    let replacement = text(peer.message().await);
    assert_eq!(replacement["id"], "replacement-after-original-join");
    assert_eq!(replacement["ok"], true);
    let replacement_id = replacement["payload"]["watchId"].as_str().unwrap();
    assert_ne!(replacement_id, original);
    assert_eq!(fixture.records.installed.load(Ordering::SeqCst), 2);
    fixture.commit().await;
    let notice = text(peer.message().await);
    assert_eq!(notice["event"], "conversation.changed");
    assert_eq!(notice["payload"], json!({"watchId": replacement_id}));
    assert!(peer.output.try_recv().is_err());
    fixture.finish(peer, socket).await;
}

#[tokio::test]
async fn notice_authority_deadline_abandons_socket_but_retains_actual_worker_until_join() {
    let mut fixture = WatchFixture::new().await;
    // Registration uses the same injected port, but only the next (notice) call is held.
    let work = Arc::new(HeldReceiverWork {
        released: Mutex::new(false),
        wake: Condvar::new(),
        entered: Notify::new(),
        completed: AtomicU64::new(0),
    });
    let _release = ReleaseHeld(work.clone());
    let authority = Arc::new(HoldFirstReceiver {
        actual: RecordBinding,
        first: AtomicBool::new(false),
        work: work.clone(),
    });
    let metadata = fixture.state.passive_read.as_ref().unwrap().1.clone();
    fixture.state = fixture
        .state
        .clone()
        .with_passive_read(authority.clone(), metadata);
    let (socket, mut peer) = test_socket(None);
    let socket = tokio::spawn(run_authenticated(
        socket,
        fixture.state.clone(),
        fixture.session.clone(),
    ));
    fixture.watch(&peer, "notice-deadline");
    assert_eq!(text(peer.message().await)["ok"], true);
    authority.first.store(true, Ordering::SeqCst);
    fixture.commit().await;
    work.entered.notified().await;
    // All actual commit/worker handshakes precede controlled-clock deadline inspection.
    tokio::time::pause();
    tokio::time::advance(RECORD_SEND_TIMEOUT).await;
    // Tokio timer wheel rounds to milliseconds; advance across its due tick
    // explicitly because held blocking work prevents idle auto-advance.
    tokio::time::advance(Duration::from_millis(1)).await;
    timeout(Duration::from_secs(1), socket)
        .await
        .unwrap()
        .unwrap();
    assert!(peer.output.try_recv().is_err()); // No changed/late error frame was sent.
    let mut drain = Box::pin(fixture.state.drain_watches());
    assert!(matches!(poll!(&mut drain), Poll::Pending));
    assert_eq!(
        fixture.state.change_watches.available_permits(),
        MAX_GLOBAL_CHANGE_WATCHES - 1
    );
    assert_eq!(work.completed.load(Ordering::SeqCst), 0);
    // The controlled socket deadline is established; physical worker and store
    // completion now use real time, rather than auto-advancing OS I/O deadlines.
    tokio::time::resume();
    work.release();
    assert_eq!(drain.await, Ok(()));
    assert_eq!(work.completed.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.records.installed.load(Ordering::SeqCst), 1);
    drop(peer);
    // event-stream converts a std::Instant shutdown bound to Tokio time.
    // Resuming this clock retains the simulated offset, so perform real store
    // cleanup on a fresh unpaused runtime after the socket/retention assertions.
    let storage = fixture.storage.clone();
    tokio::task::spawn_blocking(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(storage.shutdown())
    })
    .await
    .unwrap()
    .unwrap();
}

struct PanicNotification;
impl Wake for PanicNotification {
    fn wake(self: Arc<Self>) {
        panic!("injected synchronous catalogue notification callback unwind");
    }
}
struct RetainedCatalogueWatch(Mutex<Option<CatalogueChangeWatch>>);
impl WatchCatalogue for RetainedCatalogueWatch {
    fn watch(
        &self,
        _: &OrganizationId,
        _: &PrincipalId,
    ) -> Result<CatalogueChangeWatch, CatalogueWatchError> {
        self.0
            .lock()
            .unwrap()
            .take()
            .ok_or(CatalogueWatchError::Capacity)
    }
}

#[tokio::test]
async fn actual_durable_catalogue_notification_failure_remains_terminal_in_wire_payload() {
    let mut fixture = WatchFixture::new().await;
    let source = fixture.state.catalogue_watches.as_ref().unwrap().clone();
    let mut original = source
        .watch(
            &OrganizationId::new("organization").unwrap(),
            &PrincipalId::new("principal").unwrap(),
        )
        .unwrap();
    let waker = Waker::from(Arc::new(PanicNotification));
    let mut context = Context::from_waker(&waker);
    let mut waiting = Box::pin(original.changed());
    assert!(matches!(waiting.as_mut().poll(&mut context), Poll::Pending));
    let metadata = fixture.state.passive_read.as_ref().unwrap().1.clone();
    let changed_id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    metadata
        .create(
            Conversation::new(
                changed_id.clone(),
                OrganizationId::new("organization").unwrap(),
                PrincipalId::new("principal").unwrap(),
                "new-panel".into(),
                "create-after-registration".into(),
                2,
                AgentId::Claude,
                ConversationModelId::new("model").unwrap(),
                ConversationApprovalMode::Ask,
            )
            .unwrap(),
        )
        .await
        .unwrap(); // Real durable result survives the notification callback unwind.
    drop(waiting);
    assert_eq!(
        original.changed().await,
        CatalogueWatchState::NotificationFailed
    );
    assert!(metadata.load(&changed_id).await.unwrap().is_some());
    // Hand the SAME non-cloneable failed producer registration to the actual
    // consumer port, so the transport observes a real terminal signal without
    // injecting terminal state or modifying the frozen producer.
    fixture.state = fixture.state.clone().with_change_watches(
        fixture.records.clone(),
        Arc::new(RetainedCatalogueWatch(Mutex::new(Some(original)))),
    );
    let (socket, mut peer) = test_socket(None);
    let socket = tokio::spawn(run_authenticated(
        socket,
        fixture.state.clone(),
        fixture.session.clone(),
    ));
    fixture.send(
        &peer,
        "failed-catalogue",
        "conversation.watchCatalogue",
        json!({"receiverId": "receiver", "accessEpoch": "3"}),
    );
    let acknowledgement = text(peer.message().await);
    assert_eq!(acknowledgement["ok"], true);
    let id = acknowledgement["payload"]["watchId"].as_str().unwrap();
    let terminal = text(peer.message().await);
    assert_eq!(terminal["event"], "conversation.watchEnded");
    assert_eq!(
        terminal["payload"],
        json!({"watchId": id, "reason": "notification_failed"})
    );
    assert!(peer.output.try_recv().is_err());
    fixture.finish(peer, socket).await;
}

/// Uses the existing physical Sink fixture, gating only its first changed event.
struct ChangedFlushSocket {
    actual: TestSocket,
    changed_gate: Option<ReleaseReceiver<()>>,
}
impl Stream for ChangedFlushSocket {
    type Item = Result<Message, Error>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.actual).poll_next(cx)
    }
}
impl Sink<Message> for ChangedFlushSocket {
    type Error = WriteError;
    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.actual).poll_ready(cx)
    }
    fn start_send(mut self: Pin<&mut Self>, message: Message) -> Result<(), Self::Error> {
        let changed = match &message {
            Message::Text(text) => serde_json::from_str::<Value>(text)
                .is_ok_and(|value| value["event"] == "conversation.changed"),
            _ => false,
        };
        if changed && self.changed_gate.is_some() {
            self.actual.gate = self.changed_gate.take();
        }
        Pin::new(&mut self.actual).start_send(message)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.actual).poll_flush(cx)
    }
    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.actual).poll_close(cx)
    }
}

#[tokio::test]
async fn already_admitted_notice_can_finish_after_read_grant_revocation_but_next_notice_denies() {
    let fixture = WatchFixture::new().await;
    let (actual, mut peer) = test_socket(None);
    let (release, changed_gate) = tokio::sync::oneshot::channel();
    let socket = tokio::spawn(run_authenticated(
        ChangedFlushSocket {
            actual,
            changed_gate: Some(changed_gate),
        },
        fixture.state.clone(),
        fixture.session.clone(),
    ));
    fixture.send(
        &peer,
        "catalogue",
        "conversation.watchCatalogue",
        json!({"receiverId": "receiver", "accessEpoch": "3"}),
    );
    let acknowledged = text(peer.message().await);
    assert_eq!(acknowledged["ok"], true);
    let id = acknowledged["payload"]["watchId"].as_str().unwrap();
    peer.writing.recv().await.unwrap(); // Consume the physical ACK marker.
    fixture.catalogue_commit(2).await;
    peer.writing.recv().await.unwrap(); // The admitted first notice now owns physical flush.
    {
        let mut snapshot = fixture.authority.snapshot.lock().unwrap();
        let organization = OrganizationId::new("organization").unwrap();
        snapshot.credential = Credential::new(
            CredentialId::new("credential").unwrap(),
            PrincipalId::new("principal").unwrap(),
            organization.clone(),
            AudienceId::new("gateway").unwrap(),
            100,
            200,
            ["conversation.write", "server.read"]
                .into_iter()
                .map(|action| {
                    Grant::new(
                        Action::new(action).unwrap(),
                        Resource::new(
                            organization.clone(),
                            ResourceId::new("gateway-resource").unwrap(),
                        ),
                    )
                })
                .collect(),
        )
        .unwrap();
        snapshot.revision += 1;
    }
    fixture.catalogue_commit(3).await; // New source work demands a new conversation.read admission.
    release.send(()).unwrap();
    let admitted = text(peer.message().await);
    assert_eq!(admitted["event"], "conversation.changed");
    assert_eq!(admitted["payload"], json!({"watchId": id}));
    let Message::Close(Some(close)) = peer.message().await else {
        panic!("next notice after grant revocation must close rather than deliver");
    };
    assert_eq!(
        close.code,
        SessionCloseReason::AuthorizationLost.web_socket_code()
    );
    assert!(peer.output.try_recv().is_err());
    fixture.finish(peer, socket).await;
}

struct ObserveHealthAdmission {
    actual: Arc<dyn PolicyEvaluator>,
    passed: Notify,
}
impl PolicyEvaluator for ObserveHealthAdmission {
    fn evaluate(
        &self,
        context: &AuthContext,
        action: &Action,
        resource: &Resource,
        snapshot: &AccessSnapshot,
    ) -> Result<Decision, AccessError> {
        let decision = self.actual.evaluate(context, action, resource, snapshot)?;
        if action.as_str() == "server.read" && decision == Decision::Allow {
            self.passed.notify_one();
        }
        Ok(decision)
    }
}

#[tokio::test]
async fn unwatch_ack_is_interest_only_and_replacement_waits_for_actual_inflight_retirement() {
    let mut fixture = WatchFixture::new().await;
    let admitted = Arc::new(ObserveHealthAdmission {
        actual: fixture.state.policy.clone(),
        passed: Notify::new(),
    });
    fixture.state.policy = admitted.clone();
    let (actual, mut peer) = test_socket(None);
    let (release, changed_gate) = tokio::sync::oneshot::channel();
    let socket = tokio::spawn(run_authenticated(
        ChangedFlushSocket {
            actual,
            changed_gate: Some(changed_gate),
        },
        fixture.state.clone(),
        fixture.session.clone(),
    ));
    fixture.watch(&peer, "original");
    let accepted = text(peer.message().await);
    let original = accepted["payload"]["watchId"].as_str().unwrap().to_owned();
    peer.writing.recv().await.unwrap(); // Physical ACK marker.
    fixture.commit().await;
    peer.writing.recv().await.unwrap(); // Immutable first notice is physically in-flight.
    fixture.send(
        &peer,
        "remove-inflight",
        "conversation.unwatch",
        json!({"watchId": original}),
    );
    fixture.watch(&peer, "replacement-too-early");
    // A health control gives a dispatcher handshake while the same writer is held;
    // it remains queued until the original physical send can complete.
    fixture.send(&peer, "ordering-barrier", "server.health", json!({}));
    timeout(Duration::from_secs(1), admitted.passed.notified())
        .await
        .unwrap();
    assert_eq!(
        fixture.state.change_watches.available_permits(),
        MAX_GLOBAL_CHANGE_WATCHES - 1
    );
    release.send(()).unwrap();
    let notice = text(peer.message().await);
    assert_eq!(notice["payload"], json!({"watchId": original}));
    let mut removal = false;
    let mut refused = false;
    let mut barrier = false;
    for _ in 0..3 {
        let response = text(peer.message().await);
        match response["id"].as_str().unwrap() {
            "remove-inflight" => {
                assert_eq!(response["ok"], true);
                removal = true;
            }
            "replacement-too-early" => {
                assert_eq!(response["error"]["code"], "watch_duplicate");
                refused = true;
            }
            "ordering-barrier" => {
                assert_eq!(response["ok"], true);
                barrier = true;
            }
            other => panic!("unexpected response {other}"),
        }
    }
    assert!(removal && refused && barrier);
    timeout(Duration::from_secs(1), async {
        while fixture.state.change_watches.available_permits() != MAX_GLOBAL_CHANGE_WATCHES {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap(); // Observe actual final original owner, not merely its ACK.
    fixture.watch(&peer, "replacement-after-original");
    let replacement = text(peer.message().await);
    assert_eq!(replacement["ok"], true);
    let new = replacement["payload"]["watchId"].as_str().unwrap();
    assert_ne!(new, original);
    let admitted = fixture.receiver.admitted.load(Ordering::SeqCst);
    fixture.send(
        &peer,
        "repeat-old-cleanup",
        "conversation.unwatch",
        json!({"watchId": original}),
    );
    assert_eq!(text(peer.message().await)["ok"], true);
    assert_eq!(fixture.receiver.admitted.load(Ordering::SeqCst), admitted);
    assert_eq!(
        fixture.state.change_watches.available_permits(),
        MAX_GLOBAL_CHANGE_WATCHES - 1
    );
    assert!(peer.output.try_recv().is_err());
    fixture.finish(peer, socket).await;
}

#[tokio::test]
async fn actual_source_terminal_during_immutable_notice_send_uses_only_one_subsequent_event() {
    let fixture = WatchFixture::new().await;
    let (actual, mut peer) = test_socket(None);
    let (release, changed_gate) = tokio::sync::oneshot::channel();
    let socket = tokio::spawn(run_authenticated(
        ChangedFlushSocket {
            actual,
            changed_gate: Some(changed_gate),
        },
        fixture.state.clone(),
        fixture.session.clone(),
    ));
    fixture.watch(&peer, "terminal-after-inflight");
    let acknowledged = text(peer.message().await);
    let id = acknowledged["payload"]["watchId"].as_str().unwrap();
    peer.writing.recv().await.unwrap();
    fixture.commit().await;
    peer.writing.recv().await.unwrap();
    fixture.storage.shutdown().await.unwrap();
    assert!(peer.output.try_recv().is_err());
    assert_eq!(
        fixture.state.change_watches.available_permits(),
        MAX_GLOBAL_CHANGE_WATCHES - 1
    );
    release.send(()).unwrap();
    let immutable = text(peer.message().await);
    assert_eq!(immutable["event"], "conversation.changed");
    assert_eq!(immutable["payload"], json!({"watchId": id}));
    let terminal = text(peer.message().await);
    assert_eq!(terminal["event"], "conversation.watchEnded");
    assert_eq!(
        terminal["payload"],
        json!({"watchId": id, "reason": "closed"})
    );
    assert!(peer.output.try_recv().is_err());
    fixture.finish(peer, socket).await;
}

#[tokio::test]
async fn actual_actor_replaces_unsent_dirty_after_observing_source_terminal_before_authority_release(
) {
    let mut fixture = WatchFixture::new().await;
    let work = Arc::new(HeldReceiverWork {
        released: Mutex::new(false),
        wake: Condvar::new(),
        entered: Notify::new(),
        completed: AtomicU64::new(0),
    });
    let _release = ReleaseHeld(work.clone());
    let authority = Arc::new(HoldFirstReceiver {
        actual: RecordBinding,
        first: AtomicBool::new(false),
        work: work.clone(),
    });
    let metadata = fixture.state.passive_read.as_ref().unwrap().1.clone();
    fixture.state = fixture
        .state
        .clone()
        .with_passive_read(authority.clone(), metadata);
    let mut watches = ConnectionWatches::new(&fixture.state);
    let slot = Arc::new(Arc::new(Semaphore::new(1)).try_acquire_owned().unwrap());
    let request = RequestFrame::new("actor-registration", "conversation.watchRecords", &json!({"conversationId": fixture.id.to_string(), "receiverId": "receiver", "accessEpoch": "3"})).unwrap();
    assert!(watches
        .begin(
            &fixture.state,
            &fixture.session,
            request,
            slot,
            Instant::now() + RECORD_SEND_TIMEOUT
        )
        .is_none());
    let WatchOutcome::Reply(reply) = watches.next(&fixture.state, &fixture.session).await else {
        panic!("registration reply required");
    };
    let WatchAcknowledgement {
        completed, owner, ..
    } = reply.acknowledgement.unwrap();
    completed.send(()).unwrap();
    drop(owner);
    drop(reply.slot);
    // Actor-only completion signal; the earlier actual-Sink fixture separately
    // proves that production sends it only after physical ACK flush.
    assert!(matches!(
        watches.next(&fixture.state, &fixture.session).await,
        WatchOutcome::Progress
    ));
    authority.first.store(true, Ordering::SeqCst);
    fixture.commit().await;
    assert!(matches!(
        watches.next(&fixture.state, &fixture.session).await,
        WatchOutcome::Progress
    ));
    work.entered.notified().await;
    let original_deadline = watches.deliveries.deadline();
    assert!(original_deadline.is_some());
    fixture.storage.shutdown().await.unwrap();
    // Drive the actual source-terminal progress while authority is STILL held.
    // Releasing first would merely test an unordered race, not unsent replacement.
    assert!(matches!(
        watches.next(&fixture.state, &fixture.session).await,
        WatchOutcome::Progress
    ));
    assert_eq!(watches.deliveries.deadline(), original_deadline);
    assert!(watches.deliveries.take().is_none());
    assert_eq!(work.completed.load(Ordering::SeqCst), 0);
    work.release();
    assert!(matches!(
        watches.next(&fixture.state, &fixture.session).await,
        WatchOutcome::Progress
    ));
    let frame = watches.deliveries.take().unwrap();
    assert!(frame.terminal);
    let terminal = serde_json::to_value(&frame.message).unwrap();
    assert_eq!(terminal["event"], "conversation.watchEnded");
    assert_eq!(terminal["payload"]["reason"], "closed");
    assert_eq!(frame.deadline, original_deadline.unwrap());
    assert!(watches.deliveries.take().is_none());
    watches.deliveries.sent(&frame.id, true);
    watches.collect_retired();
    drop(watches);
    assert_eq!(
        fixture.state.change_watches.available_permits(),
        MAX_GLOBAL_CHANGE_WATCHES - 1
    );
    drop(frame);
    assert_eq!(
        fixture.state.change_watches.available_permits(),
        MAX_GLOBAL_CHANGE_WATCHES
    );
    assert_eq!(fixture.state.drain_watches().await, Ok(()));
}

/// Row H1: gateway shutdown closes watch admission before host cleanup. Each
/// live connection then closes as temporarily unavailable, releasing its
/// watch permit, and a later registration is refused as closed.
#[tokio::test]
async fn closing_watch_admission_closes_live_connections_and_refuses_new_watches() {
    let fixture = WatchFixture::new().await;
    let (socket, mut peer) = test_socket(None);
    let socket = tokio::spawn(run_authenticated(
        socket,
        fixture.state.clone(),
        fixture.session.clone(),
    ));
    fixture.watch(&peer, "install");
    let acknowledgement = text(peer.message().await);
    assert_eq!(acknowledgement["ok"], true, "{acknowledgement}");
    fixture.state.close_watch_admission();
    let Message::Close(Some(close)) = peer.message().await else {
        panic!("closed watch admission must close the live connection");
    };
    assert_eq!(
        close.code,
        SessionCloseReason::TemporaryUnavailable.web_socket_code()
    );
    socket.await.unwrap();
    assert_eq!(
        fixture.state.change_watches.available_permits(),
        MAX_GLOBAL_CHANGE_WATCHES
    );
    let refused = fixture.state.change_watches.try_acquire().err();
    assert_eq!(refused, Some(ChangeWatchErrorCode::WatchClosed));
    assert_eq!(fixture.state.drain_watches().await, Ok(()));
    fixture.storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn closed_watch_shapes_and_existing_scope_admission_refuse_before_source_installation() {
    let fixture = WatchFixture::new().await;
    let (socket, mut peer) = test_socket(None);
    let socket = tokio::spawn(run_authenticated(
        socket,
        fixture.state.clone(),
        fixture.session.clone(),
    ));
    for (id, params, cause) in [
        (
            "foreign-receiver",
            json!({"conversationId": fixture.id.to_string(), "receiverId": "foreign", "accessEpoch": "3"}),
            "wrong_receiver",
        ),
        (
            "stale-epoch",
            json!({"conversationId": fixture.id.to_string(), "receiverId": "receiver", "accessEpoch": "4"}),
            "stale_epoch",
        ),
        (
            "unknown-shape-key",
            json!({"conversationId": fixture.id.to_string(), "receiverId": "receiver", "accessEpoch": "3", "cursor": "forged"}),
            "invalid_request",
        ),
        (
            "empty-receiver",
            json!({"conversationId": fixture.id.to_string(), "receiverId": "", "accessEpoch": "3"}),
            "invalid_request",
        ),
        (
            "overflow-epoch",
            json!({"conversationId": fixture.id.to_string(), "receiverId": "receiver", "accessEpoch": "18446744073709551616"}),
            "invalid_request",
        ),
    ] {
        fixture.send(&peer, id, "conversation.watchRecords", params);
        let response = text(peer.message().await);
        assert_eq!(response["id"], id);
        assert_eq!(response["error"]["code"], cause);
        assert_eq!(fixture.records.installed.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.state.record_reads.available_permits(), 4);
        // Failure ACK also means reply delivery, not retirement of its original
        // owner. Observe that retirement before exercising the next admission.
        timeout(Duration::from_secs(1), async {
            while fixture.state.change_watches.available_permits() != MAX_GLOBAL_CHANGE_WATCHES {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    fixture.watch(&peer, "valid-after-refusals");
    let accepted = text(peer.message().await);
    assert_eq!(accepted["ok"], true);
    assert_eq!(fixture.records.installed.load(Ordering::SeqCst), 1);
    fixture.finish(peer, socket).await;
}

/// Owning product fixture consumed by ordinary host cleanup interleavings.
/// Receiver work is actual spawn_blocking work joined by the original watch task.
pub(crate) struct HostWatchFixture {
    inner: WatchFixture,
    authority: Option<Arc<HeldReceiverWork>>,
    cancellation_owner: Option<Arc<ProductWatchPermit>>,
    peer: Option<TestPeer>,
    socket: Option<JoinHandle<()>>,
}
impl HostWatchFixture {
    pub(crate) async fn held_authority(panic_after_release: bool) -> Self {
        let mut inner = WatchFixture::new().await;
        inner.commit().await;
        let work = Arc::new(HeldReceiverWork {
            released: Mutex::new(false),
            wake: Condvar::new(),
            entered: Notify::new(),
            completed: AtomicU64::new(0),
        });
        let receiver = HoldFirstReceiver {
            actual: RecordBinding,
            first: AtomicBool::new(true),
            work: work.clone(),
        };
        let receiver: Arc<dyn ReceiverAuthority> = if panic_after_release {
            Arc::new(PanicAfterReceiver(receiver))
        } else {
            Arc::new(receiver)
        };
        let metadata = inner.state.passive_read.as_ref().unwrap().1.clone();
        inner.state = inner.state.clone().with_passive_read(receiver, metadata);
        let (socket, peer) = test_socket(None);
        let socket = tokio::spawn(run_authenticated(
            socket,
            inner.state.clone(),
            inner.session.clone(),
        ));
        inner.watch(&peer, "host-original-authority");
        timeout(Duration::from_secs(5), work.entered.notified())
            .await
            .unwrap();
        Self {
            inner,
            authority: Some(work),
            cancellation_owner: None,
            peer: Some(peer),
            socket: Some(socket),
        }
    }
    pub(crate) async fn cancelled_task() -> Self {
        let inner = WatchFixture::new().await;
        inner.commit().await;
        let owner = Arc::new(inner.state.change_watches.try_acquire().unwrap());
        let guard = owner.task();
        let entered = Arc::new(Notify::new());
        let entry = entered.clone();
        let task = tokio::spawn(async move {
            let _original = guard;
            entry.notify_one();
            std::future::pending::<()>().await;
        });
        entered.notified().await;
        task.abort(); // Inject only the unexpected original future cancellation.
        assert!(task.await.unwrap_err().is_cancelled());
        Self {
            inner,
            authority: None,
            cancellation_owner: Some(owner),
            peer: None,
            socket: None,
        }
    }
    pub(crate) fn state(&self) -> ProductRouteState {
        self.inner.state.clone()
    }
    pub(crate) fn storage(&self) -> Arc<RecordStorage> {
        self.inner.storage.clone()
    }
    pub(crate) fn scope(&self) -> ReceiverReadScope {
        ReceiverReadScope {
            receiver_id: "receiver".into(),
            organization_id: OrganizationId::new("organization").unwrap(),
            owner_id: PrincipalId::new("principal").unwrap(),
            conversation_id: self.inner.id.clone(),
            access_epoch: 3,
        }
    }
    pub(crate) fn admission_closed(state: &ProductRouteState) -> bool {
        state.change_watches.is_closed()
    }
    pub(crate) fn resource_held(&self) -> bool {
        self.inner.state.change_watches.available_permits() == MAX_GLOBAL_CHANGE_WATCHES - 1
    }
    pub(crate) async fn lose_socket_observer(&mut self) {
        drop(self.peer.take());
        if let Some(socket) = self.socket.take() {
            timeout(Duration::from_secs(2), socket)
                .await
                .unwrap()
                .unwrap();
        }
    }
    pub(crate) async fn connection_stopped(&mut self) {
        if let Some(socket) = self.socket.take() {
            timeout(Duration::from_secs(2), socket)
                .await
                .unwrap()
                .unwrap();
        }
    }
    pub(crate) fn release(&mut self) {
        if let Some(work) = &self.authority {
            work.release();
        }
        drop(self.cancellation_owner.take());
    }
    pub(crate) fn completed_authority(&self) -> u64 {
        self.authority
            .as_ref()
            .map_or(0, |work| work.completed.load(Ordering::SeqCst))
    }
}
impl Drop for HostWatchFixture {
    fn drop(&mut self) {
        self.release();
    }
}
