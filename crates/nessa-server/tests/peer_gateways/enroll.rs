//! The dialing side of a peer gateway over the real `/session` route, Cedar,
//! and a real peer: gateway B enrolls, with its own native key, into gateway
//! A's peer invitation, keeping a reference to that key and never a copy.
//! Rows P1–P13 in `docs/design/auth/peer-gateways.md` ("The dialing side").
use super::product_client::ProductClient;
use super::support::{private_root, Fixture, Time, WAIT};
use base64::{engine::general_purpose::STANDARD, Engine};
use nessa_auth::{
    adapters::{
        cedar::CedarPolicyEvaluator,
        pairing::{FilePairingState, GatewayTrust, NativeIdentity, NativeTransport, OsEntropy},
    },
    application::{
        credential_admin::{IssueCredentialOutcome, IssueCredentialRequest},
        dto::{
            CredentialGrantDto, MembershipInputDto, MembershipRoleDto, MembershipStateDto,
            PrincipalInputDto, PrincipalKindDto, ResourceDto,
        },
        pairing::{
            ClientPendingStore, GatewayKeyStore, OwnerDecision, PairingStore, PrivateStateError,
        },
    },
    domain::{
        pairing::{
            AttemptId, ConsentClass, ConsentIntentId, DeviceKey, InvitationId, PublicIntent,
        },
        AudienceId, OrganizationId, ResourceId,
    },
};
use nessa_client_core::pairing::NativeEnrollmentClient;
use nessa_protocol::agents::AgentId;
use nessa_protocol::pairing::wire::NativePairingStatus;
use nessa_server::{
    agents::application::{AgentProbe, AgentProbeEvidence},
    app::dependencies::RuntimeDependencies,
    peer_gateways::{
        application::{
            PeerAudit, PeerAuditFuture, PeerAuditRecord, PeerAuditUnavailable, PeerConnectFuture,
            PeerConnector,
        },
        infrastructure::{
            DurablePeerAudit, PeerCommands, PeerEntry, PeerError, PeerPhase, PeerRecords,
            SlotRefusal, TcpPeerConnector, CONNECT,
        },
    },
    product::{ProductDependencies, ProductRouteState},
    server::entrypoint::http,
};
use serde_json::{json, Value};
use std::{
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

struct NoAgents;
impl AgentProbe for NoAgents {
    fn evidence(&self, _: AgentId) -> Option<AgentProbeEvidence> {
        None
    }
}

/// A wall clock that moves one millisecond each time it is read, so the
/// audit's observation times order its records.
struct Ticks(std::sync::atomic::AtomicU64);
impl nessa_auth::application::ports::Clock for Ticks {
    fn unix_milliseconds(&self) -> u64 {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    }
}

/// The durable peer audit, which can be told to refuse one kind of record.
struct Audit {
    durable: DurablePeerAudit,
    directory: PathBuf,
    refuse: Mutex<Option<&'static str>>,
}
impl Audit {
    /// Refuse every record of `kind` (`peer_enroll_requested`, ...) from now.
    fn refuse(&self, kind: Option<&'static str>) {
        *self.refuse.lock().unwrap() = kind;
    }
    /// Every kept record, oldest operation first, intent before outcome.
    fn records(&self) -> Vec<Value> {
        let mut records: Vec<Value> = std::fs::read_dir(&self.directory)
            .unwrap()
            .map(|entry| {
                serde_json::from_slice(&std::fs::read(entry.unwrap().path()).unwrap()).unwrap()
            })
            .collect();
        records.sort_by_key(|record| record["observedAtMs"].as_u64().unwrap());
        records
    }
}
fn kind(record: &PeerAuditRecord) -> &'static str {
    match record {
        PeerAuditRecord::EnrollRequested { .. } => "peer_enroll_requested",
        PeerAuditRecord::EnrollFinished { .. } => "peer_enroll_finished",
        PeerAuditRecord::ForgetRequested { .. } => "peer_forget_requested",
        PeerAuditRecord::ForgetFinished { .. } => "peer_forget_finished",
    }
}
impl PeerAudit for Audit {
    fn record(&self, record: PeerAuditRecord) -> PeerAuditFuture<'_> {
        if *self.refuse.lock().unwrap() == Some(kind(&record)) {
            return Box::pin(async { Err(PeerAuditUnavailable) });
        }
        self.durable.record(record)
    }
}

