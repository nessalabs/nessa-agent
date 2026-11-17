// The saved-login methods at the socket (issue #1260): refused without
// `credential.manage`, shape-checked before anything is kept,
// `logins_not_configured` without the service, and no answer ever holding
// a login.
use super::*;
use crate::logins::application::{
    AuditUnavailable, LoginAudit, LoginFacts, LoginPolicyEvaluator, LoginRecord,
    SavedLoginsService,
};
use crate::secrets::{SecretStore, SecretUnavailable};
use nessa_protocol::agents::AgentId;

const LOGIN: &str = "sk-ant-oat01-route-test";
const PEER: &str = "peer:0101010101010101010101010101010101010101010101010101010101010101";

#[derive(Default)]
struct Memory(Mutex<Option<Vec<u8>>>);

impl SecretStore for Memory {
    fn read(&self) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>, SecretUnavailable> {
        Ok(self.0.lock().unwrap().clone().map(zeroize::Zeroizing::new))
    }
    fn write(&self, bytes: &[u8]) -> Result<(), SecretUnavailable> {
        *self.0.lock().unwrap() = Some(bytes.to_vec());
        Ok(())
    }
    fn delete(&self) -> Result<(), SecretUnavailable> {
        *self.0.lock().unwrap() = None;
        Ok(())
    }
}

#[derive(Default)]
struct Kept(Mutex<Vec<LoginRecord>>);

impl LoginAudit for Kept {
    fn record(&self, record: &LoginRecord) -> Result<(), AuditUnavailable> {
        self.0.lock().unwrap().push(record.clone());
        Ok(())
    }
}

struct Nobody;
impl LoginPolicyEvaluator for Nobody {
    fn may_receive(
        &self,
        _facts: &LoginFacts<'_>,
    ) -> Result<bool, nessa_auth::application::ports::AccessError> {
        Ok(false)
    }
}

fn logins() -> (Arc<SavedLoginsService>, Arc<Memory>, Arc<Kept>) {
    let claude = Arc::new(Memory::default());
    let kept = Arc::new(Kept::default());
    let mut stores: std::collections::BTreeMap<AgentId, Arc<dyn SecretStore>> = AgentId::ALL
        .iter()
        .map(|agent| (*agent, Arc::new(Memory::default()) as Arc<dyn SecretStore>))
        .collect();
    stores.insert(AgentId::Claude, claude.clone());
    let service = Arc::new(SavedLoginsService::new(
        stores,
        Arc::new(Memory::default()),
        Arc::new(Nobody),
        kept.clone(),
    ));
    (service, claude, kept)
}

fn with(id: &str, method: &str, params: Value) -> RequestFrame {
    let mut frame = request(id, method);
    frame.params = params;
    frame
}

fn payload(message: OutgoingMessage) -> Value {
    let OutgoingMessage::Response(response) = message else {
        panic!("response expected");
    };
    assert!(response.ok, "{:?}", response.error);
    response.payload.unwrap()
}

#[tokio::test]
async fn without_saved_logins_every_method_says_it_is_not_configured() {
    let (state, _) = fixture(MembershipRole::Admin);
    let session = authenticate(&state).await;
    assert_eq!(
        code_of(dispatch(&state, &session, request("1", "logins.status")).await),
        "logins_not_configured"
    );
}

#[tokio::test]
async fn the_logins_methods_need_credential_manage() {
    let (service, claude, _) = logins();
    let (state, authority) = fixture(MembershipRole::Member);
    let state = state.with_logins(service);
    *authority.snapshot.lock().unwrap() = snapshot_with(
        MembershipRole::Member,
        MembershipStatus::Active,
        &["conversation.write"],
    );
    let session = authenticate(&state).await;
    let save = with("1", "logins.save", json!({ "agent": "claude", "login": LOGIN }));
    assert_eq!(code_of(dispatch(&state, &session, save).await), "forbidden");
    assert!(claude.0.lock().unwrap().is_none());
    let switch = with("2", "logins.setDevice", json!({ "environment": PEER, "on": true }));
    assert_eq!(code_of(dispatch(&state, &session, switch).await), "forbidden");
}

#[tokio::test]
async fn a_saved_login_is_never_answered_and_shapes_are_checked_first() {
    let (service, claude, kept) = logins();
    let (state, authority) = fixture(MembershipRole::Admin);
    *authority.snapshot.lock().unwrap() = snapshot_with(
        MembershipRole::Admin,
        MembershipStatus::Active,
        &["credential.manage"],
    );
    let state = state.with_logins(service);
    let session = authenticate(&state).await;
    for (id, method, params) in [
        ("1", "logins.save", json!({ "agent": "claude", "login": "has a space" })),
        ("2", "logins.save", json!({ "agent": "claude", "login": "" })),
        ("3", "logins.save", json!({ "agent": "gemini", "login": LOGIN })),
        ("4", "logins.save", json!({ "agent": "claude", "login": LOGIN, "extra": 1 })),
        ("5", "logins.setDevice", json!({ "environment": "here", "on": true })),
        ("6", "logins.remove", json!({})),
    ] {
        assert_eq!(
            code_of(dispatch(&state, &session, with(id, method, params)).await),
            "invalid_request",
            "{id}"
        );
    }
    assert!(kept.0.lock().unwrap().is_empty());
    let saved = payload(
        dispatch(
            &state,
            &session,
            with("7", "logins.save", json!({ "agent": "claude", "login": LOGIN })),
        )
        .await,
    );
    assert_eq!(saved["logins"][0], json!({ "agent": "claude", "saved": true }));
    assert_eq!(saved["logins"][1], json!({ "agent": "codex", "saved": false }));
    assert!(!saved.to_string().contains(LOGIN));
    assert_eq!(claude.0.lock().unwrap().as_deref(), Some(LOGIN.as_bytes()));
    let switched = payload(
        dispatch(
            &state,
            &session,
            with("8", "logins.setDevice", json!({ "environment": PEER, "on": true })),
        )
        .await,
    );
    assert_eq!(switched["devicesOn"], json!([PEER]));
    let removed = payload(
        dispatch(
            &state,
            &session,
            with("9", "logins.remove", json!({ "agent": "claude" })),
        )
        .await,
    );
    assert_eq!(removed["logins"][0]["saved"], false);
    assert!(claude.0.lock().unwrap().is_none());
    let status = payload(dispatch(&state, &session, request("10", "logins.status")).await);
    assert_eq!(status["devicesOn"], json!([PEER]));
    assert!(!format!("{:?}", kept.0.lock().unwrap()).contains(LOGIN));
}
