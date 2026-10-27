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
        catch_up(&gap.source, &inbox, &binding, "fixture_token_1234")
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
        catch_up(&gap.source, &reopened, &binding, "fixture_token_1234")
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
        catch_up(&page.source, &reopened, &binding, "fixture_token_1234")
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
            catch_up(&receipt.source, &inbox, &binding, "fixture_token_1234")
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
        catch_up(&receipt.source, &inbox, &binding, "fixture_token_1234")
            .await
            .err(),
        Some(Failure::Storage)
    );
    assert_eq!(inbox.cursor(&binding).await.unwrap(), 0);
    assert!(inbox.events(&binding).await.unwrap().is_empty());
    assert!(gap_audits(&directory).is_empty());
    db.execute_batch("DROP TRIGGER refuse_cloud_gap;").unwrap();
    assert_eq!(
        catch_up(&receipt.source, &inbox, &binding, "fixture_token_1234")
            .await
            .err(),
        Some(Failure::Gap(1))
    );
    assert_eq!(inbox.cursor(&binding).await, Err(Failure::Gap(1)));
    assert_eq!(receipt.calls.load(Ordering::SeqCst), 2);
    assert_eq!(gap_audits(&directory), vec![cloud_gap_evidence()]);
}
