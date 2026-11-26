//! Physical admission owns effect capture even after the calling future ends.
use super::{NativeClientError, NativeEnrollmentClient};
use crate::retained::RetainedEnrollmentClient;
use nessa_auth::application::ports::Clock as WallClock;
use nessa_auth::{
    application::pairing::{
        ClientPendingStore, DeviceCredential, PendingEnrollment, PrivateKeyMaterial,
        PrivateStateError,
    },
    domain::{
        pairing::{AttemptId, ConsentClass, ConsentIntentId, InvitationId, PublicIntent},
        CredentialId, ResourceId,
    },
};
use nessa_protocol::clock::Clock;
use std::{
    io::Read,
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

struct DiscoverySaved {
    intent: PublicIntent,
    pin: [u8; 44],
    credential: CredentialId,
}
impl ClientPendingStore for DiscoverySaved {
    fn load_pending(&self) -> Result<Option<PendingEnrollment>, PrivateStateError> {
        Empty.load_pending()
    }
    fn load_credential(&self) -> Result<Option<DeviceCredential>, PrivateStateError> {
        Ok(Some(DeviceCredential::new(
            PendingEnrollment::new(
                PrivateKeyMaterial::new(zeroize::Zeroizing::new([7; 32])),
                self.pin,
                self.intent,
            ),
            self.credential.clone(),
            ResourceId::new("receiver").unwrap(),
        )))
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn discovery_refuses_replaced_enrollment_before_any_tls_or_exchange_bytes() {
    let intent = PublicIntent::new(
        InvitationId::new([1; 16]),
        AttemptId::new([2; 16]),
        ConsentIntentId::new([3; 16]),
        1,
        1000,
        ConsentClass::PeerRead,
    )
    .unwrap();
    let credential = CredentialId::new("issued").unwrap();
    for (expected, expected_credential) in [
        (
            intent.with_attempt(AttemptId::new([4; 16])),
            credential.clone(),
        ),
        (intent, CredentialId::new("replaced").unwrap()),
    ] {
        let client = NativeEnrollmentClient::enrolling(
            ConsentClass::PeerRead,
            Arc::new(DiscoverySaved {
                intent,
                pin: [8; 44],
                credential: credential.clone(),
            }),
            Arc::new(Time),
        );
        let (stream, mut peer) = sockets();
        let result: Result<(), NativeClientError> = timeout(
            Duration::from_secs(5),
            client.open_discovery(stream, expected, expected_credential, |_| {
                panic!("stale attempt reached exchange")
            }),
        )
        .await
        .unwrap();
        assert_eq!(
            result,
            Err(NativeClientError::Storage(PrivateStateError::Conflict))
        );
        peer.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
        assert_eq!(
            peer.read(&mut [0; 1]).unwrap(),
            0,
            "no TLS or discovery byte preceded identity admission"
        );
        client.shutdown().await;
    }
}

#[cfg(feature = "test-support")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn joined_pinned_status_delivers_one_authenticated_refusal_without_replay() {
    use crate::retained::{joined_status::JoinedStatusRoute, ReadStop, TokioJoinedDial};
    use nessa_auth::adapters::pairing::{relay_key, NativeIdentity, NativeTransport, OsEntropy};
    use nessa_protocol::{
        pairing::{
            wire::{decode_request, encode_refused, NativePairingRequest},
            EnrollmentChannel,
        },
        relay::{read_frame, write_frame, RelayFrame},
    };

    let intent = PublicIntent::new(
        InvitationId::new([1; 16]),
        AttemptId::new([2; 16]),
        ConsentIntentId::new([3; 16]),
        1,
        1_000,
        ConsentClass::DeviceRead,
    )
    .unwrap();
    let gateway = NativeIdentity::generate(&mut OsEntropy).unwrap();
    let pin = gateway.public_spki();
    let key = relay_key(&pin).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let client = NativeEnrollmentClient::new(
        Arc::new(DiscoverySaved {
            intent,
            pin,
            credential: CredentialId::new("issued").unwrap(),
        }),
        Arc::new(Time),
    );

    // The owner selected a different pin: refuse before any TCP connection.
    let other = NativeIdentity::generate(&mut OsEntropy)
        .unwrap()
        .public_spki();
    let wrong = client
        .status_joined_completed(
            JoinedStatusRoute::new(address, other, Arc::new(TokioJoinedDial)).unwrap(),
            ReadStop::new(),
            3_000,
            None,
            || {},
            |r| r,
        )
        .await
        .unwrap();
    assert!(matches!(wrong, Err(NativeClientError::Phase)));
    listener.set_nonblocking(true).unwrap();
    assert!(matches!(listener.accept(),Err(e) if e.kind()==std::io::ErrorKind::WouldBlock));

    // Use the same accepted socket for relay framing, pinned TLS and canonical
    // status. A received application refusal is authoritative for this attempt.
    listener.set_nonblocking(false).unwrap();
    let peer = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        assert!(
            matches!(read_frame(&mut socket), Ok(RelayFrame::Connect { key: observed }) if observed==key)
        );
        write_frame(&mut socket, &RelayFrame::Joined).unwrap();
        let mut tls = EnrollmentChannel::new(NativeTransport::accept(socket, &gateway).unwrap());
        let request = decode_request(&tls.receive_envelope().unwrap()).unwrap();
        assert!(matches!(request, NativePairingRequest::Status(observed) if observed==intent));
        tls.send_envelope(&encode_refused().unwrap()).unwrap();
        assert!(
            tls.receive_envelope().is_err(),
            "cannot replay status on the joined socket"
        );
        listener.set_nonblocking(true).unwrap();
        assert!(
            matches!(listener.accept(),Err(e) if e.kind()==std::io::ErrorKind::WouldBlock),
            "cannot redial after the authenticated refusal"
        );
    });
    let refusal = timeout(
        Duration::from_secs(5),
        client.status_joined_completed(
            JoinedStatusRoute::new(address, pin, Arc::new(TokioJoinedDial)).unwrap(),
            ReadStop::new(),
            3_000,
            None,
            || {},
            |r| r,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(matches!(refusal, Err(NativeClientError::Refused)));
    peer.join().unwrap();
}

#[cfg(feature = "test-support")]
#[tokio::test(flavor = "current_thread")]
async fn joined_selected_owner_injects_one_dial_and_drains_on_stop() {
    use crate::retained::{
        joined_status::{JoinedDialFuture, JoinedStatusDial, JoinedStatusRoute},
        ReadStop,
    };
    use nessa_auth::adapters::pairing::{NativeIdentity, OsEntropy};
    use std::{
        future::poll_fn,
        io::{Error, ErrorKind},
        net::SocketAddr,
        sync::atomic::AtomicBool,
        task::Poll,
    };

    struct DropWitness(Arc<AtomicBool>);
    impl Drop for DropWitness {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    struct Dialogue {
        address: SocketAddr,
        calls: AtomicUsize,
        pending: bool,
        polled: Arc<Notify>,
        dropped: Arc<AtomicBool>,
    }
    impl JoinedStatusDial for Dialogue {
        fn dial(&self, address: SocketAddr) -> JoinedDialFuture {
            assert_eq!(address, self.address);
            self.calls.fetch_add(1, Ordering::SeqCst);
            if !self.pending {
                return Box::pin(async { Err(Error::from(ErrorKind::ConnectionRefused)) });
            }
            let witness = DropWitness(self.dropped.clone());
            let polled = self.polled.clone();
            Box::pin(poll_fn(move |_| {
                let _retained = &witness;
                polled.notify_one();
                Poll::Pending
            }))
        }
    }
    let address = "127.0.0.1:9".parse().unwrap();
    let pin = NativeIdentity::generate(&mut OsEntropy)
        .unwrap()
        .public_spki();
    let intent = PublicIntent::new(
        InvitationId::new([1; 16]),
        AttemptId::new([2; 16]),
        ConsentIntentId::new([3; 16]),
        1,
        1_000,
        ConsentClass::DeviceRead,
    )
    .unwrap();
    let client = NativeEnrollmentClient::new(
        Arc::new(DiscoverySaved {
            intent,
            pin,
            credential: CredentialId::new("issued").unwrap(),
        }),
        Arc::new(Time),
    );
    for (expires, pend, expected_calls, expected_kind) in [
        (0, false, 0, ErrorKind::TimedOut),
        (1_000, false, 1, ErrorKind::ConnectionRefused),
        (1_000, true, 1, ErrorKind::ConnectionAborted),
    ] {
        let stop = ReadStop::new();
        let port = Arc::new(Dialogue {
            address,
            calls: AtomicUsize::new(0),
            pending: pend,
            polled: Arc::new(Notify::new()),
            dropped: Arc::new(AtomicBool::new(false)),
        });
        let route = JoinedStatusRoute::new(address, pin, port.clone()).unwrap();
        let action =
            client.status_joined_completed(route, stop.clone(), expires, None, || {}, |r| r);
        let result = if pend {
            let stop_after_poll = async {
                port.polled.notified().await;
                stop.stop();
            };
            timeout(Duration::from_secs(3), async {
                tokio::join!(action, stop_after_poll).0
            })
            .await
            .unwrap()
        } else {
            timeout(Duration::from_secs(3), action).await.unwrap()
        };
        assert!(
            matches!(result.unwrap(), Err(NativeClientError::Io(kind)) if kind == expected_kind)
        );
        assert_eq!(port.calls.load(Ordering::SeqCst), expected_calls);
        assert_eq!(port.dropped.load(Ordering::SeqCst), pend);
    }
}
