//! Received permission evidence remains owned while a saved observation blocks validation.
use super::*;
use crate::application::agent_execution::{
    executions::{ExecutionEvent, ExecutionUpdate},
    permissions::{ApprovalAttribution, ApprovalBasis, CancellationOrigin},
    sessions::{SessionSnapshot, SessionStorage, SessionStorageLease, StorageFuture},
    tools::ToolReviewInput,
};
use crate::domain::agent_execution::{
    permissions::{
        CustomPermissionCancellationReason, PermissionCancellationReason, PermissionDecision,
        PermissionEffect, PermissionId, PermissionOfferPolicy, PermissionOption,
        PermissionOptionId, PermissionOptions, PermissionRequest, PermissionScope,
    },
    sessions::SessionId,
    tools::{ToolCallId, ToolObservation},
};
use std::{
    future::{poll_fn, Future},
    task::Poll,
};

type SaveGate = Arc<StateMutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>>;
struct Storage {
    backing: InMemoryStorage,
    gate: SaveGate,
}
struct Lease {
    backing: Box<dyn SessionStorageLease>,
    gate: SaveGate,
}
impl SessionStorage for Storage {
    fn open(&self, id: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
        Box::pin(async {
            Ok(Box::new(Lease {
                backing: self.backing.open(id).await?,
                gate: self.gate.clone(),
            }) as Box<dyn SessionStorageLease>)
        })
    }
}
impl SessionStorageLease for Lease {
    fn load(&self) -> StorageFuture<'_, Option<SessionSnapshot>> {
        self.backing.load()
    }
    fn save(&self, snapshot: SessionSnapshot) -> StorageFuture<'_, ()> {
        Box::pin(async {
            let gate = self.gate.lock().unwrap().take();
            if let Some((entered, release)) = gate {
                entered.send(()).unwrap();
                release.await.unwrap();
            }
            self.backing.save(snapshot).await
        })
    }
    fn erase(&self) -> StorageFuture<'_, ()> {
        self.backing.erase()
    }
}

#[tokio::test(start_paused = true)]
async fn received_permission_receipt_finishes_validation_after_close() {
    for cancel in [false, true] {
        for valid in [false, true] {
            let gate = SaveGate::default();
            let manager = SessionManager::open(
                None,
                Arc::new(Storage {
                    backing: InMemoryStorage::new(),
                    gate: gate.clone(),
                }),
            )
            .await
            .unwrap();
            let backend = Arc::new(Backend::default());
            let agent = attached_agent(Arc::new(Provider(backend.clone())), manager)
                .await
                .unwrap();
            let request = input();
            let execution = request.execution_id.clone();
            agent.inner.manager.begin(request, actor()).await.unwrap();
            agent.inner.manager.begin_dispatch(&execution);
            let decision =
                PermissionDecision::new(PermissionEffect::Allow, PermissionScope::request());
            let option = PermissionOptionId::new("allow").unwrap();
            let options = PermissionOptions::new(
                vec![PermissionOption::new(option.clone(), "Allow", decision.clone()).unwrap()],
                &PermissionOfferPolicy::new(vec![decision]).unwrap(),
            )
            .unwrap();
            let id = PermissionId::new("review").unwrap();
            let tool = ToolCallId::new("tool").unwrap();
            let original = ToolReviewInput {
                name: "read".into(),
                arguments_json: "{}".into(),
            };
            let mut permission = PermissionRequest::new(
                id.clone(),
                execution.clone(),
                tool.clone(),
                options.clone(),
            );
            let receipt_input = if valid {
                original.clone()
            } else {
                ToolReviewInput {
                    name: "different".into(),
                    arguments_json: "{}".into(),
                }
            };
            let attribution = ApprovalAttribution::new(actor(), ApprovalBasis::Explicit);
            let reason = PermissionCancellationReason::custom(
                CustomPermissionCancellationReason::new("withdraw", "caller withdrew review")
                    .unwrap(),
            );
            if cancel {
                permission.cancel(reason.clone()).unwrap();
                *backend.cancellation.lock().unwrap() = Some(
                    PermissionCancellation::from_record(
                        ExecutionSessionId::new("handoff").unwrap(),
                        permission,
                        receipt_input,
                        CancellationOrigin::Client(actor()),
                    )
                    .unwrap(),
                );
            } else {
                permission.answer(&execution, &option).unwrap();
                *backend.answer.lock().unwrap() = Some(
                    PermissionResolution::new(
                        ExecutionSessionId::new("handoff").unwrap(),
                        permission,
                        receipt_input,
                        attribution.clone(),
                    )
                    .unwrap(),
                );
            }
            let (entered, saving) = oneshot::channel();
            let (release, waiting) = oneshot::channel();
            *gate.lock().unwrap() = Some((entered, waiting));
            let writer = tokio::spawn({
                let agent = agent.clone();
                let execution = execution.clone();
                async move {
                    agent
                        .inner
                        .manager
                        .event(ExecutionEvent::new(
                            execution,
                            ExecutionUpdate::PermissionRequested {
                                id: PermissionId::new("review").unwrap(),
                                tool_id: tool,
                                observation: ToolObservation::default(),
                                input: original,
                                options,
                            },
                        ))
                        .await
                }
            });
            timeout(Duration::from_secs(2), saving)
                .await
                .unwrap()
                .unwrap();
            let mut control = tokio::spawn({
                let agent = agent.clone();
                async move {
                    if cancel {
                        agent
                            .cancel_permission(PermissionCancellationRequest {
                                execution_id: execution,
                                id,
                                reason,
                                actor: actor(),
                            })
                            .await
                            .map(|_| ())
                    } else {
                        agent
                            .answer_permission(PermissionAnswer {
                                execution_id: execution,
                                id,
                                option_id: option,
                                attribution,
                            })
                            .await
                            .map(|_| ())
                            .map_err(|failure| failure.into_parts().0)
                    }
                }
            });
            timeout(Duration::from_secs(2), backend.receipt_returned.notified())
                .await
                .unwrap();
            let closing = agent.close(actor());
            tokio::pin!(closing);
            assert!(poll_fn(|cx| Poll::Ready(closing.as_mut().poll(cx)))
                .await
                .is_pending());
            // Let every ready task observe stop while storage still owns the
            // evidence lock. A received receipt must remain pending validation.
            assert!(timeout(Duration::from_millis(1), &mut control)
                .await
                .is_err());
            release.send(()).unwrap();
            writer.await.unwrap().unwrap();
            let result = timeout(Duration::from_secs(2), control)
                .await
                .unwrap()
                .unwrap();
            if valid {
                assert_eq!(result, Ok(()));
            } else {
                assert!(matches!(result, Err(AgentError::Protocol(_))), "{result:?}");
            }
            timeout(Duration::from_secs(2), closing)
                .await
                .unwrap()
                .unwrap();
        }
    }
}
