//! Physical cleanup completion cannot manufacture acknowledgement of failed audit.
use super::*;

#[tokio::test]
async fn confirmed_cleanup_audit_failure_survives_repeated_explicit_close() {
    for report_from_control in [false, true] {
        let (agent, backend, storage) = probe(false).await;
        let report = cleaned_with_error(AgentError::AuditFailure);
        if report_from_control {
            *backend.control_attachment.lock().unwrap() =
                ProviderSessionState::CleanupReported(report);
            assert_eq!(
                invoke_control(&agent, ProviderControl::Answer).await,
                Err(AgentError::StalePermission)
            );
        } else {
            *backend.cleanup_report.lock().unwrap() = Some(report);
        }
        for _ in 0..3 {
            assert_eq!(agent.close(actor()).await, Err(AgentError::AuditFailure));
            assert_eq!(
                agent.invoke(input("blocked"), actor()).await,
                Err(AgentError::Closed)
            );
            assert!(matches!(
                agent.enqueue(input("queued"), actor()).await,
                Err(AgentError::Closed)
            ));
            assert_eq!(backend.executions.load(Ordering::SeqCst), 0);
            assert_eq!(
                backend.closes.load(Ordering::SeqCst),
                usize::from(!report_from_control)
            );
            // Even a now-successful backend cannot acknowledge the prior missing audit.
            *backend.cleanup_report.lock().unwrap() =
                Some(CleanupReport::confirmed(CloseOutcome { forced: false }));
        }
        if !report_from_control {
            assert_eq!(
                *backend.close_requests.lock().unwrap(),
                vec![SessionCloseRequest::Explicit(actor())]
            );
        }
        drop(agent);
        assert!(
            !storage.0.lock().unwrap().leased,
            "audit-only failure owns no physical cleanup lease"
        );
        tokio::task::yield_now().await;
        assert_eq!(
            backend.closes.load(Ordering::SeqCst),
            usize::from(!report_from_control)
        );
    }
}
