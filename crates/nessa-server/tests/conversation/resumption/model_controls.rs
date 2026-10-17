//! A startup resume waits for retained model evidence, as it does preset evidence.
use super::*;
use crate::conversation::application::{
    ConversationModelRequest, ConversationModelRequestState, ConversationRepository,
};
use crate::conversation::infrastructure::LocalConversationStore;
use crate::conversation_test_support::mode_fixture;
use nessa_protocol::conversation::domain::ConversationModelId;

fn start_model_run(
    metadata: Arc<LocalConversationStore>,
    storage: Arc<InMemoryStorage>,
    marks: Arc<MemoryMarks>,
) -> (ConversationService, Arc<ProviderFactory>) {
    let (mut service, provider, _, _, _) = mode_fixture();
    let inner = Arc::get_mut(&mut service.inner).unwrap();
    inner.metadata = metadata;
    inner.storage = storage;
    service.bind_unfinished_turns(
        marks,
        FailureBackoff {
            first: Duration::from_secs(3600),
            most: Duration::from_secs(3600),
            attempts: 8,
        },
    );
    (service, provider)
}

async fn startup_model_evidence(reconcile_before_start: bool) {
    let directory = tempfile::tempdir().unwrap();
    let private = directory.path().join("conversations");
    nessa_local_storage::create_directory(&private).unwrap();
    let metadata = Arc::new(
        LocalConversationStore::open(&private.join("metadata.sqlite3"))
            .unwrap()
            .with_access_policy(access_policy()),
    );
    let storage = Arc::new(InMemoryStorage::new());
    let marks = Arc::new(MemoryMarks::default());
    let (first, provider) = start_model_run(metadata.clone(), storage.clone(), marks.clone());
    let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    first
        .create(
            id.clone(),
            caller("create"),
            RequestedConversation::default(),
        )
        .await
        .unwrap();
    let (release, gate) = oneshot::channel();
    *provider.execution_gate.lock().unwrap() = Some(gate);
    *provider.execution_updates.lock().unwrap() =
        vec![ExecutionUpdate::Finished(ExecutionOutcome::Cancelled)];
    *provider.execution_reply.lock().unwrap() = Some(ProviderExecutionReply::Finished(
        ExecutionReport::cancelled_locally(CleanupReport::confirmed(CloseOutcome {
            forced: false,
        })),
    ));
    first
        .submit(
            id.clone(),
            caller("work"),
            "owed-model".into(),
            SubmittedMessage {
                text: "Finish the admitted A work".into(),
                ..SubmittedMessage::default()
            },
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    tokio::time::timeout(BOUND, provider.execution_started.notified())
        .await
        .unwrap();
    let live = first
        .live_now(&id, &caller("inspect"))
        .await
        .unwrap()
        .unwrap();
    let record = ConversationRepository::load(metadata.as_ref(), &id)
        .await
        .unwrap()
        .unwrap();
    let configured = first
        .inner
        .agents
        .resolve_model(record.agent().unwrap(), "second-model")
        .await
        .unwrap();
    let target = live.lease.model_provider(configured.provider).unwrap();
    let actor = caller("retained-model-intent");
    let intent = ConversationModelRequest {
        conversation_id: id.clone(),
        organization_id: actor.organization_id.clone(),
        request_id: actor.action_id.clone(),
        initiator_principal_id: actor.principal_id.clone(),
        initiator_surface_id: actor.surface_id.clone(),
        prior_identity: live.agent.provider_identity(),
        requested_identity: target.identity(),
        prior_model: record.model().clone(),
        prior_approval_mode: record.approval_mode(),
        requested_model: ConversationModelId::new("second-model").unwrap(),
        state: ConversationModelRequestState::Pending,
        application: None,
        requested_at_ms: first.inner.clock.unix_milliseconds(),
    };
    let stopping = {
        let first = first.clone();
        tokio::spawn(async move { first.shutdown().await })
    };
    detaching(&first, &id).await;
    release.send(()).unwrap();
    tokio::time::timeout(BOUND, stopping)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    drop(live);
    drop(first);
    // The process ended after this valid durable intent, before native effect.
    // The authoritative SDK history still names A and one suspended owed turn.
    assert_eq!(
        metadata.begin_model_change(intent.clone()).await.unwrap(),
        intent
    );
    let session = conversation_session(&id);
    let before = storage
        .read_committed(session.clone())
        .await
        .unwrap()
        .unwrap();
    let original = before.snapshot().unwrap().clone();
    assert_eq!(original.provider, intent.prior_identity);
    assert_eq!(original.invocations.len(), 1);
    assert!(marks.holds(&id));

    let (second, resumed_provider) =
        start_model_run(metadata.clone(), storage.clone(), marks.clone());
    if reconcile_before_start {
        // Normal Open owns reconciliation; resumption must not acquire another
        // recovery lock or prevent its verified prior-context opening.
        second.read(id.clone(), caller("reconcile")).await.unwrap();
        assert!(metadata.pending_model_change(&id).await.unwrap().is_none());
    }
    assert_eq!(second.resume_unfinished().await.unwrap(), 1);
    let live = second
        .live_now(&id, &caller("inspect-result"))
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(BOUND, async {
        while !live.agent.is_idle().await {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let after = live.agent.session_manager().snapshot().await.unwrap();
    let native = resumed_provider.executions.lock().unwrap().clone();
    let retained = metadata.pending_model_change(&id).await.unwrap();
    if reconcile_before_start {
        until(|| !marks.holds(&id)).await;
    }
    let mark_held = marks.holds(&id);
    second.shutdown().await.unwrap();
    assert!(resumed_provider.model_updates.lock().unwrap().is_empty());
    assert_eq!(after.provider, intent.prior_identity);
    if reconcile_before_start {
        assert_eq!(after.invocations.len(), 2);
        assert_eq!(native.len(), 1);
        assert!(retained.is_none());
        assert!(!mark_held);
    } else {
        assert_eq!(after.invocations.len(), original.invocations.len(),
            "pending model evidence admitted a successor: native={native:?}; retained={retained:?}; mark={mark_held}");
        assert!(native.is_empty());
        assert_eq!(after.invocations, original.invocations);
        assert_eq!(retained, Some(intent));
        assert!(mark_held);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn startup_resumption_waits_for_pending_model_evidence() {
    startup_model_evidence(false).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn startup_resumption_runs_after_model_evidence_settles() {
    startup_model_evidence(true).await;
}
