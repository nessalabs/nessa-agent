use super::support::*;

#[tokio::test]
async fn undrained_restored_generations_cannot_each_allocate_a_fresh_byte_budget() {
    let _slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit::default());
    let (root, mut config, model) = test_acp_configuration("byte-generation", 4096);
    config.max_frame_bytes = 16 * 1024 * 1024;
    config.max_incoming_frame_bytes = 16 * 1024 * 1024;
    config.execution_timeout = None;
    let binding = ClaudeAcpProvider::new(
        config,
        &model,
        TokenLimits::new(900, 100).unwrap(),
        audit.clone(),
    )
    .unwrap();
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    for id in ["first", "second"] {
        assert_eq!(
            opened.session.execute(prompt(id)).await.into_result(),
            Ok(ExecutionOutcome::Completed)
        );
        opened
            .session
            .shutdown(SessionCloseRequest::Explicit(close_action()))
            .await
            .into_result()
            .unwrap();
    }
    // Each retired generation still owns four 3 MiB messages. A replacement
    // worker shares that charge even though each individual channel has room.
    // Its first two chunks fit; the third crosses the shared 32 MiB ceiling.
    assert_eq!(
        opened.session.execute(prompt("third")).await.into_result(),
        Err(AgentError::Backpressure)
    );
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    assert_gone(&root, "pid");
    let mut bytes = 0;
    let mut completed = 0;
    while let Ok(Some(event)) = opened
        .events
        .next()
        .await
        .map_err(|failure| failure.into_error())
    {
        match event.into_update() {
            ExecutionUpdate::Message(chunk) if chunk.kind() == MessageKind::Text => {
                bytes += chunk.payload_bytes()
            }
            ExecutionUpdate::Finished(ExecutionOutcome::Completed) => completed += 1,
            update => panic!("unexpected retained update: {update:?}"),
        }
    }
    assert_eq!(bytes, 30 * 1024 * 1024);
    assert_eq!(completed, 2);
    // Draining releases the budget while the same context remains recoverable.
    assert_eq!(
        opened.session.execute(prompt("fourth")).await.into_result(),
        Ok(ExecutionOutcome::Completed)
    );
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    assert_gone(&root, "pid");
    assert_eq!(audit.closures.lock().unwrap().len(), 4);
}

#[tokio::test]
async fn execute_reopens_the_same_context_and_revives_an_exhausted_event_reader() {
    let _process_slot = process_test_slot().await;
    for mode in ["resume-context", "resume-no-id"] {
        let (root, binding) = test_acp_binding(mode, 16);
        let mut opened = binding
            .open(ProviderOpenRequest::without_startup_control(None))
            .await
            .unwrap();
        let id = opened.session.id().clone();
        assert_eq!(
            opened.session.execute(prompt("first")).await.into_result(),
            Ok(ExecutionOutcome::Completed)
        );
        assert_eq!(
            next(&mut opened).await,
            ExecutionUpdate::Message(MessageChunk::text("first"))
        );
        assert!(matches!(
            next(&mut opened).await,
            ExecutionUpdate::Finished(_)
        ));
        opened
            .session
            .shutdown(SessionCloseRequest::Explicit(close_action()))
            .await
            .into_result()
            .unwrap();
        assert_gone(&root, "pid");
        assert_eq!(
            opened
                .events
                .next()
                .await
                .map_err(|failure| failure.into_error())
                .unwrap(),
            None
        );
        assert_eq!(
            opened.session.execute(prompt("second")).await.into_result(),
            Ok(ExecutionOutcome::Completed)
        );
        assert_eq!(opened.session.id(), &id);
        assert_eq!(
            next(&mut opened).await,
            ExecutionUpdate::Message(MessageChunk::text("first|second"))
        );
        assert!(matches!(
            next(&mut opened).await,
            ExecutionUpdate::Finished(_)
        ));
        opened
            .session
            .shutdown(SessionCloseRequest::Explicit(close_action()))
            .await
            .into_result()
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(root.path().join("new-session-count")).unwrap(),
            "1"
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join("resume-observed")).unwrap(),
            id.as_str()
        );
        let launches: Vec<u32> =
            serde_json::from_str(&std::fs::read_to_string(root.path().join("launches")).unwrap())
                .unwrap();
        assert_eq!(launches.len(), 2);
        assert_ne!(launches[0], launches[1]);
        assert_gone(&root, "pid");
    }
}

