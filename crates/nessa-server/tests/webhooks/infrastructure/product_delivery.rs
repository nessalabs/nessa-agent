//! Public product commands through canonical storage and actual TLS POST bytes.
use super::*;
use crate::conversation::infrastructure::NessaRecordReadSource;
use crate::webhooks::application::{
    Message as SignedMessage, Outbox, PrepareFailure, WebhookHttp, WebhookPost,
};
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
#[path = "fixtures/tls.rs"]
mod tls;

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
        crate::agent_install::infrastructure::https_archives::install_tls_backend();
        let tls = tls::RecipientTls::generate();
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
                tx.blocking_send(bytes).unwrap();
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
            task.join().unwrap();
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
    assert!(f.store.due(4000).await.unwrap().is_empty());
    assert!(recipient.requests.try_recv().is_err());
    drop(client);
    assert!(f.close().await.is_ok());
}
