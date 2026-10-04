//! The client session against a real native TLS peer: the gateway key pinned,
//! the device key presented, `openProduct`, then product frames.
use super::super::session::{DeviceEvidence, LocalConnector, RpcKind, Session};
use super::super::sources::GatewayConnection;
use crate::conversation::domain::{conversation_catalogue_stream, ConversationId};
use crate::conversation::infrastructure::{conversation_catalogue_schema, NessaCatalogueSource};
use crate::read_only_sync::application::{
    Cancellation, GatewayConnector, GatewayError, GatewayPolicy, GatewayStream,
};
use nessa_auth::adapters::pairing::{NativeIdentity, NativeTransport, OsEntropy};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::clock::Clock;
use nessa_protocol::pairing::{
    encode_frame,
    wire::{decode_request, encode_refused, NativePairingRequest},
    EnrollmentChannel, FrameReader, MAX_PROTECTED_REQUEST_BYTES,
};
use nessa_protocol::product::catalogue_read::wire_descriptor;
use nessa_protocol::product::generated::{
    product_event, product_method, ProductSessionReady, SessionChallenge,
    MAX_AUTH_CREDENTIAL_CHARACTERS, MAX_PRODUCT_CLIENT_ID_CHARACTERS, MAX_RECORD_RESPONSE_BYTES,
    PRODUCT_VERSION,
};
use nessa_protocol::product::passive_read::wire_scope;
use nessa_protocol::product_contract::generated::{
    CatalogueReadErrorCode, RecordReadErrorCode, SessionCloseReason,
};
use nessa_protocol::protocol::{EventFrame, OutgoingMessage, RequestFrame, ResponseFrame};
use nessa_sync::replication::application::{Access, ScopeAuthorizer};
use nessa_sync::replication::catalogue::{
    CataloguePass, CatalogueSource, CatalogueSourceError, EntryKey, ManifestEntry, ManifestRequest,
};
use nessa_sync::replication::domain::{Id, Scope};
use serde_json::json;
use std::io::{ErrorKind, Read, Result as IoResult, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle, Result as ThreadResult};
use std::time::{Duration, Instant};

