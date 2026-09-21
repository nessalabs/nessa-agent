mod answers;
use super::support::*;

#[tokio::test]
async fn permission_is_typed_once_only_and_can_be_denied() {
    let _process_slot = process_test_slot().await;
    for (allow, attribution) in [false, true].into_iter().flat_map(|allow| {
        let human = ActionContext::new("reviewer", "nessa.panel", "human-answer").unwrap();
        let server = ActionContext::new("server", "nessa.server", "automatic-answer").unwrap();
        [
            ApprovalAttribution::new(human.clone(), ApprovalBasis::Explicit),
            attribution(),
            ApprovalAttribution::new(
                server,
                ApprovalBasis::Rule(
                    ApprovalRuleReference::new("file-rule", "revision-2", human).unwrap(),
                ),
            ),
        ]
        .into_iter()
        .map(move |attribution| (allow, attribution))
    }) {
        let (root, binding) = test_acp_binding("permission", 16);
        let mut opened = binding.open(None).await.unwrap();
        let active = start(&opened, "write").await;
        assert!(matches!(next(&mut opened).await, ExecutionUpdate::Tool(_)));
        let ExecutionUpdate::PermissionRequested {
            id,
            input,
            tool_id,
            observation,
            options,
        } = next(&mut opened).await
        else {
            panic!("expected permission");
        };
        assert_eq!(observation.title().as_deref(), Some("Write fixture.txt"));
        assert_eq!(input.name, "Write");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&input.arguments_json).unwrap(),
            serde_json::json!({"file_path":std::fs::canonicalize(root.path()).unwrap().join("fixture.txt"),"content":"fixture"})
        );
        let answer = PermissionAnswer {
            attribution: attribution.clone(),
            execution_id: ExecutionId::new("write").unwrap(),
            id: id.clone(),
            option_id: options
                .choices()
                .iter()
                .find(|option| {
                    option.decision().clone()
                        == if allow {
                            PermissionDecision::new(
                                PermissionEffect::Allow,
                                PermissionScope::request(),
                            )
                        } else {
                            PermissionDecision::new(
                                PermissionEffect::Deny,
                                PermissionScope::request(),
                            )
                        }
                })
                .unwrap()
                .id()
                .clone(),
        };
        assert_eq!(options.choices().len(), 2);
        assert_eq!(
            opened
                .session
                .answer_permission(PermissionAnswer {
                    option_id: PermissionOptionId::new("never-choose").unwrap(),
                    ..answer.clone()
                })
                .await
                .map_err(|failure| failure.into_error()),
            Err(AgentError::StalePermission)
        );
        assert_eq!(
            opened
                .session
                .answer_permission(PermissionAnswer {
                    execution_id: ExecutionId::new("another-execution").unwrap(),
                    ..answer.clone()
                })
                .await
                .map_err(|failure| failure.into_error()),
            Err(AgentError::StalePermission)
        );
        assert!(!root.path().join("fixture.txt").exists());
        let resolution = opened
            .session
            .answer_permission(answer.clone())
            .await
            .map_err(|failure| failure.into_error())
            .unwrap();
        assert_eq!(resolution.attribution(), &attribution);
        assert_eq!(resolution.input(), &input);
        assert_eq!(resolution.request().id(), &id);
        assert_eq!(resolution.request().tool_id(), &tool_id);
        assert_eq!(resolution.request().execution_id(), &answer.execution_id);
        assert!(
            matches!(resolution.request().state(), PermissionStateView::Answered { option_id, .. } if option_id == &answer.option_id)
        );
        assert_eq!(
            opened
                .session
                .answer_permission(answer)
                .await
                .map_err(|failure| failure.into_error()),
            Err(AgentError::StalePermission)
        );
        assert_eq!(active.await.unwrap().unwrap(), ExecutionOutcome::Completed);
        assert_eq!(root.path().join("fixture.txt").exists(), allow);
        let ExecutionUpdate::Tool(patch) = next(&mut opened).await else {
            panic!("expected sparse patch")
        };
        assert_eq!(patch.title().clone(), None);
        assert_eq!(patch.content().clone(), Some(vec![]));
        assert_eq!(patch.locations().clone(), None);
        opened
            .session
            .shutdown(SessionCloseRequest::Explicit(close_action()))
            .await
            .into_result()
            .unwrap();
        assert_gone(&root, "pid");
    }
}

