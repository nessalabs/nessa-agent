//! Public product commands through canonical storage and actual TLS POST bytes.
use super::*;
use crate::conversation::{
    application::{
        PrincipalRecordReadSource, PrincipalRecordScope, RecordReadError, RecordReadFuture,
        RecordReadLease, RecordReadOperation, RecordReadResponse,
    },
    infrastructure::NessaRecordReadSource,
};
use crate::webhooks::application::{
    Message as SignedMessage, Outbox, PrepareFailure, WebhookHttp, WebhookPost,
};
use crate::webhooks::infrastructure::tls_fixture::RecipientTls;
use crate::webhooks::infrastructure::{
    isolated_post, signature, DeliveryDependencies, MAX_IN_FLIGHT,
};
use async_trait::async_trait;
use nessa_sdk::{
    application::agent_execution::{
        providers::ProviderIdentity,
        sessions::{ProviderContext, SessionChange, SessionSaveUnit, SessionStorage},
    },
    domain::agent_execution::sessions::SessionId,
    infrastructure::session_storage::RecordStorage,
};
use nessa_sync::replication::domain::Id;
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener},
    thread,
};
use tokio::sync::{mpsc, Semaphore};

struct Transport {
    address: SocketAddr,
    root: Vec<u8>,
    capacity: Arc<Semaphore>,
}
#[async_trait]
impl WebhookHttp for Transport {
    async fn prepare(
        &self,
        destination: &Destination,
    ) -> Result<Box<dyn WebhookPost>, PrepareFailure> {
        assert_eq!(
            destination.expose_url().as_str(),
            "https://recipient.test/synthetic"
        );
        isolated_post(self.address, &self.root, self.capacity.clone())
    }
}
struct Recipient {
    transport: Arc<Transport>,
    requests: mpsc::Receiver<Vec<u8>>,
    stop: Arc<AtomicBool>,
    task: Option<thread::JoinHandle<()>>,
}
impl Recipient {
    fn new(always_retry: bool) -> Self {
        Self::with_response_pause(always_retry, None)
    }
    fn with_response_pause(
        always_retry: bool,
        pause: Option<(Arc<Notify>, std::sync::mpsc::Receiver<()>)>,
    ) -> Self {
        crate::agent_install::infrastructure::https_archives::install_tls_backend();
        let tls = RecipientTls::generate();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let root = tls.root.to_vec();
        let (tx, requests) = mpsc::channel(16);
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let task = thread::spawn(move || {
            let mut count = 0;
            while !stopping.load(Ordering::SeqCst) {
                let stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::park_timeout(Duration::from_millis(5));
                        continue;
                    }
                    Err(error) => panic!("{error}"),
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut stream = rustls::StreamOwned::new(
                    rustls::ServerConnection::new(Arc::new(tls.config.clone())).unwrap(),
                    stream,
                );
                let mut bytes = Vec::new();
                loop {
                    let mut buffer = [0; 1024];
                    let len = stream.read(&mut buffer).unwrap();
                    assert!(len > 0, "recipient request ended early");
                    bytes.extend_from_slice(&buffer[..len]);
                    assert!(bytes.len() <= crate::webhooks::application::MAX_BODY_BYTES + 8192);
                    if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = std::str::from_utf8(&bytes[..end])
                            .unwrap()
                            .to_ascii_lowercase();
                        let length = headers
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length: "))
                            .unwrap()
                            .parse::<usize>()
                            .unwrap();
                        if bytes.len() == end + 4 + length {
                            break;
                        }
                    }
                }
                if count == 0 {
                    if let Some((entered, release)) = &pause {
                        entered.notify_one();
                        release
                            .recv_timeout(Duration::from_secs(3))
                            .expect("fixture response released within watchdog");
                    }
                }
                count += 1;
                let response = if always_retry || count == 1 {
                    b"HTTP/1.1 503 Retry\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        .as_slice()
                } else {
                    b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        .as_slice()
                };
                stream.write_all(response).unwrap();
                stream.flush().unwrap();
                tx.try_send(bytes)
                    .expect("bounded fixture request observations");
            }
        });
        Self {
            transport: Arc::new(Transport {
                address,
                root,
                capacity: Arc::new(Semaphore::new(MAX_IN_FLIGHT)),
            }),
            requests,
            stop,
            task: Some(task),
        }
    }
    async fn next(&mut self) -> Vec<u8> {
        tokio::time::timeout(WAIT, self.requests.recv())
            .await
            .unwrap()
            .unwrap()
    }
}
impl Drop for Recipient {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(task) = self.task.take() {
            let joined = task.join();
            if !thread::panicking() {
                joined.unwrap();
            }
        }
    }
}
fn checked_request(bytes: &[u8], conversation: &ConversationId) -> (String, u64, String, Vec<u8>) {
    let end = bytes.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let headers = std::str::from_utf8(&bytes[..end]).unwrap();
    assert!(headers.starts_with("POST /synthetic HTTP/1.1\r\n"));
    let fields: HashMap<_, _> = headers
        .lines()
        .skip(1)
        .map(|line| {
            let (key, value) = line.split_once(": ").unwrap();
            (key.to_ascii_lowercase(), value)
        })
        .collect();
    let body = bytes[end + 4..].to_vec();
    let decoded: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(decoded["conversationId"], conversation.to_string());
    let id = fields["nessa-webhook-id"].to_string();
    let message = SignedMessage::encoded(
        Uuid::parse_str(&id).unwrap(),
        fields["nessa-webhook-timestamp"].parse().unwrap(),
        body.clone(),
    )
    .unwrap();
    assert_eq!(
        fields["nessa-webhook-signature"],
        signature(&MacKey::new([7; 32]), &message)
    );
    (
        id,
        message.timestamp_ms(),
        fields["nessa-webhook-signature"].to_string(),
        body,
    )
}
async fn mount(f: &Harness, http: Arc<dyn WebhookHttp>) {
    let storage = Arc::new(RecordStorage::new(f._root.path().join("sessions")).unwrap());
    storage.initialize().await.unwrap();
    for conversation in [&f.a, &f.b] {
        let session = SessionId::new(conversation.to_string()).unwrap();
        let writer = storage.open(session.clone()).await.unwrap();
        writer
            .save_changes(
                writer.load().await.unwrap().binding().clone(),
                vec![SessionSaveUnit::new(vec![SessionChange::Opened {
                    id: session,
                    provider: ProviderIdentity::new("synthetic", "model", "workspace").unwrap(),
                    context: ProviderContext::Absent,
                }])
                .unwrap()],
            )
            .await
            .unwrap();
    }
    let source = Arc::new(NessaRecordReadSource::new(
        storage,
        Id::new("synthetic-source").unwrap(),
        tokio::runtime::Handle::current(),
    ));
    f.runtime
        .start_delivery(DeliveryDependencies {
            metadata: f.grants.clone(),
            source,
            policy: Arc::new(CedarConversationAccessPolicy::new().unwrap()),
            capacity: f.state.record_reads.clone(),
            gateway: f.state.gateway.clone(),
            http,
        })
        .unwrap();
}
async fn issue_share(f: &Harness, client: &mut Client) -> String {
    let reply = client.call("credential.issue", issue_params()).await;
    assert_eq!(reply["ok"], true, "{reply}");
    let credential = reply["payload"]["credential"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let shared=client.call("conversation.share",json!({"requestId":"delivery-share","conversationId":f.a.to_string(),"credentialId":credential,"capabilities":["read"]})).await;
    assert_eq!(shared["ok"], true, "{shared}");
    credential
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn public_grant_delivers_only_its_chat_with_real_tls_signature_and_stable_retry() {
    let f = Harness::new().await;
    let mut recipient = Recipient::new(false);
    mount(&f, recipient.transport.clone()).await;
    let (mut client, ready) = Client::open(f.address, &f.owner).await;
    assert_eq!(ready["ok"], true);
    issue_share(&f, &mut client).await;
    let first = checked_request(&recipient.next().await, &f.a);
    let second = checked_request(&recipient.next().await, &f.a);
    assert_ne!(first.0, second.0);
    f.clock.0.store(4000, Ordering::SeqCst);
    f.runtime.wake_delivery();
    let retry = checked_request(&recipient.next().await, &f.a);
    assert_eq!(retry, first);
    drop(client);
    assert!(f.close().await.is_ok());
    assert!(recipient.requests.try_recv().is_err());
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn public_unshare_refuses_actual_due_retries_before_another_post() {
    let f = Harness::new().await;
    let mut recipient = Recipient::new(true);
    mount(&f, recipient.transport.clone()).await;
    let (mut client, _) = Client::open(f.address, &f.owner).await;
    let credential = issue_share(&f, &mut client).await;
    checked_request(&recipient.next().await, &f.a);
    checked_request(&recipient.next().await, &f.a);
    let removed=client.call("conversation.unshare",json!({"requestId":"delivery-unshare","conversationId":f.a.to_string(),"credentialId":credential})).await;
    assert_eq!(removed["ok"], true, "{removed}");
    let before = f.runtime.completed_delivery_cycles();
    f.clock.0.store(4000, Ordering::SeqCst);
    f.runtime.wake_delivery();
    tokio::time::timeout(WAIT, f.runtime.after_delivery_cycle(before))
        .await
        .unwrap();
    let fresh = f.runtime.completed_delivery_cycles();
    f.runtime.wake_delivery();
    tokio::time::timeout(WAIT, f.runtime.after_delivery_cycle(fresh))
        .await
        .unwrap();
    assert!(f.store.due(4000).await.unwrap().is_empty());
    assert!(recipient.requests.try_recv().is_err());
    drop(client);
    assert!(f.close().await.is_ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn public_revoke_expiry_and_regrant_refuse_the_old_due_epoch_without_post() {
    for action in ["revoke", "expiry", "regrant"] {
        let f = Harness::new().await;
        let mut recipient = Recipient::new(true);
        mount(&f, recipient.transport.clone()).await;
        let (mut client, _) = Client::open(f.address, &f.owner).await;
        let credential = issue_share(&f, &mut client).await;
        checked_request(&recipient.next().await, &f.a);
        checked_request(&recipient.next().await, &f.a);
        match action {
            "revoke" => {
                let reply = client
                    .call(
                        "credential.revoke",
                        json!({"requestId":"delivery-revoke","credentialId":credential}),
                    )
                    .await;
                assert_eq!(reply["ok"], true, "{reply}");
            }
            "regrant" => {
                let before = f
                    .grants
                    .principal_grant(&f.a, &PrincipalId::new("hook").unwrap())
                    .await
                    .unwrap()
                    .unwrap()
                    .policy_revision;
                let reply = client.call("conversation.unshare", json!({"requestId":"delivery-unshare","conversationId":f.a.to_string(),"credentialId":credential})).await;
                assert_eq!(reply["ok"], true, "{reply}");
                let reply = client.call("conversation.share", json!({"requestId":"delivery-regrant","conversationId":f.a.to_string(),"credentialId":credential,"capabilities":["read"]})).await;
                assert_eq!(reply["ok"], true, "{reply}");
                let after = f
                    .grants
                    .principal_grant(&f.a, &PrincipalId::new("hook").unwrap())
                    .await
                    .unwrap()
                    .unwrap()
                    .policy_revision;
                assert!(after > before);
            }
            "expiry" => {}
            _ => unreachable!(),
        }
        let now = if action == "expiry" { 100_000 } else { 4000 };
        let before = f.runtime.completed_delivery_cycles();
        f.clock.0.store(now, Ordering::SeqCst);
        f.runtime.wake_delivery();
        tokio::time::timeout(WAIT, f.runtime.after_delivery_cycle(before))
            .await
            .unwrap();
        // The first completed cycle can have begun before the clock edge.
        // Require one more real cycle, started after that edge, without sleeps.
        let fresh = f.runtime.completed_delivery_cycles();
        f.runtime.wake_delivery();
        tokio::time::timeout(WAIT, f.runtime.after_delivery_cycle(fresh))
            .await
            .unwrap();
        assert!(f.store.due(now).await.unwrap().is_empty(), "{action}");
        assert!(recipient.requests.try_recv().is_err(), "{action}");
        drop(client);
        assert!(f.close().await.is_ok(), "{action}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_response_survives_failed_publication_without_repeating_the_post() {
    let f = Harness::new().await;
    let entered = Arc::new(Notify::new());
    let (release, wait) = std::sync::mpsc::channel();
    let mut recipient = Recipient::with_response_pause(true, Some((entered.clone(), wait)));
    mount(&f, recipient.transport.clone()).await;
    let (mut client, _) = Client::open(f.address, &f.owner).await;
    issue_share(&f, &mut client).await;
    tokio::time::timeout(WAIT, entered.notified())
        .await
        .unwrap();
    let audit = f._root.path().join("private/audit");
    let held = audit.with_extension("held");
    std::fs::rename(&audit, &held).unwrap();
    let before = f.runtime.completed_delivery_cycles();
    release.send(()).unwrap();
    let original = checked_request(&recipient.next().await, &f.a);
    tokio::time::timeout(WAIT, f.runtime.after_delivery_cycle(before))
        .await
        .unwrap();
    // A real complete response has ended; the private transition committed,
    // while publication cannot reach its actual directory. No extra POST is safe.
    let connection =
        nessa_local_database::rusqlite::Connection::open(f._root.path().join("private/outbox.db"))
            .unwrap();
    let retries: i64 = connection
        .query_row(
            "SELECT count(*) FROM deliveries WHERE phase='retry'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        retries, 1,
        "the actual response committed before publication failed"
    );
    drop(connection);
    std::fs::rename(&held, &audit).unwrap();
    let fresh = f.runtime.completed_delivery_cycles();
    f.runtime.wake_delivery();
    tokio::time::timeout(WAIT, f.runtime.after_delivery_cycle(fresh))
        .await
        .unwrap();
    let next = checked_request(&recipient.next().await, &f.a);
    assert_ne!(
        next.0, original.0,
        "live reconciliation may send the next record but never repeat the unacknowledged original"
    );
    drop(client);
    assert!(
        f.close().await.is_ok(),
        "known response must reconcile through actual publication and join"
    );
    assert!(
        recipient.requests.try_recv().is_err(),
        "a locally pending response is not a second remote attempt"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_unacknowledged_response_and_cursor_survive_cold_restart_without_new_issuance() {
    let f = Harness::new().await;
    let root = f._root.clone();
    let conversation = f.a.clone();
    let entered = Arc::new(Notify::new());
    let (release, wait) = std::sync::mpsc::channel();
    let mut recipient = Recipient::with_response_pause(true, Some((entered.clone(), wait)));
    mount(&f, recipient.transport.clone()).await;
    let (mut client, _) = Client::open(f.address, &f.owner).await;
    let credential = issue_share(&f, &mut client).await;
    tokio::time::timeout(WAIT, entered.notified())
        .await
        .unwrap();
    let audit = root.path().join("private/audit");
    let held = audit.with_extension("held");
    std::fs::rename(&audit, &held).unwrap();
    let before = f.runtime.completed_delivery_cycles();
    release.send(()).unwrap();
    let original = checked_request(&recipient.next().await, &conversation);
    tokio::time::timeout(WAIT, f.runtime.after_delivery_cycle(before))
        .await
        .unwrap();
    drop(client);
    assert_eq!(
        f.close().await,
        Err(CredentialAdminError::Unavailable),
        "unacknowledged physical response prevents confirmed live cleanup"
    );
    std::fs::rename(&held, &audit).unwrap();
    // All prior runtime/product/Auth/outbox handles are now dropped. Reopen the
    // actual private stores and physical SDK history; no current seed is supplied.
    let cold = cold_runtime(root.path(), 4000, recipient.transport.clone()).await;
    let registration = cold
        .store
        .registration(CredentialId::new(credential).unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(registration.key.expose_bytes(), &[7; 32]);
    let retried = checked_request(&recipient.next().await, &conversation);
    assert_eq!(
        retried, original,
        "cold retry preserves actual original identity, timestamp, signature and bytes"
    );
    let next = checked_request(&recipient.next().await, &conversation);
    assert_ne!(
        next.0, original.0,
        "committed cursor resumes the second record rather than creating the first again"
    );
    cold.runtime.shutdown().await.unwrap();
    assert!(
        cold.issued.lock().unwrap().is_empty(),
        "cold restoration issues no credential"
    );
    assert!(recipient.requests.try_recv().is_err());
}

struct PreparingSource(Arc<Notify>);
impl PrincipalRecordReadSource for PreparingSource {
    fn read_principal<'a>(
        &'a self,
        _: PrincipalRecordScope,
        _: RecordReadOperation,
        _: RecordReadLease,
    ) -> RecordReadFuture<'a, RecordReadResponse> {
        Box::pin(async move {
            self.0.notify_one();
            Err(RecordReadError::SourcePreparing)
        })
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_preparing_source_pauses_without_failing_the_shared_command_owner_or_sending() {
    let f = Harness::new().await;
    let mut recipient = Recipient::new(true);
    let entered = Arc::new(Notify::new());
    f.runtime
        .start_delivery(DeliveryDependencies {
            metadata: f.grants.clone(),
            source: Arc::new(PreparingSource(entered.clone())),
            policy: Arc::new(CedarConversationAccessPolicy::new().unwrap()),
            capacity: f.state.record_reads.clone(),
            gateway: f.state.gateway.clone(),
            http: recipient.transport.clone(),
        })
        .unwrap();
    let (mut client, _) = Client::open(f.address, &f.owner).await;
    issue_share(&f, &mut client).await;
    tokio::time::timeout(WAIT, entered.notified())
        .await
        .unwrap();
    assert!(f.store.due(4000).await.unwrap().is_empty());
    assert!(recipient.requests.try_recv().is_err());
    drop(client);
    assert!(
        f.close().await.is_ok(),
        "bounded source discovery is a pause, not a supervisor failure"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mounted_capability_refusal_creates_no_cursor_delivery_or_post() {
    let f = Harness::new().await;
    let mut recipient = Recipient::new(true);
    mount(&f, recipient.transport.clone()).await;
    let (mut client, _) = Client::open(f.address, &f.owner).await;
    let issued = client.call("credential.issue", issue_params()).await;
    assert_eq!(issued["ok"], true);
    let refused = client.call("conversation.share", json!({"requestId":"delivery-forbidden-comment","conversationId":f.a.to_string(),"credentialId":issued["payload"]["credential"]["id"],"capabilities":["read","comment"]})).await;
    assert_eq!(refused["ok"], false);
    assert!(f
        .grants
        .principal_grant(&f.a, &PrincipalId::new("hook").unwrap())
        .await
        .unwrap()
        .is_none());
    let before = f.runtime.completed_delivery_cycles();
    f.runtime.wake_delivery();
    tokio::time::timeout(WAIT, f.runtime.after_delivery_cycle(before))
        .await
        .unwrap();
    f.runtime.shutdown().await.unwrap();
    let connection =
        nessa_local_database::rusqlite::Connection::open(f._root.path().join("private/outbox.db"))
            .unwrap();
    for table in ["cursors", "deliveries"] {
        let count: i64 = connection
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0, "{table}");
    }
    drop(connection);
    assert!(recipient.requests.try_recv().is_err());
    drop(client);
    f.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_post_holds_revocation_and_production_cleanup_joins_it_before_confirmation() {
    let f = Harness::new().await;
    let entered = Arc::new(Notify::new());
    let (release, wait) = std::sync::mpsc::channel();
    let mut recipient = Recipient::with_response_pause(true, Some((entered.clone(), wait)));
    mount(&f, recipient.transport.clone()).await;
    let (mut client, _) = Client::open(f.address, &f.owner).await;
    let credential = issue_share(&f, &mut client).await;
    tokio::time::timeout(WAIT, entered.notified())
        .await
        .unwrap();
    // This canonical adapter future polls its actual write-fence acquisition
    // directly, before any task spawn or registry publication.
    let mut revoke = f.runtime.credentials().revoke(RevokeCredentialRequest {
        request_id: "delivery-held-revoke".into(),
        issuer_principal_id: "owner".into(),
        credential_id: credential,
        revoked_at: 2,
    });
    assert!(
        revoke.as_mut().now_or_never().is_none(),
        "actual HTTP ownership keeps the canonical revoke fence pending"
    );
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
    assert!(
        cleanup.as_mut().now_or_never().is_none(),
        "production cleanup must join the physical POST"
    );
    assert!(!report
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .webhooks()
        .is_known());
    let early = tokio::time::timeout(Duration::from_secs(2), cleanup.as_mut()).await;
    let known_before_release = report
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .webhooks()
        .is_known();
    release.send(()).unwrap();
    let revoked = if early.is_err() {
        tokio::time::timeout(WAIT, async {
            tokio::join!(cleanup.as_mut(), revoke.as_mut())
        })
        .await
        .unwrap()
        .1
    } else {
        tokio::time::timeout(WAIT, revoke.as_mut()).await.unwrap()
    };
    assert!(
        early.is_err(),
        "cleanup completed while the actual TLS response was still held"
    );
    assert!(!known_before_release);
    assert!(revoked.is_ok());
    drop(cleanup);
    drop(revoke);
    let report = report.into_inner().unwrap().unwrap();
    assert!(report.webhooks().is_ok());
    assert!(report.confirmed());
    checked_request(&recipient.next().await, &f.a);
    assert!(
        recipient.requests.try_recv().is_err(),
        "shutdown seals before a second pending record is sent"
    );
    drop(client);
    f.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn canonical_scheduling_candidates_keep_principal_credential_budget_and_borrowed_id_checks() {
    let f = Harness::new().await;
    let (mut client, _) = Client::open(f.address, &f.owner).await;
    let credential = CredentialId::new(issue_share(&f, &mut client).await).unwrap();
    let principal = PrincipalId::new("hook").unwrap();
    let candidates = f
        .grants
        .principal_conversations(principal.clone(), credential.clone(), 1)
        .await
        .unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].0, f.a);
    assert!(f
        .grants
        .principal_conversations(PrincipalId::new("owner").unwrap(), credential.clone(), 1)
        .await
        .unwrap()
        .is_empty());
    assert!(f
        .grants
        .principal_conversations(
            principal.clone(),
            CredentialId::new("owner-key").unwrap(),
            1
        )
        .await
        .unwrap()
        .is_empty());
    let shared = client.call("conversation.share",json!({"requestId":"candidate-second","conversationId":f.b.to_string(),"credentialId":credential.as_str(),"capabilities":["read"]})).await;
    assert_eq!(shared["ok"], true, "{shared}");
    assert!(matches!(
        f.grants
            .principal_conversations(principal.clone(), credential.clone(), 1)
            .await,
        Err(ConversationError::Metadata)
    ));
    assert_eq!(
        f.grants
            .principal_conversations(principal.clone(), credential.clone(), 2)
            .await
            .unwrap()
            .len(),
        2
    );
    let connection = nessa_local_database::rusqlite::Connection::open(
        f._root.path().join("private/conversations.db"),
    )
    .unwrap();
    connection
        .execute(
            "UPDATE share_grants SET resource_id=?1 WHERE resource_id=?2",
            nessa_local_database::rusqlite::params![
                "x".repeat(nessa_auth::domain::MAX_IDENTIFIER_BYTES + 1),
                f.a.to_string()
            ],
        )
        .unwrap();
    drop(connection);
    assert!(matches!(
        f.grants
            .principal_conversations(principal, credential, 2)
            .await,
        Err(ConversationError::Metadata)
    ));
    drop(client);
    f.close().await.unwrap();
}

struct ColdRuntime {
    runtime: WebhookRuntime,
    store: Arc<SqlOutbox>,
    issued: Arc<Mutex<HashMap<String, Vec<u8>>>>,
}
async fn cold_runtime(root: &std::path::Path, now: u64, http: Arc<dyn WebhookHttp>) -> ColdRuntime {
    let private = root.join("private");
    let credentials = Arc::new(
        LocalCredentialStore::open(
            &private,
            "auth.json",
            Arc::new(CedarPolicyEvaluator::new().unwrap()),
        )
        .unwrap(),
    );
    let clock = Arc::new(Time::new(now));
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
    let policy = Arc::new(CedarConversationAccessPolicy::new().unwrap());
    let metadata = Arc::new(
        LocalConversationStore::open(&private.join("conversations.db"))
            .unwrap()
            .with_access_policy(policy.clone()),
    );
    let storage = Arc::new(RecordStorage::new(root.join("sessions")).unwrap());
    storage.initialize().await.unwrap();
    let source = Arc::new(NessaRecordReadSource::new(
        storage,
        Id::new("synthetic-source").unwrap(),
        tokio::runtime::Handle::current(),
    ));
    restored
        .start_delivery(DeliveryDependencies {
            metadata,
            source,
            policy,
            capacity: Arc::new(Semaphore::new(1)),
            gateway: nessa_auth::domain::Resource::new(
                OrganizationId::new("org").unwrap(),
                ResourceId::new("gateway").unwrap(),
            ),
            http,
        })
        .unwrap();
    ColdRuntime {
        runtime: restored,
        store,
        issued,
    }
}

struct FailedSendingAck {
    inner: Arc<Transport>,
    audit: std::path::PathBuf,
    held: std::path::PathBuf,
    entered: Arc<Notify>,
}
#[async_trait]
impl WebhookHttp for FailedSendingAck {
    async fn prepare(
        &self,
        destination: &Destination,
    ) -> Result<Box<dyn WebhookPost>, PrepareFailure> {
        // Queue publication has ended before transport preparation. Only the
        // subsequent actual Sending publication loses its real directory.
        std::fs::rename(&self.audit, &self.held).unwrap();
        self.entered.notify_one();
        self.inner.prepare(destination).await
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_sending_ack_makes_no_post_and_cold_recovery_never_reposts_unknown() {
    let f = Harness::new().await;
    let root = f._root.clone();
    let conversation = f.a.clone();
    let mut recipient = Recipient::new(true);
    let audit = root.path().join("private/audit");
    let held = audit.with_extension("held");
    let entered = Arc::new(Notify::new());
    mount(
        &f,
        Arc::new(FailedSendingAck {
            inner: recipient.transport.clone(),
            audit: audit.clone(),
            held: held.clone(),
            entered: entered.clone(),
        }),
    )
    .await;
    let (mut client, _) = Client::open(f.address, &f.owner).await;
    issue_share(&f, &mut client).await;
    tokio::time::timeout(WAIT, entered.notified())
        .await
        .unwrap();
    drop(client);
    assert_eq!(f.close().await, Err(CredentialAdminError::Unavailable));
    assert!(
        recipient.requests.try_recv().is_err(),
        "Sending acknowledgement is required before polling the real TLS POST"
    );
    let connection =
        nessa_local_database::rusqlite::Connection::open(root.path().join("private/outbox.db"))
            .unwrap();
    let sending: String = connection
        .query_row(
            "SELECT id FROM deliveries WHERE phase='sending'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    drop(connection);
    std::fs::rename(&held, &audit).unwrap();
    let cold = cold_runtime(root.path(), 2000, recipient.transport.clone()).await;
    let next = checked_request(&recipient.next().await, &conversation);
    assert_ne!(
        next.0, sending,
        "cold Sending has no durable remote observation and is never automatically POSTed"
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&next.3).unwrap()["position"],
        2
    );
    cold.runtime.shutdown().await.unwrap();
    let connection =
        nessa_local_database::rusqlite::Connection::open(root.path().join("private/outbox.db"))
            .unwrap();
    let phase: String = connection
        .query_row(
            "SELECT phase FROM deliveries WHERE id=?1",
            [sending],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(phase, "unknown");
    assert!(cold.issued.lock().unwrap().is_empty());
    assert!(recipient.requests.try_recv().is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn expiry_during_actual_sending_commit_refuses_dispatch_after_acknowledgement() {
    let f = Harness::new().await;
    let root = f._root.clone();
    let mut recipient = Recipient::new(true);
    *f.clock.1.lock().unwrap() = Some(root.path().join("private/outbox.db"));
    mount(&f, recipient.transport.clone()).await;
    let (mut client, _) = Client::open(f.address, &f.owner).await;
    issue_share(&f, &mut client).await;
    let before = f.runtime.completed_delivery_cycles();
    f.runtime.wake_delivery();
    let completed = tokio::time::timeout(WAIT, f.runtime.after_delivery_cycle(before)).await;
    if completed.is_err() {
        eprintln!("clock at failure: {}", f.clock.0.load(Ordering::SeqCst));
        let connection = Connection::open(root.path().join("private/outbox.db")).unwrap();
        connection.busy_timeout(Duration::ZERO).unwrap();
        for query in [
            "SELECT state FROM deliveries ORDER BY position",
            "SELECT record FROM audit ORDER BY rowid",
        ] {
            let mut statement = connection.prepare(query).unwrap();
            let rows = statement
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap();
            for row in rows {
                eprintln!("saved diagnostic: {:?}", row);
            }
        }
        drop(connection);
        eprintln!(
            "shutdown at failure: {:?}",
            tokio::time::timeout(WAIT, f.runtime.shutdown()).await
        );
    }
    assert!(completed.is_ok(), "expiry cycle must complete");
    let fresh = f.runtime.completed_delivery_cycles();
    f.runtime.wake_delivery();
    tokio::time::timeout(WAIT, f.runtime.after_delivery_cycle(fresh))
        .await
        .unwrap();
    assert_eq!(
        f.clock.0.load(Ordering::SeqCst),
        100_000,
        "clock changes only after real SQL Sending commits"
    );
    assert!(f.clock.1.lock().unwrap().is_none());
    drop(client);
    f.close().await.unwrap();
    assert!(
        recipient.requests.try_recv().is_err(),
        "expired authority cannot poll the actual prepared TLS POST"
    );
    let facts: Vec<Value> = std::fs::read_dir(root.path().join("private/audit"))
        .unwrap()
        .map(|entry| {
            serde_json::from_slice(&std::fs::read(entry.unwrap().path()).unwrap()).unwrap()
        })
        .collect();
    assert!(facts.iter().any(|fact| fact["cause"] == "admission"));
    let refused: Vec<_> = facts
        .iter()
        .filter(|fact| fact["cause"] == "admission_expired")
        .collect();
    assert_eq!(refused.len(), 1);
    assert_eq!(refused[0]["beforeState"]["phase"], "sending");
    assert_eq!(refused[0]["afterState"]["phase"], "not_sent");
    assert_eq!(refused[0]["attempt"], 1);
}

struct PausedPreparation {
    inner: Arc<Transport>,
    entered: Arc<Notify>,
    release: Arc<Notify>,
}
#[async_trait]
impl WebhookHttp for PausedPreparation {
    async fn prepare(
        &self,
        destination: &Destination,
    ) -> Result<Box<dyn WebhookPost>, PrepareFailure> {
        self.entered.notify_one();
        self.release.notified().await;
        self.inner.prepare(destination).await
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn public_revoke_wins_during_preparation_without_post_or_failed_cleanup() {
    let f = Harness::new().await;
    let mut recipient = Recipient::new(true);
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    mount(
        &f,
        Arc::new(PausedPreparation {
            inner: recipient.transport.clone(),
            entered: entered.clone(),
            release: release.clone(),
        }),
    )
    .await;
    let (mut client, _) = Client::open(f.address, &f.owner).await;
    let credential = issue_share(&f, &mut client).await;
    tokio::time::timeout(WAIT, entered.notified())
        .await
        .unwrap();
    let revoked = client
        .call(
            "credential.revoke",
            json!({"requestId":"revoke-before-admission","credentialId":credential}),
        )
        .await;
    assert_eq!(revoked["ok"], true, "{revoked}");
    assert!(f
        .store
        .registration(CredentialId::new(credential).unwrap())
        .await
        .unwrap()
        .is_none());
    let before = f.runtime.completed_delivery_cycles();
    release.notify_one();
    tokio::time::timeout(WAIT, f.runtime.after_delivery_cycle(before))
        .await
        .unwrap();
    assert!(recipient.requests.try_recv().is_err());
    drop(client);
    f.close().await.unwrap();
}

struct ObservationClock(AtomicBool);
impl Clock for ObservationClock {
    fn unix_milliseconds(&self) -> u64 {
        assert!(
            !self.0.swap(false, Ordering::SeqCst),
            "synthetic post-response clock panic"
        );
        2000
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_response_is_owned_before_a_later_clock_callback_panics() {
    let clock = Arc::new(ObservationClock(AtomicBool::new(false)));
    let f = Harness::with_runtime_clock(None, Some(clock.clone())).await;
    let root = f._root.clone();
    let entered = Arc::new(Notify::new());
    let (release, wait) = std::sync::mpsc::channel();
    let mut recipient = Recipient::with_response_pause(true, Some((entered.clone(), wait)));
    mount(&f, recipient.transport.clone()).await;
    let (mut client, _) = Client::open(f.address, &f.owner).await;
    issue_share(&f, &mut client).await;
    tokio::time::timeout(WAIT, entered.notified())
        .await
        .unwrap();
    // Only the runtime uses this clock. Its next callback occurs after the
    // already-started actual TLS POST returns a complete response.
    clock.0.store(true, Ordering::SeqCst);
    release.send(()).unwrap();
    checked_request(&recipient.next().await, &f.a);
    drop(client);
    assert_eq!(
        f.close().await,
        Err(CredentialAdminError::Unavailable),
        "supervisor panic must remain failed cleanup"
    );
    assert!(!clock.0.load(Ordering::SeqCst));
    let connection = Connection::open(root.path().join("private/outbox.db")).unwrap();
    let retries: i64 = connection
        .query_row(
            "SELECT count(*) FROM deliveries WHERE phase='retry'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(retries,1,"owned cleanup preserves and settles the actual received 503 despite the later callback panic");
    let sending: i64 = connection
        .query_row(
            "SELECT count(*) FROM deliveries WHERE phase='sending'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(sending, 0);
    assert!(recipient.requests.try_recv().is_err());
}