/// Gateway B: its own native key, its peer records, its peer audit, and a
/// product socket the fixture's owner authenticates to.
struct Dialing {
    identity: NativeIdentity,
    records: Arc<PeerRecords>,
    directory: PathBuf,
    audit: Arc<Audit>,
    product: SocketAddr,
}
impl Dialing {
    async fn new(fixture: &Fixture) -> Self {
        let root = private_root(fixture.directory.path(), "dialing");
        for directory in ["native-pairing", "peer-gateways", "peer-gateways-audit"] {
            nessa_local_storage::create_directory_beneath(&root, Path::new(directory)).unwrap();
        }
        let keys = Arc::new(FilePairingState::open(&root, Path::new("native-pairing")).unwrap());
        let audience = AudienceId::new("gateway-b").unwrap();
        let identity = NativeIdentity::generate(&mut OsEntropy).unwrap();
        keys.save_gateway_key(identity.key_material(), &audience, &Time)
            .unwrap();
        let records = Arc::new(
            PeerRecords::open(
                &root,
                Path::new("peer-gateways"),
                keys,
                audience,
                Arc::new(Time),
            )
            .unwrap(),
        );
        let audit = Arc::new(Audit {
            durable: DurablePeerAudit::new(
                root.join("peer-gateways-audit"),
                Arc::new(Ticks(1.into())),
            ),
            directory: root.join("peer-gateways-audit"),
            refuse: Mutex::new(None),
        });
        let product = serve(
            fixture,
            Some(PeerCommands::new(
                records.clone(),
                RuntimeDependencies::default().clock,
                audit.clone(),
                Arc::new(TcpPeerConnector),
            )),
        )
        .await;
        Self {
            identity,
            records,
            directory: root.join("peer-gateways"),
            audit,
            product,
        }
    }
    /// Every record file's bytes, by name, temporaries left out.
    fn files(&self) -> Vec<(String, Vec<u8>)> {
        let mut files: Vec<_> = std::fs::read_dir(&self.directory)
            .unwrap()
            .map(|entry| entry.unwrap())
            .map(|entry| {
                (
                    entry.file_name().into_string().unwrap(),
                    std::fs::read(entry.path()).unwrap(),
                )
            })
            .collect();
        files.sort();
        files
    }
}

/// The fixture's product route, with `peers` when given.
async fn serve(fixture: &Fixture, peers: Option<PeerCommands>) -> SocketAddr {
    let mut state = ProductRouteState::new(
        ResourceId::new("gateway").unwrap(),
        OrganizationId::new("org").unwrap(),
        AudienceId::new("gateway").unwrap(),
        ProductDependencies {
            verifier: fixture.registry.clone(),
            access: fixture.registry.clone(),
            clock: Arc::new(Time),
            policy: Arc::new(CedarPolicyEvaluator::new().unwrap()),
            uptime_clock: RuntimeDependencies::default().clock,
            agent_probe: Arc::new(NoAgents),
        },
    );
    if let Some(peers) = peers {
        state = state.with_peers(Arc::new(peers));
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, http::router(state)).await });
    address
}

fn blocking<T>(work: impl FnOnce() -> T) -> T {
    tokio::task::block_in_place(work)
}

/// A's invitation enrolling `class`, as its owner was shown the code.
async fn invite(
    fixture: &Fixture,
    class: ConsentClass,
) -> (String, nessa_auth::domain::pairing::InvitationId) {
    let created = fixture
        .gateway
        .create(fixture.session.clone(), class, OsEntropy)
        .await
        .unwrap();
    let code = std::str::from_utf8(created.code().display_bytes().as_ref())
        .unwrap()
        .to_owned();
    (code, created.record().id())
}

fn key_bytes(key: &[u8]) -> Value {
    json!(key)
}

