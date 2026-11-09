//! Startup and shutdown of mounted native pairing over the real registry, Cedar
//! and private state (design rows S4, S10, S12 in
//! `docs/design/auth/device-pairing.md`, "Owner routes and mounting").
use super::*;
use crate::agents::application::{AgentProbe, AgentProbeEvidence};
use crate::app::dependencies::RuntimeDependencies;
use crate::composition::local_auth::SystemClock;
use crate::device_pairing::infrastructure::{GatewayIdentityError, PairingRuntimeError};
use crate::product::ProductDependencies;
use nessa_auth::domain::pairing::ConsentClass;
use nessa_auth::{
    adapters::{
        cedar::CedarPolicyEvaluator,
        local::{BootstrapRequest, LocalCredentialStore},
    },
    application::{
        dto::{
            CredentialGrantDto, MembershipInputDto, MembershipRoleDto, MembershipStateDto,
            OrganizationInputDto, PrincipalInputDto, PrincipalKindDto, ResourceDto,
        },
        pairing::{PairingStore, PairingStoreError, PrivateStateError},
        session::{AuthenticateSession, AuthenticatedSession},
    },
    domain::{AudienceId, OrganizationId, ResourceId},
};
use nessa_protocol::agents::AgentId;
use std::{io::Read, time::Duration};
use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(30);

struct Namespace {
    directory: TempDir,
}
impl Namespace {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        // A namespace directory is private, as the gateway creates it.
        nessa_local_storage::create_directory(&directory.path().join("namespace/auth")).unwrap();
        Self { directory }
    }
    fn root(&self) -> PathBuf {
        self.directory.path().join("namespace")
    }
    /// The namespace's receiver journal, as composition opens it.
    fn receivers(&self) -> Arc<LocalReceiverAuthority> {
        let path = super::super::local_auth::receiver_journal(&self.root());
        nessa_local_storage::create_directory(path.parent().unwrap()).unwrap();
        Arc::new(LocalReceiverAuthority::open(&path, "policy", Arc::new(SystemClock)).unwrap())
    }
    fn registry(&self) -> Arc<LocalCredentialStore> {
        Arc::new(
            LocalCredentialStore::open(
                self.root().join("auth"),
                "credentials.json",
                std::sync::Arc::new(
                    nessa_auth::adapters::cedar::CedarPolicyEvaluator::new().unwrap(),
                ),
            )
            .unwrap(),
        )
    }
    /// The bootstrapped owner's session, on a registry this test then drops.
    async fn bootstrap(&self) -> AuthenticatedSession {
        let registry = self.registry();
        let issued = registry
            .bootstrap(BootstrapRequest {
                gateway_id: "gateway".into(),
                organization: OrganizationInputDto { id: "org".into() },
                principal: PrincipalInputDto {
                    id: "owner".into(),
                    kind: PrincipalKindDto::Human,
                },
                membership: MembershipInputDto {
                    id: "member".into(),
                    principal_id: "owner".into(),
                    organization_id: "org".into(),
                    role: MembershipRoleDto::Admin,
                    state: MembershipStateDto::Active,
                },
                credential_id: "credential".into(),
                issued_at: SystemClock.unix_seconds() - 1,
                expires_at: None,
                grants: ["credential.manage", "conversation.read"]
                    .into_iter()
                    .map(|action| CredentialGrantDto {
                        action: action.into(),
                        resource: ResourceDto {
                            organization_id: "org".into(),
                            id: "gateway".into(),
                        },
                    })
                    .collect(),
            })
            .unwrap();
        AuthenticateSession {
            policy: &nessa_auth::adapters::cedar::CedarPolicyEvaluator::new().unwrap(),
            verifier: registry.as_ref(),
            access: registry.as_ref(),
            clock: &SystemClock,
        }
        .execute(&issued.evidence, &AudienceId::new("gateway").unwrap())
        .await
        .unwrap()
    }
    fn inputs(&self, registry: Arc<LocalCredentialStore>) -> NativeInputs {
        NativeInputs {
            namespace: self.root(),
            registry,
            policy: Arc::new(CedarPolicyEvaluator::new().unwrap()),
            receivers: self.receivers(),
            clock: Arc::new(SystemClock),
            deadline_clock: RuntimeDependencies::default().clock,
            gateway: Resource::new(
                OrganizationId::new("org").unwrap(),
                ResourceId::new("gateway").unwrap(),
            ),
            audience: AudienceId::new("gateway").unwrap(),
        }
    }
    fn key_file(&self) -> PathBuf {
        self.root().join(PRIVATE_DIRECTORY).join("gateway-key")
    }
}

