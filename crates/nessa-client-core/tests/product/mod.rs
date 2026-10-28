//! Independent real TLS evidence for the native product transport ordering table.
mod fixtures;
use crate::product::{
    ConnectedProduct, LocalProductConnector, ProductCompletion, ProductConnector, ProductFailure,
    ProductInterrupt, ProductLimits, ProductSession, ProductStream, ProductWake, ThreadProductWake,
};
use fixtures::{open, peer, prepare, receive, signal};
use nessa_auth::adapters::pairing::{NativeIdentity, OsEntropy};
use nessa_protocol::clock::Clock;
use nessa_protocol::pairing::{
    encode_frame, MAX_PROTECTED_REQUEST_BYTES, MAX_PROTECTED_RESPONSE_BYTES,
};
use nessa_protocol::product::generated::{product_event, SessionTermination};
use nessa_protocol::product_contract::generated::SessionCloseReason;
use nessa_protocol::protocol::{EventFrame, OutgoingMessage, ResponseFrame};
use serde_json::json;
use std::error::Error;
use std::fmt::{Display, Formatter, Result as FormatResult};
use std::io::{Error as IoError, ErrorKind, Read, Result as IoResult, Write};
use std::net::{SocketAddr, TcpListener};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    mpsc::{Receiver, RecvTimeoutError, Sender},
    Arc, Mutex,
};
use std::thread;
use std::time::{Duration, Instant};

