//! A repeated owner-phone delete reaches the service receipt after the tombstone.
use super::{fixture, ActualHolding};
use crate::agents_test_support::StubAgentProbe;
use crate::conversation::application::ConversationLimits;
use crate::conversation_test_support::{self, EveryConversationGranted};
use crate::product::{dispatch_conversation, ProductDependencies, ProductRouteState};
use nessa_auth::adapters::cedar::CedarPolicyEvaluator;
use nessa_auth::application::ports::Clock;
use nessa_auth::domain::{AudienceId, OrganizationId, ResourceId};
use nessa_protocol::protocol::{OutgoingMessage, RequestFrame, ResponseFrame};
use nessa_sdk::domain::agent_execution::executions::Initiation;
use serde_json::{json, Value};
use std::sync::Arc;

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
