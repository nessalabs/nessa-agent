//! Protected product sessions on the native listener: slice 3 rows PR1–PR13 in
//! `docs/design/auth/device-pairing.md` ("Protected reads over the native
//! channel"). Real registry, Cedar, receiver authority, conversation store,
//! TLS and private storage; the device side is a raw probe that frames product
//! messages itself, so each refusal is observed on the wire.
use super::support::{pending, private_root, Fixture, WAIT};
use nessa_auth::{
    adapters::{
        cedar::CedarPolicyEvaluator,
        pairing::{
            FilePairingState, GatewayTrust, ManualCode, NativeIdentity, NativeTransport, OsEntropy,
        },
    },
    application::{
        credential_admin::RevokeCredentialRequest,
        pairing::{ClientPendingStore, DeviceConnectionProof, PairingStore, PrivateKeyMaterial},
        ports::{CredentialEvidence, CredentialVerifier, PortFuture, VerifiedCredential},
    },
    domain::{AudienceId, OrganizationId, ResourceId},
};
use nessa_server::{
    agents::{
        application::{AgentProbe, AgentProbeEvidence},
        domain::AgentId,
    },
    app::dependencies::RuntimeDependencies,
    conversation::infrastructure::{LocalConversationStore, NessaCatalogueReadSource},
    device_pairing::infrastructure::{
        encode_frame,
        wire::{
            decode_reply, encode_request, NativePairingReply, NativePairingRequest,
            NativePairingStatus,
        },
        EnrollmentChannel, FrameReader, NativeEnrollmentClient, ProtectedSessions,
        MAX_PROTECTED_REQUEST_BYTES, MAX_PROTECTED_RESPONSE_BYTES,
    },
    product::{DeviceCredentials, NativeSessions, ProductDependencies, ProductRouteState},
};
use nessa_sync::replication::domain::Id;
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    sync::Arc,
};

struct NoAgents;
impl AgentProbe for NoAgents {
    fn evidence(&self, _: AgentId) -> Option<AgentProbeEvidence> {
        None
    }
}
/// The registry's device verifier, as composition supplies it.
struct Devices(Arc<nessa_auth::adapters::local::LocalCredentialStore>);
impl DeviceCredentials for Devices {
    fn verify<'a>(
        &'a self,
        proof: &'a DeviceConnectionProof,
        evidence: &'a CredentialEvidence,
        audience: &'a AudienceId,
    ) -> PortFuture<'a, VerifiedCredential> {
        Box::pin(async move {
            self.0
                .device_verifier(proof)
                .verify(evidence, audience)
                .await
        })
    }
}

/// The product state a native session is served with: the fixture's registry
/// and receivers, and a real conversation store for catalogue reads.
fn sessions(fixture: &Fixture) -> Arc<dyn ProtectedSessions> {
    let root = private_root(fixture.directory.path(), "conversation-metadata");
    let metadata = Arc::new(LocalConversationStore::open(&root.join("metadata.sqlite3")).unwrap());
    let state = ProductRouteState::new(
        ResourceId::new("gateway").unwrap(),
        OrganizationId::new("org").unwrap(),
        AudienceId::new("gateway").unwrap(),
        ProductDependencies {
            verifier: fixture.registry.clone(),
            access: fixture.registry.clone(),
            clock: Arc::new(super::support::Time),
            policy: Arc::new(CedarPolicyEvaluator::new().unwrap()),
            uptime_clock: RuntimeDependencies::default().clock,
            agent_probe: Arc::new(NoAgents),
        },
    )
    .with_passive_read(fixture.receivers.clone(), metadata.clone())
    .with_catalogue_source(Arc::new(NessaCatalogueReadSource::new(
        metadata,
        Id::new("gateway").unwrap(),
    )));
    Arc::new(NativeSessions::new(
        state,
        Arc::new(Devices(fixture.registry.clone())),
    ))
}