struct Time(Instant);
impl Clock for Time {
    fn elapsed_ms(&self) -> u64 {
        self.0.elapsed().as_millis() as u64
    }
}
struct Never;
impl Cancellation for Never {
    fn cancelled(&self) -> bool {
        false
    }
}
// One test owner joins even when the caller assertion unwinds.
struct Peer(Option<JoinHandle<()>>);
impl Peer {
    fn spawn(execute: impl FnOnce() + Send + 'static) -> Self {
        Self(Some(thread::spawn(execute)))
    }
    fn join(mut self) -> ThreadResult<()> {
        self.0.take().unwrap().join()
    }
}
impl Drop for Peer {
    fn drop(&mut self) {
        if let Some(peer) = self.0.take() {
            let result = peer.join();
            if !thread::panicking() {
                assert!(result.is_ok(), "test peer panicked");
            }
        }
    }
}
fn bounded_accept(listener: &TcpListener) -> (TcpStream, SocketAddr) {
    listener.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match listener.accept() {
            Ok(accepted) => {
                accepted.0.set_nonblocking(false).unwrap();
                accepted
                    .0
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                accepted
                    .0
                    .set_write_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                return accepted;
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(1));
            }
            Err(error) => panic!("test peer accept failed: {error}"),
        }
    }
}
fn ready() -> ProductSessionReady {
    ProductSessionReady {
        version: PRODUCT_VERSION,
        gateway_id: "gateway".into(),
        principal_id: "owner".into(),
        organization_id: "org".into(),
        membership_id: "membership".into(),
        credential_id: "credential".into(),
        audience_id: "gateway".into(),
        expires_at: None,
        grants: vec![],
        methods: vec![],
    }
}
/// The gateway side of one protected connection, after `openProduct`.
struct PeerSocket {
    transport: NativeTransport<TcpStream>,
    frames: FrameReader,
}
impl PeerSocket {
    fn write_raw(&mut self, bytes: &[u8]) {
        self.transport.write_all(bytes).unwrap();
        self.transport.flush().unwrap();
    }
    fn read_text(&mut self) -> Option<String> {
        loop {
            let buffer = self.frames.unfilled().ok()?;
            let count = self
                .transport
                .read(buffer)
                .ok()
                .filter(|count| *count > 0)?;
            if let Some(body) = self.frames.filled(count).ok()? {
                return String::from_utf8(body).ok();
            }
        }
    }
    /// The client closed its side: no further frame arrives.
    fn ended(&mut self) -> bool {
        self.read_text().is_none()
    }
}
fn send(socket: &mut PeerSocket, message: OutgoingMessage) {
    let text = message.to_wire_text().unwrap();
    let frame = encode_frame(MAX_RECORD_RESPONSE_BYTES, text.as_bytes()).unwrap();
    socket.write_raw(&frame);
}
fn request(socket: &mut PeerSocket) -> RequestFrame {
    RequestFrame::decode(&socket.read_text().expect("text request")).unwrap()
}
/// Where and how a test client dials its peer: the address, the gateway key
/// it pins, and its own device key.
struct Endpoint {
    address: SocketAddr,
    pin: [u8; 44],
    device: NativeIdentity,
}
impl Endpoint {
    fn evidence<'a>(&'a self, credential: &'a str) -> DeviceEvidence<'a> {
        DeviceEvidence {
            identity: &self.device,
            pin: self.pin,
            credential,
        }
    }
}
fn challenge(socket: &mut PeerSocket) {
    send(
        socket,
        OutgoingMessage::Event(
            EventFrame::push(
                product_event::SESSION_CHALLENGE,
                &SessionChallenge {
                    min_version: PRODUCT_VERSION,
                    max_version: PRODUCT_VERSION,
                    nonce: "nonce".into(),
                    expires_at: 100,
                },
                0,
                0,
            )
            .unwrap(),
        ),
    );
}
/// A native peer that answers `openProduct` with the scripted exchange only.
fn native_peer(script: impl FnOnce(&mut PeerSocket) + Send + 'static) -> (Endpoint, Peer) {
    native_peer_answering(None, script)
}
/// A native peer that answers `openProduct` with `reply`, an enrollment
/// envelope written as the gateway's `write_reply` writes it, before the
/// scripted product exchange.
fn native_peer_answering(
    reply: Option<Vec<u8>>,
    script: impl FnOnce(&mut PeerSocket) + Send + 'static,
) -> (Endpoint, Peer) {
    let gateway = NativeIdentity::generate(&mut OsEntropy).unwrap();
    let pin = gateway.public_spki();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let thread = Peer::spawn(move || {
        let (stream, _) = bounded_accept(&listener);
        let transport = NativeTransport::accept(stream, &gateway).unwrap();
        let mut selector = EnrollmentChannel::new(transport);
        let first = selector.receive_envelope().unwrap();
        assert!(matches!(
            decode_request(&first).unwrap(),
            NativePairingRequest::OpenProduct
        ));
        if let Some(reply) = reply {
            selector.send_envelope(&reply).unwrap();
        }
        let mut socket = PeerSocket {
            transport: selector.into_transport(),
            frames: FrameReader::new(MAX_PROTECTED_REQUEST_BYTES),
        };
        script(&mut socket);
    });
    (
        Endpoint {
            address,
            pin,
            device: NativeIdentity::generate(&mut OsEntropy).unwrap(),
        },
        thread,
    )
}
/// A native peer that authenticates the client, then runs `script`.
fn peer(script: impl FnOnce(&mut PeerSocket) + Send + 'static) -> (Endpoint, Peer) {
    native_peer(move |socket| {
        challenge(socket);
        let authentication = request(socket);
        assert_eq!(authentication.method, product_method::SESSION_AUTHENTICATE);
        assert_eq!(authentication.params["nonce"], "nonce");
        assert_eq!(authentication.params["credential"], "credential");
        send(
            socket,
            OutgoingMessage::Response(
                ResponseFrame::success(&authentication.id, &ready()).unwrap(),
            ),
        );
        script(socket);
    })
}
fn policy() -> GatewayPolicy {
    GatewayPolicy::new(2000, 1000, 2).unwrap()
}
fn connect(endpoint: &Endpoint) -> Session {
    Session::connect(
        endpoint.address,
        endpoint.evidence("credential"),
        "example",
        &LocalConnector,
        Arc::new(Time(Instant::now())),
        Arc::new(Never),
        policy(),
    )
    .unwrap()
}
#[test]
fn handshake_correlated_passive_rpc_and_consumed_operation_outcome() {
    let (endpoint, peer) = peer(|socket| {
        let read = request(socket);
        assert_eq!(read.method, product_method::CONVERSATION_RECORDS_HEAD);
        send(
            socket,
            OutgoingMessage::Response(
                ResponseFrame::success(&read.id, &json!({"ok":"actual"})).unwrap(),
            ),
        );
    });
    let mut session = connect(&endpoint);
    assert_eq!(session.ready().gateway_id, "gateway");
    let operation = session.begin().unwrap();
    assert_eq!(
        session
            .rpc(
                product_method::CONVERSATION_RECORDS_HEAD,
                &json!({}),
                RpcKind::Record
            )
            .unwrap(),
        json!({"ok":"actual"})
    );
    let outcome = session.finish().unwrap();
    assert_eq!(outcome.operation, operation);
    assert_eq!(outcome.failure, None);
    assert!(matches!(session.finish(), Err(GatewayError::Busy)));
    drop(session);
    peer.join().unwrap();
}
#[test]
fn wrong_correlation_closes_connection_and_keeps_one_first_cause() {
    let (endpoint, peer) = peer(move |socket| {
        request(socket);
        send(
            socket,
            OutgoingMessage::Response(ResponseFrame::success("other", &json!({})).unwrap()),
        );
    });
    let mut session = connect(&endpoint);
    session.begin().unwrap();
    for _ in 0..2 {
        assert_eq!(
            session.rpc(
                product_method::CONVERSATION_RECORDS_HEAD,
                &json!({}),
                RpcKind::Record
            ),
            Err(GatewayError::Correlation)
        );
    }
    assert_eq!(
        session.fail(GatewayError::Protocol),
        GatewayError::Correlation
    );
    assert_eq!(
        session.finish().unwrap().failure,
        Some(GatewayError::Correlation)
    );
    assert_eq!(session.begin(), Err(GatewayError::Transport));
    drop(session);
    peer.join().unwrap();
}
/// Row W17: `source_preparing` fails its operation with one first cause, but
/// the connection stays usable, so the retried pass's next operation reaches
/// the gateway on the same session.
#[test]
fn preparing_fails_the_operation_and_keeps_the_session_for_the_next() {
    let preparing = GatewayError::Record(RecordReadErrorCode::SourcePreparing);
    let (endpoint, peer) = peer(move |socket| {
        let read = request(socket);
        send(
            socket,
            OutgoingMessage::Response(ResponseFrame::failure(
                &read.id,
                RecordReadErrorCode::SourcePreparing.as_str(),
                "temporary",
            )),
        );
        let read = request(socket);
        assert_eq!(read.method, product_method::CONVERSATION_RECORDS_HEAD);
        send(
            socket,
            OutgoingMessage::Response(
                ResponseFrame::success(&read.id, &json!({"ok":"ready"})).unwrap(),
            ),
        );
    });
    let mut session = connect(&endpoint);
    let first = session.begin().unwrap();
    for _ in 0..2 {
        assert_eq!(
            session.rpc(
                product_method::CONVERSATION_RECORDS_HEAD,
                &json!({}),
                RpcKind::Record
            ),
            Err(preparing)
        );
    }
    assert_eq!(session.fail(preparing), preparing);
    let outcome = session.finish().unwrap();
    assert_eq!(outcome.operation, first);
    assert_eq!(outcome.failure, Some(preparing));
    let second = session.begin().unwrap();
    assert_ne!(second, first);
    assert_eq!(
        session
            .rpc(
                product_method::CONVERSATION_RECORDS_HEAD,
                &json!({}),
                RpcKind::Record
            )
            .unwrap(),
        json!({"ok":"ready"})
    );
    assert_eq!(session.finish().unwrap().failure, None);
    drop(session);
    peer.join().unwrap();
}
#[test]
fn authorizer_calls_actual_head_each_time_and_returns_changed_actual_scope() {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let scope = Scope::new(
        Id::new("receiver").unwrap(),
        Id::new("gateway").unwrap(),
        Id::new("8e024fc9-0c9d-4952-8427-bcb2b3b07f8f").unwrap(),
        Id::new("incarnation").unwrap(),
        nessa_sdk::infrastructure::session_storage::physical_record_schema(),
        Id::new("epoch-3").unwrap(),
    );
    let actual = scope.clone();
    let (endpoint, peer) = peer(move |socket| {
        for _ in 0..2 {
            let read = request(socket);
            count.fetch_add(1, Ordering::SeqCst);
            assert_eq!(read.params["accessEpoch"], "3");
            send(
                socket,
                OutgoingMessage::Response(
                    ResponseFrame::success(
                        &read.id,
                        &json!({"scope":wire_scope(&actual),"head":"9"}),
                    )
                    .unwrap(),
                ),
            );
        }
    });
    let connection = GatewayConnection::new(connect(&endpoint));
    let source = connection.records(
        Id::new("receiver").unwrap(),
        3,
        ConversationId::new(scope.stream().as_str()).unwrap(),
    );
    let mut authorizer = source.authorizer();
    connection.begin().unwrap();
    let saved = Scope::new(
        scope.receiver().clone(),
        scope.origin().clone(),
        scope.stream().clone(),
        Id::new("old").unwrap(),
        scope.schema().clone(),
        scope.access_epoch().clone(),
    );
    assert_eq!(authorizer.authorize(&saved), Access::Allowed(scope.clone()));
    assert_eq!(authorizer.authorize(&saved), Access::Allowed(scope));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(connection.finish().unwrap().failure, None);
    drop(source);
    drop(authorizer);
    drop(connection);
    peer.join().unwrap();
}