/// Rows P1–P4: B enrolls with its own native key. A sees that key claim the
/// invitation; B's record pins A's key and holds no copy of B's private key,
/// in any spelling. Once A's owner approves, B's pinned status saves the
/// credential into the same record, which lists as active and is forgotten
/// locally.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_gateway_enrolls_into_a_peer_with_its_own_key_and_keeps_only_a_reference() {
    let fixture = Fixture::new().await;
    let (native, stop, listener, _) = fixture.listener().await;
    let dialing = Dialing::new(&fixture).await;
    let token = fixture.owner_token.clone();
    let (code, id) = invite(&fixture, ConsentClass::PeerRead).await;
    let peer_key = pin_at(native).await;

    let enrolled = blocking(|| {
        ProductClient::connect(dialing.product, &token).ok(
            "peer.enroll",
            json!({"address": native.to_string(), "code": code}),
        )
    });
    assert_eq!(enrolled["phase"], "pending", "{enrolled}");
    assert_eq!(enrolled["peerKey"], key_bytes(&peer_key[12..]));
    assert_eq!(enrolled["address"], native.to_string());

    // A saw B's own gateway key claim, not a key made for this enrollment.
    let (_, claimed) = fixture
        .registry
        .read_pairing(id)
        .unwrap()
        .claim_binding()
        .unwrap();
    assert_eq!(claimed.bytes(), &dialing.identity.public_spki()[12..]);

    // B's record: A's pin, a reference to B's key, and no private key.
    let files = dialing.files();
    assert_eq!(files.len(), 1, "one record and no temporary");
    let (name, bytes) = &files[0];
    let hex: String = peer_key[12..]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(name, &format!("{hex}.json"));
    let record: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(record["schemaVersion"], 1);
    assert_eq!(record["pin"], STANDARD.encode(peer_key));
    assert_eq!(
        record["gatewayKey"],
        STANDARD.encode(dialing.identity.public_spki())
    );
    let seed = dialing.identity.key_material().expose_bytes();
    let seed_hex: String = seed.iter().map(|byte| format!("{byte:02x}")).collect();
    for spelling in [
        seed.to_vec(),
        STANDARD.encode(seed).into_bytes(),
        seed_hex.into_bytes(),
    ] {
        assert!(
            !bytes
                .windows(spelling.len())
                .any(|window| window == spelling),
            "the record holds no copy of the private key"
        );
    }

    // A's owner approves the exact claimed key; B's pinned status saves the
    // credential A issued into the record (what the next part's poller does).
    let approval = fixture
        .gateway
        .approve(&fixture.session, id, claimed, OsEntropy)
        .await
        .unwrap();
    let peer = DeviceKey::new(peer_key[12..].try_into().unwrap());
    let client = NativeEnrollmentClient::enrolling(
        ConsentClass::PeerRead,
        Arc::new(dialing.records.slot(peer)),
        RuntimeDependencies::default().clock,
    );
    let status = tokio::time::timeout(
        WAIT,
        client.status(TcpStream::connect(native).unwrap(), None),
    )
    .await
    .unwrap()
    .unwrap();
    client.shutdown().await;
    let NativePairingStatus::Active { credential, .. } = &status else {
        panic!("the peer reads Active: {status:?}");
    };
    assert_eq!(Some(credential), approval.record.credential());
    let Some(PeerEntry::Readable(record)) = dialing.records.get(&peer).unwrap() else {
        panic!("the record reads");
    };
    assert!(
        matches!(record.phase(), PeerPhase::Active { credential: saved, .. } if saved == credential)
    );

    blocking(|| {
        let mut owner = ProductClient::connect(dialing.product, &token);
        let listed = owner.ok("peer.list", json!({}));
        assert_eq!(listed["items"].as_array().unwrap().len(), 1, "{listed}");
        assert_eq!(listed["items"][0]["phase"], "active");
        assert_eq!(listed["items"][0]["credentialId"], credential.as_str());
        let forgotten = owner.ok("peer.forget", json!({"peerKey": &peer_key[12..]}));
        assert_eq!(forgotten["phase"], "active");
        assert_eq!(owner.ok("peer.list", json!({}))["items"], json!([]));
        assert_eq!(
            owner.refused("peer.forget", json!({"peerKey": &peer_key[12..]})),
            "peer_not_found"
        );
    });
    assert!(dialing.files().is_empty(), "forgetting removes the record");
    stop.send(()).unwrap();
    listener.await.unwrap().unwrap();
}