#[tokio::test]
async fn provider_cancellation_retires_only_the_matching_permission() {
    let _process_slot = process_test_slot().await;
    for mode in [
        "permission-provider-cancel",
        "permission-provider-cancel-string",
    ] {
        let audit = Arc::new(RecordingAudit::default());
        let (root, binding) = test_acp_binding_with_audit(mode, 16, audit.clone());
        let mut opened = binding.open(None).await.unwrap();
        let active = start(&opened, "write").await;
        assert!(matches!(next(&mut opened).await, ExecutionUpdate::Tool(_)));
        let ExecutionUpdate::PermissionRequested { id: remaining, .. } = next(&mut opened).await
        else {
            panic!("expected first permission")
        };
        let ExecutionUpdate::PermissionRequested { id: cancelled, .. } = next(&mut opened).await
        else {
            panic!("expected second permission")
        };
        let ExecutionUpdate::PermissionCancelled(cancellation) = next(&mut opened).await else {
            panic!("expected cancellation record")
        };
        assert_eq!(cancellation.request().id(), &cancelled);
        assert_eq!(cancellation.origin(), &CancellationOrigin::Provider);
        assert_eq!(
            cancellation.request().state(),
            PermissionStateView::Cancelled {
                reason: &PermissionCancellationReason::provider_withdrawal()
            }
        );
        assert_eq!(audit.records.lock().unwrap().as_slice(), &[cancellation]);
        // The marker follows request cancellation, a duplicate and an unknown ID on
        // the same stream. Receiving it proves those notifications have been consumed.
        assert_eq!(
            next(&mut opened).await,
            ExecutionUpdate::Message(MessageChunk::text("provider cancellation processed"))
        );
        let answer = |id| PermissionAnswer {
            attribution: attribution(),
            execution_id: ExecutionId::new("write").unwrap(),
            id,
            option_id: PermissionOptionId::new("approve-one").unwrap(),
        };
        assert_eq!(
            opened
                .session
                .answer_permission(answer(cancelled))
                .await
                .map_err(|failure| failure.into_error()),
            Err(AgentError::StalePermission)
        );
        assert!(!root.path().join("fixture.txt").exists());
        opened
            .session
            .answer_permission(answer(remaining))
            .await
            .map_err(|failure| failure.into_error())
            .unwrap();
        assert_eq!(active.await.unwrap().unwrap(), ExecutionOutcome::Completed);
        assert_eq!(
            std::fs::read_to_string(root.path().join("provider-cancel-response")).unwrap(),
            "cancelled"
        );
        assert!(matches!(next(&mut opened).await, ExecutionUpdate::Tool(_)));
        assert_eq!(
            next(&mut opened).await,
            ExecutionUpdate::Finished(ExecutionOutcome::Completed)
        );
        opened
            .session
            .shutdown(SessionCloseRequest::Explicit(close_action()))
            .await
            .into_result()
            .unwrap();
        assert_gone(&root, "pid");
    }
}

#[tokio::test]
async fn close_cancels_pending_permission_before_cleanup() {
    let _process_slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit::default());
    let (root, binding) = test_acp_binding_with_audit("permission-stop", 16, audit.clone());
    let mut opened = binding.open(None).await.unwrap();
    let active = start(&opened, "write").await;
    next(&mut opened).await;
    let ExecutionUpdate::PermissionRequested { id, .. } = next(&mut opened).await else {
        panic!("expected permission")
    };
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    assert_eq!(active.await.unwrap().unwrap(), ExecutionOutcome::Cancelled);
    {
        let records = audit.records.lock().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].request().id(), &id);
        assert_eq!(records[0].session_id(), opened.session.id());
        assert_eq!(
            records[0].origin(),
            &CancellationOrigin::Client(close_action())
        );
        assert_eq!(
            records[0].request().state(),
            PermissionStateView::Cancelled {
                reason: &PermissionCancellationReason::session_closed()
            }
        );
        assert_eq!(records[0].input().name, "Write");
    }
    let choice: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root.path().join("permission-outcome")).unwrap(),
    )
    .unwrap();
    assert_eq!(choice["outcome"], "cancelled");
    assert!(!root.path().join("fixture.txt").exists());
    assert_eq!(
        opened
            .session
            .answer_permission(PermissionAnswer {
                attribution: attribution(),
                execution_id: ExecutionId::new("write").unwrap(),
                id: id.clone(),
                option_id: PermissionOptionId::new("approve-one").unwrap()
            })
            .await
            .map_err(|failure| failure.into_error()),
        Err(AgentError::Closed)
    );
    assert_gone(&root, "pid");
}

