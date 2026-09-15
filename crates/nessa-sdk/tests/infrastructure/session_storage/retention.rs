//! Restored observations must admit a feasible history under live retention limits.
use super::custom_storage::{assert_custom_retention_admission, assert_moved_retention_admission};
use super::*;

const BUDGET: usize = 32 * 1024 * 1024;

fn tools(count: usize, title_bytes: usize) -> SessionSnapshot {
    let mut value = snapshot("retention");
    let execution = value.invocations[0].request.execution_id.clone();
    value.invocations[0].events = (0..count)
        .map(|index| {
            ExecutionEvent::new(
                execution.clone(),
                ExecutionUpdate::Tool(ToolCallUpdate::new(
                    ToolCallId::new(format!("t{index}")).unwrap(),
                    Some("x".repeat(title_bytes)),
                    None,
                    None,
                    None,
                    None,
                )),
            )
        })
        .collect();
    value
}

fn reviews(count: usize, bytes: usize, overlap: bool, cancellations: bool) -> SessionSnapshot {
    let mut value = snapshot("retention");
    let execution = value.invocations[0].request.execution_id.clone();
    value.invocations[0].events.clear();
    let tool_id = ToolCallId::new("tool").unwrap();
    let observation = ToolObservation::default().with_update(ToolCallUpdate::new(
        tool_id.clone(),
        None,
        None,
        None,
        None,
        None,
    ));
    let mut trailing = Vec::new();
    for index in 0..count {
        let id = PermissionId::new(format!("r{index}")).unwrap();
        let input = ToolReviewInput {
            name: "Read".into(),
            arguments_json: "x".repeat(bytes),
        };
        value.invocations[0].events.push(ExecutionEvent::new(
            execution.clone(),
            ExecutionUpdate::PermissionRequested {
                id: id.clone(),
                tool_id: tool_id.clone(),
                observation: observation.clone(),
                input: input.clone(),
                options: choices(),
            },
        ));
        if cancellations {
            let record = PermissionCancellation::from_record(
                value.provider_session_id.clone(),
                cancelled_request(
                    PermissionRequest::new(id, execution.clone(), tool_id.clone(), choices()),
                    PermissionCancellationReason::deadline_exceeded(),
                ),
                input,
                CancellationOrigin::Runtime,
            )
            .unwrap();
            let event = ExecutionEvent::new(
                execution.clone(),
                ExecutionUpdate::PermissionCancelled(record),
            );
            if overlap {
                trailing.push(event);
            } else {
                value.invocations[0].events.push(event);
            }
        }
    }
    value.invocations[0].events.extend(trailing);
    value
}

#[tokio::test]
async fn restored_tool_count_and_seen_review_limits_match_live_admission() {
    for (count, accepted) in [(4096, true), (4097, false)] {
        assert_custom_retention_admission(tools(count, 0), accepted).await;
        assert_custom_retention_admission(reviews(count, 0, false, false), accepted).await;
    }
}

#[tokio::test]
async fn missing_answers_do_not_invent_pending_reviews_but_cancellations_prove_overlap() {
    assert_custom_retention_admission(reviews(129, 0, false, false), true).await;
    assert_custom_retention_admission(reviews(129, 0, false, true), true).await;
    assert_custom_retention_admission(reviews(128, 0, true, true), true).await;
    assert_custom_retention_admission(reviews(129, 0, true, true), false).await;
}

#[tokio::test]
async fn review_byte_limits_charge_proven_overlap_and_release_resolved_payload() {
    let overhead = "execution".len()
        + 3 * "r0".len()
        + "tool".len()
        + "Read".len()
        + choices().payload_bytes();
    assert_custom_retention_admission(reviews(1, BUDGET - overhead, false, false), true).await;
    assert_custom_retention_admission(reviews(1, BUDGET - overhead + 1, false, false), false).await;
    assert_custom_retention_admission(reviews(1, BUDGET, false, false), false).await;
    assert_custom_retention_admission(reviews(2, BUDGET / 2, true, true), false).await;
    assert_custom_retention_admission(reviews(2, BUDGET / 2, false, true), true).await;
    assert_custom_retention_admission(reviews(2, BUDGET / 2, false, false), true).await;
}

