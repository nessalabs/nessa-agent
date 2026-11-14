//! A phone may answer once. Remembering a permission rule is `conversation.remember`,
//! which this purpose forbids before the rule catalog is asked.
use super::{fixture, ActualHolding};
use crate::agents_test_support::StubAgentProbe;
use crate::conversation::application::{
    ConversationLimits, OwnerPhoneDecision, OwnerPhoneOperation, OwnerPhonePolicyEvaluator,
    OwnerPhonePolicyFacts, PermissionRuleCatalog, RulePrincipal,
};
use crate::conversation::infrastructure::{CedarOwnerPhonePolicyEvaluator, PermissionMemory};
use crate::conversation_test_support::{self, EveryConversationGranted};
use crate::product::{dispatch_conversation, ProductDependencies, ProductRouteState};
use nessa_auth::adapters::cedar::CedarPolicyEvaluator;
use nessa_auth::application::ports::{AccessError, Clock};
use nessa_auth::domain::{AudienceId, OrganizationId, ResourceId};
use nessa_protocol::protocol::{OutgoingMessage, RequestFrame, ResponseFrame};
use nessa_sdk::application::agent_execution::permissions::PermissionRules;
use nessa_sdk::domain::agent_execution::executions::{ExecutionId, Initiation};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

struct RecordPolicy {
    inner: CedarOwnerPhonePolicyEvaluator,
    seen: Mutex<Vec<String>>,
}
impl OwnerPhonePolicyEvaluator for RecordPolicy {
    fn evaluate(
        &self,
        facts: OwnerPhonePolicyFacts<'_>,
    ) -> Result<OwnerPhoneDecision, AccessError> {
        let name = match &facts.operation {
            OwnerPhoneOperation::Remember => "remember",
            OwnerPhoneOperation::ProductMethod { method, .. } => method,
            OwnerPhoneOperation::Linkage => "linkage",
            OwnerPhoneOperation::Creation { .. } => "creation",
            OwnerPhoneOperation::SubmissionInput { .. } => "input",
        };
        self.seen.lock().unwrap().push(name.to_owned());
        self.inner.evaluate(facts)
    }
}

struct FixedClock;
impl Clock for FixedClock {
    fn unix_milliseconds(&self) -> u64 {
        110_001
    }
}
impl nessa_protocol::clock::Clock for FixedClock {
    fn elapsed_ms(&self) -> u64 {
        1
    }
}

fn bind_command_store(service: &crate::conversation::application::ConversationService) {
    let directory = tempfile::tempdir().expect("command store");
    let storage = Arc::new(
        nessa_sdk::infrastructure::session_storage::RecordStorage::new(
            directory.path().join("records"),
        )
        .expect("command store"),
    );
    service.bind_commands(storage);
    std::mem::forget(directory);
}

fn response(message: OutgoingMessage) -> ResponseFrame {
    match message {
        OutgoingMessage::Response(frame) => frame,
        OutgoingMessage::Event(frame) => panic!("unexpected event {}", frame.event),
    }
}

async fn call(
    state: &ProductRouteState,
    session: &nessa_auth::application::session::AuthenticatedSession,
    id: &str,
    method: &str,
    params: Value,
) -> ResponseFrame {
    response(
        dispatch_conversation(
            state,
            session,
            RequestFrame::new(id, method, &params).unwrap(),
            Initiation::owner(),
        )
        .await,
    )
}

