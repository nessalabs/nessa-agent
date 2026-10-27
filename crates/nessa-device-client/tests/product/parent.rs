//! Parent admission and private-scope lifetime across escapable foreign children.
use super::*;
use nessa_local_storage::create_directory_beneath;

pub(crate) mod fixtures;
use crate::{
    test_support::PrivateFixture, NativeConnectionOutcome, NativeDeviceSession,
    NativeProductCallError, NativeProductCloseReason, NativeProductCompletionKind,
    NativeProductFailure, NativeProductFailureKind, NativeProductSession,
};
use fixtures::{CleanupConnector, Held, HeldConnector};
use nessa_auth::application::pairing::{DeviceCredential, PendingEnrollment, PrivateKeyMaterial};
use nessa_client_core::product::{ConnectedProduct, ProductConnector};
use nessa_protocol::{
    pairing::MAX_PROTECTED_RESPONSE_BYTES,
    protocol::{EventFrame, OutgoingMessage, ResponseFrame},
};
use serde_json::{json, Value};
use std::{
    io::{Error as IoError, Read, Result as IoResult},
    net::TcpListener,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::Instant,
};

use nessa_protocol::product_contract::connection_outcome;
use nessa_protocol::product_contract::generated::SessionCloseReason;
#[test]
fn no_issued_record_or_closed_parent_opens_no_product() {
    let root = PrivateFixture::new();
    create_directory_beneath(root.path(), Path::new("private")).unwrap();
    let connector = Arc::new(CountingConnector {
        calls: AtomicUsize::new(0),
        delegate: Arc::new(RefusingProduct),
    });
    let parent = parent(
        root.path(),
        "127.0.0.1:9".parse().unwrap(),
        connector.clone(),
        ProductLimits::default(),
    );
    assert!(matches!(
        parent.open_product("phone-instance".into()),
        Err(SessionFailure::NoCredential)
    ));
    parent.request_close().unwrap();
    assert!(matches!(
        parent.open_product("phone-instance".into()),
        Err(SessionFailure::Closed)
    ));
    parent.close().unwrap();
    assert_eq!(connector.calls.load(Ordering::SeqCst), 0);
    drop(FilePairingState::open(root.path(), Path::new("private")).unwrap());
}

#[test]
fn parent_product_failure_preserves_original_optional_close_reason() {
    let failure = crate::failures::NativeSessionFailure {
        value: SessionFailure::Product(ProductFailure::Closed(None)),
    };
    assert_eq!(
        failure.product().unwrap().kind(),
        NativeProductFailureKind::Closed
    );
    assert!(failure.product().unwrap().close_reason().is_none());
    let failure = crate::failures::NativeSessionFailure {
        value: SessionFailure::Product(ProductFailure::Authentication(
            SessionCloseReason::CredentialRevoked,
        )),
    };
    assert_eq!(
        failure.product().unwrap().kind(),
        NativeProductFailureKind::Authentication
    );
    assert_eq!(
        failure.product().unwrap().close_reason(),
        Some(NativeProductCloseReason::CredentialRevoked)
    );
}

struct Release(Arc<Held>);
impl Drop for Release {
    fn drop(&mut self) {
        self.0.release();
    }
}
struct CountingConnector {
    calls: AtomicUsize,
    delegate: Arc<dyn ProductConnector>,
}
impl ProductConnector for CountingConnector {
    fn connect(&self, address: SocketAddr, timeout: Duration) -> IoResult<ConnectedProduct> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.delegate.connect(address, timeout)
    }
}
struct RefusingProduct;
impl ProductConnector for RefusingProduct {
    fn connect(&self, _: SocketAddr, _: Duration) -> IoResult<ConnectedProduct> {
        Err(IoError::from(ErrorKind::ConnectionRefused))
    }
}
fn compose_product(
    root: &Path,
    address: SocketAddr,
    connector: Arc<dyn ProductConnector>,
    clock: Arc<dyn Clock>,
    wake: Arc<dyn ProductWake>,
    limits: ProductLimits,
) -> NativeSession {
    NativeSession::compose(Dependencies {
        state: Arc::new(FilePairingState::open(root, Path::new("private")).unwrap()),
        path: root.join("private/cache.sqlite"),
        address,
        connector: Arc::new(SocketConnector),
        entropy: Arc::new(OsEntropyFactory),
        files: Arc::new(OsCacheFiles),
        artifacts: Arc::new(DiskArtifactFiles),
        wall: Arc::new(Wall),
        clock,
        pause: Arc::new(ThreadPause),
        runtime: Builder::new_current_thread().enable_all().build().unwrap(),
        product: ProductDependencies {
            connector,
            wake,
            limits,
        },
    })
    .unwrap()
}
fn parent(
    root: &Path,
    address: SocketAddr,
    connector: Arc<dyn ProductConnector>,
    limits: ProductLimits,
) -> NativeSession {
    compose_product(
        root,
        address,
        connector,
        Arc::new(Monotonic::new()),
        Arc::new(ThreadProductWake::default()),
        limits,
    )
}
fn child(parent: &NativeSession) -> Arc<NativeProductSession> {
    let child = Arc::new(NativeProductSession {
        value: parent.open_product("phone-instance".into()).unwrap(),
    });
    child.wait_ready().unwrap();
    child
}
fn failure(error: NativeProductCallError) -> crate::NativeProductFailureKind {
    let NativeProductCallError::Product { failure } = error;
    failure.kind()
}
fn wait_failure(child: &NativeProductSession) -> crate::NativeProductFailureKind {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(failure) = child.failure() {
            return failure.kind();
        }
        assert!(Instant::now() < deadline, "native failure captured");
        thread::yield_now();
    }
}
fn recv<T>(receiver: &mpsc::Receiver<T>) -> T {
    receiver.recv_timeout(Duration::from_secs(3)).unwrap()
}

#[test]
fn foreign_last_parent_drop_closes_child_and_preserves_original_ready_response_event() {
    let root = fixtures::root(true);
    let (address, peer) = fixtures::peer(|wire| {
        let request = wire.request().unwrap();
        assert_eq!(request.id, "1");
        assert_eq!(request.params, json!({"original": [null, "request"]}));
        wire.send(OutgoingMessage::Event(
            EventFrame::push(
                "conversation.listed",
                &json!({"original":null}),
                u64::MAX,
                u64::MAX - 1,
            )
            .unwrap(),
        ));
        wire.send(OutgoingMessage::Response(
            ResponseFrame::failure_with_details(
                &request.id,
                "original-code",
                "original-message",
                Some(json!({"original": [null, {"nested":"detail"}]})),
            ),
        ));
        assert!(
            wire.request().is_none(),
            "parent Drop stopped original stream"
        );
    });
    let parent = NativeDeviceSession::open(
        root.path().to_str().unwrap().into(),
        "private".into(),
        address.to_string(),
        None,
    )
    .unwrap();
    let child = parent.open_product("phone-instance".into()).unwrap();
    let ready = child.wait_ready().unwrap();
    assert_eq!(ready.credential_id(), "issued");
    assert_eq!(ready.methods(), vec!["conversation.list"]);
    let ticket = child
        .request(
            "conversation.list".into(),
            "{\"original\":[null,\"request\"]}".into(),
        )
        .unwrap();
    let completion = ticket.wait();
    let batch = child.events(3_000).unwrap();
    drop(ticket);
    drop(parent);
    peer.join();
    assert_eq!(completion.id(), 1);
    assert_eq!(completion.kind(), NativeProductCompletionKind::Response);
    assert!(completion.failure().is_none());
    let response: Value =
        serde_json::from_str(&completion.response_json().unwrap().unwrap()).unwrap();
    assert_eq!(response["ok"], false);
    assert_eq!(response["error"]["code"], "original-code");
    assert_eq!(response["error"]["message"], "original-message");
    assert_eq!(
        response["error"]["details"],
        json!({"original":[null,{"nested":"detail"}]})
    );
    let events = batch.frames_json().unwrap();
    assert_eq!(events.len(), 1);
    let event: Value = serde_json::from_str(&events[0]).unwrap();
    assert_eq!(event["seq"].as_u64(), Some(u64::MAX));
    assert_eq!(event["stateVersion"].as_u64(), Some(u64::MAX - 1));
    assert_eq!(event["payload"], json!({"original":null}));
    assert_eq!(
        ready.json().unwrap(),
        child.ready().unwrap().json().unwrap()
    );
    assert_eq!(
        failure(
            child
                .request("conversation.list".into(), "{}".into())
                .err()
                .unwrap()
        ),
        NativeProductFailureKind::Cancelled
    );
    drop(FilePairingState::open(root.path(), Path::new("private")).unwrap());
}

