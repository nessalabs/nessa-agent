//! A ready result from a retired attachment cannot change the restored attachment's health.
use super::*;
use crate::application::agent_execution::providers::ResourceCleanup;
use crate::domain::agent_execution::{
    permissions::{
        PermissionDecision, PermissionEffect, PermissionId, PermissionOfferPolicy,
        PermissionOption, PermissionOptionId, PermissionOptions, PermissionScope,
    },
    tools::ToolCallId,
};
use std::{
    future::{poll_fn, Future},
    task::Poll,
};

async fn restore_attachment(agent: &Agent, prior: &WorkPermit) {
    agent.inner.lifecycle.record_provider_state(
        prior,
        &ProviderSessionState::CleanupReported(CleanupReport::confirmed(CloseOutcome {
            forced: false,
        })),
    );
    let authorization = agent
        .authorize_attachment(AttachmentRequest::AutomaticRecovery)
        .unwrap();
    agent
        .start_attachment(authorization)
        .unwrap()
        .wait()
        .await
        .unwrap();
    let preparation = agent.accept_preparation().unwrap();
    agent
        .run_preparation(preparation, async { Ok(()) })
        .await
        .unwrap();
    // Confirmed restoration changes the attachment generation, not the work
    // generation. Comparing the latter alone cannot reject old state effects.
    assert_eq!(
        agent.inner.lifecycle.work_generation(),
        prior.work_generation()
    );
}

#[tokio::test]
async fn retired_control_failure_returns_without_stopping_restored_attachment() {
    for state in [
        ProviderSessionState::CleanupRequired,
        ProviderSessionState::CleanupReported(CleanupReport::confirmed(CloseOutcome {
            forced: false,
        })),
        ProviderSessionState::CleanupReported(CleanupReport::new(
            ResourceCleanup::Confirmed(CloseOutcome { forced: false }),
            Err(AgentError::AuditFailure),
        )),
    ] {
        let (agent, backend) = agent_with_backend().await;
        let admission = agent.accept_control().unwrap();
        let (release, waiting) = oneshot::channel();
        let control = agent.run_control(admission.clone(), async {
            waiting.await.unwrap();
            Err::<(), _>(ProviderOperationFailure::new(
                AgentError::StalePermission,
                state,
            ))
        });
        tokio::pin!(control);
        assert!(poll_fn(|cx| Poll::Ready(control.as_mut().poll(cx)))
            .await
            .is_pending());
        // Hold the older control's polling while another operation reports
        // cleanup and the next invocation prepares a new attachment.
        restore_attachment(&agent, &admission).await;
        release.send(()).unwrap();
        assert_eq!(control.await, Err(AgentError::StalePermission));
        assert!(
            agent
                .inner
                .lifecycle
                .start_control_cleanup(&admission)
                .is_none(),
            "retired native failure cannot start cleanup of the new attachment"
        );
        let current = agent
            .accept_control()
            .expect("old failure must not fence restored provider");
        assert_eq!(agent.run_control(current, async { Ok(()) }).await, Ok(()));
        assert!(backend.closes.lock().unwrap().is_empty());
        drop(admission);
        agent.close(actor()).await.unwrap();
    }
}

#[tokio::test]
async fn retired_permission_validation_failure_cannot_stop_restored_attachment() {
    let (agent, backend) = agent_with_backend().await;
    let admission = agent.accept_control().unwrap();
    restore_attachment(&agent, &admission).await;
    let decision = PermissionDecision::new(PermissionEffect::Allow, PermissionScope::request());
    let options = PermissionOptions::new(
        vec![PermissionOption::new(
            PermissionOptionId::new("allow").unwrap(),
            "Allow",
            decision.clone(),
        )
        .unwrap()],
        &PermissionOfferPolicy::new(vec![decision]).unwrap(),
    )
    .unwrap();
    let request = PermissionRequest::new(
        PermissionId::new("old-review").unwrap(),
        ExecutionId::new("old-execution").unwrap(),
        ToolCallId::new("old-tool").unwrap(),
        options,
    );
    let error = agent
        .validate_permission_receipt(
            &admission,
            &request,
            &ToolReviewInput {
                name: "read".into(),
                arguments_json: "{}".into(),
            },
        )
        .await;
    assert!(matches!(error, Err(AgentError::Protocol(_))));
    let current = agent
        .accept_control()
        .expect("old receipt validation must not fence restored provider");
    assert_eq!(agent.run_control(current, async { Ok(()) }).await, Ok(()));
    assert!(backend.closes.lock().unwrap().is_empty());
    drop(admission);
    agent.close(actor()).await.unwrap();
}

#[tokio::test]
async fn current_control_failure_keeps_cleanup_owner_after_stopping_work_generation() {
    let (agent, backend) = agent_with_backend().await;
    let admission = agent.accept_control().unwrap();
    let error = agent
        .run_control(admission.clone(), async {
            Err::<(), _>(ProviderOperationFailure::new(
                AgentError::StalePermission,
                ProviderSessionState::CleanupRequired,
            ))
        })
        .await;
    assert_eq!(error, Err(AgentError::StalePermission));
    assert_ne!(
        agent.inner.lifecycle.work_generation(),
        admission.work_generation()
    );
    let attempt = agent
        .inner
        .lifecycle
        .start_control_cleanup(&admission)
        .expect("own failure still owns cleanup after stopping work");
    let report = attempt.clone().wait().await;
    assert!(report.is_confirmed());
    agent.inner.lifecycle.finalize_stop(&attempt, &report).await;
    drop(admission);
    assert_eq!(
        backend.closes.lock().unwrap().as_slice(),
        &[SessionCloseRequest::ExecutionFailed]
    );
    agent.close(actor()).await.unwrap();
}

#[tokio::test]
async fn retired_control_supervisor_failure_cannot_block_restored_attachment() {
    let (agent, backend) = agent_with_backend().await;
    let old = agent.accept_control().unwrap();
    restore_attachment(&agent, &old).await;
    // The old task's failed join resumes after a different attachment is ready.
    agent.inner.lifecycle.block_control(old.control_origin());
    let current = agent
        .accept_control()
        .expect("old supervisor cannot block new attachment");
    assert_eq!(
        agent.run_control(current.clone(), async { Ok(()) }).await,
        Ok(())
    );
    assert!(backend.closes.lock().unwrap().is_empty());
    // The same failure still fences the attachment that actually owns the task.
    agent
        .inner
        .lifecycle
        .block_control(current.control_origin());
    assert!(matches!(agent.accept_control(), Err(AgentError::Closed)));
    drop(current);
    drop(old);
    agent.close(actor()).await.unwrap();
}