/// Rows P5–P7: a device invitation, a wrong code and an invitation the
/// owner cancelled each refuse the enrollment before anything is saved; a
/// peer already kept is refused until forgotten.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_or_wrong_class_invitation_saves_nothing() {
    let fixture = Fixture::new().await;
    let (native, stop, listener, _) = fixture.listener().await;
    let dialing = Dialing::new(&fixture).await;
    let token = fixture.owner_token.clone();
    let enroll = |code: &str| {
        let code = code.to_owned();
        blocking(|| {
            ProductClient::connect(dialing.product, &token).call(
                "peer.enroll",
                json!({"address": native.to_string(), "code": code}),
            )
        })
    };
    let refused = |frame: Value| frame["error"]["code"].as_str().unwrap().to_owned();

    let (code, id) = invite(&fixture, ConsentClass::DeviceRead).await;
    assert_eq!(refused(enroll(&code)), "peer_wrong_invitation");
    cancel(&fixture, id).await;
    let (code, id) = invite(&fixture, ConsentClass::PeerRead).await;
    let wrong = if code == "ABCD-2345" {
        "ABCD-2346"
    } else {
        "ABCD-2345"
    };
    assert_eq!(refused(enroll(wrong)), "peer_invitation_refused");
    cancel(&fixture, id).await;
    assert_eq!(refused(enroll(&code)), "peer_invitation_refused");
    assert!(dialing.files().is_empty(), "nothing refused is saved");
    assert!(dialing.records.list().unwrap().is_empty());

    // Nothing answering is unreachable, and saves nothing either.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let nowhere = closed.local_addr().unwrap();
    drop(closed);
    let frame = blocking(|| {
        ProductClient::connect(dialing.product, &token).call(
            "peer.enroll",
            json!({"address": nowhere.to_string(), "code": code}),
        )
    });
    assert_eq!(refused(frame), "peer_unreachable");
    assert!(dialing.files().is_empty());

    let (code, id) = invite(&fixture, ConsentClass::PeerRead).await;
    assert_eq!(enroll(&code)["ok"], true);
    cancel(&fixture, id).await;
    let kept = dialing.files();
    assert_eq!(kept.len(), 1);
    let (again, _) = invite(&fixture, ConsentClass::PeerRead).await;
    assert_eq!(refused(enroll(&again)), "peer_exists");
    assert_eq!(
        dialing.files(),
        kept,
        "the kept record is unchanged, byte for byte"
    );
    // The refusal is evidence about the peer that refused it: its key, and
    // the record as it was found and left.
    let pending = json!({"phase": "pending", "address": native.to_string()});
    let finished = dialing.audit.records().pop().unwrap();
    assert_eq!(finished["kind"], "peer_enroll_finished");
    assert_eq!(
        finished["target"]["peerKey"],
        kept[0].0.strip_suffix(".json").unwrap()
    );
    assert_eq!(
        finished["transition"],
        json!({"before": pending, "after": pending})
    );
    assert_eq!(
        finished["outcome"],
        json!({"result": "refused", "code": "peer_exists"})
    );
    stop.send(()).unwrap();
    listener.await.unwrap().unwrap();
}

/// Row P8: without native pairing there is no key to enroll with, so every
/// peer method answers `peer_not_configured`; with it, a member is refused by
/// Cedar and malformed params are refused before any effect.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn peer_routes_refuse_before_any_effect() {
    let fixture = Fixture::new().await;
    let unconfigured = serve(&fixture, None).await;
    let dialing = Dialing::new(&fixture).await;
    let token = fixture.owner_token.clone();
    let member = member_token(&fixture);
    let key = vec![7; 32];
    blocking(|| {
        let mut owner = ProductClient::connect(unconfigured, &token);
        for (method, params) in [
            (
                "peer.enroll",
                json!({"address": "127.0.0.1:1", "code": "ABCD-2345"}),
            ),
            ("peer.list", json!({})),
            ("peer.forget", json!({"peerKey": key})),
        ] {
            assert_eq!(
                owner.refused(method, params),
                "peer_not_configured",
                "{method}"
            );
        }
        let mut client = ProductClient::connect(dialing.product, &member);
        for (method, params) in [
            (
                "peer.enroll",
                json!({"address": "127.0.0.1:1", "code": "ABCD-2345"}),
            ),
            ("peer.list", json!({})),
            ("peer.forget", json!({"peerKey": key})),
        ] {
            assert_eq!(client.refused(method, params), "forbidden", "{method}");
        }
        let mut owner = ProductClient::connect(dialing.product, &token);
        for (method, params) in [
            // A name is never resolved; an address needs its port.
            (
                "peer.enroll",
                json!({"address": "localhost:7443", "code": "ABCD-2345"}),
            ),
            (
                "peer.enroll",
                json!({"address": "127.0.0.1", "code": "ABCD-2345"}),
            ),
            (
                "peer.enroll",
                json!({"address": "127.0.0.1:1", "code": "ABCD-234"}),
            ),
            (
                "peer.enroll",
                json!({"address": "127.0.0.1:1", "code": "ABCD!2345"}),
            ),
            ("peer.enroll", json!({"address": "127.0.0.1:1"})),
            ("peer.list", json!({"all": true})),
            ("peer.forget", json!({"peerKey": vec![7; 31]})),
            ("peer.forget", json!({"peerKey": key, "remote": true})),
        ] {
            assert_eq!(
                owner.refused(method, params.clone()),
                "invalid_request",
                "{method} {params}"
            );
        }
    });
    assert!(dialing.files().is_empty());
}

/// A's owner cancels invitation `id`.
async fn cancel(fixture: &Fixture, id: nessa_auth::domain::pairing::InvitationId) {
    fixture
        .gateway
        .decide(&fixture.session, id, OwnerDecision::Cancel)
        .await
        .unwrap();
}

