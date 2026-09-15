//! A ready native acknowledgement survives a simultaneous explicit close.
use super::*;

#[tokio::test]
async fn ready_native_acknowledgement_survives_close_and_restoration() {
    let (agent, backend, storage) = probe(false).await;
    let (release_execution, execution_gate) = oneshot::channel();
    *backend.execution_gate.lock().unwrap() = Some(execution_gate);
    let running = tokio::spawn({
        let agent = agent.clone();
        async move { agent.invoke(input("ready-target"), actor()).await }
    });
    timeout(Duration::from_secs(2), backend.executing.notified())
        .await
        .unwrap();
    let (release_control, control_gate) = oneshot::channel();
    *backend.control_gate.lock().unwrap() = Some(control_gate);
    let steering = tokio::spawn({
        let agent = agent.clone();
        async move { agent.steer(input("ready-steering"), actor()).await }
    });
    timeout(Duration::from_secs(2), backend.control_entered.notified())
        .await
        .unwrap();
    let closing = agent.close(actor());
    tokio::pin!(closing);
    // Publish close without yielding this current-thread runtime to the steering
    // task, then publish its acknowledgement before it can be polled again.
    assert!(poll_fn(|cx| Poll::Ready(closing.as_mut().poll(cx)))
        .await
        .is_pending());
    release_control.send(()).unwrap();
    let delivery = timeout(Duration::from_secs(2), steering)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(matches!(delivery, SteeringDelivery::Injected { .. }));
    release_execution.send(()).unwrap();
    running.await.unwrap().unwrap();
    timeout(Duration::from_secs(2), closing)
        .await
        .unwrap()
        .unwrap();
    let snapshot = storage.snapshot();
    let record = &snapshot.invocations[1];
    assert_eq!(
        record.scheduling.last().unwrap().stage,
        InvocationStage::Injected
    );
    assert_eq!(
        record.scheduling.last().unwrap().cause,
        SchedulingCause::SteeringInjected
    );
    assert_eq!(
        record.scheduling.last().unwrap().target,
        Some(input("ready-target").execution_id)
    );
    assert!(matches!(
        agent.steer(input("ready-steering"), actor()).await.unwrap(),
        SteeringDelivery::Injected { .. }
    ));
    assert_eq!(backend.steers.load(Ordering::SeqCst), 1);
    let restored_storage = MemoryStorage::default();
    restored_storage.0.lock().unwrap().snapshot = Some(snapshot);
    let (restored, restored_backend) =
        probe_with_manager(false, restored_storage.manager().await).await;
    assert!(matches!(
        restored
            .steer(input("ready-steering"), actor())
            .await
            .unwrap(),
        SteeringDelivery::Injected { .. }
    ));
    assert_eq!(restored_backend.steers.load(Ordering::SeqCst), 0);
    restored.close(actor()).await.unwrap();
}