/// One device paired to Active through the listener: its private state, which
/// holds the saved credential, and its receiver and epoch.
struct Paired {
    store: Arc<FilePairingState>,
    credential: String,
    receiver: String,
    epoch: u64,
}
async fn pair(fixture: &Fixture, address: SocketAddr, name: &str) -> Paired {
    let created = fixture
        .gateway
        .create(fixture.session.clone(), OsEntropy)
        .await
        .unwrap();
    let (_, store) = pending(fixture.directory.path(), name);
    let client = NativeEnrollmentClient::new(store.clone(), RuntimeDependencies::default().clock);
    let code = ManualCode::parse(created.code().expose_bytes()).unwrap();
    tokio::time::timeout(
        WAIT,
        client.enroll(TcpStream::connect(address).unwrap(), code, OsEntropy),
    )
    .await
    .unwrap()
    .unwrap();
    let id = created.record().id();
    let (_, key) = fixture
        .registry
        .read_pairing(id)
        .unwrap()
        .claim_binding()
        .unwrap();
    fixture
        .gateway
        .approve(&fixture.session, id, key, OsEntropy)
        .await
        .unwrap();
    let status = tokio::time::timeout(
        WAIT,
        client.status(TcpStream::connect(address).unwrap(), None),
    )
    .await
    .unwrap()
    .unwrap();
    let NativePairingStatus::Active {
        receiver,
        access_epoch,
        ..
    } = status
    else {
        panic!("the device reads Active: {status:?}");
    };
    client.shutdown().await;
    Paired {
        credential: store
            .load_credential()
            .unwrap()
            .unwrap()
            .credential()
            .as_str()
            .to_owned(),
        store,
        receiver: receiver.as_str().to_owned(),
        epoch: access_epoch,
    }
}

/// A device-side protected connection that frames product messages itself.
struct Probe {
    transport: NativeTransport<TcpStream>,
    frames: FrameReader,
    next: u64,
    nonce: String,
}
impl Probe {
    /// TLS with `key`'s identity and the gateway pinned, then `openProduct`
    /// and the challenge. Blocking: run on a blocking thread.
    fn open(address: SocketAddr, store: &FilePairingState) -> Self {
        let (key, pin) = device_key(store);
        Self::open_with(address, NativeIdentity::restore(key).unwrap(), pin)
    }
    fn open_with(address: SocketAddr, identity: NativeIdentity, pin: [u8; 44]) -> Self {
        let socket = TcpStream::connect(address).unwrap();
        socket.set_read_timeout(Some(WAIT)).unwrap();
        let transport =
            NativeTransport::connect(socket, &identity, GatewayTrust::Pinned(pin)).unwrap();
        let mut selector = EnrollmentChannel::new(transport);
        selector
            .send_envelope(&encode_request(&NativePairingRequest::OpenProduct).unwrap())
            .unwrap();
        let mut probe = Self {
            transport: selector.into_transport(),
            frames: FrameReader::new(MAX_PROTECTED_RESPONSE_BYTES),
            next: 0,
            nonce: String::new(),
        };
        let challenge = probe.value().expect("challenge");
        assert_eq!(challenge["event"], "session.challenge", "{challenge}");
        probe.nonce = challenge["payload"]["nonce"].as_str().unwrap().to_owned();
        probe
    }
    /// The next product message, or `None` once the gateway closed.
    fn value(&mut self) -> Option<Value> {
        loop {
            let buffer = self.frames.unfilled().ok()?;
            let count = self
                .transport
                .read(buffer)
                .ok()
                .filter(|count| *count > 0)?;
            if let Some(body) = self.frames.filled(count).ok()? {
                return Some(serde_json::from_slice(&body).unwrap());
            }
        }
    }
    fn send_raw(&mut self, bytes: &[u8]) {
        self.transport.write_all(bytes).unwrap();
        self.transport.flush().unwrap();
    }
    fn call(&mut self, method: &str, params: Value) -> Option<Value> {
        self.next += 1;
        let id = self.next.to_string();
        let text = json!({"type":"req","id":id,"method":method,"params":params}).to_string();
        self.send_raw(&encode_frame(MAX_PROTECTED_REQUEST_BYTES, text.as_bytes()).unwrap());
        loop {
            let value = self.value()?;
            if value["id"] == id {
                return Some(value);
            }
        }
    }
    fn authenticate(&mut self, credential: &str, nonce: &str) -> Option<Value> {
        self.call(
            "session.authenticate",
            json!({"minVersion":1,"maxVersion":1,"nonce":nonce,"credential":credential,
                "client":{"id":"probe"}}),
        )
    }
    fn catalogue_head(&mut self, receiver: &str, epoch: u64) -> Option<Value> {
        self.call(
            "conversation.catalogueHead",
            json!({"receiverId":receiver,"accessEpoch":epoch.to_string()}),
        )
    }
}
/// The saved device key and gateway pin, read again from the private state.
fn device_key(store: &FilePairingState) -> (PrivateKeyMaterial, [u8; 44]) {
    let (key, pin, _) = store
        .load_credential()
        .unwrap()
        .unwrap()
        .into_enrollment()
        .into_parts();
    (key, pin)
}
fn code(response: &Value) -> &str {
    response["error"]["code"].as_str().unwrap_or("")
}
async fn blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::time::timeout(WAIT, tokio::task::spawn_blocking(work))
        .await
        .unwrap()
        .unwrap()
}