/// The pin the gateway at `native` presents: its own key's public half, read
/// off a TLS handshake that goes no further.
async fn pin_at(native: SocketAddr) -> [u8; 44] {
    let identity = NativeIdentity::generate(&mut OsEntropy).unwrap();
    tokio::task::spawn_blocking(move || {
        NativeTransport::connect(
            TcpStream::connect(native).unwrap(),
            &identity,
            GatewayTrust::ManualBootstrap,
        )
        .unwrap()
        .gateway_spki()
    })
    .await
    .unwrap()
}

/// A member's credential: active, but not an admin, so Cedar refuses it
/// `credential.manage`.
fn member_token(fixture: &Fixture) -> String {
    let IssueCredentialOutcome::Issued { evidence, .. } = fixture
        .registry
        .issue_sync(IssueCredentialRequest {
            request_id: "member-request".into(),
            issuer_principal_id: "owner".into(),
            credential_id: "member-credential".into(),
            principal: PrincipalInputDto {
                id: "member-principal".into(),
                kind: PrincipalKindDto::Human,
            },
            membership: MembershipInputDto {
                id: "member-membership".into(),
                principal_id: "member-principal".into(),
                organization_id: "org".into(),
                role: MembershipRoleDto::Member,
                state: MembershipStateDto::Active,
            },
            audience_id: "gateway".into(),
            issued_at: 100,
            expires_at: None,
            grants: vec![CredentialGrantDto {
                action: "conversation.read".into(),
                resource: ResourceDto {
                    organization_id: "org".into(),
                    id: "gateway".into(),
                },
            }],
        })
        .unwrap()
    else {
        panic!("a new member credential is issued");
    };
    String::from_utf8(evidence.expose_bytes().to_vec()).unwrap()
}

/// Rows P9 and P10, at the store: a pending save with any key but the
/// gateway's own, or for a pin that is the gateway's own key, is refused and
/// writes nothing. A record this build cannot read lists as `unreadable`,
/// refuses a new enrollment into that peer, and is forgotten like any other.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_record_takes_only_the_gateways_key_and_an_unreadable_one_can_be_forgotten() {
    let fixture = Fixture::new().await;
    let dialing = Dialing::new(&fixture).await;
    let intent = PublicIntent::new(
        InvitationId::new([1; 16]),
        AttemptId::new([2; 16]),
        ConsentIntentId::new([3; 16]),
        1,
        1,
        ConsentClass::PeerRead,
    )
    .unwrap();
    let address: SocketAddr = "127.0.0.1:7443".parse().unwrap();
    let other = NativeIdentity::generate(&mut OsEntropy).unwrap();
    let mut pin = other.public_spki();
    pin[43] ^= 1;

    let slot = dialing.records.enrolling(address);
    assert_eq!(
        slot.save_pending(other.key_material(), &pin, intent, None),
        Err(PrivateStateError::Conflict),
        "another key is never enrolled or written"
    );
    assert_eq!(slot.refusal(), None);
    assert_eq!(
        slot.save_pending(
            dialing.identity.key_material(),
            &dialing.identity.public_spki(),
            intent,
            None
        ),
        Err(PrivateStateError::Conflict)
    );
    assert_eq!(slot.refusal(), Some(SlotRefusal::OwnGateway));
    assert!(dialing.files().is_empty());

    // A record of another shape, under a peer's name.
    let hex: String = pin[12..].iter().map(|byte| format!("{byte:02x}")).collect();
    let mut file = nessa_local_storage::open(
        &dialing.directory.join(format!("{hex}.json")),
        nessa_local_storage::OpenMode::CreateNew,
    )
    .unwrap();
    std::io::Write::write_all(&mut file, br#"{"schemaVersion":2}"#).unwrap();
    drop(file);
    let peer = DeviceKey::new(pin[12..].try_into().unwrap());
    assert_eq!(
        dialing.records.list().unwrap(),
        vec![PeerEntry::Unreadable(peer)]
    );
    let slot = dialing.records.enrolling(address);
    assert_eq!(
        slot.save_pending(dialing.identity.key_material(), &pin, intent, None),
        Err(PrivateStateError::Conflict)
    );
    assert_eq!(slot.refusal(), Some(SlotRefusal::Exists));
    let token = fixture.owner_token.clone();
    blocking(|| {
        let mut owner = ProductClient::connect(dialing.product, &token);
        let listed = owner.ok("peer.list", json!({}));
        assert_eq!(
            listed["items"],
            json!([{"peerKey": &pin[12..], "phase": "unreadable"}])
        );
        owner.ok("peer.forget", json!({"peerKey": &pin[12..]}));
    });
    assert!(dialing.files().is_empty());

    // A valid record, then the same record naming another key as the one this
    // gateway enrolled with: it is not this gateway's any more.
    let (native, stop, listener, _) = fixture.listener().await;
    let a_pin = pin_at(native).await;
    let a = DeviceKey::new(a_pin[12..].try_into().unwrap());
    dialing
        .records
        .enrolling(native)
        .save_pending(dialing.identity.key_material(), &a_pin, intent, None)
        .unwrap();
    assert!(matches!(
        dialing.records.list().unwrap().as_slice(),
        [PeerEntry::Readable(_)]
    ));
    let a_hex: String = a_pin[12..]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let path = dialing.directory.join(format!("{a_hex}.json"));
    let mut record: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    record["gatewayKey"] = json!(STANDARD.encode(other.public_spki()));
    std::fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    assert_eq!(
        dialing.records.list().unwrap(),
        vec![PeerEntry::Unreadable(a)]
    );
    assert_eq!(
        dialing.records.slot(a).load_credential().err(),
        Some(PrivateStateError::Corrupt)
    );
    let tampered = std::fs::read(&path).unwrap();
    let (code, _) = invite(&fixture, ConsentClass::PeerRead).await;
    let frame = blocking(|| {
        ProductClient::connect(dialing.product, &token).call(
            "peer.enroll",
            json!({"address": native.to_string(), "code": code}),
        )
    });
    assert_eq!(frame["error"]["code"], "peer_exists", "{frame}");
    assert_eq!(std::fs::read(&path).unwrap(), tampered);
    let finished = dialing.audit.records().pop().unwrap();
    assert_eq!(finished["kind"], "peer_enroll_finished");
    assert_eq!(finished["target"]["peerKey"], a_hex);
    assert_eq!(
        finished["transition"],
        json!({"before": {"phase": "unreadable"}, "after": {"phase": "unreadable"}})
    );
    assert_eq!(
        finished["outcome"],
        json!({"result": "refused", "code": "peer_exists"})
    );
    stop.send(()).unwrap();
    listener.await.unwrap().unwrap();
}