/// The product state a protected native session would be served with.
fn product(registry: Arc<LocalCredentialStore>) -> ProductRouteState {
    struct NoAgents;
    impl AgentProbe for NoAgents {
        fn evidence(&self, _: AgentId) -> Option<AgentProbeEvidence> {
            None
        }
    }
    ProductRouteState::new(
        ResourceId::new("gateway").unwrap(),
        OrganizationId::new("org").unwrap(),
        AudienceId::new("gateway").unwrap(),
        ProductDependencies {
            owner_phone_purpose: registry.clone(),
            owner_phone_policy: Arc::new(
                crate::conversation::infrastructure::CedarOwnerPhonePolicyEvaluator::new().unwrap(),
            ),
            owner_phone_receivers: None,
            verifier: registry.clone(),
            access: registry,
            clock: Arc::new(SystemClock),
            policy: Arc::new(CedarPolicyEvaluator::new().unwrap()),
            uptime_clock: RuntimeDependencies::default().clock,
            agent_probe: Arc::new(NoAgents),
        },
    )
}

/// A specific loopback bind on a fixed port. The lifecycle table publishes
/// that address. The listener stays owned until `bind` adopts it. A
/// configured port of zero stays unpublished, including after the socket is
/// given an ephemeral port, so tests that need an unpublished listener do
/// not use this.
fn loopback() -> (NativeConfig, std::net::TcpListener) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let listen_address = listener.local_addr().unwrap();
    (
        NativeConfig {
            listen_address,
            relay: None,
            advertised_address: None,
            tunnel: None,
        },
        listener,
    )
}

/// Port zero. Nothing is published, and no port is reserved.
fn unpublished() -> NativeConfig {
    NativeConfig {
        listen_address: "127.0.0.1:0".parse().unwrap(),
        relay: None,
        advertised_address: None,
        tunnel: None,
    }
}

/// Hand `listener` to `prepared` so `bind` accepts on it instead of binding
/// the address again.
fn adopt(prepared: PreparedNative, listener: std::net::TcpListener) -> PreparedNative {
    listener.set_nonblocking(true).unwrap();
    prepared.keep_bound_socket(tokio::net::TcpListener::from_std(listener).unwrap())
}

async fn prepared_loopback(
    inputs: NativeInputs,
) -> (
    PreparedNative,
    PairingOwnerCommands,
    std::sync::Arc<PeerCommands>,
) {
    let (config, listener) = loopback();
    let (prepared, commands, peers) = prepare(&config, inputs).await.unwrap();
    (adopt(prepared, listener), commands, peers)
}

