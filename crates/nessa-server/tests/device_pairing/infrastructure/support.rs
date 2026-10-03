//! Real registry/Cedar/private roots and actual injected native sockets.
use nessa_auth::{
    adapters::{
        cedar::CedarPolicyEvaluator,
        local::{BootstrapRequest, LocalCredentialStore},
        pairing::{FilePairingState, OsEntropy},
    },
    application::{
        dto::{
            CredentialGrantDto, MembershipInputDto, MembershipRoleDto, MembershipStateDto,
            OrganizationInputDto, PrincipalInputDto, PrincipalKindDto, ResourceDto,
        },
        ports::Clock,
        session::{AuthenticateSession, AuthenticatedSession},
    },
    domain::{AudienceId, OrganizationId, Resource, ResourceId},
};
use nessa_server::{
    app::dependencies::RuntimeDependencies,
    device_pairing::infrastructure::{
        restore_gateway_identity, GatewayPairing, NativeEnrollmentConnections,
        NativeEnrollmentListener, PairingRuntimeDependencies,
    },
};
use std::{
    io::Result as IoResult,
    net::{SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tempfile::TempDir;
use tokio::{net::TcpListener as TokioTcpListener, sync::oneshot, task::JoinHandle};

/// Wall time the owner session is authenticated at.
pub const NOW_MS: u64 = 110_001;
pub struct Time;
impl Clock for Time {
    fn unix_milliseconds(&self) -> u64 {
        NOW_MS
    }
}
/// The gateway runtime's wall clock, which a test may move forward.
pub struct GatewayTime(AtomicU64);
impl GatewayTime {
    pub fn set(&self, unix_milliseconds: u64) {
        self.0.store(unix_milliseconds, Ordering::SeqCst);
    }
}
impl Clock for GatewayTime {
    fn unix_milliseconds(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}
pub const WAIT: Duration = Duration::from_secs(30);

pub fn private_root(parent: &Path, name: &str) -> PathBuf {
    #[cfg(unix)]
    let parent = parent.canonicalize().unwrap();
    #[cfg(not(unix))]
    let parent = parent.to_path_buf();
    let path = parent.join(name);
    nessa_local_storage::create_directory(&path).unwrap();
    path
}
pub fn pending(parent: &Path, name: &str) -> (PathBuf, Arc<FilePairingState>) {
    let root = private_root(parent, name);
    nessa_local_storage::create_directory_beneath(&root, Path::new("state")).unwrap();
    let state = Arc::new(FilePairingState::open(&root, Path::new("state")).unwrap());
    (root, state)
}

pub struct Fixture {
    pub directory: TempDir,
    pub registry: Arc<LocalCredentialStore>,
    pub session: AuthenticatedSession,
    pub gateway: Arc<GatewayPairing>,
    pub time: Arc<GatewayTime>,
}
impl Fixture {
    pub async fn new() -> Self {
        Self::with_read(true).await
    }
    pub async fn with_read(read: bool) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let registry = Arc::new(
            LocalCredentialStore::open(
                private_root(directory.path(), "registry"),
                "credentials.json",
            )
            .unwrap(),
        );
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
                issued_at: 100,
                expires_at: Some(500),
                grants: ["credential.manage", "conversation.read"]
                    .into_iter()
                    .filter(|action| read || *action != "conversation.read")
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
        let session = AuthenticateSession {
            verifier: registry.as_ref(),
            access: registry.as_ref(),
            clock: &Time,
        }
        .execute(&issued.evidence, &AudienceId::new("gateway").unwrap())
        .await
        .unwrap();
        let (_, keys) = pending(directory.path(), "gateway-private");
        let identity =
            restore_gateway_identity(registry.clone(), keys.clone(), Arc::new(Time), OsEntropy)
                .await
                .unwrap();
        let resource = Resource::new(
            OrganizationId::new("org").unwrap(),
            ResourceId::new("gateway").unwrap(),
        );
        let time = Arc::new(GatewayTime(AtomicU64::new(NOW_MS)));
        let gateway = Arc::new(
            GatewayPairing::open(PairingRuntimeDependencies {
                registry: registry.clone(),
                policy: Arc::new(CedarPolicyEvaluator::new().unwrap()),
                clock: time.clone(),
                gateway: resource,
                key_store: keys,
                identity,
            })
            .unwrap(),
        );
        Self {
            directory,
            registry,
            session,
            gateway,
            time,
        }
    }
    /// Reopen actual current registry/private owners after their physical drains.
    /// No domain record or private seed is synthesized by the fixture.
    pub async fn reopen(self) -> Self {
        let Self {
            directory,
            registry,
            session,
            gateway,
            time: _,
        } = self;
        drop(gateway);
        drop(registry);
        #[cfg(unix)]
        let parent = directory.path().canonicalize().unwrap();
        #[cfg(not(unix))]
        let parent = directory.path().to_path_buf();
        let registry = Arc::new(
            LocalCredentialStore::open(parent.join("registry"), "credentials.json").unwrap(),
        );
        let keys = Arc::new(
            FilePairingState::open(&parent.join("gateway-private"), Path::new("state")).unwrap(),
        );
        let identity =
            restore_gateway_identity(registry.clone(), keys.clone(), Arc::new(Time), OsEntropy)
                .await
                .unwrap();
        let resource = Resource::new(
            OrganizationId::new("org").unwrap(),
            ResourceId::new("gateway").unwrap(),
        );
        let time = Arc::new(GatewayTime(AtomicU64::new(NOW_MS)));
        let gateway = Arc::new(
            GatewayPairing::open(PairingRuntimeDependencies {
                registry: registry.clone(),
                policy: Arc::new(CedarPolicyEvaluator::new().unwrap()),
                clock: time.clone(),
                gateway: resource,
                key_store: keys,
                identity,
            })
            .unwrap(),
        );
        Self {
            directory,
            registry,
            session,
            gateway,
            time,
        }
    }
    pub async fn listener(
        &self,
    ) -> (
        SocketAddr,
        oneshot::Sender<()>,
        JoinHandle<IoResult<()>>,
        Arc<NativeEnrollmentConnections>,
    ) {
        let socket = TokioTcpListener::bind("127.0.0.1:0").await.unwrap();
        let connections = Arc::new(NativeEnrollmentConnections::new(
            self.gateway.clone(),
            RuntimeDependencies::default().clock,
        ));
        let listener = NativeEnrollmentListener::new(socket, connections.clone());
        let address = listener.local_address().unwrap();
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(listener.run(
            OsEntropy::default,
            async {
                stopped.await.unwrap();
            },
            |_| panic!("listener failed"),
        ));
        (address, stop, task, connections)
    }
}

pub fn sockets() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (server, _) = listener.accept().unwrap();
    for socket in [&server, &client] {
        socket.set_read_timeout(Some(WAIT)).unwrap();
        socket.set_write_timeout(Some(WAIT)).unwrap();
    }
    (server, client)
}