/// Rows PR1, PR3: `openProduct` makes the connection a product session; the
/// device's credential id, on the key it was issued for, authenticates; a read
/// is admitted with the issued receiver and epoch; the session holds only the
/// read grant.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn protected_session_reads_with_the_issued_credential() {
    let fixture = Fixture::new().await;
    let (address, stop, listener, _) = fixture.listener_serving(Some(sessions(&fixture))).await;
    let device = pair(&fixture, address, "device").await;
    let credential = device.credential.clone();
    let (receiver, epoch) = (device.receiver.clone(), device.epoch);
    let saved = device.store.clone();
    let (ready, head, write) = blocking(move || {
        let mut probe = Probe::open(address, &saved);
        let nonce = probe.nonce.clone();
        let ready = probe.authenticate(&credential, &nonce).unwrap();
        let head = probe.catalogue_head(&receiver, epoch).unwrap();
        let write = probe.call("conversation.list", json!({})).unwrap();
        (ready, head, write)
    })
    .await;
    assert_eq!(ready["ok"], true, "{ready}");
    assert_eq!(ready["payload"]["principalId"], "owner");
    assert_eq!(head["ok"], true, "{head}");
    assert_eq!(
        head["payload"]["scope"]["receiver"],
        device.receiver.as_str()
    );
    assert_eq!(
        head["payload"]["scope"]["accessEpoch"],
        format!("epoch-{}", device.epoch)
    );
    assert_eq!(
        code(&write),
        "forbidden",
        "the device credential reads only"
    );
    stop.send(()).unwrap();
    listener.await.unwrap().unwrap();
    fixture.gateway.shutdown().await;
}

/// Row PR4: another device's credential id on this key, the owner's bearer
/// secret, and an unknown id are all refused before ready.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn protected_session_refuses_another_devices_credential() {
    let fixture = Fixture::new().await;
    let (address, stop, listener, _) = fixture.listener_serving(Some(sessions(&fixture))).await;
    let first = pair(&fixture, address, "first").await;
    let second = pair(&fixture, address, "second").await;
    let other = first.credential.clone();
    let bearer = fixture.owner_token.clone();
    let refusals = blocking(move || {
        [other, bearer, "credential-unknown".to_owned()]
            .into_iter()
            .map(|evidence| {
                let mut probe = Probe::open(address, &second.store);
                let nonce = probe.nonce.clone();
                let refused = probe.authenticate(&evidence, &nonce);
                let after = probe.value();
                (refused, after)
            })
            .collect::<Vec<_>>()
    })
    .await;
    for (refused, after) in refusals {
        let refused = refused.unwrap();
        assert_eq!(refused["ok"], false, "{refused}");
        assert_eq!(code(&refused), "unauthorized");
        assert!(after.is_none(), "the connection closes after the refusal");
    }
    stop.send(()).unwrap();
    listener.await.unwrap().unwrap();
    fixture.gateway.shutdown().await;
}

