//! The two contexts joined by the adapter that actually joins them.
//!
//! Each half is tested on its own with a double standing in for the other,
//! which leaves the seam itself — `PreparedRuntime` — asserted nowhere. This
//! wires a real `AgentWarmUp` to a real `ConversationService` and checks the
//! claim the pair exists to make: one cold launch, joined rather than repeated.
use super::PreparedRuntime;
use crate::agent_warm_up::application::{
    AgentWarmUp, WarmUpAudit, WarmUpAuditRecord, WarmUpFuture, WarmUpRecords,
};
use crate::agent_warm_up::domain::RuntimeFingerprint;
use crate::conversation::application::{
    ConversationCaller, ConversationDependencies, ConversationLimits, ConversationService,
};
use crate::conversation::domain::ConversationId;
use crate::conversation_test_support::{
    fixture, AcceptingCreationAudit, Provider, ProviderFactory, TestClock,
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_sdk::infrastructure::session_storage::InMemoryStorage;
use std::sync::{atomic::Ordering, Arc, Mutex};
use tokio::sync::oneshot;

#[derive(Default)]
struct MemoryRecords {
    completed: Mutex<Vec<RuntimeFingerprint>>,
}
impl WarmUpRecords for MemoryRecords {
    fn completed(&self, runtime: &RuntimeFingerprint) -> WarmUpFuture<'_, bool> {
        let known = self.completed.lock().unwrap().contains(runtime);
        Box::pin(async move { Ok(known) })
    }
    fn record_completed(
        &self,
        runtime: RuntimeFingerprint,
        _observed_at_ms: u64,
    ) -> WarmUpFuture<'_, ()> {
        self.completed.lock().unwrap().push(runtime);
        Box::pin(async { Ok(()) })
    }
}

#[derive(Default)]
struct RecordingAudit {
    records: Mutex<Vec<WarmUpAuditRecord>>,
}
impl WarmUpAudit for RecordingAudit {
    fn record(&self, record: WarmUpAuditRecord) -> WarmUpFuture<'_, ()> {
        self.records.lock().unwrap().push(record);
        Box::pin(async { Ok(()) })
    }
}

fn caller() -> ConversationCaller {
    ConversationCaller {
        organization_id: OrganizationId::new("org").unwrap(),
        principal_id: PrincipalId::new("person").unwrap(),
        surface_id: "panel".into(),
        action_id: "first".into(),
    }
}

fn conversation_id() -> ConversationId {
    ConversationId::new(&uuid::Uuid::new_v4().to_string()).unwrap()
}

/// A first message arriving mid-warm-up joins that launch rather than starting
/// a second one — through the real port, the real adapter and the real service.
#[tokio::test]
async fn a_first_message_joins_the_warm_up_rather_than_launching_beside_it() {
    let (_, provider, repository, storage) = fixture(ConversationLimits::default());
    let records = Arc::new(MemoryRecords::default());
    let audit = Arc::new(RecordingAudit::default());
    let warm_up = AgentWarmUp::new(
        Arc::new(Provider(provider.clone())),
        Arc::new(InMemoryStorage::new()),
        records.clone(),
        audit.clone(),
        Arc::new(TestClock),
        RuntimeFingerprint::new("gateway-test", "test", "sha256:aa").unwrap(),
    );
    let service = ConversationService::new(
        ConversationDependencies {
            provider: Arc::new(Provider(provider.clone())),
            storage,
            metadata: repository,
            creation_audit: Arc::new(AcceptingCreationAudit),
            clock: Arc::new(TestClock),
            readiness: Some(Arc::new(PreparedRuntime(warm_up.clone()))),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();

    // Hold the warm-up inside the provider's open, so the message genuinely
    // arrives while the launch it must join is still in flight.
    let (release, gate) = oneshot::channel();
    *provider.open_gate.lock().unwrap() = Some(gate);
    warm_up.start();
    provider.opening.notified().await;

    let creating = tokio::spawn({
        let service = service.clone();
        async move { service.create(conversation_id(), caller()).await }
    });
    wait_for_waiting_conversation(&provider).await;
    // One launch so far, and it is the warm-up's.
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);

    release.send(()).unwrap();
    creating.await.unwrap().unwrap();
    // The conversation's own provider opens only after the warm-up settled.
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 2);
    assert_eq!(records.completed.lock().unwrap().len(), 1);
    assert_eq!(audit.records.lock().unwrap().len(), 1);
    service.shutdown().await.unwrap();
}

/// The conversation is parked in `PreparedRuntime::wait`, which nothing else
/// observes, so this waits for the absence of a second launch rather than for
/// a signal: a handful of scheduler turns with the warm-up still gated.
async fn wait_for_waiting_conversation(provider: &Arc<ProviderFactory>) {
    for _ in 0..64 {
        tokio::task::yield_now().await;
        assert_eq!(
            provider.open_calls.load(Ordering::SeqCst),
            1,
            "the conversation must not launch beside the warm-up"
        );
    }
}
