//! Restoration and cleanup must operate on the same attached resource generation.
use super::*;
use crate::application::agent_execution::permissions::QuestionAnswer;
use crate::application::agent_execution::providers::ResourceCleanup;
use std::{
    future::{poll_fn, Future},
    task::Poll,
};

#[tokio::test]
async fn close_during_restore_cleans_the_rearmed_attachment() {
    for report in [
        CleanupReport::confirmed(CloseOutcome { forced: false }),
        CleanupReport::unconfirmed(AgentError::CleanupUncertain),
        CleanupReport::new(
            ResourceCleanup::Confirmed(CloseOutcome { forced: false }),
            Err(AgentError::AuditFailure),
        ),
        CleanupReport::new(
            ResourceCleanup::Unconfirmed(AgentError::CleanupUncertain),
            Err(AgentError::AuditFailure),
        ),
    ] {
        let (agent, backend) = agent_with_backend().await;
        agent.close(actor()).await.unwrap();
        assert!(!agent.inner.lifecycle.attachment_needs_cleanup());
        *backend.cleanup_report.lock().unwrap() = Some(report.clone());
        let authorization = agent
            .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
            .unwrap();
        let (entered, reached) = oneshot::channel();
        let (release, waiting) = oneshot::channel();
        agent
            .inner
            .lifecycle
            .pause_next_preparation(entered, waiting);
        let attaching = agent.start_attachment(authorization).unwrap();
        reached.await.unwrap();
        let closer = ActionContext::new("owner", "phone", "close-restoring-context").unwrap();
        let (cleanup_entered, cleanup_started) = oneshot::channel();
        agent
            .inner
            .lifecycle
            .notify_next_cleanup_start(cleanup_entered);
        let attempt = agent.start_shutdown(SessionCloseRequest::Explicit(closer.clone()));
        assert!(agent.inner.lifecycle.is_closed());
        cleanup_started.await.unwrap();
        let stopped = attempt.clone().wait_physical();
        tokio::pin!(stopped);
        assert!(
            poll_fn(|cx| Poll::Ready(stopped.as_mut().poll(cx)))
                .await
                .is_pending(),
            "old cached cleanup cannot acknowledge the new generation"
        );
        release.send(()).unwrap();
        assert_eq!(
            timeout(Duration::from_secs(2), attaching.wait())
                .await
                .expect("fenced attachment settles"),
            Err(AgentError::Closed)
        );
        let actual = timeout(Duration::from_secs(2), stopped).await.unwrap();
        assert_eq!(actual, report);
        assert_eq!(
            *backend.closes.lock().unwrap(),
            vec![
                SessionCloseRequest::Explicit(actor()),
                SessionCloseRequest::Explicit(closer),
            ]
        );
        assert_eq!(
            agent.inner.lifecycle.attachment_needs_cleanup(),
            !report.is_confirmed()
        );
        assert_eq!(agent.inner.lifecycle.complete_stop(&attempt).await, actual);
        assert_eq!(
            agent.inner.lifecycle.is_closed(),
            !report.is_confirmed() || report.audit().is_err()
        );
    }
}

#[tokio::test]
async fn close_before_restore_prevents_rearming() {
    let (agent, backend) = agent_with_backend().await;
    agent.close(actor()).await.unwrap();
    let authorization = agent
        .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
        .unwrap();
    let attempt = agent.start_shutdown(SessionCloseRequest::Explicit(actor()));
    assert!(matches!(
        agent.start_attachment(authorization),
        Err(AgentError::AttachmentAuthorizationStale)
    ));
    let report = agent.inner.lifecycle.complete_stop(&attempt).await;
    assert!(report.is_confirmed());
    assert!(!agent.inner.lifecycle.attachment_needs_cleanup());
    assert_eq!(backend.closes.lock().unwrap().len(), 1);
}
