//! Explicit close retains the whole cancellation batch across storage panics.
use super::*;
use std::{pin::Pin, task::Context};

struct CancellationPanicStorage {
    backing: MemoryStorage,
    commit_first: bool,
    drop_panics: bool,
    panics: usize,
}
struct CancellationPanicLease {
    backing: Box<dyn SessionStorageLease>,
    commit_first: bool,
    drop_panics: bool,
    remaining: AtomicUsize,
}
impl SessionStorage for CancellationPanicStorage {
    fn open(&self, id: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
        Box::pin(async move {
            Ok(Box::new(CancellationPanicLease {
                backing: self.backing.open(id).await?,
                commit_first: self.commit_first,
                drop_panics: self.drop_panics,
                remaining: AtomicUsize::new(self.panics),
            }) as Box<dyn SessionStorageLease>)
        })
    }
}
impl SessionStorageLease for CancellationPanicLease {
    fn load(&self) -> StorageFuture<'_, Option<SessionSnapshot>> {
        self.backing.load()
    }
    fn save(&self, snapshot: SessionSnapshot) -> StorageFuture<'_, ()> {
        let cancelling = snapshot.invocations.iter().any(|record| {
            record.request.execution_id.as_str() == "pending-one"
                && record
                    .scheduling
                    .last()
                    .is_some_and(|last| last.stage == InvocationStage::Cancelled)
        });
        let panic = cancelling
            && self
                .remaining
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                    left.checked_sub(1)
                })
                .is_ok();
        Box::pin(CancellationSave {
            inner: Box::pin(async move {
                if !self.drop_panics {
                    assert!(
                        !panic || self.commit_first,
                        "cancellation save panicked before commit"
                    );
                }
                self.backing.save(snapshot).await?;
                assert!(
                    !panic || self.drop_panics,
                    "cancellation save panicked after commit"
                );
                Ok(())
            }),
            panic_on_drop: panic && self.drop_panics,
        })
    }
    fn erase(&self) -> StorageFuture<'_, ()> {
        self.backing.erase()
    }
}
struct CancellationSave<'a> {
    inner: StorageFuture<'a, ()>,
    panic_on_drop: bool,
}
impl Future for CancellationSave<'_> {
    type Output = Result<(), StorageError>;
    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        self.inner.as_mut().poll(context)
    }
}
impl Drop for CancellationSave<'_> {
    fn drop(&mut self) {
        assert!(
            !self.panic_on_drop,
            "cancellation save future drop panicked"
        );
    }
}

#[tokio::test]
async fn explicit_close_storage_panics_settle_every_pending_receipt_and_finalize_cleanup() {
    for (commit_first, drop_panics) in [(false, false), (true, false), (true, true)] {
        for panics in [1, 2] {
            let storage = MemoryStorage::default();
            let manager = SessionManager::open(
                Some(SessionId::new("conversation").unwrap()),
                Arc::new(CancellationPanicStorage {
                    backing: storage.clone(),
                    commit_first,
                    drop_panics,
                    panics,
                }),
            )
            .await
            .unwrap();
            let (agent, backend) = probe_with_manager(false, manager).await;
            let (release, gate) = oneshot::channel();
            *backend.execution_gate.lock().unwrap() = Some(gate);
            let active = tokio::spawn({
                let agent = agent.clone();
                async move { agent.invoke(input("active"), actor()).await }
            });
            backend.executing.notified().await;
            let first = agent.enqueue(input("pending-one"), actor()).await.unwrap();
            let priority = agent
                .enqueue_steering(input("pending-two"), actor())
                .await
                .unwrap();
            // Steering drains first. This ordinary input follows the faulted
            // ordinary input and proves the batch continues after its save panic.
            let tail = agent
                .enqueue(input("pending-three"), actor())
                .await
                .unwrap();
            let closer = ActionContext::new("owner", "phone", "close").unwrap();
            let closing = tokio::spawn({
                let agent = agent.clone();
                let closer = closer.clone();
                async move { agent.close(closer).await }
            });
            backend.closing.notified().await;
            let first_result = timeout(Duration::from_secs(2), first.wait()).await.unwrap();
            let mut retained = &first_result;
            let mut storage_failures = 0;
            while let Err(AgentError::StorageAfterExecution {
                error: StorageError::Io(_),
                execution_result,
            }) = retained
            {
                storage_failures += 1;
                retained = execution_result.as_ref();
            }
            assert_eq!(
                storage_failures, panics,
                "commit_first={commit_first}, drop_panics={drop_panics}, result={first_result:?}"
            );
            assert_eq!(retained, &Err(AgentError::Closed));
            assert_eq!(
                timeout(Duration::from_secs(2), priority.wait())
                    .await
                    .unwrap(),
                Err(AgentError::Closed)
            );
            assert_eq!(
                timeout(Duration::from_secs(2), tail.wait()).await.unwrap(),
                Err(AgentError::Closed)
            );
            // Receipt settlement must not wait for the active invocation to finish.
            release.send(()).unwrap();
            let _ = active.await.unwrap();
            let closed = timeout(Duration::from_secs(2), closing)
                .await
                .unwrap()
                .unwrap();
            match (&closed, panics) {
                (
                    Err(AgentError::StorageDuringClose { cleanup_result, .. }),
                    1,
                ) => assert!(cleanup_result.is_ok()),
                (
                    Err(AgentError::MultipleOperationFailures {
                        first_error,
                        subsequent_error,
                    }),
                    2,
                ) => {
                    assert!(matches!(
                        first_error.as_ref(),
                        AgentError::Storage(StorageError::Io(message))
                            if message == "pending cancellation persistence panicked"
                    ));
                    assert!(matches!(
                        subsequent_error.as_ref(),
                        AgentError::Storage(StorageError::Io(message))
                            if message == "pending cancellation receipt persistence panicked"
                    ));
                }
                _ => panic!(
                    "commit_first={commit_first}, drop_panics={drop_panics}, panics={panics}, close={closed:?}"
                ),
            }
            assert_eq!(backend.closes.load(Ordering::SeqCst), 1);
            assert_eq!(
                *backend.close_requests.lock().unwrap(),
                vec![SessionCloseRequest::Explicit(closer.clone())]
            );
            let saved = storage.snapshot();
            assert_eq!(saved.invocations.len(), 4);
            for (record, name) in
                saved.invocations[1..]
                    .iter()
                    .zip(["pending-one", "pending-two", "pending-three"])
            {
                assert_eq!(record.request.execution_id.as_str(), name);
                let cancellation = record.scheduling.last().unwrap();
                assert_eq!(cancellation.before, Some(InvocationStage::Queued));
                assert_eq!(cancellation.stage, InvocationStage::Cancelled);
                assert_eq!(cancellation.cause, SchedulingCause::SessionClosed);
                assert_eq!(cancellation.actor, Some(closer.clone()));
            }
            assert_eq!(saved.invocations[1].result, Some(first_result.clone()));
            assert_eq!(
                agent
                    .enqueue(input("pending-one"), actor())
                    .await
                    .unwrap()
                    .wait()
                    .await,
                first_result
            );
            // The confirmed cleanup was finalized even though close returned the
            // storage failure; caller-authorized reattachment opens a new generation.
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
        }
    }
}
