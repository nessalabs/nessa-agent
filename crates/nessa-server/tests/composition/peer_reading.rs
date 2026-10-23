//! Two composed gateways in one process: B enrolls into A as a peer, through
//! a relay the test controls, A's owner approves, and B's poller reads what A
//! grants it into B's retained cache, follows A's shares, unshares and
//! deletions, empties a cache that cannot continue, keeps to its read budget,
//! gives way to the owner's commands, and stops at A's revocation or denial.
//! Each change the poller makes is audited; with an audit that refuses those
//! records, every change still lands. With the durable audit, every record
//! lands in the order the turn was held, a stalled store holds an owner
//! command no longer than its own audit deadline, and a full queue refuses
//! at once. Rows R1–R14, R18 and R21 in
//! `docs/design/auth/peer-gateways.md` ("Reading a peer").
use crate::app::dependencies::RuntimeDependencies;
use crate::composition::local_auth::SystemClock;
use crate::composition::native_pairing::{bind, prepare, start, NativeInputs, RunningNative};
use crate::composition::runtime_config::{NativeConfig, RelayAddress};
use crate::conversation::application::{
    ConversationCaller, ConversationRepository, ShareGrantChange, ShareGrantTransition, ShareGrants,
};
use crate::conversation::domain::{Conversation, ConversationDeletion, ShareAccess};
use crate::conversation::infrastructure::{
    LocalConversationStore, LocalReceiverAuthority, NessaCatalogueReadSource,
    NessaRecordReadSource, TestReadGate,
};
use crate::device_pairing::infrastructure::PairingOwnerCommands;
use crate::peer_gateways::application::{
    CacheState, PeerAudit, PeerAuditFuture, PeerAuditRecord, PeerAuditUnavailable, PeerHolding,
    PeerState, PollerCause,
};
use crate::peer_gateways::infrastructure::{
    DurablePeerAudit, EnrollmentEntropy, EnrollmentEntropySource, PeerCommands, PeerEntry,
    PeerError, PeerPhase, PeerPoller, PeerSync, PollInputs, PollPolicy, SyncState,
    TcpPeerConnector, AUDIT_DEADLINE, READS_AT_ONCE,
};
use crate::product::{ProductDependencies, ProductRouteState};
use nessa_auth::adapters::cedar::CedarPolicyEvaluator;
use nessa_auth::adapters::local::{BootstrapRequest, LocalCredentialStore};
use nessa_auth::adapters::pairing::{ManualCode, OsEntropy};
use nessa_auth::application::credential_admin::RevokeCredentialRequest;
use nessa_auth::application::dto::{
    CredentialGrantDto, MembershipInputDto, MembershipRoleDto, MembershipStateDto,
    OrganizationInputDto, PrincipalInputDto, PrincipalKindDto, ResourceDto,
};
use nessa_auth::application::pairing::OwnerDecision;
use nessa_auth::application::ports::Clock;
use nessa_auth::application::session::{AuthenticateSession, AuthenticatedSession};
use nessa_auth::domain::pairing::{ConsentClass, DeviceKey};
use nessa_auth::domain::{
    AudienceId, CredentialId, OrganizationId, PrincipalId, Resource, ResourceId,
};
use nessa_client_core::retained::{RetainedCache, HANDSHAKE};
use nessa_protocol::agents::AgentId;
use nessa_protocol::conversation::domain::{
    ConversationApprovalMode, ConversationId, ConversationModelId,
};
use nessa_sdk::application::agent_execution::sessions::SessionStorage;
use nessa_sdk::infrastructure::session_storage::RecordStorage;
use nessa_sync::replication::domain::Id;
use serde_json::Value;
use std::future::pending;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::Handle;
use uuid::Uuid;

/// The longest any one wait below may take.
pub(super) const WAIT: Duration = Duration::from_secs(30);
/// B reads A this often, so each step settles in well under a second.
const POLICY: PollPolicy = PollPolicy {
    interval: Duration::from_millis(100),
    backoff_cap: Duration::from_millis(400),
    read_budget: Duration::from_secs(60),
    reads_at_once: READS_AT_ONCE,
};
/// Well under the handshake deadline a held read would otherwise reach (5 s),
/// so the silent-handshake cases cannot spend their whole deadline inside it.
/// The manual-clock audit cases also check the captured cycle's stop flag.
const PROMPT: Duration = Duration::from_secs(2);

pub(super) fn uuid() -> String {
    Uuid::new_v4().to_string()
}

pub(super) struct NoAgents;
impl crate::agents::application::AgentProbe for NoAgents {
    fn evidence(&self, _: AgentId) -> Option<crate::agents::application::AgentProbeEvidence> {
        None
    }
}

/// A registry with one owner who may manage credentials and read and write
/// conversations on `gateway`, and that owner's authenticated session.
pub(super) async fn owned(
    root: &Path,
    gateway: &str,
    organization: &str,
) -> (Arc<LocalCredentialStore>, AuthenticatedSession, String) {
    nessa_local_storage::create_directory(root).unwrap();
    let auth = Arc::new(
        LocalCredentialStore::open(
            root,
            "credentials.json",
            std::sync::Arc::new(nessa_auth::adapters::cedar::CedarPolicyEvaluator::new().unwrap()),
        )
        .unwrap(),
    );
    let owner = uuid();
    let grant = |action: &str| CredentialGrantDto {
        action: action.into(),
        resource: ResourceDto {
            organization_id: organization.into(),
            id: gateway.into(),
        },
    };
    let boot = auth
        .bootstrap(BootstrapRequest {
            gateway_id: gateway.into(),
            organization: OrganizationInputDto {
                id: organization.into(),
            },
            principal: PrincipalInputDto {
                id: owner.clone(),
                kind: PrincipalKindDto::Human,
            },
            membership: MembershipInputDto {
                id: uuid(),
                principal_id: owner.clone(),
                organization_id: organization.into(),
                role: MembershipRoleDto::Admin,
                state: MembershipStateDto::Active,
            },
            credential_id: uuid(),
            issued_at: SystemClock.unix_seconds(),
            expires_at: None,
            grants: [
                "conversation.read",
                "conversation.write",
                "credential.manage",
            ]
            .into_iter()
            .map(grant)
            .collect(),
        })
        .unwrap();
    let session = AuthenticateSession {
        policy: &nessa_auth::adapters::cedar::CedarPolicyEvaluator::new().unwrap(),
        verifier: auth.as_ref(),
        access: auth.as_ref(),
        clock: &SystemClock,
    }
    .execute(&boot.evidence, &AudienceId::new(gateway).unwrap())
    .await
    .unwrap();
    (auth, session, owner)
}

pub(super) fn inputs(
    root: &Path,
    auth: &Arc<LocalCredentialStore>,
    receivers: Arc<LocalReceiverAuthority>,
    gateway: &str,
    organization: &str,
) -> NativeInputs {
    NativeInputs {
        namespace: root.to_path_buf(),
        registry: auth.clone(),
        policy: Arc::new(CedarPolicyEvaluator::new().unwrap()),
        receivers,
        clock: Arc::new(SystemClock),
        deadline_clock: RuntimeDependencies::default().clock,
        gateway: Resource::new(
            OrganizationId::new(organization).unwrap(),
            ResourceId::new(gateway).unwrap(),
        ),
        audience: AudienceId::new(gateway).unwrap(),
    }
}

/// Gateway A, composed as `local_auth` composes it: native pairing, passive
/// reads over its conversation metadata and record storage.
struct Peer {
    auth: Arc<LocalCredentialStore>,
    session: AuthenticatedSession,
    owner: String,
    organization: String,
    commands: PairingOwnerCommands,
    metadata: Arc<LocalConversationStore>,
    storage: Arc<RecordStorage>,
    native: SocketAddr,
    running: Option<RunningNative>,
    record_hold: Option<Arc<TestReadGate>>,
    record_source: Arc<NessaRecordReadSource>,
}
impl Peer {
    async fn start(root: &Path) -> Self {
        Self::start_with(root, None).await
    }
    /// A, serving through `relay` too when one is given.
    async fn start_with(root: &Path, relay: Option<RelayAddress>) -> Self {
        Self::prepare(root, relay, false).await
    }

    async fn with_held_record(root: &Path) -> Self {
        Self::prepare(root, None, true).await
    }

