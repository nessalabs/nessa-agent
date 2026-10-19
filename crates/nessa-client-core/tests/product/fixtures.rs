//! A real pinned TLS peer whose product replies are controlled independently.
use crate::product::{
    ConnectedProduct, LocalProductConnector, ProductConnector, ProductLimits, ProductSession,
    ProductStream, ProductWake, ThreadProductWake,
};
use nessa_auth::adapters::pairing::{NativeIdentity, NativeTransport, OsEntropy};
use nessa_protocol::clock::Clock;
use nessa_protocol::pairing::{
    encode_frame,
    wire::{decode_request, NativePairingRequest},
    EnrollmentChannel, FrameReader, MAX_PROTECTED_REQUEST_BYTES, MAX_PROTECTED_RESPONSE_BYTES,
};
use nessa_protocol::product::generated::{
    product_event, ProductSessionReady, SessionChallenge, PRODUCT_VERSION,
};
use nessa_protocol::protocol::{EventFrame, OutgoingMessage, RequestFrame, ResponseFrame};
use serde_json::json;
use std::io::{Read, Result as IoResult, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc, Mutex,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub(super) struct Time(pub(super) Instant);
impl Clock for Time {
    fn elapsed_ms(&self) -> u64 {
        self.0.elapsed().as_millis().try_into().unwrap()
    }
}
pub(super) struct Endpoint {
    pub(super) address: SocketAddr,
    pub(super) pin: [u8; 44],
    pub(super) identity: Arc<NativeIdentity>,
}
pub(super) struct Peer(Option<JoinHandle<()>>);
impl Peer {
    pub(super) fn join(mut self) {
        self.0.take().unwrap().join().unwrap();
    }
}
impl Drop for Peer {
    fn drop(&mut self) {
        if let Some(thread) = self.0.take() {
            let result = thread.join();
            if !thread::panicking() {
                assert!(result.is_ok(), "peer panicked");
            }
        }
    }
}
pub(super) struct Wire {
    transport: NativeTransport<TcpStream>,
    frames: FrameReader,
}
impl Wire {
    pub(super) fn raw(&mut self, bytes: &[u8]) {
        self.transport.write_all(bytes).unwrap();
        self.transport.flush().unwrap();
    }
    pub(super) fn send(&mut self, message: OutgoingMessage) {
        self.raw(
            &encode_frame(
                MAX_PROTECTED_RESPONSE_BYTES,
                message.to_wire_text().unwrap().as_bytes(),
            )
            .unwrap(),
        );
    }
    pub(super) fn request(&mut self) -> Option<RequestFrame> {
        loop {
            let count = self.transport.read(self.frames.unfilled().unwrap()).ok()?;
            if count == 0 {
                return None;
            }
            if let Some(body) = self.frames.filled(count).unwrap() {
                return Some(RequestFrame::decode(std::str::from_utf8(&body).unwrap()).unwrap());
            }
        }
    }
    pub(super) fn response(&mut self, id: &str) {
        self.send(OutgoingMessage::Response(
            ResponseFrame::success(id, &json!({"reply":id})).unwrap(),
        ));
    }
    pub(super) fn key_update(&mut self) {
        self.transport.request_key_update().unwrap();
    }
    pub(super) fn event(&mut self, seq: u64) {
        self.send(OutgoingMessage::Event(
            EventFrame::push("conversation.listed", &json!({"seq":seq}), seq, seq).unwrap(),
        ));
    }
}
pub(super) fn ready() -> ProductSessionReady {
    ProductSessionReady {
        version: PRODUCT_VERSION,
        gateway_id: "gateway".into(),
        principal_id: "owner".into(),
        organization_id: "org".into(),
        membership_id: "member".into(),
        credential_id: "credential".into(),
        audience_id: "gateway".into(),
        expires_at: None,
        grants: vec![],
        methods: vec!["conversation.list".into()],
    }
}
pub(super) fn peer(script: impl FnOnce(&mut Wire) + Send + 'static) -> (Endpoint, Peer) {
    peer_ready(ready(), script)
}
pub(super) fn peer_ready(
    actual_ready: ProductSessionReady,
    script: impl FnOnce(&mut Wire) + Send + 'static,
) -> (Endpoint, Peer) {
    native_peer(move |wire| {
        wire.send(OutgoingMessage::Event(
            EventFrame::push(
                product_event::SESSION_CHALLENGE,
                &SessionChallenge {
                    min_version: PRODUCT_VERSION,
                    max_version: PRODUCT_VERSION,
                    nonce: "original-nonce".into(),
                    expires_at: 100,
                },
                0,
                0,
            )
            .unwrap(),
        ));
        let auth = wire.request().unwrap();
        assert_eq!(auth.id, "0");
        assert_eq!(auth.params["nonce"], "original-nonce");
        assert_eq!(auth.params["credential"], "credential");
        assert_eq!(auth.params["client"]["id"], "app.nessa.mobile");
        assert_eq!(auth.params["surface"]["kind"], "panel");
        assert_eq!(auth.params["surface"]["instance"], "phone-instance");
        wire.send(OutgoingMessage::Response(
            ResponseFrame::success("0", &actual_ready).unwrap(),
        ));
        script(wire);
    })
}
fn native_peer(script: impl FnOnce(&mut Wire) + Send + 'static) -> (Endpoint, Peer) {
    let gateway = NativeIdentity::generate(&mut OsEntropy).unwrap();
    let pin = gateway.public_spki();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let thread = thread::spawn(move || {
        let (socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut selector =
            EnrollmentChannel::new(NativeTransport::accept(socket, &gateway).unwrap());
        assert!(matches!(
            decode_request(&selector.receive_envelope().unwrap()).unwrap(),
            NativePairingRequest::OpenProduct
        ));
        let mut wire = Wire {
            transport: selector.into_transport(),
            frames: FrameReader::new(MAX_PROTECTED_REQUEST_BYTES),
        };
        script(&mut wire);
    });
    (
        Endpoint {
            address,
            pin,
            identity: Arc::new(NativeIdentity::generate(&mut OsEntropy).unwrap()),
        },
        Peer(Some(thread)),
    )
}
pub(super) fn refused_peer() -> (Endpoint, Peer) {
    native_peer(|wire| {
        wire.raw(
            &encode_frame(
                MAX_PROTECTED_RESPONSE_BYTES,
                &nessa_protocol::pairing::wire::encode_refused().unwrap(),
            )
            .unwrap(),
        );
        assert!(wire.request().is_none());
    })
}
pub(super) fn prepare(endpoint: Endpoint, limits: ProductLimits) -> ProductSession {
    ProductSession::prepare(
        endpoint.address,
        endpoint.identity,
        endpoint.pin,
        "credential".into(),
        "phone-instance".into(),
        Arc::new(LocalProductConnector),
        Arc::new(Time(Instant::now())),
        Arc::new(ThreadProductWake::default()),
        limits,
    )
}
pub(super) fn open(endpoint: Endpoint, limits: ProductLimits) -> ProductSession {
    let session = prepare(endpoint, limits);
    session.start().unwrap();
    session.wait_ready().unwrap();
    session
}
pub(super) fn signal() -> (mpsc::Sender<()>, mpsc::Receiver<()>) {
    mpsc::channel()
}
pub(super) fn receive(receiver: &mpsc::Receiver<()>) {
    receiver.recv_timeout(Duration::from_secs(3)).unwrap();
}

pub(super) struct Release(pub(super) mpsc::Sender<()>);
impl Drop for Release {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}
struct Cleanup {
    inner: Box<dyn ProductStream>,
    entered: mpsc::Sender<()>,
    held: Mutex<mpsc::Receiver<()>>,
    panic_read: Arc<AtomicBool>,
}
impl Read for Cleanup {
    fn read(&mut self, bytes: &mut [u8]) -> IoResult<usize> {
        assert!(
            !self.panic_read.load(Ordering::SeqCst),
            "injected native product read panic"
        );
        self.inner.read(bytes)
    }
}
impl Write for Cleanup {
    fn write(&mut self, bytes: &[u8]) -> IoResult<usize> {
        self.inner.write(bytes)
    }
    fn flush(&mut self) -> IoResult<()> {
        self.inner.flush()
    }
}
impl ProductStream for Cleanup {
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
struct CleanupConnector {
    entered: mpsc::Sender<()>,
    held: Mutex<Option<mpsc::Receiver<()>>>,
    panic_read: Arc<AtomicBool>,
}
impl ProductConnector for CleanupConnector {
    fn connect(&self, address: SocketAddr, timeout: Duration) -> IoResult<ConnectedProduct> {
        let connected = LocalProductConnector.connect(address, timeout)?;
        Ok(ConnectedProduct {
            stream: Box::new(Cleanup {
                inner: connected.stream,
                entered: self.entered.clone(),
                held: Mutex::new(self.held.lock().unwrap().take().unwrap()),
                panic_read: self.panic_read.clone(),
            }),
            interrupt: connected.interrupt,
        })
    }
}
pub(super) fn held_cleanup(
    endpoint: Endpoint,
    limits: ProductLimits,
    panic_read: Arc<AtomicBool>,
) -> (ProductSession, mpsc::Receiver<()>, Release) {
    held_cleanup_with_wake(
        endpoint,
        limits,
        panic_read,
        Arc::new(ThreadProductWake::default()),
    )
}
pub(super) fn held_cleanup_with_wake(
    endpoint: Endpoint,
    limits: ProductLimits,
    panic_read: Arc<AtomicBool>,
    wake: Arc<dyn ProductWake>,
) -> (ProductSession, mpsc::Receiver<()>, Release) {
    let (entered, seen) = signal();
    let (release, held) = signal();
    let session = ProductSession::prepare(
        endpoint.address,
        endpoint.identity,
        endpoint.pin,
        "credential".into(),
        "phone-instance".into(),
        Arc::new(CleanupConnector {
            entered,
            held: Mutex::new(Some(held)),
            panic_read,
        }),
        Arc::new(Time(Instant::now())),
        wake,
        limits,
    );
    (session, seen, Release(release))
}

pub(super) fn held_opening() -> (Endpoint, Peer, mpsc::Receiver<()>, Release) {
    let (entered, seen) = signal();
    let (release, held) = signal();
    let (endpoint, peer) = native_peer(move |wire| {
        entered.send(()).unwrap();
        receive(&held);
        assert!(wire.request().is_none());
    });
    (endpoint, peer, seen, Release(release))
}

pub(super) fn mismatched_pin_peer() -> (Endpoint, Peer) {
    let gateway = NativeIdentity::generate(&mut OsEntropy).unwrap();
    let wrong_pin = NativeIdentity::generate(&mut OsEntropy)
        .unwrap()
        .public_spki();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let thread = thread::spawn(move || {
        let (socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        assert!(
            NativeTransport::accept(socket, &gateway).is_err(),
            "mismatched pin must not produce native transport"
        );
    });
    (
        Endpoint {
            address,
            pin: wrong_pin,
            identity: Arc::new(NativeIdentity::generate(&mut OsEntropy).unwrap()),
        },
        Peer(Some(thread)),
    )
}