#[tokio::test]
async fn restore_failures_never_create_a_replacement_conversation() {
    let _process_slot = process_test_slot().await;
    for mode in ["resume-unsupported", "resume-failure", "resume-wrong-id"] {
        let (root, binding) = test_acp_binding(mode, 16);
        let opened = binding
            .open(ProviderOpenRequest::without_startup_control(None))
            .await
            .unwrap();
        let id = opened.session.id().clone();
        opened
            .session
            .shutdown(SessionCloseRequest::Explicit(close_action()))
            .await
            .into_result()
            .unwrap();
        let result = opened.session.execute(prompt("later")).await.into_result();
        match mode {
            "resume-unsupported" => assert!(matches!(result, Err(AgentError::Unsupported(_)))),
            "resume-failure" => assert_eq!(
                result,
                Err(AgentError::Provider {
                    code: -32000,
                    diagnostic: Some(ProviderDiagnostic::new("restore failed")),
                })
            ),
            _ => assert!(matches!(result, Err(AgentError::Protocol(_)))),
        }
        assert_eq!(opened.session.id(), &id);
        opened
            .session
            .shutdown(SessionCloseRequest::Explicit(close_action()))
            .await
            .into_result()
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(root.path().join("new-session-count")).unwrap(),
            "1"
        );
        assert_gone(&root, "pid");
        assert!(opened
            .session
            .execute(prompt("retry"))
            .await
            .into_result()
            .is_err());
        opened
            .session
            .shutdown(SessionCloseRequest::Explicit(close_action()))
            .await
            .into_result()
            .unwrap();
        let launches: Vec<i32> =
            serde_json::from_str(&std::fs::read_to_string(root.path().join("launches")).unwrap())
                .unwrap();
        assert_eq!(launches.len(), 3);
        for pid in launches {
            assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        }
        assert_eq!(
            std::fs::read_to_string(root.path().join("new-session-count")).unwrap(),
            "1"
        );
    }
}