/// Rows PR5, PR6, A9: revoking the credential refuses the open session's next
/// read and every later authentication; the device's pinned status reads
/// Terminal.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn protected_session_refuses_after_revocation() {
    let fixture = Fixture::new().await;
    let (address, stop, listener, _) = fixture.listener_serving(Some(sessions(&fixture))).await;
    let device = pair(&fixture, address, "device").await;
    let credential = device.credential.clone();
    let (receiver, epoch) = (device.receiver.clone(), device.epoch);
    let registry = fixture.registry.clone();
    let revoked_credential = credential.clone();
    let saved = device.store.clone();
    let (before, after, again) = blocking(move || {
        let mut probe = Probe::open(address, &saved);
        let nonce = probe.nonce.clone();
        assert_eq!(probe.authenticate(&credential, &nonce).unwrap()["ok"], true);
        let before = probe.catalogue_head(&receiver, epoch).unwrap();
        registry
            .revoke_sync(RevokeCredentialRequest {
                request_id: "revoke-device".into(),
                issuer_principal_id: "owner".into(),
                credential_id: revoked_credential,
                revoked_at: 111,
            })
            .unwrap();
        let after = probe.catalogue_head(&receiver, epoch);
        let mut fresh = Probe::open(address, &saved);
        let nonce = fresh.nonce.clone();
        let again = fresh.authenticate(&credential, &nonce).unwrap();
        (before, after, again)
    })
    .await;
    assert_eq!(before["ok"], true, "{before}");
    // Either the read's own admission refuses, or the session's periodic
    // current-state check closed it first; no page is served either way.
    if let Some(after) = after {
        assert_eq!(after["ok"], false, "{after}");
        assert_eq!(code(&after), "unauthorized");
    }
    assert_eq!(code(&again), "unauthorized");
    stop.send(()).unwrap();
    listener.await.unwrap().unwrap();
    fixture.gateway.shutdown().await;
}

/// Row PR7: a read naming another receiver or a stale epoch is refused by
/// admission with its own code.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn protected_session_refuses_scope_substitution() {
    let fixture = Fixture::new().await;
    let (address, stop, listener, _) = fixture.listener_serving(Some(sessions(&fixture))).await;
    let first = pair(&fixture, address, "first").await;
    let second = pair(&fixture, address, "second").await;
    let credential = first.credential.clone();
    let (receiver, epoch, other) = (first.receiver.clone(), first.epoch, second.receiver.clone());
    let saved = first.store.clone();
    let (wrong, stale, current) = blocking(move || {
        let mut probe = Probe::open(address, &saved);
        let nonce = probe.nonce.clone();
        assert_eq!(probe.authenticate(&credential, &nonce).unwrap()["ok"], true);
        let wrong = probe.catalogue_head(&other, epoch).unwrap();
        let stale = probe.catalogue_head(&receiver, epoch + 1).unwrap();
        let current = probe.catalogue_head(&receiver, epoch).unwrap();
        (wrong, stale, current)
    })
    .await;
    assert_eq!(code(&wrong), "wrong_receiver", "{wrong}");
    assert_eq!(code(&stale), "stale_epoch", "{stale}");
    assert_eq!(
        current["ok"], true,
        "the exact scope still reads: {current}"
    );
    stop.send(()).unwrap();
    listener.await.unwrap().unwrap();
    fixture.gateway.shutdown().await;
}