#[test]
fn persistent_permission_configuration_requires_supported_durable_scopes() {
    for decision in [
        PermissionDecision::new(
            PermissionEffect::Allow,
            PermissionScope::application(PermissionApplicationId::new("app").unwrap()),
        ),
        PermissionDecision::new(
            PermissionEffect::Deny,
            PermissionScope::application(PermissionApplicationId::new("app").unwrap()),
        ),
        PermissionDecision::new(
            PermissionEffect::Allow,
            PermissionScope::session(
                PermissionApplicationId::new("app").unwrap(),
                PermissionSessionId::new("session").unwrap(),
            ),
        ),
        PermissionDecision::new(
            PermissionEffect::Deny,
            PermissionScope::session(
                PermissionApplicationId::new("app").unwrap(),
                PermissionSessionId::new("session").unwrap(),
            ),
        ),
    ] {
        let (_root, mut config, model) = test_acp_configuration("permission", 16);
        config.permissions = PermissionOfferPolicy::new(vec![decision]).unwrap();
        assert!(matches!(
            ClaudeAcpProvider::new(
                config,
                &model,
                TokenLimits::new(900, 100).unwrap(),
                Arc::new(RecordingAudit::default())
            ),
            Err(AgentError::Unsupported(_))
        ));
    }
}

#[tokio::test]
async fn cancellation_audit_failure_is_reported_after_process_cleanup() {
    let _process_slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit {
        reject: true,
        ..Default::default()
    });
    let (root, binding) = test_acp_binding_with_audit("permission-stop", 16, audit);
    let mut opened = binding.open(None).await.unwrap();
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
    assert_gone(&root, "pid");
    assert!(!root.path().join("fixture.txt").exists());
}

#[tokio::test]
async fn dropping_the_event_reader_retains_cancellation_in_the_audit() {
    let _process_slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit::default());
    let (root, binding) = test_acp_binding_with_audit("permission-stop", 16, audit.clone());
    let mut opened = binding.open(None).await.unwrap();
    let active = start(&opened, "write").await;
    next(&mut opened).await;
    let ExecutionUpdate::PermissionRequested { id, input, .. } = next(&mut opened).await else {
        panic!("expected permission")
    };
    drop(opened.events);
    assert_eq!(active.await.unwrap(), Err(AgentError::Backpressure));
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    let records = audit.records.lock().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].request().id(), &id);
    assert_eq!(records[0].input(), &input);
    assert_eq!(records[0].origin(), &CancellationOrigin::Runtime);
    assert_eq!(
        records[0].request().state(),
        PermissionStateView::Cancelled {
            reason: &PermissionCancellationReason::event_consumer_dropped()
        }
    );
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn pending_reviews_retain_their_execution_end_cause_in_the_audit() {
    let _process_slot = process_test_slot().await;
    for (mode, reason) in [
        (
            "permission-stop",
            PermissionCancellationReason::deadline_exceeded(),
        ),
        (
            "permission-failure",
            PermissionCancellationReason::execution_failed(),
        ),
        (
            "permission-finished",
            PermissionCancellationReason::execution_finished(),
        ),
    ] {
        let audit = Arc::new(RecordingAudit::default());
        let (root, binding) = test_acp_binding_with_audit(mode, 16, audit.clone());
        let mut opened = binding.open(None).await.unwrap();
        let active = start(&opened, "write").await;
        assert!(matches!(next(&mut opened).await, ExecutionUpdate::Tool(_)));
        let ExecutionUpdate::PermissionRequested { id, input, .. } = next(&mut opened).await else {
            panic!("expected permission")
        };
        let result = active.await.unwrap();
        match reason.view() {
            PermissionCancellationReasonView::DeadlineExceeded => {
                assert_eq!(result, Err(AgentError::Deadline))
            }
            PermissionCancellationReasonView::ExecutionFailed => {
                assert!(matches!(result, Err(AgentError::Protocol(_))))
            }
            _ => assert_eq!(result, Ok(ExecutionOutcome::Completed)),
        }
        let ExecutionUpdate::PermissionCancelled(cancellation) = next(&mut opened).await else {
            panic!("expected cancellation before terminal event")
        };
        assert_eq!(cancellation.request().id(), &id);
        assert_eq!(cancellation.input(), &input);
        assert_eq!(
            cancellation.request().state(),
            PermissionStateView::Cancelled {
                reason: &reason.clone()
            }
        );
        assert_eq!(cancellation.origin(), &CancellationOrigin::Runtime);
        assert_eq!(audit.records.lock().unwrap().as_slice(), &[cancellation]);
        if reason == PermissionCancellationReason::execution_finished() {
            assert_eq!(
                next(&mut opened).await,
                ExecutionUpdate::Finished(ExecutionOutcome::Completed)
            );
        }
        opened
            .session
            .shutdown(SessionCloseRequest::Explicit(close_action()))
            .await
            .into_result()
            .unwrap();
        assert_gone(&root, "pid");
    }
}

