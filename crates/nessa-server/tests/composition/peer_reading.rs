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
use crate::composition::runtime_config::NativeConfig;
use crate::conversation::application::{
    ConversationCaller, ConversationRepository, ReadGrantChange, ReadGrantTransition, ReadGrants,
};
use crate::conversation::domain::{Conversation, ConversationDeletion};
use crate::conversation::infrastructure::{
    LocalConversationStore, LocalReceiverAuthority, NessaCatalogueReadSource, NessaRecordReadSource,
};
use crate::device_pairing::infrastructure::PairingOwnerCommands;
use crate::peer_gateways::application::{
    CacheState, PeerAudit, PeerAuditFuture, PeerAuditRecord, PeerAuditUnavailable, PeerHolding,
    PeerState, PollerCause,
};
use crate::peer_gateways::infrastructure::{
    DurablePeerAudit, EnrollmentEntropy, EnrollmentEntropySource, PeerCommands, PeerEntry,
    PeerError, PeerPhase, PeerPoller, PeerSync, PollInputs, PollPolicy, SyncState,
    TcpPeerConnector, AUDIT_DEADLINE,
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
use nessa_client_core::retained::RetainedCache;
use nessa_protocol::agents::AgentId;
use nessa_protocol::conversation::domain::{
    ConversationApprovalMode, ConversationId, ConversationModelId,
};
use nessa_sdk::application::agent_execution::sessions::SessionStorage;
use nessa_sdk::infrastructure::session_storage::RecordStorage;
use nessa_sync::replication::domain::Id;
use serde_json::Value;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::Handle;
use uuid::Uuid;

/// The longest any one wait below may take.
const WAIT: Duration = Duration::from_secs(30);
/// B reads A this often, so each step settles in well under a second.
const POLICY: PollPolicy = PollPolicy {
    interval: Duration::from_millis(100),
    backoff_cap: Duration::from_millis(400),
    read_budget: Duration::from_secs(60),
};
/// Well under the handshake deadline a held read would otherwise reach (5 s),
/// so an end inside it was a stop, not that deadline.
const PROMPT: Duration = Duration::from_secs(2);

fn uuid() -> String {
    Uuid::new_v4().to_string()
}

struct NoAgents;
impl crate::agents::application::AgentProbe for NoAgents {
    fn evidence(&self, _: AgentId) -> Option<crate::agents::application::AgentProbeEvidence> {
        None
    }
}

/// A registry with one owner who may manage credentials and read and write
/// conversations on `gateway`, and that owner's authenticated session.
async fn owned(
    root: &Path,
    gateway: &str,
    organization: &str,
) -> (Arc<LocalCredentialStore>, AuthenticatedSession, String) {
    nessa_local_storage::create_directory(root).unwrap();
    let auth = Arc::new(LocalCredentialStore::open(root, "credentials.json").unwrap());
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
        verifier: auth.as_ref(),
        access: auth.as_ref(),
        clock: &SystemClock,
    }
    .execute(&boot.evidence, &AudienceId::new(gateway).unwrap())
    .await
    .unwrap();
    (auth, session, owner)
}

