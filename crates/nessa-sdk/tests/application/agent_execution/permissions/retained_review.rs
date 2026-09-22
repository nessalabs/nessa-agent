//! Valid receipts must still describe the exact review retained for the surface.
use super::*;

struct EvidenceBackend {
    answer: Option<PermissionResolution>,
    cancellation: Option<PermissionCancellation>,
    calls: AtomicUsize,
    closes: Mutex<Vec<SessionCloseRequest>>,
    cleanup: Mutex<CleanupReport>,
}
impl ProviderSessionBackend for EvidenceBackend {
    fn prepare_invocation(&self) -> ProviderOperationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn execute(&self, _: ExecutionRequest) -> ProviderExecutionFuture<'_> {
        Box::pin(async {
            ProviderExecutionReply::Rejected(AgentError::Unsupported("review fixture".into()))
        })
    }
    fn answer_permission(
        &self,
        _: PermissionAnswer,
    ) -> ProviderOperationFuture<'_, PermissionResolution> {
        Box::pin(async {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.answer.clone().unwrap())
        })
    }
    fn cancel_permission(
        &self,
        _: PermissionCancellationRequest,
    ) -> ProviderOperationFuture<'_, PermissionCancellation> {
        Box::pin(async {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.cancellation.clone().unwrap())
        })
    }
    fn close(&self, request: SessionCloseRequest) -> CleanupFuture<'_> {
        Box::pin(async move {
            self.closes.lock().unwrap().push(request);
            self.cleanup.lock().unwrap().clone()
        })
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReviewChange {
    Original,
    ToolId,
    OptionLabel,
    OptionEffect,
    OptionScope,
    RemovedOption,
    OptionOrder,
    InputName,
    InputArguments,
    MissingReview,
}

fn review(change: ReviewChange) -> (ExecutionController, ExecutionEvent) {
    let mut controller = ExecutionController::new(ExecutionSessionId::new("fixture").unwrap());
    let execution = ExecutionId::new("execution").unwrap();
    controller.begin_execution(execution.clone()).unwrap();
    let mut options = vec![
        PermissionOption::new(
            PermissionOptionId::new("allow").unwrap(),
            if change == ReviewChange::OptionLabel {
                "altered label"
            } else {
                "Allow"
            },
            PermissionDecision::new(
                if change == ReviewChange::OptionEffect {
                    PermissionEffect::Deny
                } else {
                    PermissionEffect::Allow
                },
                if change == ReviewChange::OptionScope {
                    PermissionScope::application(
                        PermissionApplicationId::new("different-application").unwrap(),
                    )
                } else {
                    PermissionScope::request()
                },
            ),
        )
        .unwrap(),
        PermissionOption::new(
            PermissionOptionId::new("deny").unwrap(),
            "Deny",
            PermissionDecision::new(PermissionEffect::Deny, PermissionScope::request()),
        )
        .unwrap(),
    ];
    if change == ReviewChange::RemovedOption {
        options.pop();
    }
    if change == ReviewChange::OptionOrder {
        options.reverse();
    }
    let mut input = review_input();
    if change == ReviewChange::InputName {
        input.name = "different tool input".into();
    }
    if change == ReviewChange::InputArguments {
        input.arguments_json = "{\"target\":\"other.txt\"}".into();
    }
    let mut decisions = Vec::new();
    for option in &options {
        if !decisions.contains(option.decision()) {
            decisions.push(option.decision().clone());
        }
    }
    let policy = PermissionOfferPolicy::new(decisions).unwrap();
    let event = controller
        .request_permission(
            &execution,
            PermissionId::new("permission").unwrap(),
            ToolCallUpdate::new(
                ToolCallId::new(if change == ReviewChange::ToolId {
                    "different-tool"
                } else {
                    "tool"
                })
                .unwrap(),
                None,
                None,
                None,
                None,
                None,
            ),
            input,
            PermissionOptions::new(options, &policy).unwrap(),
        )
        .unwrap();
    (controller, event)
}