fn limits(pending: usize) -> ProductLimits {
    ProductLimits::new(
        pending,
        2,
        2 * MAX_PROTECTED_RESPONSE_BYTES,
        10,
        2_000,
        2_000,
        500,
    )
    .unwrap()
}
#[test]
fn canonical_actual_ready_method_admission_and_monotonic_original_ids() {
    let (endpoint, peer) = peer(|wire| {
        let first = wire.request().unwrap();
        assert_eq!(first.id, "1");
        wire.response(&first.id);
        let second = wire.request().unwrap();
        assert_eq!(second.id, "2");
        wire.response(&second.id);
        assert!(wire.request().is_none());
    });
    let session = open(endpoint, limits(2));
    let ready = session.ready().unwrap();
    assert_eq!(ready.credential_id, "credential");
    assert!(matches!(
        session.request("conversation.send", &json!({}).to_string()),
        Err(ProductFailure::UnadvertisedMethod)
    ));
    for id in 1..=2 {
        let ticket = session
            .request("conversation.list", &json!({"original":id}).to_string())
            .unwrap();
        assert_eq!(ticket.id(), id);
        assert!(
            matches!(ticket.wait().value(),ProductCompletion::Response(response) if response.id == id.to_string() && response.ok)
        );
    }
    session.close().unwrap();
    peer.join();
    assert_eq!(session.ready().unwrap().credential_id, "credential");
    assert!(matches!(
        session.request("conversation.list", &json!({}).to_string()),
        Err(ProductFailure::Cancelled)
    ));
}
#[test]
fn completed_body_and_last_settlement_observer_keep_pending_permit() {
    let (endpoint, peer) = peer(|wire| {
        let request = wire.request().unwrap();
        wire.response(&request.id);
        let next = wire.request().unwrap();
        assert_eq!(next.id, "2");
        wire.response(&next.id);
        assert!(wire.request().is_none());
    });
    let session = open(endpoint, limits(1));
    let ticket = session
        .request("conversation.list", &json!({}).to_string())
        .unwrap();
    let settlement = ticket.wait();
    drop(ticket);
    assert!(matches!(
        session.request("conversation.list", &json!({}).to_string()),
        Err(ProductFailure::Capacity)
    ));
    drop(settlement);
    let next = session
        .request("conversation.list", &json!({}).to_string())
        .unwrap();
    next.wait();
    drop(next);
    session.close().unwrap();
    peer.join();
}
#[test]
fn dropped_dispatched_ticket_does_not_release_permit_or_replay() {
    let (sent, seen) = signal();
    let (release, held) = signal();
    let (endpoint, peer) = peer(move |wire| {
        let request = wire.request().unwrap();
        assert_eq!(request.id, "1");
        sent.send(()).unwrap();
        receive(&held);
        wire.response(&request.id);
        let next = wire.request().unwrap();
        assert_eq!(next.id, "2");
        wire.response(&next.id);
        assert!(wire.request().is_none());
    });
    let session = open(endpoint, limits(1));
    let ticket = session
        .request("conversation.list", &json!({}).to_string())
        .unwrap();
    receive(&seen);
    drop(ticket);
    assert!(matches!(
        session.request("conversation.list", &json!({}).to_string()),
        Err(ProductFailure::Capacity)
    ));
    release.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let next = loop {
        match session.request("conversation.list", &json!({}).to_string()) {
            Ok(ticket) => break ticket,
            Err(ProductFailure::Capacity) if Instant::now() < deadline => thread::yield_now(),
            other => panic!("unexpected admission {:?}", other.err()),
        }
    };
    next.wait();
    session.close().unwrap();
    peer.join();
}
#[test]
fn idle_partial_frame_survives_ticks_and_event_waiter_does_not_own_reader() {
    let (prefix, seen) = signal();
    let (release, held) = signal();
    let (endpoint, peer) = peer(move |wire| {
        let event = OutgoingMessage::Event(
            EventFrame::push("conversation.listed", &json!({}), 1, 1).unwrap(),
        );
        let bytes = encode_frame(
            MAX_PROTECTED_RESPONSE_BYTES,
            event.to_wire_text().unwrap().as_bytes(),
        )
        .unwrap();
        wire.raw(&bytes[..2]);
        prefix.send(()).unwrap();
        receive(&held);
        wire.raw(&bytes[2..]);
        let request = wire.request().unwrap();
        wire.response(&request.id);
        assert!(wire.request().is_none());
    });
    let session = open(endpoint, limits(2));
    receive(&seen);
    let events = session.clone();
    let waiting = thread::spawn(move || events.events(u64::MAX).unwrap());
    thread::sleep(Duration::from_millis(40));
    let ticket = session
        .request("conversation.list", &json!({}).to_string())
        .unwrap();
    release.send(()).unwrap();
    assert!(matches!(
        ticket.wait().value(),
        ProductCompletion::Response(_)
    ));
    let batch = waiting.join().unwrap();
    assert_eq!(batch.len(), 1);
    assert_eq!(batch[0].seq, 1);
    session.close().unwrap();
    peer.join();
}
#[test]
fn actual_partial_frame_has_total_deadline_even_without_requests() {
    let (sent, seen) = signal();
    let (endpoint, peer) = peer(move |wire| {
        wire.raw(&[0]);
        sent.send(()).unwrap();
        assert!(wire.request().is_none());
    });
    let session = open(
        endpoint,
        ProductLimits::new(1, 1, MAX_PROTECTED_RESPONSE_BYTES, 5, 2_000, 2_000, 35).unwrap(),
    );
    receive(&seen);
    assert_eq!(
        session.events(u64::MAX).err(),
        Some(ProductFailure::TimedOut)
    );
    assert_eq!(session.close(), Err(ProductFailure::TimedOut));
    peer.join();
}
#[test]
fn first_inflight_deadline_is_uncertain_and_queued_original_is_not_dispatched() {
    let (sent, seen) = signal();
    let (endpoint, peer) = peer(move |wire| {
        let first = wire.request().unwrap();
        assert_eq!(first.id, "1");
        sent.send(()).unwrap();
        assert!(wire.request().is_none());
    });
    let session = open(
        endpoint,
        ProductLimits::new(2, 1, MAX_PROTECTED_RESPONSE_BYTES, 5, 70, 2_000, 500).unwrap(),
    );
    let first = session
        .request("conversation.list", &json!({}).to_string())
        .unwrap();
    receive(&seen);
    let queued = session
        .request("conversation.list", &json!({}).to_string())
        .unwrap();
    assert_eq!(
        first.wait().value(),
        &ProductCompletion::Uncertain(ProductFailure::TimedOut)
    );
    assert_eq!(
        queued.wait().value(),
        &ProductCompletion::NotDispatched(ProductFailure::TimedOut)
    );
    assert_eq!(session.close(), Err(ProductFailure::TimedOut));
    peer.join();
}
#[test]
fn close_joins_one_worker_and_preserves_received_success_and_gateway_error() {
    for success in [true, false] {
        let (endpoint, peer) = peer(move |wire| {
            let request = wire.request().unwrap();
            wire.send(OutgoingMessage::Response(if success {
                ResponseFrame::success(&request.id, &json!({"fact":true})).unwrap()
            } else {
                ResponseFrame::failure_with_details(
                    &request.id,
                    "forbidden",
                    "original",
                    Some(json!({"exact":"details"})),
                )
            }));
            assert!(wire.request().is_none());
        });
        let session = open(endpoint, limits(2));
        let ticket = session
            .request("conversation.list", &json!({}).to_string())
            .unwrap();
        let fact = ticket.wait();
        let other = session.clone();
        let close = thread::spawn(move || other.close());
        session.request_close();
        assert_eq!(close.join().unwrap(), Ok(()));
        assert_eq!(session.close(), Ok(()));
        assert!(
            matches!(fact.value(),ProductCompletion::Response(response) if response.ok==success)
        );
        if let ProductCompletion::Response(response) = fact.value() {
            if !success {
                assert_eq!(
                    response.error.as_ref().unwrap().details,
                    Some(json!({"exact":"details"}))
                );
            }
        }
        assert_eq!(ticket.wait().value(), fact.value());
        peer.join();
    }
}
#[test]
fn close_during_inflight_retains_uncertainty_and_no_extra_write() {
    let (sent, seen) = signal();
    let (endpoint, peer) = peer(move |wire| {
        wire.request().unwrap();
        sent.send(()).unwrap();
        assert!(wire.request().is_none());
    });
    let session = open(endpoint, limits(2));
    let first = session
        .request("conversation.list", &json!({}).to_string())
        .unwrap();
    receive(&seen);
    let queued = session
        .request("conversation.list", &json!({}).to_string())
        .unwrap();
    session.request_close();
    session.close().unwrap();
    assert_eq!(
        first.wait().value(),
        &ProductCompletion::Uncertain(ProductFailure::Cancelled)
    );
    assert_eq!(
        queued.wait().value(),
        &ProductCompletion::NotDispatched(ProductFailure::Cancelled)
    );
    peer.join();
}
#[test]
fn unknown_duplicate_and_malformed_responses_are_terminal_original_causes() {
    for bad in ["unknown", "duplicate", "malformed", "oversize", "eof"] {
        let (endpoint, peer) = peer(move |wire| {
            let request = wire.request().unwrap();
            match bad {
                "unknown" => wire.response("90"),
                "duplicate" => {
                    wire.response(&request.id);
                    wire.response(&request.id);
                }
                "malformed" => wire.raw(
                    &encode_frame(
                        MAX_PROTECTED_RESPONSE_BYTES,
                        b"{\"type\":\"res\",\"id\":\"1\",\"ok\":true}",
                    )
                    .unwrap(),
                ),
                "oversize" => wire.raw(
                    &u32::try_from(MAX_PROTECTED_RESPONSE_BYTES + 1)
                        .unwrap()
                        .to_be_bytes(),
                ),
                "eof" => return,
                _ => unreachable!(),
            }
            assert!(wire.request().is_none());
        });
        let session = open(endpoint, limits(2));
        let first = session
            .request("conversation.list", &json!({}).to_string())
            .unwrap();
        let fact = first.wait();
        let cause = match bad {
            "unknown" | "duplicate" => ProductFailure::Correlation,
            "malformed" => ProductFailure::Protocol,
            "oversize" => ProductFailure::ResponseTooLarge,
            _ => ProductFailure::Closed(None),
        };
        if bad == "duplicate" {
            assert!(matches!(fact.value(), ProductCompletion::Response(_)));
        } else {
            assert_eq!(fact.value(), &ProductCompletion::Uncertain(cause));
        }
        assert_eq!(session.events(u64::MAX).err(), Some(cause));
        assert_eq!(session.close(), Err(cause));
        peer.join();
    }
}
#[test]
fn event_count_and_byte_overflow_are_explicit_terminal_failures() {
    for bytes in [1, MAX_PROTECTED_RESPONSE_BYTES] {
        let (endpoint, peer) = peer(move |wire| {
            wire.event(1);
            wire.event(2);
            assert!(wire.request().is_none());
        });
        let session = open(
            endpoint,
            ProductLimits::new(1, 1, bytes, 5, 2_000, 2_000, 500).unwrap(),
        );
        // Do not consume the first event before the worker attempts the second.
        let deadline = Instant::now() + Duration::from_secs(2);
        while session.failure().is_none() && Instant::now() < deadline {
            thread::yield_now();
        }
        assert_eq!(session.failure(), Some(ProductFailure::Capacity));
        let first = session.events(u64::MAX);
        if bytes == 1 {
            assert_eq!(first.err(), Some(ProductFailure::Capacity));
        } else {
            assert_eq!(first.unwrap().len(), 1);
            assert_eq!(
                session.events(u64::MAX).err(),
                Some(ProductFailure::Capacity)
            );
        }
        assert_eq!(session.close(), Err(ProductFailure::Capacity));
        peer.join();
    }
}
#[test]
fn close_before_start_has_no_physical_connect() {
    struct NoDial;
    impl ProductConnector for NoDial {
        fn connect(&self, _: SocketAddr, _: Duration) -> IoResult<ConnectedProduct> {
            panic!("no dial after stopped preparation");
        }
    }
    let identity = Arc::new(NativeIdentity::generate(&mut OsEntropy).unwrap());
    let session = ProductSession::prepare(
        "127.0.0.1:1".parse().unwrap(),
        identity.clone(),
        identity.public_spki(),
        "credential".into(),
        "phone-instance".into(),
        Arc::new(NoDial),
        Arc::new(fixtures::Time(Instant::now())),
        Arc::new(ThreadProductWake::default()),
        limits(1),
    );
    session.request_close();
    assert_eq!(session.start(), Err(ProductFailure::Cancelled));
    session.close().unwrap();
}
#[test]
fn prepared_owner_last_release_closes_escaped_ticket_and_original_socket() {
    let (sent, seen) = signal();
    let (endpoint, peer) = peer(move |wire| {
        wire.request().unwrap();
        sent.send(()).unwrap();
        assert!(wire.request().is_none());
    });
    let session = prepare(endpoint, limits(1));
    session.start().unwrap();
    session.wait_ready().unwrap();
    let ticket = session
        .request("conversation.list", &json!({}).to_string())
        .unwrap();
    receive(&seen);
    drop(session);
    assert_eq!(
        ticket.wait().value(),
        &ProductCompletion::Uncertain(ProductFailure::Cancelled)
    );
    peer.join();
}

