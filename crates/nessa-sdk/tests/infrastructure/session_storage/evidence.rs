//! Restored observations must agree with their original request and settlement.
use super::*;
use nessa_sdk::application::agent_execution::sessions::InvocationSchedulingEvent;

fn review_snapshot() -> SessionSnapshot {
    let mut value = snapshot("evidence-integrity");
    let execution = value.invocations[0].request.execution_id.clone();
    let id = PermissionId::new("review").unwrap();
    let tool_id = ToolCallId::new("tool").unwrap();
    let tool = ToolCallUpdate::new(tool_id.clone(), None, None, None, None, None);
    let input = ToolReviewInput {
        name: "Read".into(),
        arguments_json: "{\"path\":\"original\"}".into(),
    };
    let request = PermissionRequest::new(id.clone(), execution.clone(), tool_id.clone(), choices());
    let request = cancelled_request(request, PermissionCancellationReason::execution_finished());
    value.invocations[0].events = vec![
        ExecutionEvent::new(
            execution.clone(),
            ExecutionUpdate::PermissionRequested {
                id,
                tool_id,
                observation: ToolObservation::default().with_update(tool),
                input: input.clone(),
                options: choices(),
            },
        ),
        ExecutionEvent::new(
            execution,
            ExecutionUpdate::PermissionCancelled(
                PermissionCancellation::from_record(
                    value.provider_session_id.clone(),
                    request,
                    input,
                    CancellationOrigin::Runtime,
                )
                .unwrap(),
            ),
        ),
    ];
    value
}

#[tokio::test]
async fn storage_rejects_forged_or_repeated_permission_cancellations_without_replacing_evidence() {
    let root = tempfile::tempdir().unwrap();
    private::create_directory(&root.path().join("private")).unwrap();
    let stores: Vec<Arc<dyn SessionStorage>> = vec![
        Arc::new(InMemoryStorage::new()),
        Arc::new(LocalFileStorage::new(root.path().join("private")).unwrap()),
    ];
    for storage in stores {
        let original = review_snapshot();
        let lease = storage.open(original.id.clone()).await.unwrap();
        lease.save(original.clone()).await.unwrap();
        for change in 0..7 {
            let mut invalid = original.clone();
            let events = &mut invalid.invocations[0].events;
            match change {
                0 => {
                    events.remove(0);
                }
                1 => {
                    events.push(events[1].clone());
                }
                2 => {
                    events.swap(0, 1);
                }
                _ => {
                    let execution = events[0].execution_id().clone();
                    let ExecutionUpdate::PermissionRequested {
                        id,
                        tool_id,
                        observation,
                        input,
                        options,
                    } = events[0].clone().into_update()
                    else {
                        unreachable!()
                    };
                    events[0] = ExecutionEvent::new(
                        execution,
                        ExecutionUpdate::PermissionRequested {
                            id: if change == 3 {
                                PermissionId::new("other").unwrap()
                            } else {
                                id
                            },
                            tool_id: if change == 4 {
                                ToolCallId::new("other").unwrap()
                            } else {
                                tool_id
                            },
                            input: if change == 5 {
                                ToolReviewInput {
                                    name: input.name,
                                    arguments_json: "changed".into(),
                                }
                            } else {
                                input
                            },
                            observation,
                            options: if change == 6 {
                                let choice = options.choices()[0].clone();
                                let policy =
                                    PermissionOfferPolicy::new(vec![choice.decision().clone()])
                                        .unwrap();
                                PermissionOptions::new(vec![choice], &policy).unwrap()
                            } else {
                                options
                            },
                        },
                    );
                }
            }
            assert!(
                matches!(lease.save(invalid).await, Err(StorageError::Corrupt(_))),
                "mutation {change}"
            );
            assert_same(&lease.load().await.unwrap().unwrap(), &original);
        }
    }
}

