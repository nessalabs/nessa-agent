//! The dialing side of a peer gateway over the real `/session` route, Cedar,
//! and a real peer: gateway B enrolls, with its own native key, into gateway
//! A's peer invitation, keeping a reference to that key and never a copy.
//! Rows P1–P11 in `docs/design/auth/peer-gateways.md` ("The dialing side").
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
    peer_gateways::infrastructure::{PeerCommands, PeerEntry, PeerPhase, PeerRecords, SlotRefusal},
    product::{ProductDependencies, ProductRouteState},
    server::entrypoint::http,
};
use serde_json::{json, Value};
use std::{
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    sync::Arc,
};

struct NoAgents;
impl AgentProbe for NoAgents {
    fn evidence(&self, _: AgentId) -> Option<AgentProbeEvidence> {
        None
    }
}

/// Gateway B: its own native key, its peer records, and a product socket the
/// fixture's owner authenticates to.
struct Dialing {
    identity: NativeIdentity,
    records: Arc<PeerRecords>,
    directory: PathBuf,
    product: SocketAddr,
}
impl Dialing {
    async fn new(fixture: &Fixture) -> Self {
        let root = private_root(fixture.directory.path(), "dialing");
        for directory in ["native-pairing", "peer-gateways"] {
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
        let product = serve(
            fixture,
            Some(PeerCommands::new(
                records.clone(),
                RuntimeDependencies::default().clock,
            )),
        )
        .await;
        Self {
            identity,
            records,
            directory: root.join("peer-gateways"),
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