#[test]
fn actual_tls_record_fragment_bytes_survive_nonterminal_poll_errors() {
    struct Fragment {
        enabled: Arc<AtomicBool>,
        reads: Arc<AtomicUsize>,
        inner: Box<dyn ProductStream>,
    }
    impl Read for Fragment {
        fn read(&mut self, bytes: &mut [u8]) -> IoResult<usize> {
            if self.enabled.load(Ordering::SeqCst) {
                if self.reads.fetch_add(1, Ordering::SeqCst) % 2 == 1 {
                    return Err(ErrorKind::WouldBlock.into());
                }
                return self.inner.read(&mut bytes[..1]);
            }
            self.inner.read(bytes)
        }
    }
    impl Write for Fragment {
        fn write(&mut self, bytes: &[u8]) -> IoResult<usize> {
            self.inner.write(bytes)
        }
        fn flush(&mut self) -> IoResult<()> {
            self.inner.flush()
        }
    }
    impl ProductStream for Fragment {
        fn read_timeout(&self, timeout: Duration) -> IoResult<()> {
            self.inner.read_timeout(timeout)
        }
        fn write_timeout(&self, timeout: Duration) -> IoResult<()> {
            self.inner.write_timeout(timeout)
        }
        fn shutdown(&self) -> IoResult<()> {
            self.inner.shutdown()
        }
    }
    struct Connector {
        enabled: Arc<AtomicBool>,
        reads: Arc<AtomicUsize>,
    }
    impl ProductConnector for Connector {
        fn connect(&self, address: SocketAddr, timeout: Duration) -> IoResult<ConnectedProduct> {
            let connected = LocalProductConnector::default().connect(address, timeout)?;
            Ok(ConnectedProduct {
                stream: Box::new(Fragment {
                    enabled: self.enabled.clone(),
                    reads: self.reads.clone(),
                    inner: connected.stream,
                }),
                interrupt: connected.interrupt,
            })
        }
    }
    let (endpoint, peer) = peer(|wire| {
        let request = wire.request().unwrap();
        wire.response(&request.id);
        assert!(wire.request().is_none());
    });
    let enabled = Arc::new(AtomicBool::new(false));
    let reads = Arc::new(AtomicUsize::new(0));
    let session = ProductSession::prepare(
        endpoint.address,
        endpoint.identity,
        endpoint.pin,
        "credential".into(),
        "phone-instance".into(),
        Arc::new(Connector {
            enabled: enabled.clone(),
            reads: reads.clone(),
        }),
        Arc::new(fixtures::Time(Instant::now())),
        Arc::new(ThreadProductWake::default()),
        limits(1),
    );
    session.start().unwrap();
    session.wait_ready().unwrap();
    enabled.store(true, Ordering::SeqCst);
    let ticket = session
        .request("conversation.list", &json!({}).to_string())
        .unwrap();
    assert!(
        matches!(ticket.wait().value(),ProductCompletion::Response(response) if response.id=="1")
    );
    assert!(reads.load(Ordering::SeqCst) > 20);
    assert_eq!(session.failure(), None);
    session.close().unwrap();
    peer.join();
}
#[test]
fn close_during_held_connector_interrupts_original_socket_before_tls() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let reader = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut bytes = [0; 8];
        assert_eq!(
            stream.read(&mut bytes).unwrap(),
            0,
            "stopped original must not emit TLS bytes"
        );
    });
    let (started, seen) = signal();
    let (release, held) = signal();
    struct Held {
        started: Sender<()>,
        held: Mutex<Receiver<()>>,
    }
    impl ProductConnector for Held {
        fn connect(&self, address: SocketAddr, timeout: Duration) -> IoResult<ConnectedProduct> {
            self.started.send(()).unwrap();
            receive(&self.held.lock().unwrap());
            LocalProductConnector::default().connect(address, timeout)
        }
    }
    let identity = Arc::new(NativeIdentity::generate(&mut OsEntropy).unwrap());
    let session = ProductSession::prepare(
        address,
        identity.clone(),
        identity.public_spki(),
        "credential".into(),
        "phone-instance".into(),
        Arc::new(Held {
            started,
            held: Mutex::new(held),
        }),
        Arc::new(fixtures::Time(Instant::now())),
        Arc::new(ThreadProductWake::default()),
        limits(1),
    );
    session.start().unwrap();
    receive(&seen);
    session.request_close();
    let other = session.clone();
    let (closed, closed_rx) = signal();
    let closing = thread::spawn(move || {
        other.close().unwrap();
        closed.send(()).unwrap();
    });
    assert!(matches!(
        closed_rx.recv_timeout(Duration::from_millis(50)),
        Err(RecvTimeoutError::Timeout)
    ));
    release.send(()).unwrap();
    receive(&closed_rx);
    closing.join().unwrap();
    reader.join().unwrap();
    assert_eq!(session.wait_ready().err(), Some(ProductFailure::Cancelled));
}
#[test]
fn idle_event_wait_expiry_does_not_close_connection_and_oversize_is_not_admitted() {
    let (endpoint, peer) = peer(|wire| {
        let first = wire.request().unwrap();
        assert_eq!(first.id, "1");
        wire.response(&first.id);
        assert!(wire.request().is_none());
    });
    let session = open(endpoint, limits(1));
    assert!(session.events(20).unwrap().is_empty());
    assert_eq!(session.failure(), None);
    let oversized = "x".repeat(MAX_PROTECTED_REQUEST_BYTES + 1);
    assert!(matches!(
        session.request("conversation.list", &json!({"text":oversized}).to_string()),
        Err(ProductFailure::RequestTooLarge)
    ));
    let first = session
        .request("conversation.list", &json!({}).to_string())
        .unwrap();
    assert_eq!(first.id(), 1);
    assert!(matches!(
        first.wait().value(),
        ProductCompletion::Response(_)
    ));
    session.close().unwrap();
    peer.join();
}
#[test]
fn operational_zero_and_overflow_limits_are_refused_without_effects() {
    assert!(matches!(
        ProductLimits::new(0, 1, 1, 1, 1, 1, 1),
        Err(ProductFailure::InvalidLimits)
    ));
    assert!(matches!(
        ProductLimits::new(1, usize::MAX, 1, 1, 1, 1, 1),
        Err(ProductFailure::InvalidLimits)
    ));
    assert!(matches!(
        ProductLimits::new(usize::MAX, 1, 1, 1, 1, 1, 1),
        Err(ProductFailure::InvalidLimits)
    ));
}

#[test]
fn first_physical_timeout_survives_stop_while_socket_cleanup_is_held() {
    struct HeldCleanup {
        inner: Box<dyn ProductStream>,
        entered: Sender<()>,
        held: Mutex<Receiver<()>>,
    }
    impl Read for HeldCleanup {
        fn read(&mut self, bytes: &mut [u8]) -> IoResult<usize> {
            self.inner.read(bytes)
        }
    }
    impl Write for HeldCleanup {
        fn write(&mut self, bytes: &[u8]) -> IoResult<usize> {
            self.inner.write(bytes)
        }
        fn flush(&mut self) -> IoResult<()> {
            self.inner.flush()
        }
    }
    impl ProductStream for HeldCleanup {
        fn read_timeout(&self, timeout: Duration) -> IoResult<()> {
            self.inner.read_timeout(timeout)
        }
        fn write_timeout(&self, timeout: Duration) -> IoResult<()> {
            self.inner.write_timeout(timeout)
        }
        fn shutdown(&self) -> IoResult<()> {
            let result = self.inner.shutdown();
            self.entered.send(()).unwrap();
            receive(&self.held.lock().unwrap());
            result
        }
    }
    struct Connector {
        entered: Sender<()>,
        held: Mutex<Option<Receiver<()>>>,
    }
    impl ProductConnector for Connector {
        fn connect(&self, address: SocketAddr, timeout: Duration) -> IoResult<ConnectedProduct> {
            let connected = LocalProductConnector::default().connect(address, timeout)?;
            Ok(ConnectedProduct {
                stream: Box::new(HeldCleanup {
                    inner: connected.stream,
                    entered: self.entered.clone(),
                    held: Mutex::new(self.held.lock().unwrap().take().unwrap()),
                }),
                interrupt: connected.interrupt,
            })
        }
    }
    struct Release(Sender<()>);
    impl Drop for Release {
        fn drop(&mut self) {
            let _ = self.0.send(());
        }
    }
    let (endpoint, peer) = peer(|wire| {
        wire.request().unwrap();
        assert!(wire.request().is_none());
    });
    let (entered, seen) = signal();
    let (release, held) = signal();
    let session = ProductSession::prepare(
        endpoint.address,
        endpoint.identity,
        endpoint.pin,
        "credential".into(),
        "phone-instance".into(),
        Arc::new(Connector {
            entered,
            held: Mutex::new(Some(held)),
        }),
        Arc::new(fixtures::Time(Instant::now())),
        Arc::new(ThreadProductWake::default()),
        // Long enough for the worker to be scheduled and record dispatch, and
        // short of the peer's three-second read. A 40ms budget loses that
        // race on a loaded Windows runner, so the request is never sent.
        ProductLimits::new(1, 1, MAX_PROTECTED_RESPONSE_BYTES, 5, 1_000, 2_000, 500).unwrap(),
    );
    session.start().unwrap();
    session.wait_ready().unwrap();
    let ticket = session
        .request("conversation.list", &json!({}).to_string())
        .unwrap();
    let release = Release(release);
    receive(&seen);
    assert_eq!(session.failure(), Some(ProductFailure::TimedOut));
    session.request_close();
    assert_eq!(session.failure(), Some(ProductFailure::TimedOut));
    drop(release);
    assert_eq!(session.close(), Err(ProductFailure::TimedOut));
    assert_eq!(
        ticket.wait().value(),
        &ProductCompletion::Uncertain(ProductFailure::TimedOut)
    );
    peer.join();
}

