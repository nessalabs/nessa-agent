//! Actual saved private binding and an independently controlled pinned TLS peer.
use crate::test_support::PrivateFixture;
use nessa_auth::{
    adapters::pairing::{FilePairingState, NativeIdentity, NativeTransport},
    application::pairing::{ClientPendingStore, PrivateKeyMaterial},
    domain::{
        pairing::{AttemptId, ConsentClass, ConsentIntentId, InvitationId, PublicIntent},
        CredentialId, ResourceId,
    },
};
use nessa_client_core::product::{
    ConnectedProduct, LocalProductConnector, ProductConnector, ProductStream,
};
use nessa_local_storage::create_directory_beneath;
use nessa_protocol::{
    pairing::{
        encode_frame,
        wire::{decode_request, NativePairingRequest},
        EnrollmentChannel, FrameReader, MAX_PROTECTED_REQUEST_BYTES, MAX_PROTECTED_RESPONSE_BYTES,
    },
    product::generated::{product_event, ProductSessionReady, SessionChallenge, PRODUCT_VERSION},
    protocol::{EventFrame, OutgoingMessage, RequestFrame, ResponseFrame},
};
use std::{
    io::{Read, Result as IoResult, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::Path,
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
    time::Duration,
};
use zeroize::Zeroizing;

pub(crate) fn identity(seed: u8) -> NativeIdentity {
    NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([seed; 32]))).unwrap()
}
pub(crate) fn intent() -> PublicIntent {
    PublicIntent::new(
        InvitationId::new([1; 16]),
        AttemptId::new([2; 16]),
        ConsentIntentId::new([3; 16]),
        u64::MAX,
        u64::MAX,
        ConsentClass::DeviceRead,
    )
    .unwrap()
}
pub(crate) fn root(issued: bool) -> PrivateFixture {
    let root = PrivateFixture::new();
    create_directory_beneath(root.path(), Path::new("private")).unwrap();
    let state = FilePairingState::open(root.path(), Path::new("private")).unwrap();
    state
        .save_pending(
            identity(7).key_material(),
            &identity(8).public_spki(),
            intent(),
            None,
        )
        .unwrap();
    if issued {
        state
            .save_credential(
                &CredentialId::new("issued").unwrap(),
                &ResourceId::new("receiver").unwrap(),
                intent(),
            )
            .unwrap();
    }
    root
}
pub(crate) struct Held {
    state: Mutex<(bool, bool)>,
    changed: Condvar,
}
impl Held {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new((false, false)),
            changed: Condvar::new(),
        })
    }
    pub(crate) fn hold(&self) {
        let mut state = self.state.lock().unwrap();
        state.0 = true;
        self.changed.notify_all();
        while !state.1 {
            state = self.changed.wait(state).unwrap();
        }
    }
    pub(crate) fn entered(&self) {
        let (state, timeout) = self
            .changed
            .wait_timeout_while(self.state.lock().unwrap(), Duration::from_secs(3), |s| !s.0)
            .unwrap();
        assert!(!timeout.timed_out() && state.0, "physical gate entered");
    }
    pub(crate) fn release(&self) {
        self.state.lock().unwrap().1 = true;
        self.changed.notify_all();
    }
}
pub(crate) struct Peer(Option<JoinHandle<()>>);
impl Peer {
    pub(crate) fn join(mut self) {
        self.0.take().unwrap().join().unwrap();
    }
}
impl Drop for Peer {
    fn drop(&mut self) {
        if let Some(thread) = self.0.take() {
            let result = thread.join();
            if !thread::panicking() {
                assert!(result.is_ok(), "TLS peer panicked");
            }
        }
    }
}
pub(crate) struct Wire {
    transport: NativeTransport<TcpStream>,
    frames: FrameReader,
}
impl Wire {
    pub(crate) fn send(&mut self, message: OutgoingMessage) {
        self.raw(&message.to_wire_text().unwrap());
    }
    pub(crate) fn raw(&mut self, text: &str) {
        self.transport
            .write_all(&encode_frame(MAX_PROTECTED_RESPONSE_BYTES, text.as_bytes()).unwrap())
            .unwrap();
        self.transport.flush().unwrap();
    }
    pub(crate) fn request(&mut self) -> Option<RequestFrame> {
        loop {
            let count = self.transport.read(self.frames.unfilled().unwrap()).ok()?;
            if count == 0 {
                return None;
            }
            if let Some(frame) = self.frames.filled(count).unwrap() {
                return Some(RequestFrame::decode(std::str::from_utf8(&frame).unwrap()).unwrap());
            }
        }
    }
}
pub(crate) fn peer(script: impl FnOnce(&mut Wire) + Send + 'static) -> (SocketAddr, Peer) {
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
        let transport = NativeTransport::accept(socket, &identity(8)).unwrap();
        assert_eq!(
            transport.device_proof().key(),
            identity(7).device_key(),
            "actual saved private key proves original device"
        );
        let mut selector = EnrollmentChannel::new(transport);
        assert!(matches!(
            decode_request(&selector.receive_envelope().unwrap()).unwrap(),
            NativePairingRequest::OpenProduct
        ));
        let mut wire = protected_wire(selector.into_transport());
        script(&mut wire);
    });
    (address, Peer(Some(thread)))
}
pub(crate) struct HeldConnector {
    pub(crate) gate: Arc<Held>,
}
impl ProductConnector for HeldConnector {
    fn connect(&self, address: SocketAddr, timeout: Duration) -> IoResult<ConnectedProduct> {
        self.gate.hold();
        LocalProductConnector::default().connect(address, timeout)
    }
}
pub(crate) struct CleanupConnector {
    pub(crate) gate: Arc<Held>,
}
struct Cleanup {
    inner: Box<dyn ProductStream>,
    gate: Arc<Held>,
}
impl Read for Cleanup {
    fn read(&mut self, bytes: &mut [u8]) -> IoResult<usize> {
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
        self.gate.hold();
        self.inner.shutdown()
    }
}
impl ProductConnector for CleanupConnector {
    fn connect(&self, address: SocketAddr, timeout: Duration) -> IoResult<ConnectedProduct> {
        let connected = LocalProductConnector::default().connect(address, timeout)?;
        Ok(ConnectedProduct {
            stream: Box::new(Cleanup {
                inner: connected.stream,
                gate: self.gate.clone(),
            }),
            interrupt: connected.interrupt,
        })
    }
}

