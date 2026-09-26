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
        let mut opened = binding
            .open(ProviderOpenRequest::without_startup_control(None))
            .await
            .unwrap();
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
        let mut opened = binding
            .open(ProviderOpenRequest::without_startup_control(None))
            .await
            .unwrap();
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
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
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
        Err(rejected_audits(3))
    );
    assert_eq!(active.await.unwrap(), Err(rejected_audits(3)));
    assert_gone(&root, "pid");
    assert!(!root.path().join("fixture.txt").exists());
}

#[tokio::test]
async fn dropping_the_event_reader_retains_cancellation_in_the_audit() {
    let _process_slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit::default());
    let (root, binding) = test_acp_binding_with_audit("permission-stop", 16, audit.clone());
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let active = start(&opened, "write").await;
    next(&mut opened).await;
    let ExecutionUpdate::PermissionRequested { id, input, .. } = next(&mut opened).await else {
        panic!("expected permission")
    };
    drop(opened.events);
    // The first backpressure is the drive loop observing consumer loss. The
    // second is the separately attempted final execution-event publication;
    // the cancellation record below proves teardown still ran between them.
    assert_eq!(
        active.await.unwrap(),
        Err(ordered_failures(&[
            AgentError::Backpressure,
            AgentError::Backpressure,
        ]))
    );
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
        let limit = Duration::from_secs(1);
        let (root, binding, clock) = test_acp_binding_on_clock(mode, audit.clone(), Some(limit));
        let mut opened = binding
            .open(ProviderOpenRequest::without_startup_control(None))
            .await
            .unwrap();
        let active = start(&opened, "write").await;
        assert!(matches!(next(&mut opened).await, ExecutionUpdate::Tool(_)));
        let ExecutionUpdate::PermissionRequested { id, input, .. } = next(&mut opened).await else {
            panic!("expected permission")
        };
        if matches!(
            reason.view(),
            PermissionCancellationReasonView::DeadlineExceeded
        ) {
            // The review is pending when the execution's bound passes.
            clock.advance(limit);
        }
        let result = promptly(active).await.unwrap();
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
    let provider_failure = AgentError::Provider {
        code: -32000,
        diagnostic: Some(ProviderDiagnostic::new("fixture provider failure")),
    };
    let expected = rejected_audits(3);
    let settlement_error = AgentError::ExecutionObservation {
        error: Box::new(expected.clone()),
        execution_result: Some(Box::new(Err(provider_failure))),
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
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
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
    let (root, binding, clock) = test_acp_binding_on_clock("permission-stop", audit, None);
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
    // Each stalled audit record is bounded by the grace, which passes.
    let closing = opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()));
    assert_eq!(
        timeout(Duration::from_secs(10), clock.passing(a_grace, closing))
            .await
            .unwrap()
            .into_result(),
        Err(rejected_audits(3))
    );
    assert_eq!(active.await.unwrap(), Err(rejected_audits(3)));
    assert_gone(&root, "pid");
}

