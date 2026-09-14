//! Pure entity transition tests for internal aggregate and restoration behavior.
use crate::domain::agent_execution::{
    executions::ExecutionId, permissions::*, tools::ToolCallId, ExecutionError,
};

fn choice(id: &str, decision: PermissionDecision) -> PermissionOption {
    PermissionOption::new(
        PermissionOptionId::new(id).unwrap(),
        format!("Choice {id}"),
        decision,
    )
    .unwrap()
}
fn request() -> PermissionRequest {
    PermissionRequest::new(
        PermissionId::new("permission").unwrap(),
        ExecutionId::new("execution").unwrap(),
        ToolCallId::new("tool").unwrap(),
        PermissionOptions::new(
            vec![
                choice(
                    "allow",
                    PermissionDecision::new(PermissionEffect::Allow, PermissionScope::request()),
                ),
                choice(
                    "reject",
                    PermissionDecision::new(PermissionEffect::Deny, PermissionScope::request()),
                ),
            ],
            &PermissionOfferPolicy::once_only(),
        )
        .unwrap(),
    )
}

#[test]
fn permissions_are_scoped_and_resolved_once_without_consuming_invalid_answers() {
    for (id, decision) in [
        (
            "allow",
            PermissionDecision::new(PermissionEffect::Allow, PermissionScope::request()),
        ),
        (
            "reject",
            PermissionDecision::new(PermissionEffect::Deny, PermissionScope::request()),
        ),
    ] {
        let mut permission = request();
        assert_eq!(permission.id().as_str(), "permission");
        assert_eq!(permission.execution_id().as_str(), "execution");
        assert_eq!(permission.tool_id().as_str(), "tool");
        let option_id = PermissionOptionId::new(id).unwrap();
        assert_eq!(permission.options().choices().len(), 2);
        assert_eq!(permission.state(), PermissionStateView::Pending);
        assert_eq!(
            permission.answer(&ExecutionId::new("other").unwrap(), &option_id),
            Err(ExecutionError::DifferentExecution)
        );
        let execution = permission.execution_id().clone();
        assert_eq!(
            permission.answer(&execution, &PermissionOptionId::new("unknown").unwrap()),
            Err(ExecutionError::UnknownPermissionOption)
        );
        assert_eq!(permission.state(), PermissionStateView::Pending);
        let resolved = permission.answer(&execution, &option_id).unwrap();
        assert_eq!(resolved, decision);
        assert_eq!(
            resolved.effect(),
            if id == "allow" {
                PermissionEffect::Allow
            } else {
                PermissionEffect::Deny
            }
        );
        assert_eq!(resolved.scope(), &PermissionScope::request());
        let expected = PermissionStateView::Answered {
            option_id: &option_id,
            decision: &decision,
        };
        assert_eq!(permission.state(), expected);
        assert_eq!(
            permission.answer(&execution, &option_id),
            Err(ExecutionError::PermissionResolved)
        );
        assert_eq!(
            permission.cancel(PermissionCancellationReason::provider_withdrawal()),
            Err(ExecutionError::PermissionResolved)
        );
        assert_eq!(permission.state(), expected);
    }
    let mut permission = request();
    permission
        .cancel(PermissionCancellationReason::provider_withdrawal())
        .unwrap();
    assert_eq!(
        permission.state(),
        PermissionStateView::Cancelled {
            reason: &PermissionCancellationReason::provider_withdrawal()
        }
    );
    assert_eq!(
        permission.cancel(PermissionCancellationReason::provider_withdrawal()),
        Err(ExecutionError::PermissionResolved)
    );
    assert_eq!(
        permission.answer(
            &ExecutionId::new("execution").unwrap(),
            &PermissionOptionId::new("allow").unwrap()
        ),
        Err(ExecutionError::PermissionResolved)
    );
}

#[test]
fn permission_cancellation_retains_the_first_cause_for_every_runtime_outcome() {
    for reason in [
        PermissionCancellationReason::provider_withdrawal(),
        PermissionCancellationReason::session_closed(),
        PermissionCancellationReason::execution_finished(),
        PermissionCancellationReason::execution_failed(),
        PermissionCancellationReason::deadline_exceeded(),
        PermissionCancellationReason::event_consumer_dropped(),
        PermissionCancellationReason::session_handles_dropped(),
        PermissionCancellationReason::custom(
            CustomPermissionCancellationReason::new(
                "guard.cost_limit",
                "Estimated cost exceeds the configured guard limit.",
            )
            .unwrap(),
        ),
    ] {
        let mut permission = request();
        permission.cancel(reason.clone()).unwrap();
        let expected = PermissionStateView::Cancelled { reason: &reason };
        assert_eq!(permission.state(), expected);
        for later_reason in [
            PermissionCancellationReason::execution_failed(),
            PermissionCancellationReason::session_closed(),
        ] {
            assert_eq!(
                permission.cancel(later_reason),
                Err(ExecutionError::PermissionResolved)
            );
            assert_eq!(permission.state(), expected);
        }
    }
}
