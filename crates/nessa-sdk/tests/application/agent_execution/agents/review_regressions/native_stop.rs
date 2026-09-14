//! Native delivery failures remain distinct from automatic or explicit stops.
use super::*;

#[tokio::test]
async fn native_delivery_failure_without_prior_stop_keeps_dispatch_failure() {
    let (agent, backend, storage) = probe(false).await;
    let (release_execution, gate) = oneshot::channel();
    *backend.execution_gate.lock().unwrap() = Some(gate);
    let running = tokio::spawn({
        let agent = agent.clone();
        async move { agent.invoke(input("target"), actor()).await }
    });
    timeout(Duration::from_secs(2), backend.executing.notified())
        .await
        .unwrap();
    let error = AgentError::Provider { code: -32077 };
    *backend.control_error.lock().unwrap() = Some(error.clone());
    let steering = tokio::spawn({
        let agent = agent.clone();
        async move { agent.steer(input("failed-delivery"), actor()).await }
    });
    // The provider rejects this delivery while the target remains usable.
    // Absence of a preceding stop must retain a delivery failure.
    assert_eq!(
        timeout(Duration::from_secs(2), steering)
            .await
            .unwrap()
            .unwrap()
            .err(),
        Some(error.clone())
    );
    release_execution.send(()).unwrap();
    assert_eq!(running.await.unwrap(), Ok(ExecutionOutcome::Completed));
    let saved = storage.snapshot();
    let record = &saved.invocations[1];
    let stop = record.scheduling.last().unwrap();
    assert_eq!(stop.before, Some(InvocationStage::Queued));
    assert_eq!(stop.stage, InvocationStage::Settled);
    assert_eq!(stop.cause, SchedulingCause::DispatchFailed);
    assert_eq!(stop.actor, None);
    assert_eq!(stop.target, Some(ExecutionId::new("target").unwrap()));
    assert_eq!(record.result, Some(Err(error.clone())));
    assert_eq!(
        agent.steer(input("failed-delivery"), actor()).await.err(),
        Some(error)
    );
    assert_eq!(backend.steers.load(Ordering::SeqCst), 1);
    agent.close(actor()).await.unwrap();
}