#[tokio::test]
async fn complete_retained_review_is_required_for_answers_and_cancellations() {
    for cancel in [false, true] {
        for change in [
            ReviewChange::Original,
            ReviewChange::ToolId,
            ReviewChange::OptionLabel,
            ReviewChange::OptionEffect,
            ReviewChange::OptionScope,
            ReviewChange::RemovedOption,
            ReviewChange::OptionOrder,
            ReviewChange::InputName,
            ReviewChange::InputArguments,
            ReviewChange::MissingReview,
        ] {
            let (_, expected) = review(ReviewChange::Original);
            let (mut actual, _) = review(change);
            let answer = PermissionAnswer {
                execution_id: ExecutionId::new("execution").unwrap(),
                id: PermissionId::new("permission").unwrap(),
                option_id: PermissionOptionId::new("allow").unwrap(),
                attribution: attribution(),
            };
            let cancellation = PermissionCancellationRequest {
                execution_id: answer.execution_id.clone(),
                id: answer.id.clone(),
                reason: PermissionCancellationReason::custom(
                    CustomPermissionCancellationReason::new("withdraw", "caller withdrew").unwrap(),
                ),
                actor: close_action(),
            };
            let backend = Arc::new(EvidenceBackend {
                answer: if cancel {
                    None
                } else {
                    Some(actual.answer_permission(answer.clone()).unwrap())
                },
                cancellation: if cancel {
                    Some(actual.cancel_review(cancellation.clone()).unwrap())
                } else {
                    None
                },
                calls: AtomicUsize::new(0),
                closes: Mutex::new(Vec::new()),
                cleanup: Mutex::new(CleanupReport::confirmed(CloseOutcome { forced: false })),
            });
            let agent = if change == ReviewChange::MissingReview {
                provider_agent(backend.clone()).await
            } else {
                provider_agent_with_review(backend.clone(), expected).await
            };
            let before = agent
                .session_manager()
                .snapshot()
                .await
                .unwrap()
                .invocations;
            let result = if cancel {
                agent
                    .cancel_permission(cancellation.clone())
                    .await
                    .map(|_| ())
            } else {
                agent
                    .answer_permission(answer.clone())
                    .await
                    .map(|_| ())
                    .map_err(|failure| failure.into_parts().0)
            };
            if change == ReviewChange::Original {
                assert_eq!(result, Ok(()));
            } else {
                assert!(
                    matches!(&result, Err(AgentError::Protocol(_))),
                    "cancel={cancel}, change={change:?}: {result:?}"
                );
                let retry = if cancel {
                    agent.cancel_permission(cancellation).await.map(|_| ())
                } else {
                    agent
                        .answer_permission(answer)
                        .await
                        .map(|_| ())
                        .map_err(|failure| failure.into_parts().0)
                };
                assert_eq!(retry, Err(AgentError::Closed));
                assert_eq!(
                    backend.calls.load(Ordering::SeqCst),
                    1,
                    "contradiction must fence duplicate effects"
                );
            }
            let after = agent
                .session_manager()
                .snapshot()
                .await
                .unwrap()
                .invocations;
            assert_eq!(after.len(), before.len());
            for (after, before) in after.iter().zip(&before) {
                assert_eq!(
                    after.events, before.events,
                    "receipt must not replace the original review"
                );
            }
            agent.close(close_action()).await.unwrap();
            assert_eq!(
                *backend.closes.lock().unwrap(),
                vec![SessionCloseRequest::Explicit(close_action())]
            );
        }
    }
}

#[tokio::test]
async fn contradictory_receipt_never_hides_cleanup_or_audit_failure() {
    for cancel in [false, true] {
        for audit_failed in [false, true] {
            let (_, expected) = review(ReviewChange::Original);
            let (mut actual, _) = review(ReviewChange::ToolId);
            let answer = PermissionAnswer {
                execution_id: ExecutionId::new("execution").unwrap(),
                id: PermissionId::new("permission").unwrap(),
                option_id: PermissionOptionId::new("allow").unwrap(),
                attribution: attribution(),
            };
            let cancellation = PermissionCancellationRequest {
                execution_id: answer.execution_id.clone(),
                id: answer.id.clone(),
                reason: PermissionCancellationReason::custom(
                    CustomPermissionCancellationReason::new("withdraw", "caller withdrew").unwrap(),
                ),
                actor: close_action(),
            };
            let cleanup = if audit_failed {
                CleanupReport::new(
                    ResourceCleanup::Confirmed(CloseOutcome { forced: false }),
                    Err(AgentError::AuditFailure),
                )
            } else {
                CleanupReport::unconfirmed(AgentError::CleanupUncertain)
            };
            let backend = Arc::new(EvidenceBackend {
                answer: if cancel {
                    None
                } else {
                    Some(actual.answer_permission(answer.clone()).unwrap())
                },
                cancellation: if cancel {
                    Some(actual.cancel_review(cancellation.clone()).unwrap())
                } else {
                    None
                },
                calls: AtomicUsize::new(0),
                closes: Mutex::new(Vec::new()),
                cleanup: Mutex::new(cleanup),
            });
            let agent = provider_agent_with_review(backend.clone(), expected).await;
            let result = if cancel {
                agent.cancel_permission(cancellation).await.map(|_| ())
            } else {
                agent
                    .answer_permission(answer.clone())
                    .await
                    .map(|_| ())
                    .map_err(|failure| failure.into_parts().0)
            };
            assert!(matches!(result, Err(AgentError::Protocol(_))));
            assert_eq!(
                agent.close(close_action()).await,
                Err(if audit_failed {
                    AgentError::AuditFailure
                } else {
                    AgentError::CleanupUncertain
                })
            );
            let failure = agent.answer_permission(answer).await.unwrap_err();
            assert_eq!(failure.error(), &AgentError::Closed);
            assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
            assert_eq!(
                *backend.closes.lock().unwrap(),
                vec![SessionCloseRequest::Explicit(close_action())]
            );
            *backend.cleanup.lock().unwrap() =
                CleanupReport::confirmed(CloseOutcome { forced: false });
            if audit_failed {
                assert_eq!(
                    agent.close(close_action()).await,
                    Err(AgentError::AuditFailure)
                );
            } else {
                agent.close(close_action()).await.unwrap();
            }
        }
    }
}