/// Row P11: forgetting while an enrollment runs is `peer_busy`, so it cannot
/// remove the record that enrollment is saving; once it ends, forget runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forgetting_waits_for_a_running_enrollment() {
    let fixture = Fixture::new().await;
    let dialing = Dialing::new(&fixture).await;
    let token = fixture.owner_token.clone();
    // Accepts and never speaks: the enrollment is held mid-handshake.
    let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = silent.local_addr().unwrap();
    let product = dialing.product;
    let enrolling = {
        let token = token.clone();
        std::thread::spawn(move || {
            ProductClient::connect(product, &token).call(
                "peer.enroll",
                json!({"address": address.to_string(), "code": "ABCD-2345"}),
            )
        })
    };
    let (held, _) = blocking(|| silent.accept().unwrap());
    let key = vec![7; 32];
    let busy = blocking(|| {
        ProductClient::connect(product, &token).refused("peer.forget", json!({"peerKey": key}))
    });
    assert_eq!(busy, "peer_busy");
    drop(held);
    let ended = blocking(|| enrolling.join().unwrap());
    assert_eq!(ended["error"]["code"], "peer_unreachable", "{ended}");
    let after = blocking(|| {
        ProductClient::connect(product, &token).refused("peer.forget", json!({"peerKey": key}))
    });
    assert_eq!(after, "peer_not_found");
}

