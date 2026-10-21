//! A stopping decline names the rule that stopped it, and a continue decline names none.
use nessa_sdk::application::agent_execution::permissions::{
    ActionContext, ApprovalRuleReference, CancellationOrigin, DeclinedReview,
    PermissionAnswerDelivery, ReviewDeclineRecord,
};
use nessa_sdk::application::agent_execution::{
    agents::AgentError, executions::ExecutionController,
};
use nessa_sdk::domain::agent_execution::{
    executions::{ExecutionId, ExecutionOutcome},
    permissions::{
        PermissionCancellationReason, RefusalOutcome, ReviewDecline, ReviewDeclineId,
        ReviewDeclineReason, ReviewDeclineStage,
    },
    sessions::ExecutionSessionId,
};

fn rule() -> ApprovalRuleReference {
    ApprovalRuleReference::new(
        "refuse-shell",
        "1",
        ActionContext::new("host", "policy", "grant").unwrap(),
    )
    .unwrap()
}

fn record(
    outcome: RefusalOutcome,
    rule: Option<ApprovalRuleReference>,
) -> Result<ReviewDeclineRecord, AgentError> {
    let mut controller = ExecutionController::new(ExecutionSessionId::new("session").unwrap());
    let execution = ExecutionId::new("run").unwrap();
    controller.begin_execution(execution.clone()).unwrap();
    controller.decline_review(
        &execution,
        ReviewDeclineId::new("1").unwrap(),
        ReviewDecline::new(
            Some("Bash"),
            ReviewDeclineReason::ToolNotReviewable,
            outcome,
        ),
        rule,
    )
}

#[test]
fn a_decline_record_rejects_an_outcome_that_disagrees_with_its_rule() {
    assert!(record(RefusalOutcome::Continue, None).is_ok());
    assert!(record(RefusalOutcome::EndTurn, Some(rule())).is_ok());
    assert!(record(RefusalOutcome::CloseSession, Some(rule())).is_ok());
    for (outcome, rule) in [
        (RefusalOutcome::Continue, Some(rule())),
        (RefusalOutcome::EndTurn, None),
        (RefusalOutcome::CloseSession, None),
    ] {
        assert!(matches!(
            record(outcome, rule),
            Err(AgentError::InvalidInput(_))
        ));
    }
}

#[test]
fn a_declined_review_event_uses_the_same_rule_pairing() {
    let decline = |outcome| {
        ReviewDecline::new(
            Some("Bash"),
            ReviewDeclineReason::ToolNotReviewable,
            outcome,
        )
    };
    let id = ReviewDeclineId::new("1").unwrap();
    assert!(DeclinedReview::selected(id.clone(), decline(RefusalOutcome::Continue), None).is_ok());
    let stopped =
        DeclinedReview::selected(id.clone(), decline(RefusalOutcome::EndTurn), Some(rule()))
            .unwrap();
    assert_eq!(stopped.rule().unwrap().rule_id(), "refuse-shell");
    assert_eq!(stopped.rule().unwrap().revision(), "1");
    let advanced = stopped.advance(ReviewDeclineStage::WriteConfirmed).unwrap();
    assert_eq!(advanced.rule().unwrap().granted_by().principal_id(), "host");
    assert_eq!(advanced.decline(), stopped.decline());
    assert!(
        DeclinedReview::selected(id.clone(), decline(RefusalOutcome::Continue), Some(rule()))
            .is_err()
    );
    assert!(DeclinedReview::selected(id.clone(), decline(RefusalOutcome::EndTurn), None).is_err());
    assert!(DeclinedReview::selected(id, decline(RefusalOutcome::CloseSession), None).is_err());
}

#[test]
fn refusal_requires_the_owning_controllers_active_execution() {
    let mut first = ExecutionController::new(ExecutionSessionId::new("first").unwrap());
    let mut second = ExecutionController::new(ExecutionSessionId::new("second").unwrap());
    let a = ExecutionId::new("a").unwrap();
    let b = ExecutionId::new("b").unwrap();
    let decline = ReviewDecline::new(
        Some("Bash"),
        ReviewDeclineReason::ToolNotReviewable,
        RefusalOutcome::Continue,
    );
    let id = ReviewDeclineId::new("1").unwrap();
    assert!(matches!(
        first.decline_review(&a, id.clone(), decline.clone(), None),
        Err(AgentError::InvalidInput(_))
    ));
    first.begin_execution(a.clone()).unwrap();
    second.begin_execution(b.clone()).unwrap();
    assert!(matches!(
        first.decline_review(&b, id.clone(), decline.clone(), None),
        Err(AgentError::InvalidInput(_))
    ));
    assert_eq!(first.active_execution_id(), Some(&a));
    assert_eq!(second.active_execution_id(), Some(&b));
    for (controller, target) in [(&first, &a), (&second, &b)] {
        let selected = controller
            .decline_review(target, id.clone(), decline.clone(), None)
            .unwrap();
        assert_eq!(selected.session_id(), controller.id());
        assert_eq!(selected.execution_id(), target);
        assert_eq!(selected.delivery(), &PermissionAnswerDelivery::Selected);
    }
    first
        .close(
            PermissionCancellationReason::session_closed(),
            CancellationOrigin::Runtime,
        )
        .unwrap();
    assert_eq!(
        first.decline_review(&a, id, decline, None),
        Err(AgentError::Closed)
    );
}

#[test]
fn refusal_delivery_keeps_the_selected_identity_and_rule_after_turn_replacement() {
    for outcome in [
        RefusalOutcome::Continue,
        RefusalOutcome::EndTurn,
        RefusalOutcome::CloseSession,
    ] {
        let mut controller = ExecutionController::new(ExecutionSessionId::new("original").unwrap());
        let execution = ExecutionId::new("original-turn").unwrap();
        controller.begin_execution(execution.clone()).unwrap();
        let decline = ReviewDecline::new(
            Some("Bash"),
            ReviewDeclineReason::ToolNotReviewable,
            outcome,
        );
        let selected = controller
            .decline_review(
                &execution,
                ReviewDeclineId::new("1").unwrap(),
                decline,
                (outcome != RefusalOutcome::Continue).then(rule),
            )
            .unwrap();
        controller
            .finish_execution(&execution, Ok(ExecutionOutcome::Cancelled))
            .unwrap();
        controller
            .begin_execution(ExecutionId::new("replacement-turn").unwrap())
            .unwrap();
        assert!(matches!(
            controller.decline_review(
                &execution,
                selected.id().clone(),
                selected.decline().clone(),
                selected.rule().cloned()
            ),
            Err(AgentError::InvalidInput(_))
        ));
        for delivery in [
            PermissionAnswerDelivery::Written,
            PermissionAnswerDelivery::Failed(AgentError::Closed),
        ] {
            let delivered = selected.clone().with_delivery(delivery.clone());
            assert_eq!(delivered.session_id(), selected.session_id());
            assert_eq!(delivered.execution_id(), &execution);
            assert_eq!(delivered.id(), selected.id());
            assert_eq!(delivered.decline(), selected.decline());
            assert_eq!(delivered.rule(), selected.rule());
            assert_eq!(delivered.delivery(), &delivery);
        }
    }
}
