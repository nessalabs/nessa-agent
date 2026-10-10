//! The dialing side of a peer gateway over the real `/session` route, Cedar,
//! and a real peer: gateway B enrolls, with its own native key, into gateway
//! A's peer invitation, keeping a reference to that key and never a copy.
//! Rows P1–P16 in `docs/design/auth/peer-gateways.md` ("The dialing side").
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
            DurablePeerAudit, EnrollmentEntropy, EnrollmentEntropySource, PeerCommands, PeerEntry,
            PeerError, PeerPhase, PeerRecords, SlotRefusal, SlotTransition, TcpPeerConnector,
            AUDIT_DEADLINE, CONNECT,
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
    /// Records of this kind are never answered.
    hang: Mutex<Option<&'static str>>,
    /// A record was left unanswered.
    hung: std::sync::atomic::AtomicBool,
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
/// The operating system's generator, as composition supplies it.
fn os_entropy() -> EnrollmentEntropySource {
    Arc::new(|| Box::new(OsEntropy) as Box<dyn EnrollmentEntropy>)
}

fn kind(record: &PeerAuditRecord) -> &'static str {
    match record {
        PeerAuditRecord::EnrollRequested { .. } => "peer_enroll_requested",
        PeerAuditRecord::EnrollFinished { .. } => "peer_enroll_finished",
        PeerAuditRecord::ForgetRequested { .. } => "peer_forget_requested",
        PeerAuditRecord::ForgetFinished { .. } => "peer_forget_finished",
        PeerAuditRecord::PollerChanged { .. } => "peer_poller_changed",
    }
}
impl PeerAudit for Audit {
    fn record(&self, record: PeerAuditRecord) -> PeerAuditFuture<'_> {
        if *self.refuse.lock().unwrap() == Some(kind(&record)) {
            return Box::pin(async { Err(PeerAuditUnavailable) });
        }
        if *self.hang.lock().unwrap() == Some(kind(&record)) {
            self.hung.store(true, std::sync::atomic::Ordering::SeqCst);
            return Box::pin(std::future::pending());
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
        Self::with_identity(
            fixture,
            "dialing",
            NativeIdentity::generate(&mut OsEntropy).unwrap(),
        )
        .await
    }
    /// Gateway B under `name`, enrolling with `identity`.
    async fn with_identity(fixture: &Fixture, name: &str, identity: NativeIdentity) -> Self {
        let root = private_root(fixture.directory.path(), name);
        for directory in ["native-pairing", "peer-gateways", "peer-gateways-audit"] {
            nessa_local_storage::create_directory_beneath(&root, Path::new(directory)).unwrap();
        }
        let keys = Arc::new(FilePairingState::open(&root, Path::new("native-pairing")).unwrap());
        let audience = AudienceId::new("gateway-b").unwrap();
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
            hang: Mutex::new(None),
            hung: false.into(),
        });
        let product = serve(
            fixture,
            Some(PeerCommands::new(
                records.clone(),
                RuntimeDependencies::default().clock,
                audit.clone(),
                Arc::new(TcpPeerConnector),
                os_entropy(),
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

    // Success: a forget's intent names the peer; its outcome, the record as
    // it was and as it is left.
    assert_eq!(forget()["ok"], true);
    let kept = dialing.audit.records();
    let (requested, finished) = (&kept[2], &kept[3]);
    assert_eq!(requested["kind"], "peer_forget_requested");
    assert_eq!(requested["target"]["peerKey"], hex);
    assert_eq!(
        finished["transition"]["before"],
        json!({"phase": "pending", "address": address})
    );
    assert_eq!(finished["kind"], "peer_forget_finished");
    assert_eq!(finished["operationId"], requested["operationId"]);
    assert_eq!(finished["target"]["peerKey"], hex);
    assert_eq!(finished["transition"]["after"]["phase"], "absent");
    assert_eq!(finished["initiator"], owner);

    // Failure: a refused enrollment is kept; it never reached the record.
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
    assert_eq!(finished["transition"]["after"]["phase"], "not_read");
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
        os_entropy(),
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

/// A connect that completes only when the test lets it, to a real listener.
struct Late {
    to: SocketAddr,
    release: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    dialed: Arc<std::sync::atomic::AtomicBool>,
}
impl PeerConnector for Late {
    fn connect(&self, _: SocketAddr) -> PeerConnectFuture<'_> {
        let release = self.release.lock().unwrap().take().unwrap();
        let to = self.to;
        self.dialed.store(true, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async move {
            let _ = release.await;
            let stream = tokio::net::TcpStream::connect(to).await?.into_std()?;
            stream.set_nonblocking(false)?;
            Ok(stream)
        })
    }
}

/// Row P13, the other edge: a connect that completes once the clock has
/// passed the deadline, before a tick read it, is too late all the same. The
/// connection is closed unused and the enrollment ends `peer_unreachable`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_connect_completed_after_the_deadline_is_refused_and_closed() {
    use nessa_auth::{adapters::pairing::ManualCode, domain::PrincipalId};
    use std::io::Read;
    let fixture = Fixture::new().await;
    let dialing = Dialing::new(&fixture).await;
    let clock = Arc::new(Manual(1_000.into()));
    let peer = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let (release, released) = tokio::sync::oneshot::channel();
    let dialed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let commands = Arc::new(PeerCommands::new(
        dialing.records.clone(),
        clock.clone(),
        dialing.audit.clone(),
        Arc::new(Late {
            to: peer.local_addr().unwrap(),
            release: Mutex::new(Some(released)),
            dialed: dialed.clone(),
        }),
        os_entropy(),
    ));
    let enrolling = tokio::spawn({
        let commands = commands.clone();
        async move {
            let owner = PrincipalId::new("owner").unwrap();
            commands
                .enroll(
                    "192.0.2.1:7443".parse().unwrap(),
                    ManualCode::generate(&mut OsEntropy),
                    &owner,
                )
                .await
        }
    });
    tokio::time::timeout(WAIT, async {
        while !dialed.load(std::sync::atomic::Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the enrollment dials");
    // Past the deadline and connected at once: the connect is ready when
    // the command next looks, whether or not a tick has read the clock.
    clock.0.store(
        1_000 + CONNECT.as_millis() as u64,
        std::sync::atomic::Ordering::SeqCst,
    );
    release.send(()).unwrap();
    let ended = tokio::time::timeout(WAIT, enrolling)
        .await
        .expect("the late connection is refused, not used")
        .unwrap();
    assert_eq!(ended, Err(PeerError::Unreachable));
    let (mut accepted, _) = blocking(|| peer.accept().unwrap());
    accepted
        .set_read_timeout(Some(std::time::Duration::from_secs(30)))
        .unwrap();
    let mut byte = [0; 1];
    assert_eq!(
        blocking(|| accepted.read(&mut byte)).unwrap(),
        0,
        "closed without a byte sent"
    );
    assert!(dialing.files().is_empty());
    let finished = dialing.audit.records().pop().unwrap();
    assert_eq!(
        finished["outcome"],
        json!({"result": "refused", "code": "peer_unreachable"})
    );
}

/// One audited call's expected evidence.
struct Expected {
    /// `peer_enroll` or `peer_forget`.
    command: &'static str,
    /// The wire code, or `None` for success.
    code: Option<&'static str>,
    /// The address an enrollment dialed.
    address: Option<String>,
    /// The peer's key, when the call learned it.
    peer: Option<String>,
}

/// The operation `after` added over `before`, which must be exactly one.
fn new_operation(before: &[Value], after: &[Value]) -> String {
    let seen: std::collections::HashSet<_> = before.iter().map(|r| &r["recordId"]).collect();
    let mut added: Vec<_> = after
        .iter()
        .filter(|record| !seen.contains(&record["recordId"]))
        .map(|record| record["operationId"].as_str().unwrap().to_owned())
        .collect();
    added.dedup();
    assert_eq!(added.len(), 1, "one operation per call: {added:?}");
    added.pop().unwrap()
}

/// `operation` kept exactly one intent and then exactly one outcome, each
/// naming the initiator and the target, and the outcome the answer.
fn assert_pair(records: &[Value], operation: &str, expected: &Expected) {
    let pair: Vec<_> = records
        .iter()
        .filter(|record| record["operationId"] == operation)
        .collect();
    assert_eq!(pair.len(), 2, "one intent and one outcome: {pair:?}");
    let (requested, finished) = (pair[0], pair[1]);
    let row = format!("{}/{:?}", expected.command, expected.code);
    assert_eq!(
        requested["kind"],
        format!("{}_requested", expected.command),
        "{row}"
    );
    assert_eq!(
        finished["kind"],
        format!("{}_finished", expected.command),
        "{row}"
    );
    let owner = json!({"kind": "principal", "principalId": "owner"});
    for record in [requested, finished] {
        assert_eq!(record["initiator"], owner, "{row}");
        assert_eq!(record["cause"], "owner_requested", "{row}");
        if let Some(address) = &expected.address {
            assert_eq!(record["target"]["address"], *address, "{row}");
        }
    }
    let peer = expected.peer.clone().map_or(Value::Null, Value::from);
    assert_eq!(finished["target"]["peerKey"], peer, "{row}");
    if expected.command == "peer_forget" {
        assert_eq!(requested["target"]["peerKey"], peer, "{row}");
    }
    let outcome = match expected.code {
        None => json!({"result": "succeeded"}),
        Some(code) => json!({"result": "refused", "code": code}),
    };
    assert_eq!(finished["outcome"], outcome, "{row}");
}

/// Row P14: every way peer.enroll and peer.forget can answer, success and
/// each typed refusal, keeps exactly one intent and one outcome under one
/// operation id, naming the initiator, the target and the peer once known.
/// The table must cover every answer, so a new path that skips the audit,
/// or a new refusal left out of this table, fails here.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_peer_command_answer_keeps_one_intent_and_one_outcome() {
    use nessa_auth::{adapters::pairing::ManualCode, domain::PrincipalId};
    let fixture = Fixture::new().await;
    let (native, stop, listener, _) = fixture.listener().await;
    let a = pin_at(native).await;
    let a_hex: String = a[12..].iter().map(|byte| format!("{byte:02x}")).collect();
    let other_fixture = Fixture::new().await;
    let (other, other_stop, other_listener, _) = other_fixture.listener().await;
    let b = pin_at(other).await;
    let b_hex: String = b[12..].iter().map(|byte| format!("{byte:02x}")).collect();
    let owner = PrincipalId::new("owner").unwrap();
    let commands_over = |dialing: &Dialing| {
        Arc::new(PeerCommands::new(
            dialing.records.clone(),
            RuntimeDependencies::default().clock,
            dialing.audit.clone(),
            Arc::new(TcpPeerConnector),
            os_entropy(),
        ))
    };
    let dialing = Dialing::new(&fixture).await;
    let commands = commands_over(&dialing);
    let mut covered = std::collections::BTreeSet::new();
    let code_of = |answer: &Result<PeerEntry, PeerError>| answer.as_ref().err().map(|e| e.code());

    // One call against `dialing`'s audit, then its pair checked.
    macro_rules! row {
        ($dialing:expr, $call:expr, $expected:expr) => {{
            let before = $dialing.audit.records();
            let answer = $call;
            let expected: Expected = $expected;
            assert_eq!(code_of(&answer), expected.code);
            let after = $dialing.audit.records();
            assert_pair(&after, &new_operation(&before, &after), &expected);
            covered.insert(format!(
                "{}/{}",
                expected.command,
                expected.code.unwrap_or("ok")
            ));
        }};
    }
    let enroll = |command: &'static str, code, address: SocketAddr, peer: Option<&str>| Expected {
        command,
        code,
        address: Some(address.to_string()),
        peer: peer.map(str::to_owned),
    };
    let parse = |code: &str| ManualCode::parse(code.as_bytes()).unwrap();

    let (code, id) = invite(&fixture, ConsentClass::DeviceRead).await;
    row!(
        dialing,
        commands.enroll(native, parse(&code), &owner).await,
        enroll("peer_enroll", Some("peer_wrong_invitation"), native, None)
    );
    cancel(&fixture, id).await;
    let (code, id) = invite(&fixture, ConsentClass::PeerRead).await;
    let wrong = if code == "ABCD-2345" {
        "ABCD-2346"
    } else {
        "ABCD-2345"
    };
    row!(
        dialing,
        commands.enroll(native, parse(wrong), &owner).await,
        enroll("peer_enroll", Some("peer_invitation_refused"), native, None)
    );
    row!(
        dialing,
        commands.enroll(native, parse(&code), &owner).await,
        enroll("peer_enroll", None, native, Some(&a_hex))
    );
    cancel(&fixture, id).await;
    let (code, id) = invite(&fixture, ConsentClass::PeerRead).await;
    row!(
        dialing,
        commands.enroll(native, parse(&code), &owner).await,
        enroll("peer_enroll", Some("peer_exists"), native, Some(&a_hex))
    );
    cancel(&fixture, id).await;

    // As many peers as the list holds: one real, the rest placeholders.
    let fillers: Vec<_> = (1..nessa_protocol::product::generated::MAX_PEERS)
        .map(|index| dialing.directory.join(format!("{:064x}.json", index)))
        .collect();
    for filler in &fillers {
        drop(nessa_local_storage::open(filler, nessa_local_storage::OpenMode::CreateNew).unwrap());
    }
    let (code, id) = invite(&other_fixture, ConsentClass::PeerRead).await;
    row!(
        dialing,
        commands.enroll(other, parse(&code), &owner).await,
        enroll("peer_enroll", Some("peer_capacity"), other, Some(&b_hex))
    );
    cancel(&other_fixture, id).await;
    for filler in &fillers {
        std::fs::remove_file(filler).unwrap();
    }

    // A gateway enrolling with A's own key reaches itself at A's address.
    let own = fixture
        .keys
        .restore_gateway_key(&AudienceId::new("gateway").unwrap(), &Time)
        .unwrap()
        .unwrap();
    let itself =
        Dialing::with_identity(&fixture, "itself", NativeIdentity::restore(own).unwrap()).await;
    let (code, id) = invite(&fixture, ConsentClass::PeerRead).await;
    row!(
        itself,
        commands_over(&itself)
            .enroll(native, parse(&code), &owner)
            .await,
        enroll(
            "peer_enroll",
            Some("peer_own_gateway"),
            native,
            Some(&a_hex)
        )
    );
    cancel(&fixture, id).await;

    let forget = |code, peer: &str| Expected {
        command: "peer_forget",
        code,
        address: None,
        peer: Some(peer.to_owned()),
    };
    let nobody = DeviceKey::new([7; 32]);
    row!(
        dialing,
        commands.forget(nobody, &owner).await,
        forget(Some("peer_not_found"), &"07".repeat(32))
    );

    // An enrollment held at a peer that accepts and never speaks: every
    // other call meanwhile is busy, and it ends unreachable once dropped.
    let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let held_at = silent.local_addr().unwrap();
    let before = dialing.audit.records();
    let held = tokio::spawn({
        let commands = commands.clone();
        let owner = owner.clone();
        async move { commands.enroll(held_at, parse_code(), &owner).await }
    });
    let (socket, _) = blocking(|| silent.accept().unwrap());
    let held_operation = new_operation(&before, &dialing.audit.records());
    row!(
        dialing,
        commands
            .enroll(native, ManualCode::generate(&mut OsEntropy), &owner)
            .await,
        enroll("peer_enroll", Some("peer_busy"), native, None)
    );
    let a_key = DeviceKey::new(a[12..].try_into().unwrap());
    row!(
        dialing,
        commands.forget(a_key, &owner).await,
        forget(Some("peer_busy"), &a_hex)
    );
    drop(socket);
    let answer = tokio::time::timeout(WAIT, held).await.unwrap().unwrap();
    let expected = enroll("peer_enroll", Some("peer_unreachable"), held_at, None);
    assert_eq!(code_of(&answer), expected.code);
    assert_pair(&dialing.audit.records(), &held_operation, &expected);
    covered.insert("peer_enroll/peer_unreachable".to_owned());

    row!(
        dialing,
        commands.forget(a_key, &owner).await,
        forget(None, &a_hex)
    );

    // Every answer either command gives, but the two that need storage or
    // the audit to fail (rows P12 and the store tests cover those).
    let answers: std::collections::BTreeSet<String> = [
        "peer_enroll/ok",
        "peer_enroll/peer_busy",
        "peer_enroll/peer_unreachable",
        "peer_enroll/peer_invitation_refused",
        "peer_enroll/peer_wrong_invitation",
        "peer_enroll/peer_own_gateway",
        "peer_enroll/peer_exists",
        "peer_enroll/peer_capacity",
        "peer_forget/ok",
        "peer_forget/peer_busy",
        "peer_forget/peer_not_found",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    assert_eq!(covered, answers);
    // And nothing else was kept: every operation is one pair.
    for dialing in [&dialing, &itself] {
        let records = dialing.audit.records();
        let operations: std::collections::BTreeSet<_> = records
            .iter()
            .map(|record| record["operationId"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(records.len(), 2 * operations.len());
    }
    stop.send(()).unwrap();
    listener.await.unwrap().unwrap();
    other_stop.send(()).unwrap();
    other_listener.await.unwrap().unwrap();
}

fn parse_code() -> nessa_auth::adapters::pairing::ManualCode {
    nessa_auth::adapters::pairing::ManualCode::parse(b"ABCD-2345").unwrap()
}

/// Entropy that is never available, as a failed operating-system generator.
struct NoEntropy;
impl nessa_auth::adapters::pairing::RngCore for NoEntropy {
    fn next_u32(&mut self) -> u32 {
        0
    }
    fn next_u64(&mut self) -> u64 {
        0
    }
    fn fill_bytes(&mut self, bytes: &mut [u8]) {
        bytes.fill(0);
    }
    fn try_fill_bytes(
        &mut self,
        _: &mut [u8],
    ) -> Result<(), nessa_auth::adapters::pairing::rand::Error> {
        use nessa_auth::adapters::pairing::rand::Error;
        Err(Error::from(
            std::num::NonZeroU32::new(Error::CUSTOM_START).unwrap(),
        ))
    }
}
impl nessa_auth::adapters::pairing::CryptoRng for NoEntropy {}

/// Row P15: each enrollment takes its entropy from the injected source. One
/// that fails ends the enrollment `peer_unavailable`, saving nothing, with
/// its intent and outcome kept like any other.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_enrollment_whose_entropy_fails_is_unavailable_and_audited() {
    use nessa_auth::{adapters::pairing::ManualCode, domain::PrincipalId};
    let fixture = Fixture::new().await;
    let (native, stop, listener, _) = fixture.listener().await;
    let dialing = Dialing::new(&fixture).await;
    let commands = PeerCommands::new(
        dialing.records.clone(),
        RuntimeDependencies::default().clock,
        dialing.audit.clone(),
        Arc::new(TcpPeerConnector),
        Arc::new(|| Box::new(NoEntropy) as Box<dyn EnrollmentEntropy>),
    );
    let (code, id) = invite(&fixture, ConsentClass::PeerRead).await;
    let owner = PrincipalId::new("owner").unwrap();
    let answer = commands
        .enroll(native, ManualCode::parse(code.as_bytes()).unwrap(), &owner)
        .await;
    assert_eq!(answer, Err(PeerError::Unavailable));
    assert!(dialing.files().is_empty(), "nothing is saved");
    let records = dialing.audit.records();
    let operation = records[0]["operationId"].as_str().unwrap().to_owned();
    assert_pair(
        &records,
        &operation,
        &Expected {
            command: "peer_enroll",
            code: Some("peer_unavailable"),
            address: Some(native.to_string()),
            peer: None,
        },
    );
    assert_eq!(records.len(), 2);
    cancel(&fixture, id).await;
    stop.send(()).unwrap();
    listener.await.unwrap().unwrap();
}

/// Wait, bounded, until the audit has left a record unanswered.
async fn until_hung(audit: &Audit) {
    tokio::time::timeout(WAIT, async {
        while !audit.hung.load(std::sync::atomic::Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the audit is handed the record");
}

/// Row P16: each audit record gets `AUDIT_DEADLINE` by the injected clock.
/// An intent never acknowledged ends the call `peer_audit_unavailable` once
/// the clock passes it, with nothing dialed; an outcome never acknowledged
/// does the same while the effect stands.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_audit_that_never_answers_ends_the_call_at_its_deadline() {
    use nessa_auth::{adapters::pairing::ManualCode, domain::PrincipalId};
    use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
    let fixture = Fixture::new().await;
    let dialing = Dialing::new(&fixture).await;
    let clock = Arc::new(Manual(1_000.into()));
    let dialed = Arc::new(AtomicBool::new(false));
    let commands = Arc::new(PeerCommands::new(
        dialing.records.clone(),
        clock.clone(),
        dialing.audit.clone(),
        Arc::new(Never {
            dialed: dialed.clone(),
            dropped: Arc::new(AtomicBool::new(false)),
        }),
        os_entropy(),
    ));
    let owner = PrincipalId::new("owner").unwrap();
    let deadline = AUDIT_DEADLINE.as_millis() as u64;

    // The intent: held while the clock stands, refused once it passes.
    *dialing.audit.hang.lock().unwrap() = Some("peer_enroll_requested");
    let mut enrolling = tokio::spawn({
        let (commands, owner) = (commands.clone(), owner.clone());
        async move {
            commands
                .enroll(
                    "192.0.2.1:7443".parse().unwrap(),
                    ManualCode::generate(&mut OsEntropy),
                    &owner,
                )
                .await
        }
    });
    until_hung(&dialing.audit).await;
    clock.0.store(1_000 + deadline - 1, SeqCst);
    let held = tokio::time::timeout(
        nessa_protocol::pairing::socket::WAKE_TICK * 3,
        &mut enrolling,
    )
    .await;
    assert!(held.is_err(), "the call waits on its intent");
    clock.0.store(1_000 + deadline, SeqCst);
    let answer = tokio::time::timeout(WAIT, enrolling)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(answer, Err(PeerError::AuditUnavailable));
    assert!(!dialed.load(SeqCst), "nothing is dialed without an intent");
    assert!(dialing.audit.records().is_empty());

    // The outcome: a kept peer is forgotten, and the answer refused at the
    // deadline while the removal stands.
    let intent = PublicIntent::new(
        InvitationId::new([1; 16]),
        AttemptId::new([2; 16]),
        ConsentIntentId::new([3; 16]),
        1,
        1,
        ConsentClass::PeerRead,
    )
    .unwrap();
    let peer = NativeIdentity::generate(&mut OsEntropy).unwrap();
    dialing
        .records
        .enrolling("127.0.0.1:7443".parse().unwrap())
        .save_pending(
            dialing.identity.key_material(),
            &peer.public_spki(),
            intent,
            None,
        )
        .unwrap();
    let key = DeviceKey::new(peer.public_spki()[12..].try_into().unwrap());
    dialing.audit.hung.store(false, SeqCst);
    *dialing.audit.hang.lock().unwrap() = Some("peer_forget_finished");
    let forgetting = tokio::spawn({
        let (commands, owner) = (commands.clone(), owner.clone());
        async move { commands.forget(key, &owner).await }
    });
    until_hung(&dialing.audit).await;
    let now = clock.0.load(SeqCst);
    clock.0.store(now + deadline, SeqCst);
    let answer = tokio::time::timeout(WAIT, forgetting)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(answer, Err(PeerError::AuditUnavailable));
    assert!(dialing.files().is_empty(), "the removal stands");
    let kept = dialing.audit.records();
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert_eq!(kept[0]["kind"], "peer_forget_requested");
}

/// The poller names a status's change by the transition the record's store
/// began, not by what the record holds afterwards: an approval whose save
/// went through, and an ending whose cache removal then failed, leaving the
/// record active as it was, are each named by what was begun.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_slot_names_the_transition_a_status_began_whatever_storage_did() {
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
    let peer = NativeIdentity::generate(&mut OsEntropy).unwrap();
    dialing
        .records
        .enrolling("127.0.0.1:7443".parse().unwrap())
        .save_pending(
            dialing.identity.key_material(),
            &peer.public_spki(),
            intent,
            None,
        )
        .unwrap();
    let key = DeviceKey::new(peer.public_spki()[12..].try_into().unwrap());
    let active = |records: &PeerRecords| match records.get(&key).unwrap() {
        Some(PeerEntry::Readable(record)) => matches!(record.phase(), PeerPhase::Active { .. }),
        other => panic!("{other:?}"),
    };

    // A status that saves nothing begins nothing.
    let slot = dialing.records.slot(key);
    assert_eq!(slot.transition(), None);
    assert!(slot.load_credential().unwrap().is_none());
    assert_eq!(slot.transition(), None);

    let credential = nessa_auth::domain::CredentialId::new("credential").unwrap();
    let receiver = ResourceId::new("receiver").unwrap();
    slot.save_credential(&credential, &receiver, intent)
        .unwrap();
    assert_eq!(slot.transition(), Some(SlotTransition::Approved));
    assert!(active(&dialing.records));
    // Saved again as it is: nothing begun.
    let again = dialing.records.slot(key);
    again
        .save_credential(&credential, &receiver, intent)
        .unwrap();
    assert_eq!(again.transition(), None);

    // The ending's cache removal fails: a directory stands where SQLite's
    // journal would be. The record is left active, and the change is still
    // the ending that was begun.
    let journal = dialing
        .records
        .cache_path(&key)
        .with_extension("sqlite3-journal");
    std::fs::create_dir(&journal).unwrap();
    std::fs::write(journal.join("held"), b"").unwrap();
    let ending = dialing.records.slot(key);
    assert!(ending.end_enrollment(intent).is_err());
    assert_eq!(ending.transition(), Some(SlotTransition::Ended));
    assert!(active(&dialing.records), "the record is as it was");
}
