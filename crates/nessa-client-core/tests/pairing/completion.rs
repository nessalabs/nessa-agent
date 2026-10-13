//! Physical admission owns effect capture even after the calling future ends.
use super::{NativeClientError, NativeEnrollmentClient};
use crate::retained::RetainedEnrollmentClient;
use nessa_auth::application::ports::Clock as WallClock;
use nessa_auth::{
    application::pairing::{
        ClientPendingStore, DeviceCredential, PendingEnrollment, PrivateKeyMaterial,
        PrivateStateError,
    },
    domain::{pairing::PublicIntent, CredentialId, ResourceId},
};
use nessa_protocol::clock::Clock;
use std::{
    net::{TcpListener, TcpStream},
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc::{channel, Receiver},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{sync::Notify, time::timeout};

struct Time;
impl Clock for Time {
    fn elapsed_ms(&self) -> u64 {
        0
    }
}
struct Wall;
impl WallClock for Wall {
    fn unix_milliseconds(&self) -> u64 {
        0
    }
}
struct Empty;
impl ClientPendingStore for Empty {
    fn load_pending(&self) -> Result<Option<PendingEnrollment>, PrivateStateError> {
        Ok(None)
    }
    fn load_credential(&self) -> Result<Option<DeviceCredential>, PrivateStateError> {
        Ok(None)
    }
    fn save_pending(
        &self,
        _: &PrivateKeyMaterial,
        _: &[u8; 44],
        _: PublicIntent,
        _: Option<PublicIntent>,
    ) -> Result<(), PrivateStateError> {
        Err(PrivateStateError::Unavailable)
    }
    fn save_credential(
        &self,
        _: &CredentialId,
        _: &ResourceId,
        _: PublicIntent,
    ) -> Result<(), PrivateStateError> {
        Err(PrivateStateError::Unavailable)
    }
    fn end_enrollment(&self, _: PublicIntent) -> Result<(), PrivateStateError> {
        Err(PrivateStateError::Unavailable)
    }
}

struct Held {
    entered: Arc<Notify>,
    release: Mutex<Receiver<()>>,
}
impl ClientPendingStore for Held {
    fn load_pending(&self) -> Result<Option<PendingEnrollment>, PrivateStateError> {
        self.entered.notify_one();
        self.release
            .lock()
            .unwrap()
            .recv()
            .map_err(|_| PrivateStateError::Unavailable)?;
        Ok(None)
    }
    fn load_credential(&self) -> Result<Option<DeviceCredential>, PrivateStateError> {
        Empty.load_credential()
    }
    fn save_pending(
        &self,
        key: &PrivateKeyMaterial,
        pin: &[u8; 44],
        intent: PublicIntent,
        expected: Option<PublicIntent>,
    ) -> Result<(), PrivateStateError> {
        Empty.save_pending(key, pin, intent, expected)
    }
    fn save_credential(
        &self,
        credential: &CredentialId,
        receiver: &ResourceId,
        expected: PublicIntent,
    ) -> Result<(), PrivateStateError> {
        Empty.save_credential(credential, receiver, expected)
    }
    fn end_enrollment(&self, expected: PublicIntent) -> Result<(), PrivateStateError> {
        Empty.end_enrollment(expected)
    }
}

fn sockets() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    (client, listener.accept().unwrap().0)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn completion_holds_physical_admission_and_busy_cannot_begin_capture() {
    let client = Arc::new(NativeEnrollmentClient::new(Arc::new(Empty), Arc::new(Time)));
    let captured = Arc::new(Notify::new());
    let calls = Arc::new(AtomicUsize::new(0));
    let (release, held) = channel();
    let (stream, _peer) = sockets();
    let owned = client.clone();
    let complete = captured.clone();
    let first = calls.clone();
    let operation = tokio::spawn(async move {
        owned
            .status_completed(
                stream,
                None,
                move || {
                    first.fetch_add(1, Ordering::SeqCst);
                },
                move |result| {
                    complete.notify_one();
                    held.recv().unwrap();
                    result
                },
            )
            .await
    });
    timeout(Duration::from_secs(5), captured.notified())
        .await
        .unwrap();
    let second = calls.clone();
    let (stream, _second_peer) = sockets();
    let refused = client
        .status_completed(
            stream,
            None,
            move || {
                second.fetch_add(1, Ordering::SeqCst);
            },
            |result| result,
        )
        .await;
    assert!(matches!(refused, Err(NativeClientError::Busy)));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    release.send(()).unwrap();
    assert!(matches!(
        timeout(Duration::from_secs(5), operation)
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        Err(NativeClientError::NoPending)
    ));
    let (stream, _third_peer) = sockets();
    assert_eq!(
        client.status(stream, None).await,
        Err(NativeClientError::NoPending)
    );
    client.shutdown().await;
}

#[tokio::test]
async fn shutdown_signal_refuses_new_physical_dispatch_before_effect_capture() {
    let client = NativeEnrollmentClient::new(Arc::new(Empty), Arc::new(Time));
    let begin_count = Arc::new(AtomicUsize::new(0));
    let complete_count = Arc::new(AtomicUsize::new(0));
    client.request_shutdown();
    let original_wake = client.wake_report();
    assert!(original_wake.is_some());
    let (stream, _peer) = sockets();
    let begin = begin_count.clone();
    let complete = complete_count.clone();
    let result = client
        .status_completed(
            stream,
            None,
            move || {
                begin.fetch_add(1, Ordering::SeqCst);
            },
            move |result| {
                complete.fetch_add(1, Ordering::SeqCst);
                result
            },
        )
        .await;
    assert_eq!(result, Err(NativeClientError::Busy));
    assert_eq!(begin_count.load(Ordering::SeqCst), 0);
    assert_eq!(complete_count.load(Ordering::SeqCst), 0);
    timeout(Duration::from_secs(5), client.shutdown())
        .await
        .unwrap();
    client.request_shutdown();
    assert_eq!(client.wake_report(), original_wake);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn canceled_caller_and_shutdown_waiter_keep_physical_work_and_capture() {
    let entered = Arc::new(Notify::new());
    let (release, held) = channel();
    let state = Arc::new(Held {
        entered: entered.clone(),
        release: Mutex::new(held),
    });
    let client = Arc::new(NativeEnrollmentClient::new(state, Arc::new(Time)));
    let captured = Arc::new(Notify::new());
    let complete = captured.clone();
    let owned = client.clone();
    let (stream, _peer) = sockets();
    let operation = tokio::spawn(async move {
        owned
            .status_completed(
                stream,
                None,
                || {},
                move |result| {
                    complete.notify_one();
                    result
                },
            )
            .await
    });
    timeout(Duration::from_secs(5), entered.notified())
        .await
        .unwrap();
    operation.abort();
    assert!(operation.await.unwrap_err().is_cancelled());
    let (stream, _second_peer) = sockets();
    assert_eq!(
        client.status(stream, None).await,
        Err(NativeClientError::Busy)
    );
    let owned = client.clone();
    let shutdown = tokio::spawn(async move { owned.shutdown().await });
    timeout(Duration::from_secs(5), async {
        while client.wake_report().is_none() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    shutdown.abort();
    assert!(shutdown.await.unwrap_err().is_cancelled());
    let (stream, _third_peer) = sockets();
    assert_eq!(
        client.status(stream, None).await,
        Err(NativeClientError::Busy)
    );
    release.send(()).unwrap();
    timeout(Duration::from_secs(5), captured.notified())
        .await
        .unwrap();
    timeout(Duration::from_secs(5), client.shutdown())
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn last_handle_drop_keeps_private_state_owned_until_physical_completion() {
    let directory = tempfile::tempdir().unwrap();
    let entered = Arc::new(Notify::new());
    let (release, held) = channel();
    let state = Arc::new(Held {
        entered: entered.clone(),
        release: Mutex::new(held),
    });
    let retained_state = Arc::downgrade(&state);
    let client = Arc::new(
        RetainedEnrollmentClient::new(
            state,
            directory.path().join("unused.sqlite3"),
            Arc::new(Wall),
            Arc::new(Time),
        )
        .unwrap(),
    );
    let handle = Arc::downgrade(&client);
    let (stream, _peer) = sockets();
    let operation = tokio::spawn(async move { client.status(stream, None).await });
    timeout(Duration::from_secs(5), entered.notified())
        .await
        .unwrap();
    operation.abort();
    assert!(operation.await.unwrap_err().is_cancelled());
    assert!(handle.upgrade().is_none());
    assert!(
        retained_state.upgrade().is_some(),
        "physical worker still owns private state"
    );
    release.send(()).unwrap();
    timeout(Duration::from_secs(5), async {
        while retained_state.upgrade().is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        !directory.path().join("unused.sqlite3").exists(),
        "no purge or cache open for NoPending"
    );
}
