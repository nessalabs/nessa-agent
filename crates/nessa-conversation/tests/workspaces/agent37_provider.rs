//! Nessa's Agent37 adapter against a fake Sandbox API: what a creation's
//! answer becomes in a receipt, and which instances it may delete.
use crate::workspaces::{
    application::{Provider, RestCause},
    domain::{
        AdmissionRefusal, Admitted, Agent37Connection, CloudAccount, CloudConnection, Failure,
        RefusalDisposition, VmSpec,
    },
    infrastructure::Agent37Provider,
};
use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

const TEMPLATE: &str = "nessa-runtime@1";
const DIGEST: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

#[derive(Default)]
struct Fake {
    /// What a creation answers with, over the instance it makes.
    made: Value,
    calls: Vec<String>,
    instances: Vec<Value>,
}
type Shared = Arc<Mutex<Fake>>;

async fn list(State(fake): State<Shared>) -> Json<Value> {
    let fake = fake.lock().unwrap();
    Json(json!({ "data": fake.instances }))
}
async fn create(State(fake): State<Shared>, Json(body): Json<Value>) -> Json<Value> {
    let mut fake = fake.lock().unwrap();
    fake.calls.push("create".into());
    let mut made = json!({
        "id": "ab12cd34ef", "status": "running", "template": body["template"],
        "template_revision": 1, "image_digest": DIGEST, "metadata": body["metadata"],
    });
    if let (Some(made), Some(over)) = (made.as_object_mut(), fake.made.as_object()) {
        made.extend(over.clone());
    }
    fake.instances.push(made.clone());
    Json(made)
}
async fn delete(State(fake): State<Shared>, Path(id): Path<String>) -> Json<Value> {
    fake.lock().unwrap().calls.push(format!("delete {id}"));
    Json(json!({ "id": id, "deleted": true }))
}