/// Row P12: each enroll and forget keeps an intent before its effect and an
/// outcome after it, naming the operation, the peer, the state before and
/// after, the cause and the owner who asked; refusals are kept the same way.
/// An intent that cannot be kept refuses the command before anything changes.
/// An outcome that cannot be kept refuses the answer while the effect stands,
/// so the owner never sees success without its evidence.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enrolling_and_forgetting_are_audited_and_answer_only_when_kept() {
    let fixture = Fixture::new().await;
    let (native, stop, listener, _) = fixture.listener().await;
    let dialing = Dialing::new(&fixture).await;
    let token = fixture.owner_token.clone();
    let peer_key = pin_at(native).await;
    let hex: String = peer_key[12..]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let call = |method: &str, params: Value| {
        let method = method.to_owned();
        blocking(|| ProductClient::connect(dialing.product, &token).call(&method, params))
    };
    let enroll = |code: &str| {
        call(
            "peer.enroll",
            json!({"address": native.to_string(), "code": code}),
        )
    };
    let forget = || call("peer.forget", json!({"peerKey": &peer_key[12..]}));
    let code_of = |frame: &Value| frame["error"]["code"].as_str().unwrap_or("ok").to_owned();
    let owner = json!({"kind": "principal", "principalId": "owner"});
    let address = native.to_string();

    // Success: an enrollment's intent, then its outcome with the record it made.
    let (code, _) = invite(&fixture, ConsentClass::PeerRead).await;
    assert_eq!(enroll(&code)["ok"], true);
    let kept = dialing.audit.records();
    assert_eq!(kept.len(), 2, "{kept:?}");
    let (requested, finished) = (&kept[0], &kept[1]);
    assert_eq!(requested["kind"], "peer_enroll_requested");
    assert_eq!(requested["target"]["address"], address);
    assert_eq!(requested["initiator"], owner);
    assert_eq!(requested["cause"], "owner_requested");
    assert_eq!(finished["kind"], "peer_enroll_finished");
    assert_eq!(finished["operationId"], requested["operationId"]);
    assert_eq!(finished["target"]["peerKey"], hex);
    assert_eq!(finished["transition"]["before"]["phase"], "absent");
    assert_eq!(
        finished["transition"]["after"],
        json!({"phase": "pending", "address": address})
    );
    assert_eq!(finished["outcome"], json!({"result": "succeeded"}));
    assert_eq!(finished["initiator"], owner);
    assert_ne!(requested["recordId"], finished["recordId"]);
    let text = serde_json::to_string(&kept).unwrap();
    assert!(!text.contains(&code), "the code is never kept");

    // Success: a forget's intent names the record as it was.
    assert_eq!(forget()["ok"], true);
    let kept = dialing.audit.records();
    let (requested, finished) = (&kept[2], &kept[3]);
    assert_eq!(requested["kind"], "peer_forget_requested");
    assert_eq!(
        requested["transition"]["before"],
        json!({"phase": "pending", "address": address})
    );
    assert_eq!(finished["kind"], "peer_forget_finished");
    assert_eq!(finished["operationId"], requested["operationId"]);
    assert_eq!(finished["target"]["peerKey"], hex);
    assert_eq!(finished["transition"]["after"]["phase"], "absent");
    assert_eq!(finished["initiator"], owner);

    // Failure: a refused enrollment is kept, with nothing before or after.
    let (code, id) = invite(&fixture, ConsentClass::PeerRead).await;
    let wrong = if code == "ABCD-2345" {
        "ABCD-2346"
    } else {
        "ABCD-2345"
    };
    assert_eq!(code_of(&enroll(wrong)), "peer_invitation_refused");
    let kept = dialing.audit.records();
    let finished = &kept[5];
    assert_eq!(kept[4]["kind"], "peer_enroll_requested");
    assert_eq!(finished["kind"], "peer_enroll_finished");
    assert_eq!(
        finished["outcome"],
        json!({"result": "refused", "code": "peer_invitation_refused"})
    );
    assert_eq!(finished["transition"]["after"]["phase"], "absent");
    assert_eq!(finished["target"]["peerKey"], Value::Null);
    cancel(&fixture, id).await;

    // The intent cannot be kept: nothing is dialed, so the code stays usable.
    let (code, _) = invite(&fixture, ConsentClass::PeerRead).await;
    dialing.audit.refuse(Some("peer_enroll_requested"));
    assert_eq!(code_of(&enroll(&code)), "peer_audit_unavailable");
    assert!(dialing.files().is_empty(), "nothing saved");
    assert_eq!(dialing.audit.records().len(), 6, "nothing else kept");

    // The outcome cannot be kept: the record stands, the answer is refused.
    dialing.audit.refuse(Some("peer_enroll_finished"));
    assert_eq!(code_of(&enroll(&code)), "peer_audit_unavailable");
    assert_eq!(dialing.files().len(), 1, "the enrollment's record stands");
    let listed = call("peer.list", json!({}));
    assert_eq!(listed["payload"]["items"][0]["phase"], "pending");
    assert_eq!(dialing.audit.records().len(), 7, "its intent is kept");

    // A forget whose intent cannot be kept removes nothing.
    dialing.audit.refuse(Some("peer_forget_requested"));
    assert_eq!(code_of(&forget()), "peer_audit_unavailable");
    assert_eq!(dialing.files().len(), 1, "the record stays");

    // A forget whose outcome cannot be kept still removes the record.
    dialing.audit.refuse(Some("peer_forget_finished"));
    assert_eq!(code_of(&forget()), "peer_audit_unavailable");
    assert!(dialing.files().is_empty(), "the removal stands");
    let kept = dialing.audit.records();
    assert_eq!(kept.last().unwrap()["kind"], "peer_forget_requested");
    dialing.audit.refuse(None);
    assert_eq!(code_of(&forget()), "peer_not_found");
    stop.send(()).unwrap();
    listener.await.unwrap().unwrap();
}

