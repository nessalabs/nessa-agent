mod correlation;
mod execution_targets;
mod retained_review;
use super::{
    providers::{provider_agent, provider_agent_with_review},
    support::*,
};
use nessa_sdk::application::agent_execution::permissions::{
    PermissionAnswerDelivery, PermissionAnswerRecord,
};

#[test]
fn attribution_requires_actor_and_immutable_decision_evidence() {
    for (principal, surface, request) in [("", "cli", "r"), ("p", " ", "r"), ("p", "cli", "")] {
        assert!(ActionContext::new(principal, surface, request).is_err());
    }
    let human = ActionContext::new("human", "nessa.panel", "grant-request").unwrap();
    assert!(ApprovalModeSnapshot::new("", "1").is_err());
    assert!(ApprovalModeSnapshot::new("review", " ").is_err());
    assert!(ApprovalRuleReference::new("", "1", human.clone()).is_err());
    assert!(ApprovalRuleReference::new("rule", "", human.clone()).is_err());
    let actor = ActionContext::new("server", "nessa.server", "answer-request").unwrap();
    let rule = ApprovalRuleReference::new("rule", "revision-2", human.clone()).unwrap();
    let resolution = pending_controller()
        .answer_permission(PermissionAnswer {
            execution_id: ExecutionId::new("execution").unwrap(),
            id: PermissionId::new("permission").unwrap(),
            option_id: PermissionOptionId::new("allow").unwrap(),
            attribution: ApprovalAttribution::new(actor.clone(), ApprovalBasis::Rule(rule.clone())),
        })
        .unwrap();
    assert_eq!(resolution.attribution().actor(), &actor);
    assert_eq!(rule.granted_by(), &human);
    assert_ne!(rule.granted_by(), resolution.attribution().actor());
    assert_eq!(rule.revision(), "revision-2");
    assert_eq!(resolution.attribution().basis(), &ApprovalBasis::Rule(rule));
}

#[tokio::test]
async fn substituted_binding_preserves_each_actors_explicit_mode_or_rule_basis() {
    let human = ActionContext::new("human", "nessa.panel", "grant-request").unwrap();
    let bases = [
        ApprovalBasis::Explicit,
        ApprovalBasis::Mode(ApprovalModeSnapshot::new("review-file-writes", "config-3").unwrap()),
        ApprovalBasis::Rule(ApprovalRuleReference::new("rule", "revision-2", human).unwrap()),
    ];
    for (index, basis) in bases.into_iter().enumerate() {
        let agent = reviewed_agent(Arc::new(InMemoryPermissionBackend {
            execution: Mutex::new(pending_controller()),
            cancellations: Mutex::new(Vec::new()),
            closures: Mutex::new(Vec::new()),
        }))
        .await;
        let attribution = ApprovalAttribution::new(
            ActionContext::new(
                format!("actor-{index}"),
                "nessa.cli",
                format!("answer-{index}"),
            )
            .unwrap(),
            basis,
        );
        let answer = PermissionAnswer {
            execution_id: ExecutionId::new("execution").unwrap(),
            id: PermissionId::new("permission").unwrap(),
            option_id: PermissionOptionId::new("allow").unwrap(),
            attribution: attribution.clone(),
        };
        let resolution = agent.answer_permission(answer.clone()).await.unwrap();
        assert_eq!(resolution.attribution(), &attribution);
        assert_eq!(resolution.input(), &review_input());
        assert_eq!(resolution.request().tool_id().as_str(), "tool");
        let failure = agent.answer_permission(answer).await.unwrap_err();
        assert_eq!(failure.error(), &AgentError::StalePermission);
        assert_eq!(failure.selection(), PermissionSelectionState::Pending);
    }
}