#[test]
fn foreign_completion_handle_holds_ticket_capacity_until_final_release() {
    let root = fixtures::root(true);
    let (address, peer) = fixtures::peer(|wire| {
        for id in ["1", "2"] {
            let request = wire.request().unwrap();
            assert_eq!(request.id, id);
            wire.send(OutgoingMessage::Response(
                ResponseFrame::success(id, &json!(null)).unwrap(),
            ));
        }
        assert!(wire.request().is_none());
    });
    let parent = parent(
        root.path(),
        address,
        Arc::new(LocalProductConnector::default()),
        ProductLimits::new(1, 2, 2 * MAX_PROTECTED_RESPONSE_BYTES, 5, 2_000, 2_000, 500).unwrap(),
    );
    let child = child(&parent);
    let first = child
        .request("conversation.list".into(), "{}".into())
        .unwrap();
    let completion = first.wait();
    let alias = completion.clone();
    drop(first);
    drop(completion);
    assert_eq!(
        failure(
            child
                .request("conversation.list".into(), "{}".into())
                .err()
                .unwrap()
        ),
        NativeProductFailureKind::Capacity
    );
    drop(alias);
    let next = child
        .request("conversation.list".into(), "{}".into())
        .unwrap();
    assert_eq!(next.id(), 2);
    let done = next.wait();
    assert_eq!(done.kind(), NativeProductCompletionKind::Response);
    assert_eq!(
        serde_json::from_str::<Value>(&done.response_json().unwrap().unwrap()).unwrap()["payload"],
        Value::Null
    );
    parent.close().unwrap();
    peer.join();
}

#[test]
fn escaped_foreign_ticket_drop_does_not_release_inflight_capacity() {
    let root = fixtures::root(true);
    let gate = Held::new();
    let _release = Release(gate.clone());
    let peer_gate = gate.clone();
    let (address, peer) = fixtures::peer(move |wire| {
        let first = wire.request().unwrap();
        assert_eq!(first.id, "1");
        peer_gate.hold();
        wire.send(OutgoingMessage::Response(
            ResponseFrame::success("1", &json!({"late":true})).unwrap(),
        ));
        let next = wire.request().unwrap();
        assert_eq!(next.id, "2");
        wire.send(OutgoingMessage::Response(
            ResponseFrame::success("2", &json!({})).unwrap(),
        ));
        assert!(wire.request().is_none());
    });
    let parent = parent(
        root.path(),
        address,
        Arc::new(LocalProductConnector::default()),
        ProductLimits::new(1, 2, 2 * MAX_PROTECTED_RESPONSE_BYTES, 5, 2_000, 2_000, 500).unwrap(),
    );
    let child = child(&parent);
    let first = child
        .request("conversation.list".into(), "{}".into())
        .unwrap();
    gate.entered();
    drop(first);
    assert_eq!(
        failure(
            child
                .request("conversation.list".into(), "{}".into())
                .err()
                .unwrap()
        ),
        NativeProductFailureKind::Capacity
    );
    gate.release();
    let deadline = Instant::now() + Duration::from_secs(3);
    let second = loop {
        match child.request("conversation.list".into(), "{}".into()) {
            Ok(ticket) => break ticket,
            Err(error) => {
                assert_eq!(failure(error), NativeProductFailureKind::Capacity);
                assert!(Instant::now() < deadline);
                thread::yield_now();
            }
        }
    };
    assert_eq!(second.id(), 2);
    second.wait();
    parent.close().unwrap();
    peer.join();
}

#[test]
fn foreign_delivered_event_handle_retains_count_and_byte_capacity() {
    for byte_limited in [false, true] {
        let root = fixtures::root(true);
        let event =
            EventFrame::push("conversation.listed", &json!({"value":null}), u64::MAX, 1).unwrap();
        let text_len = OutgoingMessage::Event(event.clone())
            .to_wire_text()
            .unwrap()
            .len();
        let (sent, delivered) = mpsc::channel();
        let (address, peer) = fixtures::peer(move |wire| {
            wire.send(OutgoingMessage::Event(event.clone()));
            recv(&delivered);
            wire.send(OutgoingMessage::Event(event));
            assert!(wire.request().is_none());
        });
        let parent = parent(
            root.path(),
            address,
            Arc::new(LocalProductConnector::default()),
            ProductLimits::new(
                2,
                if byte_limited { 8 } else { 1 },
                if byte_limited {
                    text_len
                } else {
                    MAX_PROTECTED_RESPONSE_BYTES
                },
                5,
                2_000,
                2_000,
                500,
            )
            .unwrap(),
        );
        let child = child(&parent);
        let batch = child.events(3_000).unwrap();
        let alias = batch.clone();
        assert_eq!(batch.frames_json().unwrap().len(), 1);
        drop(batch);
        sent.send(()).unwrap();
        assert_eq!(wait_failure(&child), NativeProductFailureKind::Capacity);
        assert_eq!(
            failure(child.close().unwrap_err()),
            NativeProductFailureKind::Capacity
        );
        assert!(matches!(
            parent.close(),
            Err(SessionFailure::Product(ProductFailure::Capacity))
        ));
        assert_eq!(alias.frames_json().unwrap().len(), 1);
        drop(alias);
        peer.join();
        drop(FilePairingState::open(root.path(), Path::new("private")).unwrap());
    }
}