#[tokio::test]
async fn sparse_tool_updates_accumulate_and_replacements_release_payload() {
    let base = tools(1, BUDGET - "execution".len() - 2 * "t0".len());
    assert_custom_retention_admission(base.clone(), true).await;
    let mut overflow = base.clone();
    let execution = overflow.invocations[0].request.execution_id.clone();
    overflow.invocations[0].events.push(ExecutionEvent::new(
        execution.clone(),
        ExecutionUpdate::Tool(ToolCallUpdate::new(
            ToolCallId::new("t1").unwrap(),
            None,
            None,
            None,
            None,
            None,
        )),
    ));
    assert_custom_retention_admission(overflow.clone(), false).await;
    let clear = ExecutionEvent::new(
        execution.clone(),
        ExecutionUpdate::Tool(ToolCallUpdate::new(
            ToolCallId::new("t0").unwrap(),
            Some(String::new()),
            None,
            None,
            None,
            None,
        )),
    );
    overflow.invocations[0].events.insert(1, clear);
    assert_custom_retention_admission(overflow, true).await;
    let mut sparse = base;
    sparse.invocations[0].events.push(ExecutionEvent::new(
        execution,
        ExecutionUpdate::Tool(ToolCallUpdate::new(
            ToolCallId::new("t0").unwrap(),
            None,
            None,
            Some(ToolStatus::Completed),
            None,
            None,
        )),
    ));
    assert_custom_retention_admission(sparse, true).await;
}

#[tokio::test]
async fn file_restore_rejects_excess_tool_count_without_rewriting() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("private");
    private::create_directory(&directory).unwrap();
    let storage = LocalFileStorage::new(directory.clone()).unwrap();
    let value = tools(4096, 0);
    let lease = storage.open(value.id.clone()).await.unwrap();
    lease.save(value).await.unwrap();
    let path = journal_path(&directory, "retention");
    let mut json: serde_json::Value = snapshot_json(&std::fs::read(&path).unwrap()).unwrap();
    let events = json
        .pointer_mut("/invocations/0/events")
        .unwrap()
        .as_array_mut()
        .unwrap();
    let mut extra = events[0].clone();
    extra["update"]["Tool"]["id"] = "overflow".into();
    events.push(extra);
    let bytes = journal_bytes(&json).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    assert!(matches!(lease.load().await, Err(StorageError::Corrupt(_))));
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

#[tokio::test]
async fn moved_custom_snapshot_cannot_hide_review_allocation_capacity() {
    for field in 0..2 {
        for oversized in [false, true] {
            let mut value = reviews(1, 0, false, false);
            let execution = value.invocations[0].request.execution_id.clone();
            let ExecutionUpdate::PermissionRequested {
                id,
                tool_id,
                observation,
                options,
                ..
            } = value.invocations[0].events[0].update()
            else {
                unreachable!()
            };
            let mut reserved = String::with_capacity(if oversized { BUDGET + 1 } else { 1024 });
            reserved.push('x');
            assert_eq!(reserved.len(), 1);
            assert!(reserved.capacity() >= if oversized { BUDGET + 1 } else { 1024 });
            let input = if field == 0 {
                ToolReviewInput {
                    name: reserved,
                    arguments_json: "{}".into(),
                }
            } else {
                ToolReviewInput {
                    name: "Read".into(),
                    arguments_json: reserved,
                }
            };
            value.invocations[0].events[0] = ExecutionEvent::new(
                execution,
                ExecutionUpdate::PermissionRequested {
                    id: id.clone(),
                    tool_id: tool_id.clone(),
                    observation: observation.clone(),
                    input,
                    options: options.clone(),
                },
            );
            // No Clone occurs between this allocation and the application's load.
            assert_moved_retention_admission(value, !oversized).await;
        }
    }
}