#[tokio::test]
async fn restored_terminal_contradictions_and_duplicate_events_are_rejected_but_failure_evidence_survives(
) {
    let root = tempfile::tempdir().unwrap();
    private::create_directory(&root.path().join("private")).unwrap();
    let storage = LocalFileStorage::new(root.path().join("private")).unwrap();
    let mut value = snapshot("terminal-integrity");
    let execution = value.invocations[0].request.execution_id.clone();
    value.invocations[0].events = vec![ExecutionEvent::new(
        execution,
        ExecutionUpdate::Finished(ExecutionOutcome::Cancelled),
    )];
    value.invocations[0].result = Some(Err(AgentError::ExecutionObservation {
        error: Box::new(AgentError::Protocol("contradiction".into())),
        execution_result: Some(Box::new(Ok(ExecutionOutcome::Completed))),
    }));
    let lease = storage.open(value.id.clone()).await.unwrap();
    lease.save(value.clone()).await.unwrap();
    assert_same(&lease.load().await.unwrap().unwrap(), &value);
    let mut invalid = value.clone();
    invalid.invocations[0].result = Some(Ok(ExecutionOutcome::Completed));
    assert!(matches!(
        lease.save(invalid).await,
        Err(StorageError::Corrupt(_))
    ));
    let path = journal_path(&root.path().join("private"), "terminal-integrity");
    let original = std::fs::read(&path).unwrap();
    for duplicate in [false, true] {
        let mut json: serde_json::Value = snapshot_json(&original).unwrap();
        if duplicate {
            let event = json["invocations"][0]["events"][0].clone();
            json["invocations"][0]["events"]
                .as_array_mut()
                .unwrap()
                .push(event);
        } else {
            json["invocations"][0]["result"] = serde_json::json!({"Ok": "Completed"});
        }
        let bytes = journal_bytes(&json).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        assert!(matches!(lease.load().await, Err(StorageError::Corrupt(_))));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}

#[tokio::test]
async fn finished_allows_only_correlated_cleanup_evidence_afterwards() {
    let storage = InMemoryStorage::new();
    let mut value = review_snapshot();
    let execution = value.invocations[0].request.execution_id.clone();
    value.invocations[0].events.insert(
        1,
        ExecutionEvent::new(
            execution.clone(),
            ExecutionUpdate::Finished(ExecutionOutcome::Completed),
        ),
    );
    value.invocations[0].result = Some(Ok(ExecutionOutcome::Completed));
    let lease = storage.open(value.id.clone()).await.unwrap();
    lease.save(value.clone()).await.unwrap();
    for update in [
        ExecutionUpdate::Message(MessageChunk::text("late")),
        value.invocations[0].events[0].clone().into_update(),
    ] {
        let mut invalid = value.clone();
        invalid.invocations[0]
            .events
            .push(ExecutionEvent::new(execution.clone(), update));
        assert!(matches!(
            lease.save(invalid).await,
            Err(StorageError::Corrupt(_))
        ));
        assert_same(&lease.load().await.unwrap().unwrap(), &value);
    }
}

#[tokio::test]
async fn file_load_rejects_cancellation_request_tampering_and_keeps_original_bytes() {
    let root = tempfile::tempdir().unwrap();
    private::create_directory(&root.path().join("private")).unwrap();
    let storage = LocalFileStorage::new(root.path().join("private")).unwrap();
    let value = review_snapshot();
    let lease = storage.open(value.id.clone()).await.unwrap();
    lease.save(value).await.unwrap();
    let path = journal_path(&root.path().join("private"), "evidence-integrity");
    let original = std::fs::read(&path).unwrap();
    for mutate in 0..3 {
        let mut json: serde_json::Value = snapshot_json(&original).unwrap();
        let events = json["invocations"][0]["events"].as_array_mut().unwrap();
        match mutate {
            0 => {
                events.remove(0);
            }
            1 => {
                events.push(events[1].clone());
            }
            _ => {
                events[0]["update"]["PermissionRequested"]["input"]["arguments_json"] =
                    serde_json::json!("tampered");
            }
        }
        let bytes = journal_bytes(&json).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        assert!(matches!(lease.load().await, Err(StorageError::Corrupt(_))));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}

#[tokio::test]
async fn file_restore_rejects_contradictory_cancellation_origin_without_rewriting_evidence() {
    let root = tempfile::tempdir().unwrap();
    private::create_directory(&root.path().join("private")).unwrap();
    let storage = LocalFileStorage::new(root.path().join("private")).unwrap();
    let value = review_snapshot();
    let lease = storage.open(value.id.clone()).await.unwrap();
    lease.save(value).await.unwrap();
    let path = journal_path(&root.path().join("private"), "evidence-integrity");
    let original: serde_json::Value = snapshot_json(&std::fs::read(&path).unwrap()).unwrap();
    for origin in [
        serde_json::json!("Provider"),
        serde_json::json!({"Client": {"principal_id": "caller", "surface_id": "host", "request_id": "withdraw"}}),
    ] {
        let mut corrupted = original.clone();
        corrupted["invocations"][0]["events"][1]["update"]["PermissionCancelled"]["origin"] =
            origin;
        let bytes = journal_bytes(&corrupted).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        assert!(matches!(lease.load().await, Err(StorageError::Corrupt(_))));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}

#[tokio::test]
async fn restored_oversized_execution_identity_is_rejected_without_rewriting_history() {
    let root = tempfile::tempdir().unwrap();
    private::create_directory(&root.path().join("private")).unwrap();
    let storage = LocalFileStorage::new(root.path().join("private")).unwrap();
    let value = snapshot("oversized-execution");
    let lease = storage.open(value.id.clone()).await.unwrap();
    lease.save(value).await.unwrap();
    let path = journal_path(&root.path().join("private"), "oversized-execution");
    let mut json: serde_json::Value = snapshot_json(&std::fs::read(&path).unwrap()).unwrap();
    json["invocations"][0]["execution_id"] =
        serde_json::json!("x".repeat(ExecutionId::MAX_BYTES + 1));
    let bytes = journal_bytes(&json).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    assert!(matches!(lease.load().await, Err(StorageError::Corrupt(_))));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[tokio::test]
async fn restored_attribution_enforces_each_identity_bound_without_rewriting_history() {
    let root = tempfile::tempdir().unwrap();
    private::create_directory(&root.path().join("private")).unwrap();
    let storage = LocalFileStorage::new(root.path().join("private")).unwrap();
    let mut value = snapshot("attribution-bounds");
    let exact = "é".repeat(ActionContext::MAX_IDENTITY_BYTES / 2);
    value.invocations[0].actor = ActionContext::new(&exact, &exact, &exact).unwrap();
    let mut queued = value.invocations[0].clone();
    queued.request.execution_id = ExecutionId::new("queued-attribution").unwrap();
    queued.submission = SubmissionMode::Queued;
    queued.events.clear();
    queued.scheduling = vec![InvocationSchedulingEvent {
        kind: InvocationKind::Queued,
        target: None,
        before: None,
        stage: InvocationStage::Queued,
        cause: SchedulingCause::Submitted,
        actor: Some(queued.actor.clone()),
    }];
    value.invocations.push(queued);
    let lease = storage.open(value.id.clone()).await.unwrap();
    lease.save(value.clone()).await.unwrap();
    assert_same(&lease.load().await.unwrap().unwrap(), &value);
    let path = journal_path(&root.path().join("private"), "attribution-bounds");
    let original: serde_json::Value = snapshot_json(&std::fs::read(&path).unwrap()).unwrap();
    for target in ["/invocations/0/actor", "/invocations/1/scheduling/0/actor"] {
        for field in ["principal_id", "surface_id", "request_id"] {
            let mut corrupted = original.clone();
            corrupted.pointer_mut(target).expect("serialized actor")[field] =
                serde_json::json!(format!("{exact}x"));
            let bytes = journal_bytes(&corrupted).unwrap();
            std::fs::write(&path, &bytes).unwrap();
            assert!(
                matches!(lease.load().await, Err(StorageError::Corrupt(_))),
                "{field}"
            );
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
    }
}

#[tokio::test]
async fn restored_session_failure_and_context_bounds_preserve_truthful_evidence() {
    let root = tempfile::tempdir().unwrap();
    private::create_directory(&root.path().join("private")).unwrap();
    let storage = LocalFileStorage::new(root.path().join("private")).unwrap();
    let mut value = review_snapshot();
    let ExecutionUpdate::PermissionCancelled(previous) = value.invocations[0].events[1].update()
    else {
        panic!("cancelled fixture");
    };
    let request = previous.request();
    let failed = cancelled_request(
        PermissionRequest::new(
            request.id().clone(),
            request.execution_id().clone(),
            request.tool_id().clone(),
            request.options().clone(),
        ),
        PermissionCancellationReason::session_failed(),
    );
    let exact = "é".repeat(ExecutionSessionId::MAX_BYTES / 2);
    value.provider_session_id = ExecutionSessionId::new(&exact).unwrap();
    let cancellation = PermissionCancellation::from_record(
        value.provider_session_id.clone(),
        failed,
        previous.input().clone(),
        CancellationOrigin::Runtime,
    )
    .unwrap();
    value.invocations[0].events[1] = ExecutionEvent::new(
        value.invocations[0].request.execution_id.clone(),
        ExecutionUpdate::PermissionCancelled(cancellation),
    );
    let lease = storage.open(value.id.clone()).await.unwrap();
    lease.save(value.clone()).await.unwrap();
    assert_same(&lease.load().await.unwrap().unwrap(), &value);
    let memory = InMemoryStorage::new();
    let in_memory = memory.open(value.id.clone()).await.unwrap();
    in_memory.save(value.clone()).await.unwrap();
    assert_same(&in_memory.load().await.unwrap().unwrap(), &value);
    let path = journal_path(&root.path().join("private"), "evidence-integrity");
    let original: serde_json::Value = snapshot_json(&std::fs::read(&path).unwrap()).unwrap();
    for pointer in [
        "/provider_session_id",
        "/invocations/0/events/1/update/PermissionCancelled/session_id",
    ] {
        let mut corrupted = original.clone();
        *corrupted.pointer_mut(pointer).expect("serialized context") =
            serde_json::json!(format!("{exact}x"));
        let bytes = journal_bytes(&corrupted).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        assert!(
            matches!(lease.load().await, Err(StorageError::Corrupt(_))),
            "{pointer}"
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
    for origin in [
        serde_json::json!("Provider"),
        serde_json::json!({"Client": {"principal_id":"caller", "surface_id":"host", "request_id":"close"}}),
    ] {
        let mut corrupted = original.clone();
        corrupted["invocations"][0]["events"][1]["update"]["PermissionCancelled"]["origin"] =
            origin;
        let bytes = journal_bytes(&corrupted).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        assert!(matches!(lease.load().await, Err(StorageError::Corrupt(_))));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}