#[test]
fn parent_close_and_repeated_close_keep_original_child_cause_after_physical_release() {
    let root = fixtures::root(true);
    let gate = Held::new();
    let (address, peer) = fixtures::peer(|wire| {
        wire.request().unwrap();
        wire.send(OutgoingMessage::Response(
            ResponseFrame::success("wrong-id", &json!({})).unwrap(),
        ));
        assert!(wire.request().is_none());
    });
    let parent = Arc::new(parent(
        root.path(),
        address,
        Arc::new(CleanupConnector { gate: gate.clone() }),
        ProductLimits::default(),
    ));
    let _release = Release(gate.clone());
    let child = child(&parent);
    let ticket = child
        .request("conversation.list".into(), "{}".into())
        .unwrap();
    gate.entered();
    assert_eq!(
        child.failure().unwrap().kind(),
        NativeProductFailureKind::Correlation
    );
    assert!(
        ticket.completion().is_none(),
        "physical cleanup remains unsettled"
    );
    parent.request_close().unwrap();
    let (sent, received) = mpsc::channel();
    let first = {
        let parent = parent.clone();
        let sent = sent.clone();
        thread::spawn(move || {
            let value = parent.close_observed();
            sent.send(value).unwrap();
        })
    };
    let second = {
        let parent = parent.clone();
        thread::spawn(move || {
            sent.send(parent.close_observed()).unwrap();
        })
    };
    assert!(received.recv_timeout(Duration::from_millis(40)).is_err());
    assert!(matches!(
        FilePairingState::open(root.path(), Path::new("private")),
        Err(PrivateStateError::Locked)
    ));
    gate.release();
    for _ in 0..2 {
        let observation = recv(&received);
        assert!(observation.released());
        assert_eq!(
            observation.outcome(),
            &Err(SessionFailure::Product(ProductFailure::Correlation))
        );
    }
    first.join().unwrap();
    second.join().unwrap();
    assert_eq!(
        parent.close(),
        Err(SessionFailure::Product(ProductFailure::Correlation))
    );
    let completion = ticket.wait();
    assert_eq!(completion.kind(), NativeProductCompletionKind::Uncertain);
    assert_eq!(
        completion.failure().unwrap().kind(),
        NativeProductFailureKind::Correlation
    );
    assert!(completion.response_json().unwrap().is_none());
    drop(FilePairingState::open(root.path(), Path::new("private")).unwrap());
    peer.join();
}

#[test]
fn provisional_child_and_independent_stop_cover_held_connector_without_driver_lock() {
    let root = fixtures::root(true);
    let gate = Held::new();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let raw_peer = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut byte = [0];
        assert_eq!(
            socket.read(&mut byte).unwrap(),
            0,
            "stopped provisional child sent no TLS"
        );
    });
    let parent = Arc::new(parent(
        root.path(),
        address,
        Arc::new(HeldConnector { gate: gate.clone() }),
        ProductLimits::default(),
    ));
    let _release = Release(gate.clone());
    let child = Arc::new(NativeProductSession {
        value: parent.open_product("phone-instance".into()).unwrap(),
    });
    gate.entered();
    assert_eq!(
        parent
            .local_enrollment()
            .unwrap()
            .unwrap()
            .issued()
            .unwrap()
            .credential()
            .as_str(),
        "issued"
    );
    parent.request_close().unwrap();
    let (sent, received) = mpsc::channel();
    let close = {
        let parent = parent.clone();
        thread::spawn(move || {
            sent.send(parent.close()).unwrap();
        })
    };
    assert!(received.recv_timeout(Duration::from_millis(40)).is_err());
    assert!(matches!(
        FilePairingState::open(root.path(), Path::new("private")),
        Err(PrivateStateError::Locked)
    ));
    gate.release();
    recv(&received).unwrap();
    close.join().unwrap();
    raw_peer.join().unwrap();
    assert_eq!(
        failure(child.wait_ready().err().unwrap()),
        NativeProductFailureKind::Cancelled
    );
    drop(FilePairingState::open(root.path(), Path::new("private")).unwrap());
}

#[test]
fn replacement_keeps_old_child_registered_until_join_and_closure_refuses_second_dial() {
    let root = fixtures::root(true);
    let gate = Held::new();
    let (address, peer) = fixtures::peer(|wire| {
        assert!(wire.request().is_none());
    });
    let connector = Arc::new(CountingConnector {
        calls: AtomicUsize::new(0),
        delegate: Arc::new(CleanupConnector { gate: gate.clone() }),
    });
    let parent = Arc::new(parent(
        root.path(),
        address,
        connector.clone(),
        ProductLimits::default(),
    ));
    let _release = Release(gate.clone());
    let old = child(&parent);
    let replace = {
        let parent = parent.clone();
        thread::spawn(move || parent.open_product("replacement".into()))
    };
    gate.entered();
    parent.request_close().unwrap();
    assert!(matches!(
        parent.local_enrollment(),
        Err(SessionFailure::Busy)
    ));
    assert_eq!(connector.calls.load(Ordering::SeqCst), 1);
    gate.release();
    assert!(matches!(
        replace.join().unwrap(),
        Err(SessionFailure::Closed)
    ));
    parent.close().unwrap();
    assert_eq!(connector.calls.load(Ordering::SeqCst), 1);
    assert_eq!(old.ready().unwrap().credential_id(), "issued");
    assert_eq!(
        failure(
            old.request("conversation.list".into(), "{}".into())
                .err()
                .unwrap()
        ),
        NativeProductFailureKind::Cancelled
    );
    peer.join();
}

struct HeldStore {
    owned: FilePairingState,
    gate: Arc<Held>,
}
impl ClientPendingStore for HeldStore {
    fn load_pending(&self) -> Result<Option<PendingEnrollment>, PrivateStateError> {
        self.gate.hold();
        self.owned.load_pending()
    }
    fn load_credential(&self) -> Result<Option<DeviceCredential>, PrivateStateError> {
        self.owned.load_credential()
    }
    fn save_pending(
        &self,
        key: &PrivateKeyMaterial,
        pin: &[u8; 44],
        intent: PublicIntent,
        replaces: Option<PublicIntent>,
    ) -> Result<(), PrivateStateError> {
        self.owned.save_pending(key, pin, intent, replaces)
    }
    fn save_credential(
        &self,
        credential: &CredentialId,
        receiver: &ResourceId,
        intent: PublicIntent,
    ) -> Result<(), PrivateStateError> {
        self.owned.save_credential(credential, receiver, intent)
    }
    fn end_enrollment(&self, intent: PublicIntent) -> Result<(), PrivateStateError> {
        self.owned.end_enrollment(intent)
    }
}
#[test]
fn close_during_issued_load_fences_provisional_installation_and_preserves_private_owner() {
    let root = fixtures::root(true);
    let gate = Held::new();
    let network = Arc::new(CountingConnector {
        calls: AtomicUsize::new(0),
        delegate: Arc::new(RefusingProduct),
    });
    let parent = Arc::new(
        NativeSession::compose(Dependencies {
            state: Arc::new(HeldStore {
                owned: FilePairingState::open(root.path(), Path::new("private")).unwrap(),
                gate: gate.clone(),
            }),
            path: root.path().join("private/cache.sqlite"),
            address: "127.0.0.1:9".parse().unwrap(),
            connector: Arc::new(SocketConnector),
            entropy: Arc::new(OsEntropyFactory),
            files: Arc::new(OsCacheFiles),
            artifacts: Arc::new(DiskArtifactFiles),
            wall: Arc::new(Wall),
            clock: Arc::new(Monotonic::new()),
            pause: Arc::new(ThreadPause),
            runtime: Builder::new_current_thread().enable_all().build().unwrap(),
            product: ProductDependencies {
                connector: network.clone(),
                wake: Arc::new(ThreadProductWake::default()),
                limits: ProductLimits::default(),
            },
        })
        .unwrap(),
    );
    let _release = Release(gate.clone());
    let opening = {
        let parent = parent.clone();
        thread::spawn(move || parent.open_product("phone-instance".into()))
    };
    gate.entered();
    parent.request_close().unwrap();
    let (sent, received) = mpsc::channel();
    let close = {
        let parent = parent.clone();
        thread::spawn(move || {
            sent.send(parent.close()).unwrap();
        })
    };
    assert!(received.recv_timeout(Duration::from_millis(40)).is_err());
    assert!(matches!(
        FilePairingState::open(root.path(), Path::new("private")),
        Err(PrivateStateError::Locked)
    ));
    gate.release();
    assert!(matches!(
        opening.join().unwrap(),
        Err(SessionFailure::Closed)
    ));
    recv(&received).unwrap();
    close.join().unwrap();
    assert_eq!(network.calls.load(Ordering::SeqCst), 0);
    drop(FilePairingState::open(root.path(), Path::new("private")).unwrap());
}