#[test]
fn cancellation_preserves_original_review_and_explicit_close_attribution() {
    let mut controller = pending_controller();
    // A newer tool observation cannot change what was originally reviewed.
    controller
        .tool_event(
            &ExecutionId::new("execution").unwrap(),
            ToolCallUpdate::new(
                ToolCallId::new("tool").unwrap(),
                Some("updated title".into()),
                None,
                None,
                None,
                None,
            ),
        )
        .unwrap();
    let action = close_action();
    let records = controller
        .close(
            PermissionCancellationReason::session_closed(),
            CancellationOrigin::Client(action.clone()),
        )
        .unwrap();
    assert_eq!(records.len(), 2);
    assert!(matches!(
        &records[0],
        ExecutionAuditRecord::SessionClosed(_)
    ));
    let ExecutionAuditRecord::Cancelled(record) = &records[1] else {
        panic!("expected cancellation")
    };
    assert_eq!(record.session_id().as_str(), "fixture");
    assert_eq!(record.request().id(), pending_permission().id());
    assert_eq!(
        record.request().execution_id(),
        pending_permission().execution_id()
    );
    assert_eq!(record.request().tool_id(), pending_permission().tool_id());
    assert_eq!(
        record.request().state(),
        PermissionStateView::Cancelled {
            reason: &PermissionCancellationReason::session_closed()
        }
    );
    assert_eq!(record.input(), &review_input());
    assert_eq!(record.origin(), &CancellationOrigin::Client(action));
    assert!(controller
        .close_execution(
            &ExecutionId::new("execution").unwrap(),
            PermissionCancellationReason::execution_failed(),
            CancellationOrigin::Runtime
        )
        .unwrap()
        .is_empty());
    assert!(matches!(
        controller
            .finish_execution(
                &ExecutionId::new("execution").unwrap(),
                Err(PermissionCancellationReason::session_closed())
            )
            .unwrap()
            .as_slice(),
        [ExecutionAuditRecord::Finished(_)]
    ));
}

#[test]
fn explicit_cancellation_rejects_automatic_causes_without_resolving_the_review() {
    for reason in [
        PermissionCancellationReason::provider_withdrawal(),
        PermissionCancellationReason::session_closed(),
        PermissionCancellationReason::execution_finished(),
        PermissionCancellationReason::execution_failed(),
        PermissionCancellationReason::deadline_exceeded(),
        PermissionCancellationReason::event_consumer_dropped(),
        PermissionCancellationReason::session_handles_dropped(),
    ] {
        let mut controller = pending_controller();
        assert!(matches!(
            controller.cancel_review(PermissionCancellationRequest {
                execution_id: ExecutionId::new("execution").unwrap(),
                id: PermissionId::new("permission").unwrap(),
                reason,
                actor: close_action(),
            }),
            Err(AgentError::InvalidInput(_))
        ));
        let resolution = controller
            .answer_permission(PermissionAnswer {
                execution_id: ExecutionId::new("execution").unwrap(),
                id: PermissionId::new("permission").unwrap(),
                option_id: PermissionOptionId::new("allow").unwrap(),
                attribution: ApprovalAttribution::new(close_action(), ApprovalBasis::Explicit),
            })
            .unwrap();
        assert_eq!(resolution.input(), &review_input());
    }
}

#[test]
fn closed_session_rejects_text_and_thought_but_preserves_terminal_projection() {
    let mut controller = ExecutionController::new(ExecutionSessionId::new("context").unwrap());
    controller
        .begin_execution(ExecutionId::new("execution").unwrap())
        .unwrap();
    let records = controller
        .close(
            PermissionCancellationReason::session_closed(),
            CancellationOrigin::Runtime,
        )
        .unwrap();
    assert_eq!(records.len(), 1);
    for message in [MessageChunk::text("late"), MessageChunk::thought("late")] {
        assert_eq!(
            controller.message_event(&ExecutionId::new("execution").unwrap(), message),
            Err(AgentError::Closed)
        );
    }
    assert!(matches!(
        controller
            .finished_event(
                &ExecutionId::new("execution").unwrap(),
                ExecutionOutcome::Cancelled
            )
            .unwrap()
            .update(),
        ExecutionUpdate::Finished(ExecutionOutcome::Cancelled)
    ));
}

