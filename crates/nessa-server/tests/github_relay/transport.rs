//! Producer JSON crosses the real HTTP decoder into encrypted catch-up storage.
use super::{HttpRelaySource, RelayConfiguration};
use crate::{
    env::Stage,
    github_relay::{
        application::{catch_up, InboxKeySource, RelaySource, RelayStore},
        domain::{Binding, Failure},
        infrastructure::SqliteRelayStore,
    },
};
use axum::{http::StatusCode, routing::get, Json, Router};
use nessa_local_database::{rusqlite::Connection, DatabaseKey, Schema};
use reqwest::Client;
use rustls::{ClientConfig, RootCertStore};
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tempfile::TempDir;
use tokio::{net::TcpListener, task::JoinHandle};

struct Server {
    source: HttpRelaySource,
    task: JoinHandle<()>,
    calls: Arc<AtomicUsize>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn server(status: StatusCode, body: Value) -> Server {
    let tls =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(RootCertStore::empty())
            .with_no_client_auth();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let calls = Arc::new(AtomicUsize::new(0));
    let requests = calls.clone();
    let router = Router::new().route(
        "/relay/v1/events",
        get(move || {
            requests.fetch_add(1, Ordering::SeqCst);
            let body = body.clone();
            async move { (status, Json(body)) }
        }),
    );
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    // Exercise HTTP decoding through the injected client without changing the
    // public constructor's HTTPS-only production-origin validation.
    Server {
        source: HttpRelaySource {
            http: Client::builder()
                .tls_backend_preconfigured(tls)
                .no_proxy()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap(),
            configuration: RelayConfiguration {
                origin,
                app_slug: "fixture-app".into(),
                app_id: 1,
                client_id: "fixture-client".into(),
            },
            stage: Stage::Dev,
        },
        task,
        calls,
    }
}
struct TestKey;
impl InboxKeySource for TestKey {
    fn load(&self) -> Result<Arc<DatabaseKey>, Failure> {
        Ok(Arc::new(DatabaseKey::new([11; 32])))
    }
}
fn store(directory: &TempDir) -> SqliteRelayStore {
    SqliteRelayStore::new(
        directory.path().join("private/inbox.sqlite"),
        Arc::new(|| 1_000_000),
        Arc::new(TestKey),
    )
}
fn binding() -> Binding {
    Binding::new(
        "11".into(),
        "42".into(),
        "nessalabs/nessa-agent".into(),
        uuid::Uuid::new_v4(),
    )
    .unwrap()
}

fn database(directory: &TempDir) -> Connection {
    let schema = Schema::new(include_str!(
        "../../src/github_relay/infrastructure/schema.sql"
    ))
    .unwrap();
    nessa_local_database::open_encrypted(
        &directory.path().join("private/inbox.sqlite"),
        &schema,
        &DatabaseKey::new([11; 32]),
    )
    .unwrap()
}
fn gap_audits(directory: &TempDir) -> Vec<Value> {
    let db = database(directory);
    let mut query = db.prepare("SELECT user_id,repository_id,before_cursor,after_cursor,cause,initiator FROM audit WHERE cause IN ('cloud_retention_gap','gap_accepted') ORDER BY id").unwrap();
    let rows = query
        .query_map([], |row| {
            Ok(json!({
                "user_id": row.get::<_, String>(0)?,
                "repository_id": row.get::<_, String>(1)?,
                "before_cursor": row.get::<_, i64>(2)?,
                "after_cursor": row.get::<_, i64>(3)?,
                "cause": row.get::<_, String>(4)?,
                "initiator": row.get::<_, String>(5)?,
            }))
        })
        .unwrap();
    rows.collect::<Result<Vec<_>, _>>().unwrap()
}
fn cloud_gap_evidence() -> Value {
    json!({"user_id":"11", "repository_id":"42", "before_cursor":0,
        "after_cursor":1, "cause":"cloud_retention_gap", "initiator":"system:cloud-relay"})
}

#[tokio::test]
async fn worker_retention_gap_pauses_encrypted_catch_up_until_explicit_acceptance() {
    let gap = server(
        StatusCode::GONE,
        serde_json::from_str(include_str!(
            "../../../../protocol/fixtures/github-relay-gap.json"
        ))
        .unwrap(),
    )
    .await;
    let binding = binding();
    assert_eq!(
        gap.source
            .page("fixture_token_1234", &binding, 0)
            .await
            .err(),
        Some(Failure::Gap(1)),
    );
    let directory = tempfile::tempdir().unwrap();
    let inbox = store(&directory);
    inbox.bind(&binding, false).await.unwrap();
    assert_eq!(
        catch_up(&gap.source, &inbox, &binding, &Credentials)
            .await
            .err(),
        Some(Failure::Gap(1))
    );
    assert_eq!(inbox.cursor(&binding).await, Err(Failure::Gap(1)));
    assert!(inbox.events(&binding).await.unwrap().is_empty());
    assert_eq!(gap_audits(&directory), vec![cloud_gap_evidence()]);
    drop(inbox);

    let reopened = store(&directory);
    reopened.bind(&binding, false).await.unwrap();
    assert_eq!(reopened.cursor(&binding).await, Err(Failure::Gap(1)));
    assert_eq!(
        catch_up(&gap.source, &reopened, &binding, &Credentials)
            .await
            .err(),
        Some(Failure::Gap(1)),
    );
    assert_eq!(gap.calls.load(Ordering::SeqCst), 2);
    assert_eq!(gap_audits(&directory), vec![cloud_gap_evidence()]);
    let db = database(&directory);
    db.execute_batch("CREATE TRIGGER refuse_gap_acceptance BEFORE INSERT ON audit WHEN NEW.cause='gap_accepted' BEGIN SELECT RAISE(ABORT,'injected acceptance audit failure'); END;").unwrap();
    assert_eq!(reopened.bind(&binding, true).await, Err(Failure::Storage));
    assert_eq!(reopened.cursor(&binding).await, Err(Failure::Gap(1)));
    assert_eq!(gap_audits(&directory), vec![cloud_gap_evidence()]);
    db.execute_batch("DROP TRIGGER refuse_gap_acceptance;")
        .unwrap();
    reopened.bind(&binding, true).await.unwrap();
    assert_eq!(
        gap_audits(&directory),
        vec![
            cloud_gap_evidence(),
            json!({
                "user_id":"11", "repository_id":"42", "before_cursor":0, "after_cursor":1,
                "cause":"gap_accepted", "initiator":"local-owner:github:11",
            })
        ]
    );
    assert_eq!(reopened.cursor(&binding).await.unwrap(), 1);
    let page = server(
        StatusCode::OK,
        json!({
            "kind": "page", "stage": "dev", "userId": "11", "next": 2,
            "hasMore": false, "ticket": "fixture-ticket",
            "events": [{
                "sequence": 2, "deliveryId": uuid::Uuid::new_v4().to_string(),
                "event": "issues", "payload": "{}", "repositoryId": "42",
                "receivedAt": "2026-10-25T00:00:00Z"
            }]
        }),
    )
    .await;
    assert_eq!(
        catch_up(&page.source, &reopened, &binding, &Credentials)
            .await
            .unwrap()
            .next,
        2
    );
    assert_eq!(reopened.cursor(&binding).await.unwrap(), 2);
    assert_eq!(reopened.events(&binding).await.unwrap().len(), 1);
}

#[tokio::test]
async fn invalid_gap_receipts_leave_the_encrypted_cursor_unmodified() {
    for (body, expected) in [
        (
            json!({"kind":"gap","stage":"prod","expiredThrough":1}),
            Failure::StageMismatch,
        ),
        (
            json!({"kind":"page","stage":"dev","expiredThrough":1}),
            Failure::InvalidResponse,
        ),
        (
            json!({"kind":"gap","stage":"dev"}),
            Failure::InvalidResponse,
        ),
        (
            json!({"kind":"gap","stage":"dev","expired_through":1}),
            Failure::InvalidResponse,
        ),
        (
            json!({"kind":"gap","stage":"dev","expiredThrough":0}),
            Failure::InvalidResponse,
        ),
        (
            json!({"kind":"gap","stage":"dev","expiredThrough":9_007_199_254_740_992_i64}),
            Failure::InvalidResponse,
        ),
    ] {
        let receipt = server(StatusCode::GONE, body).await;
        let directory = tempfile::tempdir().unwrap();
        let inbox = store(&directory);
        let binding = binding();
        inbox.bind(&binding, false).await.unwrap();
        assert_eq!(
            catch_up(&receipt.source, &inbox, &binding, &Credentials)
                .await
                .err(),
            Some(expected)
        );
        assert_eq!(inbox.cursor(&binding).await.unwrap(), 0);
        assert!(inbox.events(&binding).await.unwrap().is_empty());
        assert!(gap_audits(&directory).is_empty());
    }
}

#[tokio::test]
async fn a_cloud_gap_audit_failure_rolls_back_and_the_same_http_receipt_can_retry() {
    let receipt = server(
        StatusCode::GONE,
        serde_json::from_str(include_str!(
            "../../../../protocol/fixtures/github-relay-gap.json"
        ))
        .unwrap(),
    )
    .await;
    let directory = tempfile::tempdir().unwrap();
    let inbox = store(&directory);
    let binding = binding();
    inbox.bind(&binding, false).await.unwrap();
    let db = database(&directory);
    db.execute_batch("CREATE TRIGGER refuse_cloud_gap BEFORE INSERT ON audit WHEN NEW.cause='cloud_retention_gap' BEGIN SELECT RAISE(ABORT,'injected cloud gap audit failure'); END;").unwrap();
    assert_eq!(
        catch_up(&receipt.source, &inbox, &binding, &Credentials)
            .await
            .err(),
        Some(Failure::Storage)
    );
    assert_eq!(inbox.cursor(&binding).await.unwrap(), 0);
    assert!(inbox.events(&binding).await.unwrap().is_empty());
    assert!(gap_audits(&directory).is_empty());
    db.execute_batch("DROP TRIGGER refuse_cloud_gap;").unwrap();
    assert_eq!(
        catch_up(&receipt.source, &inbox, &binding, &Credentials)
            .await
            .err(),
        Some(Failure::Gap(1))
    );
    assert_eq!(inbox.cursor(&binding).await, Err(Failure::Gap(1)));
    assert_eq!(receipt.calls.load(Ordering::SeqCst), 2);
    assert_eq!(gap_audits(&directory), vec![cloud_gap_evidence()]);
}

#[tokio::test]
async fn oversized_refusal_bodies_are_rejected_before_decoding() {
    for (status, mut body) in [
        (StatusCode::CONFLICT, json!({"kind":"identity_changed"})),
        (
            StatusCode::GONE,
            json!({"kind":"gap","stage":"dev","expiredThrough":1}),
        ),
    ] {
        body["padding"] =
            json!("x".repeat(2 * crate::github_relay::domain::contract::PAYLOAD_BYTES + 16_384));
        let receipt = server(status, body).await;
        assert_eq!(
            receipt
                .source
                .page("fixture_token_1234", &binding(), 0)
                .await
                .err(),
            Some(Failure::InvalidResponse)
        );
    }
}

struct Credentials;
#[async_trait::async_trait]
impl crate::github_relay::application::GitHubCredentials for Credentials {
    async fn token(&self) -> Result<crate::github::domain::AccessToken, Failure> {
        Ok(crate::github::domain::AccessToken::new("fixture_token_1234".into()).unwrap())
    }
    async fn identify(&self, _: &str) -> Result<Binding, Failure> {
        Err(Failure::InvalidResponse)
    }
}

#[tokio::test]
async fn chunked_refusals_without_content_length_obey_the_same_body_limit() {
    use axum::body::Body;
    use axum::response::Response;
    for status in [StatusCode::CONFLICT, StatusCode::GONE] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let router = Router::new().route(
            "/relay/v1/events",
            get(move || async move {
                let chunks = futures_util::stream::iter(
                    (0..33).map(|_| Ok::<_, std::io::Error>(vec![b' '; 65_536])),
                );
                Response::builder()
                    .status(status)
                    .body(Body::from_stream(chunks))
                    .unwrap()
            }),
        );
        let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let mut receipt = server(StatusCode::OK, json!({})).await;
        receipt.task.abort();
        receipt.task = task;
        receipt.source.configuration.origin = origin;
        assert_eq!(
            receipt
                .source
                .page("fixture_token_1234", &binding(), 0)
                .await
                .err(),
            Some(Failure::InvalidResponse)
        );
    }
}