    async fn prepare(root: &Path, relay: Option<RelayAddress>, hold_record: bool) -> Self {
        let (gateway, organization) = (uuid(), uuid());
        let (auth, session, owner) = owned(root, &gateway, &organization).await;
        let receivers = Arc::new(
            LocalReceiverAuthority::open(
                &root.join("receivers.sqlite3"),
                &CedarPolicyEvaluator::profile_digest(),
                Arc::new(SystemClock),
            )
            .unwrap(),
        );
        let (prepared, commands, _peers) = prepare(
            &NativeConfig {
                listen_address: "127.0.0.1:0".parse().unwrap(),
                relay,
                advertised_address: None,
            },
            inputs(root, &auth, receivers.clone(), &gateway, &organization),
        )
        .await
        .unwrap();
        let metadata = Arc::new(
            LocalConversationStore::open(&root.join("metadata.sqlite3"))
                .unwrap()
                .with_access_policy(crate::conversation_test_support::access_policy()),
        );
        let storage = Arc::new(RecordStorage::new(root.join("source")).unwrap());
        storage.initialize().await.unwrap();
        let (record_source, record_hold) = if hold_record {
            let (source, gate) = NessaRecordReadSource::held_for_host_shutdown(
                storage.clone(),
                Id::new(&gateway).unwrap(),
                Handle::current(),
                false,
            );
            (source, Some(gate))
        } else {
            (
                NessaRecordReadSource::new(
                    storage.clone(),
                    Id::new(&gateway).unwrap(),
                    Handle::current(),
                ),
                None,
            )
        };
        let record_source = Arc::new(record_source);
        let state = ProductRouteState::new(
            ResourceId::new(gateway.clone()).unwrap(),
            OrganizationId::new(organization.clone()).unwrap(),
            AudienceId::new(gateway.clone()).unwrap(),
            ProductDependencies {
                owner_phone_purpose: auth.clone(),
                owner_phone_policy: Arc::new(
                    crate::conversation::infrastructure::CedarOwnerPhonePolicyEvaluator::new()
                        .unwrap(),
                ),
                owner_phone_receivers: None,
                verifier: auth.clone(),
                access: auth.clone(),
                clock: Arc::new(SystemClock),
                policy: Arc::new(CedarPolicyEvaluator::new().unwrap()),
                uptime_clock: RuntimeDependencies::default().clock,
                agent_probe: Arc::new(NoAgents),
            },
        )
        .with_passive_read(
            receivers,
            metadata.clone(),
            metadata.clone(),
            crate::conversation_test_support::access_policy(),
        )
        .with_record_source(record_source.clone())
        .with_catalogue_source(Arc::new(NessaCatalogueReadSource::new(
            metadata.clone(),
            crate::conversation_test_support::access_policy(),
            Id::new(&gateway).unwrap(),
        )));
        let bound = bind(prepared, RuntimeDependencies::default().clock, state)
            .await
            .unwrap();
        let native = bound.local_address();
        let (failure, _) = tokio::sync::watch::channel(None);
        let running = start(bound, failure);
        Self {
            auth,
            session,
            owner,
            organization,
            commands,
            metadata,
            storage,
            native,
            running: Some(running),
            record_hold,
            record_source,
        }
    }
    /// A conversation of the owner's, with an opened, empty transcript.
    async fn conversation(&self) -> ConversationId {
        let id = ConversationId::new(&uuid()).unwrap();
        self.metadata
            .create(
                Conversation::new(
                    id.clone(),
                    OrganizationId::new(self.organization.clone()).unwrap(),
                    PrincipalId::new(self.owner.clone()).unwrap(),
                    "peer".into(),
                    uuid(),
                    SystemClock.unix_milliseconds(),
                    AgentId::Claude,
                    ConversationModelId::new("model").unwrap(),
                    ConversationApprovalMode::Ask,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let session =
            nessa_sdk::domain::agent_execution::sessions::SessionId::new(id.to_string()).unwrap();
        drop(self.storage.open(session).await.unwrap());
        id
    }
    /// The owner shares, or unshares, `id` with the paired reader.
    async fn grant(
        &self,
        transition: ShareGrantTransition,
        id: &ConversationId,
        reader: &(String, String),
    ) {
        let receipt = ShareGrants::change(
            self.metadata.as_ref(),
            ShareGrantChange {
                transition,
                conversation_id: id.clone(),
                principal_id: Some(PrincipalId::new(format!("grantee-{}", reader.0)).unwrap()),
                receiver_id: Some(reader.0.clone()),
                credential_id: CredentialId::new(reader.1.clone()).unwrap(),
                expected_policy_revision: None,
                initiator: ConversationCaller {
                    organization_id: OrganizationId::new(self.organization.clone()).unwrap(),
                    principal_id: PrincipalId::new(self.owner.clone()).unwrap(),
                    surface_id: "peer".into(),
                    action_id: uuid(),
                },
                at_ms: SystemClock.unix_milliseconds(),
            },
        )
        .await
        .unwrap();
        assert!(receipt.applied);
    }
}

/// A TCP relay in front of A. It lets every new connection through, or a
/// number of them and then holds each later one open and silent: the status
/// of a cycle goes through, and its read is held.
struct Relay {
    address: SocketAddr,
    state: Arc<RelayState>,
    task: tokio::task::JoinHandle<()>,
}
#[derive(Default)]
struct RelayState {
    /// New connections still let through; `None` lets every one through.
    through: Mutex<Option<usize>>,
    accepted: AtomicUsize,
    held: Mutex<Vec<TcpStream>>,
    links: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}
impl Relay {
    async fn start(target: SocketAddr) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let state = Arc::new(RelayState::default());
        let task = tokio::spawn({
            let state = state.clone();
            async move {
                while let Ok((mut inbound, _)) = listener.accept().await {
                    state.accepted.fetch_add(1, Ordering::SeqCst);
                    let pass = match &mut *state.through.lock().unwrap() {
                        None => true,
                        Some(0) => false,
                        Some(left) => {
                            *left -= 1;
                            true
                        }
                    };
                    if !pass {
                        state.held.lock().unwrap().push(inbound);
                        continue;
                    }
                    let link = tokio::spawn(async move {
                        if let Ok(mut outbound) = TcpStream::connect(target).await {
                            let _ =
                                tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
                        }
                    });
                    state.links.lock().unwrap().push(link);
                }
            }
        });
        Self {
            address,
            state,
            task,
        }
    }
    /// From now, let `count` new connections through and hold the rest.
    fn hold_after(&self, count: usize) {
        *self.state.through.lock().unwrap() = Some(count);
    }
    /// Let every new connection through, and close the held ones.
    fn pass_all(&self) {
        *self.state.through.lock().unwrap() = None;
        self.state.held.lock().unwrap().clear();
    }
    fn accepted(&self) -> usize {
        self.state.accepted.load(Ordering::SeqCst)
    }
    fn held(&self) -> usize {
        self.state.held.lock().unwrap().len()
    }
    /// Wait until more than `count` connections are held.
    async fn until_held(&self, count: usize) {
        tokio::time::timeout(WAIT, async {
            while self.held() <= count {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("a read was never held");
    }
}
impl Drop for Relay {
    fn drop(&mut self) {
        self.task.abort();
        for link in self.state.links.lock().unwrap().drain(..) {
            link.abort();
        }
        self.state.held.lock().unwrap().clear();
    }
}

/// B's peer audit: owner command records are taken as kept; each poller
/// change is kept, or refused when `refuse`.
struct Changes {
    refuse: bool,
    kept: Mutex<Vec<PeerAuditRecord>>,
    refused: AtomicUsize,
}
impl Changes {
    fn new(refuse: bool) -> Arc<Self> {
        Arc::new(Self {
            refuse,
            kept: Mutex::new(Vec::new()),
            refused: AtomicUsize::new(0),
        })
    }
}
impl PeerAudit for Changes {
    fn record(&self, record: PeerAuditRecord) -> PeerAuditFuture<'static> {
        let poller = matches!(record, PeerAuditRecord::PollerChanged { .. });
        let refused = poller && self.refuse;
        if refused {
            self.refused.fetch_add(1, Ordering::SeqCst);
        } else {
            self.kept.lock().unwrap().push(record);
        }
        Box::pin(async move {
            if refused {
                Err(PeerAuditUnavailable)
            } else {
                Ok(())
            }
        })
    }
}

/// A monotonic clock that moves only when the test moves it.
struct Manual(AtomicU64);
impl nessa_protocol::clock::Clock for Manual {
    fn elapsed_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// Holds B's durable audit writer before it writes record `at`, until
/// opened: a store that stalls.
struct Stall {
    at: u64,
    reached: AtomicBool,
    open: Mutex<bool>,
    opened: Condvar,
}
impl Stall {
    fn new(at: u64) -> Arc<Self> {
        Arc::new(Self {
            at,
            reached: AtomicBool::new(false),
            open: Mutex::new(false),
            opened: Condvar::new(),
        })
    }
    fn hook(self: &Arc<Self>) -> Arc<dyn Fn(u64) + Send + Sync> {
        let stall = self.clone();
        Arc::new(move |sequence| {
            if sequence != stall.at {
                return;
            }
            stall.reached.store(true, Ordering::SeqCst);
            let mut open = stall.open.lock().unwrap();
            while !*open {
                open = stall.opened.wait(open).unwrap();
            }
        })
    }
    async fn until_reached(&self) {
        tokio::time::timeout(WAIT, async {
            while !self.reached.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the writer never reached the stalled record");
    }
    fn release(&self) {
        *self.open.lock().unwrap() = true;
        self.opened.notify_all();
    }
}

/// A synthetic stalled write must be released on assertion failure too.
struct ReleaseStall(Arc<Stall>);
impl Drop for ReleaseStall {
    fn drop(&mut self) {
        self.0.release();
    }
}

/// A failed test still releases the grantor's physical record-read worker.
struct ReleaseRecord(Arc<TestReadGate>);
impl Drop for ReleaseRecord {
    fn drop(&mut self) {
        self.0.release();
    }
}

#[tokio::test]
async fn a_cancelled_late_write_controller_releases_the_writer() {
    let stall = Stall::new(4);
    let release = ReleaseStall(stall.clone());
    let hook = stall.hook();
    let mut writer = tokio::task::spawn_blocking(move || hook(4));
    let controller = tokio::spawn(async move {
        let _release = release;
        pending::<()>().await;
    });
    stall.until_reached().await;
    controller.abort();
    assert!(controller.await.unwrap_err().is_cancelled());
    let completed = tokio::time::timeout(PROMPT, &mut writer).await;
    // Release even a deliberately broken guard before reporting a failure.
    stall.release();
    match completed {
        Ok(joined) => joined.unwrap(),
        Err(error) => {
            writer.await.unwrap();
            panic!("cancelled controller left its writer held: {error}");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cancelled_record_hold_controller_releases_the_worker() {
    let directory = tempfile::tempdir().unwrap();
    let a = Peer::with_held_record(&directory.path().join("a")).await;
    let gate = a.record_hold.as_ref().unwrap().clone();
    // Retain fallback cleanup outside the controller tested below.
    let _fallback = ReleaseRecord(gate.clone());
    let release = ReleaseRecord(gate.clone());
    let controller = tokio::spawn(async move {
        let _release = release;
        pending::<()>().await;
    });
    let b = Reader::prepare(&directory.path().join("b"), false).await;
    let relay = Relay::start(a.native).await;
    let (_, reader) = enroll(&a, &b, &relay, true).await;
    let conversation = a.conversation().await;
    a.grant(
        ShareGrantTransition::Grant(ShareAccess::read()),
        &conversation,
        &reader.unwrap(),
    )
    .await;
    let poller = b.poller(POLICY);
    tokio::time::timeout(WAIT, gate.entered()).await.unwrap();
    controller.abort();
    assert!(controller.await.unwrap_err().is_cancelled());
    let source = a.record_source.clone();
    let mut joined = Box::pin(source.shutdown());
    let completed = tokio::time::timeout(PROMPT, &mut joined).await;
    // Even the missing-guard control must release and join physical work.
    gate.release();
    let timely = match completed {
        Ok(result) => {
            result.unwrap();
            true
        }
        Err(_) => {
            tokio::time::timeout(WAIT, joined).await.unwrap().unwrap();
            false
        }
    };
    tokio::time::timeout(WAIT, poller.join()).await.unwrap();
    drop(relay);
    stop(a).await;
    assert!(timely, "cancelled controller left its record worker held");
}

/// A private directory for B's durable audit beneath `root`.
fn audit_directory(root: &Path) -> PathBuf {
    nessa_local_storage::create_directory(root).unwrap();
    #[cfg(unix)]
    let root = root.canonicalize().unwrap();
    let directory = root.join("b-audit");
    nessa_local_storage::create_directory(&directory).unwrap();
    directory
}

/// The durable audit's records, in sequence order.
fn audit_files(directory: &Path) -> Vec<Value> {
    let mut records: Vec<Value> = std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| {
            serde_json::from_slice(&std::fs::read(entry.unwrap().path()).unwrap()).unwrap()
        })
        .collect();
    records.sort_by_key(|record| record["sequence"].as_u64().unwrap());
    records
}

/// The sequences of the records written, in order. A gap is a record the
/// audit refused, which its log names.
fn audit_sequences(directory: &Path) -> Vec<u64> {
    audit_files(directory)
        .iter()
        .map(|record| record["sequence"].as_u64().unwrap())
        .collect()
}

/// Each record's kind, with the poller's cause, in sequence order; their
/// observation times in that order too.
fn audit_kinds(directory: &Path) -> Vec<String> {
    let records = audit_files(directory);
    let observed: Vec<u64> = records
        .iter()
        .map(|record| record["observedAtMs"].as_u64().unwrap())
        .collect();
    assert!(
        observed.windows(2).all(|pair| pair[0] <= pair[1]),
        "observed in sequence order: {observed:?}"
    );
    records
        .iter()
        .map(|record| match record["cause"].as_str() {
            Some(cause) if record["kind"] == "peer_poller_changed" => {
                format!("peer_poller_changed {cause}")
            }
            _ => record["kind"].as_str().unwrap().to_owned(),
        })
        .collect()
}

/// One kept poller change, as the table below names it: its cause, and the
/// record and cache before and after.
fn change(record: &PeerAuditRecord) -> Option<String> {
    let PeerAuditRecord::PollerChanged {
        cause,
        before,
        after,
        outcome,
        ..
    } = record
    else {
        return None;
    };
    let held = |holding: &PeerHolding| {
        let phase = match &holding.record {
            PeerState::Absent => "absent",
            PeerState::Pending { .. } => "pending",
            PeerState::Active { .. } => "active",
            PeerState::Revoked { .. } => "revoked",
            other => panic!("unexpected {other:?}"),
        };
        let cache = match holding.cache {
            CacheState::Present => "cache",
            CacheState::Absent => "none",
            CacheState::Unknown => "unknown",
        };
        format!("{phase}+{cache}")
    };
    let cause = match cause {
        PollerCause::Approved => "approved".to_owned(),
        PollerCause::Ended { detail } => format!("ended({})", detail.as_deref().unwrap_or("?")),
        PollerCause::ResetRequired => "reset_required".to_owned(),
        PollerCause::CacheDamaged => "cache_damaged".to_owned(),
        PollerCause::Withdrawn { conversations } => format!("withdrew {}", conversations.join(",")),
    };
    assert_eq!(*outcome, Ok(()), "{cause}");
    Some(format!("{cause}: {} -> {}", held(before), held(after)))
}

/// Gateway B: native pairing prepared, for its key and its peer records;
/// its peer commands audited by `Changes`; its poller started and stopped by
/// each step.
struct Reader {
    peers: Arc<PeerCommands>,
    changes: Arc<Changes>,
    owner: PrincipalId,
    directory: PathBuf,
}
impl Reader {
    async fn prepare(root: &Path, refuse: bool) -> Self {
        let changes = Changes::new(refuse);
        Self::prepare_with(
            root,
            changes.clone(),
            changes,
            RuntimeDependencies::default().clock,
        )
        .await
    }
    /// B with `audit` as its peer audit (`changes` when that is the one the
    /// test reads) and `clock` for its peer commands: every deadline, wait
    /// and read budget of theirs and its poller's.
    async fn prepare_with(
        root: &Path,
        changes: Arc<Changes>,
        audit: Arc<dyn PeerAudit>,
        clock: Arc<dyn nessa_protocol::clock::Clock>,
    ) -> Self {
        let (gateway, organization) = (uuid(), uuid());
        let (auth, _, owner) = owned(root, &gateway, &organization).await;
        let receivers = Arc::new(
            LocalReceiverAuthority::open(
                &root.join("receivers.sqlite3"),
                &CedarPolicyEvaluator::profile_digest(),
                Arc::new(SystemClock),
            )
            .unwrap(),
        );
        let (_, _, prepared) = prepare(
            &NativeConfig {
                listen_address: "127.0.0.1:0".parse().unwrap(),
                relay: None,
                advertised_address: None,
            },
            inputs(root, &auth, receivers, &gateway, &organization),
        )
        .await
        .unwrap();
        // Composition's peer commands, over the same records, with this
        // audit in place of the durable one.
        let peers = Arc::new(PeerCommands::new(
            prepared.records().clone(),
            clock,
            audit,
            Arc::new(TcpPeerConnector),
            entropy(),
        ));
        #[cfg(unix)]
        let root = root.canonicalize().unwrap();
        Self {
            peers,
            changes,
            owner: PrincipalId::new(owner).unwrap(),
            // Where composition keeps peer records and their caches.
            directory: root.join("peer-gateways"),
        }
    }
    fn poller(&self, policy: PollPolicy) -> PeerPoller {
        PeerPoller::start(
            self.peers.clone(),
            PollInputs {
                policy,
                wall: Arc::new(SystemClock),
                entropy: entropy(),
            },
        )
    }
    /// The one peer's listing.
    async fn peer(&self) -> (PeerEntry, Option<PeerSync>) {
        let mut listed = self.peers.list().await.unwrap();
        assert_eq!(listed.len(), 1);
        listed.pop().unwrap()
    }
    /// Run the poller until `done` holds of the peer's listing, then stop it
    /// and wait until nothing it started is running.
    async fn poll_until(&self, done: impl Fn(&PeerEntry, Option<&PeerSync>) -> bool) {
        self.poll_until_with(POLICY, done).await;
    }
    async fn poll_until_with(
        &self,
        policy: PollPolicy,
        done: impl Fn(&PeerEntry, Option<&PeerSync>) -> bool,
    ) {
        let poller = self.poller(policy);
        let last = Mutex::new(None);
        let reached = tokio::time::timeout(WAIT, async {
            loop {
                let (entry, sync) = self.peer().await;
                if done(&entry, sync.as_ref()) {
                    return;
                }
                *last.lock().unwrap() = Some((entry, sync));
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        tokio::time::timeout(WAIT, poller.join()).await.unwrap();
        assert!(
            reached.is_ok(),
            "the peer's listing never reached the expected state; last: {:?}",
            last.lock().unwrap()
        );
    }
    fn cache_path(&self, key: &DeviceKey) -> PathBuf {
        let hex: String = key
            .bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        self.directory.join(format!("{hex}.sqlite3"))
    }
    /// The conversations B's cache of `key` holds for `receiver`.
    fn cached(&self, key: &DeviceKey, receiver: &str) -> Vec<String> {
        let mut cache = RetainedCache::open(
            &self.cache_path(key),
            Arc::new(SystemClock),
            RuntimeDependencies::default().clock,
        )
        .unwrap();
        let mut ids = cache.conversation_ids(receiver).unwrap();
        ids.sort();
        ids
    }
    /// Change B's cache of `key` by hand, as damage or a restore would.
    fn tamper(&self, key: &DeviceKey, statement: &str) {
        nessa_local_database::rusqlite::Connection::open(self.cache_path(key))
            .unwrap()
            .execute_batch(statement)
            .unwrap();
    }
    /// B's cache of `key`, byte for byte, while no read has it open.
    fn snapshot(&self, key: &DeviceKey) -> Vec<u8> {
        std::fs::read(self.cache_path(key)).unwrap()
    }
    /// Put back a `snapshot`, in the same private file.
    fn restore(&self, key: &DeviceKey, bytes: &[u8]) {
        let mut file = nessa_local_storage::open(
            &self.cache_path(key),
            nessa_local_storage::OpenMode::ReadWrite,
        )
        .unwrap();
        file.set_len(0).unwrap();
        std::io::Write::write_all(&mut file, bytes).unwrap();
        file.sync_all().unwrap();
    }
    /// The catalogue head B's cache of `key` completed, as hex.
    fn completed(&self, key: &DeviceKey) -> String {
        nessa_local_database::rusqlite::Connection::open(self.cache_path(key))
            .unwrap()
            .query_row("SELECT hex(completed) FROM catalogue_progress", [], |row| {
                row.get(0)
            })
            .unwrap()
    }
    /// How many finished walks B's cache of `key` notes.
    fn walks(&self, key: &DeviceKey) -> i64 {
        nessa_local_database::rusqlite::Connection::open(self.cache_path(key))
            .unwrap()
            .query_row("SELECT count(*) FROM retained_walks", [], |row| row.get(0))
            .unwrap()
    }
    /// The poller changes kept so far, as `change` names them.
    fn kept(&self) -> Vec<String> {
        self.changes
            .kept
            .lock()
            .unwrap()
            .iter()
            .filter_map(change)
            .collect()
    }
}

fn entropy() -> EnrollmentEntropySource {
    Arc::new(|| Box::new(OsEntropy) as Box<dyn EnrollmentEntropy>)
}

/// A read that finished at or after `since`, complete.
fn synced_since(since: u64) -> impl Fn(&PeerEntry, Option<&PeerSync>) -> bool {
    move |_, sync| {
        sync.is_some_and(|sync| {
            sync.state == SyncState::Synced && sync.last_synced_ms.is_some_and(|at| at >= since)
        })
    }
}

/// Synced since now, holding `count` conversations.
async fn synced_with(b: &Reader, count: u64) {
    let since = SystemClock.unix_milliseconds();
    b.poll_until(move |entry, sync| {
        synced_since(since)(entry, sync)
            && sync.is_some_and(|sync| sync.conversations == Some(count))
    })
    .await;
}

/// The peer's listing says the enrollment ended: revoked, no sync.
fn revoked(entry: &PeerEntry, sync: Option<&PeerSync>) -> bool {
    sync.is_none()
        && matches!(entry, PeerEntry::Readable(record) if record.phase() == &PeerPhase::Revoked)
}

/// B enrolls through `relay` into a new invitation of A's. `approve` or
/// deny it; the reader's receiver and credential once approved.
async fn enroll(
    a: &Peer,
    b: &Reader,
    relay: &Relay,
    approve: bool,
) -> (DeviceKey, Option<(String, String)>) {
    let created = a
        .commands
        .create(&a.session, ConsentClass::PeerRead)
        .await
        .unwrap();
    let code = ManualCode::parse(created.code().expose_bytes()).unwrap();
    let pending = b
        .peers
        .enroll(relay.address, None, code, &b.owner)
        .await
        .unwrap();
    let key = *pending.key();
    let id = created.record().id();
    if !approve {
        a.commands
            .decide(&a.session, id, OwnerDecision::Deny)
            .await
            .unwrap();
        return (key, None);
    }
    let claimed = a.commands.status(&a.session, id).await.unwrap();
    let (_, claim) = claimed.claim_binding().unwrap();
    let approved = a
        .commands
        .approve(&a.session, id, claim)
        .await
        .unwrap()
        .record;
    let (receiver, _) = approved.receiver_binding().unwrap();
    (
        key,
        Some((
            receiver.as_str().to_owned(),
            approved.credential().unwrap().as_str().to_owned(),
        )),
    )
}

/// Rows R1–R14 with an audit that keeps every poller change: the effects,
/// and the table of what was kept.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_peer_reads_only_what_it_is_granted_and_stops_at_revocation() {
    reads(false).await;
}

/// The same rows with an audit that refuses every poller change: each change
/// still lands, and each refusal is the poller's own record, never an owner
/// command's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_poller_change_lands_when_its_audit_is_refused() {
    reads(true).await;
}

/// The two-gateway read, end to end.
async fn reads(refuse: bool) {
    let directory = tempfile::tempdir().unwrap();
    let mut a = Peer::start(&directory.path().join("a")).await;
    let b = Reader::prepare(&directory.path().join("b"), refuse).await;
    let relay = Relay::start(a.native).await;

    // R1: B enrolls into A's peer invitation and A's owner approves; B's
    // poller reads the Active status, saves the credential, and reads an
    // empty grant.
    let (key, reader) = enroll(&a, &b, &relay, true).await;
    let reader = reader.unwrap();
    let (x, y, z) = (
        a.conversation().await,
        a.conversation().await,
        a.conversation().await,
    );
    synced_with(&b, 0).await;
    let (entry, _) = b.peer().await;
    assert!(matches!(
        &entry,
        PeerEntry::Readable(record) if matches!(record.phase(), PeerPhase::Active { .. })
    ));
    assert!(b.cached(&key, &reader.0).is_empty());

    // R2: A shares X and not Y; B's cache gets X only.
    a.grant(
        ShareGrantTransition::Grant(ShareAccess::read()),
        &x,
        &reader,
    )
    .await;
    synced_with(&b, 1).await;
    assert_eq!(b.cached(&key, &reader.0), vec![x.to_string()]);

    // R3: A shares Y and unshares X; the next read takes both.
    a.grant(
        ShareGrantTransition::Grant(ShareAccess::read()),
        &y,
        &reader,
    )
    .await;
    a.grant(ShareGrantTransition::Revoke, &x, &reader).await;
    synced_with(&b, 1).await;
    assert_eq!(b.cached(&key, &reader.0), vec![y.to_string()]);

    // R4: A unshares Y and shares nothing else; B's cache empties. Shared
    // again, it comes back.
    a.grant(ShareGrantTransition::Revoke, &y, &reader).await;
    synced_with(&b, 0).await;
    assert!(b.cached(&key, &reader.0).is_empty());
    a.grant(
        ShareGrantTransition::Grant(ShareAccess::read()),
        &y,
        &reader,
    )
    .await;
    synced_with(&b, 1).await;

    // R5: A shares Z, then deletes it; B's cache drops it.
    a.grant(
        ShareGrantTransition::Grant(ShareAccess::read()),
        &z,
        &reader,
    )
    .await;
    synced_with(&b, 2).await;
    a.metadata
        .record_deletion(
            &z,
            ConversationDeletion::new(
                OrganizationId::new(a.organization.clone()).unwrap(),
                PrincipalId::new(a.owner.clone()).unwrap(),
                "peer".into(),
                uuid(),
                SystemClock.unix_milliseconds(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    synced_with(&b, 1).await;
    assert_eq!(b.cached(&key, &reader.0), vec![y.to_string()]);
    assert_eq!(b.walks(&key), 1, "a finished walk is noted");

    // R21: a walk cut short after its catalogue pass, then a restart (every
    // poller here is a new one). B's cache is put back to how such a walk
    // leaves it: A unshared Y, the pass reached A's new head, and the walk
    // stopped before it asked Y, so Y is still cached and no walk is noted.
    // At that unchanged head B walks again and withdraws Y; it is `synced`
    // only after that, holding nothing.
    let before = b.snapshot(&key);
    a.grant(ShareGrantTransition::Revoke, &y, &reader).await;
    synced_with(&b, 0).await;
    let head = b.completed(&key);
    b.restore(&key, &before);
    b.tamper(
        &key,
        &format!("UPDATE catalogue_progress SET completed = x'{head}'; DELETE FROM retained_walks"),
    );
    assert_eq!(b.cached(&key, &reader.0), vec![y.to_string()]);
    assert_eq!(b.completed(&key), head, "the pass is at A's head");
    synced_with(&b, 0).await;
    assert!(
        b.cached(&key, &reader.0).is_empty(),
        "withdrawn at the unchanged head"
    );
    assert_eq!(b.walks(&key), 1, "the finished walk is noted");
    a.grant(
        ShareGrantTransition::Grant(ShareAccess::read()),
        &y,
        &reader,
    )
    .await;
    synced_with(&b, 1).await;

    // R6: B's cache records more of A's catalogue than A serves, as after A
    // is restored from an older copy. The cache cannot continue
    // (ResetRequired), so B empties it and reads again on its own. A read
    // that did not reset would fail on every poll and never be `synced`.
    b.tamper(
        &key,
        "UPDATE catalogue_progress SET completed = x'7fffffffffffffff'",
    );
    synced_with(&b, 1).await;
    assert_eq!(b.cached(&key, &reader.0), vec![y.to_string()]);

    // R7: B's cache names another owner's catalogue for this receiver than
    // the one A answers. It holds one catalogue per receiver, so it does not
    // continue: ResetRequired again.
    b.tamper(
        &key,
        &format!(
            "PRAGMA foreign_keys = OFF;
             UPDATE catalogue_progress SET stream = 'conversation-owner:{}'",
            "0".repeat(64)
        ),
    );
    synced_with(&b, 1).await;
    assert_eq!(b.cached(&key, &reader.0), vec![y.to_string()]);

    // R8: B's cache has a shape this build does not read, as every cache
    // made before walks were noted has. Derived from A, it is emptied and
    // read again.
    b.tamper(&key, "PRAGMA user_version = 1");
    synced_with(&b, 1).await;
    assert_eq!(b.cached(&key, &reader.0), vec![y.to_string()]);

    // R9: a read that runs out of its budget having saved nothing made no
    // headway: it is a failure, backed off, and the cache keeps what it
    // held. The last finished read stays the last one: nothing refreshed it.
    let (_, synced) = b.peer().await;
    let last = synced.and_then(|sync| sync.last_synced_ms);
    assert!(last.is_some());
    relay.hold_after(1);
    b.poll_until_with(
        PollPolicy {
            read_budget: Duration::from_millis(300),
            ..POLICY
        },
        move |_, sync| {
            sync.is_some_and(|sync| {
                sync.state == SyncState::Unreachable
                    && sync.last_synced_ms == last
                    && sync.conversations == Some(1)
            })
        },
    )
    .await;
    relay.pass_all();
    assert_eq!(b.cached(&key, &reader.0), vec![y.to_string()]);

    // R10: stopping the poller while a read is held shuts that read at once.
    relay.hold_after(1);
    let held = relay.held();
    let poller = b.poller(POLICY);
    relay.until_held(held).await;
    tokio::time::timeout(PROMPT, poller.join())
        .await
        .expect("a stop shuts the read in progress");
    relay.pass_all();

    // R11: A's owner revokes B's credential. B's next read is refused, its
    // pinned status says the enrollment ended, and B removes the cache,
    // marks the record revoked, and reads A no more.
    a.auth
        .revoke_sync(RevokeCredentialRequest {
            request_id: uuid(),
            issuer_principal_id: a.owner.clone(),
            credential_id: reader.1.clone(),
            revoked_at: SystemClock.unix_seconds(),
        })
        .unwrap();
    b.poll_until(revoked).await;
    assert!(!b.cache_path(&key).exists(), "the cache is removed");
    // No connection reaches A over several intervals: not read again.
    let accepted = relay.accepted();
    let poller = b.poller(POLICY);
    tokio::time::sleep(POLICY.interval * 6).await;
    tokio::time::timeout(WAIT, poller.join()).await.unwrap();
    assert_eq!(relay.accepted(), accepted, "a revoked peer is not read");

    // R12: forgetting a peer removes its cache with its record. A cache left
    // by an earlier failed removal goes too.
    let cache = b.cache_path(&key);
    // Made private by the storage owner, as the cache's own file is, on
    // every system: a file anyone can read is refused.
    let mut left =
        nessa_local_storage::open(&cache, nessa_local_storage::OpenMode::CreateNew).unwrap();
    std::io::Write::write_all(&mut left, b"left behind").unwrap();
    drop(left);
    b.peers.forget(key, &b.owner).await.unwrap();
    assert!(b.peers.list().await.unwrap().is_empty());
    assert!(!cache.exists(), "forget removes the cache");

    // R13: B enrolls again and A's owner denies it: B's status says the
    // enrollment ended, and its record lists revoked.
    let (denied, _) = enroll(&a, &b, &relay, false).await;
    assert_eq!(denied, key);
    b.poll_until(revoked).await;
    b.peers.forget(key, &b.owner).await.unwrap();

    // R14: B enrolls once more and is approved. Forgetting A while a read of
    // it is held stops that read and goes ahead, rather than answering
    // `peer_busy`: the record and the cache are gone.
    let (_, reader) = enroll(&a, &b, &relay, true).await;
    let reader = reader.unwrap();
    a.grant(
        ShareGrantTransition::Grant(ShareAccess::read()),
        &y,
        &reader,
    )
    .await;
    synced_with(&b, 1).await;
    assert!(b.cache_path(&key).exists());
    relay.hold_after(1);
    let held = relay.held();
    let poller = b.poller(POLICY);
    relay.until_held(held).await;
    tokio::time::timeout(PROMPT, b.peers.forget(key, &b.owner))
        .await
        .expect("forget stops the read rather than wait for it")
        .unwrap();
    assert!(b.peers.list().await.unwrap().is_empty());
    assert!(!b.cache_path(&key).exists(), "forget removes the cache");
    tokio::time::timeout(WAIT, poller.join()).await.unwrap();
    relay.pass_all();

    // Every change the poller made, in order: kept, or each refused and
    // still made.
    let expected = [
        "approved: pending+none -> active+none".to_owned(),
        format!("withdrew {x}: active+cache -> active+cache"),
        format!("withdrew {y}: active+cache -> active+cache"),
        // R21: withdrawn, and withdrawn again after the cut walk.
        format!("withdrew {y}: active+cache -> active+cache"),
        format!("withdrew {y}: active+cache -> active+cache"),
        "reset_required: active+cache -> active+none".to_owned(),
        "reset_required: active+cache -> active+none".to_owned(),
        "cache_damaged: active+cache -> active+none".to_owned(),
        "ended(terminal: credential_revoked): active+cache -> revoked+none".to_owned(),
        "ended(terminal: denied): pending+none -> revoked+none".to_owned(),
        "approved: pending+none -> active+none".to_owned(),
    ];
    if refuse {
        assert!(b.kept().is_empty(), "{:?}", b.kept());
        assert_eq!(
            b.changes.refused.load(Ordering::SeqCst),
            expected.len(),
            "each change was offered once"
        );
    } else {
        assert_eq!(b.kept(), expected);
    }

    // Nothing left running: A's listener drains; B's pollers were joined.
    drop(relay);
    let mut running = a.running.take().unwrap();
    running.signal_stop();
    tokio::time::timeout(WAIT, running.join())
        .await
        .unwrap()
        .unwrap();
}

/// Stop A's listener and wait for it.
async fn stop(mut a: Peer) {
    let mut running = a.running.take().unwrap();
    running.signal_stop();
    tokio::time::timeout(WAIT, running.join())
        .await
        .unwrap()
        .unwrap();
}

/// Issue #266, slice R2b-2: once the address B enrolled at stops answering,
/// B's poller reaches A through the relay kept for it, A registered there:
/// its status dial and its read both go through the relay, and it reads
/// what A grants as it would directly.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_peer_whose_address_stops_answering_is_read_through_its_relay() {
    use crate::relay::{
        application::{Relay as RelayService, RelayLimits},
        infrastructure::{serve_tcp, Ed25519Proofs, OsRelayEntropy},
    };
    use crate::timer::{Timer, TokioTimer};
    let socket = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay_address = RelayAddress::try_from(socket.local_addr().unwrap().to_string()).unwrap();
    let timer: Arc<dyn Timer> = Arc::new(TokioTimer::new());
    let service = RelayService::new(
        timer.clone(),
        Arc::new(OsRelayEntropy),
        Arc::new(Ed25519Proofs),
        RelayLimits::default(),
    );
    let (stop_relay, relay_stopped) = tokio::sync::oneshot::channel::<()>();
    let relay_task = tokio::spawn(serve_tcp(socket, service, timer, async {
        relay_stopped.await.ok();
    }));

    let directory = tempfile::tempdir().unwrap();
    let a = Peer::start_with(&directory.path().join("a"), Some(relay_address.clone())).await;
    let b = Reader::prepare(&directory.path().join("b"), false).await;
    // B enrolls at an address in front of A, which then goes away.
    let front = Relay::start(a.native).await;
    let (key, reader) = enroll(&a, &b, &front, true).await;
    let reader = reader.unwrap();
    synced_with(&b, 0).await;
    drop(front);

    let kept = b
        .peers
        .relay(key, Some(relay_address.clone()), &b.owner)
        .await
        .unwrap();
    assert!(matches!(&kept, PeerEntry::Readable(record) if record.relay() == Some(&relay_address)));
    let x = a.conversation().await;
    a.grant(
        ShareGrantTransition::Grant(ShareAccess::read()),
        &x,
        &reader,
    )
    .await;
    synced_with(&b, 1).await;
    assert_eq!(b.cached(&key, &reader.0), vec![x.to_string()]);

    stop(a).await;
    stop_relay.send(()).unwrap();
    relay_task.await.unwrap();
}

/// R18: with the durable audit, records land in the order the turn was
/// held. A cycle's change is handed over while it holds the turn, so the
/// forget that stops it comes after it, though the forget's intent is kept
/// before it asks for the turn.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn records_land_in_turn_order() {
    let directory = tempfile::tempdir().unwrap();
    let a = Peer::start(&directory.path().join("a")).await;
    let audits = audit_directory(&directory.path().join("audits"));
    let audit = Arc::new(DurablePeerAudit::new(
        audits.clone(),
        Arc::new(SystemClock),
        RuntimeDependencies::default().clock,
    ));
    let b = Reader::prepare_with(
        &directory.path().join("b"),
        Changes::new(false),
        audit.clone(),
        RuntimeDependencies::default().clock,
    )
    .await;
    let relay = Relay::start(a.native).await;
    let (key, _) = enroll(&a, &b, &relay, true).await;

    // The cycle reads the Active status, saves the credential, and holds its
    // read; the forget stops it.
    relay.hold_after(1);
    let held = relay.held();
    let poller = b.poller(POLICY);
    relay.until_held(held).await;
    tokio::time::timeout(PROMPT, b.peers.forget(key, &b.owner))
        .await
        .expect("forget stops the read")
        .unwrap();
    tokio::time::timeout(WAIT, poller.join()).await.unwrap();
    assert!(audit.drained(AUDIT_DEADLINE).await);
    assert_eq!(
        audit_kinds(&audits),
        [
            "peer_enroll_requested",
            "peer_enroll_finished",
            "peer_poller_changed peer_approved",
            "peer_forget_requested",
            "peer_forget_finished",
        ]
    );
    assert_eq!(audit_sequences(&audits), [1, 2, 3, 4, 5]);

    drop(relay);
    stop(a).await;
}

/// R18: a store that stalls holds an owner command no longer than its own
/// audit deadline. The cycle's record is queued under the turn and the
/// writer stalls on it; the forget's intent waits behind it, in order, and is
/// answered `peer_audit_unavailable` once B's clock passes the deadline, with
/// nothing removed. Once the store answers, every record lands in sequence
/// and the forget goes ahead.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stalled_audit_never_holds_an_owner_command() {
    stalled_audit(None, false, None).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_late_audit_intent_does_not_spend_the_next_forgets_stop_window() {
    stalled_audit(Some(PROMPT + PROMPT / 2), false, None).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[should_panic(expected = "forget stopped the captured read")]
async fn an_ended_read_cannot_prove_a_forget_stopped_it() {
    stalled_audit(None, true, None).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_current_audit_intent_does_not_spend_the_forgets_stop_window() {
    stalled_audit(None, false, Some(5)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_current_audit_outcome_does_not_spend_the_forgets_stop_window() {
    stalled_audit(None, false, Some(6)).await;
}

async fn stalled_audit(
    late_write: Option<Duration>,
    end_read_before_forget: bool,
    delayed_current_sequence: Option<u64>,
) {
    let directory = tempfile::tempdir().unwrap();
    let a = Peer::with_held_record(&directory.path().join("a")).await;
    let record_hold = a.record_hold.as_ref().unwrap().clone();
    let _release_record = ReleaseRecord(record_hold.clone());
    let audits = audit_directory(&directory.path().join("audits"));
    let clock = Arc::new(Manual(AtomicU64::new(1_000)));
    let stall = Stall::new(3);
    let _release_first = ReleaseStall(stall.clone());
    let late = Stall::new(4);
    let release_late = ReleaseStall(late.clone());
    let current = Stall::new(delayed_current_sequence.unwrap_or(5));
    let release_current = ReleaseStall(current.clone());
    let before_write = {
        let (first, late, current) = (stall.hook(), late.hook(), current.hook());
        Arc::new(move |sequence| {
            first(sequence);
            if late_write.is_some() {
                late(sequence);
            }
            if delayed_current_sequence.is_some() {
                current(sequence);
            }
        })
    };
    let audit = Arc::new(DurablePeerAudit::with_writer_hook(
        audits.clone(),
        Arc::new(SystemClock),
        clock.clone(),
        crate::peer_gateways::infrastructure::AUDIT_QUEUE,
        before_write,
    ));
    let b = Reader::prepare_with(
        &directory.path().join("b"),
        Changes::new(false),
        audit.clone(),
        clock.clone(),
    )
    .await;
    let relay = Relay::start(a.native).await;
    let (key, reader) = enroll(&a, &b, &relay, true).await;
    let conversation = a.conversation().await;
    a.grant(
        ShareGrantTransition::Grant(ShareAccess::read()),
        &conversation,
        &reader.unwrap(),
    )
    .await;

    // Hold after authentication. Advancing the audit's five-second clock
    // must not also expire the read's five-second handshake.
    let poller = b.poller(POLICY);
    tokio::time::timeout(WAIT, record_hold.entered())
        .await
        .expect("the authenticated record read reached its source");
    let read_stop = b
        .peers
        .active_read_stop(&key)
        .expect("the captured read still owns its cycle");
    stall.until_reached().await;
    let forget = tokio::spawn({
        let (peers, owner) = (b.peers.clone(), b.owner.clone());
        async move { peers.forget(key, &owner).await }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!forget.is_finished(), "held while B's clock stands");
    clock
        .0
        .fetch_add(AUDIT_DEADLINE.as_millis() as u64, Ordering::SeqCst);
    let answered = tokio::time::timeout(PROMPT, forget)
        .await
        .expect("answered once the deadline passes")
        .unwrap();
    assert_eq!(answered.err(), Some(PeerError::AuditUnavailable));
    assert!(
        !read_stop.is_stopped(),
        "the refused forget did not stop the read"
    );
    assert_eq!(b.peers.list().await.unwrap().len(), 1, "nothing removed");

    stall.release();
    let release = tokio::spawn(async move {
        if let Some(delay) = late_write {
            tokio::time::sleep(delay).await;
        }
        drop(release_late);
    });
    // Releasing the hook is not the writer's completion. The first forget
    // already timed out; its queued intent still precedes the next command.
    tokio::time::timeout(AUDIT_DEADLINE, async {
        while audit.handled_sequence() < 4 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the old audit prefix finishes before timing the next forget");
    assert_eq!(audit_sequences(&audits), [1, 2, 3, 4]);
    if end_read_before_forget {
        record_hold.release();
        tokio::time::timeout(WAIT, async {
            while b.peers.active_read_stop(&key).is_some() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the negative control's read gave its turn back");
        assert!(
            !read_stop.is_stopped(),
            "ending a read did not ask for a stop"
        );
    }
    let current_release = tokio::spawn(async move {
        if delayed_current_sequence.is_some() {
            current.until_reached().await;
            tokio::time::sleep(PROMPT + PROMPT / 2).await;
        }
        drop(release_current);
    });
    let forget = tokio::spawn({
        let (peers, owner) = (b.peers.clone(), b.owner.clone());
        async move { peers.forget(key, &owner).await }
    });
    // The current intent must be durable before the owner may stop the
    // read. Its writer delay, like the old prefix's, is not preemption.
    let intent_kept = tokio::time::timeout(AUDIT_DEADLINE, async {
        while audit.handled_sequence() < 5 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    let stopped_in_time = if intent_kept.is_ok() {
        tokio::time::timeout(PROMPT, async {
            while !read_stop.is_stopped() || b.peers.active_read_stop(&key).is_some() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .is_ok()
    } else {
        false
    };
    // Joining the poller also stops cycles; retain the owner's fact first,
    // including when preemption failed. Release and join before asserting.
    let stopped_captured_read = read_stop.is_stopped();
    record_hold.release();
    let forgotten = tokio::time::timeout(WAIT, forget).await;
    current_release.await.unwrap();
    tokio::time::timeout(WAIT, poller.join()).await.unwrap();
    tokio::time::timeout(WAIT, a.record_source.shutdown())
        .await
        .unwrap()
        .unwrap();
    assert!(audit.drained(AUDIT_DEADLINE).await);
    assert_eq!(
        audit_kinds(&audits),
        [
            "peer_enroll_requested",
            "peer_enroll_finished",
            "peer_poller_changed peer_approved",
            // The first forget's intent, kept late; its call had already
            // been refused, so it has no outcome and changed nothing.
            "peer_forget_requested",
            "peer_forget_requested",
            "peer_forget_finished",
        ]
    );
    assert_eq!(audit_sequences(&audits), [1, 2, 3, 4, 5, 6]);
    release.await.unwrap();

    drop(relay);
    stop(a).await;
    assert!(stopped_captured_read, "forget stopped the captured read");
    assert!(intent_kept.is_ok(), "the current intent was kept");
    assert!(
        stopped_in_time,
        "forget released the captured read within its stop window"
    );
    forgotten
        .expect("forget completed after the read was released")
        .unwrap()
        .unwrap();
    assert!(
        b.peers.list().await.unwrap().is_empty(),
        "the peer is removed"
    );
}

fn filler() -> PeerAuditRecord {
    PeerAuditRecord::ForgetRequested {
        operation: Uuid::new_v4(),
        initiator: PrincipalId::new("filler").unwrap(),
        peer: DeviceKey::new([9; 32]),
    }
}

/// R21: a full audit queue answers at once, on a clock that never moves. An
/// owner command is refused `peer_audit_unavailable` before it dials or
/// removes anything; a poller change still lands, and its record, refused,
/// is logged and never written.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_full_queue_refuses_the_owner_and_drops_the_poller_record() {
    let directory = tempfile::tempdir().unwrap();
    let a = Peer::start(&directory.path().join("a")).await;
    let audits = audit_directory(&directory.path().join("audits"));
    let stall = Stall::new(3);
    let audit = Arc::new(DurablePeerAudit::with_writer_hook(
        audits.clone(),
        Arc::new(SystemClock),
        Arc::new(Manual(AtomicU64::new(1_000))),
        1,
        stall.hook(),
    ));
    let b = Reader::prepare_with(
        &directory.path().join("b"),
        Changes::new(false),
        audit.clone(),
        Arc::new(Manual(AtomicU64::new(1_000))),
    )
    .await;
    let relay = Relay::start(a.native).await;
    let (key, _) = enroll(&a, &b, &relay, true).await;

    // The writer stalls on one record, and one more fills the queue.
    drop(audit.record(filler()));
    stall.until_reached().await;
    let queued = audit.record(filler());

    // The poller's change lands; its record has no room.
    synced_with(&b, 0).await;
    let (entry, _) = b.peer().await;
    assert!(matches!(
        &entry,
        PeerEntry::Readable(record) if matches!(record.phase(), PeerPhase::Active { .. })
    ));

    // The owner is refused at once, before anything is dialed or removed.
    let accepted = relay.accepted();
    let created = a
        .commands
        .create(&a.session, ConsentClass::PeerRead)
        .await
        .unwrap();
    let code = ManualCode::parse(created.code().expose_bytes()).unwrap();
    let enrolled =
        tokio::time::timeout(PROMPT, b.peers.enroll(relay.address, None, code, &b.owner))
            .await
            .expect("a full queue answers at once");
    assert_eq!(enrolled.err(), Some(PeerError::AuditUnavailable));
    let forgotten = tokio::time::timeout(PROMPT, b.peers.forget(key, &b.owner))
        .await
        .expect("a full queue answers at once");
    assert_eq!(forgotten.err(), Some(PeerError::AuditUnavailable));
    assert_eq!(relay.accepted(), accepted, "nothing dialed");
    assert_eq!(b.peers.list().await.unwrap().len(), 1, "nothing removed");

    // The store catches up; the next record is kept, and the refused ones
    // are the gap before it: the poller's change, the enroll, the forget.
    stall.release();
    assert_eq!(queued.await, Ok(()));
    assert_eq!(audit.record(filler()).await, Ok(()));
    assert!(audit.drained(AUDIT_DEADLINE).await);
    assert_eq!(
        audit_kinds(&audits),
        [
            "peer_enroll_requested",
            "peer_enroll_finished",
            "peer_forget_requested",
            "peer_forget_requested",
            "peer_forget_requested",
        ],
        "the fillers, and no poller record"
    );
    assert_eq!(audit_sequences(&audits), [1, 2, 3, 4, 8]);

    drop(relay);
    stop(a).await;
}

/// The sync last reported for `key`, if it is still listed.
async fn sync_of(b: &Reader, key: &DeviceKey) -> Option<PeerSync> {
    b.peers
        .list()
        .await
        .unwrap()
        .into_iter()
        .find_map(|(entry, sync)| {
            let PeerEntry::Readable(record) = entry else {
                return None;
            };
            (*record.key() == *key).then_some(sync).flatten()
        })
}

/// Two grantors and one reader, each grantor behind its own relay. Neither
/// peer is enrolled until the test says so.
struct TwoPeers {
    a: Peer,
    c: Peer,
    b: Reader,
    relay_a: Relay,
    relay_c: Relay,
    key_a: DeviceKey,
    key_c: DeviceKey,
}
impl TwoPeers {
    async fn start(root: &Path) -> Self {
        let a = Peer::start(&root.join("a")).await;
        let c = Peer::start(&root.join("c")).await;
        let b = Reader::prepare(&root.join("b"), false).await;
        let relay_a = Relay::start(a.native).await;
        let relay_c = Relay::start(c.native).await;
        Self {
            a,
            c,
            b,
            relay_a,
            relay_c,
            key_a: DeviceKey::new([0; 32]),
            key_c: DeviceKey::new([0; 32]),
        }
    }
    async fn enroll_both(&mut self) {
        let (key_a, _) = enroll(&self.a, &self.b, &self.relay_a, true).await;
        let (key_c, _) = enroll(&self.c, &self.b, &self.relay_c, true).await;
        self.key_a = key_a;
        self.key_c = key_c;
    }

    /// A caller can resume after the new peer already completed its first sync.
    async fn enroll_a(&mut self, completed_before_return: bool) {
        let (key, _) = enroll(&self.a, &self.b, &self.relay_a, true).await;
        self.key_a = key;
        if completed_before_return {
            tokio::time::timeout(WAIT, async {
                loop {
                    if sync_of(&self.b, &key).await.is_some_and(|sync| {
                        sync.state == SyncState::Synced
                            && sync
                                .last_synced_ms
                                .is_some_and(|at| SystemClock.unix_milliseconds() > at)
                    }) {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            })
            .await
            .expect("A completes before the enrollment caller resumes");
        }
    }

    async fn add_a_beside_held_c(
        &mut self,
        policy: PollPolicy,
        completed_before_return: bool,
    ) -> (PeerPoller, usize) {
        let (key_c, _) = enroll(&self.c, &self.b, &self.relay_c, true).await;
        self.key_c = key_c;
        self.relay_c.hold_after(1);
        let poller = self.b.poller(policy);
        self.relay_c.until_held(0).await;
        let held = self.relay_c.held();

        let since = SystemClock.unix_milliseconds();
        self.enroll_a(completed_before_return).await;
        let beside = HANDSHAKE.saturating_sub(Duration::from_secs(2));
        let synced = tokio::time::timeout(beside, async {
            loop {
                if sync_of(&self.b, &self.key_a).await.is_some_and(|sync| {
                    sync.state == SyncState::Synced
                        && sync.last_synced_ms.is_some_and(|at| at >= since)
                }) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        assert!(
            synced.is_ok(),
            "A was not read while C's read was held; C held {} connections",
            self.relay_c.held()
        );
        assert_eq!(
            self.relay_c.held(),
            held,
            "reading A restarted C's held read"
        );
        (poller, held)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_first_sync_completed_before_observation_is_not_lost() {
    let directory = tempfile::tempdir().unwrap();
    let mut peers = TwoPeers::start(directory.path()).await;
    let policy = PollPolicy {
        interval: HANDSHAKE * 2,
        backoff_cap: HANDSHAKE * 2,
        ..POLICY
    };
    let (poller, _) = peers.add_a_beside_held_c(policy, true).await;
    peers.relay_c.pass_all();
    tokio::time::timeout(WAIT, poller.join()).await.unwrap();
    stop(peers.a).await;
    stop(peers.c).await;
}

/// A slow peer's read stays open while another peer, enrolled after that
/// read has already started, is read through to `synced`. Forgetting the
/// finished peer does not stop the slow one, and a forget that cannot take
/// the record set does not either.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_slow_peer_does_not_hold_up_the_others() {
    let directory = tempfile::tempdir().unwrap();
    let mut peers = TwoPeers::start(directory.path()).await;
    // C's status goes through and its read is held. A is not enrolled yet,
    // so the only read that can start is C's.
    let (poller, held) = peers.add_a_beside_held_c(POLICY, false).await;

    // Forgetting A stops A's cycle only. C's held read is the same one.
    peers
        .b
        .peers
        .forget(peers.key_a, &peers.b.owner)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        peers.relay_c.held(),
        held,
        "forgetting A stopped C's read, and it started another"
    );

    // An enrollment holds the record set. Forgetting C is busy, and that
    // refusal does not stop C's read: the record set is taken before, and
    // instead of, C's turn.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (sent, accepted) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        if let Ok((socket, _)) = listener.accept().await {
            let _ = sent.send(socket);
        }
    });
    let created = peers
        .a
        .commands
        .create(&peers.a.session, ConsentClass::PeerRead)
        .await
        .unwrap();
    let code = ManualCode::parse(created.code().expose_bytes()).unwrap();
    let enroll_task = tokio::spawn({
        let (commands, owner) = (peers.b.peers.clone(), peers.b.owner.clone());
        async move { commands.enroll(address, None, code, &owner).await }
    });
    let socket = tokio::time::timeout(WAIT, accepted)
        .await
        .expect("the enrollment did not connect")
        .unwrap();
    assert_eq!(
        peers
            .b
            .peers
            .forget(peers.key_c, &peers.b.owner)
            .await
            .err(),
        Some(PeerError::Busy)
    );
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(peers.relay_c.held(), held, "a busy forget stopped C's read");
    drop(socket);
    let enrolled = tokio::time::timeout(WAIT, enroll_task)
        .await
        .expect("the enrollment did not end once its socket closed")
        .unwrap();
    assert!(enrolled.is_err(), "the silent peer enrolled: {enrolled:?}");
    assert_eq!(peers.relay_c.held(), held);

    peers.relay_c.pass_all();
    tokio::time::timeout(WAIT, poller.join())
        .await
        .expect("a stop joins the slow read");
    stop(peers.a).await;
    stop(peers.c).await;
}

/// One peer's listener is gone. That peer is reported unreachable and the
/// other is still read through to synced. The failure is not the pass's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failing_peer_is_reported_and_the_other_is_read() {
    let directory = tempfile::tempdir().unwrap();
    let mut peers = TwoPeers::start(directory.path()).await;
    peers.enroll_both().await;
    stop(peers.c).await;
    let since = SystemClock.unix_milliseconds();
    let poller = peers.b.poller(POLICY);
    let reached = tokio::time::timeout(WAIT, async {
        loop {
            let a = sync_of(&peers.b, &peers.key_a).await;
            let c = sync_of(&peers.b, &peers.key_c).await;
            let a_synced = a.is_some_and(|sync| {
                sync.state == SyncState::Synced && sync.last_synced_ms.is_some_and(|at| at >= since)
            });
            let c_failed = c.is_some_and(|sync| sync.state == SyncState::Unreachable);
            if a_synced && c_failed {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    tokio::time::timeout(WAIT, poller.join()).await.unwrap();
    assert!(
        reached.is_ok(),
        "A synced: {:?}; C: {:?}",
        sync_of(&peers.b, &peers.key_a).await,
        sync_of(&peers.b, &peers.key_c).await
    );
    stop(peers.a).await;
}

/// With one read slot, a second peer's read does not start until the first
/// gives the slot back.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_read_past_the_cap_waits_for_a_slot() {
    let directory = tempfile::tempdir().unwrap();
    let mut peers = TwoPeers::start(directory.path()).await;
    peers.enroll_both().await;
    let (accepted_a, accepted_c) = (peers.relay_a.accepted(), peers.relay_c.accepted());
    peers.relay_a.hold_after(1);
    peers.relay_c.hold_after(1);
    let poller = peers.b.poller(PollPolicy {
        reads_at_once: 1,
        ..POLICY
    });
    tokio::time::timeout(WAIT, async {
        while peers.relay_a.held() == 0 && peers.relay_c.held() == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("no read was held");
    tokio::time::sleep(Duration::from_millis(400)).await;
    let (busy, waiting, waiting_before) = if peers.relay_a.held() > 0 {
        (&peers.relay_a, &peers.relay_c, accepted_c)
    } else {
        (&peers.relay_c, &peers.relay_a, accepted_a)
    };
    assert_eq!(
        waiting.accepted(),
        waiting_before,
        "the second peer was read while the only slot was taken"
    );
    let waiting_accepted = waiting_before;
    busy.pass_all();
    tokio::time::timeout(WAIT, async {
        while waiting.accepted() == waiting_accepted {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the waiting peer was not read once a slot was free");
    tokio::time::timeout(WAIT, poller.join()).await.unwrap();
    stop(peers.a).await;
    stop(peers.c).await;
}

/// A held read wakes every tick. It scans the peer records when they change,
/// not on each of those ticks, and an enrollment during the hold is still
/// read. The new record is saved pending before its owner approves it, and
/// that approval does not change the record, so the first pending status is
/// read again soon (`a_peer_seen_pending_is_read_again_soon_and_then_waits_the_interval`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_held_read_lists_when_the_records_change_not_on_every_tick() {
    let directory = tempfile::tempdir().unwrap();
    let mut peers = TwoPeers::start(directory.path()).await;
    let (key_c, _) = enroll(&peers.c, &peers.b, &peers.relay_c, true).await;
    peers.key_c = key_c;
    peers.relay_c.hold_after(1);
    let poller = peers.b.poller(PollPolicy {
        interval: Duration::from_secs(60),
        backoff_cap: Duration::from_secs(60),
        ..POLICY
    });
    peers.relay_c.until_held(0).await;
    let listed = poller.lists();
    tokio::time::sleep(Duration::from_millis(400)).await;
    let during = poller.lists();
    assert!(
        during <= listed + 1,
        "a held read scanned the records {during} times after {listed}"
    );

    let since = SystemClock.unix_milliseconds();
    let (key_a, _) = enroll(&peers.a, &peers.b, &peers.relay_a, true).await;
    peers.key_a = key_a;
    let synced = tokio::time::timeout(WAIT, async {
        loop {
            if sync_of(&peers.b, &peers.key_a).await.is_some_and(|sync| {
                sync.state == SyncState::Synced && sync.last_synced_ms.is_some_and(|at| at >= since)
            }) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(
        synced.is_ok(),
        "A was not read after its record changed; scans went from {during} to {}",
        poller.lists()
    );
    assert!(
        poller.lists() > during,
        "enrolling A did not scan the records again"
    );
    peers.relay_c.pass_all();
    tokio::time::timeout(WAIT, poller.join()).await.unwrap();
    stop(peers.a).await;
    stop(peers.c).await;
}
