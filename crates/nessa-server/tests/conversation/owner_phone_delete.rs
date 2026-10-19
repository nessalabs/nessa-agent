//! Owner-phone command targets ask current policy before reads and receipt replay.
use super::{fixture, ActualHolding};
use crate::agents_test_support::StubAgentProbe;
use crate::conversation::application::{
    AccessAction, AccessAsk, AccessDecision, AccessDenial, ConversationAccessPolicyEvaluator,
    ConversationLimits,
};
use crate::conversation_test_support::{self, EveryConversationGranted};
use crate::product::{dispatch_conversation, ProductDependencies, ProductRouteState};
use nessa_auth::adapters::cedar::CedarPolicyEvaluator;
use nessa_auth::application::ports::Clock;
use nessa_auth::domain::{AudienceId, OrganizationId, ResourceId};
use nessa_protocol::protocol::{OutgoingMessage, RequestFrame, ResponseFrame};
use nessa_sdk::domain::agent_execution::executions::Initiation;
use serde_json::{json, Value};
use std::sync::{atomic::Ordering, Arc};

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

/// Distinguish target Act admission from the neighboring Read question.
struct RefuseAct;
impl ConversationAccessPolicyEvaluator for RefuseAct {
    fn evaluate(&self, ask: &AccessAsk<'_>) -> AccessDecision {
        if ask.action == AccessAction::Act {
            AccessDecision::Deny(AccessDenial::Forbidden {
                policies: vec!["test.refuse-act".into()],
            })
        } else {
            conversation_test_support::access_policy().evaluate(ask)
        }
    }
}

#[tokio::test]
async fn owner_phone_delete_retry_returns_the_idempotent_receipt() {
    let phone = fixture().await;
    let (service, _, repository, _) =
        conversation_test_support::fixture(ConversationLimits::default());
    bind_command_store(&service);
    let clock = Arc::new(FixedClock);
    let policy = Arc::new(CedarPolicyEvaluator::new().unwrap());
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
            owner_phone_policy: Arc::new(
                crate::conversation::infrastructure::CedarOwnerPhonePolicyEvaluator::new().unwrap(),
            ),
            owner_phone_receivers: Some(Arc::new(ActualHolding(phone.receivers.clone()))),
            uptime_clock: clock,
            agent_probe: Arc::new(StubAgentProbe::answering(false, false)),
        },
    )
    .with_conversations(Arc::new(service))
    .with_passive_read(
        phone.receivers.clone(),
        repository.clone(),
        Arc::new(EveryConversationGranted),
        conversation_test_support::access_policy(),
    );
    let id = "00000000-0000-4000-8000-0000000000d1";
    let target = |request: &str| json!({"conversationId": id, "requestId": request});
    let created = call(
        &state,
        &phone.session,
        "create",
        "conversation.create",
        target("create"),
    )
    .await;
    assert!(created.ok, "{created:?}");
    // A Read-allowed target still asks the current command-target policy before
    // peeking at live execution state, just as conversation.read does.
    let restricted = state.clone().with_passive_read(
        phone.receivers.clone(),
        repository,
        Arc::new(EveryConversationGranted),
        Arc::new(RefuseAct),
    );
    let peek = call(
        &restricted,
        &phone.session,
        "peek-refused",
        "conversation.peek",
        json!({"conversationId": id}),
    )
    .await;
    assert!(
        !peek.ok,
        "Act refusal precedes the lifecycle read: {peek:?}"
    );
    assert_eq!(peek.error.unwrap().code, "conversation_not_found");
    let deleted = call(
        &state,
        &phone.session,
        "delete-1",
        "conversation.delete",
        target("delete-1"),
    )
    .await;
    assert!(deleted.ok, "{deleted:?}");
    assert_eq!(
        deleted.payload,
        Some(json!({"requestId": "delete-1", "applied": true}))
    );
    let repeated = call(
        &state,
        &phone.session,
        "delete-1-again",
        "conversation.delete",
        target("delete-1"),
    )
    .await;
    assert!(
        repeated.ok,
        "a committed tombstone must reach the idempotent receipt, not an ownership refusal: {repeated:?}"
    );
    assert_eq!(
        repeated.payload,
        Some(json!({"requestId": "delete-1", "applied": true}))
    );
    let later = call(
        &state,
        &phone.session,
        "delete-2",
        "conversation.delete",
        target("delete-2"),
    )
    .await;
    assert!(later.ok, "{later:?}");
    assert_eq!(
        later.payload,
        Some(json!({"requestId": "delete-2", "applied": false}))
    );
    let closed = call(
        &state,
        &phone.session,
        "close",
        "conversation.close",
        target("close"),
    )
    .await;
    assert!(!closed.ok, "{closed:?}");
    assert_eq!(closed.error.unwrap().code, "conversation_not_found");
}