#[test]
fn passive_authorizer_preserves_temporary_and_permanent_access_meaning() {
    for catalogue in [false, true] {
        for (record_code, catalogue_code, expected) in [
            (
                RecordReadErrorCode::Unverifiable,
                CatalogueReadErrorCode::Unverifiable,
                Access::Unverifiable,
            ),
            (
                RecordReadErrorCode::Unauthorized,
                CatalogueReadErrorCode::Unauthorized,
                Access::Denied,
            ),
            (
                RecordReadErrorCode::Forbidden,
                CatalogueReadErrorCode::Forbidden,
                Access::Denied,
            ),
        ] {
            assert_eq!(record_code.as_str(), catalogue_code.as_str());
            let code = record_code.as_str();
            let scope = Scope::new(
                Id::new("receiver").unwrap(),
                Id::new("gateway").unwrap(),
                Id::new("8e024fc9-0c9d-4952-8427-bcb2b3b07f8f").unwrap(),
                Id::new("incarnation").unwrap(),
                nessa_sdk::infrastructure::session_storage::physical_record_schema(),
                Id::new("epoch-3").unwrap(),
            );
            let (endpoint, peer) = peer(move |socket| {
                let read = request(socket);
                assert_eq!(
                    read.method,
                    if catalogue {
                        product_method::CONVERSATION_CATALOGUE_HEAD
                    } else {
                        product_method::CONVERSATION_RECORDS_HEAD
                    }
                );
                send(
                    socket,
                    OutgoingMessage::Response(ResponseFrame::failure(&read.id, code, "refused")),
                );
            });
            let connection = GatewayConnection::new(connect(&endpoint));
            let mut authorizer = if catalogue {
                connection
                    .catalogue(Id::new("receiver").unwrap(), 3)
                    .authorizer()
            } else {
                connection
                    .records(
                        Id::new("receiver").unwrap(),
                        3,
                        ConversationId::new(scope.stream().as_str()).unwrap(),
                    )
                    .authorizer()
            };
            connection.begin().unwrap();
            let actual = authorizer.authorize(&scope);
            let outcome = connection.finish().unwrap();
            drop(authorizer);
            drop(connection);
            peer.join().unwrap();
            assert_eq!(actual, expected);
            assert_eq!(
                outcome.failure,
                Some(if catalogue {
                    GatewayError::Catalogue(catalogue_code)
                } else {
                    GatewayError::Record(record_code)
                })
            );
        }
    }
}