struct ManualClock {
    value: AtomicU64,
    observed: mpsc::Sender<u64>,
}
impl Clock for ManualClock {
    fn elapsed_ms(&self) -> u64 {
        let value = self.value.load(Ordering::SeqCst);
        if thread::current().name() == Some("foreign-event-waiter") {
            let _ = self.observed.send(value);
        }
        value
    }
}
#[test]
fn foreign_relative_event_timeout_uses_one_original_native_deadline_and_checks_overflow() {
    let root = fixtures::root(true);
    let (address, peer) = fixtures::peer(|wire| {
        assert!(wire.request().is_none());
    });
    let (observed, clock_reads) = mpsc::channel();
    let clock = Arc::new(ManualClock {
        value: AtomicU64::new(10),
        observed,
    });
    let wake = Arc::new(ThreadProductWake::default());
    let parent = compose_product(
        root.path(),
        address,
        Arc::new(LocalProductConnector::default()),
        clock.clone(),
        wake.clone(),
        ProductLimits::default(),
    );
    let child = child(&parent);
    assert!(child.events(0).unwrap().frames_json().unwrap().is_empty());
    // A prepared, non-running child isolates range refusal from the live pump's
    // independent absolute deadlines; advancing its clock cannot corrupt the TLS fixture.
    let (observed, _) = mpsc::channel();
    let overflow_clock = Arc::new(ManualClock {
        value: AtomicU64::new(u64::MAX),
        observed,
    });
    let overflow = NativeProductSession {
        value: SessionProduct {
            value: ProductSession::prepare(
                "127.0.0.1:9".parse().unwrap(),
                Arc::new(fixtures::identity(7)),
                fixtures::identity(8).public_spki(),
                "issued".into(),
                "phone-instance".into(),
                Arc::new(RefusingProduct),
                overflow_clock,
                Arc::new(ThreadProductWake::default()),
                ProductLimits::default(),
            ),
        },
    };
    assert_eq!(
        failure(overflow.events(1).err().unwrap()),
        NativeProductFailureKind::InvalidLimits
    );
    clock.value.store(100, Ordering::SeqCst);
    let (sent, received) = mpsc::channel();
    let waiting = {
        let child = child.clone();
        thread::Builder::new()
            .name("foreign-event-waiter".into())
            .spawn(move || {
                sent.send(child.events(50)).unwrap();
            })
            .unwrap()
    };
    assert_eq!(
        recv(&clock_reads),
        100,
        "relative boundary captured original clock"
    );
    clock.value.store(149, Ordering::SeqCst);
    wake.wake();
    while recv(&clock_reads) != 149 {}
    assert!(received.recv_timeout(Duration::from_millis(20)).is_err());
    clock.value.store(150, Ordering::SeqCst);
    wake.wake();
    assert!(recv(&received).unwrap().frames_json().unwrap().is_empty());
    waiting.join().unwrap();
    parent.close().unwrap();
    peer.join();
}

#[test]
fn parent_last_owner_drop_waits_for_child_cleanup_with_escaped_foreign_facts() {
    let root = fixtures::root(true);
    let gate = Held::new();
    let (address, peer) = fixtures::peer(|wire| {
        let request = wire.request().unwrap();
        wire.send(OutgoingMessage::Event(
            EventFrame::push("conversation.listed", &json!({"saved":true}), u64::MAX, 3).unwrap(),
        ));
        wire.send(OutgoingMessage::Response(
            ResponseFrame::success(&request.id, &json!({"lateFact":true})).unwrap(),
        ));
        assert!(wire.request().is_none());
    });
    let parent = parent(
        root.path(),
        address,
        Arc::new(CleanupConnector { gate: gate.clone() }),
        ProductLimits::default(),
    );
    let _release = Release(gate.clone());
    let child = child(&parent);
    let ticket = child
        .request("conversation.list".into(), "{}".into())
        .unwrap();
    let completion = ticket.wait();
    let batch = child.events(3_000).unwrap();
    drop(ticket);
    let (sent, received) = mpsc::channel();
    let drop_parent = thread::spawn(move || {
        drop(parent);
        sent.send(()).unwrap();
    });
    gate.entered();
    assert!(received.recv_timeout(Duration::from_millis(40)).is_err());
    assert!(matches!(
        FilePairingState::open(root.path(), Path::new("private")),
        Err(PrivateStateError::Locked)
    ));
    assert_eq!(
        failure(
            child
                .request("conversation.list".into(), "{}".into())
                .err()
                .unwrap()
        ),
        NativeProductFailureKind::Cancelled
    );
    gate.release();
    recv(&received);
    drop_parent.join().unwrap();
    assert_eq!(completion.kind(), NativeProductCompletionKind::Response);
    assert_eq!(
        serde_json::from_str::<Value>(&completion.response_json().unwrap().unwrap()).unwrap()
            ["payload"],
        json!({"lateFact":true})
    );
    assert_eq!(batch.frames_json().unwrap().len(), 1);
    drop(FilePairingState::open(root.path(), Path::new("private")).unwrap());
    peer.join();
}