/// Row S4: a missing key with enrollment history is refused, never regenerated,
/// whether its audit evidence went with it or stayed behind to contradict it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_startup_history_without_key_refuses_before_bind() {
    for whole_directory in [false, true] {
        let namespace = Namespace::new();
        let session = namespace.bootstrap().await;
        let registry = namespace.registry();
        let (prepared, commands, peers) =
            prepare(&unpublished(), namespace.inputs(registry.clone()))
                .await
                .unwrap();
        // A first start publishes the key; an invitation is enrollment history.
        assert!(namespace.key_file().exists());
        let created = commands
            .create(&session, ConsentClass::DeviceRead)
            .await
            .unwrap();
        // The peer commands hold the same key store, so they go too.
        drop((prepared, commands, peers));
        drop(registry);
        if whole_directory {
            std::fs::remove_dir_all(namespace.root().join(PRIVATE_DIRECTORY)).unwrap();
        } else {
            std::fs::remove_file(namespace.key_file()).unwrap();
        }
        let registry = namespace.registry();
        let refused = prepare(&unpublished(), namespace.inputs(registry.clone())).await;
        let refused = refused.err();
        assert!(
            if whole_directory {
                matches!(
                    refused,
                    Some(RunError::Native(NativeFailure::Identity(
                        GatewayIdentityError::Registry(PairingStoreError::GatewayKeyHistoryExists)
                    )))
                )
            } else {
                matches!(
                    refused,
                    Some(RunError::Native(NativeFailure::Identity(
                        GatewayIdentityError::PrivateState(PrivateStateError::Corrupt)
                    )))
                )
            },
            "{refused:?}"
        );
        assert!(!namespace.key_file().exists(), "no key is regenerated");
        assert_eq!(
            registry.read_pairing(created.record().id()).unwrap(),
            *created.record(),
            "history is not erased"
        );
    }
}

/// Row S10: a bind failure is typed and leaves the key and history as they were.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_bind_failure_preserves_key_and_history() {
    let namespace = Namespace::new();
    let session = namespace.bootstrap().await;
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = taken.local_addr().unwrap();
    let registry = namespace.registry();
    let (prepared, commands, peers) = prepare(
        &NativeConfig {
            listen_address: address,
            relay: None,
            advertised_address: None,
            tunnel: None,
        },
        namespace.inputs(registry.clone()),
    )
    .await
    .unwrap();
    let created = commands
        .create(&session, ConsentClass::DeviceRead)
        .await
        .unwrap();
    let key = std::fs::read(namespace.key_file()).unwrap();
    let Err(RunError::Native(NativeFailure::Bind {
        address: refused,
        source,
    })) = bind(
        prepared,
        RuntimeDependencies::default().clock,
        product(registry.clone()),
    )
    .await
    else {
        panic!("a taken native address must refuse startup");
    };
    assert_eq!(refused, address);
    assert_eq!(source.kind(), ErrorKind::AddrInUse);
    assert_eq!(std::fs::read(namespace.key_file()).unwrap(), key);
    assert_eq!(
        registry.read_pairing(created.record().id()).unwrap(),
        *created.record()
    );
    // Once the address is free, the same key is restored, not regenerated.
    drop((commands, peers));
    drop(registry);
    drop(taken);
    let registry = namespace.registry();
    let (prepared, _commands, _peers) = prepared_loopback(namespace.inputs(registry.clone())).await;
    bind(
        prepared,
        RuntimeDependencies::default().clock,
        product(registry),
    )
    .await
    .unwrap();
    assert_eq!(std::fs::read(namespace.key_file()).unwrap(), key);
}

/// Row S12: stop wakes a held peer and the join waits for its drain and the
/// runtime's own shutdown.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_shutdown_joins_a_held_peer() {
    let namespace = Namespace::new();
    let session = namespace.bootstrap().await;
    let registry = namespace.registry();
    let (prepared, commands, _peers) = prepared_loopback(namespace.inputs(registry.clone())).await;
    let bound = bind(
        prepared,
        RuntimeDependencies::default().clock,
        product(registry),
    )
    .await
    .unwrap();
    let address = bound.local_address();
    let (failure, failed) = watch::channel(None);
    let mut running = start(bound, failure);
    // A peer that connects and never speaks holds a connection worker in TLS.
    let mut peer = std::net::TcpStream::connect(address).unwrap();
    peer.set_read_timeout(Some(WAIT)).unwrap();
    // Admission is proven by the worker holding the socket open, not by a
    // reply; give the listener a moment to accept it.
    tokio::time::sleep(Duration::from_millis(100)).await;
    running.signal_stop();
    let joined = tokio::time::timeout(WAIT, running.join()).await;
    assert!(
        matches!(joined, Ok(Ok(()))),
        "the drain is joined: {joined:?}"
    );
    // The held peer was woken: its socket ends rather than waiting for TLS.
    let mut byte = [0; 1];
    assert!(matches!(peer.read(&mut byte), Ok(0) | Err(_)));
    // The runtime's create admission is closed by the join.
    assert!(matches!(
        commands.create(&session, ConsentClass::DeviceRead).await,
        Err(PairingRuntimeError::ShuttingDown)
    ));
    assert!(
        failed.borrow().is_none(),
        "a stop is not a listener failure"
    );
}

