//! Internal evidence construction must preserve the same cause/correlation rules.
use super::*;

#[test]
fn closure_constructor_rejects_permission_causes_and_uncorrelated_execution_failure() {
    for reason in [
        PermissionCancellationReason::provider_withdrawal(),
        PermissionCancellationReason::execution_finished(),
        PermissionCancellationReason::execution_failed(),
    ] {
        assert_eq!(
            SessionClosure::new(ExecutionSessionId::new("context").unwrap(), None, reason),
            Err(ExecutionError::InvalidSessionClosureReason),
        );
    }
}

#[test]
fn closure_constructor_retains_valid_session_and_execution_failure_evidence() {
    let session = ExecutionSessionId::new("context").unwrap();
    for (execution, reason) in [
        (None, PermissionCancellationReason::session_failed()),
        (
            Some(ExecutionId::new("run").unwrap()),
            PermissionCancellationReason::execution_failed(),
        ),
    ] {
        let closure =
            SessionClosure::new(session.clone(), execution.clone(), reason.clone()).unwrap();
        assert_eq!(closure.session_id(), &session);
        assert_eq!(closure.execution_id(), execution.as_ref());
        assert_eq!(closure.reason(), &reason);
    }
}