#[test]
fn all_original_product_failure_variants_and_close_reason_payloads_survive_foreign_mapping() {
    for (source, kind) in [
        (
            ProductFailure::InvalidLimits,
            NativeProductFailureKind::InvalidLimits,
        ),
        (
            ProductFailure::InvalidRequest,
            NativeProductFailureKind::InvalidRequest,
        ),
        (
            ProductFailure::UnadvertisedMethod,
            NativeProductFailureKind::UnadvertisedMethod,
        ),
        (ProductFailure::Capacity, NativeProductFailureKind::Capacity),
        (
            ProductFailure::RequestTooLarge,
            NativeProductFailureKind::RequestTooLarge,
        ),
        (
            ProductFailure::ResponseTooLarge,
            NativeProductFailureKind::ResponseTooLarge,
        ),
        (ProductFailure::TimedOut, NativeProductFailureKind::TimedOut),
        (
            ProductFailure::Cancelled,
            NativeProductFailureKind::Cancelled,
        ),
        (
            ProductFailure::Transport,
            NativeProductFailureKind::Transport,
        ),
        (
            ProductFailure::Closed(None),
            NativeProductFailureKind::Closed,
        ),
        (
            ProductFailure::NativeHandshake,
            NativeProductFailureKind::NativeHandshake,
        ),
        (
            ProductFailure::ProductRefused,
            NativeProductFailureKind::ProductRefused,
        ),
        (ProductFailure::Protocol, NativeProductFailureKind::Protocol),
        (
            ProductFailure::Correlation,
            NativeProductFailureKind::Correlation,
        ),
        (
            ProductFailure::WorkerPanicked,
            NativeProductFailureKind::WorkerPanicked,
        ),
        (ProductFailure::NotReady, NativeProductFailureKind::NotReady),
    ] {
        let failure = NativeProductFailure { value: source };
        assert_eq!(failure.kind(), kind);
        assert!(failure.close_reason().is_none());
        let class = match source {
            ProductFailure::Closed(None) => Some(NativeConnectionOutcome::Retryable),
            _ => None,
        };
        assert_eq!(failure.outcome(), class);
    }
    for (reason, expected) in [
        (
            SessionCloseReason::AuthenticationFailed,
            NativeProductCloseReason::AuthenticationFailed,
        ),
        (
            SessionCloseReason::CredentialRevoked,
            NativeProductCloseReason::CredentialRevoked,
        ),
        (
            SessionCloseReason::CredentialExpired,
            NativeProductCloseReason::CredentialExpired,
        ),
        (
            SessionCloseReason::AuthorizationLost,
            NativeProductCloseReason::AuthorizationLost,
        ),
        (
            SessionCloseReason::ProtocolIncompatible,
            NativeProductCloseReason::ProtocolIncompatible,
        ),
        (
            SessionCloseReason::HandshakeTimeout,
            NativeProductCloseReason::HandshakeTimeout,
        ),
        (
            SessionCloseReason::TemporaryUnavailable,
            NativeProductCloseReason::TemporaryUnavailable,
        ),
        (
            SessionCloseReason::GatewayRestarting,
            NativeProductCloseReason::GatewayRestarting,
        ),
        (
            SessionCloseReason::GatewayOverloaded,
            NativeProductCloseReason::GatewayOverloaded,
        ),
        (
            SessionCloseReason::ServerShutdown,
            NativeProductCloseReason::ServerShutdown,
        ),
        (
            SessionCloseReason::TransportInterrupted,
            NativeProductCloseReason::TransportInterrupted,
        ),
    ] {
        let closed = NativeProductFailure {
            value: ProductFailure::Closed(Some(reason)),
        };
        let auth = NativeProductFailure {
            value: ProductFailure::Authentication(reason),
        };
        assert_eq!(closed.kind(), NativeProductFailureKind::Closed);
        assert_eq!(auth.kind(), NativeProductFailureKind::Authentication);
        assert_eq!(closed.close_reason(), Some(expected));
        assert_eq!(auth.close_reason(), Some(expected));
        let class = NativeConnectionOutcome::from(connection_outcome(Some(reason)));
        assert_eq!(closed.outcome(), Some(class));
        assert_eq!(auth.outcome(), Some(class));
        let parent = crate::NativeSessionFailure {
            value: SessionFailure::Product(ProductFailure::Authentication(reason)),
        };
        assert_eq!(parent.product().unwrap().close_reason(), Some(expected));
    }
}

struct PoisonableStore {
    owned: FilePairingState,
    panic_on_read: Arc<std::sync::atomic::AtomicBool>,
}
impl ClientPendingStore for PoisonableStore {
    fn load_pending(&self) -> Result<Option<PendingEnrollment>, PrivateStateError> {
        assert!(
            !self.panic_on_read.load(Ordering::SeqCst),
            "injected admitted parent store panic"
        );
        self.owned.load_pending()
    }
    fn load_credential(&self) -> Result<Option<DeviceCredential>, PrivateStateError> {
        self.owned.load_credential()
    }
    fn save_pending(
        &self,
        key: &PrivateKeyMaterial,
        pin: &[u8; 44],
        intent: PublicIntent,
        replaces: Option<PublicIntent>,
    ) -> Result<(), PrivateStateError> {
        self.owned.save_pending(key, pin, intent, replaces)
    }
    fn save_credential(
        &self,
        credential: &CredentialId,
        receiver: &ResourceId,
        intent: PublicIntent,
    ) -> Result<(), PrivateStateError> {
        self.owned.save_credential(credential, receiver, intent)
    }
    fn end_enrollment(&self, intent: PublicIntent) -> Result<(), PrivateStateError> {
        self.owned.end_enrollment(intent)
    }
}
#[test]
fn poisoned_parent_drain_preserves_distinct_faults_and_joins_before_private_release() {
    for raw_drop in [false, true] {
        let root = fixtures::root(true);
        let gate = Held::new();
        let panic_on_read = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (address, peer) = fixtures::peer(|wire| {
            wire.request().unwrap();
            wire.send(OutgoingMessage::Response(
                ResponseFrame::success("wrong-id", &json!({})).unwrap(),
            ));
            assert!(wire.request().is_none());
        });
        let parent = NativeSession::compose(Dependencies {
            state: Arc::new(PoisonableStore {
                owned: FilePairingState::open(root.path(), Path::new("private")).unwrap(),
                panic_on_read: panic_on_read.clone(),
            }),
            path: root.path().join("private/cache.sqlite"),
            address,
            connector: Arc::new(SocketConnector),
            entropy: Arc::new(OsEntropyFactory),
            files: Arc::new(OsCacheFiles),
            artifacts: Arc::new(DiskArtifactFiles),
            wall: Arc::new(Wall),
            clock: Arc::new(Monotonic::new()),
            pause: Arc::new(ThreadPause),
            runtime: Builder::new_current_thread().enable_all().build().unwrap(),
            product: ProductDependencies {
                connector: Arc::new(CleanupConnector { gate: gate.clone() }),
                wake: Arc::new(ThreadProductWake::default()),
                limits: ProductLimits::default(),
            },
        })
        .unwrap();
        let child = child(&parent);
        let ticket = child
            .request("conversation.list".into(), "{}".into())
            .unwrap();
        gate.entered();
        assert_eq!(
            child.failure().unwrap().kind(),
            NativeProductFailureKind::Correlation
        );
        panic_on_read.store(true, Ordering::SeqCst);
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(
            || parent.local_enrollment()
        ))
        .is_err());
        assert!(matches!(
            parent.open_product("phone-instance".into()),
            Err(SessionFailure::Damaged)
        ));
        let (sent, received) = mpsc::channel();
        let drain = thread::spawn(move || {
            if raw_drop {
                drop(parent);
                sent.send(None).unwrap();
            } else {
                let result = parent.close_observed();
                sent.send(Some((parent, result))).unwrap();
            }
        });
        // Release before escaped handles/queued parent values unwind on a failed
        // assertion, so the deliberately broken teardown probe cannot self-deadlock.
        let _release = Release(gate.clone());
        assert!(received.recv_timeout(Duration::from_millis(40)).is_err());
        assert!(matches!(
            FilePairingState::open(root.path(), Path::new("private")),
            Err(PrivateStateError::Locked)
        ));
        gate.release();
        if let Some((parent, result)) = recv(&received) {
            assert!(result.released());
            assert_eq!(result.outcome(), &Err(SessionFailure::Damaged));
            assert_eq!(parent.close(), Err(SessionFailure::Damaged));
            assert!(matches!(
                parent.open_product("phone-instance".into()),
                Err(SessionFailure::Damaged)
            ));
        }
        drain.join().unwrap();
        let completion = ticket.wait();
        assert_eq!(completion.kind(), NativeProductCompletionKind::Uncertain);
        assert_eq!(
            completion.failure().unwrap().kind(),
            NativeProductFailureKind::Correlation
        );
        assert_eq!(
            child.failure().unwrap().kind(),
            NativeProductFailureKind::Correlation
        );
        drop(FilePairingState::open(root.path(), Path::new("private")).unwrap());
        peer.join();
    }
}