/// Row D6: a listener task that faulted leaves its drain unknown, so `join`
/// returns that fault and does not go on to reconcile.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn faulted_listener_join_reports_the_fault_without_reconciling() {
    let namespace = Namespace::new();
    namespace.bootstrap().await;
    let registry = namespace.registry();
    let (prepared, _commands, _peers) = prepared_loopback(namespace.inputs(registry.clone())).await;
    let bound = bind(
        prepared,
        RuntimeDependencies::default().clock,
        product(registry),
    )
    .await
    .unwrap();
    let running = RunningNative {
        gateway: bound.gateway.clone(),
        stop: None,
        task: tokio::spawn(async { panic!("native listener fault") }),
        relay: None,
        poller: bound.start_poller(),
        peers: bound.peers.clone(),
        audit: bound.audit.clone(),
        announced: None,
        tunnel: None,
    };
    assert_eq!(
        tokio::time::timeout(WAIT, running.join()).await.unwrap(),
        Err(NativeShutdownFailure::ListenerFault(
            PairingWorkerFault::Panic
        ))
    );
}

/// A device's Hello over TLS pinned to `pin` on `stream`: whether the
/// gateway answered. With no invitation open the gateway refuses the Hello,
/// so a `Refused` decoded over that TLS is the answer looked for.
fn answered(stream: std::net::TcpStream, pin: [u8; 44]) -> bool {
    use nessa_auth::{
        adapters::pairing::{GatewayTrust, NativeIdentity, NativeTransport, OsEntropy},
        domain::pairing::AttemptId,
    };
    use nessa_protocol::pairing::{
        wire::{decode_reply, encode_request, NativePairingReply, NativePairingRequest},
        EnrollmentChannel,
    };
    let identity = NativeIdentity::generate(&mut OsEntropy).unwrap();
    let Ok(transport) = NativeTransport::connect(stream, &identity, GatewayTrust::Pinned(pin))
    else {
        return false;
    };
    let mut channel = EnrollmentChannel::new(transport);
    let attempt = AttemptId::new([113; AttemptId::LENGTH]);
    channel
        .send_envelope(&encode_request(&NativePairingRequest::Hello(attempt)).unwrap())
        .unwrap();
    let reply = channel.receive_envelope().map(|bytes| decode_reply(&bytes));
    matches!(reply, Ok(Ok(NativePairingReply::Refused)))
}

