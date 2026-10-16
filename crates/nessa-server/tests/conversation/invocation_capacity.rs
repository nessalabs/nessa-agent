//! Retained invocation capacity is separate from command receipt capacity.
use super::*;
use nessa_sdk::application::agent_execution::{
    commands::{MutationReceipt, MutationStage},
    executions::{ExecutionEvent, ExecutionRequest, ExecutionUpdate, SubmissionMode as SavedMode},
    permissions::ActionContext,
    sessions::{InvocationRecord, SessionChange, SessionSaveUnit, SubmissionAcknowledgement},
};
use nessa_sdk::domain::agent_execution::{
    executions::ExecutionOutcome,
    prompts::{PromptText, UserMessage},
    sessions::SessionId,
};

// Current behavior, not a proposed continuation or retry policy:
// | Order | Expected evidence | Test |
// | Original command settles -> validated history reaches 1024 -> cold reopen -> original retry | Original receipt, no execution | full_history_reopens_and_retries_the_original_command |
// | Full history -> fresh command -> same retry -> cold reopen -> same retry | InvalidInput first; durable Attempted then Interrupted; no new invocation/execution | full_history_refusal_retains_attempted_receipt_across_retry_and_reopen |

/// Keep one real completed command, then append the SDK's ordinary semantic
/// input/observation/settlement facts in one validated save. The extra records
/// are historical fixture facts, not 1023 calls to the provider.
async fn seed_full_history(root: &Path, target: &ConversationId) -> MutationReceipt {
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(root, provider.clone());
    opened(&service, &provider, target).await;
    let original = service
        .submit_command(
            storage.clone(),
            target.clone(),
            caller("original"),
            Initiation::owner(),
            true,
            "original-turn".into(),
            message("original input"),
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    assert_eq!(
        original.progress.outcome(),
        Some(MutationOutcome::Dispatched)
    );
    let turn = ExecutionId::new("original-turn").unwrap();
    tokio::time::timeout(TURN_HOLD_HANG, async {
        loop {
            let live = service.resolve(target, &caller("read")).await.unwrap();
            if turn_is_final(&live, &turn).await == Some(true) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        provider.executions.lock().unwrap().as_slice(),
        ["original-turn"]
    );
    retire(service, storage, metadata).await;

    let storage = RecordStorage::new(root.join("records")).unwrap();
    let session = SessionId::new(target.to_string()).unwrap();
    let writer = storage
        .open_existing(session.clone())
        .await
        .unwrap()
        .unwrap();
    let (prior, binding) = writer
        .load()
        .await
        .unwrap()
        .into_published(&session)
        .unwrap();
    let prior = prior.unwrap();
    assert_eq!(prior.invocations.len(), 1);
    let mut candidate = prior.clone();
    let mut changes = Vec::new();
    for index in 1..SessionSnapshot::MAX_INVOCATIONS {
        let execution = ExecutionId::new(format!("historical-{index}")).unwrap();
        let input = InvocationRecord {
            target_event_offset: None,
            submission: SavedMode::Immediate,
            request: ExecutionRequest {
                execution_id: execution.clone(),
                user_message: UserMessage::text_only(PromptText::new("historical input").unwrap()),
                estimated_input_tokens: 1,
                reserved_output_tokens: 1,
            },
            actor: ActionContext::new("person", "panel", "historical").unwrap(),
            initiation: Initiation::owner(),
            notes: Vec::new(),
            acknowledgement: SubmissionAcknowledgement::Pending,
            events: Vec::new(),
            scheduling: Vec::new(),
            provider_report: None,
            local_cancellation: None,
            local_outcome: None,
            cancellation: None,
            result: None,
        };
        changes.push(SessionChange::InputAccepted(Box::new(input.clone())));
        let event = ExecutionEvent::new(
            execution.clone(),
            ExecutionUpdate::Finished(ExecutionOutcome::Completed),
        );
        changes.push(SessionChange::ProviderObservation(event.clone()));
        changes.push(SessionChange::LocalSettlement {
            execution_id: execution,
            before: None,
            after: Ok(ExecutionOutcome::Completed),
            local_outcome: Some(ExecutionOutcome::Completed),
        });
        candidate.invocations.push(InvocationRecord {
            events: vec![event],
            result: Some(Ok(ExecutionOutcome::Completed)),
            local_outcome: Some(ExecutionOutcome::Completed),
            ..input
        });
    }
    let units = vec![SessionSaveUnit::new(changes).unwrap()];
    SessionSaveUnit::validate_plan(Some(&prior), &units, &candidate).unwrap();
    assert!(candidate.pending_order().unwrap().is_empty());
    writer
        .save_changes(binding, candidate.clone(), units)
        .await
        .unwrap();
    assert_eq!(writer.load().await.unwrap().snapshot(), Some(&candidate));
    drop(writer);
    storage.shutdown().await.unwrap();
    original.progress
}

async fn full_committed_history(
    storage: &RecordStorage,
    target: &ConversationId,
) -> Vec<InvocationRecord> {
    let session = SessionId::new(target.to_string()).unwrap();
    let committed = storage.read_committed(session).await.unwrap().unwrap();
    let snapshot = committed.snapshot().unwrap();
    assert_eq!(snapshot.invocations.len(), SessionSnapshot::MAX_INVOCATIONS);
    assert!(snapshot
        .invocations
        .iter()
        .all(|record| record.result.is_some()));
    assert!(snapshot.pending_order().unwrap().is_empty());
    assert!(!snapshot
        .invocations
        .iter()
        .any(|record| record.request.execution_id.as_str() == "overflow-turn"));
    snapshot.invocations.clone()
}

#[tokio::test]
async fn full_history_reopens_and_retries_the_original_command() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let target = id();
    let original = seed_full_history(&root, &target).await;
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let prior = full_committed_history(&storage, &target).await;
    let rows = command_rows(&root);
    let retry = service
        .submit_command(
            storage.clone(),
            target.clone(),
            caller("original"),
            Initiation::owner(),
            true,
            "original-turn".into(),
            message("original input"),
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    assert_eq!(retry.progress, original);
    assert!(retry.delivery.is_none());
    assert!(provider.executions.lock().unwrap().is_empty());
    assert_eq!(command_rows(&root), rows);
    assert_eq!(full_committed_history(&storage, &target).await, prior);
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn full_history_refusal_retains_attempted_receipt_across_retry_and_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let target = id();
    let _original = seed_full_history(&root, &target).await;
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let prior = full_committed_history(&storage, &target).await;
    let rows = command_rows(&root);
    let submit = || {
        service.submit_command(
            storage.clone(),
            target.clone(),
            caller("overflow"),
            Initiation::owner(),
            true,
            "overflow-turn".into(),
            message("one more input"),
            SubmissionMode::Queue,
        )
    };
    assert!(matches!(
        submit().await,
        Err(MutationFailure::Target(ConversationError::Agent(
            AgentError::InvalidInput(_)
        )))
    ));
    assert_eq!(command_rows(&root), rows + 2);
    assert_eq!(full_committed_history(&storage, &target).await, prior);
    // Resolving the first command may restore its attachment. Count that
    // separately from executions, which must remain empty below.
    let opens = provider.open_calls.load(Ordering::SeqCst);
    let Err(MutationFailure::Interrupted(attempted)) = submit().await else {
        panic!("same-ID capacity refusal retains its attempted receipt");
    };
    assert_eq!(attempted.stage(), MutationStage::Attempted);
    assert_eq!(attempted.outcome(), None);
    assert_eq!(command_rows(&root), rows + 2);
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), opens);
    assert!(provider.executions.lock().unwrap().is_empty());
    assert_eq!(full_committed_history(&storage, &target).await, prior);
    retire(service, storage, metadata).await;

    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(&root, provider.clone());
    assert_eq!(full_committed_history(&storage, &target).await, prior);
    let Err(MutationFailure::Interrupted(restored)) = service
        .submit_command(
            storage.clone(),
            target.clone(),
            caller("overflow"),
            Initiation::owner(),
            true,
            "overflow-turn".into(),
            message("one more input"),
            SubmissionMode::Queue,
        )
        .await
    else {
        panic!("cold retry must not dispatch the attempted command");
    };
    assert_eq!(restored, attempted);
    assert_eq!(command_rows(&root), rows + 2);
    assert!(provider.executions.lock().unwrap().is_empty());
    assert_eq!(full_committed_history(&storage, &target).await, prior);
    retire(service, storage, metadata).await;
}