#[test]
fn held_delivered_event_batch_clones_keep_count_and_byte_permits() {
    for byte_bound in [false, true] {
        let (next, held) = signal();
        let (second, sent) = signal();
        let (endpoint, peer) = peer(move |wire| {
            wire.event(1);
            receive(&held);
            wire.event(2);
            second.send(()).unwrap();
            assert!(wire.request().is_none());
        });
        let event = EventFrame::push("conversation.listed", &json!({"seq":1}), 1, 1).unwrap();
        let event_bytes = OutgoingMessage::Event(event).to_wire_text().unwrap().len();
        let limits = ProductLimits::new(
            1,
            if byte_bound { 2 } else { 1 },
            if byte_bound {
                event_bytes
            } else {
                2 * MAX_PROTECTED_RESPONSE_BYTES
            },
            5,
            2_000,
            2_000,
            500,
        )
        .unwrap();
        let session = open(endpoint, limits);
        let batch = session.events(u64::MAX).unwrap();
        assert_eq!(batch.len(), 1);
        let last = batch.clone();
        drop(batch);
        next.send(()).unwrap();
        receive(&sent);
        let deadline = Instant::now() + Duration::from_secs(2);
        while session.failure().is_none() && Instant::now() < deadline {
            thread::yield_now();
        }
        assert_eq!(session.failure(), Some(ProductFailure::Capacity));
        assert_eq!(last[0].seq, 1);
        drop(last);
        assert_eq!(session.close(), Err(ProductFailure::Capacity));
        peer.join();
    }
}
#[test]
fn final_delivered_batch_release_restores_native_event_capacity() {
    let (next, held) = signal();
    let (endpoint, peer) = peer(move |wire| {
        wire.event(1);
        receive(&held);
        wire.event(2);
        assert!(wire.request().is_none());
    });
    let session = open(
        endpoint,
        ProductLimits::new(1, 1, MAX_PROTECTED_RESPONSE_BYTES, 5, 2_000, 2_000, 500).unwrap(),
    );
    let batch = session.events(u64::MAX).unwrap();
    let last = batch.clone();
    drop(batch);
    drop(last);
    next.send(()).unwrap();
    let second = session.events(u64::MAX).unwrap();
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].seq, 2);
    assert_eq!(session.failure(), None);
    session.close().unwrap();
    peer.join();
}
#[test]
fn malformed_duplicate_and_nested_duplicate_params_are_refused_before_id_or_dispatch() {
    let (endpoint, peer) = peer(|wire| {
        let request = wire.request().unwrap();
        assert_eq!(request.id, "1");
        wire.response(&request.id);
        assert!(wire.request().is_none());
    });
    let session = open(endpoint, limits(1));
    for params in [
        r#"{"a":1,"a":2}"#,
        r#"{"nested":{"target":"A","target":"B"}}"#,
        r#"{"incomplete":"#,
        r#"{} []"#,
    ] {
        assert!(matches!(
            session.request("conversation.list", params),
            Err(ProductFailure::InvalidRequest)
        ));
    }
    let oversized_malformed = "x".repeat(MAX_PROTECTED_REQUEST_BYTES + 1);
    assert!(matches!(
        session.request("conversation.list", &oversized_malformed),
        Err(ProductFailure::RequestTooLarge)
    ));
    let ticket = session.request("conversation.list", "{}").unwrap();
    assert_eq!(ticket.id(), 1);
    assert!(matches!(
        ticket.wait().value(),
        ProductCompletion::Response(_)
    ));
    session.close().unwrap();
    peer.join();
}
#[test]
fn legitimate_idle_tls_key_update_does_not_start_product_frame_deadline() {
    let (control, seen) = signal();
    let (endpoint, peer) = peer(move |wire| {
        wire.key_update();
        control.send(()).unwrap();
        let request = wire.request().unwrap();
        wire.response(&request.id);
        assert!(wire.request().is_none());
    });
    let session = open(
        endpoint,
        ProductLimits::new(1, 1, MAX_PROTECTED_RESPONSE_BYTES, 5, 2_000, 2_000, 25).unwrap(),
    );
    receive(&seen);
    thread::sleep(Duration::from_millis(80));
    assert_eq!(session.failure(), None);
    let ticket = session.request("conversation.list", "{}").unwrap();
    assert!(matches!(
        ticket.wait().value(),
        ProductCompletion::Response(_)
    ));
    session.close().unwrap();
    peer.join();
}

