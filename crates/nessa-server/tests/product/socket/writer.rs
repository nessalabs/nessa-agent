use super::*;
use std::{
    pin::Pin,
    sync::atomic::{AtomicUsize, Ordering},
    task::{Context, Poll},
};
use tokio::sync::Notify;

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
                    _slot: slots.clone().try_acquire_owned().unwrap(),
                    _record_work: None,
                })))
                .unwrap(),
            Self::Refusal(sender) => sender.try_send(message).unwrap(),
            Self::Ordinary(sender, slots) => sender
                .try_send(QueuedResponse {
                    message: WireResponse::ordinary(message),
                    _slot: slots.clone().try_acquire_owned().unwrap(),
                    _record_work: None,
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
    type Error = std::io::Error;
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
async fn continuously_ready_lane_releases_all_record_leases(lane: PriorityLane) {
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
            .send(QueuedResponse {
                message: WireResponse::record("{\"record\":true}".into()),
                _slot: slots.clone().try_acquire_owned().unwrap(),
                _record_work: Some(RecordReadLease::new(Box::new(
                    global_reads.clone().try_acquire_owned().unwrap(),
                ))),
            })
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
        )));
        ready.notified().await;
        assert!(writes.load(Ordering::SeqCst) >= 64);
        assert_eq!(slots.available_permits(), 0);
        record_slots.push(slots);
        senders.push((control_send, refusal_send, ordinary_send, record_send));
    }
    assert_eq!(global_reads.available_permits(), 0);
    tokio::time::advance(RECORD_SEND_TIMEOUT + Duration::from_millis(1)).await;
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
    let terminated = writers.iter().all(tokio::task::JoinHandle::is_finished);
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
    continuously_ready_lane_releases_all_record_leases(PriorityLane::Control).await;
}
#[tokio::test(start_paused = true)]
async fn queued_record_deadline_survives_continuously_ready_refusals() {
    continuously_ready_lane_releases_all_record_leases(PriorityLane::Refusal).await;
}
#[tokio::test(start_paused = true)]
async fn queued_record_deadline_survives_continuously_ready_ordinary() {
    continuously_ready_lane_releases_all_record_leases(PriorityLane::Ordinary).await;
}

async fn arriving_record_interrupts_stalled_priority(close: bool) {
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
                _slot: Arc::new(Semaphore::new(1)).try_acquire_owned().unwrap(),
                _record_work: None,
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
    ));
    peer.writing.recv().await.unwrap();
    record_send
        .send(QueuedResponse {
            message: WireResponse::record("{\"record\":true}".into()),
            _slot: record_slots.clone().try_acquire_owned().unwrap(),
            _record_work: Some(RecordReadLease::new(Box::new(
                global_reads.clone().try_acquire_owned().unwrap(),
            ))),
        })
        .await
        .unwrap();
    tokio::task::yield_now().await;
    assert_eq!(record_slots.available_permits(), 0);
    assert_eq!(global_reads.available_permits(), 3);
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
    arriving_record_interrupts_stalled_priority(false).await;
}
#[tokio::test(start_paused = true)]
async fn queued_record_arrival_deadline_interrupts_stalled_close() {
    arriving_record_interrupts_stalled_priority(true).await;
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
        _slot: slots.clone().try_acquire_owned().unwrap(),
        _record_work: None,
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
        .send(QueuedResponse {
            message: WireResponse::record(serde_json::to_string(&json!({"id":"record"})).unwrap()),
            _slot: slots.clone().try_acquire_owned().unwrap(),
            _record_work: Some(RecordReadLease::new(Box::new(
                global_reads.clone().try_acquire_owned().unwrap(),
            ))),
        })
        .await
        .unwrap();
    drop((control_send, refusal_send, ordinary_send, record_send));
    let writer = tokio::spawn(write_authenticated(
        sink,
        controls,
        refusals,
        ordinary,
        records,
        Duration::from_secs(5),
    ));
    for expected in ["control", "refusal", "ordinary", "record"] {
        let Message::Text(text) = peer.message().await else {
            panic!("text response expected")
        };
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&text).unwrap()["id"],
            expected
        );
    }
    writer.await.unwrap();
    assert_eq!(slots.available_permits(), 3);
    assert_eq!(global_reads.available_permits(), 4);
}
