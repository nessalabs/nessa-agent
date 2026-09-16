//! Restoration and cleanup must operate on the same attached resource generation.
use super::*;
use crate::application::agent_execution::providers::ResourceCleanup;
use std::{
    future::{poll_fn, Future},
    task::Poll,
};
use tokio::sync::Mutex as AsyncMutex;

struct RestoredBackend {
    base: Backend,
    report: CleanupReport,
}
impl ProviderSessionBackend for RestoredBackend {
    fn prepare_invocation(&self) -> ProviderOperationFuture<'_, ()> {
        self.base.prepare_invocation()
    }
    fn execute(&self, request: ExecutionRequest) -> ProviderExecutionFuture<'_> {
        self.base.execute(request)
    }
    fn answer_permission(
        &self,
        answer: PermissionAnswer,
    ) -> ProviderOperationFuture<'_, PermissionResolution> {
        self.base.answer_permission(answer)
    }
    fn cancel_permission(
        &self,
        request: PermissionCancellationRequest,
    ) -> ProviderOperationFuture<'_, PermissionCancellation> {
        self.base.cancel_permission(request)
    }
    fn close(&self, request: SessionCloseRequest) -> CleanupFuture<'_> {
        Box::pin(async move {
            self.base.close(request).await;
            self.report.clone()
        })
    }
}

#[tokio::test]
async fn close_during_restore_cleans_the_rearmed_attachment() {
    for report in [
        CleanupReport::confirmed(CloseOutcome { forced: false }),
        CleanupReport::unconfirmed(AgentError::CleanupUncertain),
        CleanupReport::new(
            ResourceCleanup::Confirmed(CloseOutcome { forced: false }),
            Err(AgentError::AuditFailure),
        ),
    ] {
        let (agent, old_backend) = agent_with_backend().await;
        agent.close(actor()).await.unwrap();
        assert!(!agent.inner.lifecycle.attachment_needs_cleanup());
        let restored = Arc::new(RestoredBackend {
            base: Backend::default(),
            report: report.clone(),
        });
        let session = ProviderSession::new(
            agent.inner.session.id().clone(),
            restored.clone(),
            agent.inner.session.capabilities().clone(),
            Arc::new(AcceptingAudit),
        );
        let events: Arc<AsyncMutex<Box<dyn ExecutionEventStream>>> =
            Arc::new(AsyncMutex::new(Box::new(ExhaustedEvents)));
        let (entered, reached) = oneshot::channel();
        let (release, waiting) = oneshot::channel();
        agent
            .inner
            .lifecycle
            .pause_next_preparation(entered, waiting);
        let preparing = tokio::spawn({
            let lifecycle = agent.inner.lifecycle.clone();
            async move { lifecycle.prepare(session, events).await }
        });
        reached.await.unwrap();
        let closer = ActionContext::new("owner", "phone", "close-restoring-context").unwrap();
        let (cleanup_entered, cleanup_started) = oneshot::channel();
        agent
            .inner
            .lifecycle
            .notify_next_cleanup_start(cleanup_entered);
        let attempt = agent.start_shutdown(SessionCloseRequest::Explicit(closer.clone()));
        assert!(agent.inner.lifecycle.is_closed());
        // Let the independently owned cleanup task reach the held transition.
        cleanup_started.await.unwrap();
        let stopped = attempt.clone().wait();
        tokio::pin!(stopped);
        assert!(
            poll_fn(|cx| Poll::Ready(stopped.as_mut().poll(cx)))
                .await
                .is_pending(),
            "old cached cleanup cannot acknowledge the new generation"
        );
        release.send(()).unwrap();
        assert_eq!(
            timeout(Duration::from_secs(2), preparing)
                .await
                .unwrap()
                .unwrap(),
            Err(AgentError::Closed)
        );
        let actual = timeout(Duration::from_secs(2), stopped).await.unwrap();
        assert_eq!(actual, report);
        assert_eq!(
            *restored.base.closes.lock().unwrap(),
            vec![SessionCloseRequest::Explicit(closer)]
        );
        assert_eq!(old_backend.closes.lock().unwrap().len(), 1);
        assert_eq!(
            agent.inner.lifecycle.attachment_needs_cleanup(),
            !report.is_confirmed()
        );
        agent.inner.lifecycle.finalize_stop(&attempt, &actual).await;
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
    let attempt = agent.start_shutdown(SessionCloseRequest::Explicit(actor()));
    let events: Arc<AsyncMutex<Box<dyn ExecutionEventStream>>> =
        Arc::new(AsyncMutex::new(Box::new(ExhaustedEvents)));
    assert_eq!(
        agent
            .inner
            .lifecycle
            .prepare(agent.inner.session.clone(), events)
            .await,
        Err(AgentError::Closed)
    );
    let report = attempt.clone().wait().await;
    assert!(report.is_confirmed());
    agent.inner.lifecycle.finalize_stop(&attempt, &report).await;
    assert!(!agent.inner.lifecycle.attachment_needs_cleanup());
    assert_eq!(backend.closes.lock().unwrap().len(), 1);
}