/// A review this binding will not put to a host costs that tool, not the turn.
///
/// This is the shape of the bug that started it: a tool the adapter would not
/// review ended the execution and closed the session, and the caller was shown
/// nothing at all. Each mode here refuses for a different reason, and each one
/// has to leave the agent told, the turn finishing, and the refusal recorded.
async fn declined_updates(
    opened: &mut OpenedProviderSession,
) -> (ReviewDeclineId, ReviewDecline, ReviewDeclineStage) {
    let ExecutionUpdate::ReviewDeclined(selected) = next(opened).await else {
        panic!("expected selected declined-review notice")
    };
    assert_eq!(selected.stage(), ReviewDeclineStage::Selected);
    let ExecutionUpdate::ReviewDeclined(final_observation) = next(opened).await else {
        panic!("expected final declined-review notice")
    };
    assert_eq!(final_observation.id(), selected.id());
    assert_eq!(final_observation.decline(), selected.decline());
    assert_ne!(final_observation.stage(), ReviewDeclineStage::Selected);
    (
        selected.id().clone(),
        selected.decline().clone(),
        final_observation.stage(),
    )
}

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
        let mut opened = binding
            .open(ProviderOpenRequest::without_startup_control(None))
            .await
            .unwrap();
        let active = start(&opened, "write").await;

        // The call is observed. Refusing the frame hid the tool as well as
        // ending the turn, so the tool row still has to arrive.
        assert!(
            matches!(next(&mut opened).await, ExecutionUpdate::Tool(_)),
            "{mode}: the declined call was not shown"
        );
        let (decline_id, notice, delivery) = declined_updates(&mut opened).await;
        assert_eq!(notice.reason(), reason, "{mode}");
        assert_eq!(notice.declared(), named, "{mode}");
        assert_eq!(delivery, ReviewDeclineStage::WriteConfirmed, "{mode}");
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
            assert_eq!(record.decline().declared(), named, "{mode}");
            assert_eq!(record.id(), &decline_id, "{mode}");
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
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let active = start(&opened, "write").await;
    assert!(matches!(next(&mut opened).await, ExecutionUpdate::Tool(_)));
    assert_eq!(
        declined_updates(&mut opened).await.2,
        ReviewDeclineStage::WriteConfirmed
    );

    // The execution ends on the audit failure — evidence is mandatory here, and
    // that is a different failure from "a tool was unfamiliar".
    let outcome = timeout(Duration::from_secs(3), active)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(outcome, Err(rejected_audits(4)), "{outcome:?}");

    // The refusal went anyway: the provider recorded the answer it was given.
    // Exactly one answer, and it is the refusal: a request answered twice is
    // its own protocol fault, and the audit failure must not cause one.
    //
    // The execution ends on its own audit failure, which says nothing about
    // whether the provider has read the refusal yet — so wait for the provider
    // to say so rather than assuming this side's ending ordered the other's.
    wait_for_file(&root, "permission-outcomes").await;
    let answered = std::fs::read_to_string(root.path().join("permission-outcomes")).unwrap();
    // Compared as JSON, not as text: which order a serializer writes an
    // object's keys in is a build's business, not this contract's.
    assert_eq!(
        answered
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>(),
        vec![serde_json::json!({"outcome":"selected","optionId":"deny-one"})],
        "the refusal was not the provider's only answer"
    );
    let _ = opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await;
}

/// A refusal that cannot be written is recorded as one that was not delivered.
///
/// The decision was still made locally, and the record says both things: that
/// this binding refused, and that the agent may never have heard it.
#[tokio::test]
async fn a_refusal_that_cannot_be_written_keeps_its_decision_and_its_delivery_failure() {
    let _process_slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit::default());
    let (root, binding) = test_acp_binding_with_audit("declined-write-failure", 16, audit.clone());
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let active = start(&opened, "write").await;
    assert!(matches!(next(&mut opened).await, ExecutionUpdate::Tool(_)));
    assert_eq!(
        declined_updates(&mut opened).await.2,
        ReviewDeclineStage::WriteUnconfirmed
    );
    let outcome = timeout(Duration::from_secs(5), active)
        .await
        .unwrap()
        .unwrap();
    let Err(failure) = outcome else {
        panic!("an undeliverable refusal cannot report success: {outcome:?}");
    };
    assert!(matches!(failure, AgentError::Transport(_)), "{failure:?}");

    let declines = audit.declines.lock().unwrap().clone();
    assert_eq!(declines.len(), 2, "{declines:?}");
    assert_eq!(declines[0].delivery(), &PermissionAnswerDelivery::Selected);
    assert_eq!(
        declines[1].delivery(),
        &PermissionAnswerDelivery::Failed(failure)
    );
    assert_eq!(declines[0].decline(), declines[1].decline());
    let _ = opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await;
    wait_until_gone(&root, "pid").await;
}