#[test]
fn driver_panic_and_returned_value_have_owned_outcomes_and_physical_peer_close() {
    let (endpoint, peer) = peer(|socket| {
        assert!(socket.ended());
    });
    let connection = GatewayConnection::new(connect(&endpoint));
    let attempt = connection
        .run(|| panic!("injected driver failure"))
        .unwrap();
    assert!(attempt.result.is_none());
    assert_eq!(attempt.outcome.failure, Some(GatewayError::DriverPanicked));
    assert!(matches!(connection.begin(), Err(GatewayError::Transport)));
    drop(connection);
    peer.join().unwrap();
}

/// Row PR9 (client): an unrequested event flood and an announced frame above
/// the response bound end the attempt with their typed cause; the oversized
/// body is never read.
#[test]
fn bounded_event_and_frame_refusals_preserve_actual_typed_cause() {
    for expected in [GatewayError::EventCapacity, GatewayError::ResponseTooLarge] {
        let (endpoint, peer) = peer(move |socket| {
            let _ = request(socket);
            match expected {
                GatewayError::EventCapacity => {
                    for _ in 0..3 {
                        send(
                            socket,
                            OutgoingMessage::Event(
                                EventFrame::push("unobserved", &json!({}), 0, 0).unwrap(),
                            ),
                        );
                    }
                }
                _ => {
                    let announced = u32::try_from(MAX_RECORD_RESPONSE_BYTES + 1).unwrap();
                    socket.write_raw(&announced.to_be_bytes());
                }
            }
            while !socket.ended() {}
        });
        let mut session = connect(&endpoint);
        session.begin().unwrap();
        assert_eq!(
            session.rpc(
                product_method::CONVERSATION_RECORDS_HEAD,
                &json!({}),
                RpcKind::Record
            ),
            Err(expected)
        );
        assert_eq!(session.finish().unwrap().failure, Some(expected));
        drop(session);
        peer.join().unwrap();
    }
}

