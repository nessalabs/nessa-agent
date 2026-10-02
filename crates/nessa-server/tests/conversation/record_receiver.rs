// Separate gateway and receiver processes exercise the authenticated product
// route. Receiver files are test data, never a second gateway journal.
use super::*;
use crate::agents::domain::AgentId;
use crate::conversation::application::{
    ConversationRepository, ReadRefusal, ReceiverAuthority, ReceiverBinding,
};
use crate::conversation::domain::{
    Conversation, ConversationApprovalMode, ConversationId, ConversationModelId,
};
use crate::conversation::infrastructure::{LocalConversationStore, NessaRecordReadSource};
use crate::product::generated::{ConversationRecordsHeadResult, ConversationRecordsPageResult};
use crate::product_contract::generated::RecordReadErrorCode;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use nessa_local_database::rusqlite::{params, Connection};
use nessa_sdk::application::agent_execution::providers::ProviderIdentity;
use nessa_sdk::application::agent_execution::sessions::{
    ProviderContext, SessionChange, SessionSaveGeneration, SessionSnapshot, SessionStorage,
};
use nessa_sdk::domain::agent_execution::sessions::{ExecutionSessionId, SessionId};
use nessa_sdk::infrastructure::session_storage::{
    RecordStorage, MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
};
use nessa_sync::replication::application::{RecordSource, SourceError};
use nessa_sync::replication::domain::{
    validate_page, Checkpoint, Id, Limits, Page, PageRequest, Record, Scope,
};
use serde_json::Value;
use std::future::Future;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::Path;
use std::pin::Pin;
use std::process::{Child, Command, Stdio};
use tokio::net::TcpListener;
use tokio::runtime::Handle;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

const GATEWAY_CHILD: &str = "product::socket::tests::record_receiver::gateway_child";
const RECEIVER_CHILD: &str = "product::socket::tests::record_receiver::receiver_child";
const CONVERSATION: &str = "018fa012-2222-7222-8222-222222222222";

fn sid(value: &str) -> Id {
    Id::new(value).unwrap()
}
struct Binding;
impl ReceiverAuthority for Binding {
    fn resolve<'a>(
        &'a self,
        _: &'a CredentialId,
    ) -> Pin<Box<dyn Future<Output = Result<Option<ReceiverBinding>, ReadRefusal>> + Send + 'a>>
    {
        Box::pin(async {
            Ok(Some(ReceiverBinding {
                receiver_id: "receiver".into(),
                credential_id: CredentialId::new("credential").unwrap(),
                organization_id: OrganizationId::new("organization").unwrap(),
                owner_id: PrincipalId::new("principal").unwrap(),
                access_epoch: 3,
                active: true,
            }))
        })
    }
}

