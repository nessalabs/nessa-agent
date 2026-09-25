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
use crate::agents::domain::AgentId;
use crate::conversation::application::{
    ConversationAgent, ConversationAgents, ConversationCaller, ConversationDependencies,
    ConversationLifecyclePhase, ConversationLimits, ConversationService, ProviderSessionErasers,
};
use crate::conversation::domain::ConversationId;
use crate::conversation_test_support::{
    fixture, AcceptingCreationAudit, AcceptingDeletionAudit, MemorySummaries, Provider,
    ProviderFactory, RecordingFileLinkAudit, TestClock, DELETION_BUDGETS,
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_sdk::infrastructure::session_storage::InMemoryStorage;
use std::collections::HashMap;
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
    fail: std::sync::atomic::AtomicBool,
}
impl WarmUpAudit for RecordingAudit {
    fn record(&self, record: WarmUpAuditRecord) -> WarmUpFuture<'_, ()> {
        self.records.lock().unwrap().push(record);
        let fail = self.fail.load(Ordering::SeqCst);
        Box::pin(async move {
            if fail {
                Err(crate::agent_warm_up::application::WarmUpError::Audit(
                    "warm-up audit unavailable".into(),
                ))
            } else {
                Ok(())
            }
        })
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
async fn a_failed_warm_up_releases_readiness_without_becoming_conversation_failure() {
    let (_, provider, repository, storage) = fixture(ConversationLimits::default());
    let records = Arc::new(MemoryRecords::default());
    let audit = Arc::new(RecordingAudit::default());
    let warm_up = AgentWarmUp::new(
        Arc::new(Provider::new(provider.clone())),
        Arc::new(crate::conversation_test_support::AcceptingAudit),
        Arc::new(InMemoryStorage::new()),
        records.clone(),
        audit.clone(),
        Arc::new(TestClock),
        RuntimeFingerprint::new("gateway-test", "test", "sha256:aa").unwrap(),
    );
    let service = ConversationService::new(
        ConversationDependencies {
            agents: ConversationAgents::new(
                HashMap::from([(
                    AgentId::Claude,
                    ConversationAgent {
                        provider: Arc::new(Provider::new(provider.clone())),
                        execution_audit: Arc::new(crate::conversation_test_support::AcceptingAudit),
                        reserved_output_tokens: 4096,
                        readiness: Some(Arc::new(PreparedRuntime(warm_up.clone()))),
                    },
                )]),
                AgentId::Claude,
            )
            .expect("one configured agent is its own default"),
            storage,
            metadata: repository,
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            summaries: Arc::new(MemorySummaries::default()),
            deletion_audit: Arc::new(AcceptingDeletionAudit),
            provider_sessions: ProviderSessionErasers::default(),
            deletion_budgets: DELETION_BUDGETS,
            clock: Arc::new(TestClock),
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
    let conversation = conversation_id();
    service
        .create(conversation.clone(), caller(), None)
        .await
        .expect("preparation returns before runtime readiness");
    wait_for_waiting_conversation(&provider).await;
    // One launch so far, and it is the warm-up's.
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);

    audit.fail.store(true, Ordering::SeqCst);
    release.send(()).unwrap();
    for _ in 0..64 {
        if provider.open_calls.load(Ordering::SeqCst) == 2 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 2);
    let mut attached = None;
    for _ in 0..64 {
        let view = service.read(conversation.clone(), caller()).await.unwrap();
        if view.lifecycle.phase == ConversationLifecyclePhase::Attached {
            attached = Some(view);
            break;
        }
        tokio::task::yield_now().await;
    }
    let view = attached.expect("conversation attachment completes after readiness settles");
    assert_eq!(view.lifecycle.phase, ConversationLifecyclePhase::Attached);
    assert!(view.lifecycle.failure.is_none());
    assert!(records.completed.lock().unwrap().is_empty());
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
