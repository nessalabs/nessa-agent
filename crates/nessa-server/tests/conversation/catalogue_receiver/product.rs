// Real product sockets connect a gateway process to separately owned receiver
// processes. Receivers receive only port, credential, and their cache path.
use super::*;
use nessa_protocol::agents::AgentId;
use crate::catalogue_receiver_store::SqliteReceiver;
use crate::conversation::application::{CatalogueHead, CataloguePage, CataloguePageRequest, CatalogueValue, ConversationCatalogue, ConversationDependencies, ConversationFuture, ConversationLimits, ConversationRepository, ConversationService, ConversationSummaries, ReceiverAuthority, ReceiverBinding};
use nessa_protocol::conversation::read_scope::ReadRefusal;
use crate::conversation::domain::{Conversation, ConversationDeletion};
use nessa_protocol::conversation::domain::{ConversationApprovalMode, ConversationId, ConversationModelId, ConversationSummary};
use crate::conversation::infrastructure::{LocalConversationStore, NessaCatalogueReadSource};
use nessa_protocol::product::catalogue_read as catalogue_wire;
use nessa_protocol::product::generated::{
    CatalogueDescriptor, ConversationCatalogueHeadResult, ConversationCatalogueManifestResult,
    ConversationCatalogueResolveResult,
};
use nessa_protocol::product::passive_read as shared_wire;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use conversation_support::{
    AcceptingCreationAudit, AcceptingDeletionAudit, AcceptingModeAudit, Provider, ProviderFactory,
    RecordingFileLinkAudit, TestClock,
};
use nessa_local_database::rusqlite::Connection;
use nessa_sdk::application::agent_execution::executions::ExecutionUpdate;
use nessa_sdk::application::agent_execution::providers::{
    ExecutionReport, ProviderExecutionReply, ProviderSessionState,
};
use nessa_sdk::domain::agent_execution::executions::ExecutionOutcome;
use nessa_sdk::infrastructure::session_storage::{InMemoryStorage, RuntimeMessageCommitClock};
use nessa_sync::replication::catalogue::{
    apply_next_page, begin_or_resume, validate_catalogue_pass, validate_manifest_request,
    validate_resolved, CatalogueError, CataloguePass, CatalogueSource, CatalogueSourceError,
    CatalogueStore, CatalogueValidationError, EntryKey, ManifestEntry, ManifestPage,
    ManifestRequest, ResolvedEntry, MAX_CATALOGUE_ENTRIES,
};
use nessa_sync::replication::domain::{Id, Scope};
use nessa_sync::replication::infrastructure::MemoryAuthorizer;
use serde_json::Value;
use std::collections::HashMap;
use std::future::Future;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::atomic::AtomicUsize;
use std::time::Instant;
use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

const GATEWAY_CHILD: &str = "product::socket::tests::catalogue_receiver::gateway_child";
const RECEIVER_CHILD: &str = "product::socket::tests::catalogue_receiver::receiver_child";
const SEED: usize = 620;
fn sid(value: &str) -> Id {
    Id::new(value).unwrap()
}
fn cid(owner: &str, index: usize) -> ConversationId {
    ConversationId::new(&format!(
        "00000000-0000-4000-8000-{:012x}",
        (if owner == "alice" { 10000 } else { 20000 }) + index
    ))
    .unwrap()
}
fn organization() -> OrganizationId {
    OrganizationId::new("organization").unwrap()
}
fn conversation(id: ConversationId, owner: &str) -> Conversation {
    Conversation::new(
        id,
        organization(),
        PrincipalId::new(owner).unwrap(),
        "fixture".into(),
        "seed".into(),
        1,
        AgentId::Claude,
        ConversationModelId::new("test").unwrap(),
        ConversationApprovalMode::Ask,
    )
    .unwrap()
}