async fn watch_peer(frames: Vec<u8>) -> (super::RelayWatch, JoinHandle<()>) {
    use tokio::io::AsyncWriteExt;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let request = format!("ws://{}/", listener.local_addr().unwrap())
        .into_client_request()
        .unwrap();
    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        let mut stream = socket.into_inner();
        stream.write_all(&frames).await.unwrap();
        std::future::pending::<()>().await;
    });
    (super::RelayWatch::connect(request).await.unwrap(), task)
}

#[tokio::test]
async fn watch_rejects_oversized_frames_from_the_header() {
    use futures_util::StreamExt;
    use tokio_tungstenite::tungstenite::Error;
    // Advertise 256 bytes with no payload: refusal must precede waiting for it.
    for opcode in [0x81, 0x82] {
        let (mut watch, task) = watch_peer(vec![opcode, 126, 1, 0]).await;
        let result = tokio::time::timeout(Duration::from_secs(2), watch.0.next()).await;
        task.abort();
        let _ = task.await;
        assert!(matches!(result, Ok(Some(Err(Error::Capacity(_))))));
    }
}

#[tokio::test]
async fn watch_bounds_fragment_assembly() {
    use futures_util::StreamExt;
    use tokio_tungstenite::tungstenite::Error;
    // Both frames fit the frame bound; the assembled 20-byte message must fail.
    let mut fragments = vec![0x01, 10];
    fragments.extend_from_slice(&[b'x'; 10]);
    fragments.extend_from_slice(&[0x80, 10]);
    fragments.extend_from_slice(&[b'x'; 10]);
    let (mut watch, task) = watch_peer(fragments).await;
    let result = tokio::time::timeout(Duration::from_secs(2), watch.0.next()).await;
    task.abort();
    let _ = task.await;
    assert!(matches!(result, Ok(Some(Err(Error::Capacity(_))))));
}

#[tokio::test]
async fn watch_accepts_fragmented_changed_notifications_and_full_control_frames() {
    use crate::github_relay::application::RelayNotifications;
    use futures_util::{SinkExt, StreamExt};
    use tokio::io::AsyncWriteExt;
    use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let request = format!("ws://{}/", listener.local_addr().unwrap())
        .into_client_request()
        .unwrap();
    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        socket
            .send(Message::Ping(vec![7; 125].into()))
            .await
            .unwrap();
        assert_eq!(
            socket.next().await.unwrap().unwrap(),
            Message::Pong(vec![7; 125].into())
        );
        let mut stream = socket.into_inner();
        let mut frames = vec![0x01, 8];
        frames.extend_from_slice(&super::CHANGED.as_bytes()[..8]);
        frames.extend_from_slice(&[0x80, (super::CHANGED.len() - 8) as u8]);
        frames.extend_from_slice(&super::CHANGED.as_bytes()[8..]);
        stream.write_all(&frames).await.unwrap();
    });
    let mut watch = super::RelayWatch::connect(request).await.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), watch.changed()).await;
    if result.is_err() {
        task.abort();
    }
    task.await.unwrap();
    assert_eq!(result.unwrap(), Ok(()));
}