/// Row PR9 (client): a frame cut short by the gateway's close is an untyped
/// close of this attempt, not a response.
#[test]
fn truncated_frame_is_an_untyped_close() {
    let (endpoint, peer) = peer(|socket| {
        let _ = request(socket);
        socket.write_raw(&100u32.to_be_bytes());
        socket.write_raw(b"{\"type\":\"res\"");
    });
    let mut session = connect(&endpoint);
    session.begin().unwrap();
    assert_eq!(
        session.rpc(
            product_method::CONVERSATION_RECORDS_HEAD,
            &json!({}),
            RpcKind::Record
        ),
        Err(GatewayError::Closed(None))
    );
    drop(session);
    peer.join().unwrap();
}

#[test]
fn catalogue_identity_owner_rejects_other_organization_principal_and_schema() {
    let org = OrganizationId::new("org").unwrap();
    let owner = PrincipalId::new("owner").unwrap();
    let scope = Scope::new(
        Id::new("receiver").unwrap(),
        Id::new("gateway").unwrap(),
        conversation_catalogue_stream(&org, &owner),
        Id::new("incarnation").unwrap(),
        conversation_catalogue_schema(),
        Id::new("epoch-3").unwrap(),
    );
    assert!(NessaCatalogueSource::check_scope_identity(&org, &owner, &scope).is_ok());
    assert!(NessaCatalogueSource::check_scope_identity(
        &OrganizationId::new("other").unwrap(),
        &owner,
        &scope
    )
    .is_err());
    assert!(NessaCatalogueSource::check_scope_identity(
        &org,
        &PrincipalId::new("other").unwrap(),
        &scope
    )
    .is_err());
    let wrong = Scope::new(
        scope.receiver().clone(),
        scope.origin().clone(),
        scope.stream().clone(),
        scope.incarnation().clone(),
        Id::new("other").unwrap(),
        scope.access_epoch().clone(),
    );
    assert!(NessaCatalogueSource::check_scope_identity(&org, &owner, &wrong).is_err());
}