/// The durable adapter refuses, rather than drops, a record it cannot keep.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_durable_peer_audit_reports_a_record_it_could_not_keep() {
    let directory = tempfile::tempdir().unwrap();
    let audit = DurablePeerAudit::new(directory.path().join("missing"), Arc::new(Time));
    let record = PeerAuditRecord::EnrollRequested {
        operation: uuid::Uuid::new_v4(),
        initiator: nessa_auth::domain::PrincipalId::new("owner").unwrap(),
        address: "127.0.0.1:1".parse().unwrap(),
    };
    assert_eq!(audit.record(record).await, Err(PeerAuditUnavailable));
}

/// A monotonic clock that moves only when the test moves it.
struct Manual(std::sync::atomic::AtomicU64);
impl nessa_protocol::clock::Clock for Manual {
    fn elapsed_ms(&self) -> u64 {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// A connect that never answers, as a black-holed address does. It flags
/// when it is dialed and when its attempt is dropped.
struct Never {
    dialed: Arc<std::sync::atomic::AtomicBool>,
    dropped: Arc<std::sync::atomic::AtomicBool>,
}
struct Flag(Arc<std::sync::atomic::AtomicBool>);
impl Drop for Flag {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}
impl PeerConnector for Never {
    fn connect(&self, _: SocketAddr) -> PeerConnectFuture<'_> {
        self.dialed.store(true, std::sync::atomic::Ordering::SeqCst);
        let attempt = Flag(self.dropped.clone());
        Box::pin(async move {
            let _attempt = attempt;
            std::future::pending().await
        })
    }
}

/// Row P13: the connect runs under the injected deadline clock, not an
/// operating-system timeout. A connect that never answers holds the
/// enrollment while the clock stands still, and ends it `peer_unreachable`,
/// audited, once the clock passes the connect deadline; the abandoned attempt
/// is dropped, so nothing is left running.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_connect_that_never_answers_ends_when_the_injected_clock_passes_its_deadline() {
    use nessa_auth::{adapters::pairing::ManualCode, domain::PrincipalId};
    use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
    let fixture = Fixture::new().await;
    let dialing = Dialing::new(&fixture).await;
    let clock = Arc::new(Manual(1_000.into()));
    let dialed = Arc::new(AtomicBool::new(false));
    let dropped = Arc::new(AtomicBool::new(false));
    let commands = Arc::new(PeerCommands::new(
        dialing.records.clone(),
        clock.clone(),
        dialing.audit.clone(),
        Arc::new(Never {
            dialed: dialed.clone(),
            dropped: dropped.clone(),
        }),
    ));
    let address: SocketAddr = "192.0.2.1:7443".parse().unwrap();
    let mut enrolling = tokio::spawn({
        let commands = commands.clone();
        async move {
            let owner = PrincipalId::new("owner").unwrap();
            commands
                .enroll(address, ManualCode::generate(&mut OsEntropy), &owner)
                .await
        }
    });
    tokio::time::timeout(WAIT, async {
        while !dialed.load(SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the enrollment dials");

    // One millisecond short of the deadline, real time passing ends nothing:
    // several clock reads go by and the connect is still held.
    clock
        .0
        .store(1_000 + CONNECT.as_millis() as u64 - 1, SeqCst);
    let held = tokio::time::timeout(
        nessa_protocol::pairing::socket::WAKE_TICK * 3,
        &mut enrolling,
    )
    .await;
    assert!(held.is_err(), "the enrollment is still connecting");
    assert!(!dropped.load(SeqCst));

    clock.0.store(1_000 + CONNECT.as_millis() as u64, SeqCst);
    let ended = tokio::time::timeout(WAIT, enrolling)
        .await
        .expect("the deadline ends the enrollment")
        .unwrap();
    assert_eq!(ended, Err(PeerError::Unreachable));
    assert!(dropped.load(SeqCst), "the abandoned connect is dropped");
    assert!(dialing.files().is_empty(), "nothing is saved");
    let finished = dialing.audit.records().pop().unwrap();
    assert_eq!(finished["kind"], "peer_enroll_finished");
    assert_eq!(
        finished["outcome"],
        json!({"result": "refused", "code": "peer_unreachable"})
    );
}
