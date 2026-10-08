//! Controlled HTTP body chunks and physical destruction gates shared by POST regressions.
use super::super::{HttpBody, HttpChunks, HttpFailure};
use async_trait::async_trait;
use serde_json::Value;
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, Notify};

pub(super) struct DropGate {
    pub(super) entered: Notify,
    pub(super) released: Mutex<bool>,
    pub(super) ready: Condvar,
}

impl DropGate {
    pub(super) fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.ready.notify_all();
    }
}

pub(super) struct ReleaseOnDrop(pub(super) Arc<DropGate>);

impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        self.0.release();
    }
}

struct Body {
    chunks: mpsc::UnboundedReceiver<Result<Option<Vec<u8>>, HttpFailure>>,
    pub(super) dropped: Arc<AtomicUsize>,
    pub(super) changed: Arc<Notify>,
    pub(super) polled: Arc<AtomicUsize>,
    pub(super) polling: Arc<Notify>,
    pub(super) drop_gate: Arc<Mutex<Option<Arc<DropGate>>>>,
}

impl Drop for Body {
    fn drop(&mut self) {
        if let Some(gate) = self.drop_gate.lock().unwrap().as_ref() {
            gate.entered.notify_one();
            let mut released = gate.released.lock().unwrap();
            while !*released {
                released = gate.ready.wait(released).unwrap();
            }
        }
        self.dropped.fetch_add(1, Ordering::SeqCst);
        self.changed.notify_one();
    }
}

#[async_trait]
impl HttpChunks for Body {
    async fn next(&mut self) -> Result<Option<Vec<u8>>, HttpFailure> {
        self.polled.fetch_add(1, Ordering::SeqCst);
        self.polling.notify_one();
        self.chunks.recv().await.unwrap_or(Ok(None))
    }
}

pub(super) struct Probe {
    pub(super) send: mpsc::UnboundedSender<Result<Option<Vec<u8>>, HttpFailure>>,
    pub(super) dropped: Arc<AtomicUsize>,
    pub(super) changed: Arc<Notify>,
    pub(super) polled: Arc<AtomicUsize>,
    pub(super) polling: Arc<Notify>,
    pub(super) drop_gate: Arc<Mutex<Option<Arc<DropGate>>>>,
}

impl Probe {
    pub(super) fn body() -> (Self, HttpBody) {
        let (send, chunks) = mpsc::unbounded_channel();
        let dropped = Arc::new(AtomicUsize::new(0));
        let changed = Arc::new(Notify::new());
        let polled = Arc::new(AtomicUsize::new(0));
        let polling = Arc::new(Notify::new());
        let drop_gate = Arc::new(Mutex::new(None));
        (
            Self {
                send,
                dropped: dropped.clone(),
                changed: changed.clone(),
                polled: polled.clone(),
                polling: polling.clone(),
                drop_gate: drop_gate.clone(),
            },
            HttpBody::Stream(Box::new(Body {
                chunks,
                dropped,
                changed,
                polled,
                polling,
                drop_gate,
            })),
        )
    }

    pub(super) fn event(&self, value: Value) {
        self.send
            .send(Ok(Some(format!("data: {value}\n\n").into_bytes())))
            .unwrap();
    }

    pub(super) async fn polled(&self, count: usize) {
        bounded(async {
            while self.polled.load(Ordering::SeqCst) < count {
                self.polling.notified().await;
            }
        })
        .await;
    }

    pub(super) async fn released(&self) {
        bounded(async {
            while self.dropped.load(Ordering::SeqCst) == 0 {
                self.changed.notified().await;
            }
        })
        .await;
        assert_eq!(self.dropped.load(Ordering::SeqCst), 1);
    }
}

pub(super) async fn bounded<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(3), future)
        .await
        .expect("controlled lifecycle completes")
}
