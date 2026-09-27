//! Scheduling completion follows retained outcomes, not diagnostic error shapes.
use super::*;

#[tokio::test]
async fn queued_preparation_and_rejection_failures_do_not_claim_a_known_outcome() {
    for preparation in [false, true] {
        let storage = MemoryStorage::default();
        let (agent, backend) = probe_with_manager(false, storage.manager().await).await;
        if preparation {
            let (release, gate) = oneshot::channel();
            *backend.prepare_gate.lock().unwrap() = Some(gate);
            drop(release);
        } else {
            backend.suppress_terminal.store(true, Ordering::SeqCst);
            backend.execution_rejected.store(true, Ordering::SeqCst);
            *backend.execution_error.lock().unwrap() =
                Some(AgentError::InvalidInput("rejected input".into()));
        }
        assert!(agent
            .enqueue(input("failed"), actor())
            .await
            .unwrap()
            .wait()
            .await
            .is_err());
        let saved = storage.snapshot();
        let record = &saved.invocations[0];
        assert!(record.provider_report.is_none());
        assert_eq!(
            record.scheduling.last().unwrap().stage,
            InvocationStage::Settled
        );
        assert_eq!(
            record.scheduling.last().unwrap().cause,
            SchedulingCause::ExecutionFailed
        );
        agent.close(actor()).await.unwrap();
    }
}

struct FailAfterOutcome;
impl InvocationHook for FailAfterOutcome {
    fn after_invocation(
        &self,
        _: &InvocationContext<'_>,
        _: &Result<ExecutionOutcome, AgentError>,
    ) -> Result<(), HookError> {
        Err(HookError::Failed("local hook failed".into()))
    }
}

#[tokio::test]
async fn queued_failure_cause_uses_provider_facts_instead_of_local_error_shape() {
    for known_outcome in [false, true] {
        let storage = MemoryStorage::default();
        let provider = Arc::new(TestProvider {
            identity: ProviderIdentity::new("cause", "test", "test").unwrap(),
            calls: Arc::new(ProviderCalls::default()),
            wait_for_close: false,
            outcome: if known_outcome {
                Ok(ExecutionOutcome::Completed)
            } else {
                Err(AgentError::Protocol(
                    "provider failed without outcome".into(),
                ))
            },
        });
        let agent = attached_agent(provider, storage.manager().await)
            .await
            .unwrap();
        if known_outcome {
            agent.add_invocation_hook(Arc::new(FailAfterOutcome));
        }
        assert!(agent
            .enqueue(input("failed"), actor())
            .await
            .unwrap()
            .wait()
            .await
            .is_err());
        let saved = storage.snapshot();
        let record = &saved.invocations[0];
        assert!(record.result.as_ref().unwrap().is_err());
        assert_eq!(
            record.scheduling.last().unwrap().stage,
            InvocationStage::Settled
        );
        assert_eq!(
            record.scheduling.last().unwrap().cause,
            if known_outcome {
                SchedulingCause::ExecutionSettled
            } else {
                SchedulingCause::ExecutionFailed
            }
        );
        agent.close(actor()).await.unwrap();
    }
}