/// Issue #266, slice R2a: a gateway configured with a relay registers with
/// it at start, and a reader that reaches only the relay completes TLS
/// pinned to the gateway's key and is answered; stop ends the registration
/// with the listener.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_gateway_with_a_relay_answers_a_reader_that_reaches_only_the_relay() {
    use crate::relay::{
        application::{Relay, RelayLimits},
        infrastructure::{serve_tcp, Ed25519Proofs, OsRelayEntropy},
    };
    use crate::timer::{Timer, TokioTimer};
    use nessa_protocol::relay::{read_frame, write_frame, RelayFrame};

    let relay_socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay_address = relay_socket.local_addr().unwrap();
    let timer: Arc<dyn Timer> = Arc::new(TokioTimer::new());
    let relay = Relay::new(
        timer.clone(),
        Arc::new(OsRelayEntropy),
        Arc::new(Ed25519Proofs),
        RelayLimits::default(),
    );
    let (stop_relay, relay_stopped) = oneshot::channel::<()>();
    let relay_task = tokio::spawn(serve_tcp(relay_socket, relay, timer, async {
        relay_stopped.await.ok();
    }));

    let namespace = Namespace::new();
    namespace.bootstrap().await;
    let registry = namespace.registry();
    let config = NativeConfig {
        listen_address: "127.0.0.1:0".parse().unwrap(),
        relay: Some(RelayAddress::try_from(relay_address.to_string()).unwrap()),
        advertised_address: None,
        tunnel: None,
    };
    let (prepared, _commands, _peers) = prepare(&config, namespace.inputs(registry.clone()))
        .await
        .unwrap();
    let bound = bind(
        prepared,
        RuntimeDependencies::default().clock,
        product(registry),
    )
    .await
    .unwrap();
    let key = bound.gateway.identity().public_key();
    let pin = bound.gateway.identity().public_spki();
    let (failure, failed) = watch::channel(None);
    let mut running = start(bound, failure);

    let answered = tokio::task::spawn_blocking(move || {
        // The registration may not have landed yet: ask until joined.
        let deadline = std::time::Instant::now() + WAIT;
        let stream = loop {
            let mut stream = std::net::TcpStream::connect(relay_address).unwrap();
            stream.set_read_timeout(Some(WAIT)).unwrap();
            write_frame(&mut stream, &RelayFrame::Connect { key }).unwrap();
            match read_frame(&mut stream).unwrap() {
                RelayFrame::Joined => break stream,
                RelayFrame::Offline if std::time::Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                other => panic!("the relay answered {other:?}"),
            }
        };
        answered(stream, pin)
    });
    assert!(
        tokio::time::timeout(WAIT, answered).await.unwrap().unwrap(),
        "the gateway answered through the relay"
    );

    running.signal_stop();
    let joined = tokio::time::timeout(WAIT, running.join()).await;
    assert!(
        matches!(joined, Ok(Ok(()))),
        "the drain is joined: {joined:?}"
    );
    assert!(failed.borrow().is_none());
    stop_relay.send(()).unwrap();
    relay_task.await.unwrap();
}

/// Issue #266, row RL15 and slice R2a's "no relay fault ends the
/// listener": with the relay out of reach, direct readers are answered,
/// the listener reports no failure, and stop still joins cleanly.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_relay_that_cannot_be_reached_leaves_direct_readers_served() {
    // A port bound and released: nothing listens there.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let nowhere = closed.local_addr().unwrap();
    drop(closed);
    let namespace = Namespace::new();
    namespace.bootstrap().await;
    let registry = namespace.registry();
    let config = NativeConfig {
        listen_address: "127.0.0.1:0".parse().unwrap(),
        relay: Some(RelayAddress::try_from(nowhere.to_string()).unwrap()),
        advertised_address: None,
        tunnel: None,
    };
    let (prepared, _commands, _peers) = prepare(&config, namespace.inputs(registry.clone()))
        .await
        .unwrap();
    let bound = bind(
        prepared,
        RuntimeDependencies::default().clock,
        product(registry),
    )
    .await
    .unwrap();
    let address = bound.local_address();
    let pin = bound.gateway.identity().public_spki();
    let (failure, failed) = watch::channel(None);
    let mut running = start(bound, failure);
    let direct = tokio::task::spawn_blocking(move || {
        let stream = std::net::TcpStream::connect(address).unwrap();
        stream.set_read_timeout(Some(WAIT)).unwrap();
        answered(stream, pin)
    });
    assert!(
        tokio::time::timeout(WAIT, direct).await.unwrap().unwrap(),
        "a direct reader is answered while the relay is out of reach"
    );
    assert!(
        failed.borrow().is_none(),
        "the relay never ends the listener"
    );
    running.signal_stop();
    let joined = tokio::time::timeout(WAIT, running.join()).await;
    assert!(
        matches!(joined, Ok(Ok(()))),
        "the drain is joined: {joined:?}"
    );
}

#[path = "owner_phone.rs"]
mod owner_phone;
#[path = "phone_endpoint.rs"]
mod phone_endpoint;

