//! Real loopback HTTP -> real Cedar -> durable command store -> real SDK runner.
//! Only the provider, clock and credential proof are local deterministic fixtures.
use super::{
    application::EventBinding,
    domain::EventId,
    entrypoint::{router, SigningKey, MAX_EVENT_BYTES},
};
use crate::{
    conversation::{application::*, infrastructure::LocalConversationStore},
    conversation_test_support::{
        only, AcceptingCreationAudit, AcceptingDeletionAudit, AcceptingModeAudit, Provider,
        ProviderFactory, RecordingFileLinkAudit, TestClock, DELETION_BUDGETS,
    },
};
use base64::{engine::general_purpose::STANDARD, Engine};
use hmac::{Hmac, Mac};
use nessa_auth::{
    adapters::cedar::CedarPolicyEvaluator,
    application::{
        ports::*,
        session::{AuthenticateSession, AuthenticatedSession},
    },
    domain::*,
};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::{
    application::agent_execution::{
        commands::{CreationFuture, CreationStorage, CreationStorageError, CreationStorageLease},
        sessions::{SessionStorage, StorageError},
    },
    infrastructure::session_storage::{RecordStorage, RuntimeMessageCommitClock},
};
use serde_json::{json, Value};
use sha2::Sha256;
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
};

const KEY: [u8; 32] = [42; 32];
struct Authority {
    snapshot: Mutex<AccessSnapshot>,
    now: AtomicU64,
    unavailable: AtomicBool,
    clock_reads: AtomicU64,
    panic_read: AtomicBool,
}
impl CredentialVerifier for Authority {
    fn verify<'a>(
        &'a self,
        proof: &'a CredentialEvidence,
        _: &'a AudienceId,
    ) -> PortFuture<'a, VerifiedCredential> {
        Box::pin(async move {
            if proof.expose_bytes() != b"local-event-test" {
                return Err(AccessError::InvalidCredential);
            }
            Ok(VerifiedCredential {
                credential_id: CredentialId::new("credential").unwrap(),
                expires_at: None,
            })
        })
    }
}
impl AccessReader for Authority {
    fn read<'a>(&'a self, _: &'a CredentialId) -> PortFuture<'a, AccessSnapshot> {
        Box::pin(async {
            assert!(
                !self.panic_read.load(Ordering::SeqCst),
                "local authority fixture panic"
            );
            if self.unavailable.load(Ordering::SeqCst) {
                Err(AccessError::Unavailable)
            } else {
                Ok(self.snapshot.lock().unwrap().clone())
            }
        })
    }
}
impl Clock for Authority {
    fn unix_milliseconds(&self) -> u64 {
        self.clock_reads.fetch_add(1, Ordering::SeqCst);
        self.now.load(Ordering::SeqCst)
    }
}
fn resource() -> Resource {
    Resource::new(
        OrganizationId::new("org").unwrap(),
        ResourceId::new("gateway").unwrap(),
    )
}
async fn authority() -> (Arc<Authority>, AuthenticatedSession) {
    let authority = Arc::new(Authority {
        snapshot: Mutex::new(AccessSnapshot {
            credential: Credential::new(
                CredentialId::new("credential").unwrap(),
                PrincipalId::new("person").unwrap(),
                OrganizationId::new("org").unwrap(),
                AudienceId::new("gateway").unwrap(),
                1,
                10_000,
                vec![Grant::new(
                    Action::new("conversation.write").unwrap(),
                    resource(),
                )],
            )
            .unwrap(),
            membership: Membership::new(
                MembershipId::new("membership").unwrap(),
                PrincipalId::new("person").unwrap(),
                OrganizationId::new("org").unwrap(),
                MembershipRole::Member,
                MembershipStatus::Active,
            ),
            revision: 1,
        }),
        now: AtomicU64::new(100_000),
        unavailable: AtomicBool::new(false),
        clock_reads: AtomicU64::new(0),
        panic_read: AtomicBool::new(false),
    });
    let session = AuthenticateSession {
        verifier: authority.as_ref(),
        access: authority.as_ref(),
        clock: authority.as_ref(),
    }
    .execute(
        &CredentialEvidence::new(b"local-event-test".to_vec()).unwrap(),
        &AudienceId::new("gateway").unwrap(),
    )
    .await
    .unwrap();
    (authority, session)
}
fn caller(action: &str) -> ConversationCaller {
    ConversationCaller {
        organization_id: OrganizationId::new("org").unwrap(),
        principal_id: PrincipalId::new("person").unwrap(),
        surface_id: "test".into(),
        action_id: action.into(),
    }
}
struct CommandGate {
    entered: AtomicU64,
    released: tokio::sync::watch::Sender<bool>,
}
struct Store {
    inner: Arc<RecordStorage>,
    fail: AtomicBool,
    fail_terminal: Arc<AtomicBool>,
    gate: Mutex<Option<Arc<CommandGate>>>,
}
impl CreationStorage for Store {
    fn open_creation(
        &self,
        principal: String,
        request: String,
        create: bool,
    ) -> CreationFuture<'_, Option<Arc<dyn CreationStorageLease>>> {
        Box::pin(async move {
            if self.fail.load(Ordering::SeqCst) {
                return Err(CreationStorageError::Storage(StorageError::Closed));
            }
            let gate = self.gate.lock().unwrap().clone();
            if let Some(gate) = gate {
                let mut released = gate.released.subscribe();
                gate.entered.fetch_add(1, Ordering::SeqCst);
                released.wait_for(|released| *released).await.unwrap();
            }
            Ok(self
                .inner
                .open_creation(principal, request, create)
                .await?
                .map(|inner| {
                    Arc::new(Lease {
                        inner,
                        fail_terminal: self.fail_terminal.clone(),
                    }) as Arc<dyn CreationStorageLease>
                }))
        })
    }
}
struct Lease {
    inner: Arc<dyn CreationStorageLease>,
    fail_terminal: Arc<AtomicBool>,
}
impl CreationStorageLease for Lease {
    fn load(
        &self,
        id: &str,
    ) -> CreationFuture<
        '_,
        Option<nessa_sdk::application::agent_execution::commands::CreationReceipt>,
    > {
        let id = id.to_owned();
        Box::pin(async move { self.inner.load(&id).await })
    }
    fn save(
        &self,
        receipt: nessa_sdk::application::agent_execution::commands::CreationReceipt,
    ) -> CreationFuture<'_, ()> {
        self.inner.save(receipt)
    }
    fn load_mutation(
        &self,
        id: &str,
    ) -> CreationFuture<
        '_,
        Option<nessa_sdk::application::agent_execution::commands::MutationReceipt>,
    > {
        let id = id.to_owned();
        Box::pin(async move { self.inner.load_mutation(&id).await })
    }
    fn save_mutation(
        &self,
        receipt: nessa_sdk::application::agent_execution::commands::MutationReceipt,
    ) -> CreationFuture<'_, ()> {
        Box::pin(async move {
            if receipt.outcome().is_some() && self.fail_terminal.load(Ordering::SeqCst) {
                return Err(CreationStorageError::Storage(StorageError::Closed));
            }
            self.inner.save_mutation(receipt).await
        })
    }
}
struct Fixture {
    service: ConversationService,
    provider: Arc<ProviderFactory>,
    records: Arc<RecordStorage>,
    commands: Arc<Store>,
    target: ConversationId,
    authority: Arc<Authority>,
    session: AuthenticatedSession,
}
impl Fixture {
    async fn new(root: &Path, target: ConversationId) -> Self {
        nessa_local_storage::create_directory(root).unwrap();
        let provider = Arc::new(ProviderFactory::default());
        let records = Arc::new(RecordStorage::new(root.join("records")).unwrap());
        let metadata = Arc::new(
            LocalConversationStore::open(&root.join("metadata.sqlite3"))
                .unwrap()
                .with_access_policy(crate::conversation_test_support::access_policy()),
        );
        let service = ConversationService::new(
            ConversationDependencies {
                access_policy: crate::conversation_test_support::access_policy(),
                agents: only(Arc::new(Provider::new(provider.clone()))),
                storage: records.clone(),
                metadata: metadata.clone(),
                mode_audit: Arc::new(AcceptingModeAudit),
                creation_audit: Arc::new(AcceptingCreationAudit),
                file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
                attachments: None,
                summaries: metadata.clone(),
                listing: metadata,
                deletion_audit: Arc::new(AcceptingDeletionAudit),
                provider_sessions: ProviderSessionErasers::default(),
                deletion_budgets: DELETION_BUDGETS,
                message_commit_clock: Arc::new(RuntimeMessageCommitClock::new()),
                clock: Arc::new(TestClock),
                environment: crate::conversation::infrastructure::in_process_environment().into(),
            },
            ConversationLimits::default(),
            None,
        )
        .unwrap();
        service
            .create(
                target.clone(),
                caller("create"),
                RequestedConversation::default(),
            )
            .await
            .unwrap();
        let (authority, session) = authority().await;
        let commands = Arc::new(Store {
            inner: records.clone(),
            fail: AtomicBool::new(false),
            fail_terminal: Arc::new(AtomicBool::new(false)),
            gate: Mutex::new(None),
        });
        Self {
            service,
            provider,
            records,
            commands,
            target,
            authority,
            session,
        }
    }
    fn routes(&self) -> axum::Router {
        let binding = EventBinding {
            id: EventId::new("ci".into()).unwrap(),
            conversation: self.target.clone(),
            session: self.session.clone(),
            resource: resource(),
            access: self.authority.clone(),
            clock: self.authority.clone(),
            policy: Arc::new(CedarPolicyEvaluator::new().unwrap()),
            service: self.service.clone(),
            commands: self.commands.clone(),
        };
        router(binding, SigningKey::new(KEY.to_vec()).unwrap())
    }
    async fn server(&self) -> HttpServer {
        let routes = self.routes();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, routes).await.unwrap();
        });
        HttpServer { address, task }
    }
    async fn completed(&self, execution: &str) {
        self.terminal(
            execution,
            nessa_protocol::conversation::view::ConversationMessageStatus::Completed,
        )
        .await;
    }
    async fn terminal(
        &self,
        execution: &str,
        status: nessa_protocol::conversation::view::ConversationMessageStatus,
    ) {
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let view = self
                    .service
                    .read(self.target.clone(), caller("read"))
                    .await
                    .unwrap();
                if view
                    .messages
                    .iter()
                    .any(|message| message.execution_id == execution && message.status == status)
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    async fn shutdown(self) {
        self.service.shutdown().await.unwrap();
        drop(self.service);
        self.records.shutdown().await.unwrap();
    }
}
struct HttpServer {
    address: std::net::SocketAddr,
    task: JoinHandle<()>,
}
impl Drop for HttpServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl HttpServer {
    async fn request(&self, body: &str, timestamp: &str, key: &[u8]) -> (u16, Value) {
        let mut mac = Hmac::<Sha256>::new_from_slice(key).unwrap();
        mac.update(b"nessa-event\nci\n");
        mac.update(timestamp.as_bytes());
        mac.update(b"\n");
        mac.update(body.as_bytes());
        let signature = STANDARD.encode(mac.finalize().into_bytes());
        self.raw(body, &format!("Content-Type: application/json\r\nx-nessa-timestamp: {timestamp}\r\nx-nessa-signature: {signature}\r\n")).await
    }
    async fn raw(&self, body: &str, headers: &str) -> (u16, Value) {
        let mut socket = TcpStream::connect(self.address).await.unwrap();
        socket.write_all(format!("POST /events HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {}\r\n{headers}\r\n{body}", body.len()).as_bytes()).await.unwrap();
        let mut response = Vec::new();
        tokio::time::timeout(Duration::from_secs(20), socket.read_to_end(&mut response))
            .await
            .unwrap()
            .unwrap();
        let response = String::from_utf8(response).unwrap();
        let (head, body) = response.split_once("\r\n\r\n").unwrap();
        let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
        (status, serde_json::from_str(body).unwrap())
    }
    async fn event(&self, id: &str) -> (u16, Value) {
        self.request(
            &json!({"id": id, "payload": {"conclusion":"success"}}).to_string(),
            "100",
            &KEY,
        )
        .await
    }
}
fn id() -> ConversationId {
    ConversationId::new(&uuid::Uuid::new_v4().to_string()).unwrap()
}