#[tokio::test]
async fn provider_error_cannot_hide_a_failed_cancellation_audit() {
    let _process_slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit {
        reject: true,
        ..Default::default()
    });
    // This check is about a provider error not hiding a failed cancellation
    // audit, so the child must be reaped inside its budget rather than adding
    // honest uncertain-cleanup evidence to the failure being asserted. The
    // shared fixture's two-second budget has been measured close to the time a
    // busy host needs, and it stays short there because other checks escalate
    // against a stop-resistant child. Widen it only here.
    let (root, mut config, model) = test_acp_configuration("permission-provider-error", 16);
    config.kill_timeout = Duration::from_secs(30);
    let binding =
        ClaudeAcpProvider::new(config, &model, TokenLimits::new(900, 100).unwrap(), audit).unwrap();
    let mut opened = binding.open(None).await.unwrap();
    let active = start(&opened, "write").await;
    next(&mut opened).await;
    assert!(matches!(
        next(&mut opened).await,
        ExecutionUpdate::PermissionRequested { .. }
    ));
    let expected = AgentError::OperationAndCleanupFailure {
        operation_error: Box::new(AgentError::Provider { code: -32000 }),
        cleanup_error: Box::new(AgentError::AuditFailure),
    };
    let settlement_error = AgentError::ExecutionObservation {
        error: Box::new(expected.clone()),
        execution_result: Some(Box::new(Err(AgentError::Provider { code: -32000 }))),
    };
    assert_eq!(active.await.unwrap(), Err(settlement_error.clone()));
    assert_eq!(
        opened
            .session
            .shutdown(SessionCloseRequest::Explicit(close_action()))
            .await
            .into_result(),
        Err(expected.clone())
    );
    assert!(matches!(
        opened
            .events
            .next()
            .await
            .map_err(|failure| failure.into_error())
            .unwrap()
            .unwrap()
            .update(),
        ExecutionUpdate::PermissionCancelled(_)
    ));
    assert_eq!(
        opened
            .events
            .next()
            .await
            .map_err(|failure| failure.into_error()),
        Err(settlement_error)
    );
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn custom_guard_cancellation_preserves_its_reason_actor_and_review() {
    let _process_slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit::default());
    let (root, binding) = test_acp_binding_with_audit("permission-stop", 16, audit.clone());
    let mut opened = binding.open(None).await.unwrap();
    let active = start(&opened, "write").await;
    next(&mut opened).await;
    let ExecutionUpdate::PermissionRequested { id, input, .. } = next(&mut opened).await else {
        panic!("expected permission")
    };
    let actor = ActionContext::new("automatic-guard", "nessa.server", "guard-cancel").unwrap();
    let reason = PermissionCancellationReason::custom(
        CustomPermissionCancellationReason::new(
            "workspace-policy",
            "The target is outside the allowed workspace",
        )
        .unwrap(),
    );
    let request = PermissionCancellationRequest {
        execution_id: ExecutionId::new("write").unwrap(),
        id: id.clone(),
        reason: reason.clone(),
        actor: actor.clone(),
    };
    assert_eq!(
        opened
            .session
            .cancel_permission(PermissionCancellationRequest {
                execution_id: ExecutionId::new("another-execution").unwrap(),
                ..request.clone()
            })
            .await
            .map_err(|failure| failure.into_error()),
        Err(AgentError::StalePermission)
    );
    assert!(audit.records.lock().unwrap().is_empty());
    let cancellation = opened
        .session
        .cancel_permission(request.clone())
        .await
        .map_err(|failure| failure.into_error())
        .unwrap();
    assert_eq!(cancellation.request().id(), &id);
    assert_eq!(
        cancellation.request().state(),
        PermissionStateView::Cancelled {
            reason: &reason.clone()
        }
    );
    assert_eq!(cancellation.input(), &input);
    assert_eq!(cancellation.origin(), &CancellationOrigin::Client(actor));
    assert_eq!(
        audit.records.lock().unwrap().as_slice(),
        std::slice::from_ref(&cancellation)
    );
    assert_eq!(
        next(&mut opened).await,
        ExecutionUpdate::PermissionCancelled(cancellation)
    );
    assert_eq!(
        opened
            .session
            .cancel_permission(request)
            .await
            .map_err(|failure| failure.into_error()),
        Err(AgentError::StalePermission)
    );
    assert!(!root.path().join("fixture.txt").exists());
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    assert_eq!(active.await.unwrap(), Ok(ExecutionOutcome::Cancelled));
    assert_eq!(audit.records.lock().unwrap().len(), 1);
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn a_stalled_audit_is_bounded_and_does_not_prevent_process_cleanup() {
    let _process_slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit {
        stall: true,
        ..Default::default()
    });
    let (root, binding) = test_acp_binding_with_audit("permission-stop", 16, audit);
    let mut opened = binding.open(None).await.unwrap();
    let active = start(&opened, "write").await;
    next(&mut opened).await;
    assert!(matches!(
        next(&mut opened).await,
        ExecutionUpdate::PermissionRequested { .. }
    ));
    assert_eq!(
        timeout(
            Duration::from_secs(2),
            opened
                .session
                .shutdown(SessionCloseRequest::Explicit(close_action()))
        )
        .await
        .unwrap()
        .into_result(),
        Err(AgentError::AuditFailure)
    );
    assert_eq!(active.await.unwrap(), Err(AgentError::AuditFailure));
    assert_gone(&root, "pid");
}