/// Shared actual protected handshake fixture after the canonical OpenProduct selector.
pub(crate) fn protected_wire(transport: NativeTransport<TcpStream>) -> Wire {
    let mut wire = Wire {
        transport,
        frames: FrameReader::new(MAX_PROTECTED_REQUEST_BYTES),
    };
    wire.send(OutgoingMessage::Event(
        EventFrame::push(
            product_event::SESSION_CHALLENGE,
            &SessionChallenge {
                min_version: PRODUCT_VERSION,
                max_version: PRODUCT_VERSION,
                nonce: "original-nonce".into(),
                expires_at: 10_000,
            },
            0,
            0,
        )
        .unwrap(),
    ));
    let auth = wire.request().unwrap();
    assert_eq!(auth.id, "0");
    assert_eq!(auth.params["credential"], "issued");
    assert_eq!(auth.params["nonce"], "original-nonce");
    assert_eq!(auth.params["client"]["id"], "app.nessa.mobile");
    assert_eq!(auth.params["surface"]["instance"], "phone-instance");
    let ready = ProductSessionReady {
        version: PRODUCT_VERSION,
        gateway_id: "gateway".into(),
        principal_id: "owner".into(),
        organization_id: "org".into(),
        membership_id: "member".into(),
        credential_id: "issued".into(),
        audience_id: "gateway".into(),
        expires_at: None,
        grants: vec![],
        methods: vec!["conversation.list".into()],
    };
    wire.send(OutgoingMessage::Response(
        ResponseFrame::success("0", &ready).unwrap(),
    ));
    wire
}