/// What the fake announcer was asked: each announcement, and how many were
/// withdrawn.
#[derive(Default)]
struct Announcements {
    made: std::sync::Mutex<Vec<LinkingAnnouncement>>,
    withdrawn: std::sync::atomic::AtomicUsize,
}
struct FakeAnnouncer(Arc<Announcements>);
struct FakeAnnounced(Arc<Announcements>);
impl LinkingAnnouncer for FakeAnnouncer {
    fn announce(
        &self,
        announcement: &LinkingAnnouncement,
    ) -> Result<Box<dyn Announced>, crate::device_pairing::application::AnnouncementUnavailable>
    {
        self.0.made.lock().unwrap().push(announcement.clone());
        Ok(Box::new(FakeAnnounced(self.0.clone())))
    }
}
impl Announced for FakeAnnounced {
    fn withdraw(self: Box<Self>) {
        self.0
            .withdrawn
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// A listener on every interface is announced with its address and the
/// gateway's id once it serves, and withdrawn as stop begins.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_network_listener_is_announced_and_withdrawn_on_stop() {
    let namespace = Namespace::new();
    namespace.bootstrap().await;
    let registry = namespace.registry();
    let listener = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
    let config = NativeConfig {
        listen_address: listener.local_addr().unwrap(),
        relay: None,
        advertised_address: None,
        tunnel: None,
    };
    let (prepared, _commands, _peers) = prepare(&config, namespace.inputs(registry.clone()))
        .await
        .unwrap();
    let announcements = Arc::new(Announcements::default());
    let bound = bind(
        adopt(prepared, listener),
        RuntimeDependencies::default().clock,
        product(registry.clone()),
    )
    .await
    .unwrap()
    .with_announcer(Arc::new(FakeAnnouncer(announcements.clone())));
    let address = bound.local_address();
    let (failure, _failed) = watch::channel(None);
    let mut running = start(bound, failure);
    assert_eq!(
        *announcements.made.lock().unwrap(),
        [LinkingAnnouncement {
            address,
            gateway: registry.gateway_id().unwrap(),
        }]
    );
    running.signal_stop();
    assert_eq!(
        announcements
            .withdrawn
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    tokio::time::timeout(WAIT, running.join())
        .await
        .unwrap()
        .unwrap();
}

/// A loopback listener serves this machine alone: nothing is announced.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_loopback_listener_is_not_announced() {
    let namespace = Namespace::new();
    namespace.bootstrap().await;
    let registry = namespace.registry();
    let (prepared, _commands, _peers) = prepared_loopback(namespace.inputs(registry.clone())).await;
    let announcements = Arc::new(Announcements::default());
    let bound = bind(
        prepared,
        RuntimeDependencies::default().clock,
        product(registry),
    )
    .await
    .unwrap()
    .with_announcer(Arc::new(FakeAnnouncer(announcements.clone())));
    let (failure, _failed) = watch::channel(None);
    let running = start(bound, failure);
    assert!(announcements.made.lock().unwrap().is_empty());
    tokio::time::timeout(WAIT, running.join())
        .await
        .unwrap()
        .unwrap();
}

/// A network listener on an ephemeral port is not one the next start
/// repeats: nothing is announced, though the port it got is not zero.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_ephemeral_network_listener_is_not_announced() {
    let namespace = Namespace::new();
    namespace.bootstrap().await;
    let registry = namespace.registry();
    let config = NativeConfig {
        listen_address: "0.0.0.0:0".parse().unwrap(),
        relay: None,
        advertised_address: None,
        tunnel: None,
    };
    let (prepared, _commands, _peers) = prepare(&config, namespace.inputs(registry.clone()))
        .await
        .unwrap();
    let announcements = Arc::new(Announcements::default());
    let bound = bind(
        prepared,
        RuntimeDependencies::default().clock,
        product(registry),
    )
    .await
    .unwrap()
    .with_announcer(Arc::new(FakeAnnouncer(announcements.clone())));
    assert_ne!(bound.local_address().port(), 0);
    let (failure, _failed) = watch::channel(None);
    let running = start(bound, failure);
    assert!(announcements.made.lock().unwrap().is_empty());
    tokio::time::timeout(WAIT, running.join())
        .await
        .unwrap()
        .unwrap();
}