#[tokio::test]
async fn owner_phone_model_target_refusal_precedes_intent_and_native_control() {
    let phone = fixture().await;
    let (service, provider, repository, _, _) = conversation_test_support::mode_fixture();
    bind_command_store(&service);
    let clock = Arc::new(FixedClock);
    let state = ProductRouteState::new(
        ResourceId::new("gateway").unwrap(),
        OrganizationId::new("org").unwrap(),
        AudienceId::new("gateway").unwrap(),
        ProductDependencies {
            verifier: phone.registry.clone(),
            access: phone.registry.clone(),
            clock: clock.clone(),
            policy: Arc::new(CedarPolicyEvaluator::new().unwrap()),
            owner_phone_purpose: phone.registry.clone(),
            owner_phone_policy: Arc::new(
                crate::conversation::infrastructure::CedarOwnerPhonePolicyEvaluator::new().unwrap(),
            ),
            owner_phone_receivers: Some(Arc::new(ActualHolding(phone.receivers.clone()))),
            uptime_clock: clock,
            agent_probe: Arc::new(StubAgentProbe::answering(false, false)),
        },
    )
    .with_conversations(Arc::new(service.clone()))
    .with_passive_read(
        phone.receivers.clone(),
        repository.clone(),
        Arc::new(EveryConversationGranted),
        conversation_test_support::access_policy(),
    );
    let id = "00000000-0000-4000-8000-0000000000d2";
    let created = call(
        &state,
        &phone.session,
        "create-model-target",
        "conversation.create",
        json!({"conversationId": id, "requestId": "create-model-target"}),
    )
    .await;
    assert!(created.ok, "{created:?}");
    let opened_before = provider.open_calls.load(Ordering::SeqCst);
    let restricted = state.clone().with_passive_read(
        phone.receivers.clone(),
        repository.clone(),
        Arc::new(EveryConversationGranted),
        Arc::new(RefuseAct),
    );
    let params = json!({
        "conversationId": id,
        "requestId": "model-target-choice",
        "model": "second-model"
    });
    // Isolate the typed handler's target admission. The socket's current
    // method-purpose policy independently refuses model selection for phones.
    let denied = call(
        &restricted,
        &phone.session,
        "model-target-refused",
        "conversation.setModel",
        params.clone(),
    )
    .await;
    assert!(
        !denied.ok,
        "target Act refusal must precede model effects: {denied:?}"
    );
    assert_eq!(denied.error.unwrap().code, "conversation_not_found");
    assert!(repository.model_requests.lock().unwrap().is_empty());
    assert!(provider.model_updates.lock().unwrap().is_empty());
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), opened_before);
    let accepted = call(
        &state,
        &phone.session,
        "model-target-allowed",
        "conversation.setModel",
        params,
    )
    .await;
    assert!(
        accepted.ok,
        "an admitted target still reaches the model owner: {accepted:?}"
    );
    assert_eq!(accepted.payload.unwrap()["model"], "second-model");
    assert_eq!(repository.model_requests.lock().unwrap().len(), 1);
    assert_eq!(provider.model_updates.lock().unwrap().len(), 1);
    service.shutdown().await.unwrap();
}