struct FailedStore {
    owned: FilePairingState,
    error: Option<PrivateStateError>,
}
impl ClientPendingStore for FailedStore {
    fn load_pending(&self) -> Result<Option<PendingEnrollment>, PrivateStateError> {
        match self.error {
            Some(error) => Err(error),
            None => self
                .owned
                .load_credential()
                .map(|issued| issued.map(DeviceCredential::into_enrollment)),
        }
    }
    fn load_credential(&self) -> Result<Option<DeviceCredential>, PrivateStateError> {
        self.owned.load_credential()
    }
    fn save_pending(
        &self,
        _: &PrivateKeyMaterial,
        _: &[u8; 44],
        _: PublicIntent,
        _: Option<PublicIntent>,
    ) -> Result<(), PrivateStateError> {
        panic!("unexpected write")
    }
    fn save_credential(
        &self,
        _: &CredentialId,
        _: &ResourceId,
        _: PublicIntent,
    ) -> Result<(), PrivateStateError> {
        panic!("unexpected write")
    }
    fn end_enrollment(&self, _: PublicIntent) -> Result<(), PrivateStateError> {
        panic!("unexpected removal")
    }
}
#[test]
fn exact_private_failure_conflicting_records_and_pending_only_refuse_without_connect() {
    for error in [
        Some(PrivateStateError::Corrupt),
        Some(PrivateStateError::Uncertain),
        None,
    ] {
        let root = fixtures::root(true);
        let connector = Arc::new(CountingConnector {
            calls: AtomicUsize::new(0),
            delegate: Arc::new(RefusingProduct),
        });
        let parent = NativeSession::compose(Dependencies {
            state: Arc::new(FailedStore {
                owned: FilePairingState::open(root.path(), Path::new("private")).unwrap(),
                error,
            }),
            path: root.path().join("private/cache.sqlite"),
            address: "127.0.0.1:9".parse().unwrap(),
            connector: Arc::new(SocketConnector),
            entropy: Arc::new(OsEntropyFactory),
            files: Arc::new(OsCacheFiles),
            artifacts: Arc::new(DiskArtifactFiles),
            wall: Arc::new(Wall),
            clock: Arc::new(Monotonic::new()),
            pause: Arc::new(ThreadPause),
            runtime: Builder::new_current_thread().enable_all().build().unwrap(),
            product: ProductDependencies {
                connector: connector.clone(),
                wake: Arc::new(ThreadProductWake::default()),
                limits: ProductLimits::default(),
            },
        })
        .unwrap();
        assert!(
            matches!(parent.open_product("phone-instance".into()),Err(SessionFailure::PrivateState(value)) if value==error.unwrap_or(PrivateStateError::Conflict))
        );
        assert_eq!(connector.calls.load(Ordering::SeqCst), 0);
        parent.close().unwrap();
    }
    let root = fixtures::root(false);
    let connector = Arc::new(CountingConnector {
        calls: AtomicUsize::new(0),
        delegate: Arc::new(RefusingProduct),
    });
    let parent = parent(
        root.path(),
        "127.0.0.1:9".parse().unwrap(),
        connector.clone(),
        ProductLimits::default(),
    );
    assert!(matches!(
        parent.open_product("phone-instance".into()),
        Err(SessionFailure::NoCredential)
    ));
    assert_eq!(connector.calls.load(Ordering::SeqCst), 0);
    parent.close().unwrap();
}

struct RefusingEnrollment(AtomicUsize);
impl EnrollmentConnector for RefusingEnrollment {
    fn connect(&self, _: SocketAddr) -> Result<TcpStream, NativeClientError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(NativeClientError::Io(ErrorKind::ConnectionRefused))
    }
}
#[test]
fn retained_refresh_admits_while_foreign_event_waiter_owns_its_original_child() {
    let root = fixtures::root(true);
    let (address, peer) = fixtures::peer(|wire| {
        assert!(wire.request().is_none());
    });
    let network = Arc::new(RefusingEnrollment(AtomicUsize::new(0)));
    let (observed, reads) = mpsc::channel();
    let clock = Arc::new(ManualClock {
        value: AtomicU64::new(10),
        observed,
    });
    let wake = Arc::new(ThreadProductWake::default());
    let parent = NativeSession::compose(Dependencies {
        state: Arc::new(FilePairingState::open(root.path(), Path::new("private")).unwrap()),
        path: root.path().join("private/cache.sqlite"),
        address,
        connector: network.clone(),
        entropy: Arc::new(OsEntropyFactory),
        files: Arc::new(OsCacheFiles),
        artifacts: Arc::new(DiskArtifactFiles),
        wall: Arc::new(Wall),
        clock: clock.clone(),
        pause: Arc::new(ThreadPause),
        runtime: Builder::new_current_thread().enable_all().build().unwrap(),
        product: ProductDependencies {
            connector: Arc::new(LocalProductConnector::default()),
            wake: wake.clone(),
            limits: ProductLimits::default(),
        },
    })
    .unwrap();
    let child = child(&parent);
    let waiting = {
        let child = child.clone();
        thread::Builder::new()
            .name("foreign-event-waiter".into())
            .spawn(move || child.events(1_000))
            .unwrap()
    };
    assert_eq!(recv(&reads), 10);
    let refresh = parent.refresh(ReadStop::new()).unwrap();
    assert!(matches!(
        refresh.initial(),
        SessionStatus::ReadOnly(Err(NativeClientError::Io(ErrorKind::ConnectionRefused)))
    ));
    assert!(refresh.read().is_none());
    assert!(refresh.recheck().is_none());
    assert_eq!(network.0.load(Ordering::SeqCst), 1);
    clock.value.store(1_010, Ordering::SeqCst);
    wake.wake();
    assert!(waiting
        .join()
        .unwrap()
        .unwrap()
        .frames_json()
        .unwrap()
        .is_empty());
    parent.close().unwrap();
    peer.join();
}

#[test]
fn foreign_canonical_response_copy_bound_preserves_factual_response_and_live_admission() {
    for normalized_growth in [false, true] {
        let root = fixtures::root(true);
        let (address, peer) = fixtures::peer(move |wire| {
            let request = wire.request().unwrap();
            assert_eq!(request.id, "1");
            let head = if normalized_growth {
                "{\"type\":\"res\",\"id\":\"1\",\"ok\":true,\"payload\":{\"n\":1e3,\"padding\":\""
            } else {
                "{\"type\":\"res\",\"id\":\"1\",\"ok\":true,\"payload\":{\"padding\":\""
            };
            let tail = "\"}}";
            let text = format!(
                "{head}{}{tail}",
                "x".repeat(MAX_PROTECTED_RESPONSE_BYTES - head.len() - tail.len())
            );
            assert_eq!(text.len(), MAX_PROTECTED_RESPONSE_BYTES);
            wire.raw(&text);
            let next = wire.request().unwrap();
            assert_eq!(next.id, "2");
            wire.send(OutgoingMessage::Response(
                ResponseFrame::success("2", &json!({"next":true})).unwrap(),
            ));
            assert!(wire.request().is_none());
        });
        let parent = parent(
            root.path(),
            address,
            Arc::new(LocalProductConnector::default()),
            ProductLimits::new(
                1,
                2,
                2 * MAX_PROTECTED_RESPONSE_BYTES,
                5,
                30_000,
                30_000,
                30_000,
            )
            .unwrap(),
        );
        let child = child(&parent);
        let ticket = child
            .request("conversation.list".into(), "{}".into())
            .unwrap();
        let completion = ticket.wait();
        assert_eq!(completion.kind(), NativeProductCompletionKind::Response);
        assert!(completion.failure().is_none());
        if normalized_growth {
            for _ in 0..2 {
                assert_eq!(
                    failure(completion.response_json().err().unwrap()),
                    NativeProductFailureKind::ResponseTooLarge
                );
            }
        } else {
            assert_eq!(
                completion.response_json().unwrap().unwrap().len(),
                MAX_PROTECTED_RESPONSE_BYTES
            );
        }
        assert_eq!(completion.kind(), NativeProductCompletionKind::Response);
        assert!(completion.failure().is_none());
        assert!(
            child.failure().is_none(),
            "copy refusal did not close physical child"
        );
        assert_eq!(
            failure(
                child
                    .request("conversation.list".into(), "{}".into())
                    .err()
                    .unwrap()
            ),
            NativeProductFailureKind::Capacity
        );
        drop(ticket);
        drop(completion);
        let next = child
            .request("conversation.list".into(), "{}".into())
            .unwrap();
        assert_eq!(next.id(), 2);
        assert_eq!(next.wait().kind(), NativeProductCompletionKind::Response);
        parent.close().unwrap();
        peer.join();
    }
}

