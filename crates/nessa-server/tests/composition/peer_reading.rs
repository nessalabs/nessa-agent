//! Two composed gateways in one process: B enrolls into A as a peer, A's
//! owner approves, and B's poller reads what A grants it into B's retained
//! cache, follows A's shares and unshares, and stops at A's revocation.
//! Rows R1–R4 in `docs/design/auth/peer-gateways.md` ("Reading a peer").
use crate::app::dependencies::RuntimeDependencies;
use crate::composition::local_auth::SystemClock;
use crate::composition::native_pairing::{bind, prepare, start, NativeInputs, RunningNative};
use crate::composition::runtime_config::NativeConfig;
use crate::conversation::application::{
    ConversationCaller, ConversationRepository, ReadGrantChange, ReadGrantTransition, ReadGrants,
};
use crate::conversation::domain::Conversation;
use crate::conversation::infrastructure::{
    LocalConversationStore, LocalReceiverAuthority, NessaCatalogueReadSource, NessaRecordReadSource,
};
use crate::device_pairing::infrastructure::PairingOwnerCommands;
use crate::peer_gateways::infrastructure::{
    PeerCommands, PeerEntry, PeerPhase, PeerPoller, PeerSync, PollPolicy, SyncState,
};
use crate::product::{ProductDependencies, ProductRouteState};
use nessa_auth::adapters::cedar::CedarPolicyEvaluator;
use nessa_auth::adapters::local::{BootstrapRequest, LocalCredentialStore};
use nessa_auth::adapters::pairing::ManualCode;
use nessa_auth::application::credential_admin::RevokeCredentialRequest;
use nessa_auth::application::dto::{
    CredentialGrantDto, MembershipInputDto, MembershipRoleDto, MembershipStateDto,
    OrganizationInputDto, PrincipalInputDto, PrincipalKindDto, ResourceDto,
};
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
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Handle;
use uuid::Uuid;

/// The longest any one wait below may take.
const WAIT: Duration = Duration::from_secs(30);
/// B reads A this often, so each step settles in well under a second.
const POLICY: PollPolicy = PollPolicy {
    interval: Duration::from_millis(100),
    backoff_cap: Duration::from_millis(400),
};

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

/// Gateway B: native pairing prepared, for its key and its peer commands;
/// its poller is started and stopped by each step.
struct Reader {
    peers: Arc<PeerCommands>,
    owner: PrincipalId,
    directory: PathBuf,
}
impl Reader {
    async fn prepare(root: &Path) -> Self {
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
        let (_, _, peers) = prepare(
            &NativeConfig {
                listen_address: "127.0.0.1:0".parse().unwrap(),
            },
            inputs(root, &auth, receivers, &gateway, &organization),
        )
        .await
        .unwrap();
        #[cfg(unix)]
        let root = root.canonicalize().unwrap();
        Self {
            peers,
            owner: PrincipalId::new(owner).unwrap(),
            // Where composition keeps peer records and their caches.
            directory: root.join("peer-gateways"),
        }
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
        let poller = PeerPoller::start(self.peers.clone(), POLICY, Arc::new(SystemClock));
        let last = std::sync::Mutex::new(None);
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
        poller.join().await;
        assert!(
            reached.is_ok(),
            "the peer's listing never reached the expected state; last: {:?}",
            last.lock().unwrap()
        );
    }
    /// The conversations B's cache of `key` holds for `receiver`.
    fn cached(&self, key: &DeviceKey, receiver: &str) -> Vec<String> {
        let hex: String = key
            .bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let mut cache = RetainedCache::open(
            &self.directory.join(format!("{hex}.sqlite3")),
            Arc::new(SystemClock),
            RuntimeDependencies::default().clock,
        )
        .unwrap();
        let mut ids = cache.conversation_ids(receiver).unwrap();
        ids.sort();
        ids
    }
}

