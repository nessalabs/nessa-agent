//! Independent client/gateway processes use canonical credential, receiver and cache owners.
use super::{private_write, uuid, Setup};
use crate::agents::application::{AgentProbe, AgentProbeEvidence};
use crate::agents::domain::AgentId;
use crate::app::dependencies::RuntimeDependencies;
use crate::composition::local_auth::SystemClock;
use crate::composition::native_pairing::{bind, prepare, start, NativeInputs};
use crate::composition::runtime_config::NativeConfig;
use crate::conversation::application::{
    ConversationRepository, ReceiverReadScope, RecordReadError, RecordReadFuture, RecordReadLease,
    RecordReadOperation, RecordReadResponse, RecordReadSource,
};
use crate::conversation::domain::{
    Conversation, ConversationApprovalMode, ConversationId, ConversationModelId,
};
use crate::conversation::infrastructure::{
    LocalConversationStore, LocalReceiverAuthority, NessaCatalogueReadSource,
    NessaRecordReadSource, NessaRecordWatches,
};
use crate::device_pairing::infrastructure::{NativeEnrollmentClient, PairingOwnerCommands};
use crate::product::{ProductDependencies, ProductRouteState};
use nessa_auth::adapters::cedar::CedarPolicyEvaluator;
use nessa_auth::adapters::local::{BootstrapRequest, LocalCredentialStore};
use nessa_auth::adapters::pairing::{FilePairingState, ManualCode, OsEntropy};
use nessa_auth::application::credential_admin::RevokeCredentialRequest;
use nessa_auth::application::dto::{
    CredentialGrantDto, MembershipInputDto, MembershipRoleDto, MembershipStateDto,
    OrganizationInputDto, PrincipalInputDto, PrincipalKindDto, ResourceDto,
};
use nessa_auth::application::ports::Clock;
use nessa_auth::application::session::{AuthenticateSession, AuthenticatedSession};
use nessa_auth::domain::{AudienceId, OrganizationId, PrincipalId, Resource, ResourceId};
use nessa_protocol::pairing::wire::NativePairingStatus;
use nessa_sdk::application::agent_execution::providers::ProviderIdentity;
use nessa_sdk::application::agent_execution::sessions::{
    SessionChange, SessionSaveUnit, SessionSnapshot, SessionStorage,
};
use nessa_sdk::domain::agent_execution::sessions::{
    ExecutionSessionId, ProviderContext, SessionId,
};
use nessa_sdk::infrastructure::session_storage::RecordStorage;
use nessa_sync::replication::domain::Id;
use serde_json::json;
use std::io::{self, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::io::{stdin, AsyncBufReadExt, BufReader};
use tokio::runtime::Handle;

struct NoAgents;
impl AgentProbe for NoAgents {
    fn evidence(&self, _: AgentId) -> Option<AgentProbeEvidence> {
        None
    }
}
// Deterministic timing at the actual source port; replies still come from SDK storage.
struct GatedRead {
    source: Arc<NessaRecordReadSource>,
    storage: Arc<RecordStorage>,
    /// The paired receiver and the seeded conversation, known once the first
    /// start has paired the device.
    target: Arc<OnceLock<(String, SessionId)>>,
    authority: Arc<LocalReceiverAuthority>,
    initiator: PrincipalId,
    mode: String,
    pages: AtomicUsize,
    heads: Arc<HeadHold>,
}
/// Live mode's held head read: the head reads counted so far, the one
/// `hold` named (zero for none), and the `release` that lets it answer.
#[derive(Default)]
struct HeadHold {
    count: AtomicUsize,
    held: AtomicUsize,
    release: tokio::sync::Notify,
}
impl HeadHold {
    /// Hold the second head read from now: a `watch`'s discovery reads the
    /// first, its recheck pass the second.
    fn hold_recheck(&self) {
        let next = self.count.load(Ordering::SeqCst) + 2;
        self.held.store(next, Ordering::SeqCst);
    }
}
impl RecordReadSource for GatedRead {
    fn read<'a>(
        &'a self,
        admitted: ReceiverReadScope,
        operation: RecordReadOperation,
        lease: RecordReadLease,
    ) -> RecordReadFuture<'a, RecordReadResponse> {
        let page = matches!(&operation, RecordReadOperation::Page(_));
        Box::pin(async move {
            let head = (!page).then(|| self.heads.count.fetch_add(1, Ordering::SeqCst) + 1);
            // Source latency exercises the real socket phase owner (B1), not a
            // fabricated timeout reply. Successful reads still use SDK storage.
            match (self.mode.as_str(), head) {
                ("delayed-head", Some(1)) => tokio::time::sleep(Duration::from_secs(6)).await,
                // The recheck pass `hold` named answers only after `release`,
                // so a commit acknowledged in between lands while it reads
                // the head (committed change watches L9).
                ("live", Some(count)) if count == self.heads.held.load(Ordering::SeqCst) => {
                    self.heads.release.notified().await
                }
                ("timeout-head", Some(1)) => std::future::pending::<()>().await,
                // A cold first head read, as a loaded gateway answers it
                // (row W17); the next head read succeeds.
                ("preparing-head", Some(1)) => return Err(RecordReadError::SourcePreparing),
                ("cumulative-head", Some(count)) if count > 1 => {
                    tokio::time::sleep(Duration::from_secs(3)).await
                }
                ("cumulative-head", None) => tokio::time::sleep(Duration::from_secs(7)).await,
                _ => {}
            }
            let response = self.source.read(admitted, operation, lease).await?;
            if !page && self.mode == "before-page" && head == Some(3) {
                self.authority
                    .change(
                        self.target.get().unwrap().0.clone(),
                        None,
                        false,
                        self.initiator.clone(),
                        uuid(),
                    )
                    .await
                    .unwrap();
            }
            if page {
                let count = self.pages.fetch_add(1, Ordering::SeqCst) + 1;
                if self.mode == "epoch" && count == 1 {
                    self.authority
                        .change(
                            self.target.get().unwrap().0.clone(),
                            None,
                            false,
                            self.initiator.clone(),
                            uuid(),
                        )
                        .await
                        .unwrap();
                }
                if self.mode == "append" && count == 1 {
                    let conversation = self.target.get().unwrap().1.clone();
                    let lease = self.storage.open(conversation.clone()).await.unwrap();
                    let mut snapshot = lease
                        .load()
                        .await
                        .unwrap()
                        .into_published(&conversation)
                        .unwrap()
                        .0
                        .unwrap();
                    let before = snapshot.provider_context.clone();
                    snapshot.provider_context = ProviderContext::Recorded(
                        ExecutionSessionId::new("later-source-context").unwrap(),
                    );
                    let after = snapshot.provider_context.clone();
                    lease
                        .save_changes(
                            lease.load().await.unwrap().binding().clone(),
                            snapshot,
                            vec![SessionSaveUnit::new(vec![SessionChange::ProviderContext {
                                before,
                                after,
                            }])
                            .unwrap()],
                        )
                        .await
                        .unwrap();
                    drop(lease);
                }
                if self.mode == "disconnect" && count == 2 {
                    std::process::exit(0);
                }
            }
            Ok(response)
        })
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gateway_child() {
    let Ok(root) = std::env::var("NESSA_ONLINE_GATEWAY") else {
        return;
    };
    let root = Path::new(&root);
    nessa_local_storage::create_directory(root).unwrap();
    let auth = Arc::new(LocalCredentialStore::open(root, "credentials.json").unwrap());
    // "regrant": the receiver policy revision changes, which moves every
    // active receiver to a new access epoch; the device's next status reports
    // it, and the saved cache scope no longer matches.
    let policy = match std::env::var("NESSA_ONLINE_GATE").as_deref() {
        Ok("regrant") => "regrant-policy".to_owned(),
        _ => CedarPolicyEvaluator::profile_digest(),
    };
    let receivers = Arc::new(
        LocalReceiverAuthority::open(
            &root.join("receivers.sqlite3"),
            &policy,
            Arc::new(SystemClock),
        )
        .unwrap(),
    );
    let setup_path = root.join("setup.json");
    let first_start = !setup_path.exists();
    let (gateway, organization, owner, owner_evidence) = if first_start {
        let gateway = uuid();
        let organization = uuid();
        let owner = uuid();
        let grant = |action: &str| CredentialGrantDto {
            action: action.into(),
            resource: ResourceDto {
                organization_id: organization.clone(),
                id: gateway.clone(),
            },
        };
        let boot = auth
            .bootstrap(BootstrapRequest {
                gateway_id: gateway.clone(),
                organization: OrganizationInputDto {
                    id: organization.clone(),
                },
                principal: PrincipalInputDto {
                    id: owner.clone(),
                    kind: PrincipalKindDto::Human,
                },
                membership: MembershipInputDto {
                    id: uuid(),
                    principal_id: owner.clone(),
                    organization_id: organization.clone(),
                    role: MembershipRoleDto::Admin,
                    state: MembershipStateDto::Active,
                },
                credential_id: uuid(),
                issued_at: SystemClock.unix_seconds(),
                expires_at: None,
                grants: vec![
                    grant("server.read"),
                    grant("conversation.read"),
                    grant("conversation.write"),
                    grant("credential.manage"),
                ],
            })
            .unwrap();
        (gateway, organization, owner, Some(boot.evidence))
    } else {
        let setup = serde_json::from_slice::<Setup>(&std::fs::read(&setup_path).unwrap()).unwrap();
        (setup.gateway, setup.organization, setup.owner, None)
    };
    // The real native composition: key restored or first published, then the
    // listener, on the address the first start chose and every restart keeps.
    let address: SocketAddr = match std::fs::read_to_string(root.join("native-address")) {
        Ok(address) => address.parse().unwrap(),
        Err(_) => "127.0.0.1:0".parse().unwrap(),
    };
    let (prepared, commands) = prepare(
        &NativeConfig {
            listen_address: address,
        },
        NativeInputs {
            namespace: root.to_path_buf(),
            registry: auth.clone(),
            policy: Arc::new(CedarPolicyEvaluator::new().unwrap()),
            receivers: receivers.clone(),
            clock: Arc::new(SystemClock),
            gateway: Resource::new(
                OrganizationId::new(organization.clone()).unwrap(),
                ResourceId::new(gateway.clone()).unwrap(),
            ),
        },
    )
    .await
    .unwrap();
    let metadata = Arc::new(LocalConversationStore::open(&root.join("metadata.sqlite3")).unwrap());
    let storage = Arc::new(RecordStorage::new(root.join("source")).unwrap());
    storage.initialize().await.unwrap();
    let record = Arc::new(NessaRecordReadSource::new(
        storage.clone(),
        Id::new(&gateway).unwrap(),
        Handle::current(),
    ));
    let mode = std::env::var("NESSA_ONLINE_GATE").unwrap_or_default();
    // Gated reads need the receiver, known after the first start pairs.
    let receiver_slot = Arc::new(OnceLock::new());
    let heads = Arc::new(HeadHold::default());
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
    .with_passive_read(receivers.clone(), metadata.clone())
    // The same producers `composition::local_auth` composes, over the same
    // storage and metadata owners the reads use.
    .with_change_watches(
        Arc::new(NessaRecordWatches::new(storage.clone())),
        metadata.clone(),
    )
    .with_record_source(Arc::new(GatedRead {
        source: record.clone(),
        storage: storage.clone(),
        target: receiver_slot.clone(),
        authority: receivers.clone(),
        initiator: PrincipalId::new(owner.clone()).unwrap(),
        mode: mode.clone(),
        pages: AtomicUsize::new(0),
        heads: heads.clone(),
    }))
    .with_catalogue_source(Arc::new(NessaCatalogueReadSource::new(
        metadata.clone(),
        Id::new(&gateway).unwrap(),
    )));
    let bound = bind(prepared, RuntimeDependencies::default().clock, state)
        .await
        .unwrap();
    let native = bound.local_address();
    if first_start {
        private_write(&root.join("native-address"), native.to_string().as_bytes());
    }
    let (failure, _failed) = tokio::sync::watch::channel(None);
    let _running = start(bound, failure);
    let setup = match owner_evidence {
        // First start: pair this test's device through the real listener.
        Some(evidence) => {
            let session = AuthenticateSession {
                verifier: auth.as_ref(),
                access: auth.as_ref(),
                clock: &SystemClock,
            }
            .execute(&evidence, &AudienceId::new(gateway.clone()).unwrap())
            .await
            .unwrap();
            let (receiver, epoch, credential) =
                pair_device(&commands, &session, root, native, "device").await;
            // Live mode follows two devices on this one gateway (L8); only
            // it pays for the second pairing.
            let second_receiver = if mode == "live" {
                let (receiver, _, _) =
                    pair_device(&commands, &session, root, native, "device-b").await;
                private_write(
                    &root.join("profile-b.json"),
                    &serde_json::to_vec(&json!({"stateRoot":root,"stateDirectory":"device-b",
                        "cache":root.join("device-b-cache.sqlite3"),
                        "gatewayAddress":native.to_string()}))
                    .unwrap(),
                );
                Some(receiver)
            } else {
                None
            };
            let setup = Setup {
                gateway: gateway.clone(),
                organization: organization.clone(),
                owner: owner.clone(),
                credential,
                receiver,
                epoch,
                second_receiver,
                conversation: uuid(),
                empty: uuid(),
            };
            private_write(&setup_path, &serde_json::to_vec(&setup).unwrap());
            private_write(
                &root.join("profile.json"),
                &serde_json::to_vec(&json!({"stateRoot":root,"stateDirectory":"device",
                    "cache":root.join("device-cache.sqlite3"),
                    "gatewayAddress":native.to_string()}))
                .unwrap(),
            );
            setup
        }
        None => serde_json::from_slice::<Setup>(&std::fs::read(&setup_path).unwrap()).unwrap(),
    };
    receiver_slot
        .set((
            setup.receiver.clone(),
            SessionId::new(setup.conversation.clone()).unwrap(),
        ))
        .unwrap();
    if mode == "revoke" {
        // The owner revokes the device's credential: its enrollment ends
        // Terminal(CredentialRevoked) and its next pinned status says so.
        auth.revoke_sync(RevokeCredentialRequest {
            request_id: uuid(),
            issuer_principal_id: setup.owner.clone(),
            credential_id: setup.credential.clone(),
            revoked_at: SystemClock.unix_seconds(),
        })
        .unwrap();
    }
    for (target, large) in [(&setup.conversation, true), (&setup.empty, false)] {
        let id = ConversationId::new(target).unwrap();
        if metadata.load(&id).await.unwrap().is_none() {
            metadata
                .create(
                    Conversation::new(
                        id,
                        OrganizationId::new(setup.organization.clone()).unwrap(),
                        PrincipalId::new(setup.owner.clone()).unwrap(),
                        "setup".into(),
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
            let session = SessionId::new(target).unwrap();
            let lease = storage.open(session.clone()).await.unwrap();
            if large {
                let provider = ProviderIdentity::new("provider", "model", "workspace").unwrap();
                let mut context = ProviderContext::Absent;
                let mut changes = vec![SessionChange::Opened {
                    id: session.clone(),
                    provider: provider.clone(),
                    context: context.clone(),
                }];
                for index in 0..300 {
                    let next = ProviderContext::Recorded(
                        ExecutionSessionId::new(format!("{index:05}{}", "x".repeat(240))).unwrap(),
                    );
                    changes.push(SessionChange::ProviderContext {
                        before: context,
                        after: next.clone(),
                    });
                    context = next;
                }
                lease
                    .save_changes(
                        lease.load().await.unwrap().binding().clone(),
                        SessionSnapshot {
                            id: session,
                            provider,
                            provider_context: context,
                            invocations: vec![],
                            queue_history: vec![],
                        },
                        vec![SessionSaveUnit::new(changes).unwrap()],
                    )
                    .await
                    .unwrap();
            }
            drop(lease);
        }
    }
    if mode == "live" {
        tokio::spawn(live_control(
            storage.clone(),
            SessionId::new(setup.conversation.clone()).unwrap(),
            receivers.clone(),
            [
                setup.receiver.clone(),
                setup.second_receiver.clone().unwrap(),
            ],
            PrincipalId::new(setup.owner.clone()).unwrap(),
            heads,
        ));
    }
    println!("ONLINE_READY");
    io::stdout().flush().unwrap();
    // Serve until the test kills this process.
    std::future::pending::<()>().await;
}

/// Live mode's control lines, each answered with `DONE <line>` once its effect
/// is durable: `commit` appends one provider-context save to the conversation
/// through this process's own storage owner, so its committed-change watch
/// publishes; `revoke` removes the first device's receiver binding, as an
/// owner would, and `revoke b` the second device's. `hold` makes the next
/// `watch`'s recheck pass wait at its head read until `release`.
async fn live_control(
    storage: Arc<RecordStorage>,
    conversation: SessionId,
    receivers: Arc<LocalReceiverAuthority>,
    [receiver, second]: [String; 2],
    owner: PrincipalId,
    heads: Arc<HeadHold>,
) {
    let mut lines = BufReader::new(stdin()).lines();
    let mut commits = 0_u64;
    while let Ok(Some(line)) = lines.next_line().await {
        match line.as_str() {
            "commit" => {
                commits += 1;
                let lease = storage.open(conversation.clone()).await.unwrap();
                let loaded = lease.load().await.unwrap();
                let binding = loaded.binding().clone();
                let mut snapshot = loaded.into_published(&conversation).unwrap().0.unwrap();
                let before = snapshot.provider_context.clone();
                snapshot.provider_context = ProviderContext::Recorded(
                    ExecutionSessionId::new(format!("live-commit-{commits}")).unwrap(),
                );
                let after = snapshot.provider_context.clone();
                lease
                    .save_changes(
                        binding,
                        snapshot,
                        vec![SessionSaveUnit::new(vec![SessionChange::ProviderContext {
                            before,
                            after,
                        }])
                        .unwrap()],
                    )
                    .await
                    .unwrap();
                drop(lease);
            }
            "revoke" | "revoke b" => {
                let target = if line == "revoke" { &receiver } else { &second };
                receivers
                    .change(target.clone(), None, false, owner.clone(), uuid())
                    .await
                    .unwrap();
            }
            "hold" => heads.hold_recheck(),
            "release" => heads.release.notify_one(),
            other => panic!("unknown live control {other}"),
        }
        println!("DONE {line}");
        io::stdout().flush().unwrap();
    }
}

/// Pair one device through the real listener, its private state under
/// `directory`: create a code, enroll the device's key with it, approve the
/// claimed key and read Active. Returns its receiver, epoch and credential.
async fn pair_device(
    commands: &PairingOwnerCommands,
    session: &AuthenticatedSession,
    root: &Path,
    native: SocketAddr,
    directory: &str,
) -> (String, u64, String) {
    let created = commands.create(session).await.unwrap();
    let code = ManualCode::parse(created.code().expose_bytes()).unwrap();
    nessa_local_storage::create_directory_beneath(root, Path::new(directory)).unwrap();
    let device = Arc::new(FilePairingState::open(root, Path::new(directory)).unwrap());
    let client = NativeEnrollmentClient::new(device.clone(), RuntimeDependencies::default().clock);
    let claimed = client
        .enroll(TcpStream::connect(native).unwrap(), code, OsEntropy)
        .await
        .unwrap();
    assert!(matches!(claimed, NativePairingStatus::Claimed(_)));
    let id = created.record().id();
    let record = commands.status(session, id).await.unwrap();
    let (_, key) = record.claim_binding().unwrap();
    let approved = commands.approve(session, id, key).await.unwrap().record;
    let (receiver, epoch) = approved.receiver_binding().unwrap();
    let credential = approved.credential().unwrap().as_str().to_owned();
    let active = client
        .status(TcpStream::connect(native).unwrap(), None)
        .await
        .unwrap();
    assert!(matches!(active, NativePairingStatus::Active { .. }));
    client.shutdown().await;
    (receiver.as_str().to_owned(), epoch, credential)
}