#[test]
fn attribution_identifiers_and_rule_revisions_enforce_utf8_byte_bounds() {
    let exact = "é".repeat(ActionContext::MAX_IDENTITY_BYTES / 2);
    let oversized = format!("{exact}x");
    assert_eq!(exact.len(), ActionContext::MAX_IDENTITY_BYTES);
    for field in 0..3 {
        let mut values = ["principal", "surface", "request"];
        values[field] = &exact;
        let accepted = ActionContext::new(values[0], values[1], values[2]).unwrap();
        assert_eq!(
            [
                accepted.principal_id(),
                accepted.surface_id(),
                accepted.request_id()
            ][field],
            exact
        );
        values[field] = &oversized;
        assert!(matches!(
            ActionContext::new(values[0], values[1], values[2]),
            Err(AgentError::InvalidInput(_))
        ));
    }
    let actor = ActionContext::new("principal", "surface", "request").unwrap();
    assert!(ApprovalModeSnapshot::new(&exact, &exact).is_ok());
    assert!(ApprovalRuleReference::new(&exact, &exact, actor.clone()).is_ok());
    for field in 0..2 {
        let mut values = [exact.as_str(), exact.as_str()];
        values[field] = &oversized;
        assert!(matches!(
            ApprovalModeSnapshot::new(values[0], values[1]),
            Err(AgentError::InvalidInput(_))
        ));
        assert!(matches!(
            ApprovalRuleReference::new(values[0], values[1], actor.clone()),
            Err(AgentError::InvalidInput(_))
        ));
    }
    // Oversized caller capacity is discarded by compact immutable ownership.
    let mut reserved = String::with_capacity(1_000_000);
    reserved.push_str("principal");
    let compact = ActionContext::new(reserved, "surface", "request").unwrap();
    assert_eq!(compact.clone(), actor);
}

#[test]
fn idle_attachment_failure_emits_session_cause_without_inventing_execution() {
    let mut controller = ExecutionController::new(ExecutionSessionId::new("idle").unwrap());
    assert!(matches!(
        controller.close(
            PermissionCancellationReason::execution_failed(),
            CancellationOrigin::Runtime
        ),
        Err(AgentError::InvalidInput(_))
    ));
    assert!(matches!(
        controller.close(
            PermissionCancellationReason::session_failed(),
            CancellationOrigin::Client(ActionContext::new("caller", "host", "close").unwrap())
        ),
        Err(AgentError::InvalidInput(_))
    ));
    let records = controller
        .close(
            PermissionCancellationReason::session_failed(),
            CancellationOrigin::Runtime,
        )
        .unwrap();
    assert_eq!(records.len(), 1);
    let ExecutionAuditRecord::SessionClosed(record) = &records[0] else {
        panic!("expected session closure");
    };
    assert_eq!(
        record.closure().reason(),
        &PermissionCancellationReason::session_failed()
    );
    assert_eq!(record.closure().execution_id(), None);
    assert_eq!(record.origin(), &CancellationOrigin::Runtime);
    assert!(controller
        .close(
            PermissionCancellationReason::execution_failed(),
            CancellationOrigin::Runtime
        )
        .is_err());
}