struct PanickingConnector;
impl ProductConnector for PanickingConnector {
    fn connect(&self, _: SocketAddr, _: Duration) -> IoResult<ConnectedProduct> {
        panic!("injected product worker fault")
    }
}
#[test]
fn opening_worker_fault_is_original_product_failure_and_parent_releases_after_join() {
    let root = fixtures::root(true);
    let parent = parent(
        root.path(),
        "127.0.0.1:9".parse().unwrap(),
        Arc::new(PanickingConnector),
        ProductLimits::default(),
    );
    let child = NativeProductSession {
        value: parent.open_product("phone-instance".into()).unwrap(),
    };
    assert_eq!(
        failure(child.wait_ready().err().unwrap()),
        NativeProductFailureKind::WorkerPanicked
    );
    assert!(child.ready().is_none());
    assert_eq!(
        parent.close(),
        Err(SessionFailure::Product(ProductFailure::WorkerPanicked))
    );
    assert_eq!(
        parent.close(),
        Err(SessionFailure::Product(ProductFailure::WorkerPanicked))
    );
    drop(FilePairingState::open(root.path(), Path::new("private")).unwrap());
}

#[test]
fn poisoned_gate_signal_stops_escaped_child_and_physical_close_retains_private_lease() {
    let root = fixtures::root(true);
    let gate = Held::new();
    let (address, peer) = fixtures::peer(|wire| {
        assert!(wire.request().is_none());
    });
    let parent = parent(
        root.path(),
        address,
        Arc::new(CleanupConnector { gate: gate.clone() }),
        ProductLimits::default(),
    );
    let _release = Release(gate.clone());
    let child = child(&parent);
    // Gate poisoning itself has no public constructor. Exercise its recovery
    // branch with an actual live child and actual private lease, rather than a
    // fake joined flag; future public admission remains Damaged.
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _owned = parent.gate.lock().unwrap();
        panic!("injected native gate fault");
    }))
    .is_err());
    assert!(matches!(
        parent.open_product("phone-instance".into()),
        Err(SessionFailure::Damaged)
    ));
    assert_eq!(parent.request_close(), Err(SessionFailure::Damaged));
    assert_eq!(
        failure(
            child
                .request("conversation.list".into(), "{}".into())
                .err()
                .unwrap()
        ),
        NativeProductFailureKind::Cancelled
    );
    gate.entered();
    let (sent, received) = mpsc::channel();
    let close = thread::spawn(move || {
        let result = parent.close();
        sent.send((parent, result)).unwrap();
    });
    assert!(received.recv_timeout(Duration::from_millis(40)).is_err());
    assert!(matches!(
        FilePairingState::open(root.path(), Path::new("private")),
        Err(PrivateStateError::Locked)
    ));
    gate.release();
    let (parent, result) = recv(&received);
    assert_eq!(result, Err(SessionFailure::Damaged));
    assert_eq!(parent.close(), Err(SessionFailure::Damaged));
    close.join().unwrap();
    peer.join();
    drop(FilePairingState::open(root.path(), Path::new("private")).unwrap());
}

#[test]
fn parent_signal_interrupts_actual_tls_opening_before_handshake_deadline() {
    let root = fixtures::root(true);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (sent, hello) = mpsc::channel();
    let peer = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut bytes = [0; 4096];
        assert!(
            socket.read(&mut bytes).unwrap() > 0,
            "actual client TLS bytes arrived"
        );
        sent.send(()).unwrap();
        while socket.read(&mut bytes).unwrap() > 0 {}
    });
    let parent = parent(
        root.path(),
        address,
        Arc::new(LocalProductConnector::default()),
        ProductLimits::default(),
    );
    let child = NativeProductSession {
        value: parent.open_product("phone-instance".into()).unwrap(),
    };
    recv(&hello);
    let started = Instant::now();
    parent.request_close().unwrap();
    assert_eq!(
        child.failure().unwrap().kind(),
        NativeProductFailureKind::Cancelled
    );
    parent.close().unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "independent stop bypassed thirty-second TLS wait"
    );
    assert_eq!(
        failure(child.wait_ready().err().unwrap()),
        NativeProductFailureKind::Cancelled
    );
    peer.join().unwrap();
    drop(FilePairingState::open(root.path(), Path::new("private")).unwrap());
}

#[test]
fn closed_registration_gate_prevents_worker_start_and_native_connect() {
    let root = fixtures::root(true);
    let loading = Held::new();
    let network = Arc::new(CountingConnector {
        calls: AtomicUsize::new(0),
        delegate: Arc::new(RefusingProduct),
    });
    let parent = Arc::new(
        NativeSession::compose(Dependencies {
            state: Arc::new(HeldStore {
                owned: FilePairingState::open(root.path(), Path::new("private")).unwrap(),
                gate: loading.clone(),
            }),
            path: root.path().join("private/cache.sqlite"),
            address: "127.0.0.1:9".parse().unwrap(),
            connector: Arc::new(SocketConnector),
            entropy: Arc::new(OsEntropyFactory),
            files: Arc::new(OsCacheFiles),
            artifacts: Arc::new(DiskArtifactFiles),
            wall: Arc::new(Wall),
            clock: Arc::new(Monotonic::new()),
            pause: Arc::new(ThreadPause),
            runtime: Builder::new_current_thread().enable_all().build().unwrap(),
            product: ProductDependencies {
                connector: network.clone(),
                wake: Arc::new(ThreadProductWake::default()),
                limits: ProductLimits::default(),
            },
        })
        .unwrap(),
    );
    let _release = Release(loading.clone());
    let opening = {
        let parent = parent.clone();
        thread::spawn(move || parent.open_product("phone-instance".into()))
    };
    loading.entered();
    let mut registration = parent.gate.lock().unwrap();
    // Hold the actual registration mutex at the final gate, after the earlier
    // gate read. Closing this same gate simulates its request_close transition.
    registration.closed = true;
    loading.release();
    thread::sleep(Duration::from_millis(40));
    assert_eq!(
        network.calls.load(Ordering::SeqCst),
        0,
        "no native I/O before closed gate installation decision"
    );
    drop(registration);
    assert!(matches!(
        opening.join().unwrap(),
        Err(SessionFailure::Closed)
    ));
    assert_eq!(network.calls.load(Ordering::SeqCst), 0);
    parent.close().unwrap();
}

