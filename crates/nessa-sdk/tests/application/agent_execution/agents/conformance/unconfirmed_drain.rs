//! Unconfirmed cleanup ends observation draining while preserving the accepted prefix.
use super::*;

struct ReadyOutputProvider {
    backend: Arc<WorkflowBackend>,
    polls: Arc<AtomicUsize>,
    release: Mutex<Option<oneshot::Sender<()>>>,
}
struct ReadyOutput {
    backend: Arc<WorkflowBackend>,
    // Retain the paired receiver; the backend's normal completion send stays valid.
    _original: Box<dyn ExecutionEventStream>,
    polls: Arc<AtomicUsize>,
    release: Option<oneshot::Sender<()>>,
}
impl AgentProvider for ReadyOutputProvider {
    fn identity(&self) -> ProviderIdentity {
        WorkflowProvider(self.backend.clone()).identity()
    }
    fn capabilities(&self) -> &EffectiveCapabilities {
        capabilities_ref()
    }
    fn open(&self, restore: Option<ExecutionSessionId>) -> ProviderOpenFuture<'_> {
        Box::pin(async move {
            let mut opened = WorkflowProvider(self.backend.clone()).open(restore).await?;
            opened.events = Box::new(ReadyOutput {
                backend: self.backend.clone(),
                _original: opened.events,
                polls: self.polls.clone(),
                release: self.release.lock().unwrap().take(),
            });
            Ok(opened)
        })
    }
}
impl ExecutionEventStream for ReadyOutput {
    fn next(&mut self) -> ProviderObservationFuture<'_> {
        Box::pin(async move {
            // This fixture produces invocation observations only after execute starts.
            if self.backend.executions.lock().unwrap().is_empty() {
                return std::future::pending().await;
            }
            let poll = self.polls.fetch_add(1, Ordering::SeqCst);
            if let Some(release) = self.release.take() {
                release.send(()).unwrap();
            }
            // A finite sentinel keeps the broken version's regression cheap. All
            // preceding chunks are immediately ready, as with an endless stream.
            if poll == 1_024 {
                return Ok(None);
            }
            Ok(Some(ExecutionEvent::new(
                ExecutionId::new("unconfirmed").unwrap(),
                ExecutionUpdate::Message(MessageChunk::text("still producing")),
            )))
        })
    }
}

#[tokio::test]
async fn unconfirmed_cleanup_stops_ready_output_and_preserves_settlement() {
    for mode in Mode::ALL {
        for reported in [false, true] {
            for audit_failed in [false, true] {
                let storage = MemoryStorage::default();
                let backend = workflow_backend();
                let cleanup = CleanupReport::new(
                    ResourceCleanup::Unconfirmed(AgentError::CleanupUncertain),
                    if audit_failed {
                        Err(AgentError::AuditFailure)
                    } else {
                        Ok(())
                    },
                );
                let report = ExecutionReport::new(
                    Some(Ok(ExecutionOutcome::Completed)),
                    None,
                    if reported {
                        ProviderSessionState::CleanupReported(cleanup.clone())
                    } else {
                        ProviderSessionState::CleanupRequired
                    },
                );
                *backend.execution_report.lock().unwrap() = Some(report.clone());
                *backend.cleanup.lock().unwrap() = cleanup.clone();
                let (release, gate) = oneshot::channel();
                *backend.execution_gate.lock().unwrap() = Some(gate);
                let polls = Arc::new(AtomicUsize::new(0));
                let agent = attached_agent(
                    Arc::new(ReadyOutputProvider {
                        backend: backend.clone(),
                        polls: polls.clone(),
                        release: Mutex::new(Some(release)),
                    }),
                    storage.manager().await,
                )
                .await
                .unwrap();
                let result = bounded(mode.start(agent.clone(), request("unconfirmed")))
                    .await
                    .unwrap();
                assert_eq!(
                    polls.load(Ordering::SeqCst),
                    1,
                    "{mode:?} reported={reported} audit_failed={audit_failed}"
                );
                let expected = if reported {
                    report.clone().into_result()
                } else {
                    Err(AgentError::ExecutionObservation {
                        error: Box::new(cleanup.into_result().unwrap_err()),
                        execution_result: Some(Box::new(Ok(ExecutionOutcome::Completed))),
                    })
                };
                assert_eq!(result, expected);
                let saved = storage.snapshot();
                assert_eq!(saved.invocations[0].events.len(), 1);
                assert_eq!(saved.invocations[0].provider_report, Some(report));
                assert_eq!(saved.invocations[0].result, Some(result));
                assert_eq!(backend.executions.lock().unwrap().len(), 1);
                // A later confirmed retry can finish close without waiting for the
                // old stream to exhaust. It cannot turn its prior result into success.
                *backend.cleanup.lock().unwrap() =
                    CleanupReport::confirmed(CloseOutcome { forced: false });
                bounded(agent.close(actor())).await.unwrap();
                assert_eq!(polls.load(Ordering::SeqCst), 1);
                let restored_storage = MemoryStorage::default();
                restored_storage.0.lock().unwrap().snapshot = Some(saved);
                let (restored, _, _) = workflow_from_storage(restored_storage).await;
                bounded(restored.close(actor())).await.unwrap();
            }
        }
    }
}