/// A review this binding will not put to a host costs that tool, not the turn.
///
/// This is the shape of the bug that started it: a tool the adapter would not
/// review ended the execution and closed the session, and the caller was shown
/// nothing at all. Each mode here refuses for a different reason, and each one
/// has to leave the agent told, the turn finishing, and the refusal recorded.
#[tokio::test]
async fn a_declined_review_refuses_the_tool_and_leaves_the_turn_running() {
    let _process_slot = process_test_slot().await;
    for (mode, reason, outcome, named) in [
        (
            "declined-tool",
            ReviewDeclineReason::ToolNotReviewable,
            serde_json::json!({"outcome":"selected","optionId":"deny-one"}),
            Some("Bash"),
        ),
        (
            // No rejection was offered — only a persistent choice this binding
            // may not make for a host — so the review is cancelled instead.
            "declined-options",
            ReviewDeclineReason::UnusableOptions,
            serde_json::json!({"outcome":"cancelled"}),
            Some("Write"),
        ),
        (
            // Unreadable, but the provider still offered a rejection to choose:
            // why this binding refuses does not change how plainly it can say
            // so, and a chosen "no" is plainer than a cancellation.
            "declined-unreadable",
            ReviewDeclineReason::UnreadableRequest,
            serde_json::json!({"outcome":"selected","optionId":"deny-one"}),
            Some("Write"),
        ),
    ] {
        let audit = Arc::new(RecordingAudit::default());
        let (root, binding) = test_acp_binding_with_audit(mode, 16, audit.clone());
        let mut opened = binding.open(None).await.unwrap();
        let active = start(&opened, "write").await;

        // The call is observed. Refusing the frame hid the tool as well as
        // ending the turn, so the tool row still has to arrive.
        assert!(
            matches!(next(&mut opened).await, ExecutionUpdate::Tool(_)),
            "{mode}: the declined call was not shown"
        );
        // No review reaches a host: there was nothing this binding could put
        // to one. The turn carries on to its own ending.
        let ExecutionUpdate::Message(chunk) = next(&mut opened).await else {
            panic!("{mode}: expected the turn to continue after the decline");
        };
        assert_eq!(chunk.as_str(), "declined and carried on");
        assert_eq!(
            timeout(Duration::from_secs(3), active)
                .await
                .unwrap()
                .unwrap(),
            Ok(ExecutionOutcome::Completed),
            "{mode}: a refused tool ended the execution"
        );

        // The agent was told, and told in the strongest terms the frame
        // allowed: a chosen rejection where one was offered.
        let answered: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(root.path().join("permission-outcome")).unwrap(),
        )
        .unwrap();
        assert_eq!(answered, outcome, "{mode}");

        // Both halves of the evidence: the decision, then what the wire did.
        let declines = audit.declines.lock().unwrap().clone();
        assert_eq!(declines.len(), 2, "{mode}: {declines:?}");
        for record in &declines {
            assert_eq!(record.execution_id().as_str(), "write");
            assert_eq!(record.decline().reason(), reason, "{mode}");
            assert_eq!(record.decline().tool(), named, "{mode}");
        }
        assert_eq!(declines[0].delivery(), &PermissionAnswerDelivery::Selected);
        assert_eq!(declines[1].delivery(), &PermissionAnswerDelivery::Written);
        // A decline is not a cancellation: nothing was pending to cancel.
        assert!(audit.records.lock().unwrap().is_empty(), "{mode}");
        assert!(audit.answers.lock().unwrap().is_empty(), "{mode}");

        // The session outlives the refusal and can still be closed cleanly.
        opened
            .session
            .shutdown(SessionCloseRequest::Explicit(close_action()))
            .await
            .into_result()
            .unwrap();
    }
}

