use super::super::{
    session::{LocalConnector, RpcKind, Session},
    sources::GatewayConnection,
};
use crate::conversation::{
    domain::{conversation_catalogue_stream, ConversationId},
    infrastructure::{conversation_catalogue_schema, NessaCatalogueSource},
};
use crate::product_contract::generated::{RecordReadErrorCode, SessionCloseReason};
use crate::{
    app::ports::Clock,
    product::{
        generated::{
            product_event, product_method, ProductSessionReady, SessionChallenge,
            MAX_AUTH_CREDENTIAL_CHARACTERS, MAX_PRODUCT_CLIENT_ID_CHARACTERS, PRODUCT_VERSION,
        },
        passive_read::wire::wire_scope,
    },
    protocol::{EventFrame, OutgoingMessage, RequestFrame, ResponseFrame},
    read_only_sync::application::{
        Cancellation, GatewayConnector, GatewayError, GatewayPolicy, GatewayStream,
    },
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_gateway_endpoint::domain::{EndpointIdentity, GatewayEndpoint};
use nessa_sync::replication::{
    application::{Access, ScopeAuthorizer},
    domain::{Id, Scope},
};
use serde_json::json;
use std::{
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Arc,
    },
    thread::{self, JoinHandle, Result as ThreadResult},
    time::{Duration, Instant},
};
use tungstenite::{error::ProtocolError, Error, Message, WebSocket};
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
            Err(error)
                if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline =>
            {
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
fn send(socket: &mut WebSocket<TcpStream>, message: OutgoingMessage) {
    socket
        .send(Message::Text(message.to_wire_text().unwrap().into()))
        .unwrap();
}
fn request(socket: &mut WebSocket<TcpStream>) -> RequestFrame {
    match socket.read().unwrap() {
        Message::Text(text) => RequestFrame::decode(&text).unwrap(),
        _ => panic!("text request"),
    }
}
fn peer(
    script: impl FnOnce(&mut WebSocket<TcpStream>) + Send + 'static,
) -> (GatewayEndpoint, Peer) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let thread = Peer::spawn(move || {
        let (stream, _) = bounded_accept(&listener);
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut socket = tungstenite::accept(stream).unwrap();
        send(
            &mut socket,
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
        let authentication = request(&mut socket);
        assert_eq!(authentication.method, product_method::SESSION_AUTHENTICATE);
        assert_eq!(authentication.params["nonce"], "nonce");
        send(
            &mut socket,
            OutgoingMessage::Response(
                ResponseFrame::success(&authentication.id, &ready()).unwrap(),
            ),
        );
        script(&mut socket);
    });
    (
        GatewayEndpoint::new(
            format!("ws://{address}"),
            EndpointIdentity::new("018fa012-2222-7222-8222-123456789abc".into(), 1).unwrap(),
        )
        .unwrap(),
        thread,
    )
}
fn connect(endpoint: &GatewayEndpoint) -> Session {
    Session::connect(
        endpoint,
        "credential",
        "example",
        &LocalConnector,
        Arc::new(Time(Instant::now())),
        Arc::new(Never),
        GatewayPolicy::new(2000, 1000, 8192, 2, 2).unwrap(),
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
fn preparing_and_wrong_correlation_close_connection_and_keep_one_first_cause() {
    for expected in [
        GatewayError::Record(RecordReadErrorCode::SourcePreparing),
        GatewayError::Correlation,
    ] {
        let (endpoint, peer) = peer(move |socket| {
            let read = request(socket);
            let response = match expected {
                GatewayError::Correlation => ResponseFrame::success("other", &json!({})).unwrap(),
                _ => ResponseFrame::failure(
                    &read.id,
                    RecordReadErrorCode::SourcePreparing.as_str(),
                    "temporary",
                ),
            };
            send(socket, OutgoingMessage::Response(response));
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
        assert_eq!(
            session.rpc(
                product_method::CONVERSATION_RECORDS_HEAD,
                &json!({}),
                RpcKind::Record
            ),
            Err(expected)
        );
        assert_eq!(session.fail(GatewayError::Protocol), expected);
        assert_eq!(session.finish().unwrap().failure, Some(expected));
        drop(session);
        peer.join().unwrap();
    }
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
    use crate::product_contract::generated::CatalogueReadErrorCode;
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
        assert!(socket.read().is_err());
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

#[test]
fn bounded_control_event_and_message_refusals_preserve_actual_typed_cause() {
    for expected in [
        GatewayError::ControlCapacity,
        GatewayError::EventCapacity,
        GatewayError::ResponseTooLarge,
    ] {
        let (endpoint, peer) = peer(move |socket| {
            let _ = request(socket);
            match expected {
                GatewayError::ControlCapacity => {
                    for _ in 0..3 {
                        socket.send(Message::Ping(Vec::new().into())).unwrap();
                    }
                }
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
                    socket
                        .send(Message::Text(
                            "x".repeat(crate::product::generated::MAX_RECORD_RESPONSE_BYTES + 1)
                                .into(),
                        ))
                        .unwrap();
                }
            }
            while socket.read().is_ok() {}
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

#[test]
fn bounded_http_upgrade_retains_factory_cause_and_closes_the_physical_peer() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let endpoint = GatewayEndpoint::new(
        format!("ws://{address}"),
        EndpointIdentity::new("018fa012-2222-7222-8222-123456789abc".into(), 1).unwrap(),
    )
    .unwrap();
    let peer = Peer::spawn(move || {
        let (mut stream, _) = bounded_accept(&listener);
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut request = [0; 4096];
        assert!(stream.read(&mut request).unwrap() > 0);
        let response = format!(
            "HTTP/1.1 101 Switching Protocols\r\nX: {}\r\n",
            "x".repeat(1024)
        );
        stream.write_all(response.as_bytes()).unwrap();
        assert!(
            stream.read(&mut request).is_err()
                || stream.read(&mut request).unwrap_or_default() == 0
        );
    });
    let result = Session::connect(
        &endpoint,
        "credential",
        "example",
        &LocalConnector,
        Arc::new(Time(Instant::now())),
        Arc::new(Never),
        GatewayPolicy::new(2000, 1000, 128, 2, 2).unwrap(),
    );
    assert!(matches!(result, Err(GatewayError::UpgradeTooLarge)));
    peer.join().unwrap();
}

#[test]
fn challenge_scalar_refusal_happens_before_any_authentication_request() {
    for nonce in [String::new(), "x".repeat(257)] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let endpoint = GatewayEndpoint::new(
            format!("ws://{address}"),
            EndpointIdentity::new("018fa012-2222-7222-8222-123456789abc".into(), 1).unwrap(),
        )
        .unwrap();
        let peer = Peer::spawn(move || {
            let (stream, _) = bounded_accept(&listener);
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut socket = tungstenite::accept(stream).unwrap();
            send(
                &mut socket,
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
            assert!(socket.read().is_err());
        });
        assert!(matches!(
            Session::connect(
                &endpoint,
                "credential",
                "example",
                &LocalConnector,
                Arc::new(Time(Instant::now())),
                Arc::new(Never),
                GatewayPolicy::new(2000, 1000, 8192, 2, 2).unwrap()
            ),
            Err(GatewayError::Protocol)
        ));
        peer.join().unwrap();
    }
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
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let endpoint = GatewayEndpoint::new(
            format!("ws://{address}"),
            EndpointIdentity::new("018fa012-2222-7222-8222-123456789abc".into(), 1).unwrap(),
        )
        .unwrap();
        let peer = Peer::spawn(move || {
            let (stream, _) = bounded_accept(&listener);
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut socket = tungstenite::accept(stream).unwrap();
            send(
                &mut socket,
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
            let auth = request(&mut socket);
            send(
                &mut socket,
                OutgoingMessage::Response(ResponseFrame::failure(&auth.id, code, "sanitized")),
            );
            assert!(socket.read().is_err());
        });
        assert!(
            matches!(Session::connect(&endpoint,"credential","example",&LocalConnector,Arc::new(Time(Instant::now())),Arc::new(Never),GatewayPolicy::new(2000,1000,8192,2,2).unwrap()),Err(GatewayError::Authentication(reason)) if reason==expected)
        );
        peer.join().unwrap();
    }
}

#[test]
fn borrowed_authentication_scalar_acquisition_is_bounded_before_connect() {
    struct RefuseConnect(AtomicUsize);
    impl GatewayConnector for RefuseConnect {
        fn connect(&self, _: SocketAddr, _: Duration) -> io::Result<Box<dyn GatewayStream>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(io::ErrorKind::ConnectionRefused.into())
        }
    }
    let endpoint = GatewayEndpoint::new(
        "ws://127.0.0.1:8000".into(),
        EndpointIdentity::new("018fa012-2222-7222-8222-123456789abc".into(), 1).unwrap(),
    )
    .unwrap();
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
            matches!(Session::connect(&endpoint,&credential,&client,&connector,
            Arc::new(Time(Instant::now())),Arc::new(Never),GatewayPolicy::new(2000,1000,8192,2,2).unwrap()),Err(error) if error==expected)
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
            assert!(matches!(
                socket.read(),
                Err(Error::Protocol(ProtocolError::ResetWithoutClosingHandshake))
                    | Err(Error::ConnectionClosed)
                    | Err(Error::AlreadyClosed)
            ));
        });
        let clock = Arc::new(ManualClock(AtomicU64::new(0)));
        let cancel = Arc::new(ManualCancel(AtomicBool::new(false)));
        let session = Session::connect(
            &endpoint,
            "credential",
            "example",
            &LocalConnector,
            clock.clone(),
            cancel.clone(),
            GatewayPolicy::new(2000, 1000, 8192, 2, 2).unwrap(),
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
    use crate::product::catalogue_read::wire::wire_descriptor;
    use crate::product_contract::generated::CatalogueReadErrorCode;
    use nessa_sync::replication::catalogue::{
        CataloguePass, CatalogueSource, CatalogueSourceError, EntryKey, ManifestEntry,
        ManifestRequest,
    };
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
            assert!(socket.read().is_err());
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