/// Both failures at once keep both causes, as an answered review's do.
///
/// An audit that will not take the record must not swallow the reason the agent
/// never heard the refusal: they are two different problems for two different
/// people, and a generic label for both helps neither.
#[tokio::test]
async fn a_refusal_failing_to_write_and_to_record_preserves_both_causes() {
    let _process_slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit {
        reject: true,
        ..RecordingAudit::default()
    });
    let (root, binding) = test_acp_binding_with_audit("declined-write-failure", 16, audit);
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let active = start(&opened, "write").await;
    assert!(matches!(next(&mut opened).await, ExecutionUpdate::Tool(_)));
    assert_eq!(
        declined_updates(&mut opened).await.2,
        ReviewDeclineStage::WriteUnconfirmed
    );
    let outcome = timeout(Duration::from_secs(5), active)
        .await
        .unwrap()
        .unwrap();
    // The sink refuses the teardown's own records too, so the refusal's pair of
    // causes arrives inside that outer failure rather than instead of it. All
    // four survive, each still saying what it is.
    assert_eq!(
        outcome,
        Err(ordered_failures(&[
            AgentError::AuditFailure,
            AgentError::PermissionAnswerDeliveryAndAuditFailure {
                delivery_error: Box::new(AgentError::Transport("stdin write failed".into())),
                cleanup_error: None,
            },
            AgentError::AuditFailure,
            AgentError::AuditFailure,
        ])),
        "both causes and both later audits are required"
    );
    let _ = opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await;
    wait_until_gone(&root, "pid").await;
}

/// The recorded name is the provider's claim about its own frame, and the
/// refusal is not decided from it.
///
/// A provider that observes one tool and then asks to review another gets the
/// answer its observation earned. The record keeps the name it declared, under
/// a field that says whose claim it is, rather than a name this binding checked.
#[tokio::test]
async fn a_declared_name_is_recorded_as_a_claim_and_does_not_decide_the_refusal() {
    let _process_slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit::default());
    let (root, binding) = test_acp_binding_with_audit("declined-divergent-name", 16, audit.clone());
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let active = start(&opened, "write").await;
    assert!(matches!(next(&mut opened).await, ExecutionUpdate::Tool(_)));
    let (_, notice, delivery) = declined_updates(&mut opened).await;
    assert_eq!(delivery, ReviewDeclineStage::WriteConfirmed);
    assert_eq!(notice.declared(), Some("Read"));
    let ExecutionUpdate::Message(chunk) = next(&mut opened).await else {
        panic!("expected the turn to continue after the decline");
    };
    assert_eq!(chunk.as_str(), "declined and carried on");
    assert_eq!(
        timeout(Duration::from_secs(3), active)
            .await
            .unwrap()
            .unwrap(),
        Ok(ExecutionOutcome::Completed)
    );

    let declines = audit.declines.lock().unwrap().clone();
    assert_eq!(declines.len(), 2, "{declines:?}");
    // Refused for what was observed under this identity — a denied tool — and
    // not for the reviewable name the frame put forward.
    assert_eq!(
        declines[0].decline().reason(),
        ReviewDeclineReason::ToolNotReviewable
    );
    assert_eq!(declines[0].decline().declared(), Some("Read"));
    let _ = opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await;
    wait_until_gone(&root, "pid").await;
}