#[test]
fn contradictory_actual_ready_credential_is_refused_by_canonical_handshake() {
    let mut ready = fixtures::ready();
    ready.credential_id = "different-valid-credential".into();
    let (endpoint, peer) = fixtures::peer_ready(ready, |wire| assert!(wire.request().is_none()));
    let session = prepare(endpoint, limits(1));
    session.start().unwrap();
    assert_eq!(
        session.wait_ready().err(),
        Some(ProductFailure::Correlation)
    );
    assert!(session.ready().is_none());
    assert_eq!(session.close(), Err(ProductFailure::Correlation));
    peer.join();
}
#[test]
fn known_decoded_failure_is_reserved_before_held_cleanup_and_later_stop() {
    for case in [
        "correlation",
        "protocol",
        "capacity",
        "ready-credential",
        "ready-shape",
        "refused",
        "panic",
    ] {
        let opening = matches!(case, "ready-credential" | "ready-shape" | "refused");
        let (endpoint, peer) = if case == "refused" {
            fixtures::refused_peer()
        } else if opening {
            let mut ready = fixtures::ready();
            ready.credential_id = if case == "ready-shape" {
                "".into()
            } else {
                "other-credential".into()
            };
            fixtures::peer_ready(ready, |wire| assert!(wire.request().is_none()))
        } else {
            peer(move |wire| {
                if case == "panic" {
                    let _ = wire.request();
                    assert!(wire.request().is_none());
                    return;
                }
                wire.request().unwrap();
                match case {
                    "correlation" => wire.response("foreign-id"),
                    "protocol" => wire.raw(
                        &encode_frame(
                            MAX_PROTECTED_RESPONSE_BYTES,
                            b"{\"type\":\"res\",\"id\":\"1\",\"ok\":true}",
                        )
                        .unwrap(),
                    ),
                    "capacity" => {
                        wire.event(1);
                        wire.event(2);
                    }
                    _ => unreachable!(),
                }
                assert!(wire.request().is_none());
            })
        };
        let cause = match case {
            "correlation" | "ready-credential" => ProductFailure::Correlation,
            "protocol" | "ready-shape" => ProductFailure::Protocol,
            "capacity" => ProductFailure::Capacity,
            "refused" => ProductFailure::ProductRefused,
            _ => ProductFailure::WorkerPanicked,
        };
        let panic_read = Arc::new(AtomicBool::new(false));
        let (session, seen, release) = fixtures::held_cleanup(
            endpoint,
            ProductLimits::new(2, 1, MAX_PROTECTED_RESPONSE_BYTES, 5, 2_000, 2_000, 500).unwrap(),
            panic_read.clone(),
        );
        session.start().unwrap();
        let ticket = if opening {
            None
        } else {
            session.wait_ready().unwrap();
            let ticket = session.request("conversation.list", "{}").unwrap();
            if case == "panic" {
                panic_read.store(true, Ordering::SeqCst);
            }
            Some(ticket)
        };
        receive(&seen);
        assert_eq!(session.failure(), Some(cause), "{case}");
        assert!(matches!(session.request("conversation.list","{}"),Err(failure) if failure==cause));
        session.request_close();
        assert_eq!(session.failure(), Some(cause));
        drop(release);
        assert_eq!(session.close(), Err(cause));
        if let Some(ticket) = ticket {
            assert!(
                matches!(ticket.wait().value(),ProductCompletion::Uncertain(failure)|ProductCompletion::NotDispatched(failure) if *failure==cause)
            );
        } else {
            assert_eq!(session.wait_ready().err(), Some(cause));
            assert!(session.ready().is_none());
        }
        peer.join();
    }
}

#[test]
fn injected_wake_panic_after_admission_returns_the_ticket() {
    struct PanicWake {
        armed: Arc<AtomicBool>,
        inner: ThreadProductWake,
    }
    impl ProductWake for PanicWake {
        fn revision(&self) -> u64 {
            self.inner.revision()
        }
        fn wait(&self, revision: u64, timeout: Duration) {
            self.inner.wait(revision, timeout)
        }
        fn wake(&self) {
            if self.armed.load(Ordering::SeqCst) {
                panic!("injected wake");
            }
            self.inner.wake();
        }
    }
    let armed = Arc::new(AtomicBool::new(false));
    let (release, held) = signal();
    let (endpoint, peer) = peer(move |_wire| receive(&held));
    let session = ProductSession::prepare(
        endpoint.address,
        endpoint.identity,
        endpoint.pin,
        "credential".into(),
        "phone-instance".into(),
        Arc::new(LocalProductConnector::default()),
        Arc::new(fixtures::Time(Instant::now())),
        Arc::new(PanicWake {
            armed: armed.clone(),
            inner: ThreadProductWake::default(),
        }),
        limits(1),
    );
    session.start().unwrap();
    session.wait_ready().unwrap();
    armed.store(true, Ordering::SeqCst);
    let admitted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        session.request("conversation.list", "{}")
    }));
    let ticket = admitted
        .expect("wake panic must not escape request")
        .expect("the admitted ticket is returned");
    assert_eq!(session.failure(), Some(ProductFailure::WorkerPanicked));
    assert!(matches!(
        session.request("conversation.list", "{}"),
        Err(ProductFailure::WorkerPanicked)
    ));
    drop(ticket);
    release.send(()).unwrap();
    assert_eq!(session.close(), Err(ProductFailure::WorkerPanicked));
    peer.join();
}

#[test]
fn injected_wake_revision_panic_is_a_typed_open_failure() {
    struct PanicRevision {
        inner: ThreadProductWake,
    }
    impl ProductWake for PanicRevision {
        fn revision(&self) -> u64 {
            panic!("injected revision");
        }
        fn wait(&self, revision: u64, timeout: Duration) {
            self.inner.wait(revision, timeout)
        }
        fn wake(&self) {
            self.inner.wake()
        }
    }
    let (endpoint, peer, seen, release) = fixtures::held_opening();
    let session = ProductSession::prepare(
        endpoint.address,
        endpoint.identity,
        endpoint.pin,
        "credential".into(),
        "phone-instance".into(),
        Arc::new(LocalProductConnector::default()),
        Arc::new(fixtures::Time(Instant::now())),
        Arc::new(PanicRevision {
            inner: ThreadProductWake::default(),
        }),
        limits(1),
    );
    session.start().unwrap();
    receive(&seen);
    assert!(matches!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| session.wait_ready()))
            .expect("revision panic must not escape wait_ready"),
        Err(ProductFailure::WorkerPanicked)
    ));
    assert!(matches!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| session.events(u64::MAX)))
            .expect("revision panic must not escape events"),
        Err(ProductFailure::WorkerPanicked)
    ));
    assert_eq!(session.failure(), Some(ProductFailure::WorkerPanicked));
    assert_eq!(session.close(), Err(ProductFailure::WorkerPanicked));
    drop(release);
    peer.join();
}

#[test]
fn injected_wake_wait_panic_settles_the_ticket_and_events() {
    struct PanicWait {
        armed: Arc<AtomicBool>,
        inner: ThreadProductWake,
    }
    impl ProductWake for PanicWait {
        fn revision(&self) -> u64 {
            self.inner.revision()
        }
        fn wait(&self, revision: u64, timeout: Duration) {
            if self.armed.load(Ordering::SeqCst) {
                panic!("injected wait");
            }
            self.inner.wait(revision, timeout)
        }
        fn wake(&self) {
            self.inner.wake()
        }
    }
    let armed = Arc::new(AtomicBool::new(false));
    let (release, held) = signal();
    let (endpoint, peer) = peer(move |_wire| receive(&held));
    let session = ProductSession::prepare(
        endpoint.address,
        endpoint.identity,
        endpoint.pin,
        "credential".into(),
        "phone-instance".into(),
        Arc::new(LocalProductConnector::default()),
        Arc::new(fixtures::Time(Instant::now())),
        Arc::new(PanicWait {
            armed: armed.clone(),
            inner: ThreadProductWake::default(),
        }),
        limits(1),
    );
    session.start().unwrap();
    session.wait_ready().unwrap();
    let ticket = session.request("conversation.list", "{}").unwrap();
    armed.store(true, Ordering::SeqCst);
    assert!(matches!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| session.events(u64::MAX)))
            .expect("wait panic must not escape events"),
        Err(ProductFailure::WorkerPanicked)
    ));
    let settlement = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| ticket.wait()))
        .expect("wait panic must not escape ticket wait");
    assert!(matches!(
        settlement.value(),
        ProductCompletion::Uncertain(ProductFailure::WorkerPanicked)
            | ProductCompletion::NotDispatched(ProductFailure::WorkerPanicked)
    ));
    drop(ticket);
    release.send(()).unwrap();
    assert_eq!(session.close(), Err(ProductFailure::WorkerPanicked));
    peer.join();
}