#[tokio::test]
async fn owner_phone_remember_answer_stores_no_rule() {
    let phone = fixture().await;
    let (service, _, repository, _) =
        conversation_test_support::fixture(ConversationLimits::default());
    bind_command_store(&service);
    let directory = tempfile::tempdir().expect("permission rules");
    let private = directory.path().join("rules");
    nessa_local_storage::create_directory(&private).unwrap();
    let clock = Arc::new(FixedClock);
    let rules = Arc::new(
        PermissionMemory::open(&private.join("permission-rules.sqlite3"), clock.clone()).unwrap(),
    );
    service.bind_permission_rules(rules.clone());
    let policy = Arc::new(CedarPolicyEvaluator::new().unwrap());
    let phone_policy = Arc::new(RecordPolicy {
        inner: CedarOwnerPhonePolicyEvaluator::new().unwrap(),
        seen: Mutex::new(Vec::new()),
    });
    let state = ProductRouteState::new(
        ResourceId::new("gateway").unwrap(),
        OrganizationId::new("org").unwrap(),
        AudienceId::new("gateway").unwrap(),
        ProductDependencies {
            verifier: phone.registry.clone(),
            access: phone.registry.clone(),
            clock: clock.clone(),
            policy,
            owner_phone_purpose: phone.registry.clone(),
            owner_phone_policy: phone_policy.clone(),
            owner_phone_receivers: Some(Arc::new(ActualHolding(phone.receivers.clone()))),
            uptime_clock: clock,
            agent_probe: Arc::new(StubAgentProbe::answering(false, false)),
        },
    )
    .with_conversations(Arc::new(service))
    .with_permission_rules(rules.clone())
    .with_passive_read(
        phone.receivers.clone(),
        repository.clone(),
        Arc::new(EveryConversationGranted),
        conversation_test_support::access_policy(),
    );
    let id = "00000000-0000-4000-8000-0000000000e1";
    let created = call(
        &state,
        &phone.session,
        "create",
        "conversation.create",
        json!({"conversationId": id, "requestId": "create"}),
    )
    .await;
    assert!(created.ok, "{created:?}");
    phone_policy.seen.lock().unwrap().clear();
    let sent = call(
        &state,
        &phone.session,
        "send",
        "conversation.send",
        json!({
            "conversationId": id,
            "requestId": "send-1",
            "executionId": "exec-send",
            "text": "hello",
            "attachments": [],
            "files": []
        }),
    )
    .await;
    let asked = phone_policy.seen.lock().unwrap().clone();
    assert!(
        asked.iter().any(|operation| operation == "remember"),
        "a phone send asks conversation.remember before offering a choice: {asked:?} {sent:?}"
    );
    assert!(
        !rules.offers_remember(id, &ExecutionId::new("exec-send").unwrap()),
        "the refused decision is not armed"
    );
    let answer = |request: &str, option: &str| {
        json!({
            "conversationId": id,
            "requestId": request,
            "executionId": "exec-1",
            "review": "0123456789abcdef0123456789abcdef",
            "permissionId": "perm-1",
            "optionId": option,
        })
    };
    let remembered = call(
        &state,
        &phone.session,
        "remember",
        "conversation.answer",
        answer("remember", "nessa.remember.allow"),
    )
    .await;
    assert!(!remembered.ok, "{remembered:?}");
    assert_eq!(remembered.error.unwrap().code, "forbidden");
    assert!(rules.list(RulePrincipal::Owner).unwrap().is_empty());
    assert!(rules
        .answerer(id, &ExecutionId::new("exec-1").unwrap(), "remember",)
        .is_none());
    let denied = call(
        &state,
        &phone.session,
        "remember-deny",
        "conversation.answer",
        answer("remember-deny", "nessa.remember.deny"),
    )
    .await;
    assert!(!denied.ok, "{denied:?}");
    assert_eq!(denied.error.unwrap().code, "forbidden");
    assert!(rules.list(RulePrincipal::Owner).unwrap().is_empty());
    phone_policy.seen.lock().unwrap().clear();
    let once = call(
        &state,
        &phone.session,
        "once",
        "conversation.answer",
        answer("once", "allow"),
    )
    .await;
    assert_ne!(
        once.error.as_ref().map(|error| error.code.as_str()),
        Some("forbidden"),
        "a one-shot answer stays admitted: {once:?}"
    );
    assert!(
        !phone_policy
            .seen
            .lock()
            .unwrap()
            .iter()
            .any(|operation| operation == "remember"),
        "a one-shot answer does not ask conversation.remember"
    );
}
