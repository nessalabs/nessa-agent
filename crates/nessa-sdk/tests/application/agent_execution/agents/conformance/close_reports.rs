//! Public close/suspend preserve the same transaction's resource and evidence facts.
use super::*;

#[path = "close_reports/owned_root.rs"]
mod owned_root;

#[tokio::test]
async fn close_reports_preserve_actual_resources_and_independent_failures_in_both_modes() {
    for suspend in [false, true] {
        for forced in [false, true] {
            for resources in [
                ResourceCleanup::Confirmed(CloseOutcome { forced }),
                ResourceCleanup::Unconfirmed(AgentError::AuditFailure),
                ResourceCleanup::ReleasePending {
                    physical: CloseOutcome { forced },
                    failure: AgentError::AuditFailure,
                },
            ] {
                let (agent, backend, _) = workflow().await;
                let expected = CleanupReport::new(resources, Err(AgentError::AuditFailure))
                    .with_operation_failure(Some(AgentError::Deadline))
                    .with_completion_failure(Some(AgentError::CleanupUncertain));
                *backend.cleanup.lock().unwrap() = expected.clone();
                let reported = if suspend {
                    bounded(agent.suspend_report(close_action())).await.unwrap()
                } else {
                    bounded(agent.close_report(close_action())).await.unwrap()
                };
                assert_eq!(reported.cleanup(), &expected);
                assert_eq!(reported.scheduling(), &Ok(()));
                assert_eq!(reported.into_result(), expected.into_result());
                assert_eq!(backend.shutdowns.lock().unwrap().len(), 1);
                *backend.cleanup.lock().unwrap() =
                    CleanupReport::confirmed(CloseOutcome { forced });
                let _ = bounded(agent.close_report(close_action())).await.unwrap();
            }
        }
    }
}

#[tokio::test]
async fn report_and_result_callers_join_the_same_confirmed_close_owner() {
    for suspend in [false, true] {
        let (agent, backend, _) = workflow().await;
        let reported = if suspend {
            let (report, result) = tokio::join!(
                agent.suspend_report(close_action()),
                agent.suspend(close_action())
            );
            assert_eq!(result, Ok(CloseOutcome { forced: false }));
            report.unwrap()
        } else {
            let (report, result) = tokio::join!(
                agent.close_report(close_action()),
                agent.close(close_action())
            );
            assert_eq!(result, Ok(CloseOutcome { forced: false }));
            report.unwrap()
        };
        assert!(reported.cleanup().is_confirmed());
        assert_eq!(reported.scheduling(), &Ok(()));
        assert_eq!(backend.shutdowns.lock().unwrap().len(), 1);
    }
}