#[test]
fn injected_clock_panic_is_worker_panicked_and_admits_nothing() {
    struct PanicClock {
        armed: Arc<AtomicBool>,
        inner: fixtures::Time,
    }
    impl Clock for PanicClock {
        fn elapsed_ms(&self) -> u64 {
            if self.armed.load(Ordering::SeqCst) {
                panic!("injected clock");
            }
            self.inner.elapsed_ms()
        }
    }
    let armed = Arc::new(AtomicBool::new(false));
    let (release, held) = signal();
    let (endpoint, peer) = peer(move |_wire| receive(&held));
    let session = ProductSession::prepare(
        endpoint.address,
        endpoint.identity,
        endpoint.pin,
        "credential".into(),
        "phone-instance".into(),
        Arc::new(LocalProductConnector::default()),
        Arc::new(PanicClock {
            armed: armed.clone(),
            inner: fixtures::Time(Instant::now()),
        }),
        Arc::new(ThreadProductWake::default()),
        limits(1),
    );
    session.start().unwrap();
    session.wait_ready().unwrap();
    armed.store(true, Ordering::SeqCst);
    assert!(matches!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            session.request("conversation.list", "{}")
        }))
        .expect("clock panic must not escape request"),
        Err(ProductFailure::WorkerPanicked)
    ));
    assert!(matches!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| session.events(u64::MAX)))
            .expect("clock panic must not escape events"),
        Err(ProductFailure::WorkerPanicked)
    ));
    assert!(matches!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| session.events_for(50)))
            .expect("clock panic must not escape events_for"),
        Err(ProductFailure::WorkerPanicked)
    ));
    assert_eq!(session.failure(), Some(ProductFailure::WorkerPanicked));
    release.send(()).unwrap();
    assert_eq!(session.close(), Err(ProductFailure::WorkerPanicked));
    peer.join();
}

#[test]
fn injected_interrupt_panic_stays_inside_stop_and_close_still_joins() {
    struct PanicInterrupt;
    impl ProductInterrupt for PanicInterrupt {
        fn interrupt(&self) {
            panic!("injected interrupt");
        }
    }
    struct Stream {
        inner: Box<dyn ProductStream>,
    }
    impl Read for Stream {
        fn read(&mut self, bytes: &mut [u8]) -> IoResult<usize> {
            self.inner.read(bytes)
        }
    }
    impl Write for Stream {
        fn write(&mut self, bytes: &[u8]) -> IoResult<usize> {
            self.inner.write(bytes)
        }
        fn flush(&mut self) -> IoResult<()> {
            self.inner.flush()
        }
    }
    impl ProductStream for Stream {
        fn read_timeout(&self, timeout: Duration) -> IoResult<()> {
            self.inner.read_timeout(timeout)
        }
        fn write_timeout(&self, timeout: Duration) -> IoResult<()> {
            self.inner.write_timeout(timeout)
        }
        fn shutdown(&self) -> IoResult<()> {
            self.inner.shutdown()
        }
    }
    struct Connector;
    impl ProductConnector for Connector {
        fn connect(&self, address: SocketAddr, timeout: Duration) -> IoResult<ConnectedProduct> {
            let connected = LocalProductConnector::default().connect(address, timeout)?;
            Ok(ConnectedProduct {
                stream: Box::new(Stream {
                    inner: connected.stream,
                }),
                interrupt: Arc::new(PanicInterrupt),
            })
        }
    }
    let (release, held) = signal();
    let (endpoint, peer) = peer(move |_wire| receive(&held));
    let session = ProductSession::prepare(
        endpoint.address,
        endpoint.identity,
        endpoint.pin,
        "credential".into(),
        "phone-instance".into(),
        Arc::new(Connector),
        Arc::new(fixtures::Time(Instant::now())),
        Arc::new(ThreadProductWake::default()),
        limits(1),
    );
    session.start().unwrap();
    session.wait_ready().unwrap();
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| session.request_close())).is_ok()
    );
    assert_eq!(session.failure(), Some(ProductFailure::WorkerPanicked));
    release.send(()).unwrap();
    assert_eq!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| session.close())).unwrap(),
        Err(ProductFailure::WorkerPanicked)
    );
    peer.join();
}

#[test]
fn late_installed_interrupt_panic_upgrades_the_stored_cancel() {
    struct PanicInterrupt;
    impl ProductInterrupt for PanicInterrupt {
        fn interrupt(&self) {
            panic!("late interrupt");
        }
    }
    struct Held {
        started: Sender<()>,
        held: Mutex<Receiver<()>>,
    }
    impl ProductConnector for Held {
        fn connect(&self, address: SocketAddr, timeout: Duration) -> IoResult<ConnectedProduct> {
            self.started.send(()).unwrap();
            receive(&self.held.lock().unwrap());
            let connected = LocalProductConnector::default().connect(address, timeout)?;
            Ok(ConnectedProduct {
                stream: connected.stream,
                interrupt: Arc::new(PanicInterrupt),
            })
        }
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let reader = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        drop(stream);
    });
    let (started, seen) = signal();
    let (release, held) = signal();
    let identity = Arc::new(NativeIdentity::generate(&mut OsEntropy).unwrap());
    let session = ProductSession::prepare(
        address,
        identity.clone(),
        identity.public_spki(),
        "credential".into(),
        "phone-instance".into(),
        Arc::new(Held {
            started,
            held: Mutex::new(held),
        }),
        Arc::new(fixtures::Time(Instant::now())),
        Arc::new(ThreadProductWake::default()),
        limits(1),
    );
    session.start().unwrap();
    receive(&seen);
    session.request_close();
    let other = session.clone();
    let (closed, closed_rx) = signal();
    let closing = thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| other.close()));
        closed.send(()).unwrap();
        result
    });
    assert!(matches!(
        closed_rx.recv_timeout(Duration::from_millis(50)),
        Err(RecvTimeoutError::Timeout)
    ));
    release.send(()).unwrap();
    receive(&closed_rx);
    assert_eq!(
        closing
            .join()
            .expect("close joins")
            .expect("late interrupt panic must not escape close"),
        Err(ProductFailure::WorkerPanicked)
    );
    assert_eq!(session.failure(), Some(ProductFailure::WorkerPanicked));
    reader.join().unwrap();
}

#[test]
fn injected_shutdown_panic_upgrades_the_stored_cancel() {
    struct Stream {
        inner: Box<dyn ProductStream>,
    }
    impl Read for Stream {
        fn read(&mut self, bytes: &mut [u8]) -> IoResult<usize> {
            self.inner.read(bytes)
        }
    }
    impl Write for Stream {
        fn write(&mut self, bytes: &[u8]) -> IoResult<usize> {
            self.inner.write(bytes)
        }
        fn flush(&mut self) -> IoResult<()> {
            self.inner.flush()
        }
    }
    impl ProductStream for Stream {
        fn read_timeout(&self, timeout: Duration) -> IoResult<()> {
            self.inner.read_timeout(timeout)
        }
        fn write_timeout(&self, timeout: Duration) -> IoResult<()> {
            self.inner.write_timeout(timeout)
        }
        fn shutdown(&self) -> IoResult<()> {
            panic!("injected shutdown");
        }
    }
    struct Connector;
    impl ProductConnector for Connector {
        fn connect(&self, address: SocketAddr, timeout: Duration) -> IoResult<ConnectedProduct> {
            let connected = LocalProductConnector::default().connect(address, timeout)?;
            Ok(ConnectedProduct {
                stream: Box::new(Stream {
                    inner: connected.stream,
                }),
                interrupt: connected.interrupt,
            })
        }
    }
    let (release, held) = signal();
    let (endpoint, peer) = peer(move |_wire| receive(&held));
    let session = ProductSession::prepare(
        endpoint.address,
        endpoint.identity,
        endpoint.pin,
        "credential".into(),
        "phone-instance".into(),
        Arc::new(Connector),
        Arc::new(fixtures::Time(Instant::now())),
        Arc::new(ThreadProductWake::default()),
        limits(1),
    );
    session.start().unwrap();
    session.wait_ready().unwrap();
    assert_eq!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| session.close()))
            .expect("shutdown panic must not escape close"),
        Err(ProductFailure::WorkerPanicked)
    );
    assert_eq!(session.failure(), Some(ProductFailure::WorkerPanicked));
    release.send(()).unwrap();
    peer.join();
}

