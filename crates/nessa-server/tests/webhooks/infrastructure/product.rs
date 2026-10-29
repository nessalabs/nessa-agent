//! Actual product credential/share contracts with synthetic private authorities.
use crate::agents::application::{AgentProbe, AgentProbeEvidence};
use crate::conversation::{
    application::{
        error_code, ConversationCaller, ConversationError, ConversationRepository,
        ReceiverAuthority, ReceiverBinding, ShareGrantChange, ShareGrantTransition, ShareGrants,
    },
    domain::{Conversation, ConversationDeletion, ShareAccess},
    infrastructure::{
        CedarConversationAccessPolicy, CedarOwnerPhonePolicyEvaluator, GrantWritePause,
        LocalConversationStore,
    },
};
use crate::core::{Outcome, ShutdownReport};
use crate::product::{ProductDependencies, ProductRouteState};
use crate::server::entrypoint::http;
use crate::webhooks::{
    application::{MacKey, RegistrationConfig},
    domain::Destination,
    infrastructure::{ConfiguredWebhook, SqlOutbox, WebhookRuntime},
};
use futures_util::{FutureExt, SinkExt, StreamExt};
use nessa_auth::{
    adapters::{
        cedar::CedarPolicyEvaluator,
        local::{BootstrapRequest, LocalCredentialStore},
    },
    application::{
        credential_admin::{
            CredentialAdmin, CredentialAdminError, CredentialIssuePurpose, IssueCredentialOutcome,
            IssueCredentialRequest, ListCredentialsRequest, RevokeCredentialOutcome,
            RevokeCredentialRequest,
        },
        dto::{
            CredentialGrantDto, CredentialMetadataDto, MembershipInputDto, MembershipRoleDto,
            MembershipStateDto, OrganizationInputDto, PrincipalInputDto, PrincipalKindDto,
            ResourceDto,
        },
        ports::{AccessReader, Clock, PortFuture},
    },
    domain::{AudienceId, CredentialId, IssuanceCause, OrganizationId, PrincipalId, ResourceId},
};
use nessa_local_database::rusqlite::{Connection, OpenFlags};
use nessa_protocol::{
    agents::AgentId,
    conversation::{
        domain::{ConversationApprovalMode, ConversationId, ConversationModelId},
        read_scope::ReadRefusal,
    },
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    future::{Future, Ready},
    path::PathBuf,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{
    net::TcpStream,
    sync::{oneshot, Notify},
    task::JoinHandle,
};
use tokio_tungstenite::{tungstenite::Message, MaybeTlsStream, WebSocketStream};
use uuid::Uuid;
const WAIT: Duration = Duration::from_secs(10);
struct Time(AtomicU64, Mutex<Option<PathBuf>>);
impl Time {
    fn new(now: u64) -> Self {
        Self(AtomicU64::new(now), Mutex::new(None))
    }
}

impl Clock for Time {
    fn unix_milliseconds(&self) -> u64 {
        let mut boundary = self.1.lock().unwrap();
        let sending = boundary.as_ref().is_some_and(|path| {
            Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .ok()
                .is_some_and(|connection| {
                    connection.busy_timeout(Duration::ZERO).unwrap();
                    connection
                        .query_row(
                            "SELECT EXISTS(SELECT 1 FROM deliveries WHERE phase='sending' LIMIT 1)",
                            [],
                            |row| row.get::<_, bool>(0),
                        )
                        .unwrap_or(false)
                })
        });
        if sending {
            self.0.store(100_000, Ordering::SeqCst);
            *boundary = None;
        }
        self.0.load(Ordering::SeqCst)
    }
}
impl nessa_protocol::clock::Clock for Time {
    fn elapsed_ms(&self) -> u64 {
        0
    }
}
struct NoAgents;
impl AgentProbe for NoAgents {
    fn evidence(&self, _: AgentId) -> Option<AgentProbeEvidence> {
        None
    }
}
struct NoReceivers {
    credentials: Arc<LocalCredentialStore>,
}
impl ReceiverAuthority for NoReceivers {
    fn resolve<'a>(
        &'a self,
        credential: &'a CredentialId,
    ) -> Pin<Box<dyn Future<Output = Result<Option<ReceiverBinding>, ReadRefusal>> + Send + 'a>>
    {
        Box::pin(async move {
            let snapshot = self
                .credentials
                .read(credential)
                .await
                .expect("synthetic fixture credential remains readable");
            assert_ne!(
                snapshot.origin.cause(),
                IssuanceCause::WebhookIssue,
                "webhook targets never ask paired receiver authority"
            );
            Ok(None)
        })
    }
}