#[test]
fn foreign_close_retains_distinct_inflight_and_queued_dispatch_stages() {
    let root = fixtures::root(true);
    let pending = Held::new();
    let peer_pending = pending.clone();
    let (address, peer) = fixtures::peer(move |wire| {
        let first = wire.request().unwrap();
        assert_eq!(first.id, "1");
        peer_pending.hold();
        assert!(wire.request().is_none(), "queued request never dispatched");
    });
    let parent = parent(
        root.path(),
        address,
        Arc::new(LocalProductConnector::default()),
        ProductLimits::default(),
    );
    let _release = Release(pending.clone());
    let child = child(&parent);
    let first = child
        .request("conversation.list".into(), "{}".into())
        .unwrap();
    pending.entered();
    let queued = child
        .request("conversation.list".into(), "{}".into())
        .unwrap();
    assert_eq!(queued.id(), 2);
    parent.request_close().unwrap();
    parent.close().unwrap();
    let first = first.wait();
    let queued = queued.wait();
    assert_eq!(first.kind(), NativeProductCompletionKind::Uncertain);
    assert_eq!(queued.kind(), NativeProductCompletionKind::NotDispatched);
    assert_eq!(
        first.failure().unwrap().kind(),
        NativeProductFailureKind::Cancelled
    );
    assert_eq!(
        queued.failure().unwrap().kind(),
        NativeProductFailureKind::Cancelled
    );
    assert!(first.response_json().unwrap().is_none());
    assert!(queued.response_json().unwrap().is_none());
    pending.release();
    peer.join();
}

struct ReplacementConnector {
    calls: AtomicUsize,
    cleanup: Arc<Held>,
}
impl ProductConnector for ReplacementConnector {
    fn connect(&self, address: SocketAddr, timeout: Duration) -> IoResult<ConnectedProduct> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            CleanupConnector {
                gate: self.cleanup.clone(),
            }
            .connect(address, timeout)
        } else {
            Err(IoError::from(ErrorKind::ConnectionRefused))
        }
    }
}
#[test]
fn replacement_cannot_publish_or_connect_before_old_physical_join() {
    let root = fixtures::root(true);
    let cleanup = Held::new();
    let (address, peer) = fixtures::peer(|wire| {
        assert!(wire.request().is_none());
    });
    let connector = Arc::new(ReplacementConnector {
        calls: AtomicUsize::new(0),
        cleanup: cleanup.clone(),
    });
    let parent = Arc::new(parent(
        root.path(),
        address,
        connector.clone(),
        ProductLimits::default(),
    ));
    let old = child(&parent);
    let _release = Release(cleanup.clone());
    let (sent, received) = mpsc::channel();
    let replacing = {
        let parent = parent.clone();
        thread::spawn(move || {
            sent.send(parent.open_product("replacement".into()))
                .unwrap();
        })
    };
    cleanup.entered();
    assert!(
        received.recv_timeout(Duration::from_millis(40)).is_err(),
        "parent remains open but replacement cannot publish before original join"
    );
    assert_eq!(
        connector.calls.load(Ordering::SeqCst),
        1,
        "no second native connect before old physical join"
    );
    assert_eq!(
        old.failure().unwrap().kind(),
        NativeProductFailureKind::Cancelled
    );
    cleanup.release();
    let replacement = recv(&received).unwrap();
    replacing.join().unwrap();
    assert_eq!(
        replacement.session().wait_ready().err(),
        Some(ProductFailure::Transport)
    );
    assert_eq!(connector.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        parent.close(),
        Err(SessionFailure::Product(ProductFailure::Transport))
    );
    peer.join();
}

impl OwnerPhoneOriginalStore for HeldStore {
    fn reserve_original(
        &self,
        reservation: OwnerPhoneReservation,
    ) -> Result<
        Box<dyn nessa_auth::application::pairing::OwnerPhoneOriginalCreation + '_>,
        PrivateStateError,
    > {
        self.owned.reserve_original(reservation)
    }
    fn load_original(
        &self,
    ) -> Result<
        Option<nessa_auth::application::pairing::OwnerPhonePrivateOriginal>,
        PrivateStateError,
    > {
        self.owned.load_original()
    }
    fn enrollment_signed(&self) -> Result<bool, PrivateStateError> {
        self.owned.enrollment_signed()
    }
}

impl OwnerPhoneOriginalStore for PoisonableStore {
    fn reserve_original(
        &self,
        reservation: OwnerPhoneReservation,
    ) -> Result<
        Box<dyn nessa_auth::application::pairing::OwnerPhoneOriginalCreation + '_>,
        PrivateStateError,
    > {
        self.owned.reserve_original(reservation)
    }
    fn load_original(
        &self,
    ) -> Result<
        Option<nessa_auth::application::pairing::OwnerPhonePrivateOriginal>,
        PrivateStateError,
    > {
        self.owned.load_original()
    }
    fn enrollment_signed(&self) -> Result<bool, PrivateStateError> {
        self.owned.enrollment_signed()
    }
}

impl OwnerPhoneOriginalStore for FailedStore {
    fn reserve_original(
        &self,
        reservation: OwnerPhoneReservation,
    ) -> Result<
        Box<dyn nessa_auth::application::pairing::OwnerPhoneOriginalCreation + '_>,
        PrivateStateError,
    > {
        self.owned.reserve_original(reservation)
    }
    fn load_original(
        &self,
    ) -> Result<
        Option<nessa_auth::application::pairing::OwnerPhonePrivateOriginal>,
        PrivateStateError,
    > {
        self.owned.load_original()
    }
    fn enrollment_signed(&self) -> Result<bool, PrivateStateError> {
        self.owned.enrollment_signed()
    }
}

impl nessa_auth::application::pairing::OwnerPhoneEnrollmentStore for HeldStore {
    fn adopt_owner_phone(
        &self,
        original: &nessa_auth::domain::pairing::OwnerPhoneRequest,
        digest: &[u8; 32],
        consent: &DisclosedConsent,
        issued: Option<(&CredentialId, &ResourceId)>,
    ) -> Result<
        nessa_auth::application::pairing::OwnerPhoneAdoption,
        nessa_auth::application::pairing::OwnerPhoneAdoptionFailure,
    > {
        self.owned
            .adopt_owner_phone(original, digest, consent, issued)
    }
}

impl nessa_auth::application::pairing::OwnerPhoneEnrollmentStore for PoisonableStore {
    fn adopt_owner_phone(
        &self,
        original: &nessa_auth::domain::pairing::OwnerPhoneRequest,
        digest: &[u8; 32],
        consent: &DisclosedConsent,
        issued: Option<(&CredentialId, &ResourceId)>,
    ) -> Result<
        nessa_auth::application::pairing::OwnerPhoneAdoption,
        nessa_auth::application::pairing::OwnerPhoneAdoptionFailure,
    > {
        self.owned
            .adopt_owner_phone(original, digest, consent, issued)
    }
}

impl nessa_auth::application::pairing::OwnerPhoneEnrollmentStore for FailedStore {
    fn adopt_owner_phone(
        &self,
        original: &nessa_auth::domain::pairing::OwnerPhoneRequest,
        digest: &[u8; 32],
        consent: &DisclosedConsent,
        issued: Option<(&CredentialId, &ResourceId)>,
    ) -> Result<
        nessa_auth::application::pairing::OwnerPhoneAdoption,
        nessa_auth::application::pairing::OwnerPhoneAdoptionFailure,
    > {
        self.owned
            .adopt_owner_phone(original, digest, consent, issued)
    }
}
