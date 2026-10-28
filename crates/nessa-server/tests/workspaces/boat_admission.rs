//! Hostile provider responses cross real HTTP; no live account or VM is used.
use crate::workspaces::{
    application::Provider,
    domain::{AdmissionRefusal, BoatConnection, Failure},
    infrastructure::BoatProvider,
};
use axum::{
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tokio::task::JoinHandle;

const VM: &str = "bx_23456789";
fn environment() -> Value {
    serde_json::from_str(include_str!("fixtures/boat-v1-environment.json")).unwrap()
}

fn sandbox() -> Value {
    let mut value: Value =
        serde_json::from_str(include_str!("fixtures/boat-v1-sandbox.json")).unwrap();
    value["sandbox"]["id"] = json!(VM);
    value["sandbox"]["state"] = json!("ready");
    value
}
struct State {
    environment: Value,
    receipt: Value,
    created: usize,
    resumed: usize,
    commands: usize,
    drift_on_create: bool,
    drift_on_resume: bool,
    creation_ack: Value,
    read_override: Option<Value>,
    expected_ttl: Option<u32>,
}
struct Server {
    provider: BoatProvider,
    state: Arc<Mutex<State>>,
    task: JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn server() -> Server {
    let state = Arc::new(Mutex::new(State {
        environment: environment(),
        receipt: sandbox(),
        created: 0,
        resumed: 0,
        commands: 0,
        drift_on_create: false,
        drift_on_resume: false,
        creation_ack: json!({"ok":true,"type":"sandbox.created","status":"provisioning"}),
        read_override: None,
        expected_ttl: Some(3600),
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/api/v1", listener.local_addr().unwrap());
    let env = state.clone();
    let create = state.clone();
    let read = state.clone();
    let resume = state.clone();
    let commands = state.clone();
    let router = Router::new()
        .route("/api/v1/me", get(|| async move { Json(json!({"ok":true,"type":"me","user":{"id":"11111111-1111-4111-8111-111111111111"}})) }))
        .route("/api/v1/environments", get(move || { let env=env.lock().unwrap().environment.clone(); async move { Json(json!({"ok":true,"type":"environments.list","environments":[env]})) } }))
        .route("/api/v1/sandboxes", post(move |headers:axum::http::HeaderMap, Json(request):Json<Value>| {
            assert_eq!(headers.get("authorization").unwrap(),"Bearer fixture-token");
            assert_eq!(headers.get("x-boat-org").unwrap(),"personal");
            assert_eq!(headers.get("idempotency-key").unwrap(),"nessa-thread-fixture-thread");
            let mut state=create.lock().unwrap();
            assert_eq!(request,json!({"type":"small","ttlSeconds":state.expected_ttl,"environment":"nessa","env":{"NESSA_THREAD_ID":"fixture-thread"},"from":"bsh_fixture","snapshots":true}));
            state.created+=1; if state.drift_on_create { state.environment["passGithub"]=json!(true); } let mut body=state.receipt.clone(); for field in ["ok","type","status"] {body.as_object_mut().unwrap().remove(field);} for (field,value) in state.creation_ack.as_object().unwrap() {body[field.as_str()]=value.clone();} body["ttlSeconds"]=json!(state.expected_ttl); async { Json(body) } }))
        .route("/api/v1/sandboxes/{id}", get(move || { let state=read.lock().unwrap(); let body=state.read_override.as_ref().unwrap_or(&state.receipt).clone(); async { Json(body) } }))
        .route("/api/v1/sandboxes/{id}/resume", post(move |Json(request):Json<Value>| { let mut state=resume.lock().unwrap(); assert_eq!(request["ttlSeconds"],json!(state.expected_ttl)); state.resumed+=1; if state.drift_on_resume { state.environment["passSecrets"]=json!(true); } state.receipt["sandbox"]["state"]=json!("ready"); async move { Json(json!({"ok":true,"type":"sandbox.resumed","id":VM,"status":"ready"})) } }))
        .fallback(move || { commands.lock().unwrap().commands+=1; async { axum::http::StatusCode::FORBIDDEN } });
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(rustls::RootCertStore::empty())
    .with_no_client_auth();
    let provider = BoatProvider {
        api_base: base,
        http: reqwest::Client::builder()
            .tls_backend_preconfigured(tls)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap(),
        relay_socket: "unused".into(),
        relay_program: "unused".into(),
    };
    Server {
        provider,
        state,
        task,
    }
}
fn connection() -> BoatConnection {
    let mut connection =
        BoatConnection::new("fixture-token".into(), "nessa".into(), "bsh_fixture".into()).unwrap();
    connection.auto_stop =
        crate::conversation::domain::VmLifetime::from_seconds(Some(3600)).unwrap();
    connection
}

#[tokio::test]
async fn admitted_creation_sends_explicit_null_for_no_automatic_stop() {
    let server = server().await;
    {
        let mut state = server.state.lock().unwrap();
        state.expected_ttl = None;
        state.receipt["sandbox"]["ttlSeconds"] = Value::Null;
    }
    let mut unlimited = connection();
    unlimited.auto_stop = crate::conversation::domain::VmLifetime::Unlimited;
    let receipt = server
        .provider
        .create(&unlimited, "fixture-thread")
        .await
        .unwrap();
    assert_eq!(receipt.vm, VM);
    assert!(receipt.refusal.is_none());
    assert_eq!(server.state.lock().unwrap().created, 1);
}

#[tokio::test]
async fn full_agent_only_posture_rejects_each_extra_credential_and_setup_path_before_create() {
    let server = server().await;
    server.provider.validate(&connection()).await.unwrap();
    for (field, value) in [
        ("safeForThirdParties", json!(true)),
        ("passAgentsCredentials", json!(false)),
        ("passGithub", json!(true)),
        ("passSecrets", json!(true)),
        ("passSandboxCredentials", json!(true)),
        ("envContents", json!("ATTACK=1")),
        ("secretFiles", json!([{"path":".x","contents":"x"}])),
        ("selectedRepositories", json!([{"setupScript":"attack"}])),
        ("credentialProfileId", json!("unexpected")),
        ("proxyAiCredentials", json!(true)),
        ("access", json!("use")),
        ("access", json!({"restricted":true,"members":[]})),
        ("access", json!({"restricted":false,"members":["other"]})),
        (
            "effective",
            json!({"passGithub":true,"passSecrets":false,"passSandboxCredentials":false,"passAgentsCredentials":true}),
        ),
        ("futureCredentialSource", json!(true)),
        ("latestVersionNumber", json!(3)),
        (
            "latestVersionId",
            json!("33333333-3333-4333-8333-333333333333"),
        ),
    ] {
        let mut bad = environment();
        bad[field] = value;
        server.state.lock().unwrap().environment = bad;
        assert_eq!(
            server
                .provider
                .create(&connection(), "fixture-thread")
                .await
                .err(),
            Some(Failure::EnvironmentUnavailable),
            "{field}"
        );
    }
    for field in [
        "credentialProfileId",
        "proxyAiCredentials",
        "latestVersionNumber",
        "access",
        "effective",
    ] {
        let mut bad = environment();
        bad.as_object_mut().unwrap().remove(field);
        server.state.lock().unwrap().environment = bad;
        assert_eq!(
            server
                .provider
                .create(&connection(), "fixture-thread")
                .await
                .err(),
            Some(Failure::EnvironmentUnavailable),
            "missing {field}"
        );
    }
    for field in [
        "passGithub",
        "passSecrets",
        "passSandboxCredentials",
        "passAgentsCredentials",
    ] {
        let mut bad = environment();
        bad["effective"][field] = json!(!bad["effective"][field].as_bool().unwrap());
        server.state.lock().unwrap().environment = bad;
        assert_eq!(
            server
                .provider
                .create(&connection(), "fixture-thread")
                .await
                .err(),
            Some(Failure::EnvironmentUnavailable),
            "effective.{field}"
        );
    }
    assert_eq!(server.state.lock().unwrap().created, 0);
}

#[tokio::test]
async fn post_create_environment_drift_retains_the_real_target_but_grants_no_authority() {
    let server = server().await;
    server.state.lock().unwrap().drift_on_create = true;
    let receipt = server
        .provider
        .create(&connection(), "fixture-thread")
        .await
        .unwrap();
    assert_eq!(receipt.vm, VM);
    assert_eq!(
        receipt.refusal.map(|r| r.cause()),
        Some(AdmissionRefusal::EnvironmentUnavailable)
    );
    assert_eq!(server.state.lock().unwrap().commands, 0);
}

#[tokio::test]
async fn mismatched_vm_receipts_retain_targets_without_extending_workspace_authority() {
    let server = server().await;
    for (field, value, cause) in [
        (
            "environment",
            Value::Null,
            AdmissionRefusal::MissingEnvironment,
        ),
        (
            "environment",
            json!("other"),
            AdmissionRefusal::WrongEnvironment,
        ),
        (
            "environmentVersion",
            Value::Null,
            AdmissionRefusal::MissingVersion,
        ),
        (
            "environmentVersion",
            json!(3),
            AdmissionRefusal::WrongVersion,
        ),
        (
            "environmentVersion",
            json!(4_294_967_296_u64),
            AdmissionRefusal::InvalidReceipt,
        ),
        (
            "memoryGB",
            json!("invalid"),
            AdmissionRefusal::InvalidReceipt,
        ),
        ("state", json!("unknown"), AdmissionRefusal::InvalidReceipt),
        (
            "futureCredentialSource",
            json!(true),
            AdmissionRefusal::InvalidReceipt,
        ),
        (
            "team",
            json!({"id":"other","name":"Other"}),
            AdmissionRefusal::InvalidReceipt,
        ),
        (
            "visibility",
            json!("shared"),
            AdmissionRefusal::InvalidReceipt,
        ),
        (
            "wipePendingUntilRestart",
            json!(true),
            AdmissionRefusal::InvalidReceipt,
        ),
        (
            "setupStatus",
            json!("running"),
            AdmissionRefusal::InvalidReceipt,
        ),
    ] {
        let mut body = sandbox();
        body["sandbox"][field] = value;
        server.state.lock().unwrap().receipt = body;
        let receipt = server
            .provider
            .create(&connection(), "fixture-thread")
            .await
            .unwrap();
        assert_eq!(receipt.vm, VM);
        assert_eq!(receipt.refusal.map(|r| r.cause()), Some(cause), "{field}");
    }
    assert_eq!(server.state.lock().unwrap().commands, 0);
}

#[tokio::test]
async fn reconnect_refuses_changed_environment_identity_before_resume() {
    let server = server().await;
    let receipt = server
        .provider
        .create(&connection(), "fixture-thread")
        .await
        .unwrap();
    assert_eq!(receipt.refusal, None);
    {
        let mut state = server.state.lock().unwrap();
        state.receipt["sandbox"]["state"] = json!("archived");
        state.environment["id"] = json!("33333333-3333-4333-8333-333333333333");
    }
    assert_eq!(
        server
            .provider
            .connect(&connection(), "fixture-thread", VM, &receipt.admission)
            .await
            .err(),
        Some(Failure::AdmissionRejected(
            crate::workspaces::domain::AdmissionRejection::new(
                AdmissionRefusal::EnvironmentChanged,
                None
            )
        ))
    );
    let state = server.state.lock().unwrap();
    assert_eq!(state.resumed, 0);
    assert_eq!(state.commands, 0);
}

#[tokio::test]
async fn resume_rechecks_environment_before_any_sdk_command_or_grant() {
    let server = server().await;
    let receipt = server
        .provider
        .create(&connection(), "fixture-thread")
        .await
        .unwrap();
    {
        let mut state = server.state.lock().unwrap();
        state.receipt["sandbox"]["state"] = json!("archived");
        state.drift_on_resume = true;
    }
    assert_eq!(
        server
            .provider
            .connect(&connection(), "fixture-thread", VM, &receipt.admission)
            .await
            .err(),
        Some(Failure::AdmissionRejected(
            crate::workspaces::domain::AdmissionRejection::new(
                AdmissionRefusal::EnvironmentUnavailable,
                None
            )
        ))
    );
    let state = server.state.lock().unwrap();
    assert_eq!(state.resumed, 1);
    assert_eq!(state.commands, 0);
}

#[tokio::test]
async fn archived_resume_sends_explicit_null_after_lifetime_changes() {
    let server = server().await;
    let receipt = server
        .provider
        .create(&connection(), "fixture-thread")
        .await
        .unwrap();
    {
        let mut state = server.state.lock().unwrap();
        state.receipt["sandbox"]["state"] = json!("archived");
        state.expected_ttl = None;
        state.drift_on_resume = true;
    }
    let mut unlimited = connection();
    unlimited.auto_stop = crate::conversation::domain::VmLifetime::Unlimited;
    assert!(matches!(
        server
            .provider
            .connect(&unlimited, "fixture-thread", VM, &receipt.admission)
            .await,
        Err(Failure::AdmissionRejected(_))
    ));
    assert_eq!(server.state.lock().unwrap().resumed, 1);
}

#[tokio::test]
async fn a_personal_vm_without_stored_logins_does_not_gain_extra_authority() {
    let server = server().await;
    server.state.lock().unwrap().receipt["sandbox"]["holdsCreatorLogins"] = json!(false);
    let receipt = server
        .provider
        .create(&connection(), "fixture-thread")
        .await
        .unwrap();
    assert_eq!(receipt.refusal, None);
    server.state.lock().unwrap().receipt["sandbox"]
        .as_object_mut()
        .unwrap()
        .remove("holdsCreatorLogins");
    let receipt = server
        .provider
        .create(&connection(), "fixture-thread")
        .await
        .unwrap();
    assert_eq!(
        receipt.refusal.map(|r| r.cause()),
        Some(AdmissionRefusal::InvalidReceipt)
    );
    assert_eq!(server.state.lock().unwrap().commands, 0);
}

#[tokio::test]
async fn replayed_or_unrecognized_creation_receipts_do_not_authorize_rollback() {
    use crate::workspaces::domain::RefusalDisposition;
    let server = server().await;
    for ack in [
        json!({"ok":true,"type":"sandbox.created","status":"ready"}),
        json!({"ok":false,"type":"sandbox.created","status":"provisioning"}),
        json!({"ok":true,"type":"unrecognized","status":"provisioning"}),
        json!({"ok":true,"type":"sandbox.created","status":"unrecognized"}),
        json!({"type":"sandbox.created","status":"provisioning"}),
        json!({"ok":true,"status":"provisioning"}),
        json!({"ok":true,"type":"sandbox.created"}),
    ] {
        {
            let mut state = server.state.lock().unwrap();
            state.creation_ack = ack;
            state.receipt["sandbox"]["environmentVersion"] = json!(3);
        }
        let receipt = server
            .provider
            .create(&connection(), "fixture-thread")
            .await
            .unwrap();
        assert_eq!(receipt.vm, VM);
        assert_eq!(receipt.disposition, RefusalDisposition::RetainTarget);
        assert!(receipt.refusal.is_some());
    }
    assert_eq!(server.state.lock().unwrap().commands, 0);
}

#[tokio::test]
async fn oversized_vm_bodies_are_bounded_without_losing_a_previously_received_target() {
    let server = server().await;
    let mut large = sandbox();
    large["sandbox"]["labels"] = json!({"padding":"x".repeat(65_537)});
    server.state.lock().unwrap().receipt = large.clone();
    assert_eq!(
        server
            .provider
            .create(&connection(), "fixture-thread")
            .await
            .err(),
        Some(Failure::ProvisioningUncertain)
    );
    {
        let mut state = server.state.lock().unwrap();
        state.receipt = sandbox();
        state.read_override = Some(large);
    }
    let receipt = server
        .provider
        .create(&connection(), "fixture-thread")
        .await
        .unwrap();
    assert_eq!(receipt.vm, VM);
    assert_eq!(
        receipt.refusal.map(|r| r.cause()),
        Some(AdmissionRefusal::InvalidReceipt)
    );
    assert_eq!(server.state.lock().unwrap().commands, 0);
}