/// An agent asks, a host answers, and the agent hears exactly what was chosen.
///
/// The whole path in one test, because every part of it only means something
/// with the others: what arrives is a schema, what is shown is a question, and
/// what goes back is the content that schema asked for.
#[tokio::test]
async fn a_question_reaches_a_host_and_its_answer_reaches_the_agent() {
    let _process_slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit::default());
    let (root, binding) = test_acp_binding_with_audit("question", 16, audit.clone());
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let active = start(&opened, "write").await;

    let ExecutionUpdate::QuestionAsked { id, question } = next(&mut opened).await else {
        panic!("expected the agent's question");
    };
    assert_eq!(question.message(), "Which environment should I deploy to?");
    assert_eq!(
        question.questions().len(),
        1,
        "the companion is not a question"
    );
    let asked = &question.questions()[0];
    assert_eq!(asked.key(), "question_0");
    assert_eq!(asked.header(), Some("Environment"));
    assert_eq!(asked.shape(), AnswerShape::One);
    assert!(asked.free_text(), "its companion offers words of their own");
    assert_eq!(asked.options()[0].value(), "staging");
    assert_eq!(asked.options()[0].description(), Some("Safe to break"));

    // An answer that chooses what the question did not offer never leaves here.
    let refused = opened
        .session
        .answer_question(QuestionAnswer {
            actor: answerer(),
            execution_id: ExecutionId::new("write").unwrap(),
            id: id.clone(),
            choices: Some(vec![QuestionChoice::new(
                "question_0",
                vec!["elsewhere".into()],
                None,
            )
            .unwrap()]),
        })
        .await;
    let refused = refused.map_err(ProviderOperationFailure::into_error);
    assert!(
        matches!(refused, Err(AgentError::InvalidInput(_))),
        "{refused:?}"
    );

    opened
        .session
        .answer_question(QuestionAnswer {
            actor: answerer(),
            execution_id: ExecutionId::new("write").unwrap(),
            id: id.clone(),
            choices: Some(vec![QuestionChoice::new(
                "question_0",
                vec!["staging".into()],
                Some("and only the eu region".into()),
            )
            .unwrap()]),
        })
        .await
        .unwrap();

    // The ask stops waiting as its own observation, so a surface that did not
    // answer still sees it close.
    let ExecutionUpdate::QuestionClosed { id: closed } = next(&mut opened).await else {
        panic!("expected the question to close");
    };
    assert_eq!(closed, id);

    let ExecutionUpdate::Message(chunk) = next(&mut opened).await else {
        panic!("expected the turn to continue");
    };
    assert_eq!(chunk.as_str(), "answered and carried on");
    assert_eq!(
        timeout(Duration::from_secs(3), active)
            .await
            .unwrap()
            .unwrap(),
        Ok(ExecutionOutcome::Completed)
    );

    // What the agent received is the content its own schema asked for.
    wait_for_file(&root, "question-answer").await;
    let answered: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root.path().join("question-answer")).unwrap(),
    )
    .unwrap();
    assert_eq!(answered["action"], "accept");
    assert_eq!(answered["content"]["question_0"], "staging");
    assert_eq!(
        answered["content"]["question_0_custom"],
        "and only the eu region"
    );

    // Both halves of the evidence, in order.
    let answers = audit.answered_questions.lock().unwrap().clone();
    assert_eq!(answers.len(), 2, "{answers:?}");
    assert_eq!(answers[0].delivery(), &PermissionAnswerDelivery::Selected);
    assert_eq!(answers[1].delivery(), &PermissionAnswerDelivery::Written);
    for record in &answers {
        assert_eq!(record.execution_id().as_str(), "write");
        assert_eq!(record.question_id(), &id);
        // Who answered is kept, not merely checked on the way in: an explicit
        // answer's audit without its initiator is not evidence of anything.
        assert_eq!(record.actor(), Some(&answerer()));
        let QuestionResponse::Answered(accepted) = record.response() else {
            panic!("the question was answered, not declined");
        };
        assert_eq!(accepted.choices()[0].key(), "question_0");
    }

    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
}

/// The verified caller these tests answer as.
fn answerer() -> ActionContext {
    ActionContext::new("reviewer", "nessa.panel", "answer-question").unwrap()
}