#[test]
fn independent_interrupt_drains_pinned_opening_before_handshake_deadline() {
    let (endpoint, peer, seen, release) = fixtures::held_opening();
    let session = prepare(endpoint, limits(1));
    let release = release;
    session.start().unwrap();
    receive(&seen);
    session.request_close();
    let other = session.clone();
    let (closed, completion) = signal();
    let closing = thread::spawn(move || {
        other.close().unwrap();
        closed.send(()).unwrap();
    });
    assert!(
        completion.recv_timeout(Duration::from_millis(500)).is_ok(),
        "independent stop must interrupt opening rather than wait its two-second deadline"
    );
    assert_eq!(session.wait_ready().err(), Some(ProductFailure::Cancelled));
    assert!(session.ready().is_none());
    drop(release);
    closing.join().unwrap();
    peer.join();
}

#[test]
fn canonical_pin_failure_is_published_before_held_cleanup_and_later_stop() {
    let (endpoint, peer) = fixtures::mismatched_pin_peer();
    let (session, seen, release) =
        fixtures::held_cleanup(endpoint, limits(1), Arc::new(AtomicBool::new(false)));
    session.start().unwrap();
    receive(&seen);
    assert_eq!(session.failure(), Some(ProductFailure::NativeHandshake));
    assert_eq!(
        session.wait_ready().err(),
        Some(ProductFailure::NativeHandshake)
    );
    session.request_close();
    assert_eq!(session.failure(), Some(ProductFailure::NativeHandshake));
    drop(release);
    assert_eq!(session.close(), Err(ProductFailure::NativeHandshake));
    assert!(session.ready().is_none());
    peer.join();
}