/// A gateway presenting any key but the pinned one is refused by TLS before
/// `openProduct` or any credential is sent.
#[test]
fn another_gateway_key_is_refused_before_any_product_frame() {
    let pinned = NativeIdentity::generate(&mut OsEntropy)
        .unwrap()
        .public_spki();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let served = NativeIdentity::generate(&mut OsEntropy).unwrap();
    let peer = Peer::spawn(move || {
        let (stream, _) = bounded_accept(&listener);
        assert!(NativeTransport::accept(stream, &served).is_err());
    });
    let endpoint = Endpoint {
        address,
        pin: pinned,
        device: NativeIdentity::generate(&mut OsEntropy).unwrap(),
    };
    let result = Session::connect(
        endpoint.address,
        endpoint.evidence("credential"),
        "example",
        &LocalConnector,
        Arc::new(Time(Instant::now())),
        Arc::new(Never),
        policy(),
    );
    assert!(matches!(result, Err(GatewayError::NativeHandshake)));
    peer.join().unwrap();
}

#[test]
fn challenge_scalar_refusal_happens_before_any_authentication_request() {
    for nonce in [String::new(), "x".repeat(257)] {
        let (endpoint, peer) = native_peer(move |socket| {
            send(
                socket,
                OutgoingMessage::Event(
                    EventFrame::push(
                        product_event::SESSION_CHALLENGE,
                        &json!({"minVersion":1,"maxVersion":1,"nonce":nonce,"expiresAt":100}),
                        0,
                        0,
                    )
                    .unwrap(),
                ),
            );
            assert!(socket.ended());
        });
        assert!(matches!(
            Session::connect(
                endpoint.address,
                endpoint.evidence("credential"),
                "example",
                &LocalConnector,
                Arc::new(Time(Instant::now())),
                Arc::new(Never),
                policy()
            ),
            Err(GatewayError::Protocol)
        ));
        peer.join().unwrap();
    }
}

/// Rows PR5, PR15 (client): the enrollment `Refused` reply answering
/// `openProduct` is the typed `ProductRefused`, which asks pinned status
/// again (row PC5). Any other first frame that is not a product message, and
/// a `Refused` envelope arriving after product messages began, stay
/// `Protocol`: only the opening frame is read through the enrollment decoder.
#[test]
fn open_product_refusal_is_typed_and_other_frames_stay_protocol() {
    let refused = encode_refused().unwrap();
    let (endpoint, peer) = native_peer_answering(Some(refused.clone()), |socket| {
        assert!(socket.ended());
    });
    let result = Session::connect(
        endpoint.address,
        endpoint.evidence("credential"),
        "example",
        &LocalConnector,
        Arc::new(Time(Instant::now())),
        Arc::new(Never),
        policy(),
    );
    assert!(matches!(result, Err(GatewayError::ProductRefused)));
    peer.join().unwrap();

    let (endpoint, peer) =
        native_peer_answering(Some(b"{\"kind\":\"unknown\"}".to_vec()), |socket| {
            assert!(socket.ended());
        });
    assert!(matches!(
        Session::connect(
            endpoint.address,
            endpoint.evidence("credential"),
            "example",
            &LocalConnector,
            Arc::new(Time(Instant::now())),
            Arc::new(Never),
            policy()
        ),
        Err(GatewayError::Protocol)
    ));
    peer.join().unwrap();

    let (endpoint, peer) = native_peer(move |socket| {
        challenge(socket);
        let _ = request(socket);
        socket.write_raw(&encode_frame(MAX_RECORD_RESPONSE_BYTES, &refused).unwrap());
        assert!(socket.ended());
    });
    assert!(matches!(
        Session::connect(
            endpoint.address,
            endpoint.evidence("credential"),
            "example",
            &LocalConnector,
            Arc::new(Time(Instant::now())),
            Arc::new(Never),
            policy()
        ),
        Err(GatewayError::Protocol)
    ));
    peer.join().unwrap();
}

