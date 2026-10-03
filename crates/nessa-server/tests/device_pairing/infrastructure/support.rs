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
        pairing::{
            AttemptReservation, ConfirmedClaim, DeviceConnectionProof, GatewayKeyStore,
            NoReceiverCleanupProof, OwnerDecision, PairingAdmission, PairingStore,
            PairingStoreError, PrivateKeyMaterial, ReceiverOutcome, RuntimeEnd, StageOwnership,
        },
        ports::{Clock, PortFuture},
        session::{AuthenticateSession, AuthenticatedSession},
    },
    domain::{
        pairing::{AttemptFailure, AttemptId, InvitationId, PairingRecord},
        AudienceId, CredentialId, OrganizationId, Resource, ResourceId,
    },
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
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{sync_channel, Receiver, SyncSender},
        Arc, Mutex,
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
/// The owner's credential outlives every invitation a test creates.
const OWNER_EXPIRES_S: u64 = 100_000;
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
        Self::build(read, OWNER_EXPIRES_S, |registry| registry).await
    }
    /// The owner's credential expires at 500 s, before an invitation created now.
    pub async fn short_lived_owner() -> Self {
        Self::build(true, 500, |registry| registry).await
    }
    /// A fixture whose runtime reaches the registry through `enrollments`.
    pub async fn with_store(
        enrollments: impl FnOnce(Arc<LocalCredentialStore>) -> Arc<dyn PairingStore>,
    ) -> Self {
        Self::build(true, OWNER_EXPIRES_S, enrollments).await
    }
    async fn build(
        read: bool,
        owner_expires_s: u64,
        enrollments: impl FnOnce(Arc<LocalCredentialStore>) -> Arc<dyn PairingStore>,
    ) -> Self {
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
                expires_at: Some(owner_expires_s),
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
                enrollments: enrollments(registry.clone()),
                access: registry.clone(),
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
            time,
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

        let gateway = Arc::new(
            GatewayPairing::open(PairingRuntimeDependencies {
                enrollments: registry.clone(),
                access: registry.clone(),
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

/// Store calls a `FaultyStore` can refuse, each with `Unavailable`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Reserve,
    Settle,
    Claim,
    /// Every read after a claim commits.
    ReadsAfterClaim,
    /// Every read.
    Reads,
}
/// The real registry behind the `PairingStore` port, refusing one chosen call.
pub struct FaultyStore {
    inner: Arc<LocalCredentialStore>,
    fault: Mutex<Option<Fault>>,
    claimed: AtomicBool,
    gate: Mutex<Option<Receiver<()>>>,
    gate_timed_out: AtomicBool,
}
impl FaultyStore {
    pub fn new(inner: Arc<LocalCredentialStore>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            fault: Mutex::new(None),
            claimed: AtomicBool::new(false),
            gate: Mutex::new(None),
            gate_timed_out: AtomicBool::new(false),
        })
    }
    /// The next store read waits until the returned sender fires, for at most
    /// two seconds. On a current-thread runtime, a read made on the async
    /// thread cannot be released by a task on that thread, so it times out.
    pub fn gate_next_read(&self) -> SyncSender<()> {
        let (release, gate) = sync_channel(1);
        *self.gate.lock().unwrap() = Some(gate);
        release
    }
    /// Whether a gated read timed out.
    pub fn gate_timed_out(&self) -> bool {
        self.gate_timed_out.load(Ordering::SeqCst)
    }
    pub fn refuse(&self, fault: Option<Fault>) {
        *self.fault.lock().unwrap() = fault;
    }
    fn refuses(&self, fault: Fault) -> Result<(), PairingStoreError> {
        if *self.fault.lock().unwrap() == Some(fault) {
            Err(PairingStoreError::Unavailable)
        } else {
            Ok(())
        }
    }
    fn read(&self) -> Result<(), PairingStoreError> {
        let gate = self.gate.lock().unwrap().take();
        if let Some(gate) = gate {
            if gate.recv_timeout(Duration::from_secs(2)).is_err() {
                self.gate_timed_out.store(true, Ordering::SeqCst);
            }
        }
        self.refuses(Fault::Reads)?;
        if self.claimed.load(Ordering::SeqCst) {
            self.refuses(Fault::ReadsAfterClaim)
        } else {
            Ok(())
        }
    }
}
impl PairingStore for FaultyStore {
    fn publish_first_gateway_key(
        &self,
        keys: &dyn GatewayKeyStore,
        key: &PrivateKeyMaterial,
        clock: &dyn Clock,
    ) -> Result<(), PairingStoreError> {
        self.inner.publish_first_gateway_key(keys, key, clock)
    }
    fn acquire_stage(
        self: Arc<Self>,
        id: InvitationId,
    ) -> Result<StageOwnership, PairingStoreError> {
        self.inner.clone().acquire_stage(id)
    }
    fn finish_no_receiver(
        &self,
        proof: &NoReceiverCleanupProof,
        clock: &dyn Clock,
    ) -> Result<PairingRecord, PairingStoreError> {
        self.inner.finish_no_receiver(proof, clock)
    }
    fn pending_pairings(&self) -> Result<Box<[PairingRecord]>, PairingStoreError> {
        self.read()?;
        self.inner.pending_pairings()
    }
    fn expire_pairing_if_due(
        &self,
        id: InvitationId,
        clock: &dyn Clock,
    ) -> Result<PairingRecord, PairingStoreError> {
        self.read()?;
        self.inner.expire_pairing_if_due(id, clock)
    }
    fn end_pairing(
        &self,
        id: InvitationId,
        cause: RuntimeEnd,
        clock: &dyn Clock,
    ) -> Result<PairingRecord, PairingStoreError> {
        self.inner.end_pairing(id, cause, clock)
    }
    fn read_pairing(&self, id: InvitationId) -> Result<PairingRecord, PairingStoreError> {
        self.read()?;
        self.inner.read_pairing(id)
    }
    fn create_pairing<'a>(
        &'a self,
        record: &'a PairingRecord,
        admission: &'a PairingAdmission,
        clock: &'a dyn Clock,
    ) -> PortFuture<'a, PairingRecord, PairingStoreError> {
        self.inner.create_pairing(record, admission, clock)
    }
    fn decide_pairing<'a>(
        &'a self,
        id: InvitationId,
        decision: OwnerDecision,
        admission: &'a PairingAdmission,
        clock: &'a dyn Clock,
    ) -> PortFuture<'a, PairingRecord, PairingStoreError> {
        self.inner.decide_pairing(id, decision, admission, clock)
    }
    fn reserve_attempt(
        &self,
        id: InvitationId,
        attempt: AttemptId,
        device: &DeviceConnectionProof,
        input: [u8; 32],
        clock: &dyn Clock,
    ) -> Result<AttemptReservation, PairingStoreError> {
        self.refuses(Fault::Reserve)?;
        self.inner
            .reserve_attempt(id, attempt, device, input, clock)
    }
    fn confirm_claim(
        &self,
        proof: &ConfirmedClaim,
        clock: &dyn Clock,
    ) -> Result<PairingRecord, PairingStoreError> {
        self.refuses(Fault::Claim)?;
        let record = self.inner.confirm_claim(proof, clock)?;
        self.claimed.store(true, Ordering::SeqCst);
        Ok(record)
    }
    fn fail_attempt(
        &self,
        id: InvitationId,
        attempt: AttemptId,
        device: &DeviceConnectionProof,
        cause: AttemptFailure,
        clock: &dyn Clock,
    ) -> Result<PairingRecord, PairingStoreError> {
        self.refuses(Fault::Settle)?;
        self.inner.fail_attempt(id, attempt, device, cause, clock)
    }
    fn stage_pairing(
        &self,
        id: InvitationId,
        credential: CredentialId,
        request: AttemptId,
        admission: &PairingAdmission,
        clock: &dyn Clock,
    ) -> Result<PairingRecord, PairingStoreError> {
        self.inner
            .stage_pairing(id, credential, request, admission, clock)
    }
    fn remember_receiver(
        &self,
        id: InvitationId,
        outcome: &ReceiverOutcome,
        clock: &dyn Clock,
    ) -> Result<PairingRecord, PairingStoreError> {
        self.inner.remember_receiver(id, outcome, clock)
    }
    fn publish_pairing(
        &self,
        id: InvitationId,
        admission: &PairingAdmission,
        clock: &dyn Clock,
    ) -> Result<PairingRecord, PairingStoreError> {
        self.inner.publish_pairing(id, admission, clock)
    }
    fn finish_cleanup(
        &self,
        id: InvitationId,
        outcome: &ReceiverOutcome,
        clock: &dyn Clock,
    ) -> Result<PairingRecord, PairingStoreError> {
        self.inner.finish_cleanup(id, outcome, clock)
    }
}
