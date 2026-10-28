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
use nessa_auth::domain::Resource;
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
    panic::AssertUnwindSafe,
    sync::mpsc::{Receiver as BlockingReceiver, Sender as BlockingSender},
    thread::{self, JoinHandle as ThreadJoinHandle},
};
use tokio::{
    sync::{
        mpsc::{self, Receiver},
        Semaphore,
    },
    time::Instant,
};

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
    requests: Receiver<Vec<u8>>,
    stop: Arc<AtomicBool>,
    task: Option<ThreadJoinHandle<()>>,
}
impl Recipient {
    fn new(always_retry: bool) -> Self {
        Self::with_response_pause(always_retry, None)
    }
    fn with_response_pause(
        always_retry: bool,
        pause: Option<(Arc<Notify>, BlockingReceiver<()>)>,
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
                // BSD may inherit the listener's nonblocking mode. This
                // synchronous TLS reader uses the bounded socket timeouts.
                stream.set_nonblocking(false).unwrap();
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
async fn canonical_storage(f: &Harness) -> Arc<RecordStorage> {
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
    storage
}
async fn mount(f: &Harness, http: Arc<dyn WebhookHttp>) -> Arc<RecordStorage> {
    let storage = canonical_storage(f).await;
    let source = Arc::new(NessaRecordReadSource::new(
        storage.clone(),
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
    storage
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
        // This case exercises mutation after consumed response ACK. The pinned
        // publication-outage race is exercised separately below.
        two_fresh_cycles(&f).await;
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
        Err(ConversationError::Capacity)
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
    // Even at a smaller discovery bound, malformed returned metadata is not
    // reclassified as ordinary capacity exhaustion.
    assert!(matches!(
        f.grants
            .principal_conversations(principal.clone(), credential.clone(), 1)
            .await,
        Err(ConversationError::Metadata)
    ));
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
            gateway: Resource::new(
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
    failed: AtomicBool,
}
#[async_trait]
impl WebhookHttp for FailedSendingAck {
    async fn prepare(
        &self,
        destination: &Destination,
    ) -> Result<Box<dyn WebhookPost>, PrepareFailure> {
        // Queue publication has ended before transport preparation. Only the
        // subsequent actual Sending publication loses its real directory.
        if !self.failed.swap(true, Ordering::SeqCst) {
            std::fs::rename(&self.audit, &self.held).unwrap();
            self.entered.notify_one();
        }
        self.inner.prepare(destination).await
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_sending_ack_commits_unposted_unknown_and_cold_recovery_never_reposts_it() {
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
            failed: AtomicBool::new(false),
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
            "SELECT id FROM deliveries WHERE phase='unknown'",
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
        "a locally reconciled unposted Unknown is never automatically POSTed"
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

async fn two_fresh_cycles(f: &Harness) {
    for _ in 0..2 {
        let before = f.runtime.completed_delivery_cycles();
        f.runtime.wake_delivery();
        tokio::time::timeout(WAIT, f.runtime.after_delivery_cycle(before))
            .await
            .unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn idle_expiry_retries_its_original_stop_after_publication_returns_without_grants() {
    let f = Harness::new().await;
    let mut recipient = Recipient::new(true);
    let (mut client, _) = Client::open(f.address, &f.owner).await;
    let issue = client.call("credential.issue", issue_params()).await;
    assert_eq!(issue["ok"], true);
    let credential =
        CredentialId::new(issue["payload"]["credential"]["id"].as_str().unwrap()).unwrap();
    let original = f.store.intents().await.unwrap().pop().unwrap();
    let audit = f._root.path().join("private/audit");
    let held = audit.with_extension("idle-held");
    std::fs::rename(&audit, &held).unwrap();
    f.clock.0.store(100_000, Ordering::SeqCst);
    mount(&f, recipient.transport.clone()).await;
    two_fresh_cycles(&f).await;
    let connection = Connection::open(f._root.path().join("private/outbox.db")).unwrap();
    let first: String = connection.query_row(
        "SELECT record FROM audit WHERE json_extract(record,'$.cause')='credential_expiry_observed'", [], |r|r.get(0),
    ).unwrap();
    let first: Value = serde_json::from_str(&first).unwrap();
    assert_eq!(first["initiator"], format!("source:{}", original.id));
    std::fs::rename(&held, &audit).unwrap();
    two_fresh_cycles(&f).await;
    assert!(f.store.registration(credential).await.unwrap().is_none());
    assert!(f.store.intents().await.unwrap().is_empty());
    // Retirement reclaims retained SQL facts after their actual publication.
    // Assert the immutable published stop, not indefinite SQL retention.
    let stops: Vec<Value> = std::fs::read_dir(&audit)
        .unwrap()
        .map(|entry| {
            serde_json::from_slice::<Value>(&std::fs::read(entry.unwrap().path()).unwrap()).unwrap()
        })
        .filter(|record| record["cause"] == "credential_expired")
        .collect();
    assert!(!stops.is_empty());
    for record in stops {
        assert_eq!(record["initiator"], first["initiator"]);
        assert_eq!(record["credential"], original.issue.credential_id);
    }
    for table in ["cursors", "deliveries"] {
        let count: i64 = connection
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }
    drop(connection);
    assert!(recipient.requests.try_recv().is_err());
    drop(client);
    f.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lost_sending_ack_reconciles_unknown_and_sends_next_record_in_the_same_live_owner() {
    let f = Harness::new().await;
    let mut recipient = Recipient::new(true);
    let audit = f._root.path().join("private/audit");
    let held = audit.with_extension("live-held");
    let entered = Arc::new(Notify::new());
    mount(
        &f,
        Arc::new(FailedSendingAck {
            inner: recipient.transport.clone(),
            audit: audit.clone(),
            held: held.clone(),
            entered: entered.clone(),
            failed: AtomicBool::new(false),
        }),
    )
    .await;
    let (mut client, _) = Client::open(f.address, &f.owner).await;
    issue_share(&f, &mut client).await;
    tokio::time::timeout(WAIT, entered.notified())
        .await
        .unwrap();
    two_fresh_cycles(&f).await;
    assert!(recipient.requests.try_recv().is_err());
    // Control commands remain admitted while the serial sender waits for storage.
    let list = client.call("credential.list", json!({})).await;
    assert_eq!(list["ok"], true, "{list}");
    let connection = Connection::open(f._root.path().join("private/outbox.db")).unwrap();
    let sending: String = connection
        .query_row("SELECT id FROM deliveries WHERE phase='unknown'", [], |r| {
            r.get(0)
        })
        .unwrap();
    std::fs::rename(&held, &audit).unwrap();
    f.runtime.wake_delivery();
    let next = checked_request(&recipient.next().await, &f.a);
    assert_ne!(next.0, sending);
    assert_eq!(
        serde_json::from_slice::<Value>(&next.3).unwrap()["position"],
        2
    );
    two_fresh_cycles(&f).await;
    let phase: String = connection
        .query_row(
            "SELECT phase FROM deliveries WHERE id=?1",
            [&sending],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(phase, "unknown");
    let fact: String = connection.query_row("SELECT record FROM audit WHERE delivery=?1 AND json_extract(record,'$.cause')='reconcile'",[&sending],|r|r.get(0)).unwrap();
    let fact: Value = serde_json::from_str(&fact).unwrap();
    assert_eq!(fact["initiator"], format!("delivery:{sending}"));
    drop(connection);
    assert!(recipient.requests.try_recv().is_err());
    drop(client);
    f.close().await.unwrap();
}

#[tokio::test]
async fn dispatch_deadline_releases_grant_while_native_sending_sql_stays_owned() {
    for after_commit in [false, true] {
        let f = Harness::new().await;
        let mut recipient = Recipient::new(true);
        let pause = f.store.pause_admission(after_commit);
        let (mut client, _) = Client::open(f.address, &f.owner).await;
        let mut health = health_reader(&f, &mut client).await;
        mount(&f, recipient.transport.clone()).await;
        let credential = issue_share(&f, &mut client).await;
        tokio::time::timeout(WAIT, pause.entered()).await.unwrap();
        tokio::time::pause();
        tokio::time::advance(crate::webhooks::application::POST_DEADLINE).await;
        // A paused blocking SQL worker prevents virtual auto-advance. Resume
        // real time before observing the timer; keep the original one-second
        // notification bound and release/join SQL before asserting failures.
        tokio::time::resume();
        let released =
            tokio::time::timeout(Duration::from_secs(1), f.runtime.after_dispatch_deadline()).await;
        // Real product unshare performs actual metadata work after time resumes.
        // Always release/join the SQL worker before asserting a negative result.
        let status = tokio::time::timeout(
            WAIT,
            AssertUnwindSafe(health.call("server.health", json!({}))).catch_unwind(),
        )
        .await;
        let conversation = f.a.clone();
        let mut unshare = tokio::spawn(async move {
            let reply = client.call("conversation.unshare",json!({"requestId":"unshare-at-dispatch-deadline","conversationId":conversation.to_string(),"credentialId":credential})).await;
            (client, reply)
        });
        let unshared = tokio::time::timeout(WAIT, &mut unshare).await;
        let before_release = unshared.is_ok();
        let runtime = f.runtime.clone();
        let mut shutdown = Box::pin(runtime.shutdown());
        let still_owned = shutdown.as_mut().now_or_never().is_none();
        pause.release();
        let (client, reply) = match unshared {
            Ok(result) => result.unwrap(),
            Err(_) => unshare.await.unwrap(),
        };
        let drained = tokio::time::timeout(WAIT, shutdown).await.unwrap();
        let status = status
            .expect("health observer is bounded")
            .expect("health observer did not panic");
        assert_eq!(status["payload"]["webhook"]["pendingOutcome"], "unposted");
        assert_eq!(
            status["payload"]["webhook"]["nativeMutations"],
            "temporarily-unavailable"
        );
        assert!(
            released.is_ok(),
            "authority deadline must fire, stage={after_commit}"
        );
        assert!(
            before_release,
            "unshare must finish while SQL is still paused"
        );
        assert_eq!(reply["ok"], true, "{reply}");
        assert!(
            still_owned,
            "cleanup must join the actual native SQL worker"
        );
        assert_eq!(drained, Ok(()));
        let connection = Connection::open(f._root.path().join("private/outbox.db")).unwrap();
        let (phase,fact): (String,String) = connection.query_row(
            "SELECT d.phase,a.record FROM deliveries d JOIN audit a ON a.delivery=d.id WHERE json_extract(a.record,'$.cause')=?1",[if after_commit {"reconcile"} else {"audit_unavailable"}],|r|Ok((r.get(0)?,r.get(1)?)),
        ).unwrap();
        assert_eq!(phase, if after_commit { "unknown" } else { "not_sent" });
        let fact: Value = serde_json::from_str(&fact).unwrap();
        assert_eq!(
            fact["initiator"],
            format!("delivery:{}", fact["deliveryId"].as_str().unwrap())
        );
        drop(connection);
        assert!(recipient.requests.try_recv().is_err());
        drop(health);
        drop(client);
        f.close().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn public_reissue_keeps_stopped_sending_history_separate_from_the_new_registration() {
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
    // A synthetic native Sending row represents a late committed attempt whose
    // physical owner has ended. This test does not claim an HTTP request occurred.
    let old = f.store.due(2000).await.unwrap().into_iter().next().unwrap();
    let sending = old.delivery.admit(old.delivery.epoch(), 2000).unwrap();
    f.store
        .transition(
            &old.delivery,
            &sending,
            crate::webhooks::application::DeliveryCause::Admission,
            2000,
        )
        .await
        .unwrap();
    let revoked = client
        .call(
            "credential.revoke",
            json!({"requestId":"revoke-retained-sending","credentialId":credential}),
        )
        .await;
    let connection = Connection::open(f._root.path().join("private/outbox.db")).unwrap();
    let retained: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM registrations r WHERE r.id=?1 AND r.active=0 AND EXISTS(SELECT 1 FROM deliveries d WHERE d.registration=r.id AND d.phase='sending'))",[old.registration.id.to_string()],|r|r.get(0),
    ).unwrap();
    drop(connection);
    let mut params = issue_params();
    params["requestId"] = json!("reissue-with-stopped-sending");
    params["expiresAt"] = json!(200);
    let next = client.call("credential.issue", params).await;
    let next_id = next["payload"]["credential"]["id"]
        .as_str()
        .map(str::to_owned);
    let intents = f.store.intents().await.unwrap();
    let grant = f
        .grants
        .principal_grant(&f.a, &PrincipalId::new("hook").unwrap())
        .await
        .unwrap()
        .unwrap();
    // Release the existing worker on every assertion path; it then reconciles
    // the old unowned Sending row live, without granting the new credential.
    let before = f.runtime.completed_delivery_cycles();
    release.notify_one();
    let progressed = tokio::time::timeout(WAIT, f.runtime.after_delivery_cycle(before)).await;
    two_fresh_cycles(&f).await;
    let facts: Vec<Value> = std::fs::read_dir(f._root.path().join("private/audit"))
        .unwrap()
        .map(|entry| {
            serde_json::from_slice::<Value>(&std::fs::read(entry.unwrap().path()).unwrap()).unwrap()
        })
        .collect();
    let reconciled = facts.iter().any(|fact| {
        fact["cause"] == "reconcile"
            && fact["deliveryId"] == sending.id().to_string()
            && fact["after"] == "unknown"
    });
    drop(client);
    let drained = f.close().await;
    assert_eq!(revoked["ok"], true, "{revoked}");
    assert!(retained);
    assert_eq!(next["ok"], true, "{next}");
    assert_ne!(next_id.as_deref(), Some(credential.as_str()));
    assert_eq!(
        intents.len(),
        2,
        "the stopped original and active new intent are distinct"
    );
    assert_eq!(grant.credential_id.as_str(), credential);
    assert!(progressed.is_ok());
    assert!(
        reconciled,
        "the unowned old Sending row reconciles live, without POST"
    );
    assert_eq!(drained, Ok(()));
    assert!(recipient.requests.try_recv().is_err());
}

// Own the real native connection lock on a separate OS worker. Drop releases
// and joins it even when an assertion fails; no async task holds a std guard.
struct MetadataBlock {
    release: Option<BlockingSender<()>>,
    worker: Option<ThreadJoinHandle<()>>,
}
impl MetadataBlock {
    async fn new(store: Arc<LocalConversationStore>) -> Self {
        let (entered, wait) = oneshot::channel();
        let (release, unblock) = std::sync::mpsc::channel();
        let worker = thread::spawn(move || {
            let _connection = store.hold_mutation();
            entered.send(()).unwrap();
            let _ = unblock.recv();
        });
        wait.await.unwrap();
        Self {
            release: Some(release),
            worker: Some(worker),
        }
    }
    fn release(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap();
        }
    }
}
impl Drop for MetadataBlock {
    fn drop(&mut self) {
        self.release();
    }
}
#[tokio::test]
async fn dispatch_deadline_releases_partial_admission_before_joining_native_metadata() {
    let f = Harness::new().await;
    let (mut client, _) = Client::open(f.address, &f.owner).await;
    let credential = issue_share(&f, &mut client).await;
    let registration = f
        .store
        .registration(CredentialId::new(&credential).unwrap())
        .await
        .unwrap()
        .unwrap();
    let epoch = f
        .grants
        .principal_grant(&f.a, &PrincipalId::new("hook").unwrap())
        .await
        .unwrap()
        .unwrap()
        .policy_revision;
    let policy = CedarConversationAccessPolicy::new().unwrap();
    let admission = crate::webhooks::application::AdmitDelivery {
        credentials: f.runtime.credentials(),
        grants: f.grants.as_ref(),
        conversations: f.grants.as_ref(),
        policy: &policy,
        clock: f.clock.as_ref(),
        gateway: &f.state.gateway,
    };
    let mut blocked = MetadataBlock::new(f.grants.clone()).await;
    tokio::time::pause();
    let mut work = Box::pin(f.runtime.admit_dispatch_before(
        &admission,
        &registration.target,
        &f.a,
        epoch,
        Instant::now() + crate::webhooks::application::POST_DEADLINE,
    ));
    assert!(
        work.as_mut().now_or_never().is_none(),
        "real native metadata read is pending"
    );
    tokio::time::advance(crate::webhooks::application::POST_DEADLINE).await;
    // Resume real time before waiting for OS work. Drive this SAME future
    // concurrently with its notice; waiting for the notice alone stops its timer.
    tokio::time::resume();
    let mut completed = None;
    let released = tokio::time::timeout(WAIT, async {
        tokio::select! {
            _ = f.runtime.after_dispatch_deadline() => true,
            result = work.as_mut() => { completed = Some(result); false },
        }
    })
    .await;
    let still_owned = completed.is_none() && work.as_mut().now_or_never().is_none();
    // Only this observation is cancelled on timeout. The real product owner
    // retains its revoke job, which cleanup below joins after releasing the lock.
    let mut revoke = Box::pin(client.call(
        "credential.revoke",
        json!({"requestId":"revoke-admission-metadata-wait","credentialId":credential}),
    ));
    let revoked =
        tokio::time::timeout(WAIT, AssertUnwindSafe(revoke.as_mut()).catch_unwind()).await;
    drop(revoke);
    blocked.release();
    let admitted = match completed {
        Some(result) => result.unwrap(),
        None => tokio::time::timeout(WAIT, work.as_mut())
            .await
            .unwrap()
            .unwrap(),
    };
    drop(work); // Both branches joined/completed it; release its borrow of the fixture.
    let no_admission = admitted.is_none();
    drop(admitted); // A broken implementation's returned hold cannot strand cleanup.
    drop(client);
    let drained = f.close().await;
    assert!(
        matches!(released, Ok(true)),
        "the driven admission must release authority at its deadline"
    );
    assert!(
        still_owned,
        "deadline cannot detach the physical metadata read"
    );
    assert!(
        matches!(&revoked, Ok(Ok(_))),
        "public revoke must finish while metadata remains blocked"
    );
    let reply = match revoked {
        Ok(Ok(reply)) => reply,
        _ => unreachable!(),
    };
    assert_eq!(reply["ok"], true, "{reply}");
    assert!(
        no_admission,
        "expired admission cannot produce a dispatch hold"
    );
    assert_eq!(drained, Ok(()));
}

async fn health_reader(f: &Harness, owner: &mut Client) -> Client {
    let mut params = ordinary_params();
    params["requestId"] = json!("status-reader-issue");
    params["grants"] =
        json!([{"action":"server.read","resource":{"organizationId":"org","id":"gateway"}}]);
    let issued = owner.call("credential.issue", params).await;
    assert_eq!(issued["ok"], true, "{issued}");
    let evidence = issued["payload"]["secret"].as_str().unwrap();
    let (client, ready) = Client::open(f.address, evidence).await;
    assert_eq!(ready["ok"], true, "{ready}");
    client
}

struct RefusedPreparation {
    inner: Arc<Transport>,
    reason: PrepareFailure,
    refused: AtomicBool,
    entered: Notify,
    released: AtomicBool,
    wake: Notify,
}
#[async_trait]
impl WebhookHttp for RefusedPreparation {
    async fn prepare(
        &self,
        destination: &Destination,
    ) -> Result<Box<dyn WebhookPost>, PrepareFailure> {
        if !self.refused.swap(true, Ordering::SeqCst) {
            self.entered.notify_one();
            return Err(self.reason);
        }
        loop {
            let changed = self.wake.notified();
            if self.released.load(Ordering::SeqCst) {
                break;
            }
            changed.await;
        }
        self.inner.prepare(destination).await
    }
}

#[tokio::test]
async fn each_preparation_refusal_keeps_queue_and_controls_then_recovers_with_fresh_validation() {
    for reason in [
        PrepareFailure::Busy,
        PrepareFailure::Unavailable,
        PrepareFailure::TimedOut,
        PrepareFailure::Destination,
    ] {
        let f = Harness::new().await;
        let mut recipient = Recipient::new(false);
        let transport = Arc::new(RefusedPreparation {
            inner: recipient.transport.clone(),
            reason,
            refused: AtomicBool::new(false),
            entered: Notify::new(),
            released: AtomicBool::new(false),
            wake: Notify::new(),
        });
        let (mut client, _) = Client::open(f.address, &f.owner).await;
        let mut health = health_reader(&f, &mut client).await;
        mount(&f, transport.clone()).await;
        issue_share(&f, &mut client).await;
        tokio::time::timeout(WAIT, transport.entered.notified())
            .await
            .unwrap();
        let c = Connection::open(f._root.path().join("private/outbox.db")).unwrap();
        let attempts: i64 = c
            .query_row(
                "SELECT count(*) FROM audit WHERE json_extract(record,'$.cause')='admission'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let first: String = c
            .query_row(
                "SELECT id FROM deliveries ORDER BY position LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(attempts, 0, "{reason:?}");
        assert_eq!(
            c.query_row(
                "SELECT count(*) FROM deliveries WHERE phase<>'queued'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        drop(c);
        assert!(recipient.requests.try_recv().is_err());
        // A refusal never pins native work or kills the shared product owner.
        assert!(!f.store.due(2000).await.unwrap().is_empty());
        let status = health.call("server.health", json!({})).await;
        let refusal = match reason {
            PrepareFailure::Busy => "preparation-busy",
            PrepareFailure::Unavailable => "preparation-unavailable",
            PrepareFailure::TimedOut => "preparation-timed-out",
            PrepareFailure::Destination => "destination-refused",
        };
        assert_eq!(status["ok"], true, "{reason:?}: {status}");
        assert_eq!(
            status["payload"]["webhook"],
            json!({"pendingOutcome":"none","nativeMutations":"available","lastRefusal":refusal})
        );
        let denied = client.call("server.health", json!({})).await;
        assert_eq!(
            denied["error"]["code"], "forbidden",
            "status must retain server.read admission"
        );
        transport.released.store(true, Ordering::SeqCst);
        transport.wake.notify_waiters();
        let received = checked_request(&recipient.next().await, &f.a);
        assert_eq!(
            received.0.to_string(),
            first,
            "refusal must preserve delivery identity"
        );
        checked_request(&recipient.next().await, &f.a);
        two_fresh_cycles(&f).await;
        let status = health.call("server.health", json!({})).await;
        assert_eq!(
            status["payload"]["webhook"]["lastRefusal"], refusal,
            "diagnostic survives healthy progress"
        );
        drop(health);
        drop(client);
        f.close().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pinned_response_refuses_native_cleanup_but_canonical_revoke_and_unshare_commit() {
    for action in ["revoke", "unshare"] {
        let f = Harness::new().await;
        let entered = Arc::new(Notify::new());
        let (release, wait) = std::sync::mpsc::channel();
        let mut recipient = Recipient::with_response_pause(true, Some((entered.clone(), wait)));
        let (mut client, _) = Client::open(f.address, &f.owner).await;
        let mut health = health_reader(&f, &mut client).await;
        mount(&f, recipient.transport.clone()).await;
        let credential = issue_share(&f, &mut client).await;
        tokio::time::timeout(WAIT, entered.notified())
            .await
            .unwrap();
        let audit = f._root.path().join("private/audit");
        let held = audit.with_extension("held");
        std::fs::rename(&audit, &held).unwrap();
        let before = f.runtime.completed_delivery_cycles();
        release.send(()).unwrap();
        checked_request(&recipient.next().await, &f.a);
        tokio::time::timeout(WAIT, f.runtime.after_delivery_cycle(before))
            .await
            .unwrap();
        assert_eq!(
            f.store.due(2000).await.err(),
            Some(crate::webhooks::application::OutboxError::Unavailable)
        );
        let c = Connection::open(f._root.path().join("private/outbox.db")).unwrap();
        let audits: i64 = c
            .query_row("SELECT count(*) FROM audit WHERE published=0", [], |r| {
                r.get(0)
            })
            .unwrap();
        for _ in 0..2 {
            let status = health.call("server.health", json!({})).await;
            assert_eq!(
                status["payload"]["webhook"],
                json!({"pendingOutcome":"response","nativeMutations":"temporarily-unavailable"})
            );
        }
        assert_eq!(
            c.query_row("SELECT count(*) FROM audit WHERE published=0", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            audits,
            "status does not publish or consume settlement"
        );
        drop(c);
        if action == "revoke" {
            let reply = client
                .call(
                    "credential.revoke",
                    json!({"requestId":"pinned-revoke","credentialId":credential}),
                )
                .await;
            assert_eq!(
                reply["error"]["code"], "credential_store_unavailable",
                "native cleanup has no ACK: {reply}"
            );
            let snapshot = f
                .runtime
                .credentials()
                .read(&CredentialId::new(&credential).unwrap())
                .await
                .unwrap();
            assert!(
                snapshot.credential.revoked_at().is_some(),
                "canonical revocation must already be durable"
            );
        } else {
            let reply = client.call("conversation.unshare", json!({"requestId":"pinned-unshare","conversationId":f.a.to_string(),"credentialId":credential})).await;
            assert_eq!(reply["ok"], true, "{reply}");
            assert!(f
                .grants
                .principal_grant(&f.a, &PrincipalId::new("hook").unwrap())
                .await
                .unwrap()
                .is_none());
        }
        assert!(recipient.requests.try_recv().is_err());
        std::fs::rename(&held, &audit).unwrap();
        f.clock.0.store(4000, Ordering::SeqCst);
        two_fresh_cycles(&f).await;
        assert!(
            recipient.requests.try_recv().is_err(),
            "{action} must prevent subsequent dispatch"
        );
        let status = health.call("server.health", json!({})).await;
        assert_eq!(status["payload"]["webhook"]["pendingOutcome"], "none");
        assert_eq!(status["payload"]["webhook"]["nativeMutations"], "available");
        drop(health);
        drop(client);
        f.close().await.unwrap();
    }
}

struct PermanentlyRefusedDestination {
    transport: Arc<Transport>,
    refused: Destination,
    healthy: Destination,
}
#[async_trait]
impl WebhookHttp for PermanentlyRefusedDestination {
    async fn prepare(
        &self,
        destination: &Destination,
    ) -> Result<Box<dyn WebhookPost>, PrepareFailure> {
        if destination == &self.refused {
            return Err(PrepareFailure::Destination);
        }
        assert_eq!(destination, &self.healthy);
        // Logical destinations are synthetic; this seam changes only socket/CA.
        isolated_post(
            self.transport.address,
            &self.transport.root,
            self.transport.capacity.clone(),
        )
    }
}
#[tokio::test]
async fn permanently_refused_registration_does_not_starve_a_healthy_public_sibling() {
    let config = |principal: &str, url: &str| ConfiguredWebhook {
        principal: PrincipalId::new(principal).unwrap(),
        registration: RegistrationConfig {
            destination: Destination::parse(url).unwrap(),
            key: Arc::new(MacKey::new([7; 32])),
        },
    };
    let f = Harness::with_configured(
        None,
        None,
        vec![
            config("hook", "https://first.test/synthetic"),
            config("healthy", "https://second.test/synthetic"),
        ],
    )
    .await;
    // Both public registrations/shares precede mounting: an owned healthy POST
    // intentionally makes concurrent native issuance transiently unavailable.
    let (mut client, _) = Client::open(f.address, &f.owner).await;
    let first = issue_share(&f, &mut client).await;
    let mut params = issue_params();
    params["requestId"] = json!("healthy-sibling-issue");
    params["principal"]["id"] = json!("healthy");
    params["membership"]["principalId"] = json!("healthy");
    params["membership"]["id"] = json!("healthy-member");
    let issued = client.call("credential.issue", params).await;
    assert_eq!(issued["ok"], true, "{issued}");
    let second = issued["payload"]["credential"]["id"].as_str().unwrap();
    let shared = client.call("conversation.share", json!({"requestId":"healthy-sibling-share","conversationId":f.b.to_string(),"credentialId":second,"capabilities":["read"]})).await;
    assert_eq!(shared["ok"], true, "{shared}");
    let a = f
        .store
        .registration(CredentialId::new(&first).unwrap())
        .await
        .unwrap()
        .unwrap();
    let b = f
        .store
        .registration(CredentialId::new(second).unwrap())
        .await
        .unwrap()
        .unwrap();
    // Force the permanently refused registration to be the lexically first
    // one. Removing fair progression must fail regardless of generated UUIDs.
    let (refused, healthy, conversation) = if a.id < b.id {
        (a, b, f.b.clone())
    } else {
        (b, a, f.a.clone())
    };
    let mut recipient = Recipient::new(false);
    mount(
        &f,
        Arc::new(PermanentlyRefusedDestination {
            transport: recipient.transport.clone(),
            refused: refused.destination.clone(),
            healthy: healthy.destination,
        }),
    )
    .await;
    checked_request(&recipient.next().await, &conversation);
    checked_request(&recipient.next().await, &conversation);
    two_fresh_cycles(&f).await;
    let c = Connection::open(f._root.path().join("private/outbox.db")).unwrap();
    let attempts: i64 = c.query_row("SELECT count(*) FROM audit WHERE registration=?1 AND json_extract(record,'$.cause')='admission'", [refused.id.to_string()], |r| r.get(0)).unwrap();
    assert_eq!(attempts, 0, "permanent refusal is not a transport attempt");
    let queued: i64 = c
        .query_row(
            "SELECT count(*) FROM deliveries WHERE registration=?1 AND phase='queued'",
            [refused.id.to_string()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(queued, 2, "refused conversation keeps its actual queue");
    drop(c);
    assert!(recipient.requests.try_recv().is_err());
    drop(client);
    f.close().await.unwrap();
}

#[path = "product_delivery/owner_reset.rs"]
mod owner_reset;

#[path = "product_delivery/owner_query.rs"]
mod owner_query;

#[path = "product_delivery/discovery.rs"]
mod discovery;
