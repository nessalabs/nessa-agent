//! A stopping decline names the rule that stopped it, and a continue decline names none.
use nessa_sdk::application::agent_execution::permissions::{
    ActionContext, ApprovalRuleReference, PermissionAnswerDelivery, ReviewDeclineRecord,
};
use nessa_sdk::domain::agent_execution::{
    executions::ExecutionId,
    permissions::{RefusalOutcome, ReviewDecline, ReviewDeclineId, ReviewDeclineReason},
    sessions::ExecutionSessionId,
    ExecutionError,
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
) -> Result<ReviewDeclineRecord, ExecutionError> {
    ReviewDeclineRecord::new(
        ExecutionSessionId::new("session").unwrap(),
        ExecutionId::new("run").unwrap(),
        ReviewDeclineId::new("1").unwrap(),
        ReviewDecline::new(
            Some("Bash"),
            ReviewDeclineReason::ToolNotReviewable,
            outcome,
        ),
        PermissionAnswerDelivery::Selected,
        rule,
    )
}

#[test]
fn a_decline_record_rejects_an_outcome_that_disagrees_with_its_rule() {
    assert!(record(RefusalOutcome::Continue, None).is_ok());
    assert!(record(RefusalOutcome::EndTurn, Some(rule())).is_ok());
    assert!(record(RefusalOutcome::CloseSession, Some(rule())).is_ok());
    assert_eq!(
        record(RefusalOutcome::Continue, Some(rule())).unwrap_err(),
        ExecutionError::InvalidReviewDecline
    );
    assert_eq!(
        record(RefusalOutcome::EndTurn, None).unwrap_err(),
        ExecutionError::InvalidReviewDecline
    );
    assert_eq!(
        record(RefusalOutcome::CloseSession, None).unwrap_err(),
        ExecutionError::InvalidReviewDecline
    );
}