#[test]
fn answer_delivery_retains_controller_session_despite_identical_local_identities() {
    let request = pending_permission();
    let mut resolutions = Vec::new();
    for session in ["session-a", "session-b"] {
        let mut controller = ExecutionController::new(ExecutionSessionId::new(session).unwrap());
        controller
            .begin_execution(request.execution_id().clone())
            .unwrap();
        let tool = ToolCallUpdate::new(request.tool_id().clone(), None, None, None, None, None);
        controller
            .request_permission(
                &ExecutionId::new("execution").unwrap(),
                request.id().clone(),
                tool,
                review_input(),
                request.options().clone(),
            )
            .unwrap();
        let resolution = controller
            .answer_permission(PermissionAnswer {
                execution_id: request.execution_id().clone(),
                id: request.id().clone(),
                option_id: PermissionOptionId::new("allow").unwrap(),
                attribution: attribution(),
            })
            .unwrap();
        assert_eq!(resolution.session_id(), controller.id());
        // A later lifecycle boundary must not relabel already-produced evidence.
        controller
            .finish_execution(
                &ExecutionId::new("execution").unwrap(),
                Ok(ExecutionOutcome::Completed),
            )
            .unwrap();
        for delivery in [
            PermissionAnswerDelivery::Selected,
            PermissionAnswerDelivery::Written,
            PermissionAnswerDelivery::Failed(AgentError::Deadline),
        ] {
            let record = PermissionAnswerRecord::new(resolution.clone(), delivery.clone());
            assert_eq!(record.session_id().as_str(), session);
            assert_eq!(record.resolution(), &resolution);
            assert_eq!(record.resolution().input(), &review_input());
            assert_eq!(record.resolution().attribution(), &attribution());
            assert_eq!(record.delivery(), &delivery);
        }
        resolutions.push(resolution);
    }
    // Everything within each session may have the same identity; session ownership
    // still distinguishes the resolutions and all their cloned audit records.
    assert_eq!(resolutions[0].request(), resolutions[1].request());
    assert_eq!(resolutions[0].input(), resolutions[1].input());
    assert_eq!(resolutions[0].attribution(), resolutions[1].attribution());
    assert_ne!(resolutions[0], resolutions[1]);
}

#[tokio::test]
async fn substituted_backend_cannot_return_a_foreign_sessions_permission_as_success() {
    let request = pending_permission();
    let mut controller = ExecutionController::new(ExecutionSessionId::new("foreign").unwrap());
    controller
        .begin_execution(request.execution_id().clone())
        .unwrap();
    controller
        .request_permission(
            &ExecutionId::new("execution").unwrap(),
            request.id().clone(),
            ToolCallUpdate::new(request.tool_id().clone(), None, None, None, None, None),
            review_input(),
            request.options().clone(),
        )
        .unwrap();
    let backend = Arc::new(InMemoryPermissionBackend {
        execution: Mutex::new(controller),
        cancellations: Mutex::new(Vec::new()),
        closures: Mutex::new(Vec::new()),
    });
    // provider_agent exposes the "fixture" session; all nested request identities
    // and the supplied actor are nevertheless otherwise correct for the foreign review.
    let agent = reviewed_agent(backend.clone()).await;
    let failure = agent
        .answer_permission(PermissionAnswer {
            execution_id: request.execution_id().clone(),
            id: request.id().clone(),
            option_id: PermissionOptionId::new("allow").unwrap(),
            attribution: attribution(),
        })
        .await
        .unwrap_err();
    assert!(matches!(failure.error(), AgentError::Protocol(_)));
    assert_eq!(failure.selection(), PermissionSelectionState::Consumed);
    // Validation is of returned evidence, not a claim that the adapter effect was undone.
    assert!(matches!(
        backend
            .execution
            .lock()
            .unwrap()
            .answer_permission(PermissionAnswer {
                execution_id: request.execution_id().clone(),
                id: request.id().clone(),
                option_id: PermissionOptionId::new("allow").unwrap(),
                attribution: attribution(),
            }),
        Err(AgentError::StalePermission)
    ));
}

fn retained_review() -> ExecutionEvent {
    let request = pending_permission();
    ExecutionEvent::new(
        request.execution_id().clone(),
        ExecutionUpdate::PermissionRequested {
            id: request.id().clone(),
            tool_id: request.tool_id().clone(),
            observation: ToolObservation::default(),
            input: review_input(),
            options: request.options().clone(),
        },
    )
}
async fn reviewed_agent(backend: Arc<dyn ProviderSessionBackend>) -> Agent {
    provider_agent_with_review(backend, retained_review()).await
}