/// The child starts the same HTTP router used by gateway composition. Its
/// credential/access ports are explicit fixture authorities; source and wire
/// transport are real. The writer lease remains live beside passive reads.
#[tokio::test]
async fn gateway_child() {
    let Ok(root) = std::env::var("NESSA_296_GATEWAY_ROOT") else {
        return;
    };
    let root = Path::new(&root);
    let private = root.join("conversations");
    nessa_local_storage::create_directory(&private).unwrap();
    let conversations =
        Arc::new(LocalConversationStore::open(&private.join("metadata.sqlite3")).unwrap());
    let conversation = ConversationId::new(CONVERSATION).unwrap();
    if conversations.load(&conversation).await.unwrap().is_none() {
        conversations
            .create(
                Conversation::new(
                    conversation,
                    OrganizationId::new("organization").unwrap(),
                    PrincipalId::new("principal").unwrap(),
                    "panel".into(),
                    "create".into(),
                    1,
                    AgentId::Claude,
                    ConversationModelId::new("model").unwrap(),
                    ConversationApprovalMode::Ask,
                )
                .unwrap(),
            )
            .await
            .unwrap();
    }
    let storage = Arc::new(RecordStorage::new(root.join("records")).unwrap());
    storage.initialize().await.unwrap();
    let session = SessionId::new(CONVERSATION).unwrap();
    let writer = storage.open(session.clone()).await.unwrap();
    if storage
        .record_identity(&session, sid("gateway-resource"))
        .await
        .unwrap()
        .is_none()
    {
        panic!("opening the writer must establish stream identity");
    }
    // A seed marker belongs only to fixture setup. Reopening the child must
    // retain the canonical stream rather than seed it again.
    if !root.join("seeded").exists() {
        let provider = ProviderIdentity::new("fixture", "model", "workspace").unwrap();
        let snapshot = SessionSnapshot {
            id: session.clone(),
            provider: provider.clone(),
            provider_context: ProviderContext::Absent,
            invocations: vec![],
            queue_history: vec![],
        };
        writer
            .save_changes(
                SessionSaveGeneration::initial(),
                snapshot.clone(),
                vec![SessionChange::Opened {
                    id: session.clone(),
                    provider,
                    context: ProviderContext::Absent,
                }],
            )
            .await
            .unwrap();
        let mut changes = Vec::new();
        for index in 0..800 {
            let context = ProviderContext::Recorded(
                ExecutionSessionId::new(format!("remote-{index:04}-{}", "x".repeat(120))).unwrap(),
            );
            changes.push(SessionChange::ProviderContext {
                before: ProviderContext::Absent,
                after: context.clone(),
            });
            changes.push(SessionChange::ProviderContext {
                before: context,
                after: ProviderContext::Absent,
            });
        }
        let mut generation = SessionSaveGeneration::initial().checked_next().unwrap();
        writer
            .save_changes(generation, snapshot.clone(), changes)
            .await
            .unwrap();
        for index in 0..20 {
            generation = generation.checked_next().unwrap();
            let context = ProviderContext::Recorded(
                ExecutionSessionId::new(format!("tail-{index}")).unwrap(),
            );
            writer
                .save_changes(
                    generation,
                    snapshot.clone(),
                    vec![
                        SessionChange::ProviderContext {
                            before: ProviderContext::Absent,
                            after: context.clone(),
                        },
                        SessionChange::ProviderContext {
                            before: context,
                            after: ProviderContext::Absent,
                        },
                    ],
                )
                .await
                .unwrap();
        }
        std::fs::write(root.join("seeded"), b"seeded").unwrap();
    }
    let (state, authority) = fixture(MembershipRole::Member);
    {
        let mut snapshot = authority.snapshot.lock().unwrap();
        snapshot.credential = Credential::new(
            CredentialId::new("credential").unwrap(),
            PrincipalId::new("principal").unwrap(),
            OrganizationId::new("organization").unwrap(),
            AudienceId::new("gateway").unwrap(),
            100,
            200,
            vec![Grant::new(
                Action::new("conversation.read").unwrap(),
                Resource::new(
                    OrganizationId::new("organization").unwrap(),
                    ResourceId::new("gateway-resource").unwrap(),
                ),
            )],
        )
        .unwrap();
    }
    let source = Arc::new(NessaRecordReadSource::new(
        storage,
        sid("gateway-resource"),
        Handle::current(),
    ));
    let state = state
        .with_passive_read(Arc::new(Binding), conversations)
        .with_record_source(source);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    println!(
        "\nNESSA_296_READY {}",
        listener.local_addr().unwrap().port()
    );
    std::io::stdout().flush().unwrap();
    axum::serve(listener, crate::server::entrypoint::http::router(state))
        .await
        .unwrap();
    drop(writer);
}