/// A read that finished at or after `since`, complete.
fn synced_since(since: u64) -> impl Fn(&PeerEntry, Option<&PeerSync>) -> bool {
    move |_, sync| {
        sync.is_some_and(|sync| {
            sync.state == SyncState::Synced && sync.last_synced_ms.is_some_and(|at| at >= since)
        })
    }
}

/// Rows R1–R4: the two-gateway read, end to end.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_peer_reads_only_what_it_is_granted_and_stops_at_revocation() {
    let directory = tempfile::tempdir().unwrap();
    let mut a = Peer::start(&directory.path().join("a")).await;
    let b = Reader::prepare(&directory.path().join("b")).await;

    // R1: B enrolls into A's peer invitation and A's owner approves; B's
    // poller reads the Active status, saves the credential, and reads an
    // empty grant.
    let created = a
        .commands
        .create(&a.session, ConsentClass::PeerRead)
        .await
        .unwrap();
    let code = ManualCode::parse(created.code().expose_bytes()).unwrap();
    let pending = b.peers.enroll(a.native, code, &b.owner).await.unwrap();
    let key = *pending.key();
    let id = created.record().id();
    let claimed = a.commands.status(&a.session, id).await.unwrap();
    let (_, claim) = claimed.claim_binding().unwrap();
    let approved = a
        .commands
        .approve(&a.session, id, claim)
        .await
        .unwrap()
        .record;
    let (receiver, _) = approved.receiver_binding().unwrap();
    let reader = (
        receiver.as_str().to_owned(),
        approved.credential().unwrap().as_str().to_owned(),
    );
    let (x, y) = (a.conversation().await, a.conversation().await);
    let since = SystemClock.unix_milliseconds();
    b.poll_until(synced_since(since)).await;
    let (entry, sync) = b.peer().await;
    assert!(matches!(
        &entry,
        PeerEntry::Readable(record) if matches!(record.phase(), PeerPhase::Active { .. })
    ));
    assert_eq!(sync.unwrap().conversations, Some(0));
    assert!(b.cached(&key, &reader.0).is_empty());

    // R2: A shares X and not Y; B's cache gets X only.
    a.grant(ReadGrantTransition::Grant, &x, &reader).await;
    let since = SystemClock.unix_milliseconds();
    b.poll_until(synced_since(since)).await;
    assert_eq!(b.peer().await.1.unwrap().conversations, Some(1));
    assert_eq!(b.cached(&key, &reader.0), vec![x.to_string()]);

    // R3: A shares Y and unshares X; the next read takes both.
    a.grant(ReadGrantTransition::Grant, &y, &reader).await;
    a.grant(ReadGrantTransition::Revoke, &x, &reader).await;
    let since = SystemClock.unix_milliseconds();
    b.poll_until(synced_since(since)).await;
    assert_eq!(b.peer().await.1.unwrap().conversations, Some(1));
    assert_eq!(b.cached(&key, &reader.0), vec![y.to_string()]);

    // R4: A's owner revokes B's credential. B's next read is refused, its
    // pinned status says the enrollment ended, and B marks the record
    // revoked, removes the cache, and reads A no more.
    a.auth
        .revoke_sync(RevokeCredentialRequest {
            request_id: uuid(),
            issuer_principal_id: a.owner.clone(),
            credential_id: reader.1.clone(),
            revoked_at: SystemClock.unix_seconds(),
        })
        .unwrap();
    b.poll_until(|entry, sync| {
        sync.is_none()
            && matches!(entry, PeerEntry::Readable(record) if record.phase() == &PeerPhase::Revoked)
    })
    .await;
    let hex: String = key
        .bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert!(
        !b.directory.join(format!("{hex}.sqlite3")).exists(),
        "the cache is removed"
    );

    // Nothing left running: A's listener drains; B's pollers were joined.
    let mut running = a.running.take().unwrap();
    running.signal_stop();
    tokio::time::timeout(WAIT, running.join())
        .await
        .unwrap()
        .unwrap();
}