#[test]
fn authentication_temporary_permanent_and_unknown_refusals_consume_the_shared_close_policy() {
    for (code, expected) in [
        (
            "temporarily_unavailable",
            SessionCloseReason::TemporaryUnavailable,
        ),
        ("unauthorized", SessionCloseReason::AuthenticationFailed),
        ("unknown", SessionCloseReason::AuthenticationFailed),
    ] {
        let (endpoint, peer) = native_peer(move |socket| {
            challenge(socket);
            let auth = request(socket);
            send(
                socket,
                OutgoingMessage::Response(ResponseFrame::failure(&auth.id, code, "sanitized")),
            );
            assert!(socket.ended());
        });
        assert!(
            matches!(Session::connect(endpoint.address,endpoint.evidence("credential"),"example",&LocalConnector,Arc::new(Time(Instant::now())),Arc::new(Never),policy()),Err(GatewayError::Authentication(reason)) if reason==expected)
        );
        peer.join().unwrap();
    }
}

#[test]
fn borrowed_authentication_scalar_acquisition_is_bounded_before_connect() {
    struct RefuseConnect(AtomicUsize);
    impl GatewayConnector for RefuseConnect {
        fn connect(&self, _: SocketAddr, _: Duration) -> IoResult<Box<dyn GatewayStream>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(ErrorKind::ConnectionRefused.into())
        }
    }
    let endpoint = Endpoint {
        address: "127.0.0.1:8000".parse().unwrap(),
        pin: NativeIdentity::generate(&mut OsEntropy)
            .unwrap()
            .public_spki(),
        device: NativeIdentity::generate(&mut OsEntropy).unwrap(),
    };
    for (credential, client, expected, calls) in [
        (
            "a".repeat(MAX_AUTH_CREDENTIAL_CHARACTERS + 1),
            "client".into(),
            GatewayError::InvalidCredential,
            0,
        ),
        (
            "credential".into(),
            "😀".repeat(MAX_PRODUCT_CLIENT_ID_CHARACTERS + 1),
            GatewayError::InvalidCredential,
            0,
        ),
        (
            "a".repeat(MAX_AUTH_CREDENTIAL_CHARACTERS),
            "😀".repeat(MAX_PRODUCT_CLIENT_ID_CHARACTERS),
            GatewayError::Transport,
            1,
        ),
    ] {
        let connector = RefuseConnect(AtomicUsize::new(0));
        assert!(
            matches!(Session::connect(endpoint.address,endpoint.evidence(&credential),&client,&connector,
            Arc::new(Time(Instant::now())),Arc::new(Never),policy()),Err(error) if error==expected)
        );
        assert_eq!(connector.0.load(Ordering::SeqCst), calls);
    }
}

#[test]
fn peer_owner_joins_on_caller_unwind() {
    let completed = Arc::new(AtomicUsize::new(0));
    let finished = completed.clone();
    let result = std::panic::catch_unwind(move || {
        let _peer = Peer::spawn(move || {
            thread::sleep(Duration::from_millis(10));
            finished.store(1, Ordering::SeqCst);
        });
        panic!("caller failed");
    });
    assert!(result.is_err());
    assert_eq!(completed.load(Ordering::SeqCst), 1);
}