#[test]
fn queued_expiry_after_prior_factual_response_is_not_dispatched_and_session_survives() {
    struct Time(AtomicU64);
    impl Clock for Time {
        fn elapsed_ms(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
    }
    struct Wake {
        clock: Arc<Time>,
        armed: AtomicBool,
        inner: ThreadProductWake,
    }
    impl ProductWake for Wake {
        fn revision(&self) -> u64 {
            self.inner.revision()
        }
        fn wait(&self, revision: u64, timeout: Duration) {
            self.inner.wait(revision, timeout)
        }
        fn wake(&self) {
            if self.armed.swap(false, Ordering::SeqCst) {
                self.clock.0.store(100, Ordering::SeqCst);
            }
            self.inner.wake();
        }
    }
    let (sent, seen) = signal();
    let (release, held) = signal();
    let (endpoint, peer) = peer(move |wire| {
        let first = wire.request().unwrap();
        assert_eq!(first.id, "1");
        sent.send(()).unwrap();
        receive(&held);
        wire.response(&first.id);
        let next = wire.request().unwrap();
        assert_eq!(
            next.id, "3",
            "expired queued ticket must have zero wire dispatch"
        );
        wire.response(&next.id);
        assert!(wire.request().is_none());
    });
    let clock = Arc::new(Time(AtomicU64::new(0)));
    let wake = Arc::new(Wake {
        clock: clock.clone(),
        armed: AtomicBool::new(false),
        inner: ThreadProductWake::default(),
    });
    let session = ProductSession::prepare(
        endpoint.address,
        endpoint.identity,
        endpoint.pin,
        "credential".into(),
        "phone-instance".into(),
        Arc::new(LocalProductConnector::default()),
        clock,
        wake.clone(),
        ProductLimits::new(3, 1, MAX_PROTECTED_RESPONSE_BYTES, 5, 50, 2_000, 500).unwrap(),
    );
    session.start().unwrap();
    session.wait_ready().unwrap();
    let first = session.request("conversation.list", "{}").unwrap();
    receive(&seen);
    let queued = session.request("conversation.list", "{}").unwrap();
    wake.armed.store(true, Ordering::SeqCst);
    release.send(()).unwrap();
    assert!(
        matches!(first.wait().value(),ProductCompletion::Response(response) if response.id=="1")
    );
    assert_eq!(
        queued.wait().value(),
        &ProductCompletion::NotDispatched(ProductFailure::TimedOut)
    );
    assert_eq!(session.failure(), None);
    let next = session.request("conversation.list", "{}").unwrap();
    assert_eq!(next.id(), 3);
    assert!(matches!(
        next.wait().value(),
        ProductCompletion::Response(_)
    ));
    session.close().unwrap();
    peer.join();
}

#[test]
fn first_physical_failure_survives_stop_before_error_payload_release() {
    #[derive(Debug)]
    struct Payload {
        entered: Sender<()>,
        held: Mutex<Receiver<()>>,
    }
    impl Display for Payload {
        fn fmt(&self, out: &mut Formatter<'_>) -> FormatResult {
            write!(out, "injected physical failure")
        }
    }
    impl Error for Payload {}
    impl Drop for Payload {
        fn drop(&mut self) {
            self.entered.send(()).unwrap();
            receive(&self.held.lock().unwrap());
        }
    }
    struct Stream {
        inner: Box<dyn ProductStream>,
        enabled: Arc<AtomicBool>,
        payload: Option<Payload>,
    }
    impl Read for Stream {
        fn read(&mut self, bytes: &mut [u8]) -> IoResult<usize> {
            if self.enabled.load(Ordering::SeqCst) {
                if let Some(payload) = self.payload.take() {
                    return Err(IoError::new(ErrorKind::ConnectionReset, payload));
                }
            }
            self.inner.read(bytes)
        }
    }
    impl Write for Stream {
        fn write(&mut self, bytes: &[u8]) -> IoResult<usize> {
            self.inner.write(bytes)
        }
        fn flush(&mut self) -> IoResult<()> {
            self.inner.flush()
        }
    }
    impl ProductStream for Stream {
        fn read_timeout(&self, timeout: Duration) -> IoResult<()> {
            self.inner.read_timeout(timeout)
        }
        fn write_timeout(&self, timeout: Duration) -> IoResult<()> {
            self.inner.write_timeout(timeout)
        }
        fn shutdown(&self) -> IoResult<()> {
            self.inner.shutdown()
        }
    }
    struct Connector {
        enabled: Arc<AtomicBool>,
        payload: Mutex<Option<Payload>>,
    }
    impl ProductConnector for Connector {
        fn connect(&self, address: SocketAddr, timeout: Duration) -> IoResult<ConnectedProduct> {
            let connected = LocalProductConnector::default().connect(address, timeout)?;
            Ok(ConnectedProduct {
                stream: Box::new(Stream {
                    inner: connected.stream,
                    enabled: self.enabled.clone(),
                    payload: self.payload.lock().unwrap().take(),
                }),
                interrupt: connected.interrupt,
            })
        }
    }
    let (endpoint, peer) = peer(|wire| {
        let _ = wire.request();
        assert!(wire.request().is_none());
    });
    let (entered, seen) = signal();
    let (release, held) = signal();
    let enabled = Arc::new(AtomicBool::new(false));
    let session = ProductSession::prepare(
        endpoint.address,
        endpoint.identity,
        endpoint.pin,
        "credential".into(),
        "phone-instance".into(),
        Arc::new(Connector {
            enabled: enabled.clone(),
            payload: Mutex::new(Some(Payload {
                entered,
                held: Mutex::new(held),
            })),
        }),
        Arc::new(fixtures::Time(Instant::now())),
        Arc::new(ThreadProductWake::default()),
        limits(1),
    );
    let release = fixtures::Release(release);
    session.start().unwrap();
    session.wait_ready().unwrap();
    let ticket = session.request("conversation.list", "{}").unwrap();
    enabled.store(true, Ordering::SeqCst);
    receive(&seen);
    assert_eq!(session.failure(), Some(ProductFailure::Closed(None)));
    session.request_close();
    assert_eq!(session.failure(), Some(ProductFailure::Closed(None)));
    drop(release);
    assert_eq!(session.close(), Err(ProductFailure::Closed(None)));
    assert!(
        matches!(ticket.wait().value(),ProductCompletion::Uncertain(cause)|ProductCompletion::NotDispatched(cause) if *cause==ProductFailure::Closed(None))
    );
    peer.join();
}

fn termination(reason: SessionCloseReason, seq: u64) -> OutgoingMessage {
    OutgoingMessage::Event(
        EventFrame::push(
            product_event::SESSION_TERMINATED,
            &SessionTermination {
                code: reason,
                retryable: reason.retryable(),
                retry_after_ms: None,
            },
            seq,
            0,
        )
        .unwrap(),
    )
}

#[test]
fn session_terminated_is_the_close_reason_and_is_not_queued() {
    let (sent, seen) = signal();
    let (endpoint, peer) = peer(move |wire| {
        wire.event(1);
        let request = wire.request().unwrap();
        sent.send(()).unwrap();
        wire.send(termination(SessionCloseReason::CredentialRevoked, 2));
        wire.send(termination(SessionCloseReason::ServerShutdown, 3));
        let _ = request;
    });
    let session = open(endpoint, limits(1));
    let ticket = session
        .request("conversation.list", &json!({}).to_string())
        .unwrap();
    receive(&seen);
    let batch = session.events(u64::MAX).unwrap();
    assert_eq!(batch.len(), 1);
    assert_eq!(batch[0].event, "conversation.listed");
    assert_eq!(
        session.events(u64::MAX).err(),
        Some(ProductFailure::Closed(Some(
            SessionCloseReason::CredentialRevoked
        )))
    );
    assert!(matches!(
        ticket.wait().value(),
        ProductCompletion::Uncertain(ProductFailure::Closed(Some(
            SessionCloseReason::CredentialRevoked
        )))
    ));
    session.close().unwrap_err();
    peer.join();
}

#[test]
fn plain_eof_stays_an_untyped_close_and_a_bad_termination_is_protocol() {
    let (endpoint, closed) = peer(|_| {});
    let session = open(endpoint, limits(1));
    assert_eq!(
        session.events(u64::MAX).err(),
        Some(ProductFailure::Closed(None))
    );
    closed.join();
    let (endpoint, malformed) = peer(|wire| {
        wire.send(OutgoingMessage::Event(
            EventFrame::push(
                product_event::SESSION_TERMINATED,
                &json!({"code": "credential_revoked", "retryable": true}),
                1,
                0,
            )
            .unwrap(),
        ));
    });
    let session = open(endpoint, limits(1));
    assert_eq!(
        session.events(u64::MAX).err(),
        Some(ProductFailure::Protocol)
    );
    malformed.join();
}

#[test]
fn close_and_pending_tickets_agree_when_the_completion_wake_panics() {
    struct Wake {
        armed: AtomicBool,
        calls: AtomicUsize,
        stopped: Sender<()>,
        inner: ThreadProductWake,
    }
    impl ProductWake for Wake {
        fn revision(&self) -> u64 {
            self.inner.revision()
        }
        fn wait(&self, revision: u64, timeout: Duration) {
            self.inner.wait(revision, timeout);
        }
        fn wake(&self) {
            if self.armed.load(Ordering::SeqCst) {
                match self.calls.fetch_add(1, Ordering::SeqCst) {
                    0 => self.stopped.send(()).unwrap(),
                    1 => panic!("completion wake"),
                    _ => {}
                }
            }
            self.inner.wake();
        }
    }
    let (sent, seen) = signal();
    let (endpoint, peer) = peer(move |wire| {
        wire.request().unwrap();
        sent.send(()).unwrap();
        assert!(wire.request().is_none());
    });
    let (stopped, stop_seen) = signal();
    let wake = Arc::new(Wake {
        armed: AtomicBool::new(false),
        calls: AtomicUsize::new(0),
        stopped,
        inner: ThreadProductWake::default(),
    });
    let (session, draining, release) = fixtures::held_cleanup_with_wake(
        endpoint,
        limits(2),
        Arc::new(AtomicBool::new(false)),
        wake.clone(),
    );
    session.start().unwrap();
    session.wait_ready().unwrap();
    let active = session.request("conversation.list", "{}").unwrap();
    receive(&seen);
    let queued = session.request("conversation.list", "{}").unwrap();
    session.request_close();
    receive(&draining);
    wake.armed.store(true, Ordering::SeqCst);
    let other = session.clone();
    let closing = thread::spawn(move || other.close());
    receive(&stop_seen); // close's stop notification succeeded.
    drop(release); // The next notification belongs to physical settlement.
    assert_eq!(closing.join().unwrap(), Err(ProductFailure::WorkerPanicked));
    assert_eq!(
        active.wait().value(),
        &ProductCompletion::Uncertain(ProductFailure::WorkerPanicked)
    );
    assert_eq!(
        queued.wait().value(),
        &ProductCompletion::NotDispatched(ProductFailure::WorkerPanicked)
    );
    peer.join();
}

#[test]
fn wake_failure_waits_for_physical_settlement_without_inventing_dispatch_evidence() {
    struct Wake {
        armed: AtomicBool,
        at_wait: bool,
        observed: Sender<()>,
        inner: ThreadProductWake,
    }
    impl ProductWake for Wake {
        fn revision(&self) -> u64 {
            if !self.at_wait && self.armed.load(Ordering::SeqCst) {
                panic!("waiter revision");
            }
            self.inner.revision()
        }
        fn wait(&self, revision: u64, timeout: Duration) {
            assert!(
                !self.at_wait || !self.armed.load(Ordering::SeqCst),
                "waiter wait"
            );
            self.inner.wait(revision, timeout);
        }
        fn wake(&self) {
            if self.armed.load(Ordering::SeqCst) {
                let _ = self.observed.send(());
            }
            self.inner.wake();
        }
    }
    for at_wait in [false, true] {
        let (sent, seen) = signal();
        let (endpoint, peer) = peer(move |wire| {
            wire.request().unwrap();
            sent.send(()).unwrap();
            assert!(wire.request().is_none());
        });
        let (observed, fault_seen) = signal();
        let wake = Arc::new(Wake {
            armed: AtomicBool::new(false),
            at_wait,
            observed,
            inner: ThreadProductWake::default(),
        });
        let (session, draining, release) = fixtures::held_cleanup_with_wake(
            endpoint,
            limits(2),
            Arc::new(AtomicBool::new(false)),
            wake.clone(),
        );
        session.start().unwrap();
        session.wait_ready().unwrap();
        let active = session.request("conversation.list", "{}").unwrap();
        receive(&seen);
        let queued = session.request("conversation.list", "{}").unwrap();
        session.request_close();
        receive(&draining);
        wake.armed.store(true, Ordering::SeqCst);
        let waiting = active.clone();
        let (settled, settlement) = std::sync::mpsc::channel();
        let waiter = thread::spawn(move || settled.send(waiting.wait()).unwrap());
        receive(&fault_seen);
        assert_eq!(session.failure(), Some(ProductFailure::WorkerPanicked));
        assert_eq!(
            settlement.recv_timeout(Duration::from_millis(50)).err(),
            Some(RecvTimeoutError::Timeout),
            "an observer cannot settle before physical drain"
        );
        assert!(active.completion().is_none());
        assert!(queued.completion().is_none());
        drop(release);
        let result = settlement.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(
            result.value(),
            &ProductCompletion::Uncertain(ProductFailure::WorkerPanicked)
        );
        assert_eq!(
            queued.wait().value(),
            &ProductCompletion::NotDispatched(ProductFailure::WorkerPanicked)
        );
        assert_eq!(session.close(), Err(ProductFailure::WorkerPanicked));
        waiter.join().unwrap();
        peer.join();
    }
}