/// The provider's recorded answers, each with its request id exactly as sent.
fn provider_answers(root: &TempDir) -> Vec<serde_json::Value> {
    std::fs::read_to_string(root.path().join("answers"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/// Answer one open ask with `value`, as the verified caller.
async fn answer_with(opened: &OpenedProviderSession, id: &QuestionId, value: &str) {
    opened
        .session
        .answer_question(QuestionAnswer {
            actor: answerer(),
            execution_id: ExecutionId::new("write").unwrap(),
            id: id.clone(),
            choices: Some(vec![QuestionChoice::new(
                "question_0",
                vec![value.into()],
                None,
            )
            .unwrap()]),
        })
        .await
        .unwrap();
}

/// Two requests whose ids share their text are still two asks.
///
/// The review reproduced `1` and `"1"` colliding: identity was minted from the
/// provider's id as text, so the second ask replaced the first and answering
/// the one on screen wrote to the wrong request. Identity is now minted here.
#[tokio::test]
async fn two_asks_whose_ids_share_their_text_are_answered_separately() {
    let _process_slot = process_test_slot().await;
    let (root, binding) = test_acp_binding("asks-collide", 16);
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let active = start(&opened, "write").await;

    let ExecutionUpdate::QuestionAsked { id: first, .. } = next(&mut opened).await else {
        panic!("expected the first ask");
    };
    let ExecutionUpdate::QuestionAsked { id: second, .. } = next(&mut opened).await else {
        panic!("expected the second ask");
    };
    assert_ne!(first, second, "two requests are two asks");

    answer_with(&opened, &first, "staging").await;
    assert!(matches!(
        next(&mut opened).await,
        ExecutionUpdate::QuestionClosed { .. }
    ));
    answer_with(&opened, &second, "production").await;
    assert!(matches!(
        next(&mut opened).await,
        ExecutionUpdate::QuestionClosed { .. }
    ));
    assert_eq!(
        timeout(Duration::from_secs(3), active)
            .await
            .unwrap()
            .unwrap(),
        Ok(ExecutionOutcome::Completed)
    );

    // Each provider request received the answer meant for it: number `1` got
    // the first, string `"1"` the second.
    let answers = provider_answers(&root);
    assert_eq!(answers.len(), 2, "{answers:?}");
    let answered = |id: serde_json::Value| {
        answers
            .iter()
            .find(|answer| answer["id"] == id)
            .unwrap_or_else(|| panic!("no answer for {id}"))["result"]["content"]["question_0"]
            .clone()
    };
    assert_eq!(answered(serde_json::json!(1)), "staging");
    assert_eq!(answered(serde_json::json!("1")), "production");
    let _ = opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await;
}

/// A question the provider takes back ends with its evidence, and cannot then
/// be answered into a request nobody is waiting on.
#[tokio::test]
async fn a_withdrawn_ask_closes_is_recorded_and_can_no_longer_be_answered() {
    let _process_slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit::default());
    let (root, binding) = test_acp_binding_with_audit("ask-withdrawn", 16, audit.clone());
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let active = start(&opened, "write").await;

    let ExecutionUpdate::QuestionAsked { id, .. } = next(&mut opened).await else {
        panic!("expected the ask");
    };
    let ExecutionUpdate::QuestionClosed { id: closed } = next(&mut opened).await else {
        panic!("a withdrawn ask must close as its own observation");
    };
    assert_eq!(closed, id);
    assert_eq!(
        timeout(Duration::from_secs(3), active)
            .await
            .unwrap()
            .unwrap(),
        Ok(ExecutionOutcome::Completed)
    );

    // The provider was told, so it is not left waiting.
    let answers = provider_answers(&root);
    assert_eq!(answers.len(), 1, "{answers:?}");
    assert_eq!(answers[0]["id"], "ask");
    assert_eq!(answers[0]["result"]["action"], "cancel");

    // The ending is recorded with its cause, and with no initiator invented.
    let records = audit.answered_questions.lock().unwrap().clone();
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(
        records[0].response(),
        &QuestionResponse::Cancelled(QuestionCancellation::ProviderWithdrawal)
    );
    assert_eq!(records[0].actor(), None);
    // The ask it ended travels with it, as asked.
    assert_eq!(records[0].question().message(), "Which environment?");
    assert_eq!(records[0].delivery(), &PermissionAnswerDelivery::Written);

    // Nothing is waiting any more, so an answer is refused rather than sent.
    let late = opened
        .session
        .answer_question(QuestionAnswer {
            actor: answerer(),
            execution_id: ExecutionId::new("write").unwrap(),
            id,
            choices: None,
        })
        .await
        .map_err(ProviderOperationFailure::into_error);
    assert!(matches!(late, Err(AgentError::InvalidInput(_))), "{late:?}");
    let _ = opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await;
}

/// No more asks are admitted than a surface can show.
///
/// The review found a ninth ask stranded: the binding admitted it, the view
/// could not show it, and nothing could ever answer it. The excess is now
/// answered at once, so the agent is not left waiting on it.
#[tokio::test]
async fn asks_beyond_what_a_surface_can_show_are_answered_rather_than_stranded() {
    let _process_slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit::default());
    let (root, binding) = test_acp_binding_with_audit("asks-overflow", 32, audit.clone());
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let active = start(&opened, "write").await;

    let mut asked = 0;
    loop {
        match next(&mut opened).await {
            ExecutionUpdate::QuestionAsked { .. } => asked += 1,
            ExecutionUpdate::Message(chunk) if chunk.as_str() == "done asking" => break,
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(asked, MAX_OPEN_QUESTIONS);
    assert_eq!(
        timeout(Duration::from_secs(3), active)
            .await
            .unwrap()
            .unwrap(),
        Ok(ExecutionOutcome::Completed)
    );
    let overflow = provider_answers(&root)
        .into_iter()
        .find(|answer| answer["id"] == "a8")
        .expect("the ninth ask was answered");
    assert_eq!(overflow["result"]["action"], "cancel");
    // A refusal the agent acts on is a decision, so it is on record — decided,
    // then written — and says which limit it ran into.
    assert_refused(&audit, QuestionRefusalReason::TooManyOpen);
    // The eight it did admit were still open when the turn finished. The turn
    // ended and the session did not, and each record says exactly that.
    let ended = audit.answered_questions.lock().unwrap().clone();
    assert_eq!(ended.len(), MAX_OPEN_QUESTIONS, "{ended:?}");
    for record in &ended {
        assert_eq!(
            record.response(),
            &QuestionResponse::Cancelled(QuestionCancellation::ExecutionFinished)
        );
        assert_eq!(record.actor(), None);
    }
    let _ = opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await;
}

/// The one refusal an ask can meet, decided and then written, both on record.
fn assert_refused(audit: &RecordingAudit, reason: QuestionRefusalReason) {
    let records = audit.refused_questions.lock().unwrap().clone();
    assert_eq!(records.len(), 2, "{records:?}");
    for record in &records {
        assert_eq!(record.reason(), reason);
        assert_eq!(record.execution_id().as_str(), "write");
    }
    assert_eq!(records[0].delivery(), &PermissionAnswerDelivery::Selected);
    assert_eq!(records[1].delivery(), &PermissionAnswerDelivery::Written);
    // One refused request, named alike on its decision and its write.
    assert_eq!(records[0].id(), records[1].id());
    // A refusal for room keeps the ask it could not fit and what was already
    // open; any other refusal happened before there was an ask to keep.
    for record in &records {
        match reason {
            QuestionRefusalReason::TooManyOpen | QuestionRefusalReason::TooLarge => {
                let refused = record.refused().expect("a refusal for room keeps its ask");
                assert!(!refused.ask().questions().is_empty());
                if reason == QuestionRefusalReason::TooManyOpen {
                    assert_eq!(refused.open_asks(), MAX_OPEN_QUESTIONS);
                } else {
                    assert!(
                        refused.open_cost() + refused.ask().carrying_cost() > MAX_OPEN_ASK_COST
                    );
                }
            }
            _ => assert!(record.refused().is_none()),
        }
    }
}

/// An ask nobody here can answer is refused on the record, and the turn goes on.
#[tokio::test]
async fn an_ask_that_cannot_be_put_to_anybody_is_refused_on_the_record() {
    let _process_slot = process_test_slot().await;
    for (mode, reason) in [
        ("ask-unsupported", QuestionRefusalReason::Unsupported),
        ("ask-unreadable", QuestionRefusalReason::UnreadableQuestion),
        // Valid, and more than a surface could show: admitted, nobody could
        // ever have answered it.
        ("ask-too-large", QuestionRefusalReason::TooLarge),
    ] {
        let audit = Arc::new(RecordingAudit::default());
        // Room for an ask that is valid on the wire and still too large to
        // show: its carrying cost runs well ahead of its frame.
        let (root, mut config, model) = test_acp_configuration(mode, 16);
        config.max_incoming_frame_bytes = 64 * 1024;
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
        let active = start(&opened, "write").await;
        assert!(matches!(
            next(&mut opened).await,
            ExecutionUpdate::Message(chunk) if chunk.as_str() == "done asking"
        ));
        assert_eq!(
            timeout(Duration::from_secs(3), active)
                .await
                .unwrap()
                .unwrap(),
            Ok(ExecutionOutcome::Completed),
            "{mode}"
        );
        let answers = provider_answers(&root);
        assert_eq!(answers.len(), 1, "{mode}: {answers:?}");
        assert_eq!(answers[0]["result"]["action"], "cancel");
        assert_refused(&audit, reason);
        // Refused, not asked: there is no ask to have answered or ended.
        assert!(audit.answered_questions.lock().unwrap().is_empty());
        let _ = opened
            .session
            .shutdown(SessionCloseRequest::Explicit(close_action()))
            .await;
    }
}

/// A refusal the audit will not take still reaches the agent, exactly once.
///
/// The agent is waiting, so the refusal is sent whether or not it could be
/// recorded; each refused record is then reported rather than swallowed, and
/// the request is not answered a second time by the dispatcher on the way out.
#[tokio::test]
async fn a_refused_ask_the_audit_will_not_take_is_answered_once_and_reported() {
    let _process_slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit {
        reject: true,
        ..RecordingAudit::default()
    });
    let (root, binding) = test_acp_binding_with_audit("ask-unsupported", 16, audit);
    let opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let active = start(&opened, "write").await;
    assert_eq!(
        timeout(Duration::from_secs(5), active)
            .await
            .unwrap()
            .unwrap(),
        // Each record the sink refused is its own fact: the refusal's decision
        // and its write, then the turn's own lifecycle records.
        Err(rejected_audits(4))
    );
    let answers = provider_answers(&root);
    assert_eq!(answers.len(), 1, "answered exactly once: {answers:?}");
    assert_eq!(answers[0]["id"], "u");
    assert_eq!(answers[0]["result"]["action"], "cancel");
    assert_eq!(
        opened
            .session
            .shutdown(SessionCloseRequest::Explicit(close_action()))
            .await
            .into_result(),
        Err(rejected_audits(4))
    );
    wait_until_gone(&root, "pid").await;
}

/// An ask arriving while the session is being stopped is refused on the record.
///
/// Nobody could answer it, so it is refused as one the session was ending for,
/// while the ask that was already open ends as the session's own — two
/// different facts, recorded as two.
#[tokio::test]
async fn an_ask_arriving_while_the_session_ends_is_refused_on_the_record() {
    let _process_slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit::default());
    let (root, binding) = test_acp_binding_with_audit("ask-after-cancel", 16, audit.clone());
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let active = start(&opened, "write").await;
    assert!(matches!(
        next(&mut opened).await,
        ExecutionUpdate::QuestionAsked { .. }
    ));
    let _ = opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await;
    let _ = timeout(Duration::from_secs(5), active).await.unwrap();
    wait_until_gone(&root, "pid").await;

    let refused = audit.refused_questions.lock().unwrap().clone();
    assert_eq!(refused.len(), 2, "{refused:?}");
    for record in &refused {
        assert_eq!(record.reason(), QuestionRefusalReason::SessionEnding);
    }
    assert_eq!(refused[0].delivery(), &PermissionAnswerDelivery::Selected);
    let ended = audit.answered_questions.lock().unwrap().clone();
    assert_eq!(ended.len(), 1, "{ended:?}");
    assert_eq!(
        ended[0].response(),
        &QuestionResponse::Cancelled(QuestionCancellation::SessionEnded)
    );
    let mut ids: Vec<_> = provider_answers(&root)
        .into_iter()
        .map(|answer| answer["id"].as_str().unwrap().to_owned())
        .collect();
    ids.sort();
    assert_eq!(ids, ["first", "late"], "each request answered once");
}

/// A second ask under an identity still open is a protocol fault, not a refusal.
///
/// Answering it would answer the first, which is still being offered. So the
/// session fails as it does for a review's duplicate, and its teardown ends the
/// first ask with its evidence — told once, closed, and recorded.
#[tokio::test]
async fn an_ask_reusing_an_open_asks_identity_fails_the_session_and_ends_the_first() {
    let _process_slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit::default());
    let (root, binding) = test_acp_binding_with_audit("asks-duplicate", 16, audit.clone());
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let active = start(&opened, "write").await;
    let ExecutionUpdate::QuestionAsked { id, .. } = next(&mut opened).await else {
        panic!("expected the first ask");
    };
    let ExecutionUpdate::QuestionClosed { id: closed } = next(&mut opened).await else {
        panic!("the first ask closes when the session ends");
    };
    assert_eq!(closed, id);
    assert!(timeout(Duration::from_secs(3), active)
        .await
        .unwrap()
        .unwrap()
        .is_err());
    let answers = provider_answers(&root);
    assert_eq!(answers.len(), 1, "one request, answered once: {answers:?}");
    assert_eq!(answers[0]["id"], "d");
    assert_eq!(answers[0]["result"]["action"], "cancel");
    let ended = audit.answered_questions.lock().unwrap().clone();
    assert_eq!(ended.len(), 1, "{ended:?}");
    assert_eq!(
        ended[0].response(),
        &QuestionResponse::Cancelled(QuestionCancellation::SessionEnded)
    );
    assert!(audit.refused_questions.lock().unwrap().is_empty());
    let _ = opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await;
}
