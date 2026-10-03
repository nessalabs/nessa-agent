//! Startup and shutdown of mounted native pairing over the real registry, Cedar
//! and private state (design rows S4, S10, S12 in
//! `docs/design/auth/device-pairing.md`, "Owner routes and mounting").
use super::*;
use crate::app::dependencies::RuntimeDependencies;
use crate::composition::local_auth::SystemClock;
use crate::device_pairing::infrastructure::{GatewayIdentityError, PairingRuntimeError};
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
    /// The namespace's receiver-access store, as composition opens it.
    fn receivers(&self) -> Arc<LocalReceiverAuthority> {
        let root = self.root().join("conversations");
        nessa_local_storage::create_directory(&root).unwrap();
        Arc::new(
            LocalReceiverAuthority::open(
                &root.join("receiver-access.sqlite3"),
                "policy",
                Arc::new(SystemClock),
            )
            .unwrap(),
        )
    }
    fn registry(&self) -> Arc<LocalCredentialStore> {
        Arc::new(LocalCredentialStore::open(self.root().join("auth"), "credentials.json").unwrap())
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
            gateway: Resource::new(
                OrganizationId::new("org").unwrap(),
                ResourceId::new("gateway").unwrap(),
            ),
        }
    }
    fn key_file(&self) -> PathBuf {
        self.root().join(PRIVATE_DIRECTORY).join("gateway-key")
    }
}

fn loopback() -> NativeConfig {
    NativeConfig {
        listen_address: "127.0.0.1:0".parse().unwrap(),
    }
}

/// Row S4: a missing key with enrollment history is refused, never regenerated,
/// whether its audit evidence went with it or stayed behind to contradict it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_startup_history_without_key_refuses_before_bind() {
    for whole_directory in [false, true] {
        let namespace = Namespace::new();
        let session = namespace.bootstrap().await;
        let registry = namespace.registry();
        let (prepared, commands) = prepare(&loopback(), namespace.inputs(registry.clone()))
            .await
            .unwrap();
        // A first start publishes the key; an invitation is enrollment history.
        assert!(namespace.key_file().exists());
        let created = commands.create(&session).await.unwrap();
        drop((prepared, commands));
        drop(registry);
        if whole_directory {
            std::fs::remove_dir_all(namespace.root().join(PRIVATE_DIRECTORY)).unwrap();
        } else {
            std::fs::remove_file(namespace.key_file()).unwrap();
        }
        let registry = namespace.registry();
        let refused = prepare(&loopback(), namespace.inputs(registry.clone())).await;
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
    let (prepared, commands) = prepare(
        &NativeConfig {
            listen_address: address,
        },
        namespace.inputs(registry.clone()),
    )
    .await
    .unwrap();
    let created = commands.create(&session).await.unwrap();
    let key = std::fs::read(namespace.key_file()).unwrap();
    let Err(RunError::Native(NativeFailure::Bind {
        address: refused,
        source,
    })) = bind(prepared, RuntimeDependencies::default().clock).await
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
    drop(commands);
    drop(registry);
    drop(taken);
    let (prepared, _commands) = prepare(&loopback(), namespace.inputs(namespace.registry()))
        .await
        .unwrap();
    bind(prepared, RuntimeDependencies::default().clock)
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
    let (prepared, commands) = prepare(&loopback(), namespace.inputs(namespace.registry()))
        .await
        .unwrap();
    let bound = bind(prepared, RuntimeDependencies::default().clock)
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
        commands.create(&session).await,
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
    let (prepared, _commands) = prepare(&loopback(), namespace.inputs(namespace.registry()))
        .await
        .unwrap();
    let bound = bind(prepared, RuntimeDependencies::default().clock)
        .await
        .unwrap();
    let running = RunningNative {
        gateway: bound.gateway.clone(),
        stop: None,
        task: tokio::spawn(async { panic!("native listener fault") }),
    };
    assert_eq!(
        tokio::time::timeout(WAIT, running.join()).await.unwrap(),
        Err(NativeShutdownFailure::ListenerFault(
            PairingWorkerFault::Panic
        ))
    );
}