fn inputs(
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
}
impl Peer {
    async fn start(root: &Path) -> Self {
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
            },
            inputs(root, &auth, receivers.clone(), &gateway, &organization),
        )
        .await
        .unwrap();
        let metadata =
            Arc::new(LocalConversationStore::open(&root.join("metadata.sqlite3")).unwrap());
        let storage = Arc::new(RecordStorage::new(root.join("source")).unwrap());
        storage.initialize().await.unwrap();
        let state = ProductRouteState::new(
            ResourceId::new(gateway.clone()).unwrap(),
            OrganizationId::new(organization.clone()).unwrap(),
            AudienceId::new(gateway.clone()).unwrap(),
            ProductDependencies {
                verifier: auth.clone(),
                access: auth.clone(),
                clock: Arc::new(SystemClock),
                policy: Arc::new(CedarPolicyEvaluator::new().unwrap()),
                uptime_clock: RuntimeDependencies::default().clock,
                agent_probe: Arc::new(NoAgents),
            },
        )
        .with_passive_read(receivers, metadata.clone(), metadata.clone())
        .with_record_source(Arc::new(NessaRecordReadSource::new(
            storage.clone(),
            Id::new(&gateway).unwrap(),
            Handle::current(),
        )))
        .with_catalogue_source(Arc::new(NessaCatalogueReadSource::new(
            metadata.clone(),
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
        transition: ReadGrantTransition,
        id: &ConversationId,
        reader: &(String, String),
    ) {
        ReadGrants::change(
            self.metadata.as_ref(),
            ReadGrantChange {
                transition,
                conversation_id: id.clone(),
                receiver_id: Some(reader.0.clone()),
                credential_id: CredentialId::new(reader.1.clone()).unwrap(),
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
    fn record(&self, record: PeerAuditRecord) -> PeerAuditFuture<'_> {
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

/// Each record's kind, with the poller's cause, in sequence order; and the
/// sequences are 1, 2, 3 ... with no gap, their observation times in order.
fn audit_kinds(directory: &Path) -> Vec<String> {
    let records = audit_files(directory);
    let sequences: Vec<u64> = records
        .iter()
        .map(|record| record["sequence"].as_u64().unwrap())
        .collect();
    assert_eq!(sequences, (1..=records.len() as u64).collect::<Vec<_>>());
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
    let pending = b.peers.enroll(relay.address, code, &b.owner).await.unwrap();
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
    a.grant(ReadGrantTransition::Grant, &x, &reader).await;
    synced_with(&b, 1).await;
    assert_eq!(b.cached(&key, &reader.0), vec![x.to_string()]);

    // R3: A shares Y and unshares X; the next read takes both.
    a.grant(ReadGrantTransition::Grant, &y, &reader).await;
    a.grant(ReadGrantTransition::Revoke, &x, &reader).await;
    synced_with(&b, 1).await;
    assert_eq!(b.cached(&key, &reader.0), vec![y.to_string()]);

    // R4: A unshares Y and shares nothing else; B's cache empties. Shared
    // again, it comes back.
    a.grant(ReadGrantTransition::Revoke, &y, &reader).await;
    synced_with(&b, 0).await;
    assert!(b.cached(&key, &reader.0).is_empty());
    a.grant(ReadGrantTransition::Grant, &y, &reader).await;
    synced_with(&b, 1).await;

    // R5: A shares Z, then deletes it; B's cache drops it.
    a.grant(ReadGrantTransition::Grant, &z, &reader).await;
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

    // R21: a cache whose last walk was cut short, after a failure or a
    // restart, has no note: every poller here is a new one, as after a
    // restart. At A's unchanged head, the cache is walked again, not taken
    // as settled, and the walk notes itself once it finishes.
    b.tamper(&key, "DELETE FROM retained_walks");
    synced_with(&b, 1).await;
    assert_eq!(b.walks(&key), 1, "walked again at an unchanged head");

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
    a.grant(ReadGrantTransition::Grant, &y, &reader).await;
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
    let directory = tempfile::tempdir().unwrap();
    let a = Peer::start(&directory.path().join("a")).await;
    let audits = audit_directory(&directory.path().join("audits"));
    let clock = Arc::new(Manual(AtomicU64::new(1_000)));
    let stall = Stall::new(3);
    let audit = Arc::new(DurablePeerAudit::with_writer_hook(
        audits.clone(),
        Arc::new(SystemClock),
        clock.clone(),
        crate::peer_gateways::infrastructure::AUDIT_QUEUE,
        stall.hook(),
    ));
    let b = Reader::prepare_with(
        &directory.path().join("b"),
        Changes::new(false),
        audit.clone(),
        clock.clone(),
    )
    .await;
    let relay = Relay::start(a.native).await;
    let (key, _) = enroll(&a, &b, &relay, true).await;

    relay.hold_after(1);
    let held = relay.held();
    let poller = b.poller(POLICY);
    relay.until_held(held).await;
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
    assert_eq!(b.peers.list().await.unwrap().len(), 1, "nothing removed");

    stall.release();
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
            // The first forget's intent, kept late; its call had already
            // been refused, so it has no outcome and changed nothing.
            "peer_forget_requested",
            "peer_forget_requested",
            "peer_forget_finished",
        ]
    );

    drop(relay);
    stop(a).await;
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
    drop(audit.record(filler()));

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
    let enrolled = tokio::time::timeout(PROMPT, b.peers.enroll(relay.address, code, &b.owner))
        .await
        .expect("a full queue answers at once");
    assert_eq!(enrolled.err(), Some(PeerError::AuditUnavailable));
    let forgotten = tokio::time::timeout(PROMPT, b.peers.forget(key, &b.owner))
        .await
        .expect("a full queue answers at once");
    assert_eq!(forgotten.err(), Some(PeerError::AuditUnavailable));
    assert_eq!(relay.accepted(), accepted, "nothing dialed");
    assert_eq!(b.peers.list().await.unwrap().len(), 1, "nothing removed");

    stall.release();
    assert!(audit.drained(AUDIT_DEADLINE).await);
    assert_eq!(
        audit_kinds(&audits),
        [
            "peer_enroll_requested",
            "peer_enroll_finished",
            "peer_forget_requested",
            "peer_forget_requested",
        ],
        "the two fillers, and no poller record"
    );

    drop(relay);
    stop(a).await;
}
