//! Declined-review identity and delivery stages survive storage without relabeling.
use super::*;
use nessa_sdk::domain::agent_execution::permissions::{
    ReviewDeclineObservation, ReviewDeclineStage,
};

fn declined(execution: &ExecutionId, id: &str, delivery: ReviewDeclineStage) -> ExecutionEvent {
    declined_reason(
        execution,
        id,
        ReviewDeclineReason::ToolNotReviewable,
        delivery,
    )
}

fn declined_reason(
    execution: &ExecutionId,
    id: &str,
    reason: ReviewDeclineReason,
    delivery: ReviewDeclineStage,
) -> ExecutionEvent {
    let selected = ReviewDeclineObservation::selected(
        ReviewDeclineId::new(id).unwrap(),
        ReviewDecline::new(Some("Read"), reason),
    );
    let observation = if delivery == ReviewDeclineStage::Selected {
        selected
    } else {
        selected.advance(delivery).unwrap()
    };
    ExecutionEvent::new(
        execution.clone(),
        ExecutionUpdate::ReviewDeclined(observation),
    )
}

#[tokio::test]
async fn selected_and_final_declines_round_trip_with_distinct_identical_identities() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("private");
    private::create_directory(&directory).unwrap();
    let storage = LocalFileStorage::new(directory).unwrap();
    let mut value = snapshot("declined-review");
    let execution = value.invocations[0].request.execution_id.clone();
    value.invocations[0].events.extend([
        declined(&execution, "1", ReviewDeclineStage::Selected),
        declined(&execution, "1", ReviewDeclineStage::WriteConfirmed),
        declined(&execution, "2", ReviewDeclineStage::Selected),
        declined(&execution, "2", ReviewDeclineStage::WriteUnconfirmed),
    ]);
    let lease = storage.open(value.id.clone()).await.unwrap();
    lease.save(value.clone()).await.unwrap();
    assert_same(&lease.load().await.unwrap().unwrap(), &value);
}

#[tokio::test]
async fn restoration_rejects_final_without_selection_and_conflicting_reuse() {
    let mut final_only = snapshot("final-only");
    let execution = final_only.invocations[0].request.execution_id.clone();
    final_only.invocations[0].events.push(declined(
        &execution,
        "1",
        ReviewDeclineStage::WriteConfirmed,
    ));
    super::custom_storage::assert_custom_retention_admission(final_only, false).await;

    let mut repeated = snapshot("repeated-selection");
    let execution = repeated.invocations[0].request.execution_id.clone();
    repeated.invocations[0].events.extend([
        declined(&execution, "1", ReviewDeclineStage::Selected),
        declined(&execution, "1", ReviewDeclineStage::Selected),
    ]);
    super::custom_storage::assert_custom_retention_admission(repeated, false).await;

    let mut duplicate_final = snapshot("duplicate-final");
    let execution = duplicate_final.invocations[0].request.execution_id.clone();
    duplicate_final.invocations[0].events.extend([
        declined(&execution, "1", ReviewDeclineStage::Selected),
        declined(&execution, "1", ReviewDeclineStage::WriteConfirmed),
        declined(&execution, "1", ReviewDeclineStage::WriteConfirmed),
    ]);
    super::custom_storage::assert_custom_retention_admission(duplicate_final, false).await;

    let mut changed_reason = snapshot("changed-decline-reason");
    let execution = changed_reason.invocations[0].request.execution_id.clone();
    changed_reason.invocations[0].events.extend([
        declined(&execution, "1", ReviewDeclineStage::Selected),
        declined_reason(
            &execution,
            "1",
            ReviewDeclineReason::UnusableOptions,
            ReviewDeclineStage::WriteConfirmed,
        ),
    ]);
    super::custom_storage::assert_custom_retention_admission(changed_reason, false).await;
}

#[tokio::test]
async fn file_restore_rejects_changed_or_oversized_decline_fields_before_rewriting() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("private");
    private::create_directory(&directory).unwrap();
    let storage = LocalFileStorage::new(directory.clone()).unwrap();
    let mut value = snapshot("decline-corruption");
    let execution = value.invocations[0].request.execution_id.clone();
    value.invocations[0]
        .events
        .push(declined(&execution, "1", ReviewDeclineStage::Selected));
    let lease = storage.open(value.id.clone()).await.unwrap();
    lease.save(value).await.unwrap();
    let path = journal_path(&directory, "decline-corruption");
    let original = std::fs::read(&path).unwrap();
    for (pointer, replacement) in [
        (
            "/invocations/0/events/1/update/ReviewDeclined/tool",
            serde_json::Value::String(String::new()),
        ),
        (
            "/invocations/0/events/1/update/ReviewDeclined/tool",
            serde_json::Value::String("two\nlines".into()),
        ),
        (
            "/invocations/0/events/1/update/ReviewDeclined/tool",
            serde_json::Value::String("M".repeat(129)),
        ),
        (
            "/invocations/0/events/1/update/ReviewDeclined/id",
            serde_json::Value::String("9".repeat(21)),
        ),
    ] {
        let mut invalid = snapshot_json(&original).unwrap();
        *invalid.pointer_mut(pointer).unwrap() = replacement;
        let bytes = journal_bytes(&invalid).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        assert!(matches!(lease.load().await, Err(StorageError::Corrupt(_))));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
    std::fs::write(&path, original).unwrap();
}