struct IssuePause {
    entered: Notify,
    release: Notify,
    panic_after_commit: bool,
}
struct BaseAdmin {
    store: Arc<LocalCredentialStore>,
    issued: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    pause: Option<Arc<IssuePause>>,
}
impl CredentialAdmin for BaseAdmin {
    fn issue<'a>(
        &'a self,
        request: IssueCredentialRequest,
    ) -> PortFuture<'a, IssueCredentialOutcome, CredentialAdminError> {
        let store = self.store.clone();
        let issued = self.issued.clone();
        let pause = self
            .pause
            .clone()
            .filter(|_| request.principal.id == "hook");
        Box::pin(async move {
            let outcome = tokio::task::spawn_blocking(
                move || -> Result<IssueCredentialOutcome, CredentialAdminError> {
                    let outcome = store
                        .issue_sync(request)
                        .map_err(CredentialAdminError::from)?;
                    if let IssueCredentialOutcome::Issued {
                        metadata, evidence, ..
                    } = &outcome
                    {
                        issued
                            .lock()
                            .unwrap()
                            .insert(metadata.id.clone(), evidence.expose_bytes().to_vec());
                    }
                    Ok(outcome)
                },
            )
            .await
            .map_err(|_| CredentialAdminError::Unavailable)??;
            if let Some(pause) = pause {
                pause.entered.notify_one();
                pause.release.notified().await;
                assert!(
                    !pause.panic_after_commit,
                    "synthetic post-commit issue panic"
                );
            }
            Ok(outcome)
        })
    }
    fn list<'a>(
        &'a self,
        request: ListCredentialsRequest,
    ) -> PortFuture<'a, Vec<CredentialMetadataDto>, CredentialAdminError> {
        let store = self.store.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                store
                    .list_sync(&request)
                    .map_err(CredentialAdminError::from)
            })
            .await
            .map_err(|_| CredentialAdminError::Unavailable)?
        })
    }
    fn revoke<'a>(
        &'a self,
        request: RevokeCredentialRequest,
    ) -> PortFuture<'a, RevokeCredentialOutcome, CredentialAdminError> {
        let store = self.store.clone();
        Box::pin(async move {
            let handle = tokio::runtime::Handle::current();
            tokio::task::spawn_blocking(move || handle.block_on(store.revoke(request)))
                .await
                .map_err(|_| CredentialAdminError::Unavailable)?
        })
    }
}
struct Client {
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    next: u64,
}
impl Client {
    async fn open(address: std::net::SocketAddr, token: &str) -> (Self, Value) {
        let (socket, _) = tokio::time::timeout(
            WAIT,
            tokio_tungstenite::connect_async(format!("ws://{address}/session")),
        )
        .await
        .unwrap()
        .unwrap();
        let mut client = Self { socket, next: 0 };
        let challenge = client.read().await;
        assert_eq!(challenge["event"], "session.challenge");
        let ready=client.call("session.authenticate",json!({"minVersion":1,"maxVersion":1,"nonce":challenge["payload"]["nonce"],"credential":token,"client":{"id":"synthetic-webhook-owner"},"surface":{"kind":"cli","instance":"synthetic-webhook-owner"}})).await;
        (client, ready)
    }
    async fn read(&mut self) -> Value {
        self.read_observation()
            .await
            .expect("unexpected product close")
    }
    async fn read_observation(&mut self) -> Result<Value, Value> {
        tokio::time::timeout(WAIT, async {
            loop {
                match self.socket.next().await.unwrap().unwrap() {
                    Message::Text(text) => return Ok(serde_json::from_str(&text).unwrap()),
                    Message::Close(Some(close)) => return Err(json!({"closeCode":u16::from(close.code),"reason":serde_json::from_str::<Value>(&close.reason).unwrap()})),
                    Message::Close(None) => return Err(json!({"closeCode":null})),
                    Message::Ping(_) | Message::Pong(_) => {}
                    other => panic!("unexpected product message {other:?}"),
                }
            }
        })
        .await
        .expect("product frame is bounded")
    }
    async fn call(&mut self, method: &str, params: Value) -> Value {
        self.call_until_close(method, params)
            .await
            .expect("unexpected product close")
    }
    async fn call_until_close(&mut self, method: &str, params: Value) -> Result<Value, Value> {
        self.next += 1;
        let id = self.next.to_string();
        self.socket
            .send(Message::Text(
                json!({"type":"req","id":id,"method":method,"params":params})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        tokio::time::timeout(WAIT, async {
            loop {
                let frame = self.read_observation().await?;
                if frame["type"] == "res" && frame["id"] == id {
                    return Ok(frame);
                }
            }
        })
        .await
        .expect("product command is bounded")
    }
}
struct Harness {
    _root: Arc<tempfile::TempDir>,
    clock: Arc<Time>,
    state: ProductRouteState,
    runtime: Arc<WebhookRuntime>,
    store: Arc<SqlOutbox>,
    grants: Arc<LocalConversationStore>,
    issued: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    owner: String,
    address: std::net::SocketAddr,
    stop: oneshot::Sender<()>,
    server: JoinHandle<std::io::Result<()>>,
    a: ConversationId,
    b: ConversationId,
}
impl Harness {
    async fn new() -> Self {
        Self::with_issue_pause(None).await
    }
    async fn with_issue_pause(pause: Option<Arc<IssuePause>>) -> Self {
        Self::with_runtime_clock(pause, None).await
    }
    async fn with_runtime_clock(
        pause: Option<Arc<IssuePause>>,
        runtime_clock: Option<Arc<dyn Clock>>,
    ) -> Self {
        Self::with_configured(
            pause,
            runtime_clock,
            vec![ConfiguredWebhook {
                principal: PrincipalId::new("hook").unwrap(),
                registration: RegistrationConfig {
                    destination: Destination::parse("https://recipient.test/synthetic").unwrap(),
                    key: Arc::new(MacKey::new([7; 32])),
                },
            }],
        )
        .await
    }
    async fn with_configured(
        pause: Option<Arc<IssuePause>>,
        runtime_clock: Option<Arc<dyn Clock>>,
        configured: Vec<ConfiguredWebhook>,
    ) -> Self {
        Self::with_owner_read(pause, runtime_clock, configured, true).await
    }
    async fn with_owner_read(
        pause: Option<Arc<IssuePause>>,
        runtime_clock: Option<Arc<dyn Clock>>,
        configured: Vec<ConfiguredWebhook>,
        owner_read: bool,
    ) -> Self {
        let root = Arc::new(tempfile::tempdir().unwrap());
        let private = root.path().join("private");
        nessa_local_storage::create_directory(&private).unwrap();
        let credentials = Arc::new(
            LocalCredentialStore::open(
                &private,
                "auth.json",
                Arc::new(CedarPolicyEvaluator::new().unwrap()),
            )
            .unwrap(),
        );
        let grant = |action: &str| CredentialGrantDto {
            action: action.into(),
            resource: ResourceDto {
                organization_id: "org".into(),
                id: "gateway".into(),
            },
        };
        let bootstrap = credentials
            .bootstrap(BootstrapRequest {
                gateway_id: "gateway".into(),
                organization: OrganizationInputDto { id: "org".into() },
                principal: PrincipalInputDto {
                    id: "owner".into(),
                    kind: PrincipalKindDto::Human,
                },
                membership: MembershipInputDto {
                    id: "owner-member".into(),
                    principal_id: "owner".into(),
                    organization_id: "org".into(),
                    role: MembershipRoleDto::Admin,
                    state: MembershipStateDto::Active,
                },
                credential_id: "owner-key".into(),
                issued_at: 1,
                expires_at: None,
                grants: if owner_read {
                    vec![grant("credential.manage"), grant("conversation.read")]
                } else {
                    vec![grant("credential.manage")]
                },
            })
            .unwrap();
        let owner = String::from_utf8(bootstrap.evidence.expose_bytes().to_vec()).unwrap();
        let clock = Arc::new(Time::new(2000));
        let access = Arc::new(CedarConversationAccessPolicy::new().unwrap());
        let grants = Arc::new(
            LocalConversationStore::open(&private.join("conversations.db"))
                .unwrap()
                .with_access_policy(access.clone()),
        );
        let a = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
        let b = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
        for id in [&a, &b] {
            let _ = grants
                .create(
                    Conversation::new(
                        id.clone(),
                        OrganizationId::new("org").unwrap(),
                        PrincipalId::new("owner").unwrap(),
                        "synthetic".into(),
                        format!("create-{id}"),
                        1,
                        AgentId::Claude,
                        ConversationModelId::new("synthetic").unwrap(),
                        ConversationApprovalMode::Ask,
                    )
                    .unwrap(),
                )
                .await
                .unwrap();
        }
        let store = Arc::new(
            SqlOutbox::open(
                &private.join("outbox.db"),
                private.join("audit"),
                clock.clone(),
            )
            .unwrap(),
        );
        let issued = Arc::new(Mutex::new(HashMap::new()));
        let base = Arc::new(BaseAdmin {
            store: credentials.clone(),
            issued: issued.clone(),
            pause,
        });
        let runtime = Arc::new(
            WebhookRuntime::new(
                configured,
                base,
                credentials.clone(),
                store.clone(),
                runtime_clock.unwrap_or_else(|| clock.clone()),
            )
            .unwrap(),
        );
        runtime.recover().await.unwrap();
        let state = ProductRouteState::new(
            ResourceId::new("gateway").unwrap(),
            OrganizationId::new("org").unwrap(),
            AudienceId::new("gateway").unwrap(),
            ProductDependencies {
                verifier: credentials.clone(),
                access: credentials.clone(),
                owner_phone_purpose: credentials.clone(),
                owner_phone_policy: Arc::new(CedarOwnerPhonePolicyEvaluator::new().unwrap()),
                owner_phone_receivers: None,
                clock: clock.clone(),
                uptime_clock: clock.clone(),
                policy: Arc::new(CedarPolicyEvaluator::new().unwrap()),
                agent_probe: Arc::new(NoAgents),
            },
        )
        .with_passive_read(
            Arc::new(NoReceivers {
                credentials: credentials.clone(),
            }),
            grants.clone(),
            grants.clone(),
            access,
        )
        .with_webhooks(runtime.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (stop, stopped) = oneshot::channel();
        let router = http::router(state.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
        });
        Self {
            _root: root,
            clock,
            state,
            runtime,
            store,
            grants,
            issued,
            owner,
            address,
            stop,
            server,
            a,
            b,
        }
    }
    async fn close(self) -> Result<(), CredentialAdminError> {
        self.state.close_sockets();
        let _ = self.stop.send(());
        let drained = tokio::time::timeout(WAIT, self.runtime.shutdown())
            .await
            .unwrap();
        tokio::time::timeout(WAIT, self.server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        drained
    }
}
fn issue_params() -> Value {
    json!({"requestId":"product-hook-issue","principal":{"id":"hook","kind":"integration"},"membership":{"id":"hook-member","principalId":"hook","organizationId":"org","role":"member","state":"active"},"expiresAt":100,"grants":[{"action":"conversation.read","resource":{"organizationId":"org","id":"gateway"}}]})
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn product_credential_issue_registers_and_share_targets_the_actual_webhook_principal() {
    let f = Harness::new().await;
    let (mut client, ready) = Client::open(f.address, &f.owner).await;
    assert_eq!(ready["ok"], true);
    let reply = client.call("credential.issue", issue_params()).await;
    assert_eq!(reply["ok"], true);
    assert!(reply["payload"].get("secret").is_none());
    assert_eq!(reply["payload"]["secretUnavailable"], true);
    let credential =
        CredentialId::new(reply["payload"]["credential"]["id"].as_str().unwrap()).unwrap();
    let registered = f
        .store
        .registration(credential.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(registered.target.principal.as_str(), "hook");
    let shared=client.call("conversation.share",json!({"requestId":"product-share","conversationId":f.a.to_string(),"credentialId":credential.as_str(),"capabilities":["read"]})).await;
    assert_eq!(shared["ok"], true, "{shared}");
    let grant = f
        .grants
        .principal_grant(&f.a, &registered.target.principal)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(grant.credential_id, credential);
    assert!(grant.receiver_id.is_none());
    assert!(f
        .grants
        .principal_grant(&f.b, &registered.target.principal)
        .await
        .unwrap()
        .is_none());
    let secret = String::from_utf8(
        f.issued
            .lock()
            .unwrap()
            .get(credential.as_str())
            .unwrap()
            .clone(),
    )
    .unwrap();
    let (unauthorized, denied) = Client::open(f.address, &secret).await;
    assert_eq!(
        denied["ok"], false,
        "webhook credential has no product session"
    );
    drop(unauthorized);
    let unshared=client.call("conversation.unshare",json!({"requestId":"product-unshare","conversationId":f.a.to_string(),"credentialId":credential.as_str()})).await;
    assert_eq!(unshared["ok"], true);
    assert!(f
        .grants
        .principal_grant(&f.a, &registered.target.principal)
        .await
        .unwrap()
        .is_none());
    let revoked = client
        .call(
            "credential.revoke",
            json!({"requestId":"product-revoke","credentialId":credential.as_str()}),
        )
        .await;
    assert_eq!(revoked["ok"], true);
    assert!(f.store.registration(credential).await.unwrap().is_none());
    drop(client);
    f.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lost_product_observer_and_cancelled_shutdown_observer_retain_the_actual_issue() {
    let pause = Arc::new(IssuePause {
        entered: Notify::new(),
        release: Notify::new(),
        panic_after_commit: false,
    });
    let f = Harness::with_issue_pause(Some(pause.clone())).await;
    let (mut client, ready) = Client::open(f.address, &f.owner).await;
    assert_eq!(ready["ok"], true);
    let caller = tokio::spawn(async move { client.call("credential.issue", issue_params()).await });
    tokio::time::timeout(WAIT, pause.entered.notified())
        .await
        .unwrap();
    let credential = CredentialId::new(f.issued.lock().unwrap().keys().next().unwrap()).unwrap();
    assert!(
        f.store
            .registration(credential.clone())
            .await
            .unwrap()
            .is_none(),
        "actual Auth issue precedes installation"
    );
    let intent = f.store.intents().await.unwrap().pop().unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    let mut observer = Box::pin(f.runtime.shutdown());
    assert!(
        observer.as_mut().now_or_never().is_none(),
        "drain cannot confirm the paused issued command"
    );
    drop(observer);
    assert!(
        matches!(
            f.runtime.issue(intent.issue).await,
            Err(CredentialAdminError::Unavailable)
        ),
        "shutdown seals command admission"
    );
    pause.release.notify_one();
    tokio::time::timeout(WAIT, f.runtime.shutdown())
        .await
        .unwrap()
        .unwrap();
    assert!(
        f.store.registration(credential).await.unwrap().is_some(),
        "original retained command installs after both observers disappear"
    );
    f.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn first_issue_worker_panic_seals_admission_and_does_not_confirm_cleanup() {
    let pause = Arc::new(IssuePause {
        entered: Notify::new(),
        release: Notify::new(),
        panic_after_commit: true,
    });
    let f = Harness::with_issue_pause(Some(pause.clone())).await;
    let (mut client, ready) = Client::open(f.address, &f.owner).await;
    assert_eq!(ready["ok"], true);
    let caller = tokio::spawn(async move { client.call("credential.issue", issue_params()).await });
    tokio::time::timeout(WAIT, pause.entered.notified())
        .await
        .unwrap();
    let intent = f.store.intents().await.unwrap().pop().unwrap();
    let root = f._root.clone();
    let credential = CredentialId::new(&intent.issue.credential_id).unwrap();
    pause.release.notify_one();
    let refused = tokio::time::timeout(WAIT, caller).await.unwrap().unwrap();
    assert_eq!(refused["ok"], false);
    assert!(matches!(
        f.runtime.issue(intent.issue).await,
        Err(CredentialAdminError::Unavailable)
    ));
    let report = Mutex::new(None);
    crate::composition::cleanup_product(
        &report,
        &f.state,
        async { Ok(()) },
        async { Ok(()) },
        None::<Ready<Result<(), ConversationError>>>,
        async { Ok(()) },
        async { Ok(()) },
        WAIT,
    )
    .await;
    let report = report.into_inner().unwrap().unwrap();
    assert_eq!(
        report.webhooks(),
        &Outcome::Failed(CredentialAdminError::Unavailable)
    );
    assert!(
        !report.confirmed(),
        "host cleanup cannot replace this failed owner with successful no-work"
    );
    assert!(
        matches!(f.close().await, Err(CredentialAdminError::Unavailable)),
        "an issued but uninstalled command and worker panic remain failed cleanup"
    );
    // Actual cold open: old product/router/runtime/Auth/outbox handles have
    // dropped. No current seed is supplied; replay owns the original intent.
    let private = root.path().join("private");
    let credentials = Arc::new(
        LocalCredentialStore::open(
            &private,
            "auth.json",
            Arc::new(CedarPolicyEvaluator::new().unwrap()),
        )
        .unwrap(),
    );
    let clock = Arc::new(Time::new(2000));
    let store = Arc::new(
        SqlOutbox::open(
            &private.join("outbox.db"),
            private.join("audit"),
            clock.clone(),
        )
        .unwrap(),
    );
    let issued = Arc::new(Mutex::new(HashMap::new()));
    let base = Arc::new(BaseAdmin {
        store: credentials.clone(),
        issued: issued.clone(),
        pause: None,
    });
    let restored =
        WebhookRuntime::new(Vec::new(), base, credentials, store.clone(), clock).unwrap();
    restored.recover().await.unwrap();
    let registration = store.registration(credential).await.unwrap().unwrap();
    assert_eq!(registration.id, intent.id);
    assert_eq!(registration.destination, intent.config.destination);
    assert_eq!(
        registration.key.expose_bytes(),
        intent.config.key.expose_bytes()
    );
    assert!(
        issued.lock().unwrap().is_empty(),
        "cold replay must not issue another bearer"
    );
    restored.shutdown().await.unwrap();
}

fn ordinary_params() -> Value {
    let mut params = issue_params();
    params["requestId"] = json!("ordinary-product-issue");
    params["principal"]["id"] = json!("ordinary");
    params["membership"]["principalId"] = json!("ordinary");
    params["membership"]["id"] = json!("ordinary-member");
    params
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ordinary_issue_replay_and_revoke_keep_their_existing_product_contract() {
    let f = Harness::new().await;
    let (mut client, ready) = Client::open(f.address, &f.owner).await;
    assert_eq!(ready["ok"], true);
    let issued = client.call("credential.issue", ordinary_params()).await;
    assert_eq!(issued["ok"], true);
    assert!(issued["payload"]["secret"].as_str().is_some());
    let id = CredentialId::new(issued["payload"]["credential"]["id"].as_str().unwrap()).unwrap();
    assert!(f.store.registration(id.clone()).await.unwrap().is_none());
    assert!(f.store.intents().await.unwrap().is_empty());
    let replay = client.call("credential.issue", ordinary_params()).await;
    assert_eq!(replay["ok"], true);
    assert_eq!(
        replay["payload"]["credential"],
        issued["payload"]["credential"]
    );
    assert_eq!(replay["payload"]["secretUnavailable"], true);
    assert!(replay["payload"].get("secret").is_none());
    assert_eq!(f.issued.lock().unwrap().len(), 1);
    let revoked = client
        .call(
            "credential.revoke",
            json!({"requestId":"ordinary-revoke","credentialId":id.as_str()}),
        )
        .await;
    assert_eq!(revoked["ok"], true);
    assert!(f.store.intents().await.unwrap().is_empty());
    drop(client);
    f.close().await.unwrap();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configured_replay_preserves_identity_and_a_new_command_conflicts_before_reservation() {
    let f = Harness::new().await;
    let (mut client, ready) = Client::open(f.address, &f.owner).await;
    assert_eq!(ready["ok"], true);
    let first = client.call("credential.issue", issue_params()).await;
    assert_eq!(first["ok"], true);
    let replay = client.call("credential.issue", issue_params()).await;
    assert_eq!(replay["ok"], true);
    assert_eq!(replay["payload"], first["payload"]);
    let mut changed = issue_params();
    changed["requestId"] = json!("other-webhook-command");
    let refused = client.call("credential.issue", changed).await;
    assert_eq!(refused["ok"], false);
    assert_eq!(refused["error"]["code"], "credential_conflict");
    assert_eq!(
        f.issued.lock().unwrap().len(),
        1,
        "no second actual issuance"
    );
    assert_eq!(
        f.store.intents().await.unwrap().len(),
        1,
        "refusal precedes another durable reservation"
    );
    drop(client);
    f.close().await.unwrap();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configured_and_ordinary_product_issue_share_the_same_retained_issuance_gate() {
    let pause = Arc::new(IssuePause {
        entered: Notify::new(),
        release: Notify::new(),
        panic_after_commit: false,
    });
    let f = Harness::with_issue_pause(Some(pause.clone())).await;
    let (mut hook, ready) = Client::open(f.address, &f.owner).await;
    assert_eq!(ready["ok"], true);
    let hook = tokio::spawn(async move { hook.call("credential.issue", issue_params()).await });
    tokio::time::timeout(WAIT, pause.entered.notified())
        .await
        .unwrap();
    let (mut ordinary, ready) = Client::open(f.address, &f.owner).await;
    let ordinary =
        tokio::spawn(async move { ordinary.call("credential.issue", ordinary_params()).await });
    let admitted = tokio::time::timeout(WAIT, async {
        while f.runtime.ordinary_in_flight() != 2 {
            tokio::task::yield_now().await;
        }
    })
    .await;
    let before = f.issued.lock().unwrap().len();
    // Collect every observation before asserting, so even a regression releases
    // the original physical command and joins both real product callers.
    pause.release.notify_one();
    let hook = tokio::time::timeout(WAIT, hook).await.unwrap().unwrap();
    let ordinary = tokio::time::timeout(WAIT, ordinary).await.unwrap().unwrap();
    let after = f.issued.lock().unwrap().len();
    f.close().await.unwrap();
    assert_eq!(ready["ok"], true);
    assert!(
        admitted.is_ok(),
        "both real commands must reach retained admission"
    );
    assert_eq!(
        before, 1,
        "ordinary issuance must not overtake the paused configured command"
    );
    assert_eq!(hook["ok"], true);
    assert_eq!(ordinary["ok"], true);
    assert_eq!(after, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_cleanup_joins_the_retained_issue_and_seals_before_reader_drains() {
    let pause = Arc::new(IssuePause {
        entered: Notify::new(),
        release: Notify::new(),
        panic_after_commit: false,
    });
    let f = Harness::with_issue_pause(Some(pause.clone())).await;
    let (mut client, ready) = Client::open(f.address, &f.owner).await;
    assert_eq!(ready["ok"], true);
    let caller = tokio::spawn(async move {
        client
            .call_until_close("credential.issue", issue_params())
            .await
    });
    tokio::time::timeout(WAIT, pause.entered.notified())
        .await
        .unwrap();
    let intent = f.store.intents().await.unwrap().pop().unwrap();
    let report = Mutex::new(None);
    let reader_polled = AtomicBool::new(false);
    let mut cleanup = Box::pin(crate::composition::cleanup_product(
        &report,
        &f.state,
        async {
            reader_polled.store(true, Ordering::SeqCst);
            Ok(())
        },
        async { Ok(()) },
        None::<Ready<Result<(), ConversationError>>>,
        async { Ok(()) },
        async { Ok(()) },
        WAIT,
    ));
    let pending = cleanup.as_mut().now_or_never().is_none();
    let before = report
        .lock()
        .unwrap()
        .as_ref()
        .map(|r| r.webhooks().is_known());
    let admission = tokio::time::timeout(WAIT, f.runtime.issue(intent.issue.clone())).await;
    pause.release.notify_one();
    if pending {
        tokio::time::timeout(WAIT, cleanup).await.unwrap();
    } else {
        drop(cleanup);
    }
    let report: ShutdownReport = report.into_inner().unwrap().unwrap();
    let caller = tokio::time::timeout(WAIT, caller).await; // the host closes this socket; its observation may be lost
    let registered = f
        .store
        .registration(CredentialId::new(&intent.issue.credential_id).unwrap())
        .await
        .unwrap()
        .is_some();
    f.close().await.unwrap();
    assert!(pending);
    assert_eq!(before, Some(false));
    assert!(matches!(
        admission,
        Ok(Err(CredentialAdminError::Unavailable))
    ));
    assert!(
        reader_polled.load(Ordering::SeqCst),
        "other drains run while the actual command is retained"
    );
    let closed = caller
        .expect("the closed socket observer must end")
        .expect("the observer must not panic")
        .expect_err("host stop closes the socket before the paused command replies");
    assert_eq!(closed["closeCode"], 4010);
    assert_eq!(closed["reason"]["code"], "temporary_unavailable");
    assert!(registered);
    assert!(report.webhooks().is_ok());
    assert!(report.confirmed());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn product_comment_share_is_refused_without_creating_a_principal_grant() {
    let f = Harness::new().await;
    let (mut client, ready) = Client::open(f.address, &f.owner).await;
    assert_eq!(ready["ok"], true);
    let issued = client.call("credential.issue", issue_params()).await;
    assert_eq!(issued["ok"], true);
    let refused=client.call("conversation.share",json!({"requestId":"forbidden-comment","conversationId":f.a.to_string(),"credentialId":issued["payload"]["credential"]["id"],"capabilities":["read","comment"]})).await;
    assert_eq!(refused["ok"], false);
    assert_eq!(refused["error"]["code"], "capabilities_refused");
    assert!(f
        .grants
        .principal_grant(&f.a, &PrincipalId::new("hook").unwrap())
        .await
        .unwrap()
        .is_none());
    drop(client);
    f.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_cleanup_waits_for_physical_share_and_unshare_mutations() {
    for unshare in [false, true] {
        let f = Harness::new().await;
        let (mut client, ready) = Client::open(f.address, &f.owner).await;
        assert_eq!(ready["ok"], true);
        let issued = client.call("credential.issue", issue_params()).await;
        assert_eq!(issued["ok"], true);
        let credential = issued["payload"]["credential"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let share = json!({"requestId":"initial-share","conversationId":f.a.to_string(),"credentialId":credential,"capabilities":["read"]});
        if unshare {
            assert_eq!(
                client.call("conversation.share", share.clone()).await["ok"],
                true
            );
        }
        let pause = GrantWritePause::new();
        f.grants.pause_grant_writes(pause.clone());
        let method = if unshare {
            "conversation.unshare"
        } else {
            "conversation.share"
        };
        let params = if unshare {
            json!({"requestId":"retained-unshare","conversationId":f.a.to_string(),"credentialId":credential})
        } else {
            share
        };
        let caller = tokio::spawn(async move { client.call_until_close(method, params).await });
        let entered = {
            let pause = pause.clone();
            tokio::task::spawn_blocking(move || pause.wait_until_entered_timeout(WAIT))
                .await
                .unwrap()
        };
        let report = Mutex::new(None);
        let mut cleanup = Box::pin(crate::composition::cleanup_product(
            &report,
            &f.state,
            async { Ok(()) },
            async { Ok(()) },
            None::<Ready<Result<(), ConversationError>>>,
            async { Ok(()) },
            async { Ok(()) },
            WAIT,
        ));
        let early = tokio::time::timeout(Duration::from_secs(2), cleanup.as_mut()).await;
        let before = report
            .lock()
            .unwrap()
            .as_ref()
            .map(|r| r.webhooks().is_known());
        pause.release();
        if early.is_err() {
            tokio::time::timeout(WAIT, cleanup).await.unwrap();
        } else {
            drop(cleanup);
        }
        let caller = tokio::time::timeout(WAIT, caller).await;
        let grant = f
            .grants
            .principal_grant(&f.a, &PrincipalId::new("hook").unwrap())
            .await
            .unwrap();
        let report = report.into_inner().unwrap().unwrap();
        f.close().await.unwrap();
        assert!(
            entered,
            "the real canonical write must be held in its physical worker"
        );
        assert!(
            early.is_err(),
            "host cleanup completed before the physical grant mutation: unshare={unshare}"
        );
        assert_eq!(before, Some(false));
        assert!(
            caller.unwrap().is_ok(),
            "the socket observer must not panic"
        );
        assert_eq!(grant.is_some(), !unshare);
        assert!(report.confirmed());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ordinary_unpaired_targets_stay_unpaired_and_a_nonowner_cannot_share_the_hook() {
    let f = Harness::new().await;
    let (mut owner, ready) = Client::open(f.address, &f.owner).await;
    assert_eq!(ready["ok"], true);
    let hook = owner.call("credential.issue", issue_params()).await;
    assert_eq!(hook["ok"], true);
    let mut params = ordinary_params();
    params["principal"]["kind"] = json!("human");
    let ordinary = owner.call("credential.issue", params).await;
    assert_eq!(ordinary["ok"], true);
    let target = ordinary["payload"]["credential"]["id"].as_str().unwrap();
    let refused=owner.call("conversation.share",json!({"requestId":"unpaired-control","conversationId":f.a.to_string(),"credentialId":target,"capabilities":["read"]})).await;
    assert_eq!(refused["ok"], false);
    assert_eq!(refused["error"]["code"], "share_target_not_paired");
    let (mut ordinary_client, ready) =
        Client::open(f.address, ordinary["payload"]["secret"].as_str().unwrap()).await;
    assert_eq!(
        ready["ok"], true,
        "the control is an authenticated ordinary member"
    );
    let refused=ordinary_client.call("conversation.share",json!({"requestId":"nonowner-control","conversationId":f.a.to_string(),"credentialId":hook["payload"]["credential"]["id"],"capabilities":["read"]})).await;
    assert_eq!(refused["ok"], false);
    assert!(f
        .grants
        .principal_grant(&f.a, &PrincipalId::new("hook").unwrap())
        .await
        .unwrap()
        .is_none());
    assert!(f
        .grants
        .principal_grant(&f.a, &PrincipalId::new("ordinary").unwrap())
        .await
        .unwrap()
        .is_none());
    drop(ordinary_client);
    drop(owner);
    f.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deleted_conversation_unshare_preserves_webhook_and_departed_receiver_cleanup() {
    for webhook in [false, true] {
        let f = Harness::new().await;
        let (mut owner, ready) = Client::open(f.address, &f.owner).await;
        assert_eq!(ready["ok"], true);
        let issued = owner
            .call(
                "credential.issue",
                if webhook {
                    issue_params()
                } else {
                    ordinary_params()
                },
            )
            .await;
        assert_eq!(issued["ok"], true);
        let credential =
            CredentialId::new(issued["payload"]["credential"]["id"].as_str().unwrap()).unwrap();
        let principal = PrincipalId::new(if webhook { "hook" } else { "ordinary" }).unwrap();
        if webhook {
            assert_eq!(owner.call("conversation.share",json!({"requestId":"grant-before-deletion","conversationId":f.a.to_string(),"credentialId":credential.as_str(),"capabilities":["read"]})).await["ok"],true);
        } else {
            // Real canonical legacy grant whose receiver is no longer paired.
            // Unshare must remove it without requiring that departed binding.
            let receipt = f
                .grants
                .change(ShareGrantChange {
                    transition: ShareGrantTransition::Grant(ShareAccess::read()),
                    conversation_id: f.a.clone(),
                    principal_id: Some(principal.clone()),
                    receiver_id: Some("synthetic-departed-receiver".into()),
                    credential_id: credential.clone(),
                    expected_policy_revision: None,
                    initiator: ConversationCaller {
                        organization_id: OrganizationId::new("org").unwrap(),
                        principal_id: PrincipalId::new("owner").unwrap(),
                        surface_id: "synthetic".into(),
                        action_id: "canonical-legacy-grant".into(),
                    },
                    at_ms: 2_000,
                })
                .await
                .unwrap();
            assert!(receipt.applied);
        }
        assert!(f
            .grants
            .principal_grant(&f.a, &principal)
            .await
            .unwrap()
            .is_some());
        f.grants
            .record_deletion(
                &f.a,
                ConversationDeletion::new(
                    OrganizationId::new("org").unwrap(),
                    PrincipalId::new("owner").unwrap(),
                    "synthetic".into(),
                    "delete-owned-conversation".into(),
                    2_000,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let revoked=owner.call("conversation.unshare",json!({"requestId":"revoke-deleted","conversationId":f.a.to_string(),"credentialId":credential.as_str()})).await;
        let grant = f.grants.principal_grant(&f.a, &principal).await.unwrap();
        drop(owner);
        f.close().await.unwrap();
        assert_eq!(
            revoked["ok"], true,
            "deleted owner cleanup webhook={webhook}: {revoked}"
        );
        assert_eq!(revoked["payload"]["applied"], true);
        assert!(grant.is_none());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_action_attribution_is_refused_before_new_target_classification() {
    let f = Harness::new().await;
    let (mut owner, ready) = Client::open(f.address, &f.owner).await;
    assert_eq!(ready["ok"], true);
    let issued = owner.call("credential.issue", issue_params()).await;
    assert_eq!(issued["ok"], true);
    f.grants
        .record_deletion(
            &f.a,
            ConversationDeletion::new(
                OrganizationId::new("org").unwrap(),
                PrincipalId::new("owner").unwrap(),
                "synthetic".into(),
                "delete-for-attribution-control".into(),
                2_000,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    for method in ["conversation.share", "conversation.unshare"] {
        for conversation in [Uuid::new_v4().to_string(), f.a.to_string()] {
            for request in [
                String::new(),
                "invalid\nrequest".to_owned(),
                "x".repeat(Conversation::MAX_CREATOR_CONTEXT_BYTES + 1),
            ] {
                let mut params = json!({"requestId":request,"conversationId":conversation,"credentialId":issued["payload"]["credential"]["id"]});
                if method == "conversation.share" {
                    params["capabilities"] = json!(["read"]);
                }
                let refused = owner.call(method, params).await;
                assert_eq!(refused["ok"], false);
                assert_eq!(
                    refused["error"]["code"],
                    error_code(&ConversationError::InvalidInput).as_str(),
                    "attribution must precede resource/target lookup: {refused}"
                );
            }
        }
    }
    drop(owner);
    f.close().await.unwrap();
}

#[path = "product_delivery.rs"]
mod delivery;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fresh_public_issue_retires_an_expired_idle_registration_before_reissue() {
    let f = Harness::new().await;
    let (mut client, _) = Client::open(f.address, &f.owner).await;
    let first = client.call("credential.issue", issue_params()).await;
    assert_eq!(first["ok"], true, "{first}");
    let old = CredentialId::new(first["payload"]["credential"]["id"].as_str().unwrap()).unwrap();
    let original = f.store.intents().await.unwrap().pop().unwrap();
    assert!(f.store.registration(old.clone()).await.unwrap().is_some());
    f.clock.0.store(100_000, Ordering::SeqCst);
    let mut request = issue_params();
    request["requestId"] = json!("reissue-after-expiry");
    request["expiresAt"] = json!(200);
    let next = client.call("credential.issue", request).await;
    // Collect cleanup before assertions so a failing counterexample owns no server.
    let intents = f.store.intents().await.unwrap();
    let old_registration = f.store.registration(old.clone()).await.unwrap();
    let issued = f.issued.lock().unwrap().len();
    drop(client);
    f.close().await.unwrap();
    assert_eq!(next["ok"], true, "{next}");
    assert_ne!(next["payload"]["credential"]["id"], old.as_str());
    assert_eq!(issued, 2);
    assert!(
        old_registration.is_none(),
        "drained old registration is retired"
    );
    assert_eq!(intents.len(), 1);
    assert_ne!(intents[0].id, original.id);
    assert_eq!(intents[0].issue.request_id, "reissue-after-expiry");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cold_runtime_without_seeds_replays_webhook_history_before_ordinary_routing() {
    let f = Harness::new().await;
    let (mut client, _) = Client::open(f.address, &f.owner).await;
    let first = client.call("credential.issue", issue_params()).await;
    assert_eq!(first["ok"], true);
    let original = f.store.intents().await.unwrap().pop().unwrap();
    let root = f._root.clone();
    drop(client);
    f.close().await.unwrap();
    let private = root.path().join("private");
    let credentials = Arc::new(
        LocalCredentialStore::open(
            &private,
            "auth.json",
            Arc::new(CedarPolicyEvaluator::new().unwrap()),
        )
        .unwrap(),
    );
    let clock = Arc::new(Time::new(3000));
    let store = Arc::new(
        SqlOutbox::open(
            &private.join("outbox.db"),
            private.join("audit"),
            clock.clone(),
        )
        .unwrap(),
    );
    let issued = Arc::new(Mutex::new(HashMap::new()));
    let base = Arc::new(BaseAdmin {
        store: credentials.clone(),
        issued: issued.clone(),
        pause: None,
    });
    let runtime = WebhookRuntime::new(Vec::new(), base, credentials, store.clone(), clock).unwrap();
    runtime.recover().await.unwrap();
    let mut replay = original.issue.clone();
    replay.purpose = CredentialIssuePurpose::Product;
    replay.credential_id = Uuid::new_v4().to_string();
    replay.issued_at = 3;
    let replayed = match runtime.issue(replay.clone()).await {
        Ok(IssueCredentialOutcome::ExistingSecretUnavailable {
            metadata,
            transitions,
        }) => {
            metadata.id == original.issue.credential_id
                && metadata.issued_at == original.issue.issued_at
                && transitions.len() == 1
                && transitions[0].correlation.as_deref() == Some(original.issue.request_id.as_str())
        }
        _ => false,
    };
    let mut fresh = replay.clone();
    fresh.request_id = "no-seed-fresh-webhook-request".into();
    let refused = matches!(
        runtime.issue(fresh).await,
        Err(CredentialAdminError::Conflict)
    );
    let mut ordinary = replay;
    ordinary.request_id = "no-seed-ordinary-request".into();
    ordinary.credential_id = Uuid::new_v4().to_string();
    ordinary.principal.id = "ordinary-cold".into();
    ordinary.membership.id = "ordinary-cold-member".into();
    ordinary.membership.principal_id = "ordinary-cold".into();
    let ordinary_issued = matches!(
        runtime.issue(ordinary).await,
        Ok(IssueCredentialOutcome::Issued { .. })
    );
    let retained = store.intents().await.unwrap();
    let count = issued.lock().unwrap().len();
    runtime.shutdown().await.unwrap();
    assert!(
        replayed,
        "the original Webhook purpose/identity survives seed removal"
    );
    assert!(
        refused,
        "active webhook history cannot fall through to ordinary issuance"
    );
    assert!(
        ordinary_issued,
        "genuinely unregistered targets retain ordinary issuance"
    );
    assert_eq!(count, 1, "only the ordinary command created a new bearer");
    assert_eq!(retained.len(), 1);
    assert_eq!(retained[0].id, original.id);
}