/// A sink that will not take the refusal does not make the refusal go away.
///
/// The agent still has to be told: an answer it never receives is a turn that
/// waits forever, which is the failure this whole path exists to prevent. The
/// audit failure is reported after the answer has gone, not instead of it.
#[tokio::test]
async fn a_refusal_reaches_the_agent_even_when_its_audit_cannot_be_recorded() {
    let _process_slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit {
        reject: true,
        ..RecordingAudit::default()
    });
    let (root, binding) = test_acp_binding_with_audit("declined-tool", 16, audit.clone());
    let mut opened = binding.open(None).await.unwrap();
    let active = start(&opened, "write").await;
    assert!(matches!(next(&mut opened).await, ExecutionUpdate::Tool(_)));

    // The execution ends on the audit failure — evidence is mandatory here, and
    // that is a different failure from "a tool was unfamiliar".
    let outcome = timeout(Duration::from_secs(3), active)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(outcome, Err(AgentError::AuditFailure), "{outcome:?}");

    // The refusal went anyway: the provider recorded the answer it was given.
    // Exactly one answer, and it is the refusal: a request answered twice is
    // its own protocol fault, and the audit failure must not cause one.
    let answered = std::fs::read_to_string(root.path().join("permission-outcomes")).unwrap();
    assert_eq!(
        answered.lines().collect::<Vec<_>>(),
        vec![r#"{"optionId": "deny-one", "outcome": "selected"}"#],
        "the refusal was not the provider's only answer"
    );
    let _ = opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await;
}
