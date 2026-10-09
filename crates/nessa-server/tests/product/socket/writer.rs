use super::*;
use crate::conversation::application::{CatalogueReadError, CatalogueReadFuture, CatalogueReadOperation, CatalogueReadSource, RecordHead, RecordReadValue};
use nessa_protocol::conversation::read_scope::CatalogueReadScope;
use crate::product::change_watch::{watch_principal, Notice, WatchOwners};
use nessa_sync::replication::domain::{Id, Scope};
use serde_json::Value;
use std::{
    io::Error,
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Condvar, Mutex,
    },
    task::{Context, Poll},
    thread::JoinHandle,
};
use tokio::{
    sync::{oneshot::Receiver, Notify},
    task::JoinHandle as AsyncJoinHandle,
};

// Each successful physical flush replenishes one bounded higher-priority lane.
// No producer can run out and accidentally let the old biased writer pass.
enum ReadyLane {
    Control(mpsc::Sender<ControlOutput>, Arc<Semaphore>),
    Refusal(mpsc::Sender<OutgoingMessage>),
    Ordinary(mpsc::Sender<QueuedResponse>, Arc<Semaphore>),
}
impl ReadyLane {
    fn refill(&self) {
        let message = success("priority", &json!({}));
        match self {
            Self::Control(sender, slots) => sender
                .try_send(ControlOutput::Response(Box::new(QueuedResponse {
                    message: WireResponse::ordinary(message),
                    _slot: slots.clone().try_acquire_owned().unwrap().into(),
                    _record_work: None,
                    _mount: None,
                })))
                .unwrap(),
            Self::Refusal(sender) => sender.try_send(message).unwrap(),
            Self::Ordinary(sender, slots) => sender
                .try_send(QueuedResponse {
                    message: WireResponse::ordinary(message),
                    _slot: slots.clone().try_acquire_owned().unwrap().into(),
                    _record_work: None,
                    _mount: None,
                })
                .unwrap(),
        }
    }
}
struct ReadySocket {
    lane: ReadyLane,
    writes: Arc<AtomicUsize>,
    records_written: Arc<AtomicUsize>,
    ready: Arc<Notify>,
}
impl Stream for ReadySocket {
    type Item = Result<Message, axum::Error>;
    fn poll_next(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Pending
    }
}
impl Sink<Message> for ReadySocket {
    type Error = Error;
    fn poll_ready(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
    fn start_send(self: Pin<&mut Self>, message: Message) -> Result<(), Self::Error> {
        if matches!(message, Message::Text(ref text) if text.as_str() == "{\"record\":true}") {
            self.records_written.fetch_add(1, Ordering::SeqCst);
        }
        if self.writes.fetch_add(1, Ordering::SeqCst) == 63 {
            self.ready.notify_one();
        }
        Ok(())
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.lane.refill();
        Poll::Ready(Ok(()))
    }
    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
}

#[derive(Clone, Copy)]
enum PriorityLane {
    Control,
    Refusal,
    Ordinary,
}
async fn continuously_ready_lane_releases_all_record_leases(
    lane: PriorityLane,
    refusal: Option<&str>,
) {
    let global_reads = Arc::new(Semaphore::new(4));
    let records_written = Arc::new(AtomicUsize::new(0));
    let mut writers = Vec::new();
    let mut record_slots = Vec::new();
    let mut senders = Vec::new();
    for _ in 0..4 {
        let (control_send, controls) = mpsc::channel(4);
        let (refusal_send, refusals) = mpsc::channel(1);
        let (ordinary_send, ordinary) = mpsc::channel(16);
        let (record_send, records) = mpsc::channel(1);
        let slots = Arc::new(Semaphore::new(1));
        record_send
            .send(QueuedRecordResponse::new(QueuedResponse {
                message: refusal.map_or_else(
                    || WireResponse::record("{\"record\":true}".into()),
                    |code| WireResponse::ordinary(failure("record", code)),
                ),
                _slot: slots.clone().try_acquire_owned().unwrap().into(),
                _record_work: refusal.is_none().then(|| {
                    RecordReadLease::new(Box::new(
                        global_reads.clone().try_acquire_owned().unwrap(),
                    ))
                }),
                _mount: None,
            }))
            .await
            .unwrap();
        let priority_slots = Arc::new(Semaphore::new(2));
        let ready_lane = match lane {
            PriorityLane::Control => ReadyLane::Control(control_send.clone(), priority_slots),
            PriorityLane::Refusal => ReadyLane::Refusal(refusal_send.clone()),
            PriorityLane::Ordinary => ReadyLane::Ordinary(ordinary_send.clone(), priority_slots),
        };
        ready_lane.refill();
        let ready = Arc::new(Notify::new());
        let writes = Arc::new(AtomicUsize::new(0));
        let socket = ReadySocket {
            lane: ready_lane,
            writes: writes.clone(),
            records_written: records_written.clone(),
            ready: ready.clone(),
        };
        let (sink, _incoming) = socket.split();
        writers.push(tokio::spawn(write_authenticated(
            sink,
            controls,
            refusals,
            ordinary,
            records,
            Duration::from_secs(60),
            Arc::new(WatchDeliveries::new()),
            Arc::new(SubscriptionDeliveries::new(Default::default())),
        )));
        ready.notified().await;
        assert!(writes.load(Ordering::SeqCst) >= 64);
        assert_eq!(slots.available_permits(), 0);
        record_slots.push(slots);
        senders.push((control_send, refusal_send, ordinary_send, record_send));
    }
    assert_eq!(
        global_reads.available_permits(),
        if refusal.is_some() { 4 } else { 0 }
    );
    tokio::time::advance(RECORD_SEND_TIMEOUT + Duration::from_millis(1)).await;
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
    let terminated = writers.iter().all(AsyncJoinHandle::is_finished);
    let released = global_reads.available_permits();
    let slots_released = record_slots
        .iter()
        .all(|slots| slots.available_permits() == 1);
    // Keep a failed probe finite, without mistaking abort cleanup for the result.
    for writer in writers {
        if !writer.is_finished() {
            writer.abort();
        }
        let _ = writer.await;
    }
    drop(senders);
    assert!(
        terminated,
        "queued absolute record deadline must end a continuously busy writer"
    );
    assert_eq!(
        released, 4,
        "deadline must release all four queued global read leases"
    );
    assert!(
        slots_released,
        "deadline must release each actual record slot"
    );
    assert_eq!(records_written.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn queued_record_deadline_survives_continuously_ready_controls() {
    continuously_ready_lane_releases_all_record_leases(PriorityLane::Control, None).await;
}
#[tokio::test(start_paused = true)]
async fn queued_record_deadline_survives_continuously_ready_refusals() {
    continuously_ready_lane_releases_all_record_leases(PriorityLane::Refusal, None).await;
}
#[tokio::test(start_paused = true)]
async fn queued_record_deadline_survives_continuously_ready_ordinary() {
    continuously_ready_lane_releases_all_record_leases(PriorityLane::Ordinary, None).await;
}

async fn arriving_record_interrupts_stalled_priority(close: bool, refusal: Option<&str>) {
    let (_release, gate) = tokio::sync::oneshot::channel();
    let (socket, mut peer) = test_socket(Some(gate));
    let (sink, _incoming) = socket.split();
    let (control_send, controls) = mpsc::channel(4);
    let (_refusal_send, refusals) = mpsc::channel(1);
    let (_ordinary_send, ordinary) = mpsc::channel(16);
    let (record_send, records) = mpsc::channel(1);
    let record_slots = Arc::new(Semaphore::new(1));
    let global_reads = Arc::new(Semaphore::new(4));
    if close {
        control_send
            .send(ControlOutput::Close(SessionCloseReason::CredentialRevoked))
            .await
            .unwrap();
    } else {
        control_send
            .send(ControlOutput::Response(Box::new(QueuedResponse {
                message: WireResponse::ordinary(success("control", &json!({}))),
                _slot: Arc::new(Semaphore::new(1))
                    .try_acquire_owned()
                    .unwrap()
                    .into(),
                _record_work: None,
                _mount: None,
            })))
            .await
            .unwrap();
    }
    let writer = tokio::spawn(write_authenticated(
        sink,
        controls,
        refusals,
        ordinary,
        records,
        Duration::from_secs(60),
        Arc::new(WatchDeliveries::new()),
        Arc::new(SubscriptionDeliveries::new(Default::default())),
    ));
    peer.writing.recv().await.unwrap();
    record_send
        .send(QueuedRecordResponse::new(QueuedResponse {
            message: refusal.map_or_else(
                || WireResponse::record("{\"record\":true}".into()),
                |code| WireResponse::ordinary(failure("record", code)),
            ),
            _slot: record_slots.clone().try_acquire_owned().unwrap().into(),
            _record_work: refusal.is_none().then(|| {
                RecordReadLease::new(Box::new(global_reads.clone().try_acquire_owned().unwrap()))
            }),
            _mount: None,
        }))
        .await
        .unwrap();
    tokio::task::yield_now().await;
    assert_eq!(record_slots.available_permits(), 0);
    assert_eq!(
        global_reads.available_permits(),
        if refusal.is_some() { 4 } else { 3 }
    );
    tokio::time::advance(RECORD_SEND_TIMEOUT + Duration::from_millis(1)).await;
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
    let terminated = writer.is_finished();
    let released = global_reads.available_permits();
    if !terminated {
        writer.abort();
    }
    let _ = writer.await;
    assert!(
        terminated,
        "arriving record deadline must govern the physical priority write or close"
    );
    assert_eq!(released, 4);
    assert_eq!(record_slots.available_permits(), 1);
    assert!(
        peer.output.try_recv().is_err(),
        "no second frame or completed stalled frame"
    );
}
#[tokio::test(start_paused = true)]
async fn queued_record_arrival_deadline_interrupts_stalled_control() {
    arriving_record_interrupts_stalled_priority(false, None).await;
}
#[tokio::test(start_paused = true)]
async fn queued_record_arrival_deadline_interrupts_stalled_close() {
    arriving_record_interrupts_stalled_priority(true, None).await;
}

#[tokio::test]
async fn nonexpired_pending_record_preserves_physical_priority_and_releases_lease() {
    let (socket, mut peer) = test_socket(None);
    let (sink, _incoming) = socket.split();
    let (control_send, controls) = mpsc::channel(4);
    let (refusal_send, refusals) = mpsc::channel(1);
    let (ordinary_send, ordinary) = mpsc::channel(16);
    let (record_send, records) = mpsc::channel(1);
    let slots = Arc::new(Semaphore::new(3));
    let global_reads = Arc::new(Semaphore::new(4));
    let response = |id: &str| QueuedResponse {
        message: WireResponse::ordinary(success(id, &json!({}))),
        _slot: slots.clone().try_acquire_owned().unwrap().into(),
        _record_work: None,
        _mount: None,
    };
    control_send
        .send(ControlOutput::Response(Box::new(response("control"))))
        .await
        .unwrap();
    refusal_send
        .send(success("refusal", &json!({})))
        .await
        .unwrap();
    ordinary_send.send(response("ordinary")).await.unwrap();
    record_send
        .send(QueuedRecordResponse::new(QueuedResponse {
            message: WireResponse::record(serde_json::to_string(&json!({"id":"record"})).unwrap()),
            _slot: slots.clone().try_acquire_owned().unwrap().into(),
            _record_work: Some(RecordReadLease::new(Box::new(
                global_reads.clone().try_acquire_owned().unwrap(),
            ))),
            _mount: None,
        }))
        .await
        .unwrap();
    let deliveries = Arc::new(WatchDeliveries::new());
    // Close the same delivery interest as the production connection owner.
    deliveries.close();
    drop((control_send, refusal_send, ordinary_send, record_send));
    let writer = tokio::spawn(write_authenticated(
        sink,
        controls,
        refusals,
        ordinary,
        records,
        Duration::from_secs(5),
        deliveries.clone(),
        Arc::new(SubscriptionDeliveries::new(Default::default())),
    ));
    for expected in ["control", "refusal", "ordinary", "record"] {
        let Message::Text(text) = peer.message().await else {
            panic!("text response expected")
        };
        assert_eq!(
            serde_json::from_str::<Value>(&text).unwrap()["id"],
            expected
        );
    }
    writer.await.unwrap();
    assert_eq!(slots.available_permits(), 3);
    assert_eq!(global_reads.available_permits(), 4);
}

#[tokio::test(start_paused = true)]
async fn queued_record_refusals_expire_under_continuously_ready_priority() {
    for code in [
        "source_preparing",
        "read_timeout",
        "temporarily_unavailable",
    ] {
        for lane in [
            PriorityLane::Control,
            PriorityLane::Refusal,
            PriorityLane::Ordinary,
        ] {
            continuously_ready_lane_releases_all_record_leases(lane, Some(code)).await;
        }
    }
}

#[tokio::test(start_paused = true)]
async fn arriving_record_refusals_interrupt_stalled_priority() {
    for code in [
        "source_preparing",
        "read_timeout",
        "temporarily_unavailable",
    ] {
        for close in [false, true] {
            arriving_record_interrupts_stalled_priority(close, Some(code)).await;
        }
    }
}

// Completion must remain observable while an unrelated authority read is held.
struct HeldSocketAuthority {
    authority: Arc<Authority>,
    hold: AtomicBool,
    entered: Notify,
    release: Notify,
    waiting: AtomicUsize,
}
struct HeldAuthorityRead<'a>(&'a AtomicUsize);
impl Drop for HeldAuthorityRead<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
impl AccessReader for HeldSocketAuthority {
    fn read<'a>(&'a self, id: &'a CredentialId) -> PortFuture<'a, AccessSnapshot> {
        Box::pin(async move {
            if self.hold.load(Ordering::SeqCst) {
                self.waiting.fetch_add(1, Ordering::SeqCst);
                let _waiting = HeldAuthorityRead(&self.waiting);
                self.entered.notify_one();
                self.release.notified().await;
            }
            self.authority.read(id).await
        })
    }
}
struct HeldSuccessfulRead {
    entered: Notify,
    release: Notify,
    completed: AtomicUsize,
}
impl RecordReadSource for HeldSuccessfulRead {
    fn read<'a>(
        &'a self,
        admitted: ReceiverReadScope,
        operation: RecordReadOperation,
        lease: RecordReadLease,
    ) -> RecordReadFuture<'a, RecordReadResponse> {
        Box::pin(async move {
            assert!(matches!(operation, RecordReadOperation::Head));
            self.entered.notify_one();
            self.release.notified().await;
            let id = |value| Id::new(value).unwrap();
            let value = RecordReadValue::Head(RecordHead {
                scope: Scope::new(
                    id(admitted.receiver_id),
                    id("gateway".into()),
                    id(admitted.conversation_id.to_string()),
                    id("incarnation".into()),
                    id("physical-schema".into()),
                    id(format!("epoch-{}", admitted.access_epoch)),
                ),
                head: 1,
            });
            self.completed.fetch_add(1, Ordering::SeqCst);
            Ok(RecordReadResponse { value, lease })
        })
    }
}
struct HealthEffects(AtomicUsize);
impl UptimeClock for HealthEffects {
    fn elapsed_ms(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst);
        42
    }
}
#[derive(Clone, Copy, Debug)]
enum HeldAuthorityInput {
    Periodic,
    Malformed,
    Ordinary,
}
async fn successful_read_during_held_authority(input: HeldAuthorityInput, stalled: bool) {
    let (state, authority) = fixture(MembershipRole::Member);
    authority.snapshot.lock().unwrap().credential = Credential::new(
        CredentialId::new("credential").unwrap(),
        PrincipalId::new("principal").unwrap(),
        OrganizationId::new("organization").unwrap(),
        AudienceId::new("gateway").unwrap(),
        100,
        200,
        ["conversation.read", "server.read"]
            .into_iter()
            .map(|action| {
                Grant::new(
                    Action::new(action).unwrap(),
                    Resource::new(
                        OrganizationId::new("organization").unwrap(),
                        ResourceId::new("gateway-resource").unwrap(),
                    ),
                )
            })
            .collect(),
    )
    .unwrap();
    let session = authenticate(&state).await;
    let held = Arc::new(HeldSocketAuthority {
        authority,
        hold: AtomicBool::new(false),
        entered: Notify::new(),
        release: Notify::new(),
        waiting: AtomicUsize::new(0),
    });
    let source = Arc::new(HeldSuccessfulRead {
        entered: Notify::new(),
        release: Notify::new(),
        completed: AtomicUsize::new(0),
    });
    let repository = Arc::new(conversation_support::MemoryRepository::default());
    let id = ConversationId::new(&uuid::Uuid::new_v4().to_string()).unwrap();
    repository.records.lock().unwrap().insert(
        id.clone(),
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
    );
    let effects = Arc::new(HealthEffects(AtomicUsize::new(0)));
    let mut state = state
        .with_passive_read(Arc::new(RecordBinding), repository)
        .with_record_source(source.clone());
    state.access = held.clone();
    state.uptime_clock = effects.clone();
    state.settings = SessionSettings::new(
        Duration::from_secs(60),
        Duration::from_secs(60),
        if matches!(input, HeldAuthorityInput::Periodic) {
            Duration::from_secs(1)
        } else {
            Duration::from_secs(120)
        },
    )
    .unwrap();
    let capacity = state.record_reads.clone();
    let count = if stalled { 4 } else { 1 };
    let mut tasks = Vec::new();
    let mut peers = Vec::new();
    let mut send_releases = Vec::new();
    for _ in 0..count {
        let (send_release, send_gate) = tokio::sync::oneshot::channel();
        let (socket, peer) = test_socket(stalled.then_some(send_gate));
        tasks.push(tokio::spawn(run_authenticated(
            socket,
            state.clone(),
            session.clone(),
        )));
        peer.input.send(Ok(Message::Text(json!({
            "type":"req", "id":"record", "method":"conversation.recordsHead",
            "params":{"conversationId":id.to_string(),"receiverId":"receiver","accessEpoch":"3"}
        }).to_string().into()))).unwrap();
        timeout(Duration::from_secs(1), source.entered.notified())
            .await
            .unwrap();
        peers.push(peer);
        send_releases.push(send_release);
    }
    assert_eq!(capacity.available_permits(), 4 - count);
    held.hold.store(true, Ordering::SeqCst);
    match input {
        HeldAuthorityInput::Periodic => tokio::time::advance(Duration::from_secs(1)).await,
        HeldAuthorityInput::Malformed => {
            for peer in &peers {
                peer.input
                    .send(Ok(Message::Text(
                        r#"{"type":"req","id":"deferred","method":false,"params":{}}"#.into(),
                    )))
                    .unwrap();
            }
        }
        HeldAuthorityInput::Ordinary => {
            for peer in &peers {
                peer.request("deferred");
            }
        }
    }
    timeout(Duration::from_secs(1), async {
        while held.waiting.load(Ordering::SeqCst) != count {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(effects.0.load(Ordering::SeqCst), 0);
    source.release.notify_waiters();
    timeout(Duration::from_secs(1), async {
        while source.completed.load(Ordering::SeqCst) != count {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    if stalled {
        let mut started = true;
        for peer in &mut peers {
            started &= timeout(Duration::from_secs(1), peer.writing.recv())
                .await
                .is_ok();
        }
        tokio::time::advance(RECORD_SEND_TIMEOUT + Duration::from_millis(1)).await;
        let _ = timeout(Duration::from_secs(1), async {
            while !tasks.iter().all(AsyncJoinHandle::is_finished) {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await;
        let ended = tasks.iter().all(AsyncJoinHandle::is_finished);
        let released = capacity.available_permits();
        let cancelled_checks = held.waiting.load(Ordering::SeqCst);
        let effects_before = effects.0.load(Ordering::SeqCst);
        let gate_unreleased = held.hold.load(Ordering::SeqCst);
        // Cleanup follows measurement and cannot make a failed deadline appear valid.
        for task in tasks {
            if !task.is_finished() {
                task.abort();
            }
            let _ = task.await;
        }
        drop(send_releases);
        assert!(
            started,
            "{input:?}: all ready sources must reach their sinks during held authority"
        );
        assert!(
            ended,
            "{input:?}: writer termination must remain observable during held authority"
        );
        assert_eq!(
            released, 4,
            "{input:?}: original delivery deadlines must return all shared read capacity"
        );
        assert!(gate_unreleased, "authority gate was not released");
        assert_eq!(
            cancelled_checks, 0,
            "socket teardown drops pending authority owners"
        );
        assert_eq!(effects_before, 0);
        for peer in &mut peers {
            assert!(peer.output.try_recv().is_err());
        }
    } else {
        let mut peer = peers.pop().unwrap();
        let task = tasks.pop().unwrap();
        let received = timeout(Duration::from_secs(1), peer.output.recv()).await;
        let released = capacity.available_permits();
        let still_held = held.waiting.load(Ordering::SeqCst);
        let effects_before = effects.0.load(Ordering::SeqCst);
        held.hold.store(false, Ordering::SeqCst);
        held.release.notify_waiters();
        let deferred = if received.is_ok() && !matches!(input, HeldAuthorityInput::Periodic) {
            Some(timeout(Duration::from_secs(1), peer.output.recv()).await)
        } else {
            None
        };
        if received.is_err() {
            task.abort();
        } else {
            drop(peer.input);
        }
        let _ = task.await;
        let Message::Text(text) = received
            .expect("completed read must deliver during held authority")
            .unwrap()
        else {
            panic!("record response expected")
        };
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["id"], "record");
        assert_eq!(value["ok"], true);
        assert_eq!(released, 4);
        assert_eq!(still_held, 1);
        assert_eq!(
            effects_before, 0,
            "pending input has no preauthorization effect"
        );
        if let Some(deferred) = deferred {
            let Message::Text(text) = deferred.unwrap().unwrap() else {
                panic!("deferred reply expected")
            };
            let value: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(value["id"], "deferred");
            match input {
                HeldAuthorityInput::Malformed => {
                    assert_eq!(value["error"]["code"], "invalid_request");
                    assert_eq!(effects.0.load(Ordering::SeqCst), 0);
                }
                HeldAuthorityInput::Ordinary => {
                    assert_eq!(value["ok"], true);
                    assert_eq!(effects.0.load(Ordering::SeqCst), 1);
                }
                HeldAuthorityInput::Periodic => unreachable!(),
            }
        }
    }
}
#[tokio::test(start_paused = true)]
async fn completed_record_delivers_during_periodic_authority() {
    successful_read_during_held_authority(HeldAuthorityInput::Periodic, false).await;
}
#[tokio::test(start_paused = true)]
async fn completed_record_delivers_during_malformed_authority() {
    successful_read_during_held_authority(HeldAuthorityInput::Malformed, false).await;
}
#[tokio::test(start_paused = true)]
async fn completed_record_delivers_during_ordinary_authority() {
    successful_read_during_held_authority(HeldAuthorityInput::Ordinary, false).await;
}
#[tokio::test(start_paused = true)]
async fn completed_record_expires_during_periodic_authority() {
    successful_read_during_held_authority(HeldAuthorityInput::Periodic, true).await;
}
#[tokio::test(start_paused = true)]
async fn completed_record_expires_during_malformed_authority() {
    successful_read_during_held_authority(HeldAuthorityInput::Malformed, true).await;
}
#[tokio::test(start_paused = true)]
async fn completed_record_expires_during_ordinary_authority() {
    successful_read_during_held_authority(HeldAuthorityInput::Ordinary, true).await;
}

#[tokio::test(start_paused = true)]
async fn held_input_authority_does_not_suspend_credential_expiry() {
    let (mut state, authority) = fixture(MembershipRole::Member);
    *authority.proof_expires_at.lock().unwrap() = Some(102);
    let session = authenticate(&state).await;
    let held = Arc::new(HeldSocketAuthority {
        authority: authority.clone(),
        hold: AtomicBool::new(true),
        entered: Notify::new(),
        release: Notify::new(),
        waiting: AtomicUsize::new(0),
    });
    let effects = Arc::new(HealthEffects(AtomicUsize::new(0)));
    state.access = held.clone();
    state.uptime_clock = effects.clone();
    state.settings = SessionSettings::new(
        Duration::from_secs(60),
        Duration::from_secs(60),
        Duration::from_secs(120),
    )
    .unwrap();
    let (socket, mut peer) = test_socket(None);
    let task = tokio::spawn(run_authenticated(socket, state, session));
    peer.request("deferred");
    timeout(Duration::from_secs(1), held.entered.notified())
        .await
        .unwrap();
    authority.now.store(102, Ordering::SeqCst);
    tokio::time::advance(Duration::from_secs(2)).await;
    let close = timeout(Duration::from_secs(1), peer.output.recv())
        .await
        .unwrap()
        .unwrap();
    let Message::Close(Some(close)) = close else {
        panic!("credential expiry close expected")
    };
    let reason: Value = serde_json::from_str(&close.reason).unwrap();
    assert_eq!(reason["code"], "credential_expired");
    timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(held.waiting.load(Ordering::SeqCst), 0);
    assert!(held.hold.load(Ordering::SeqCst));
    assert_eq!(effects.0.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn periodic_authority_cannot_authorize_deferred_input() {
    for malformed in [false, true] {
        let (mut state, authority) = fixture(MembershipRole::Member);
        let session = authenticate(&state).await;
        let held = Arc::new(HeldSocketAuthority {
            authority: authority.clone(),
            hold: AtomicBool::new(true),
            entered: Notify::new(),
            release: Notify::new(),
            waiting: AtomicUsize::new(0),
        });
        let effects = Arc::new(HealthEffects(AtomicUsize::new(0)));
        state.access = held.clone();
        state.uptime_clock = effects.clone();
        state.settings = SessionSettings::new(
            Duration::from_secs(60),
            Duration::from_secs(60),
            Duration::from_secs(1),
        )
        .unwrap();
        let (socket, mut peer) = test_socket(None);
        let task = tokio::spawn(run_authenticated(socket, state, session));
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(1)).await;
        timeout(Duration::from_secs(1), held.entered.notified())
            .await
            .unwrap();
        if malformed {
            peer.input
                .send(Ok(Message::Text(
                    r#"{"type":"req","id":"deferred","method":false,"params":{}}"#.into(),
                )))
                .unwrap();
        } else {
            peer.request("deferred");
        }
        timeout(Duration::from_secs(1), held.entered.notified())
            .await
            .unwrap();
        assert_eq!(
            held.waiting.load(Ordering::SeqCst),
            2,
            "periodic and input own distinct reads"
        );
        // Notify releases waiters in registration order: only the earlier periodic read.
        held.release.notify_one();
        timeout(Duration::from_secs(1), async {
            while held.waiting.load(Ordering::SeqCst) != 1 {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(held.waiting.load(Ordering::SeqCst), 1);
        assert!(
            peer.output.try_recv().is_err(),
            "periodic success must not admit deferred input"
        );
        assert_eq!(effects.0.load(Ordering::SeqCst), 0);
        *authority.snapshot.lock().unwrap() =
            snapshot(MembershipRole::Member, MembershipStatus::Disabled);
        held.release.notify_one();
        let close = timeout(Duration::from_secs(1), peer.output.recv())
            .await
            .unwrap()
            .unwrap();
        let Message::Close(Some(close)) = close else {
            panic!("invalid current authority close expected")
        };
        let reason: Value = serde_json::from_str(&close.reason).unwrap();
        assert_eq!(reason["code"], "authorization_lost");
        timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(held.waiting.load(Ordering::SeqCst), 0);
        assert_eq!(effects.0.load(Ordering::SeqCst), 0);
        assert!(
            peer.output.try_recv().is_err(),
            "no deferred refusal or health after close"
        );
    }
}

// Real non-entered threads retain the port's physical lease after cancellation.
// This fixture tests socket ownership; SDK storage/join adapters have their own tests.
struct HeldPhysicalRead {
    calls: AtomicUsize,
    gate: Arc<(Mutex<bool>, Condvar)>,
    joins: Mutex<Vec<JoinHandle<()>>>,
}
impl HeldPhysicalRead {
    fn start(&self, lease: RecordReadLease) -> Receiver<()> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let released = *self.gate.0.lock().unwrap();
        let gate = self.gate.clone();
        let (send, receive) = tokio::sync::oneshot::channel();
        let join = std::thread::spawn(move || {
            let (lock, release) = &*gate;
            let mut open = lock.lock().unwrap();
            while !*open {
                open = release.wait(open).unwrap();
            }
            drop(open);
            drop(lease);
            let _ = send.send(());
        });
        self.joins.lock().unwrap().push(join);
        if released {
            // Paused Tokio can outrun a runnable OS thread. After release, join
            // this real worker before exposing its completion receiver. Held
            // reads keep their asynchronous lease and cancellation behavior.
            self.release_and_join();
        }
        receive
    }
    fn release_and_join(&self) {
        let (lock, release) = &*self.gate;
        *lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        release.notify_all();
        let mut worker_panicked = false;
        for join in self
            .joins
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .drain(..)
        {
            worker_panicked |= join.join().is_err();
        }
        assert!(
            !worker_panicked || std::thread::panicking(),
            "physical fixture worker panicked"
        );
    }
}
impl Drop for HeldPhysicalRead {
    fn drop(&mut self) {
        self.release_and_join();
    }
}
impl RecordReadSource for HeldPhysicalRead {
    fn read<'a>(
        &'a self,
        _: ReceiverReadScope,
        _: RecordReadOperation,
        lease: RecordReadLease,
    ) -> RecordReadFuture<'a, RecordReadResponse> {
        let finished = self.start(lease);
        Box::pin(async move {
            finished.await.unwrap();
            Err(RecordReadError::TemporarilyUnavailable)
        })
    }
}
impl CatalogueReadSource for HeldPhysicalRead {
    fn read(
        &self,
        _: CatalogueReadScope,
        _: CatalogueReadOperation,
        lease: RecordReadLease,
    ) -> CatalogueReadFuture<'_> {
        let finished = self.start(lease);
        Box::pin(async move {
            finished.await.unwrap();
            Err(CatalogueReadError::SourceUnavailable)
        })
    }
}
async fn physical_response(peer: &mut TestPeer, id: &str, method: &str, params: &Value) -> Value {
    peer.input
        .send(Ok(Message::Text(
            json!({"type":"req","id":id,"method":method,"params":params})
                .to_string()
                .into(),
        )))
        .unwrap();
    let Message::Text(text) = timeout(Duration::from_secs(11), peer.output.recv())
        .await
        .unwrap()
        .unwrap()
    else {
        panic!("response expected")
    };
    serde_json::from_str(&text).unwrap()
}
async fn held_physical_session() -> (
    ProductRouteState,
    AuthenticatedSession,
    ConversationId,
    Arc<HeldPhysicalRead>,
) {
    let (state, authority) = fixture(MembershipRole::Member);
    authority.snapshot.lock().unwrap().credential = Credential::new(
        CredentialId::new("credential").unwrap(),
        PrincipalId::new("principal").unwrap(),
        OrganizationId::new("organization").unwrap(),
        AudienceId::new("gateway").unwrap(),
        100,
        200,
        vec![Grant::new(
            Action::new("conversation.read").unwrap(),
            Resource::new(
                OrganizationId::new("organization").unwrap(),
                ResourceId::new("gateway-resource").unwrap(),
            ),
        )],
    )
    .unwrap();
    let session = authenticate(&state).await;
    let repository = Arc::new(conversation_support::MemoryRepository::default());
    let id = ConversationId::new(&uuid::Uuid::new_v4().to_string()).unwrap();
    repository.records.lock().unwrap().insert(
        id.clone(),
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
    );
    let source = Arc::new(HeldPhysicalRead {
        calls: AtomicUsize::new(0),
        gate: Arc::new((Mutex::new(false), Condvar::new())),
        joins: Mutex::new(Vec::new()),
    });
    let mut state = state
        .with_passive_read(Arc::new(RecordBinding), repository)
        .with_record_source(source.clone())
        .with_catalogue_source(source.clone());
    state.settings = SessionSettings::new(
        Duration::from_secs(60),
        Duration::from_secs(60),
        Duration::from_secs(120),
    )
    .unwrap();
    (state, session, id, source)
}

async fn physical_read_timeout_retains_socket_admission(catalogue: bool) {
    let (state, session, id, source) = held_physical_session().await;
    let capacity = state.record_reads.clone();
    let (socket, mut peer) = test_socket(None);
    let first = tokio::spawn(run_authenticated(socket, state.clone(), session.clone()));
    let (socket, mut other) = test_socket(None);
    let second = tokio::spawn(run_authenticated(socket, state, session));
    let method = if catalogue {
        "conversation.catalogueHead"
    } else {
        "conversation.recordsHead"
    };
    let params = if catalogue {
        json!({"receiverId":"receiver","accessEpoch":"3"})
    } else {
        json!({"conversationId":id.to_string(),"receiverId":"receiver","accessEpoch":"3"})
    };
    let initial = physical_response(&mut peer, "initial", method, &params).await;
    let mut retries = Vec::new();
    let mut capacities = Vec::new();
    for attempt in 0..4 {
        retries
            .push(physical_response(&mut peer, &format!("retry-{attempt}"), method, &params).await);
        capacities.push(capacity.available_permits());
    }
    let competing = physical_response(&mut other, "other", method, &params).await;
    let calls_while_held = source.calls.load(Ordering::SeqCst);
    let remaining_while_held = capacity.available_permits();
    drop(other.input);
    second.await.unwrap();
    let after_disconnect = capacity.available_permits();
    // Cleanup precedes every assertion so the original-code probe cannot hang.
    source.release_and_join();
    let later = physical_response(&mut peer, "later", method, &params).await;
    source.release_and_join();
    drop(peer.input);
    first.await.unwrap();
    assert_eq!(initial["error"]["code"], "read_timeout");
    assert_eq!(
        capacities,
        vec![3; 4],
        "one socket must retain exactly one physical admission"
    );
    for retry in retries {
        assert_eq!(retry["error"]["code"], "temporarily_unavailable");
    }
    assert_eq!(
        competing["error"]["code"], "read_timeout",
        "another socket can use remaining capacity"
    );
    assert_eq!(calls_while_held, 2);
    assert_eq!(remaining_while_held, 2);
    assert_eq!(after_disconnect, 2, "disconnect retains physical ownership");
    assert_eq!(
        later["error"]["code"],
        if catalogue {
            "source_unavailable"
        } else {
            "temporarily_unavailable"
        }
    );
    assert_eq!(
        source.calls.load(Ordering::SeqCst),
        3,
        "later retry reaches source after physical release"
    );
    assert_eq!(capacity.available_permits(), 4);
}

#[tokio::test(start_paused = true)]
async fn record_physical_read_timeout_retains_socket_admission() {
    physical_read_timeout_retains_socket_admission(false).await;
}
#[tokio::test(start_paused = true)]
async fn catalogue_physical_read_timeout_retains_socket_admission() {
    physical_read_timeout_retains_socket_admission(true).await;
}

// Observe receive-side selection without exposing or replacing production lanes.
// This counter does not establish the resulting queue. Final typed delivery and
// deadline effects prove admission routing after selection.
struct ObservedAdmissionSocket {
    socket: TestSocket,
    busy_inputs: Arc<AtomicUsize>,
    control_inputs: Arc<AtomicUsize>,
    // Install backpressure only after the initial timeout is physically flushed.
    after_initial_flush: Option<Receiver<()>>,
}
impl Stream for ObservedAdmissionSocket {
    type Item = Result<Message, axum::Error>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let next = Pin::new(&mut self.socket).poll_next(cx);
        if let Poll::Ready(Some(Ok(Message::Text(text)))) = &next {
            let frame: Value = serde_json::from_str(text).unwrap();
            if frame["id"]
                .as_str()
                .is_some_and(|id| id.starts_with("control-"))
            {
                self.control_inputs.fetch_add(1, Ordering::SeqCst);
            }
            if frame["id"]
                .as_str()
                .is_some_and(|id| id.starts_with("busy"))
            {
                self.busy_inputs.fetch_add(1, Ordering::SeqCst);
            }
        }
        next
    }
}
impl Sink<Message> for ObservedAdmissionSocket {
    type Error = Error;
    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.socket).poll_ready(cx)
    }
    fn start_send(mut self: Pin<&mut Self>, message: Message) -> Result<(), Self::Error> {
        Pin::new(&mut self.socket).start_send(message)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let flushed = Pin::new(&mut self.socket).poll_flush(cx);
        if matches!(flushed, Poll::Ready(Ok(()))) && self.after_initial_flush.is_some() {
            self.socket.gate = self.after_initial_flush.take();
        }
        flushed
    }
    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.socket).poll_close(cx)
    }
}

fn passive_request(peer: &TestPeer, id: &str, method: &str, params: &Value) {
    peer.input
        .send(Ok(Message::Text(
            json!({"type":"req","id":id,"method":method,"params":params})
                .to_string()
                .into(),
        )))
        .unwrap();
}

async fn wait_for_count(count: &AtomicUsize, expected: usize) {
    // Yield keeps paused Tokio time from auto-advancing while admission runs.
    for _ in 0..1000 {
        if count.load(Ordering::SeqCst) == expected {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("admission did not reach {expected}");
}

const PASSIVE_METHODS: [&str; 5] = [
    "conversation.recordsHead",
    "conversation.recordsPage",
    "conversation.catalogueHead",
    "conversation.catalogueManifest",
    "conversation.catalogueResolve",
];

async fn slot_busy_passive_refusal(method: &str, stalled: bool, overflow: bool) -> bool {
    let (state, session, id, source) = held_physical_session().await;
    let capacity = state.record_reads.clone();
    let (release, gate) = tokio::sync::oneshot::channel();
    let (socket, mut peer) = test_socket(None);
    let busy_inputs = Arc::new(AtomicUsize::new(0));
    let control_inputs = Arc::new(AtomicUsize::new(0));
    let socket = ObservedAdmissionSocket {
        socket,
        busy_inputs: busy_inputs.clone(),
        control_inputs: control_inputs.clone(),
        after_initial_flush: if stalled { Some(gate) } else { None },
    };
    let task = tokio::spawn(run_authenticated(socket, state, session));
    let params = json!({"conversationId":id.to_string(),"receiverId":"receiver","accessEpoch":"3"});
    let initial =
        physical_response(&mut peer, "initial", "conversation.recordsHead", &params).await;
    // Discard the already completed timeout write's physical marker.
    peer.writing.try_recv().unwrap();
    wait_for_count(&source.calls, 1).await;
    let mut control_started = false;
    if stalled {
        // One control is physically writing and three remain queued: every
        // existing control slot is occupied before the passive retry arrives.
        for index in 0..4 {
            passive_request(
                &peer,
                &format!("control-{index}"),
                "conversation.close",
                &json!({}),
            );
        }
        wait_for_count(&control_inputs, 4).await;
        // Awaiting a ready notification does not advance the paused clock.
        for _ in 0..1000 {
            if peer.writing.try_recv().is_ok() {
                control_started = true;
                break;
            }
            tokio::task::yield_now().await;
        }
    }
    passive_request(&peer, "busy", method, &params);
    wait_for_count(&busy_inputs, 1).await;
    if overflow {
        // Let the writer capture the first refusal as its pending item. With a
        // stalled control send, the second fills the one-item record channel;
        // the third triggers bounded teardown, leaving a fourth input unread.
        for _ in 0..100 {
            tokio::task::yield_now().await;
        }
        passive_request(&peer, "busy-2", method, &params);
        wait_for_count(&busy_inputs, 2).await;
        passive_request(&peer, "busy-3", method, &params);
        wait_for_count(&busy_inputs, 3).await;
        passive_request(&peer, "busy-4", method, &params);
        for _ in 0..100 {
            tokio::task::yield_now().await;
        }
    }
    let received = busy_inputs.load(Ordering::SeqCst);
    let mut response = None;
    let ended_at_deadline;
    if stalled {
        tokio::time::advance(Duration::from_secs(31)).await;
        for _ in 0..100 {
            tokio::task::yield_now().await;
        }
        ended_at_deadline = task.is_finished();
    } else {
        response = Some(
            timeout(Duration::from_secs(1), peer.output.recv())
                .await
                .unwrap()
                .unwrap(),
        );
        ended_at_deadline = false;
    }
    let calls = source.calls.load(Ordering::SeqCst);
    let retained = capacity.available_permits();
    // Release and join even if the original implementation failed the deadline.
    source.release_and_join();
    if stalled && !task.is_finished() {
        task.abort();
    }
    drop(peer.input);
    let _ = task.await;
    drop(release);
    assert_eq!(initial["id"], "initial");
    assert_eq!(initial["error"]["code"], "read_timeout");
    assert_eq!(
        received,
        if overflow { 3 } else { 1 },
        "record-lane overflow must stop receiving further retries"
    );
    assert_eq!(calls, 1, "busy {method} must not enter either source");
    assert_eq!(
        retained, 3,
        "delivery teardown preserves physical ownership"
    );
    assert_eq!(
        capacity.available_permits(),
        4,
        "physical join releases capacity"
    );
    if stalled {
        assert!(
            control_started,
            "control saturation must include an unfinished physical write"
        );
        assert!(
            peer.output.try_recv().is_err(),
            "stalled transport wrote no late frame"
        );
    } else {
        let Message::Text(text) = response.unwrap() else {
            panic!("response expected")
        };
        let response: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(response["id"], "busy");
        assert_eq!(
            response["error"]["code"], "temporarily_unavailable",
            "{method}"
        );
    }
    ended_at_deadline
}

#[tokio::test(start_paused = true)]
async fn authenticated_slot_busy_passive_refusals_expire_during_control_saturation() {
    let mut completed = Vec::new();
    for method in PASSIVE_METHODS {
        completed.push(slot_busy_passive_refusal(method, true, false).await);
    }
    assert_eq!(
        completed,
        vec![true; 5],
        "all five passive refusals must own a deadline after the initial timeout was delivered"
    );
}
#[tokio::test(start_paused = true)]
async fn authenticated_slot_busy_passive_refusals_deliver_on_ready_transport() {
    for method in PASSIVE_METHODS {
        slot_busy_passive_refusal(method, false, false).await;
    }
}

#[tokio::test(start_paused = true)]
async fn authenticated_slot_busy_passive_refusal_overflow_stops_admission() {
    assert!(slot_busy_passive_refusal("conversation.recordsHead", true, true).await);
}

#[tokio::test(start_paused = true)]
async fn slotless_record_refusal_expires_under_continuously_ready_controls() {
    let (control_send, controls) = mpsc::channel(4);
    let (_refusal_send, refusals) = mpsc::channel(1);
    let (_ordinary_send, ordinary) = mpsc::channel(16);
    let (record_send, records) = mpsc::channel(1);
    record_send
        .send(QueuedRecordResponse::refusal(failure(
            "busy",
            "temporarily_unavailable",
        )))
        .await
        .unwrap();
    let lane = ReadyLane::Control(control_send, Arc::new(Semaphore::new(2)));
    lane.refill();
    let ready = Arc::new(Notify::new());
    let writes = Arc::new(AtomicUsize::new(0));
    let records_written = Arc::new(AtomicUsize::new(0));
    let socket = ReadySocket {
        lane,
        writes: writes.clone(),
        records_written: records_written.clone(),
        ready: ready.clone(),
    };
    let (sink, _incoming) = socket.split();
    let writer = tokio::spawn(write_authenticated(
        sink,
        controls,
        refusals,
        ordinary,
        records,
        Duration::from_secs(60),
        Arc::new(WatchDeliveries::new()),
        Arc::new(SubscriptionDeliveries::new(Default::default())),
    ));
    ready.notified().await;
    assert!(writes.load(Ordering::SeqCst) >= 64);
    tokio::time::advance(RECORD_SEND_TIMEOUT + Duration::from_millis(1)).await;
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
    let ended = writer.is_finished();
    if !ended {
        writer.abort();
    }
    let _ = writer.await;
    drop(record_send);
    assert!(
        ended,
        "continuously ready controls must not starve slotless passive refusal expiry"
    );
    assert_eq!(records_written.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn original_pending_watch_deadline_expires_during_another_physical_frame() {
    let (captured, _guard) = limit_log();
    let owners = Arc::new(WatchOwners::new(1, 1));
    let deliveries = Arc::new(WatchDeliveries::new());
    let original = Arc::new(owners.try_acquire(&watch_principal("a")).unwrap());
    assert!(deliveries.reserve(
        "watch".into(),
        original.clone()
    ));
    drop(original);
    deliveries.activate("watch");
    assert!(deliveries.notice("watch", Notice::Changed)); // Authority remains pending.
    let deadline = deliveries.deadline().unwrap();
    tokio::time::advance(Duration::from_secs(20)).await;
    let (_release, gate) = tokio::sync::oneshot::channel();
    let (socket, mut peer) = test_socket(Some(gate));
    let (sink, _incoming) = socket.split();
    let (_control_send, controls) = mpsc::channel(4);
    let (_refusal_send, refusals) = mpsc::channel(1);
    let (ordinary_send, ordinary) = mpsc::channel(16);
    let (_record_send, records) = mpsc::channel(1);
    let slots = Arc::new(Semaphore::new(1));
    ordinary_send
        .send(QueuedResponse {
            message: WireResponse::ordinary(success("ordinary-in-flight", &json!({}))),
            _slot: slots.clone().try_acquire_owned().unwrap().into(),
            _record_work: None,
            _mount: None,
        })
        .await
        .unwrap();
    let writer = tokio::spawn(write_authenticated(
        sink,
        controls,
        refusals,
        ordinary,
        records,
        Duration::from_secs(60),
        deliveries.clone(),
        Arc::new(SubscriptionDeliveries::new(Default::default())),
    ));
    peer.writing.recv().await.unwrap(); // Ordinary physical flush has a later own budget.
    assert!(peer.output.try_recv().is_err());
    assert_eq!(deliveries.deadline(), Some(deadline));
    assert_eq!(owners.available_permits(), 0);
    tokio::time::advance(Duration::from_secs(10)).await;
    timeout(Duration::from_secs(1), writer)
        .await
        .unwrap()
        .unwrap();
    assert!(peer.output.try_recv().is_err()); // Abandoned frame never physically flushed.
    assert!(peer.writing.try_recv().is_err()); // No hint or later error send.
    assert_eq!(slots.available_permits(), 1);
    assert_eq!(owners.available_permits(), 0); // Pending authority/source interest is separately owned.
    let logged = limit_text(&captured);
    assert!(
        logged.contains("socket.watch_delivery_deadline"),
        "{logged}"
    );
    assert!(
        !logged.contains("socket.record_delivery_deadline"),
        "{logged}"
    );
    deliveries.close();
    assert_eq!(owners.available_permits(), 1);
}

/// Row B4: neither continuously ready controls nor continuously ready
/// ordinary responses can starve a pending watch deadline.
#[tokio::test(start_paused = true)]
async fn pending_watch_deadline_cannot_be_starved_by_continuously_ready_controls() {
    pending_watch_deadline_survives_a_continuously_ready_lane(false).await;
}

#[tokio::test(start_paused = true)]
async fn pending_watch_deadline_cannot_be_starved_by_continuously_ready_ordinary_responses() {
    pending_watch_deadline_survives_a_continuously_ready_lane(true).await;
}

async fn pending_watch_deadline_survives_a_continuously_ready_lane(ordinary_lane: bool) {
    let owners = Arc::new(WatchOwners::new(1, 1));
    let deliveries = Arc::new(WatchDeliveries::new());
    assert!(deliveries.reserve(
        "watch".into(),
        Arc::new(owners.try_acquire(&watch_principal("a")).unwrap())
    ));
    deliveries.activate("watch");
    assert!(deliveries.notice("watch", Notice::Changed)); // No fabricated allowed authority snapshot.
    let (control_send, controls) = mpsc::channel(4);
    let (_refusal_send, refusals) = mpsc::channel(1);
    let (ordinary_send, ordinary) = mpsc::channel(16);
    let (_record_send, records) = mpsc::channel(1);
    // The busy lane refills itself; the other lane stays open and idle.
    let (lane, _idle_control, _idle_ordinary) = if ordinary_lane {
        let lane = ReadyLane::Ordinary(ordinary_send, Arc::new(Semaphore::new(2)));
        (lane, Some(control_send), None)
    } else {
        let lane = ReadyLane::Control(control_send, Arc::new(Semaphore::new(2)));
        (lane, None, Some(ordinary_send))
    };
    lane.refill();
    let ready = Arc::new(Notify::new());
    let writes = Arc::new(AtomicUsize::new(0));
    let records_written = Arc::new(AtomicUsize::new(0));
    let socket = ReadySocket {
        lane,
        writes: writes.clone(),
        records_written: records_written.clone(),
        ready: ready.clone(),
    };
    let (sink, _incoming) = socket.split();
    let writer = tokio::spawn(write_authenticated(
        sink,
        controls,
        refusals,
        ordinary,
        records,
        Duration::from_secs(60),
        deliveries.clone(),
        Arc::new(SubscriptionDeliveries::new(Default::default())),
    ));
    ready.notified().await;
    assert!(writes.load(Ordering::SeqCst) >= 64); // Traffic genuinely remains ready.
    tokio::time::advance(RECORD_SEND_TIMEOUT).await;
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
    let completed = writer.is_finished();
    if !completed {
        writer.abort();
    } // Bound a failing removal probe without concealing failure.
    let _ = writer.await;
    assert!(
        completed,
        "a continuously ready lane must not starve the original watch deadline"
    );
    assert_eq!(records_written.load(Ordering::SeqCst), 0);
    assert_eq!(owners.available_permits(), 0);
    deliveries.close();
    assert_eq!(owners.available_permits(), 1);
}

/// Row U3: a notice is ready while the writer is busy with an ordinary frame,
/// and an unwatch retires the watch before the writer chooses the notice.
/// Nothing for that watch may follow the unwatch acknowledgement.
#[tokio::test]
async fn unwatch_before_writer_selection_sends_no_hint_after_the_acknowledgement() {
    let owners = Arc::new(WatchOwners::new(1, 1));
    let deliveries = Arc::new(WatchDeliveries::new());
    assert!(deliveries.reserve(
        "watch".into(),
        Arc::new(owners.try_acquire(&watch_principal("a")).unwrap())
    ));
    deliveries.activate("watch");
    assert!(deliveries.notice("watch", Notice::Changed));
    deliveries.authorize("watch"); // The hint is ready to send.
    let (release, gate) = tokio::sync::oneshot::channel();
    let (socket, mut peer) = test_socket(Some(gate));
    let (sink, _incoming) = socket.split();
    let (control_send, controls) = mpsc::channel(4);
    let (refusal_send, refusals) = mpsc::channel(1);
    let (ordinary_send, ordinary) = mpsc::channel(16);
    let (record_send, records) = mpsc::channel(1);
    let slots = Arc::new(Semaphore::new(2));
    let response = |id: &str| QueuedResponse {
        message: WireResponse::ordinary(success(id, &json!({}))),
        _slot: slots.clone().try_acquire_owned().unwrap().into(),
        _record_work: None,
        _mount: None,
    };
    ordinary_send.send(response("ordinary")).await.unwrap();
    let writer = tokio::spawn(write_authenticated(
        sink,
        controls,
        refusals,
        ordinary,
        records,
        Duration::from_secs(60),
        deliveries.clone(),
        Arc::new(SubscriptionDeliveries::new(Default::default())),
    ));
    peer.writing.recv().await.unwrap(); // The ordinary frame is mid-flush.
    // What `ConnectionWatches::begin` does for an unwatch: retire, then queue
    // the acknowledgement on the ordinary lane.
    deliveries.retire("watch");
    ordinary_send.send(response("unwatch")).await.unwrap();
    release.send(()).unwrap();
    for expected in ["ordinary", "unwatch"] {
        let Message::Text(text) = peer.message().await else {
            panic!("expected {expected}")
        };
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["id"], expected, "{value}");
    }
    // With every lane closed, a locally held hint would be all that is left.
    drop((control_send, refusal_send, ordinary_send, record_send));
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
    assert!(
        peer.output.try_recv().is_err(),
        "no hint may follow the unwatch acknowledgement"
    );
    assert!(!writer.is_finished());
    deliveries.close();
    timeout(Duration::from_secs(1), writer)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(owners.available_permits(), 1);
}
