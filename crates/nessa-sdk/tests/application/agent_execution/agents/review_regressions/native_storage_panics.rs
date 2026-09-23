//! A committed steering write whose acknowledgement panics remains one submission.
use super::*;

struct PanicStorage {
    backing: MemoryStorage,
    stage: InvocationStage,
    fired: Arc<AtomicBool>,
}
struct PanicLease {
    backing: Box<dyn SessionStorageLease>,
    stage: InvocationStage,
    fired: Arc<AtomicBool>,
}
impl SessionStorage for PanicStorage {
    fn open(&self, id: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
        Box::pin(async move {
            Ok(Box::new(PanicLease {
                backing: self.backing.open(id).await?,
                stage: self.stage,
                fired: self.fired.clone(),
            }) as Box<dyn SessionStorageLease>)
        })
    }
}
impl SessionStorageLease for PanicLease {
    fn load(&self) -> StorageFuture<'_, Option<SessionSnapshot>> {
        self.backing.load()
    }
    fn save(&self, snapshot: SessionSnapshot) -> StorageFuture<'_, ()> {
        Box::pin(async move {
            let matches = snapshot.invocations.iter().any(|record| {
                record.request.execution_id.as_str() == "steering"
                    && record
                        .scheduling
                        .last()
                        .is_some_and(|event| event.stage == self.stage)
            });
            self.backing.save(snapshot).await?;
            if matches && !self.fired.swap(true, Ordering::SeqCst) {
                panic!("committed steering acknowledgement panicked");
            }
            Ok(())
        })
    }
}

#[tokio::test]
async fn native_steering_storage_panic_keeps_receipt_evidence_and_cleanup_barrier() {
    for stage in [InvocationStage::Queued, InvocationStage::Injected] {
        let storage = MemoryStorage::default();
        let manager = SessionManager::open(
            Some(SessionId::new("conversation").unwrap()),
            Arc::new(PanicStorage {
                backing: storage.clone(),
                stage,
                fired: Arc::new(AtomicBool::new(false)),
            }),
        )
        .await
        .unwrap();
        let (agent, backend) = probe_with_manager(false, manager).await;
        let (release_execution, gate) = oneshot::channel();
        *backend.execution_gate.lock().unwrap() = Some(gate);
        let active = tokio::spawn({
            let agent = agent.clone();
            async move { agent.invoke(input("active"), actor()).await }
        });
        backend.executing.notified().await;
        if stage == InvocationStage::Queued {
            let result = timeout(
                Duration::from_secs(2),
                agent.steer(input("steering"), actor()),
            )
            .await
            .unwrap();
            assert!(matches!(result, Err(AgentError::SubmissionUnresolved)));
            assert!(matches!(
                agent.steer(input("steering"), actor()).await,
                Err(AgentError::SubmissionUnresolved)
            ));
            assert_eq!(backend.steers.load(Ordering::SeqCst), 0);
            assert_eq!(backend.closes.load(Ordering::SeqCst), 0);
            release_execution.send(()).unwrap();
            assert_eq!(active.await.unwrap(), Ok(ExecutionOutcome::Completed));
            // A later successful save cannot erase the uncertain committed admission.
            assert_eq!(
                agent
                    .enqueue(input("next"), actor())
                    .await
                    .unwrap()
                    .wait()
                    .await,
                Ok(ExecutionOutcome::Completed)
            );
            let saved = storage.snapshot();
            let record = saved
                .invocations
                .iter()
                .find(|record| record.request.execution_id.as_str() == "steering")
                .unwrap();
            assert_eq!(record.result, None);
            assert_eq!(record.actor, actor());
            assert_eq!(record.scheduling.len(), 1);
            assert_eq!(record.scheduling[0].stage, InvocationStage::Queued);
            assert_eq!(record.scheduling[0].cause, SchedulingCause::Submitted);
            assert!(matches!(
                agent.steer(input("steering"), actor()).await,
                Err(AgentError::SubmissionUnresolved)
            ));
            assert_eq!(backend.steers.load(Ordering::SeqCst), 0);
            agent.close(actor()).await.unwrap();
            continue;
        }
        let (release_cleanup, gate) = oneshot::channel();
        *backend.close_gate.lock().unwrap() = Some(gate);
        let steering = tokio::spawn({
            let agent = agent.clone();
            async move { agent.steer(input("steering"), actor()).await }
        });
        timeout(Duration::from_secs(2), backend.closing.notified())
            .await
            .expect("panic must start supervised cleanup");
        assert!(matches!(
            agent.enqueue(input("blocked"), actor()).await,
            Err(AgentError::Closed)
        ));
        assert_eq!(backend.executions.load(Ordering::SeqCst), 1);
        release_execution.send(()).unwrap();
        release_cleanup.send(()).unwrap();
        let delivery = timeout(Duration::from_secs(2), steering)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let original_evidence = match &delivery {
            SteeringDelivery::Injected { evidence, .. } => evidence.clone(),
            SteeringDelivery::Queued(_) => panic!("confirmed native injection must stay injected"),
        };
        assert!(matches!(
            &delivery,
            SteeringDelivery::Injected {
                evidence: SteeringEvidence::Failed(failure),
                ..
            } if matches!(failure.storage(), Some(StorageError::Io(_)))
        ));
        assert_eq!(active.await.unwrap(), Ok(ExecutionOutcome::Completed));
        let retry = timeout(
            Duration::from_secs(2),
            agent.steer(input("steering"), actor()),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(matches!(
            retry,
            SteeringDelivery::Injected {
                evidence: SteeringEvidence::Failed(_),
                ..
            }
        ));
        assert_eq!(
            backend.steers.load(Ordering::SeqCst),
            usize::from(stage == InvocationStage::Injected)
        );
        let saved = storage.snapshot();
        let record = saved
            .invocations
            .iter()
            .find(|record| record.request.execution_id.as_str() == "steering")
            .unwrap();
        assert_eq!(record.result, None);
        assert_eq!(record.actor, actor());
        let final_event = record.scheduling.last().unwrap();
        if stage == InvocationStage::Injected {
            assert_eq!(final_event.stage, InvocationStage::Injected);
            assert_eq!(final_event.cause, SchedulingCause::SteeringInjected);
        } else {
            assert_eq!(final_event.stage, InvocationStage::Settled);
            assert_eq!(final_event.cause, SchedulingCause::DispatchFailed);
        }
        assert!(final_event.actor.is_none());
        assert!(matches!(
            agent.enqueue(input("still-blocked"), actor()).await,
            Err(AgentError::Closed)
        ));
        agent.close(actor()).await.unwrap();
        reattach_after_explicit_close(&agent).await;
        assert_eq!(
            agent
                .enqueue(input("recovered"), actor())
                .await
                .unwrap()
                .wait()
                .await,
            Ok(ExecutionOutcome::Completed)
        );
        agent.close(actor()).await.unwrap();
        drop(agent);
        let (restored, restored_backend) = probe_with_manager(false, storage.manager().await).await;
        let restored_delivery = restored.steer(input("steering"), actor()).await.unwrap();
        assert!(matches!(
            restored_delivery,
            SteeringDelivery::Injected { evidence, .. } if evidence == original_evidence
        ));
        assert_eq!(restored_backend.steers.load(Ordering::SeqCst), 0);
        restored.close(actor()).await.unwrap();
    }
}