struct Owners {
    snapshots: Mutex<HashMap<CredentialId, AccessSnapshot>>,
    bindings: Mutex<HashMap<CredentialId, ReceiverBinding>>,
}
impl Owners {
    fn new() -> Self {
        let mut snapshots = HashMap::new();
        let mut bindings = HashMap::new();
        for owner in ["alice", "bob"] {
            let credential = CredentialId::new(format!("{owner}-phone")).unwrap();
            let principal = PrincipalId::new(owner).unwrap();
            let mut current = snapshot(MembershipRole::Member, MembershipStatus::Active);
            current.credential = Credential::new(
                credential.clone(),
                principal.clone(),
                organization(),
                AudienceId::new("gateway").unwrap(),
                100,
                200,
                ["conversation.read", "conversation.write", "server.read"]
                    .into_iter()
                    .map(|action| {
                        Grant::new(
                            Action::new(action).unwrap(),
                            Resource::new(
                                organization(),
                                ResourceId::new("gateway-resource").unwrap(),
                            ),
                        )
                    })
                    .collect(),
            )
            .unwrap();
            current.membership = Membership::new(
                MembershipId::new(format!("{owner}-member")).unwrap(),
                principal.clone(),
                organization(),
                MembershipRole::Member,
                MembershipStatus::Active,
            );
            snapshots.insert(credential.clone(), current);
            bindings.insert(
                credential.clone(),
                ReceiverBinding {
                    receiver_id: format!("{owner}-receiver"),
                    credential_id: credential,
                    organization_id: organization(),
                    owner_id: principal,
                    access_epoch: 7,
                    active: true,
                },
            );
        }
        Self {
            snapshots: Mutex::new(snapshots),
            bindings: Mutex::new(bindings),
        }
    }
}
impl CredentialVerifier for Owners {
    fn verify<'a>(
        &'a self,
        evidence: &'a CredentialEvidence,
        _: &'a AudienceId,
    ) -> PortFuture<'a, VerifiedCredential> {
        Box::pin(async move {
            let value = std::str::from_utf8(evidence.expose_bytes())
                .map_err(|_| AccessError::InvalidCredential)?;
            let credential_id =
                CredentialId::new(value).map_err(|_| AccessError::InvalidCredential)?;
            if !self.snapshots.lock().unwrap().contains_key(&credential_id) {
                return Err(AccessError::InvalidCredential);
            }
            Ok(VerifiedCredential {
                credential_id,
                expires_at: Some(200),
            })
        })
    }
}
impl AccessReader for Owners {
    fn read<'a>(&'a self, id: &'a CredentialId) -> PortFuture<'a, AccessSnapshot> {
        Box::pin(async move {
            self.snapshots
                .lock()
                .unwrap()
                .get(id)
                .cloned()
                .ok_or(AccessError::InvalidCredential)
        })
    }
}
impl ReceiverAuthority for Owners {
    fn resolve<'a>(
        &'a self,
        id: &'a CredentialId,
    ) -> Pin<Box<dyn Future<Output = Result<Option<ReceiverBinding>, ReadRefusal>> + Send + 'a>>
    {
        Box::pin(async move { Ok(self.bindings.lock().unwrap().get(id).cloned()) })
    }
}
struct Counts {
    head: AtomicUsize,
    page: AtomicUsize,
    page_done: AtomicUsize,
    resolve: AtomicUsize,
    hold: AtomicBool,
    release: Semaphore,
}
impl Default for Counts {
    fn default() -> Self {
        Self {
            head: AtomicUsize::new(0),
            page: AtomicUsize::new(0),
            page_done: AtomicUsize::new(0),
            resolve: AtomicUsize::new(0),
            hold: AtomicBool::new(false),
            release: Semaphore::new(0),
        }
    }
}
struct CountingCatalogue {
    store: Arc<LocalConversationStore>,
    counts: Arc<Counts>,
}
impl ConversationCatalogue for CountingCatalogue {
    fn head(
        &self,
        organization: &OrganizationId,
        owner: &PrincipalId,
    ) -> ConversationFuture<'_, CatalogueHead> {
        self.counts.head.fetch_add(1, Ordering::SeqCst);
        let organization = organization.clone();
        let owner = owner.clone();
        Box::pin(async move {
            if self.counts.hold.load(Ordering::SeqCst) {
                let permit = self.counts.release.acquire().await.unwrap();
                permit.forget();
            }
            self.store.head(&organization, &owner).await
        })
    }
    fn page(&self, request: CataloguePageRequest) -> ConversationFuture<'_, CataloguePage> {
        self.counts.page.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            let result = self.store.page(request).await;
            self.counts.page_done.fetch_add(1, Ordering::SeqCst);
            result
        })
    }
    fn resolve(
        &self,
        organization: &OrganizationId,
        owner: &PrincipalId,
        incarnation: &str,
        id: &ConversationId,
    ) -> ConversationFuture<'_, Option<CatalogueValue>> {
        self.counts.resolve.fetch_add(1, Ordering::SeqCst);
        self.store.resolve(organization, owner, incarnation, id)
    }
}

