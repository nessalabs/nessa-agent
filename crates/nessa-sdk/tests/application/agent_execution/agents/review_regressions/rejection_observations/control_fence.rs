//! Rejected execution observations fence controls before storage can suspend.
use super::*;

type ObservationGate = Arc<Mutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>>;

struct PausedObservationStorage {
    backing: MemoryStorage,
    gate: ObservationGate,
}
struct PausedObservationLease {
    backing: Box<dyn SessionStorageLease>,
    gate: ObservationGate,
}
impl SessionStorage for PausedObservationStorage {
    fn open(&self, id: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
        Box::pin(async move {
            Ok(Box::new(PausedObservationLease {
                backing: self.backing.open(id).await?,
                gate: self.gate.clone(),
            }) as Box<dyn SessionStorageLease>)
        })
    }
}
impl SessionStorageLease for PausedObservationLease {
    fn load(&self) -> StorageFuture<'_, Option<SessionSnapshot>> {
        self.backing.load()
    }
    fn save(&self, snapshot: SessionSnapshot) -> StorageFuture<'_, ()> {
        Box::pin(async move {
            if snapshot
                .invocations
                .iter()
                .any(|record| !record.events.is_empty())
            {
                let gate = self.gate.lock().unwrap().take();
                if let Some((entered, release)) = gate {
                    entered.send(()).unwrap();
                    release.await.unwrap();
                }
            }
            self.backing.save(snapshot).await
        })
    }
    fn erase(&self) -> StorageFuture<'_, ()> {
        self.backing.erase()
    }
}

#[derive(Clone, Copy, Debug)]
enum Observation {
    Tool,
    Review,
}
#[derive(Clone, Copy, Debug)]
enum Control {
    Answer,
    Cancel,
}
#[derive(Clone, Copy, Debug)]
enum Admission {
    BeforeContradiction,
    AfterContradiction,
}

#[tokio::test]
async fn rejected_observation_fences_permission_controls_before_its_save_resumes() {
    for observation in [Observation::Tool, Observation::Review] {
        for control in [Control::Answer, Control::Cancel] {
            for admission in [
                Admission::BeforeContradiction,
                Admission::AfterContradiction,
            ] {
                let storage = MemoryStorage::default();
                let (saving, paused) = oneshot::channel();
                let (resume, release) = oneshot::channel();
                let manager = SessionManager::open(
                    Some(SessionId::new("conversation").unwrap()),
                    Arc::new(PausedObservationStorage {
                        backing: storage.clone(),
                        gate: Arc::new(Mutex::new(Some((saving, release)))),
                    }),
                )
                .await
                .unwrap();
                let (agent, backend) = probe_with_manager(false, manager).await;
                let pending = pending_permission();
                let update = if matches!(observation, Observation::Review) {
                    ExecutionUpdate::PermissionRequested {
                        id: pending.id().clone(),
                        tool_id: pending.tool_id().clone(),
                        observation: ToolObservation::default(),
                        input: review_input(),
                        options: pending.options().clone(),
                    }
                } else {
                    ExecutionUpdate::Tool(ToolCallUpdate::new(
                        pending.tool_id().clone(),
                        None,
                        None,
                        None,
                        None,
                        None,
                    ))
                };
                backend.execution_rejected.store(true, Ordering::SeqCst);
                backend.suppress_terminal.store(true, Ordering::SeqCst);
                *backend.execution_error.lock().unwrap() =
                    Some(AgentError::InvalidInput("not admitted".into()));
                *backend.late_output.lock().unwrap() = Some(update.clone());
                let control_operation = {
                    let agent = agent.clone();
                    async move {
                        if matches!(control, Control::Cancel) {
                            agent
                                .cancel_permission(PermissionCancellationRequest {
                                    execution_id: pending.execution_id().clone(),
                                    id: pending.id().clone(),
                                    reason: PermissionCancellationReason::custom(
                                        CustomPermissionCancellationReason::new(
                                            "withdraw",
                                            "caller withdrew",
                                        )
                                        .unwrap(),
                                    ),
                                    actor: close_action(),
                                })
                                .await
                                .map(|_| ())
                        } else {
                            agent
                                .answer_permission(PermissionAnswer {
                                    execution_id: pending.execution_id().clone(),
                                    id: pending.id().clone(),
                                    option_id: PermissionOptionId::new("allow").unwrap(),
                                    attribution: attribution(),
                                })
                                .await
                                .map(|_| ())
                                .map_err(|failure| failure.into_parts().0)
                        }
                    }
                };
                let (control_release, control_wait) = oneshot::channel();
                let mut operation = Some(control_operation);
                let admitted = if matches!(admission, Admission::BeforeContradiction) {
                    *backend.control_gate.lock().unwrap() = Some(control_wait);
                    let admitted = tokio::spawn(operation.take().unwrap());
                    backend.control_entered.notified().await;
                    Some(admitted)
                } else {
                    None
                };
                let invocation = tokio::spawn({
                    let agent = agent.clone();
                    async move { agent.invoke(input("execution"), actor()).await }
                });
                paused.await.unwrap();
                let result = if let Some(admitted) = admitted {
                    let result = admitted.await.unwrap();
                    // The suspended provider callback was discarded, never resumed.
                    assert!(control_release.is_closed());
                    result
                } else {
                    operation.unwrap().await
                };
                let control_calls = backend.controls.load(Ordering::SeqCst);
                resume.send(()).unwrap();
                let invocation_result = invocation.await.unwrap();
                let saved = storage.snapshot();
                assert_eq!(backend.closes.load(Ordering::SeqCst), 1);
                assert_eq!(
                    *backend.close_requests.lock().unwrap(),
                    vec![SessionCloseRequest::ExecutionFailed]
                );
                agent.close(close_action()).await.unwrap();
                assert_eq!(
                    control_calls,
                    usize::from(matches!(admission, Admission::BeforeContradiction)),
                    "{observation:?}, {control:?}, {admission:?}, returned={result:?}"
                );
                assert_eq!(result, Err(AgentError::Closed));
                assert!(matches!(
                    invocation_result,
                    Err(AgentError::ExecutionObservation { .. })
                ));
                assert_eq!(saved.invocations[0].events[0].update(), &update);
            }
        }
    }
}