#[test]
fn cancellation_and_deadline_overflow_before_callback_close_without_entering_driver() {
    struct ManualClock(AtomicU64);
    impl Clock for ManualClock {
        fn elapsed_ms(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
    }
    struct ManualCancel(AtomicBool);
    impl Cancellation for ManualCancel {
        fn cancelled(&self) -> bool {
            self.0.load(Ordering::SeqCst)
        }
    }
    for expected in [GatewayError::Cancelled, GatewayError::TimedOut] {
        let (endpoint, peer) = peer(|socket| {
            assert!(socket.ended());
        });
        let clock = Arc::new(ManualClock(AtomicU64::new(0)));
        let cancel = Arc::new(ManualCancel(AtomicBool::new(false)));
        let session = Session::connect(
            endpoint.address,
            endpoint.evidence("credential"),
            "example",
            &LocalConnector,
            clock.clone(),
            cancel.clone(),
            policy(),
        )
        .unwrap();
        let connection = GatewayConnection::new(session);
        match expected {
            GatewayError::Cancelled => cancel.0.store(true, Ordering::SeqCst),
            GatewayError::TimedOut => clock.0.store(u64::MAX, Ordering::SeqCst),
            _ => unreachable!(),
        }
        let calls = AtomicUsize::new(0);
        assert!(
            matches!(connection.run(|| calls.fetch_add(1,Ordering::SeqCst)),Err(error) if error==expected)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        peer.join().unwrap();
    }
}

#[test]
fn catalogue_resolve_preserves_oversized_entry_and_transport_cause() {
    for (code, expected) in [
        (
            CatalogueReadErrorCode::OversizedEntry,
            CatalogueSourceError::OversizedEntry,
        ),
        (
            CatalogueReadErrorCode::SourceUnavailable,
            CatalogueSourceError::Unavailable,
        ),
    ] {
        let scope = Scope::new(
            Id::new("receiver").unwrap(),
            Id::new("gateway").unwrap(),
            conversation_catalogue_stream(
                &OrganizationId::new("org").unwrap(),
                &PrincipalId::new("owner").unwrap(),
            ),
            Id::new("incarnation").unwrap(),
            conversation_catalogue_schema(),
            Id::new("epoch-3").unwrap(),
        );
        let descriptor = ManifestEntry {
            key: EntryKey {
                creation: 1,
                id: Id::new("conversation").unwrap(),
            },
            revision: 1,
            deleted: false,
        };
        let actual = scope.clone();
        let (endpoint, peer) = peer(move |socket| {
            let head = request(socket);
            assert_eq!(head.method, product_method::CONVERSATION_CATALOGUE_HEAD);
            send(
                socket,
                OutgoingMessage::Response(
                    ResponseFrame::success(
                        &head.id,
                        &json!({"scope": wire_scope(&actual), "head":"1"}),
                    )
                    .unwrap(),
                ),
            );
            let manifest = request(socket);
            assert_eq!(
                manifest.method,
                product_method::CONVERSATION_CATALOGUE_MANIFEST
            );
            let page = json!({
                "request": manifest.params["request"],
                "entries": [wire_descriptor(&descriptor)],
                "hasMore": false,
            });
            send(
                socket,
                OutgoingMessage::Response(ResponseFrame::success(&manifest.id, &page).unwrap()),
            );
            let resolve = request(socket);
            assert_eq!(
                resolve.method,
                product_method::CONVERSATION_CATALOGUE_RESOLVE
            );
            assert_eq!(resolve.params["maxPayloadBytes"], 1);
            send(
                socket,
                OutgoingMessage::Response(ResponseFrame::failure(
                    &resolve.id,
                    code.as_str(),
                    "typed source refusal",
                )),
            );
            assert!(socket.ended());
        });
        let connection = GatewayConnection::new(connect(&endpoint));
        let mut source = connection.catalogue(Id::new("receiver").unwrap(), 3);
        let discovery = connection.run(|| source.discover()).unwrap();
        assert_eq!(discovery.result.unwrap().unwrap(), (scope.clone(), 1));
        let pass = CataloguePass {
            scope,
            completed: 0,
            boundary: 1,
            cursor: None,
            generation: 1,
        };
        let manifest = connection
            .run(|| {
                source.manifest(&ManifestRequest {
                    pass: pass.clone(),
                    max_entries: 1,
                })
            })
            .unwrap();
        let manifest = manifest.result.unwrap().unwrap();
        let resolve = connection
            .run(|| source.resolve(&pass, &manifest.entries[0].key.id, 1))
            .unwrap();
        assert_eq!(resolve.result.unwrap(), Err(expected));
        assert_eq!(resolve.outcome.failure, Some(GatewayError::Catalogue(code)));
        assert!(matches!(connection.begin(), Err(GatewayError::Transport)));
        drop(source);
        drop(connection);
        peer.join().unwrap();
    }
}

#[path = "session/watch.rs"]
mod watch;