#[tokio::test]
async fn gateway_child() {
    let Ok(root) = std::env::var("NESSA_297_GATEWAY_ROOT") else {
        return;
    };
    let root = Path::new(&root);
    nessa_local_storage::create_directory(root).unwrap();
    let store = Arc::new(LocalConversationStore::open(&root.join("metadata.sqlite3")).unwrap());
    for owner in ["alice", "bob"] {
        for index in 0..std::env::var("NESSA_297_SEED_COUNT")
            .ok()
            .map_or(SEED, |value| value.parse().unwrap())
        {
            let id = cid(owner, index);
            if ConversationRepository::load(store.as_ref(), &id)
                .await
                .unwrap()
                .is_none()
            {
                store.create(conversation(id.clone(), owner)).await.unwrap();
                if index % 10 == 0 {
                    store
                        .record(&id, ConversationSummary::new(None, None, 1, true).unwrap())
                        .await
                        .unwrap();
                }
            }
        }
    }
    let owners = Arc::new(Owners::new());
    let counts = Arc::new(Counts::default());
    let catalogue = Arc::new(CountingCatalogue {
        store: store.clone(),
        counts: counts.clone(),
    });
    let source = Arc::new(NessaCatalogueReadSource::new(
        catalogue,
        sid("gateway-resource"),
    ));
    let provider = Arc::new(ProviderFactory::default());
    let (execution_release, execution_gate) = tokio::sync::oneshot::channel();
    *provider.execution_gate.lock().unwrap() = Some(execution_gate);
    let service = ConversationService::new(
        ConversationDependencies {
            agents: conversation_support::only(Arc::new(Provider::new(provider.clone()))),
            storage: Arc::new(InMemoryStorage::new()),
            metadata: store.clone(),
            summaries: store.clone(),
            listing: store.clone(),
            mode_audit: Arc::new(AcceptingModeAudit),
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            deletion_audit: Arc::new(AcceptingDeletionAudit),
            provider_sessions: conversation_support::claude_erasers(),
            deletion_budgets: conversation_support::DELETION_BUDGETS,
            message_commit_clock: Arc::new(RuntimeMessageCommitClock::new()),
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    let (mut state, _) = fixture(MembershipRole::Member);
    state.verifier = owners.clone();
    state.access = owners.clone();
    state = state
        .with_passive_read(owners.clone(), store.clone())
        .with_catalogue_source(source)
        .with_conversations(Arc::new(service));
    let capacity = state.record_reads.clone();
    let (commands, mut received) = tokio::sync::mpsc::channel::<Value>(8);
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            commands
                .blocking_send(serde_json::from_str(&line).unwrap())
                .unwrap();
        }
    });
    let mutate = store.clone();
    // Confirmed provider cleanup must unblock execute (providers/ports.rs).
    // Keep this invocation live until close really runs; passive worker gates
    // are independent and remain retained throughout the control effect.
    let execution_released = Arc::new(AtomicUsize::new(0));
    let released = execution_released.clone();
    let closing_provider = provider.clone();
    tokio::spawn(async move {
        closing_provider.close_finished.notified().await;
        closing_provider
            .execution_updates
            .lock()
            .unwrap()
            .push(ExecutionUpdate::Finished(ExecutionOutcome::Cancelled));
        *closing_provider.execution_reply.lock().unwrap() =
            Some(ProviderExecutionReply::Finished(ExecutionReport::new(
                Some(Ok(ExecutionOutcome::Cancelled)),
                None,
                ProviderSessionState::Usable,
            )));
        released.store(1, Ordering::SeqCst);
        let _ = execution_release.send(());
    });
    let metadata_path = root.join("metadata.sqlite3");
    tokio::spawn(async move {
        let mut tick = 0usize;
        while let Some(command) = received.recv().await {
            let owner = command["owner"].as_str().unwrap_or("alice");
            let result = match command["action"].as_str().unwrap() {
                "stats" => {
                    json!({"head":counts.head.load(Ordering::SeqCst),"page":counts.page.load(Ordering::SeqCst),"pageDone":counts.page_done.load(Ordering::SeqCst),"resolve":counts.resolve.load(Ordering::SeqCst),"permits":capacity.available_permits(),"closes":provider.close_calls.load(Ordering::SeqCst),"executionReleased":execution_released.load(Ordering::SeqCst)})
                }
                "block" => {
                    counts.hold.store(true, Ordering::SeqCst);
                    json!({"blocked":true})
                }
                "release" => {
                    counts.hold.store(false, Ordering::SeqCst);
                    counts.release.add_permits(8);
                    json!({"released":true})
                }
                "wait-execution" => {
                    provider.execution_started.notified().await;
                    json!({"executions":provider.executions.lock().unwrap().len()})
                }
                "tick" => {
                    tick += 1;
                    mutate
                        .create(conversation(
                            ConversationId::new(&Uuid::new_v4().to_string()).unwrap(),
                            owner,
                        ))
                        .await
                        .unwrap();
                    let id = cid(owner, 1);
                    mutate
                        .record(
                            &id,
                            ConversationSummary::after_message(
                                None,
                                &format!("tick-{tick}"),
                                None,
                                2,
                            ),
                        )
                        .await
                        .unwrap();
                    json!({"tick":tick})
                }
                "delete" => {
                    let id = ConversationId::new(command["id"].as_str().unwrap()).unwrap();
                    mutate
                        .record_deletion(
                            &id,
                            ConversationDeletion::new(
                                organization(),
                                PrincipalId::new(owner).unwrap(),
                                "fixture".into(),
                                "delete".into(),
                                2,
                            )
                            .unwrap(),
                        )
                        .await
                        .unwrap();
                    json!({"deleted":id.to_string()})
                }
                "corrupt" => {
                    let connection = Connection::open(&metadata_path).unwrap();
                    connection
                        .execute(
                            "UPDATE conversations SET model='' WHERE id=?1",
                            [cid(owner, 0).to_string()],
                        )
                        .unwrap();
                    json!({"corrupt":owner})
                }
                "deny" | "allow" => {
                    let read = command["action"] == "allow";
                    let principal = PrincipalId::new(owner).unwrap();
                    let credential = CredentialId::new(format!("{owner}-phone")).unwrap();
                    let grants = if read {
                        vec!["conversation.read", "conversation.write", "server.read"]
                    } else {
                        vec!["conversation.write", "server.read"]
                    };
                    owners
                        .snapshots
                        .lock()
                        .unwrap()
                        .get_mut(&credential)
                        .unwrap()
                        .credential = Credential::new(
                        credential.clone(),
                        principal,
                        organization(),
                        AudienceId::new("gateway").unwrap(),
                        100,
                        200,
                        grants
                            .into_iter()
                            .map(|action| {
                                Grant::new(
                                    Action::new(action).unwrap(),
                                    Resource::new(
                                        organization(),
                                        ResourceId::new("gateway-resource").unwrap(),
                                    ),
                                )
                            })
                            .collect(),
                    )
                    .unwrap();
                    json!({"read":read})
                }
                "epoch" => {
                    owners
                        .bindings
                        .lock()
                        .unwrap()
                        .get_mut(&CredentialId::new(format!("{owner}-phone")).unwrap())
                        .unwrap()
                        .access_epoch = 8;
                    json!({"epoch":8})
                }
                "wrong-owner" => {
                    owners
                        .bindings
                        .lock()
                        .unwrap()
                        .get_mut(&CredentialId::new(format!("{owner}-phone")).unwrap())
                        .unwrap()
                        .owner_id = PrincipalId::new("foreign").unwrap();
                    json!({"owner":"foreign"})
                }
                "restore-owner" => {
                    owners
                        .bindings
                        .lock()
                        .unwrap()
                        .get_mut(&CredentialId::new(format!("{owner}-phone")).unwrap())
                        .unwrap()
                        .owner_id = PrincipalId::new(owner).unwrap();
                    json!({"owner":owner})
                }
                _ => panic!("unknown fixture instruction"),
            };
            println!("NESSA_297_ACK {result}");
            std::io::stdout().flush().unwrap();
        }
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    println!("NESSA_297_READY {}", listener.local_addr().unwrap().port());
    std::io::stdout().flush().unwrap();
    axum::serve(listener, crate::server::entrypoint::http::router(state))
        .await
        .unwrap();
}

#[derive(Clone, Copy)]
enum WireFault {
    RequestEcho,
    OldValue,
}

struct ProductSource {
    socket: WebSocket<MaybeTlsStream<TcpStream>>,
    next: u64,
    scope: Scope,
    access_epoch: String,
    descriptors: HashMap<Id, ManifestEntry>,
    events: bool,
    owner: String,
    cache_path: PathBuf,
    delayed: bool,
    fault: Option<WireFault>,
}
impl ProductSource {
    fn connect(port: u16, owner: &str, epoch: &str, cache_path: PathBuf, events: bool) -> Self {
        let (mut socket, _) =
            tungstenite::connect(format!("ws://127.0.0.1:{port}/session")).unwrap();
        let challenge: Value =
            serde_json::from_str(socket.read().unwrap().to_text().unwrap()).unwrap();
        socket.send(Message::Text(json!({"type":"req","id":"auth","method":"session.authenticate","params":{"minVersion":1,"maxVersion":1,"nonce":challenge["payload"]["nonce"],"credential":format!("{owner}-phone"),"client":{"id":"catalogue-receiver"}}}).to_string().into())).unwrap();
        let ready: Value = serde_json::from_str(socket.read().unwrap().to_text().unwrap()).unwrap();
        assert_eq!(ready["ok"], true, "{ready}");
        let placeholder = Scope::new(
            sid("receiver"),
            sid("origin"),
            sid("stream"),
            sid("incarnation"),
            sid("schema"),
            sid(epoch),
        );
        let mut source = Self {
            socket,
            next: 0,
            scope: placeholder,
            access_epoch: epoch.into(),
            descriptors: HashMap::new(),
            events,
            owner: owner.into(),
            cache_path,
            delayed: false,
            fault: None,
        };
        let reply = source.call(
            "conversation.catalogueHead",
            json!({"receiverId":format!("{owner}-receiver"),"accessEpoch":epoch}),
        );
        assert_eq!(reply["ok"], true, "{reply}");
        let head: ConversationCatalogueHeadResult =
            serde_json::from_value(reply["payload"].clone()).unwrap();
        source.scope = shared_wire::decode_scope(&head.scope).unwrap();
        assert_eq!(
            source.scope.receiver().as_str(),
            format!("{owner}-receiver")
        );
        assert_eq!(
            source.scope.access_epoch().as_str(),
            format!("epoch-{epoch}")
        );
        source
    }
    fn read_json(&mut self) -> Value {
        loop {
            match self.socket.read().unwrap() {
                Message::Text(text) => {
                    assert!(text.len() <= MAX_RECORD_RESPONSE_BYTES);
                    return serde_json::from_str(&text).unwrap();
                }
                Message::Ping(bytes) => self.socket.send(Message::Pong(bytes)).unwrap(),
                value => panic!("unexpected frame {value:?}"),
            }
        }
    }
    fn call(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = self.next.to_string();
        self.socket
            .send(Message::Text(
                json!({"type":"req","id":id,"method":method,"params":params})
                    .to_string()
                    .into(),
            ))
            .unwrap();
        loop {
            let reply = self.read_json();
            if reply["id"] == id {
                return reply;
            }
        }
    }
    fn event(&self, action: &str, id: Option<&str>) {
        println!(
            "NESSA_297_EVENT {}",
            json!({"action":action,"owner":self.owner,"id":id})
        );
        std::io::stdout().flush().unwrap();
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).unwrap();
        assert_eq!(line.trim(), "continue");
    }
    fn read_resolved(
        &mut self,
        pass: &CataloguePass,
        descriptor: &ManifestEntry,
        max: usize,
    ) -> Result<ResolvedEntry, CatalogueSourceError> {
        validate_catalogue_pass(pass).map_err(|_| CatalogueSourceError::InvalidRequest)?;
        let reply=self.call("conversation.catalogueResolve",json!({"pass":catalogue_wire::wire_pass(pass),"descriptor":catalogue_wire::wire_descriptor(descriptor),"maxPayloadBytes":max,"accessEpoch":self.access_epoch}));
        if reply["ok"] != true {
            return Err(CatalogueSourceError::Unavailable);
        }
        let response: ConversationCatalogueResolveResult =
            serde_json::from_value(reply["payload"].clone()).unwrap();
        // The core return type omits wire-only echoes. This adapter owns their
        // correlation before discarding them; core validate_resolved owns values.
        assert_eq!(catalogue_wire::decode_pass(&response.pass).unwrap(), *pass);
        assert_eq!(
            catalogue_wire::decode_descriptor(&response.descriptor).unwrap(),
            *descriptor
        );
        let mut value = ResolvedEntry {
            manifest: catalogue_wire::decode_descriptor(&response.entry).unwrap(),
            payload: STANDARD.decode(response.payload).unwrap(),
        };
        if matches!(self.fault, Some(WireFault::OldValue)) {
            value.manifest.revision = 0;
        }
        Ok(value)
    }
}
impl CatalogueSource for ProductSource {
    fn head(&mut self, scope: &Scope) -> Result<u64, CatalogueSourceError> {
        let reply = self.call(
            "conversation.catalogueHead",
            json!({"receiverId":scope.receiver().as_str(),"accessEpoch":self.access_epoch}),
        );
        if reply["ok"] != true {
            return Err(CatalogueSourceError::Unavailable);
        }
        let result: ConversationCatalogueHeadResult =
            serde_json::from_value(reply["payload"].clone()).unwrap();
        assert_eq!(shared_wire::decode_scope(&result.scope).unwrap(), *scope);
        Ok(shared_wire::decimal_u64(&result.head).unwrap())
    }
    fn manifest(
        &mut self,
        request: &ManifestRequest,
    ) -> Result<ManifestPage, CatalogueSourceError> {
        validate_manifest_request(request, MAX_CATALOGUE_ENTRIES)
            .map_err(|_| CatalogueSourceError::InvalidRequest)?;
        let reply=self.call("conversation.catalogueManifest",json!({"request":{"pass":catalogue_wire::wire_pass(&request.pass),"maxEntries":request.max_entries},"accessEpoch":self.access_epoch}));
        if reply["ok"] != true {
            return Err(CatalogueSourceError::Unavailable);
        }
        let result: ConversationCatalogueManifestResult =
            serde_json::from_value(reply["payload"].clone()).unwrap();
        let mut page = ManifestPage {
            request: catalogue_wire::decode_manifest(&result.request).unwrap(),
            entries: result
                .entries
                .iter()
                .map(|entry| catalogue_wire::decode_descriptor(entry).unwrap())
                .collect(),
            has_more: result.has_more,
        };
        if matches!(self.fault, Some(WireFault::RequestEcho)) {
            page.request.pass.generation += 1;
        }
        self.descriptors = page
            .entries
            .iter()
            .map(|entry| (entry.key.id.clone(), entry.clone()))
            .collect();
        if self.events {
            if self.delayed && !self.cache_path.with_extension("delayed.json").exists() {
                let descriptor = page.entries.first().unwrap();
                let old = self
                    .read_resolved(&request.pass, descriptor, 65536)
                    .unwrap();
                std::fs::write(self.cache_path.with_extension("delayed.json"),json!({"descriptor":catalogue_wire::wire_descriptor(&old.manifest),"payload":STANDARD.encode(old.payload)}).to_string()).unwrap();
                self.event("delete", Some(descriptor.key.id.as_str()));
            }
            self.event("tick", None);
        }
        Ok(page)
    }
    fn resolve(
        &mut self,
        pass: &CataloguePass,
        id: &Id,
        max: usize,
    ) -> Result<ResolvedEntry, CatalogueSourceError> {
        let descriptor = self.descriptors.get(id).cloned().unwrap();
        self.read_resolved(pass, &descriptor, max)
    }
}