async fn fake(made: Value) -> (Agent37Provider, Shared) {
    let shared = Shared::default();
    shared.lock().unwrap().made = made;
    let router = Router::new()
        .route("/v1/instances", get(list).post(create))
        .route("/v1/instances/{id}", axum::routing::delete(delete))
        .route(
            "/v1/ssh-keys",
            post(|| async { Json(json!({ "id": "k" })) }),
        )
        .with_state(shared.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (
        Agent37Provider {
            relay_socket: "/tmp/nessa-relay.sock".into(),
            relay_program: "/usr/bin/true".into(),
            endpoints: Some((
                format!("http://{address}/v1"),
                "ws://127.0.0.1:9/{id}".into(),
            )),
        },
        shared,
    )
}

async fn account() -> CloudAccount {
    let identity = workspace_sdk::providers::agent37::Agent37Identity::generate("test")
        .await
        .unwrap();
    let connection = Agent37Connection::new(
        "sk_live_fixture".into(),
        identity.private_key().to_vec(),
        identity.public_key().into(),
        TEMPLATE.into(),
    )
    .unwrap();
    CloudConnection::Agent37(connection).account()
}
fn spec() -> VmSpec {
    CloudConnection::Agent37(
        Agent37Connection::new(
            "sk_live_fixture".into(),
            b"-".to_vec(),
            "ssh-ed25519 AAAA".into(),
            TEMPLATE.into(),
        )
        .unwrap(),
    )
    .new_vm(
        None,
        crate::domain::VmLifetime::from_seconds(Some(3600)).unwrap(),
    )
}

#[tokio::test]
async fn an_admitted_creation_is_a_receipt_holding_its_release_and_digest() {
    let (provider, _) = fake(json!({})).await;
    let receipt = provider
        .create(&account().await, &spec(), "fixture-thread")
        .await
        .unwrap();
    assert_eq!(receipt.vm, "ab12cd34ef");
    assert!(receipt.refusal.is_none());
    let Admitted::Agent37(release) = receipt.admission else {
        panic!("not an Agent37 admission")
    };
    assert_eq!(
        (release.template(), release.revision(), release.digest()),
        (TEMPLATE, Some(1), Some(DIGEST))
    );
}

/// A refused instance is named with why, so the workspace records and
/// removes it; one that does not carry the thread's key is never removed.
#[tokio::test]
async fn a_refused_creation_names_its_instance_and_who_may_delete_it() {
    let (provider, shared) = fake(json!({ "template": "agent37-base" })).await;
    let receipt = provider
        .create(&account().await, &spec(), "fixture-thread")
        .await
        .unwrap();
    assert_eq!(
        receipt.refusal.as_ref().map(|r| r.cause()),
        Some(AdmissionRefusal::WrongEnvironment)
    );
    assert_eq!(receipt.disposition, RefusalDisposition::RollbackNewVm);
    provider
        .remove(&account().await, &receipt.vm, &receipt.admission)
        .await
        .unwrap();
    assert_eq!(
        shared.lock().unwrap().calls,
        ["create", "delete ab12cd34ef"]
    );

    let (provider, _) = fake(json!({ "metadata": { "workspace_sdk_key": "someone-else" } })).await;
    let receipt = provider
        .create(&account().await, &spec(), "fixture-thread")
        .await
        .unwrap();
    assert_eq!(
        (
            receipt.refusal.as_ref().map(|r| r.cause()),
            receipt.disposition
        ),
        (
            Some(AdmissionRefusal::InvalidReceipt),
            RefusalDisposition::RetainTarget
        )
    );
}

#[tokio::test]
async fn a_boat_account_or_admission_is_refused_before_anything_is_sent() {
    let (provider, shared) = fake(json!({})).await;
    let boat = CloudConnection::Boat(
        crate::workspaces::domain::BoatConnection::new(
            "token".into(),
            "nessa".into(),
            "image".into(),
        )
        .unwrap(),
    )
    .account();
    assert_eq!(
        provider.create(&boat, &spec(), "t").await.err(),
        Some(Failure::InvalidInput)
    );
    assert_eq!(
        provider.stop(&boat, "ab12cd34ef", RestCause::Idle).await,
        Err(Failure::InvalidInput)
    );
    assert!(shared.lock().unwrap().calls.is_empty());
}

/// An instance reporting a template or digest Nessa cannot keep is still
/// named, refused and deleted: never left billing with no receipt.
#[tokio::test]
async fn an_unreadable_report_is_refused_and_removed_not_lost() {
    let (provider, _) = fake(json!({ "image_digest": "sha512:not-a-sha256" })).await;
    let receipt = provider
        .create(&account().await, &spec(), "fixture-thread")
        .await
        .unwrap();
    assert_eq!(
        (
            receipt.refusal.as_ref().map(|r| r.cause()),
            receipt.disposition
        ),
        (
            Some(AdmissionRefusal::WrongEnvironment),
            RefusalDisposition::RollbackNewVm
        )
    );
    let Admitted::Agent37(release) = receipt.admission else {
        panic!("not an Agent37 admission")
    };
    assert_eq!((release.template(), release.digest()), (TEMPLATE, None));
}

/// One that does not carry the thread's key is kept even when its report
/// cannot be read.
#[tokio::test]
async fn an_unreadable_report_from_someone_elses_instance_is_kept() {
    let (provider, shared) = fake(json!({
        "image_digest": "sha512:not-a-sha256",
        "metadata": { "workspace_sdk_key": "someone-else" },
    }))
    .await;
    let receipt = provider
        .create(&account().await, &spec(), "fixture-thread")
        .await
        .unwrap();
    assert_eq!(receipt.disposition, RefusalDisposition::RetainTarget);
    assert_eq!(shared.lock().unwrap().calls, ["create"]);
}

/// Against Agent37 itself, with the published runtime template: Nessa's
/// provider registers its key, makes a thread's VM, connects with the relay
/// forward, asks the image's `nessa` for its lease protocol, stops it and
/// deletes it. Billed, so run only when asked, with AGENT37_API_KEY set:
/// `cargo test -p nessa-conversation --lib agent37_live -- --ignored`.
#[tokio::test]
#[ignore = "makes a billed Agent37 VM; needs AGENT37_API_KEY and the published template"]
async fn agent37_live_a_thread_vm_is_made_connected_stopped_and_deleted() {
    let key = std::env::var("AGENT37_API_KEY").expect("AGENT37_API_KEY");
    let template = crate::workspaces::infrastructure::agent37_template()
        .expect("this build pins a published template");
    let identity = workspace_sdk::providers::agent37::Agent37Identity::generate("nessa-live")
        .await
        .unwrap();
    let connection = CloudConnection::Agent37(
        Agent37Connection::new(
            key,
            identity.private_key().to_vec(),
            identity.public_key().into(),
            template.into(),
        )
        .unwrap(),
    );
    let place = tempfile::tempdir().unwrap();
    let relay = place.path().join("relay.sock");
    let _listening = tokio::net::UnixListener::bind(&relay).unwrap();
    let provider = Agent37Provider {
        relay_socket: relay,
        relay_program: "/usr/bin/true".into(),
        endpoints: None,
    };
    let account = connection.account();
    let spec = connection.new_vm(
        None,
        crate::domain::VmLifetime::from_seconds(Some(900)).unwrap(),
    );
    provider.validate(&account, &spec).await.unwrap();
    let thread = uuid::Uuid::new_v4().to_string();
    let receipt = provider.create(&account, &spec, &thread).await.unwrap();
    eprintln!("made {} under {:?}", receipt.vm, receipt.admission);
    let outcome = async {
        assert!(receipt.refusal.is_none(), "{:?}", receipt.refusal);
        // A key registered moments ago reaches a running VM within minutes.
        let mut runtime = None;
        for attempt in 0..20 {
            match provider
                .connect(&account, &spec, &thread, &receipt.vm, &receipt.admission)
                .await
            {
                Ok(connected) => {
                    runtime = Some(connected);
                    break;
                }
                Err(error) => eprintln!("connect attempt {attempt}: {error:?}"),
            }
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        }
        let runtime = runtime.expect("never connected");
        let protocol = runtime.image_protocol().await;
        eprintln!("the image's nessa speaks {protocol:?}");
        assert_eq!(protocol.as_deref(), Some(crate::env::LEASE_PROTOCOL));
        drop(runtime);
        provider
            .stop(&account, &receipt.vm, RestCause::Closed)
            .await
            .unwrap();
    }
    .await;
    provider
        .remove(&account, &receipt.vm, &receipt.admission)
        .await
        .unwrap();
    eprintln!("deleted {}", receipt.vm);
    outcome
}