#[tokio::test]
async fn audit_failure_prevents_automatic_process_restart() {
    let _process_slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit {
        reject: true,
        ..Default::default()
    });
    let (root, binding) = test_acp_binding_with_audit("permission-stop", 16, audit);
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let active = start(&opened, "write").await;
    next(&mut opened).await;
    assert!(matches!(
        next(&mut opened).await,
        ExecutionUpdate::PermissionRequested { .. }
    ));
    assert_eq!(
        opened
            .session
            .shutdown(SessionCloseRequest::Explicit(close_action()))
            .await
            .into_result(),
        Err(AgentError::AuditFailure)
    );
    assert_eq!(active.await.unwrap(), Err(AgentError::AuditFailure));
    assert_eq!(
        opened.session.execute(prompt("later")).await.into_result(),
        Err(AgentError::AuditFailure)
    );
    let launches: Vec<u32> =
        serde_json::from_str(&std::fs::read_to_string(root.path().join("launches")).unwrap())
            .unwrap();
    assert_eq!(launches.len(), 1);
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn permission_ids_do_not_repeat_when_the_same_execution_id_is_resumed() {
    let _process_slot = process_test_slot().await;
    let (root, binding) = test_acp_binding("permission-stop", 16);
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let first = start(&opened, "write").await;
    next(&mut opened).await;
    let ExecutionUpdate::PermissionRequested { id: old_id, .. } = next(&mut opened).await else {
        panic!("expected review")
    };
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    assert_eq!(first.await.unwrap(), Ok(ExecutionOutcome::Cancelled));
    while opened
        .events
        .next()
        .await
        .map_err(|failure| failure.into_error())
        .unwrap()
        .is_some()
    {}
    let second = start(&opened, "write").await;
    assert!(matches!(
        next_after_restore(&mut opened).await,
        ExecutionUpdate::Tool(_)
    ));
    let ExecutionUpdate::PermissionRequested { id: new_id, .. } = next(&mut opened).await else {
        panic!("expected restored review")
    };
    assert_ne!(old_id, new_id);
    let answer = |id| PermissionAnswer {
        execution_id: ExecutionId::new("write").unwrap(),
        id,
        option_id: PermissionOptionId::new("deny-one").unwrap(),
        attribution: attribution(),
    };
    assert_eq!(
        opened
            .session
            .answer_permission(answer(old_id))
            .await
            .map_err(|failure| failure.into_error()),
        Err(AgentError::StalePermission)
    );
    opened
        .session
        .answer_permission(answer(new_id))
        .await
        .map_err(|failure| failure.into_error())
        .unwrap();
    assert_eq!(second.await.unwrap(), Ok(ExecutionOutcome::Completed));
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    assert!(!root.path().join("fixture.txt").exists());
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn close_interrupts_a_stalled_restore_before_the_startup_deadline() {
    let _process_slot = process_test_slot().await;
    let (root, mut config, model) = test_acp_configuration("resume-stall", 16);
    config.launch_timeout = Duration::from_secs(30);
    config.startup_timeout = Duration::from_secs(30);
    let binding = ClaudeAcpProvider::new(
        config,
        &model,
        TokenLimits::new(900, 100).unwrap(),
        Arc::new(RecordingAudit::default()),
    )
    .unwrap();
    let opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    let pending = start(&opened, "later").await;
    timeout(Duration::from_secs(3), async {
        while !root.path().join("resume-observed").exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    timeout(
        Duration::from_secs(2),
        opened
            .session
            .shutdown(SessionCloseRequest::Explicit(close_action())),
    )
    .await
    .unwrap()
    .into_result()
    .unwrap();
    assert_eq!(pending.await.unwrap(), Err(AgentError::Closed));
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn reopened_execution_remains_interruptible_and_does_not_start_parallel_workers() {
    let _process_slot = process_test_slot().await;
    let (root, binding) = test_acp_binding("stall", 16);
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    let active = start(&opened, "resumed").await;
    assert!(matches!(
        next_after_restore(&mut opened).await,
        ExecutionUpdate::Message(_)
    ));
    assert_eq!(
        opened
            .session
            .execute(prompt("competing"))
            .await
            .into_result(),
        Err(AgentError::Busy)
    );
    timeout(
        Duration::from_secs(3),
        opened
            .session
            .shutdown(SessionCloseRequest::Explicit(close_action())),
    )
    .await
    .unwrap()
    .into_result()
    .unwrap();
    assert_eq!(active.await.unwrap(), Ok(ExecutionOutcome::Cancelled));
    let launches: Vec<i32> =
        serde_json::from_str(&std::fs::read_to_string(root.path().join("launches")).unwrap())
            .unwrap();
    assert_eq!(launches.len(), 2);
    assert_gone(&root, "pid");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restoration_drains_old_evidence_without_replaying_its_reported_failure() {
    let _slot = process_test_slot().await;
    let (root, binding) = test_acp_binding("provider-error-once", 16);
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    assert_eq!(
        opened
            .session
            .execute(prompt("old-input"))
            .await
            .into_result(),
        Err(AgentError::Provider {
            code: -32000,
            diagnostic: Some(ProviderDiagnostic::new(
                "provider plan does not allow this request",
            )),
        })
    );
    // Leave the old reader untouched. Restoration retains its output with its
    // original identity, while the prior failure already belongs to old-input.
    assert_eq!(
        opened
            .session
            .execute(prompt("new-input"))
            .await
            .into_result(),
        Ok(ExecutionOutcome::Completed)
    );
    let old = opened
        .events
        .next()
        .await
        .map_err(|failure| failure.into_error())
        .unwrap()
        .unwrap();
    assert_eq!(old.execution_id().as_str(), "old-input");
    assert_eq!(
        old.update(),
        &ExecutionUpdate::Message(MessageChunk::text("retained-before-failure"))
    );
    let new = opened
        .events
        .next()
        .await
        .map_err(|failure| failure.into_error())
        .unwrap()
        .unwrap();
    assert_eq!(new.execution_id().as_str(), "new-input");
    assert_eq!(
        new.update(),
        &ExecutionUpdate::Message(MessageChunk::text("new-input"))
    );
    let terminal = opened
        .events
        .next()
        .await
        .map_err(|failure| failure.into_error())
        .unwrap()
        .unwrap();
    assert_eq!(terminal.execution_id().as_str(), "new-input");
    assert_eq!(
        terminal.update(),
        &ExecutionUpdate::Finished(ExecutionOutcome::Completed)
    );
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    assert_eq!(
        opened
            .events
            .next()
            .await
            .map_err(|failure| failure.into_error()),
        Ok(None)
    );
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn cancelled_preparation_keeps_its_worker_and_publishes_events_when_ready() {
    let _slot = process_test_slot().await;
    let (root, binding) = test_acp_binding("resume-gated", 16);
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    {
        let prepare = opened.session.prepare_invocation();
        tokio::pin!(prepare);
        tokio::select! {
            result = &mut prepare => panic!("gated startup completed: {result:?}"),
            _ = timeout(Duration::from_secs(5), async {
                while !root.path().join("resume-observed").exists() {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            }) => {}
        }
        assert!(root.path().join("resume-observed").exists());
        // Receipt of session/resume is not completed restoration. The old
        // generation is exhausted until startup publishes its replacement reader.
        assert_eq!(opened.events.next().await.unwrap(), None);
        // Drop just the caller's preparation future, retaining the session.
    }
    std::fs::write(root.path().join("resume-healthy"), "ready").unwrap();
    assert_eq!(
        opened
            .session
            .execute(prompt("after-cancellation"))
            .await
            .into_result(),
        Ok(ExecutionOutcome::Completed)
    );
    let mut messages = 0;
    let mut finished = 0;
    while let Some(event) = opened.events.next().await.unwrap() {
        match event.into_update() {
            ExecutionUpdate::Message(_) => messages += 1,
            ExecutionUpdate::Finished(ExecutionOutcome::Completed) => {
                finished += 1;
                break;
            }
            _ => {}
        }
    }
    assert!(messages > 0);
    assert_eq!(finished, 1);
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    let launches: Vec<i32> =
        serde_json::from_slice(&std::fs::read(root.path().join("launches")).unwrap()).unwrap();
    assert_eq!(
        launches.len(),
        2,
        "the cancelled waiter must not spawn a second restoration"
    );
    for pid in launches {
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    }
}