#[test]
fn receiver_child() {
    let Ok(port) = std::env::var("NESSA_297_RECEIVER_PORT") else {
        return;
    };
    let owner = std::env::var("NESSA_297_RECEIVER_OWNER").unwrap();
    let cache_path = PathBuf::from(std::env::var("NESSA_297_RECEIVER_CACHE").unwrap());
    let mode = std::env::var("NESSA_297_RECEIVER_MODE").unwrap();
    let epoch = if mode == "reset" { "8" } else { "7" };
    let mut source = ProductSource::connect(
        port.parse().unwrap(),
        &owner,
        epoch,
        cache_path.clone(),
        mode == "partial" || mode == "finite",
    );
    source.delayed = mode == "partial";
    let scope = source.scope.clone();
    let mut cache = SqliteReceiver::open(&cache_path);
    let mut authorization = MemoryAuthorizer::allowed(scope.clone());
    if mode == "reset" {
        let previous = cache.progress(&scope).unwrap().unwrap();
        let next = cache.reset(&scope, previous).unwrap();
        assert_eq!(next.completed, 0);
        assert!(cache
            .cached_revision(&scope, &sid(&cid(&owner, 0).to_string()))
            .unwrap()
            .is_some());
        assert_eq!(
            cache
                .cached_revision(&scope, &sid(&cid(&owner, 1).to_string()))
                .unwrap(),
            None
        );
        return;
    }
    let Some(mut pass) =
        begin_or_resume(&scope, &mut authorization, &mut source, &mut cache).unwrap()
    else {
        return;
    };
    if mode == "reject-evidence" {
        let before = cache.progress(&scope).unwrap();
        for (fault, error) in [
            (
                WireFault::RequestEcho,
                CatalogueValidationError::WrongRequest,
            ),
            (WireFault::OldValue, CatalogueValidationError::WrongPayload),
        ] {
            source.fault = Some(fault);
            assert_eq!(
                apply_next_page(
                    &pass,
                    37,
                    65536,
                    &mut authorization,
                    &mut source,
                    &mut cache
                ),
                Err(CatalogueError::Validation(error))
            );
            assert_eq!(cache.progress(&scope).unwrap(), before);
            assert_eq!(cache.count(), 0);
        }
        return;
    }
    let boundary = pass.boundary;
    if mode == "lost" {
        // Send the real request, wait through the gateway's source completion,
        // then drop the socket without reading or decoding its response.
        source.socket.send(Message::Text(json!({"type":"req","id":"lost","method":"conversation.catalogueManifest","params":{"request":{"pass":catalogue_wire::wire_pass(&pass),"maxEntries":37},"accessEpoch":epoch}}).to_string().into())).unwrap();
        source.event("wait-page", None);
        return;
    }
    let mut pages = 0;
    loop {
        let progress = apply_next_page(
            &pass,
            37,
            65536,
            &mut authorization,
            &mut source,
            &mut cache,
        )
        .unwrap();
        pages += 1;
        assert!(pages < 100, "finite pass must complete despite writes");
        if mode == "partial" {
            return;
        }
        match progress.active {
            Some(next) => {
                assert_eq!(next.boundary, boundary);
                pass = next
            }
            None => {
                assert_eq!(progress.completed, boundary);
                break;
            }
        }
    }
    if mode == "verify-delayed" {
        let value: Value = serde_json::from_slice(
            &std::fs::read(cache_path.with_extension("delayed.json")).unwrap(),
        )
        .unwrap();
        let descriptor: CatalogueDescriptor =
            serde_json::from_value(value["descriptor"].clone()).unwrap();
        let old = ResolvedEntry {
            manifest: catalogue_wire::decode_descriptor(&descriptor).unwrap(),
            payload: STANDARD.decode(value["payload"].as_str().unwrap()).unwrap(),
        };
        let current = cache
            .conn
            .query_row(
                "SELECT revision,deleted FROM entries WHERE id=?1",
                [old.manifest.key.id.as_str()],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, bool>(1)?)),
            )
            .unwrap();
        assert!(current.1);
        let retained = ManifestEntry {
            revision: current.0 as u64,
            deleted: true,
            ..old.manifest.clone()
        };
        assert!(validate_resolved(&retained, &old, 65536).is_err());
    }
    println!(
        "NESSA_297_DONE {}",
        json!({"pages":pages,"boundary":boundary,"count":cache.count()})
    );
}

struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
struct Gateway {
    child: ChildGuard,
    output: BufReader<ChildStdout>,
    port: u16,
}
impl Gateway {
    fn start(root: &Path) -> Self {
        Self::start_with_count(root, SEED)
    }
    fn start_with_count(root: &Path, count: usize) -> Self {
        let mut child = ChildGuard(
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", GATEWAY_CHILD, "--nocapture"])
                .env("NESSA_297_GATEWAY_ROOT", root)
                .env("NESSA_297_SEED_COUNT", count.to_string())
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap(),
        );
        let mut output = BufReader::new(child.0.stdout.take().unwrap());
        let mut line = String::new();
        loop {
            line.clear();
            assert_ne!(output.read_line(&mut line).unwrap(), 0);
            if let Some(port) = line.trim().strip_prefix("NESSA_297_READY ") {
                return Self {
                    child,
                    output,
                    port: port.parse().unwrap(),
                };
            }
        }
    }
    fn command(&mut self, value: Value) -> Value {
        writeln!(self.child.0.stdin.as_mut().unwrap(), "{value}").unwrap();
        let mut line = String::new();
        loop {
            line.clear();
            assert_ne!(self.output.read_line(&mut line).unwrap(), 0);
            if let Some(value) = line.trim().strip_prefix("NESSA_297_ACK ") {
                return serde_json::from_str(value).unwrap();
            }
        }
    }
    fn receiver(&mut self, cache: &Path, owner: &str, mode: &str) {
        let previous_done = self.command(json!({"action":"stats"}))["pageDone"]
            .as_u64()
            .unwrap();
        let mut child = ChildGuard(
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", RECEIVER_CHILD, "--nocapture"])
                .env("NESSA_297_RECEIVER_PORT", self.port.to_string())
                .env("NESSA_297_RECEIVER_OWNER", owner)
                .env("NESSA_297_RECEIVER_CACHE", cache)
                .env("NESSA_297_RECEIVER_MODE", mode)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let mut output = BufReader::new(child.0.stdout.take().unwrap());
        let mut line = String::new();
        loop {
            line.clear();
            if output.read_line(&mut line).unwrap() == 0 {
                break;
            }
            if let Some(value) = line.trim().strip_prefix("NESSA_297_EVENT ") {
                let event: Value = serde_json::from_str(value).unwrap();
                if event["action"] == "wait-page" {
                    let mut observed = false;
                    for _ in 0..1000 {
                        if self.command(json!({"action":"stats"}))["pageDone"]
                            .as_u64()
                            .unwrap()
                            > previous_done
                        {
                            observed = true;
                            break;
                        }
                    }
                    assert!(
                        observed,
                        "gateway completed the dropped reply's source read"
                    );
                } else {
                    self.command(event);
                }
                writeln!(child.0.stdin.as_mut().unwrap(), "continue").unwrap();
            }
        }
        let status = child.0.wait().unwrap();
        let mut error = String::new();
        Read::read_to_string(child.0.stderr.as_mut().unwrap(), &mut error).unwrap();
        assert!(status.success(), "receiver {owner} {mode}: {error}");
    }
}

#[test]
fn two_owned_receiver_processes_finish_fixed_pass_across_loss_restart_writes_deletion_and_reset() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("gateway");
    let left = directory.path().join("alice.sqlite3");
    let right = directory.path().join("bob.sqlite3");
    let mut gateway = Gateway::start(&root);
    gateway.receiver(
        &directory.path().join("rejected.sqlite3"),
        "alice",
        "reject-evidence",
    );
    gateway.receiver(&left, "alice", "lost");
    let cache = SqliteReceiver::open(&left);
    let initial = SqliteReceiver::snapshot(&cache.conn).unwrap().unwrap();
    assert_eq!(initial.completed, 0);
    assert!(initial.active.as_ref().unwrap().cursor.is_none());
    drop(cache);
    gateway.receiver(&left, "alice", "partial");
    assert_eq!(SqliteReceiver::open(&left).count(), 37);
    gateway.receiver(&right, "bob", "finite");
    assert_eq!(SqliteReceiver::open(&right).count(), SEED as i64);
    let before = SqliteReceiver::snapshot(&SqliteReceiver::open(&left).conn)
        .unwrap()
        .unwrap();
    let old_incarnation = before.scope.incarnation().clone();
    drop(gateway);
    let mut gateway = Gateway::start(&root);
    gateway.receiver(&left, "alice", "finite");
    let cache = SqliteReceiver::open(&left);
    let completed = SqliteReceiver::snapshot(&cache.conn).unwrap().unwrap();
    assert_eq!(completed.scope.incarnation(), &old_incarnation);
    assert_eq!(completed.completed, before.active.unwrap().boundary);
    assert_eq!(cache.count(), SEED as i64);
    drop(cache);
    gateway.receiver(&left, "alice", "verify-delayed");
    gateway.receiver(&right, "bob", "resume");
    let left_cache = SqliteReceiver::open(&left);
    let right_cache = SqliteReceiver::open(&right);
    assert!(left_cache.count() > SEED as i64);
    assert!(right_cache.count() > SEED as i64);
    assert_ne!(
        SqliteReceiver::snapshot(&left_cache.conn)
            .unwrap()
            .unwrap()
            .scope
            .stream(),
        SqliteReceiver::snapshot(&right_cache.conn)
            .unwrap()
            .unwrap()
            .scope
            .stream()
    );
    assert_eq!(
        left_cache
            .conn
            .query_row(
                "SELECT COUNT(*) FROM entries WHERE id=?1",
                [cid("bob", 0).to_string()],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    assert!(left_cache.payload(&cid("alice", 1))["summary"]["preview"]
        .as_str()
        .unwrap()
        .starts_with("tick-"));
    drop(left_cache);
    drop(right_cache);
    gateway.command(json!({"action":"epoch","owner":"alice"}));
    gateway.receiver(&left, "alice", "reset");
}

#[test]
fn product_admission_refusals_touch_no_catalogue_and_source_refusals_are_typed() {
    let directory = tempfile::tempdir().unwrap();
    let mut gateway = Gateway::start_with_count(&directory.path().join("gateway"), 2);
    let mut source = ProductSource::connect(
        gateway.port,
        "alice",
        "7",
        directory.path().join("cache"),
        false,
    );
    let scope = source.scope.clone();
    let head = source.head(&scope).unwrap();
    let pass = CataloguePass {
        scope: scope.clone(),
        completed: 0,
        boundary: head,
        cursor: None,
        generation: 1,
    };
    let stats = gateway.command(json!({"action":"stats"}));
    for (receiver, epoch, code) in [
        ("bob-receiver", "7", "wrong_receiver"),
        ("alice-receiver", "6", "stale_epoch"),
    ] {
        let reply = source.call(
            "conversation.catalogueHead",
            json!({"receiverId":receiver,"accessEpoch":epoch}),
        );
        assert_eq!(reply["error"]["code"], code, "{reply}");
        assert!(reply["payload"].is_null());
    }
    for field in ["origin", "stream", "schema"] {
        let mut wire = serde_json::to_value(catalogue_wire::wire_pass(&pass)).unwrap();
        wire["scope"][field] = json!("foreign");
        let reply = source.call(
            "conversation.catalogueManifest",
            json!({"request":{"pass":wire,"maxEntries":37},"accessEpoch":"7"}),
        );
        assert_eq!(
            reply["error"]["code"],
            if field == "stream" {
                "wrong_owner"
            } else {
                "identity_changed"
            },
            "{reply}"
        );
        assert!(reply["payload"].is_null());
    }
    let descriptor = ManifestEntry {
        key: EntryKey {
            creation: 1,
            id: sid(&cid("alice", 0).to_string()),
        },
        revision: head,
        deleted: false,
    };
    for (field, value) in [
        ("generation", json!("0")),
        ("boundary", json!("0")),
        (
            "cursor",
            json!({"creation":"0","id":descriptor.key.id.as_str()}),
        ),
        (
            "cursor",
            json!({"creation":(head+1).to_string(),"id":descriptor.key.id.as_str()}),
        ),
    ] {
        let mut invalid = serde_json::to_value(catalogue_wire::wire_pass(&pass)).unwrap();
        invalid[field] = value;
        for (method, params) in [
            (
                "conversation.catalogueManifest",
                json!({"request":{"pass":invalid,"maxEntries":37},"accessEpoch":"7"}),
            ),
            (
                "conversation.catalogueResolve",
                json!({"pass":invalid,"descriptor":catalogue_wire::wire_descriptor(&descriptor),"maxPayloadBytes":1,"accessEpoch":"7"}),
            ),
        ] {
            let reply = source.call(method, params);
            assert_eq!(reply["error"]["code"], "invalid_request", "{reply}");
            assert!(reply["payload"].is_null());
        }
    }
    for count in [0, MAX_CATALOGUE_ENTRIES + 1] {
        let reply = source.call("conversation.catalogueManifest", json!({"request":{"pass":catalogue_wire::wire_pass(&pass),"maxEntries":count},"accessEpoch":"7"}));
        assert_eq!(reply["error"]["code"], "invalid_request", "{reply}");
    }
    gateway.command(json!({"action":"wrong-owner","owner":"alice"}));
    let reply = source.call(
        "conversation.catalogueHead",
        json!({"receiverId":"alice-receiver","accessEpoch":"7"}),
    );
    assert_eq!(reply["error"]["code"], "wrong_owner");
    gateway.command(json!({"action":"restore-owner","owner":"alice"}));
    gateway.command(json!({"action":"deny","owner":"alice"}));
    let reply = source.call(
        "conversation.catalogueHead",
        json!({"receiverId":"alice-receiver","accessEpoch":"7"}),
    );
    assert_eq!(reply["error"]["code"], "forbidden");
    gateway.command(json!({"action":"allow","owner":"alice"}));
    assert_eq!(
        gateway.command(json!({"action":"stats"})),
        stats,
        "admission and nonphysical identity refusals precede metadata"
    );
    let page = source
        .manifest(&ManifestRequest {
            pass: pass.clone(),
            max_entries: 37,
        })
        .unwrap();
    let descriptor = page.entries.first().unwrap();
    let reply=source.call("conversation.catalogueResolve",json!({"pass":catalogue_wire::wire_pass(&pass),"descriptor":catalogue_wire::wire_descriptor(descriptor),"maxPayloadBytes":1,"accessEpoch":"7"}));
    assert_eq!(reply["error"]["code"], "oversized_entry");
    assert!(reply["payload"].is_null());
    let mut old = serde_json::to_value(catalogue_wire::wire_pass(&pass)).unwrap();
    old["scope"]["incarnation"] = json!("old");
    let reply = source.call(
        "conversation.catalogueManifest",
        json!({"request":{"pass":old,"maxEntries":37},"accessEpoch":"7"}),
    );
    assert_eq!(reply["error"]["code"], "identity_changed");
    assert!(reply["payload"].is_null());
    gateway.command(json!({"action":"corrupt","owner":"bob"}));
    assert!(
        source
            .manifest(&ManifestRequest {
                pass: pass.clone(),
                max_entries: 37
            })
            .is_ok(),
        "another owner's corruption does not affect an authorized page"
    );
    gateway.command(json!({"action":"corrupt","owner":"alice"}));
    let reply = source.call(
        "conversation.catalogueManifest",
        json!({"request":{"pass":catalogue_wire::wire_pass(&pass),"maxEntries":37},"accessEpoch":"7"}),
    );
    assert_eq!(reply["error"]["code"], "source_unavailable");
    assert!(reply["payload"].is_null());
}

#[test]
fn socket_close_stops_active_provider_while_catalogue_workers_are_full_and_a_caller_disconnects() {
    let directory = tempfile::tempdir().unwrap();
    let mut gateway = Gateway::start_with_count(&directory.path().join("gateway"), 2);
    let mut clients = (0..5)
        .map(|index| {
            ProductSource::connect(
                gateway.port,
                "alice",
                "7",
                directory.path().join(format!("unused-{index}")),
                false,
            )
        })
        .collect::<Vec<_>>();
    let reply=clients[0].call("conversation.send",json!({"conversationId":cid("alice",1).to_string(),"requestId":Uuid::new_v4().to_string(),"executionId":Uuid::new_v4().to_string(),"text":"held provider work","attachments":[],"files":[]}));
    assert_eq!(reply["ok"], true, "{reply}");
    gateway.command(json!({"action":"wait-execution"}));
    let before = gateway.command(json!({"action":"stats"}));
    assert_eq!(before["closes"], 0);
    assert_eq!(
        before["executionReleased"], 0,
        "provider invocation is still live"
    );
    gateway.command(json!({"action":"block"}));
    for (index, client) in clients.iter_mut().take(4).enumerate() {
        client.socket.send(Message::Text(json!({"type":"req","id":format!("held-{index}"),"method":"conversation.catalogueHead","params":{"receiverId":"alice-receiver","accessEpoch":"7"}}).to_string().into())).unwrap();
    }
    let expected = before["head"].as_u64().unwrap() + 4;
    let mut full = false;
    for _ in 0..1000 {
        let stats = gateway.command(json!({"action":"stats"}));
        if stats["head"].as_u64() == Some(expected) && stats["permits"] == 0 {
            full = true;
            break;
        }
    }
    assert!(
        full,
        "four admitted source workers reached the deterministic metadata gate"
    );
    let disconnected = clients.remove(3);
    drop(disconnected);
    assert_eq!(
        gateway.command(json!({"action":"stats"}))["permits"],
        0,
        "caller loss retains physical worker capacity"
    );
    let refusal = clients[3].call(
        "conversation.catalogueHead",
        json!({"receiverId":"alice-receiver","accessEpoch":"7"}),
    );
    assert_eq!(
        refusal["error"]["code"], "temporarily_unavailable",
        "{refusal}"
    );
    let started = Instant::now();
    let stopped = clients[0].call(
        "conversation.close",
        json!({"conversationId":cid("alice",1).to_string(),"requestId":Uuid::new_v4().to_string()}),
    );
    let elapsed = started.elapsed();
    assert_eq!(stopped["ok"], true, "{stopped}");
    assert!(
        elapsed < Duration::from_secs(5),
        "close control took {elapsed:?}"
    );
    let stats = gateway.command(json!({"action":"stats"}));
    assert_eq!(
        stats["closes"], 1,
        "the real SDK called provider cleanup despite read pressure"
    );
    assert_eq!(
        stats["executionReleased"], 1,
        "actual provider close unblocked execute"
    );
    assert_eq!(stats["permits"], 0);
    gateway.command(json!({"action":"release"}));
    let mut drained = false;
    for _ in 0..1000 {
        if gateway.command(json!({"action":"stats"}))["permits"] == 4 {
            drained = true;
            break;
        }
    }
    assert!(
        drained,
        "completed and disconnected source workers released all capacity"
    );
    eprintln!(
        "real /session close control elapsed_ms={} under four blocked catalogue workers",
        elapsed.as_millis()
    );
}
