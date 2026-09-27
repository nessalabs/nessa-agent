//! A shared cleanup attempt does not supply the cause of newly stopped work.
use super::*;
use crate::application::agent_execution::sessions::SessionStorage;
use crate::domain::agent_execution::sessions::SessionId;

#[tokio::test]
async fn explicit_close_owns_waiters_first_stopped_during_automatic_cleanup() {
    for finalized in [false, true] {
        let (agent, backend) = agent_with_backend().await;
        let invocation = agent.inner.invocation.lock().await;
        let queued = agent.enqueue(input(), actor()).await.unwrap();
        let waiting = agent.inner.lifecycle.accept_waiting_work().unwrap();
        let native = agent.inner.lifecycle.accept_control().unwrap();
        let report = CleanupReport::confirmed(CloseOutcome { forced: false });
        // A native operation reports completed physical cleanup. Its active work
        // stops now; the previously admitted waiting input remains eligible.
        agent.inner.lifecycle.record_provider_state(
            &native,
            &ProviderSessionState::CleanupReported(report.clone()),
        );
        let automatic = agent.start_shutdown(SessionCloseRequest::ExecutionFailed);
        if finalized {
            agent.inner.lifecycle.complete_stop(&automatic).await;
        }
        let closer = ActionContext::new("owner", "phone", "close-waiting-input").unwrap();
        let joined = agent.start_shutdown(SessionCloseRequest::Explicit(closer.clone()));
        assert_eq!(joined.request, SessionCloseRequest::ExecutionFailed);
        assert_eq!(
            native.cancellation().unwrap().cause,
            SchedulingCause::RunnerStopped
        );
        assert_eq!(native.cancellation().unwrap().actor, None);
        let queued_cancellation = waiting.cancellation().unwrap();
        assert_eq!(queued_cancellation.cause, SchedulingCause::SessionClosed);
        assert_eq!(queued_cancellation.actor, Some(closer.clone()));
        drop(native);
        drop(waiting);
        drop(invocation);
        timeout(Duration::from_secs(2), agent.close(closer.clone()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(queued.wait().await, Err(AgentError::Closed));
        let saved = agent.session_manager().snapshot().await.unwrap();
        let cancellation = saved.invocations[0].scheduling.last().unwrap();
        assert_eq!(cancellation.stage, InvocationStage::Cancelled);
        assert_eq!(cancellation.cause, SchedulingCause::SessionClosed);
        assert_eq!(cancellation.actor, Some(closer));
        assert!(
            backend.closes.lock().unwrap().is_empty(),
            "physical cleanup remains shared"
        );
        // Restore the actual saved receipt through the normal storage boundary.
        let storage = Arc::new(InMemoryStorage::new());
        let id = SessionId::new("restored-first-stop").unwrap();
        let mut saved = saved;
        saved.id = id.clone();
        let lease = storage.open(id.clone()).await.unwrap();
        lease.save(saved.clone()).await.unwrap();
        drop(lease);
        let restored = SessionManager::open(Some(id), storage).await.unwrap();
        let restored_agent =
            attached_agent(Arc::new(Provider(Arc::new(Backend::default()))), restored)
                .await
                .unwrap();
        let restored = restored_agent.session_manager().snapshot().await.unwrap();
        assert_eq!(
            restored.invocations[0].scheduling,
            saved.invocations[0].scheduling
        );
        assert_eq!(restored.invocations[0].result, saved.invocations[0].result);
        assert_eq!(
            restored_agent
                .enqueue(input(), actor())
                .await
                .unwrap()
                .wait()
                .await,
            Err(AgentError::Closed)
        );
        restored_agent.close(actor()).await.unwrap();
    }
}