/// Row PR8: an authentication frame carrying another connection's nonce is
/// refused, though the key and credential are right.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn protected_session_refuses_replayed_authentication() {
    let fixture = Fixture::new().await;
    let (address, stop, listener, _) = fixture.listener_serving(Some(sessions(&fixture))).await;
    let device = pair(&fixture, address, "device").await;
    let credential = device.credential.clone();
    let saved = device.store.clone();
    let (replayed, fresh) = blocking(move || {
        let first = Probe::open(address, &saved);
        let mut second = Probe::open(address, &saved);
        let replayed = second.authenticate(&credential, &first.nonce).unwrap();
        let mut third = Probe::open(address, &saved);
        let nonce = third.nonce.clone();
        let fresh = third.authenticate(&credential, &nonce).unwrap();
        (replayed, fresh)
    })
    .await;
    assert_eq!(code(&replayed), "unauthorized", "{replayed}");
    assert_eq!(fresh["ok"], true, "{fresh}");
    stop.send(()).unwrap();
    listener.await.unwrap().unwrap();
    fixture.gateway.shutdown().await;
}

/// Row PR9: an announced frame above the request bound, and a frame cut short
/// by the device's close, end the session without a reply; the gateway goes on
/// serving and every permit returns.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn protected_session_ends_on_truncated_and_oversized_frames() {
    let fixture = Fixture::new().await;
    let (address, stop, listener, connections) =
        fixture.listener_serving(Some(sessions(&fixture))).await;
    let device = pair(&fixture, address, "device").await;
    let credential = device.credential.clone();
    let saved = device.store.clone();
    let (oversized, healthy) = blocking(move || {
        let mut probe = Probe::open(address, &saved);
        let nonce = probe.nonce.clone();
        assert_eq!(probe.authenticate(&credential, &nonce).unwrap()["ok"], true);
        let announced = u32::try_from(MAX_PROTECTED_REQUEST_BYTES + 1).unwrap();
        probe.send_raw(&announced.to_be_bytes());
        let oversized = probe.value();
        let mut truncated = Probe::open(address, &saved);
        let nonce = truncated.nonce.clone();
        assert_eq!(
            truncated.authenticate(&credential, &nonce).unwrap()["ok"],
            true
        );
        truncated.send_raw(&100u32.to_be_bytes());
        truncated.send_raw(b"{\"type\":");
        drop(truncated);
        let mut healthy = Probe::open(address, &saved);
        let nonce = healthy.nonce.clone();
        (
            oversized,
            healthy.authenticate(&credential, &nonce).unwrap(),
        )
    })
    .await;
    assert!(oversized.is_none(), "no reply to an oversized frame");
    assert_eq!(healthy["ok"], true, "{healthy}");
    stop.send(()).unwrap();
    listener.await.unwrap().unwrap();
    tokio::time::timeout(WAIT, connections.shutdown())
        .await
        .expect("every protected session returned its permit");
    fixture.gateway.shutdown().await;
}

/// Rows PR2, PR13: only a first envelope selects the product session, and
/// only where sessions are composed; otherwise the answer is `Refused`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn open_product_is_only_a_first_envelope_and_needs_sessions() {
    let fixture = Fixture::new().await;
    let (served, stop, listener, _) = fixture.listener_serving(Some(sessions(&fixture))).await;
    let (unserved, stop_unserved, unserved_listener, _) = fixture.listener_serving(None).await;
    let pin = fixture.gateway.identity().public_spki();
    // An open invitation, so the Hello is answered and the next envelope is
    // the one under test.
    fixture
        .gateway
        .create(fixture.session.clone(), OsEntropy)
        .await
        .unwrap();
    let replies = blocking(move || {
        let exchange = |address: SocketAddr, requests: Vec<NativePairingRequest>| {
            let identity = NativeIdentity::generate(&mut OsEntropy).unwrap();
            let socket = TcpStream::connect(address).unwrap();
            socket.set_read_timeout(Some(WAIT)).unwrap();
            let transport =
                NativeTransport::connect(socket, &identity, GatewayTrust::Pinned(pin)).unwrap();
            let mut channel = EnrollmentChannel::new(transport);
            let mut replies = Vec::new();
            for request in requests {
                channel
                    .send_envelope(&encode_request(&request).unwrap())
                    .unwrap();
                replies.push(decode_reply(&channel.receive_envelope().unwrap()).unwrap());
            }
            replies
        };
        let after_hello = exchange(
            served,
            vec![
                NativePairingRequest::Hello(nessa_auth::domain::pairing::AttemptId::new([7; 16])),
                NativePairingRequest::OpenProduct,
            ],
        );
        let without_sessions = exchange(unserved, vec![NativePairingRequest::OpenProduct]);
        (after_hello, without_sessions)
    })
    .await;
    let (after_hello, without_sessions) = replies;
    // After Hello, OpenProduct is out of order: the enrollment rows' Phase
    // refusal, answered `Refused`.
    assert!(matches!(
        after_hello.as_slice(),
        [NativePairingReply::Hello(_), NativePairingReply::Refused]
    ));
    assert!(matches!(
        without_sessions.as_slice(),
        [NativePairingReply::Refused]
    ));
    stop.send(()).unwrap();
    listener.await.unwrap().unwrap();
    stop_unserved.send(()).unwrap();
    unserved_listener.await.unwrap().unwrap();
    fixture.gateway.shutdown().await;
}