struct ProductSource {
    socket: WebSocket<MaybeTlsStream<TcpStream>>,
    next: u64,
}
impl ProductSource {
    fn connect(port: u16) -> Self {
        let (socket, _) = tungstenite::connect(format!("ws://127.0.0.1:{port}/session")).unwrap();
        let mut source = Self { socket, next: 0 };
        let challenge = source.json();
        let nonce = challenge["payload"]["nonce"].as_str().unwrap();
        let reply = source.call("session.authenticate", json!({"minVersion":1,"maxVersion":1,"nonce":nonce,"credential":"secret","client":{"id":"receiver"}}));
        assert_eq!(reply["ok"], true, "{reply}");
        source
    }
    fn json(&mut self) -> Value {
        loop {
            match self.socket.read().unwrap() {
                Message::Text(text) => return serde_json::from_str(&text).unwrap(),
                Message::Ping(bytes) => self.socket.send(Message::Pong(bytes)).unwrap(),
                value => panic!("unexpected product frame {value:?}"),
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
            let response = self.json();
            if response["id"] == id {
                return response;
            }
        }
    }
    fn discover(&mut self) -> Result<(Scope, u64), RecordReadErrorCode> {
        let reply = self.call(
            "conversation.recordsHead",
            json!({"conversationId":CONVERSATION,"accessEpoch":"3","receiverId":"receiver"}),
        );
        if reply["ok"] != true {
            return Err(serde_json::from_value(reply["error"]["code"].clone()).unwrap());
        }
        let response: ConversationRecordsHeadResult =
            serde_json::from_value(reply["payload"].clone()).unwrap();
        Ok((
            crate::product::passive_read::wire::decode_scope(&response.scope).unwrap(),
            response.head.parse().unwrap(),
        ))
    }
    fn request(scope: &Scope) -> Value {
        serde_json::to_value(crate::product::passive_read::wire::wire_scope(scope)).unwrap()
    }
}
impl RecordSource for ProductSource {
    fn head(&mut self, scope: &Scope) -> Result<u64, SourceError> {
        let (actual, head) = self.discover().map_err(|_| SourceError::Unavailable)?;
        if &actual != scope {
            return Err(SourceError::IdentityChanged);
        }
        Ok(head)
    }
    fn page(&mut self, request: &PageRequest) -> Result<Page, SourceError> {
        let reply = self.call("conversation.recordsPage", json!({"conversationId":CONVERSATION,"accessEpoch":"3","request":{"scope":Self::request(&request.scope),"after":request.after.to_string(),"target":request.target.to_string(),"maxRecords":request.max_records,"maxPayloadBytes":request.max_payload_bytes,"maxRecordBytes":request.max_record_bytes}}));
        if reply["ok"] != true {
            return Err(SourceError::Unavailable);
        }
        let response: ConversationRecordsPageResult =
            serde_json::from_value(reply["payload"].clone()).unwrap();
        let echoed =
            crate::product::record_read::wire::decode_page_request(&response.request).unwrap();
        let records = response
            .records
            .into_iter()
            .map(|r| Record {
                position: r.position.parse().unwrap(),
                id: sid(&r.id),
                scope: echoed.scope.clone(),
                payload: STANDARD.decode(r.payload).unwrap(),
            })
            .collect();
        Ok(Page {
            request: echoed,
            records,
        })
    }
}

struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
struct Gateway {
    _child: ChildGuard,
    port: u16,
}
impl Gateway {
    fn start(root: &Path) -> Self {
        let mut child = ChildGuard(
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", GATEWAY_CHILD, "--nocapture"])
                .env("NESSA_296_GATEWAY_ROOT", root)
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap(),
        );
        let mut output = BufReader::new(child.0.stdout.take().unwrap());
        let mut line = String::new();
        loop {
            line.clear();
            assert_ne!(
                output.read_line(&mut line).unwrap(),
                0,
                "gateway did not become ready"
            );
            if let Some(value) = line.trim().strip_prefix("NESSA_296_READY ") {
                return Self {
                    _child: child,
                    port: value.parse().unwrap(),
                };
            }
        }
    }
    fn receiver(&self, db: &Path, mode: &str) {
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", RECEIVER_CHILD, "--nocapture"])
            .env("NESSA_296_RECEIVER_PORT", self.port.to_string())
            .env("NESSA_296_RECEIVER_DB", db)
            .env("NESSA_296_RECEIVER_MODE", mode)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn receiver_child() {
    let Ok(port) = std::env::var("NESSA_296_RECEIVER_PORT") else {
        return;
    };
    let mode = std::env::var("NESSA_296_RECEIVER_MODE").unwrap();
    let mut source = ProductSource::connect(port.parse().unwrap());
    if mode == "refusals" {
        for (receiver, epoch, expected) in [
            ("another-receiver", "3", RecordReadErrorCode::WrongReceiver),
            ("receiver", "2", RecordReadErrorCode::StaleEpoch),
        ] {
            let reply = source.call(
                "conversation.recordsHead",
                json!({
                    "conversationId": CONVERSATION,
                    "receiverId": receiver,
                    "accessEpoch": epoch,
                }),
            );
            assert_eq!(reply["ok"], false, "{reply}");
            let error: RecordReadErrorCode =
                serde_json::from_value(reply["error"]["code"].clone()).unwrap();
            assert_eq!(error, expected);
            assert!(reply.get("payload").is_none(), "{reply}");
            assert!(reply["error"].get("scope").is_none(), "{reply}");
            assert!(reply["error"].get("head").is_none(), "{reply}");
        }
        return;
    }
    let mut preparing = 0;
    let (scope, target) = loop {
        match source.discover() {
            Ok(actual) => break actual,
            Err(RecordReadErrorCode::SourcePreparing) => {
                preparing += 1;
                assert!(preparing < 100);
            }
            Err(error) => panic!("discovery refused: {error:?}"),
        }
    };
    if mode == "lost-page" {
        assert!(
            preparing > 0,
            "cold large history must report preparing before a head"
        );
    }
    let mut db = Connection::open(std::env::var("NESSA_296_RECEIVER_DB").unwrap()).unwrap();
    db.execute_batch("CREATE TABLE IF NOT EXISTS progress (singleton INTEGER PRIMARY KEY CHECK(singleton=1), scope TEXT NOT NULL, downloaded INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS records(position INTEGER PRIMARY KEY,event_id TEXT NOT NULL UNIQUE,payload BLOB NOT NULL);").unwrap();
    let scope_wire = ProductSource::request(&scope).to_string();
    db.execute(
        "INSERT OR IGNORE INTO progress VALUES(1,?1,0)",
        [&scope_wire],
    )
    .unwrap();
    let (saved, downloaded): (String, i64) = db
        .query_row("SELECT scope,downloaded FROM progress", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(saved, scope_wire);
    let mut downloaded = u64::try_from(downloaded).unwrap();
    while downloaded < target {
        let request = PageRequest {
            scope: scope.clone(),
            after: downloaded,
            target,
            max_records: 1,
            max_payload_bytes: MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
            max_record_bytes: MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
        };
        let page = source.page(&request).unwrap();
        let plan = validate_page(
            &Checkpoint::new(scope.clone(), downloaded),
            &request,
            page,
            Limits::new(
                1,
                MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
                MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
            )
            .unwrap(),
        )
        .unwrap();
        if mode == "lost-page" {
            return;
        }
        let (_, next, records) = plan.into_parts();
        let tx = db.transaction().unwrap();
        for record in records {
            tx.execute(
                "INSERT INTO records VALUES(?1,?2,?3)",
                params![
                    i64::try_from(record.position).unwrap(),
                    record.id.as_str(),
                    record.payload
                ],
            )
            .unwrap();
        }
        assert_eq!(
            tx.execute(
                "UPDATE progress SET downloaded=?1 WHERE singleton=1 AND downloaded=?2",
                params![
                    i64::try_from(next.position()).unwrap(),
                    i64::try_from(downloaded).unwrap()
                ]
            )
            .unwrap(),
            1
        );
        tx.commit().unwrap();
        downloaded = next.position();
        if mode == "partial" {
            return;
        }
    }
    assert_eq!(source.head(&scope).unwrap(), target);
}

#[test]
fn authenticated_product_processes_resume_download_after_lost_page_and_both_restarts() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("gateway");
    let db = directory.path().join("receiver.sqlite3");
    let gateway = Gateway::start(&root);
    gateway.receiver(&db, "refusals");
    assert!(!db.exists(), "refused selectors create no receiver cache");
    gateway.receiver(&db, "lost-page");
    let read = || {
        Connection::open(&db)
            .unwrap()
            .query_row("SELECT downloaded FROM progress", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap()
    };
    assert_eq!(read(), 0, "lost answer does not advance durable download");
    gateway.receiver(&db, "partial");
    assert_eq!(read(), 1);
    drop(gateway);
    let restarted = Gateway::start(&root);
    restarted.receiver(&db, "resume");
    let completed = read();
    assert!(completed > 2, "seed includes a multi-frame semantic fact");
    restarted.receiver(&db, "resume");
    assert_eq!(read(), completed);
}
