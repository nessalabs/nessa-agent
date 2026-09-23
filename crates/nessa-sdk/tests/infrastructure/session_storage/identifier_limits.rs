//! Restored tool/review identities obey the same byte admission as live observations.
use super::custom_storage::assert_custom_retention_admission;
use super::*;
use nessa_sdk::application::agent_execution::executions::ExecutionController;

fn bounded_snapshot(tool_text: &str, review_text: &str) -> SessionSnapshot {
    let mut value = snapshot("identifier-limits");
    let execution = value.invocations[0].request.execution_id.clone();
    let review = PermissionId::new(review_text).unwrap();
    let tool_id = ToolCallId::new(tool_text).unwrap();
    let tool = ToolCallUpdate::new(tool_id.clone(), None, None, None, None, None);
    let observation = ToolObservation::default().with_update(tool.clone());
    let input = ToolReviewInput {
        name: "Read".into(),
        arguments_json: "{}".into(),
    };
    let cancellation = PermissionCancellation::from_record(
        value.provider_context.recorded().unwrap().clone(),
        cancelled_request(
            PermissionRequest::new(
                review.clone(),
                execution.clone(),
                tool_id.clone(),
                choices(),
            ),
            PermissionCancellationReason::deadline_exceeded(),
        ),
        input.clone(),
        CancellationOrigin::Runtime,
    )
    .unwrap();
    value.invocations[0].events = vec![
        ExecutionEvent::new(execution.clone(), ExecutionUpdate::Tool(tool)),
        ExecutionEvent::new(
            execution.clone(),
            ExecutionUpdate::PermissionRequested {
                id: review,
                tool_id,
                observation,
                input,
                options: choices(),
            },
        ),
        ExecutionEvent::new(
            execution,
            ExecutionUpdate::PermissionCancelled(cancellation),
        ),
    ];
    value
}

fn assert_identifier_limit(error: StorageError) {
    assert_eq!(
        error,
        StorageError::Corrupt(
            AgentError::InvalidInput(
                "observation identity must contain 1 through 256 bytes".into(),
            )
            .to_string(),
        )
    );
}

#[test]
fn live_observation_ids_accept_256_utf8_bytes_and_reject_257_without_mutation() {
    let boundary = "é".repeat(128);
    let oversized = format!("{boundary}x");
    let mut controller = ExecutionController::new(ExecutionSessionId::new("provider").unwrap());
    controller
        .begin_execution(ExecutionId::new("execution").unwrap())
        .unwrap();
    let tool =
        |id: &str| ToolCallUpdate::new(ToolCallId::new(id).unwrap(), None, None, None, None, None);
    assert!(matches!(
        controller.tool_event(&ExecutionId::new("execution").unwrap(), tool(&oversized)),
        Err(AgentError::InvalidInput(_))
    ));
    controller
        .tool_event(&ExecutionId::new("execution").unwrap(), tool(&boundary))
        .unwrap();
    let input = || ToolReviewInput {
        name: "Read".into(),
        arguments_json: "{}".into(),
    };
    assert!(matches!(
        controller.request_permission(
            &ExecutionId::new("execution").unwrap(),
            PermissionId::new(oversized).unwrap(),
            tool(&boundary),
            input(),
            choices(),
        ),
        Err(AgentError::InvalidInput(_))
    ));
    controller
        .request_permission(
            &ExecutionId::new("execution").unwrap(),
            PermissionId::new(boundary.clone()).unwrap(),
            tool(&boundary),
            input(),
            choices(),
        )
        .unwrap();
    let cancellations = controller
        .cancel_permissions(
            &ExecutionId::new("execution").unwrap(),
            PermissionCancellationReason::deadline_exceeded(),
            CancellationOrigin::Runtime,
        )
        .unwrap();
    assert_eq!(cancellations.len(), 1);
    assert_eq!(cancellations[0].request().id().as_str(), boundary);
    assert_eq!(cancellations[0].request().tool_id().as_str(), boundary);
}

#[tokio::test]
async fn file_decode_rejects_every_oversized_observation_identity_without_rewriting() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("private");
    private::create_directory(&directory).unwrap();
    let storage = LocalFileStorage::new(directory.clone()).unwrap();
    let boundary = "é".repeat(128);
    let value = bounded_snapshot(&boundary, &boundary);
    let lease = storage.open(value.id.clone()).await.unwrap();
    lease.save(value.clone()).await.unwrap();
    assert_same(&lease.load().await.unwrap().unwrap(), &value);
    let path = journal_path(&directory, "identifier-limits");
    let original: serde_json::Value = snapshot_json(&std::fs::read(&path).unwrap()).unwrap();
    for pointer in [
        "/invocations/0/events/0/update/Tool/id",
        "/invocations/0/events/1/update/PermissionRequested/id",
        "/invocations/0/events/1/update/PermissionRequested/tool/id",
        "/invocations/0/events/2/update/PermissionCancelled/id",
        "/invocations/0/events/2/update/PermissionCancelled/tool_id",
    ] {
        let mut invalid = original.clone();
        *invalid.pointer_mut(pointer).expect("fixture field exists") =
            format!("{boundary}x").into();
        let bytes = journal_bytes(&invalid).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let error = lease.load().await.unwrap_err();
        assert!(
            matches!(&error, StorageError::Corrupt(_)),
            "oversized identity must be reported as corruption: {error:?}"
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes, "{pointer}");
    }
}

#[tokio::test]
async fn custom_storage_cannot_bypass_live_id_limits_or_trigger_provider_open_and_rewrite() {
    let boundary = "é".repeat(128);
    for oversized in [false, true] {
        let text = if oversized {
            format!("{boundary}x")
        } else {
            boundary.clone()
        };
        for target in [
            "tool-update",
            "review",
            "review-tool",
            "cancelled-review",
            "cancelled-tool",
        ] {
            let (tool, review) = match target {
                "tool-update" | "review-tool" | "cancelled-tool" => (text.as_str(), "review"),
                _ => ("tool", text.as_str()),
            };
            let mut value = bounded_snapshot(tool, review);
            match target {
                "tool-update" => value.invocations[0].events.truncate(1),
                "review" | "review-tool" => {
                    value.invocations[0].events.remove(0);
                    value.invocations[0].events.truncate(1);
                }
                _ => {}
            }
            let error = assert_custom_retention_admission(value, !oversized).await;
            if oversized {
                let AgentError::Storage(error) = error else {
                    panic!("{target}: {error:?}")
                };
                assert_identifier_limit(error);
            }
        }
    }
}