/// Rows PR10, PR11 and review F6b/F6c: product sessions have a pool of their
/// own, so eight held sessions refuse a ninth `openProduct` with `Refused` yet
/// leave enrollment and pinned status served; a stop wakes every held
/// session at once, an unauthenticated one included, well inside the
/// handshake deadline.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn protected_sessions_have_their_own_pool_and_are_woken_by_shutdown() {
    let fixture = Fixture::new().await;
    let (address, stop, listener, connections) =
        fixture.listener_serving(Some(sessions(&fixture))).await;
    let device = pair(&fixture, address, "device").await;
    let credential = device.credential.clone();
    let saved = device.store.clone();
    let (held, ninth) = blocking(move || {
        let mut held = (0..7)
            .map(|_| {
                let mut probe = Probe::open(address, &saved);
                let nonce = probe.nonce.clone();
                assert_eq!(probe.authenticate(&credential, &nonce).unwrap()["ok"], true);
                probe
            })
            .collect::<Vec<_>>();
        // The eighth holds its permit before authenticating.
        held.push(Probe::open(address, &saved));
        let (key, pin) = device_key(&saved);
        let identity = NativeIdentity::restore(key).unwrap();
        let socket = TcpStream::connect(address).unwrap();
        socket.set_read_timeout(Some(WAIT)).unwrap();
        let transport =
            NativeTransport::connect(socket, &identity, GatewayTrust::Pinned(pin)).unwrap();
        let mut channel = EnrollmentChannel::new(transport);
        channel
            .send_envelope(&encode_request(&NativePairingRequest::OpenProduct).unwrap())
            .unwrap();
        let ninth = decode_reply(&channel.receive_envelope().unwrap()).unwrap();
        (held, ninth)
    })
    .await;
    assert!(
        matches!(ninth, NativePairingReply::Refused),
        "a full product pool refuses the ninth session: {ninth:?}"
    );
    // Pinned status, which a purge depends on, is still served.
    let client =
        NativeEnrollmentClient::new(device.store.clone(), RuntimeDependencies::default().clock);
    let status = tokio::time::timeout(
        WAIT,
        client.status(TcpStream::connect(address).unwrap(), None),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        matches!(status, NativePairingStatus::Active { .. }),
        "{status:?}"
    );
    client.shutdown().await;
    let stopped = std::time::Instant::now();
    stop.send(()).unwrap();
    tokio::time::timeout(WAIT, listener)
        .await
        .expect("stop wakes and collects every held protected session")
        .unwrap()
        .unwrap();
    assert!(
        stopped.elapsed() < std::time::Duration::from_secs(5),
        "woken at once, not at the 10 s handshake deadline: {:?}",
        stopped.elapsed()
    );
    tokio::time::timeout(WAIT, connections.shutdown())
        .await
        .expect("every permit, connection and product, is back");
    assert_eq!(connections.product_wake_report().unwrap().iter().count(), 8);
    let closed = blocking(move || held.into_iter().all(|mut probe| probe.value().is_none())).await;
    assert!(closed, "every held session was closed by the wake");
    fixture.gateway.shutdown().await;
}