#[tokio::test]
async fn signed_http_event_runs_deterministically_and_replays_after_reopen() {
    let root = tempfile::tempdir().unwrap();
    let target = id();
    let fixture = Fixture::new(root.path(), target.clone()).await;
    let server = fixture.server().await;
    let (status, receipt) = server.event("delivery-1").await;
    assert_eq!(status, 202, "{receipt}");
    let execution = receipt["executionId"].as_str().unwrap();
    fixture.completed(execution).await;
    assert_eq!(server.event("delivery-1").await, (202, receipt.clone()));
    assert_eq!(fixture.provider.executions.lock().unwrap().len(), 1);
    drop(server);
    fixture.shutdown().await;
    let fixture = Fixture::new(root.path(), target).await;
    let server = fixture.server().await;
    assert_eq!(server.event("delivery-1").await, (202, receipt));
    assert!(fixture.provider.executions.lock().unwrap().is_empty());
    drop(server);
    fixture.shutdown().await;
}
#[tokio::test]
async fn concurrent_duplicates_share_a_turn_and_changed_payload_conflicts() {
    let root = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(root.path(), id()).await;
    let server = Arc::new(fixture.server().await);
    let mut requests = Vec::new();
    for _ in 0..8 {
        let server = server.clone();
        requests.push(tokio::spawn(async move { server.event("same").await }));
    }
    let mut execution = None;
    for task in requests {
        let (status, receipt) = task.await.unwrap();
        if status == 503 {
            assert_eq!(receipt["error"], "dispatch_unconfirmed");
            continue;
        }
        assert_eq!(status, 202, "{receipt}");
        let current = receipt["executionId"].as_str().unwrap().to_owned();
        if let Some(prior) = &execution {
            assert_eq!(prior, &current);
        }
        execution = Some(current);
    }
    let (status, replay) = server.event("same").await;
    assert_eq!(status, 202, "{replay}");
    assert_eq!(replay["executionId"].as_str(), execution.as_deref());
    fixture.completed(execution.as_ref().unwrap()).await;
    assert_eq!(fixture.provider.executions.lock().unwrap().len(), 1);
    assert_eq!(
        server
            .request(r#"{"id":"same","payload":"changed"}"#, "100", &KEY)
            .await
            .0,
        409
    );
    let (_, other) = server.event("distinct").await;
    fixture
        .completed(other["executionId"].as_str().unwrap())
        .await;
    assert_eq!(fixture.provider.executions.lock().unwrap().len(), 2);
    drop(server);
    fixture.shutdown().await;
}
#[tokio::test]
async fn rejects_invalid_signatures_freshness_shapes_and_oversized_payloads_before_execution() {
    let root = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(root.path(), id()).await;
    let server = fixture.server().await;
    let valid = r#"{"id":"event","payload":{}}"#;
    assert_eq!(server.request(valid, "100", &[0; 32]).await.0, 401);
    assert_eq!(server.request(valid, "401", &KEY).await.0, 401);
    fixture.authority.now.store(500_000, Ordering::SeqCst);
    assert_eq!(server.request(valid, "100", &KEY).await.0, 401);
    fixture.authority.now.store(100_000, Ordering::SeqCst);
    assert_eq!(server.request(valid, "0100", &KEY).await.0, 401);
    assert_eq!(
        server
            .raw(valid, "Content-Type: application/json\r\n")
            .await
            .0,
        401
    );
    assert_eq!(server.raw(valid, "").await.0, 415);
    for signature in ["short".to_owned(), "!".repeat(44)] {
        assert_eq!(server.raw(valid,&format!("Content-Type: application/json\r\nx-nessa-timestamp: 100\r\nx-nessa-signature: {signature}\r\n")).await.0,401);
    }
    assert_eq!(server.raw(valid,"Content-Type: application/json\r\nx-nessa-timestamp: 100\r\nx-nessa-timestamp: 100\r\nx-nessa-signature: AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=\r\n").await.0,401);

    for body in [
        "{",
        r#"{"id":"event"}"#,
        r#"{"id":"","payload":{}}"#,
        r#"{"id":"a/b","payload":{}}"#,
        r#"{"id":"event","payload":{},"actor":"admin"}"#,
        r#"{"id":"a","id":"b","payload":{}}"#,
    ] {
        assert_eq!(server.request(body, "100", &KEY).await.0, 400, "{body}");
    }
    assert_eq!(
        server
            .request(&"x".repeat(MAX_EVENT_BYTES + 1), "100", &KEY)
            .await
            .0,
        413
    );
    assert!(fixture.provider.executions.lock().unwrap().is_empty());
    assert_eq!(server.event("after-rejections").await.0, 202);
    drop(server);
    fixture.shutdown().await;
}
#[tokio::test]
async fn current_authority_denies_replay_revocation_expiry_and_unavailability() {
    let root = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(root.path(), id()).await;
    let server = fixture.server().await;
    let (_, receipt) = server.event("first").await;
    fixture
        .completed(receipt["executionId"].as_str().unwrap())
        .await;
    fixture.authority.unavailable.store(true, Ordering::SeqCst);
    assert_eq!(server.event("first").await.0, 503);
    fixture.authority.unavailable.store(false, Ordering::SeqCst);
    let saved = fixture.authority.snapshot.lock().unwrap().clone();
    fixture
        .authority
        .snapshot
        .lock()
        .unwrap()
        .credential
        .restore_revoked_at(100)
        .unwrap();
    assert_eq!(server.event("first").await.0, 403);
    *fixture.authority.snapshot.lock().unwrap() = saved.clone();
    fixture.authority.snapshot.lock().unwrap().credential = Credential::new(
        CredentialId::new("credential").unwrap(),
        PrincipalId::new("person").unwrap(),
        OrganizationId::new("org").unwrap(),
        AudienceId::new("gateway").unwrap(),
        1,
        10_000,
        vec![],
    )
    .unwrap();
    assert_eq!(server.event("first").await.0, 403);
    *fixture.authority.snapshot.lock().unwrap() = saved;
    fixture.authority.now.store(10_000_000, Ordering::SeqCst);
    assert_eq!(
        server
            .request(r#"{"id":"expired","payload":{}}"#, "10000", &KEY)
            .await
            .0,
        403
    );
    assert_eq!(fixture.provider.executions.lock().unwrap().len(), 1);
    drop(server);
    fixture.shutdown().await;
}
#[tokio::test]
async fn failed_storage_can_recover_with_same_delivery_without_fake_acceptance() {
    let root = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(root.path(), id()).await;
    let server = fixture.server().await;
    fixture.commands.fail.store(true, Ordering::SeqCst);
    assert_eq!(server.event("recover").await.0, 503);
    assert!(fixture.provider.executions.lock().unwrap().is_empty());
    fixture.commands.fail.store(false, Ordering::SeqCst);
    let (status, receipt) = server.event("recover").await;
    assert_eq!(status, 202, "{receipt}");
    fixture
        .completed(receipt["executionId"].as_str().unwrap())
        .await;
    assert_eq!(fixture.provider.executions.lock().unwrap().len(), 1);
    drop(server);
    fixture.shutdown().await;
}
#[test]
fn signing_key_is_bounded() {
    assert!(SigningKey::new(vec![0; 31]).is_err());
    assert!(SigningKey::new(vec![0; 1025]).is_err());
    assert!(SigningKey::new(vec![0; 32]).is_ok());
}

#[tokio::test]
async fn lost_terminal_acknowledgement_is_interrupted_after_restart_and_does_not_redispatch() {
    let root = tempfile::tempdir().unwrap();
    let target = id();
    let fixture = Fixture::new(root.path(), target.clone()).await;
    let server = fixture.server().await;
    fixture.commands.fail_terminal.store(true, Ordering::SeqCst);
    assert_eq!(server.event("uncertain").await.0, 503);
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if !fixture.provider.executions.lock().unwrap().is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let execution = fixture.provider.executions.lock().unwrap()[0].clone();
    fixture.completed(&execution).await;
    fixture
        .commands
        .fail_terminal
        .store(false, Ordering::SeqCst);
    let (status, result) = server.event("uncertain").await;
    assert_eq!(status, 409);
    assert_eq!(result["error"], "interrupted");
    assert_eq!(fixture.provider.executions.lock().unwrap().len(), 1);
    drop(server);
    fixture.shutdown().await;
    let fixture = Fixture::new(root.path(), target).await;
    let server = fixture.server().await;
    assert_eq!(server.event("uncertain").await.1["error"], "interrupted");
    assert!(fixture.provider.executions.lock().unwrap().is_empty());
    drop(server);
    fixture.shutdown().await;
}

#[tokio::test]
async fn execution_failure_is_separate_from_dispatch_and_an_explicit_new_event_can_run() {
    let root = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(root.path(), id()).await;
    let server = fixture.server().await;
    *fixture.provider.prepare_failure.lock().unwrap() = Some(
        nessa_sdk::application::agent_execution::agents::AgentError::Protocol(
            "local fixture failure".into(),
        ),
    );
    let (status, receipt) = server.event("fails").await;
    assert_eq!(status, 202, "{receipt}");
    fixture
        .terminal(
            receipt["executionId"].as_str().unwrap(),
            nessa_protocol::conversation::view::ConversationMessageStatus::Failed,
        )
        .await;
    assert_eq!(server.event("fails").await, (202, receipt));
    assert!(fixture.provider.executions.lock().unwrap().is_empty());
    let (status, receipt) = server.event("explicit-retry").await;
    assert_eq!(status, 202, "{receipt}");
    fixture
        .completed(receipt["executionId"].as_str().unwrap())
        .await;
    let message = fixture.provider.messages.lock().unwrap()[0]
        .text_str()
        .to_owned();
    assert_eq!(
        serde_json::from_str::<Value>(&message).unwrap(),
        json!({"eventId":"explicit-retry","payload":{"conclusion":"success"}})
    );
    drop(server);
    fixture.shutdown().await;
}
#[tokio::test]
async fn slow_bodies_have_bounded_concurrency_and_a_deadline_then_release_capacity() {
    let root = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(root.path(), id()).await;
    let server = fixture.server().await;
    let before = fixture.authority.clock_reads.load(Ordering::SeqCst);
    let mut sockets = Vec::new();
    for _ in 0..8 {
        let mut socket = TcpStream::connect(server.address).await.unwrap();
        socket.write_all(b"POST /events HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: 1\r\nContent-Type: application/json\r\nx-nessa-timestamp: 100\r\nx-nessa-signature: AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=\r\n\r\n").await.unwrap();
        sockets.push(socket);
    }
    tokio::time::timeout(Duration::from_secs(3), async {
        while fixture.authority.clock_reads.load(Ordering::SeqCst) < before + 8 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(server.event("busy").await.0, 503);
    for mut socket in sockets {
        let mut response = String::new();
        tokio::time::timeout(Duration::from_secs(8), socket.read_to_string(&mut response))
            .await
            .unwrap()
            .unwrap();
        assert!(response.starts_with("HTTP/1.1 408"), "{response}");
    }
    assert!(fixture.provider.executions.lock().unwrap().is_empty());
    assert_eq!(server.event("after-timeout").await.0, 202);
    drop(server);
    fixture.shutdown().await;
}

#[tokio::test]
async fn queued_event_can_be_stopped_with_existing_exact_turn_command_and_replay_does_not_revive_it(
) {
    let root = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(root.path(), id()).await;
    let server = fixture.server().await;
    let (release, gate) = tokio::sync::oneshot::channel();
    *fixture.provider.execution_gate.lock().unwrap() = Some(gate);
    let (status, first) = server.event("running").await;
    assert_eq!(status, 202);
    tokio::time::timeout(Duration::from_secs(15), async {
        while fixture.provider.executions.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let (status, second) = server.event("queued").await;
    assert_eq!(status, 202, "{second}");
    let stopped = fixture
        .service
        .stop_command(
            fixture.records.clone(),
            fixture.target.clone(),
            caller("cancel-queued-event"),
            second["executionId"].as_str().unwrap().into(),
        )
        .await
        .unwrap();
    assert_eq!(
        stopped.outcome(),
        Some(nessa_sdk::application::agent_execution::commands::MutationOutcome::Withdrawn)
    );
    assert_eq!(server.event("queued").await, (202, second.clone()));
    release.send(()).unwrap();
    fixture
        .completed(first["executionId"].as_str().unwrap())
        .await;
    fixture
        .terminal(
            second["executionId"].as_str().unwrap(),
            nessa_protocol::conversation::view::ConversationMessageStatus::Cancelled,
        )
        .await;
    assert_eq!(fixture.provider.executions.lock().unwrap().len(), 1);
    drop(server);
    fixture.shutdown().await;
}

fn signed_request(id: &str) -> axum::http::Request<axum::body::Body> {
    let body = json!({"id":id,"payload":{"conclusion":"success"}}).to_string();
    let mut mac = Hmac::<Sha256>::new_from_slice(&KEY).unwrap();
    mac.update(b"nessa-event\nci\n100\n");
    mac.update(body.as_bytes());
    axum::http::Request::builder()
        .method("POST")
        .uri("/events")
        .header("content-type", "application/json")
        .header("x-nessa-timestamp", "100")
        .header(
            "x-nessa-signature",
            STANDARD.encode(mac.finalize().into_bytes()),
        )
        .body(axum::body::Body::from(body))
        .unwrap()
}
#[tokio::test]
async fn cancelled_http_responses_keep_command_capacity_until_detached_admission_settles() {
    use tower::ServiceExt;
    let root = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(root.path(), id()).await;
    let routes = fixture.routes();
    let gate = Arc::new(CommandGate {
        entered: AtomicU64::new(0),
        released: tokio::sync::watch::channel(false).0,
    });
    *fixture.commands.gate.lock().unwrap() = Some(gate.clone());
    let mut responses = Vec::new();
    for index in 0..8 {
        let routes = routes.clone();
        responses.push(tokio::spawn(async move {
            routes
                .oneshot(signed_request(&format!("cancelled-{index}")))
                .await
                .unwrap()
        }));
    }
    tokio::time::timeout(Duration::from_secs(15), async {
        while gate.entered.load(Ordering::SeqCst) != 8 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    for response in &responses {
        response.abort();
    }
    for response in responses {
        assert!(response.await.unwrap_err().is_cancelled());
    }
    let refused = tokio::time::timeout(
        Duration::from_secs(1),
        routes.clone().oneshot(signed_request("ninth")),
    )
    .await
    .expect("cancelled response futures must not release unfinished command capacity")
    .unwrap();
    assert_eq!(
        refused.status(),
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(gate.entered.load(Ordering::SeqCst), 8);
    gate.released.send(true).unwrap();
    let admitted = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let response = routes
                .clone()
                .oneshot(signed_request("after-release"))
                .await
                .unwrap();
            if response.status() == axum::http::StatusCode::ACCEPTED {
                break response;
            }
            assert_eq!(
                response.status(),
                axum::http::StatusCode::SERVICE_UNAVAILABLE
            );
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let bytes = axum::body::to_bytes(admitted.into_body(), 1024)
        .await
        .unwrap();
    let receipt: Value = serde_json::from_slice(&bytes).unwrap();
    fixture
        .completed(receipt["executionId"].as_str().unwrap())
        .await;
    for index in 0..8 {
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let response = routes
                    .clone()
                    .oneshot(signed_request(&format!("cancelled-{index}")))
                    .await
                    .unwrap();
                if response.status() == axum::http::StatusCode::ACCEPTED {
                    break;
                }
                assert_eq!(
                    response.status(),
                    axum::http::StatusCode::SERVICE_UNAVAILABLE
                );
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    assert_eq!(fixture.provider.executions.lock().unwrap().len(), 9);
    fixture.shutdown().await;
}
#[tokio::test]
async fn cancelled_http_body_reads_release_capacity_before_command_handoff() {
    use tower::ServiceExt;
    let root = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(root.path(), id()).await;
    let routes = fixture.routes();
    let polled = Arc::new(AtomicU64::new(0));
    let mut responses = Vec::new();
    for _ in 0..8 {
        let mut request = signed_request("not-admitted");
        let polled = polled.clone();
        let mut first = true;
        *request.body_mut() =
            axum::body::Body::from_stream(futures_util::stream::poll_fn(move |_| {
                if first {
                    first = false;
                    polled.fetch_add(1, Ordering::SeqCst);
                }
                std::task::Poll::<Option<Result<axum::body::Bytes, std::io::Error>>>::Pending
            }));
        let routes = routes.clone();
        responses.push(tokio::spawn(async move {
            routes.oneshot(request).await.unwrap()
        }));
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        while polled.load(Ordering::SeqCst) != 8 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    for response in &responses {
        response.abort();
    }
    for response in responses {
        assert!(response.await.unwrap_err().is_cancelled());
    }
    assert!(fixture.provider.executions.lock().unwrap().is_empty());
    let response = routes
        .oneshot(signed_request("after-body-drop"))
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::ACCEPTED);
    fixture.shutdown().await;
}
#[tokio::test]
async fn admission_task_failure_is_visible_and_recovers_its_slot() {
    let root = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(root.path(), id()).await;
    let server = fixture.server().await;
    fixture.authority.panic_read.store(true, Ordering::SeqCst);
    let (status, body) = server.event("task-panic").await;
    assert_eq!(status, 503);
    assert_eq!(body["error"], "dispatch_unconfirmed");
    assert!(fixture.provider.executions.lock().unwrap().is_empty());
    fixture.authority.panic_read.store(false, Ordering::SeqCst);
    let (status, receipt) = server.event("task-panic").await;
    assert_eq!(status, 202);
    fixture
        .completed(receipt["executionId"].as_str().unwrap())
        .await;
    drop(server);
    fixture.shutdown().await;
}
