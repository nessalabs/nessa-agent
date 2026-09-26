//! Answer selection and delivery must survive lost callers and failed effects.
use super::*;
use crate::application::agent_execution::providers::ProviderOperationFuture;
use std::task::{Context, Waker};
use tokio::sync::oneshot;

#[derive(Default)]
struct AnswerAudit {
    records: Mutex<Vec<ExecutionAuditRecord>>,
    reject_call: Option<usize>,
    stall_call: Option<usize>,
    pause: Mutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>,
}
impl ExecutionAudit for AnswerAudit {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        Box::pin(async move {
            let call = {
                let mut records = self.records.lock().unwrap();
                records.push(record);
                records.len()
            };
            let pause = self.pause.lock().unwrap().take();
            if let Some((entered, release)) = pause {
                entered.send(()).unwrap();
                release.await.unwrap();
            }
            if self.stall_call == Some(call) {
                return std::future::pending().await;
            }
            if self.reject_call == Some(call) {
                return Err(AgentError::AuditFailure);
            }
            Ok(())
        })
    }
}
async fn fixture(
    mode: &str,
    audit: Arc<AnswerAudit>,
) -> (
    TempDir,
    OpenedProviderSession,
    tokio::task::JoinHandle<Result<ExecutionOutcome, AgentError>>,
    PermissionAnswer,
) {
    let (root, mut config, model) = test_acp_configuration(mode, 16);
    config.shutdown_grace = Duration::from_secs(2);
    config.execution_timeout = None;
    let binding =
        ClaudeAcpProvider::new(config, &model, TokenLimits::new(900, 100).unwrap(), audit).unwrap();
    let mut opened = binding.open(None).await.unwrap();
    let active = start(&opened, "write").await;
    next(&mut opened).await;
    let ExecutionUpdate::PermissionRequested { id, .. } = next(&mut opened).await else {
        panic!("expected review")
    };
    let answer = PermissionAnswer {
        execution_id: ExecutionId::new("write").unwrap(),
        id,
        option_id: PermissionOptionId::new("approve-one").unwrap(),
        attribution: attribution(),
    };
    (root, opened, active, answer)
}
fn answers(audit: &AnswerAudit) -> Vec<PermissionAnswerRecord> {
    audit
        .records
        .lock()
        .unwrap()
        .iter()
        .filter_map(|record| match record {
            ExecutionAuditRecord::SessionClosed(_) | ExecutionAuditRecord::Finished(_) => None,
            ExecutionAuditRecord::Answered(record) => Some(record.clone()),
            ExecutionAuditRecord::Cancelled(_) => {
                panic!("resolved answer must not be relabelled on cleanup")
            }
            ExecutionAuditRecord::QueueReordered(_) => None,
            ExecutionAuditRecord::ReviewDeclined(record) => {
                panic!("an offered review must not be refused unoffered: {record:?}")
            }
            ExecutionAuditRecord::QuestionAnswered(record) => {
                panic!("a review is not a question: {record:?}")
            }
            ExecutionAuditRecord::QuestionRefused(record) => {
                panic!("a review is not a question: {record:?}")
            }
        })
        .collect()
}

#[tokio::test]
async fn allow_and_deny_retain_selection_delivery_and_exact_attribution_once() {
    let _slot = process_test_slot().await;
    for allow in [true, false] {
        let audit = Arc::new(AnswerAudit::default());
        let (root, opened, active, mut answer) = fixture("permission", audit.clone()).await;
        if !allow {
            answer.option_id = PermissionOptionId::new("deny-one").unwrap();
        }
        let resolution = opened
            .session
            .answer_permission(answer.clone())
            .await
            .map_err(|failure| failure.into_error())
            .unwrap();
        assert_eq!(resolution.session_id(), opened.session.id());
        assert_eq!(active.await.unwrap().unwrap(), ExecutionOutcome::Completed);
        assert_eq!(
            opened
                .session
                .answer_permission(answer)
                .await
                .map_err(|failure| failure.into_error()),
            Err(AgentError::StalePermission)
        );
        opened
            .session
            .shutdown(SessionCloseRequest::Explicit(close_action()))
            .await
            .into_result()
            .unwrap();
        opened
            .session
            .shutdown(SessionCloseRequest::Explicit(close_action()))
            .await
            .into_result()
            .unwrap();
        let records = answers(&audit);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].delivery(), &PermissionAnswerDelivery::Selected);
        assert_eq!(records[1].delivery(), &PermissionAnswerDelivery::Written);
        for record in records {
            assert_eq!(record.session_id(), opened.session.id());
            assert_eq!(record.resolution(), &resolution);
            assert_eq!(record.resolution().attribution(), &attribution());
            assert!(matches!(
                record.resolution().request().state(),
                PermissionStateView::Answered { .. }
            ));
        }
        assert_eq!(root.path().join("fixture.txt").exists(), allow);
        assert_gone(&root, "pid");
    }
}

#[tokio::test]
async fn dropped_answer_wait_cannot_erase_admitted_selection_or_delivery() {
    let _slot = process_test_slot().await;
    let (entered, observing) = oneshot::channel();
    let (release, waiting) = oneshot::channel();
    let audit = Arc::new(AnswerAudit {
        pause: Mutex::new(Some((entered, waiting))),
        ..Default::default()
    });
    let (root, opened, active, answer) = fixture("permission", audit.clone()).await;
    let session = opened.session.clone();
    let caller = tokio::spawn(async move {
        session
            .answer_permission(answer)
            .await
            .map_err(|failure| failure.into_error())
    });
    observing.await.unwrap();
    assert!(!root.path().join("fixture.txt").exists());
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    release.send(()).unwrap();
    assert_eq!(active.await.unwrap().unwrap(), ExecutionOutcome::Completed);
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    let records = answers(&audit);
    assert_eq!(records.len(), 2);
    assert_eq!(records[1].delivery(), &PermissionAnswerDelivery::Written);
    assert!(root.path().join("fixture.txt").exists());
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn selection_and_delivery_audit_failures_are_visible_and_cleanup_still_runs() {
    let _slot = process_test_slot().await;
    for reject_call in [1, 2] {
        let audit = Arc::new(AnswerAudit {
            reject_call: Some(reject_call),
            ..Default::default()
        });
        let (root, opened, active, answer) = fixture("permission", audit.clone()).await;
        let failure = opened.session.answer_permission(answer).await.unwrap_err();
        assert_eq!(failure.error(), &AgentError::AuditFailure);
        assert_eq!(
            failure.permission_selection(),
            Some(PermissionSelectionState::Consumed)
        );
        assert_eq!(active.await.unwrap(), Err(AgentError::AuditFailure));
        assert_eq!(
            opened
                .session
                .shutdown(SessionCloseRequest::Explicit(close_action()))
                .await
                .into_result(),
            Err(AgentError::AuditFailure)
        );
        let records = answers(&audit);
        assert_eq!(records.len(), reject_call);
        assert_eq!(records[0].delivery(), &PermissionAnswerDelivery::Selected);
        if reject_call == 1 {
            assert!(!root.path().join("fixture.txt").exists());
        }
        assert_gone(&root, "pid");
    }
}

#[tokio::test]
async fn failed_answer_write_retains_selected_decision_and_uncertain_delivery() {
    let _slot = process_test_slot().await;
    let audit = Arc::new(AnswerAudit::default());
    let (root, opened, active, answer) = fixture("permission-write-failure", audit.clone()).await;
    let failure = opened.session.answer_permission(answer).await.unwrap_err();
    assert_eq!(
        failure.permission_selection(),
        Some(PermissionSelectionState::Consumed)
    );
    let failure = failure.into_error();
    assert!(matches!(failure, AgentError::Transport(_)));
    assert!(active.await.unwrap().is_err());
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    let records = answers(&audit);
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].delivery(), &PermissionAnswerDelivery::Selected);
    assert_eq!(
        records[1].delivery(),
        &PermissionAnswerDelivery::Failed(failure)
    );
    assert_eq!(records[0].resolution(), records[1].resolution());
    assert!(!root.path().join("fixture.txt").exists());
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn failed_answer_write_and_failed_delivery_audit_preserve_both_errors() {
    let _slot = process_test_slot().await;
    let audit = Arc::new(AnswerAudit {
        reject_call: Some(2),
        ..Default::default()
    });
    let (root, opened, active, answer) = fixture("permission-write-failure", audit.clone()).await;
    let failure = opened
        .session
        .answer_permission(answer)
        .await
        .map_err(|failure| failure.into_error())
        .unwrap_err();
    let AgentError::PermissionAnswerDeliveryAndAuditFailure {
        delivery_error,
        cleanup_error,
    } = &failure
    else {
        panic!("both failures required")
    };
    assert!(matches!(delivery_error.as_ref(), AgentError::Transport(_)));
    assert_eq!(cleanup_error, &None);
    let report = opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await;
    assert!(report.is_confirmed());
    assert_eq!(report.audit(), &Err(AgentError::AuditFailure));
    assert_eq!(report.operation_failure(), Some(&failure));
    let expected = report.into_result().unwrap_err();
    assert_eq!(active.await.unwrap(), Err(expected));
    assert_eq!(answers(&audit).len(), 2);
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn stalled_answer_audit_is_bounded_and_prevents_wire_effect() {
    let _slot = process_test_slot().await;
    let audit = Arc::new(AnswerAudit {
        stall_call: Some(1),
        ..Default::default()
    });
    let (root, opened, active, answer) = fixture("permission", audit.clone()).await;
    assert_eq!(
        timeout(
            Duration::from_secs(5),
            opened.session.answer_permission(answer)
        )
        .await
        .unwrap()
        .map_err(|failure| failure.into_error()),
        Err(AgentError::AuditFailure)
    );
    assert_eq!(active.await.unwrap(), Err(AgentError::AuditFailure));
    assert_eq!(
        opened
            .session
            .shutdown(SessionCloseRequest::Explicit(close_action()))
            .await
            .into_result(),
        Err(AgentError::AuditFailure)
    );
    assert_eq!(answers(&audit).len(), 1);
    assert!(!root.path().join("fixture.txt").exists());
    assert_gone(&root, "pid");
}

// On the current-thread runtime, this synchronous poll admits the command into
// the empty worker queue and reaches its reply wait. No worker task can dequeue
// it until we yield; dropping the future here closes the reply first.
fn abandon_admitted<T>(mut command: ProviderOperationFuture<'_, T>) {
    let mut context = Context::from_waker(Waker::noop());
    assert!(command.as_mut().poll(&mut context).is_pending());
    drop(command);
}

#[tokio::test(flavor = "current_thread")]
async fn answer_admitted_before_dispatch_survives_dropped_waiter() {
    let _slot = process_test_slot().await;
    let audit = Arc::new(AnswerAudit::default());
    let (root, opened, active, answer) = fixture("permission", audit.clone()).await;
    abandon_admitted(opened.session.answer_permission(answer.clone()));
    assert_eq!(
        timeout(Duration::from_secs(5), active)
            .await
            .unwrap()
            .unwrap(),
        Ok(ExecutionOutcome::Completed)
    );
    assert_eq!(
        opened
            .session
            .answer_permission(answer.clone())
            .await
            .map_err(|failure| failure.into_error()),
        Err(AgentError::StalePermission)
    );
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    let records = answers(&audit);
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].delivery(), &PermissionAnswerDelivery::Selected);
    assert_eq!(records[1].delivery(), &PermissionAnswerDelivery::Written);
    for record in records {
        assert_eq!(record.session_id(), opened.session.id());
        assert_eq!(record.resolution().request().id(), &answer.id);
        assert_eq!(
            record.resolution().request().execution_id(),
            &answer.execution_id
        );
        assert_eq!(record.resolution().attribution(), &answer.attribution);
    }
    assert!(root.path().join("fixture.txt").exists());
    assert_gone(&root, "pid");
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_admitted_before_dispatch_survives_dropped_waiter() {
    let _slot = process_test_slot().await;
    let audit = Arc::new(AnswerAudit::default());
    let (root, mut opened, active, answer) = fixture("permission-stop", audit.clone()).await;
    let request = PermissionCancellationRequest {
        execution_id: answer.execution_id.clone(),
        id: answer.id.clone(),
        reason: PermissionCancellationReason::custom(
            CustomPermissionCancellationReason::new("guard-policy", "Guard withdrew this request")
                .unwrap(),
        ),
        actor: close_action(),
    };
    abandon_admitted(opened.session.cancel_permission(request.clone()));
    let ExecutionUpdate::PermissionCancelled(cancellation) = next(&mut opened).await else {
        panic!("expected admitted cancellation");
    };
    assert_eq!(cancellation.request().id(), &request.id);
    assert_eq!(cancellation.request().execution_id(), &request.execution_id);
    assert_eq!(cancellation.session_id(), opened.session.id());
    assert_eq!(
        cancellation.origin(),
        &CancellationOrigin::Client(request.actor)
    );
    assert_eq!(
        cancellation.request().state(),
        PermissionStateView::Cancelled {
            reason: &request.reason
        }
    );
    assert_eq!(
        opened
            .session
            .answer_permission(answer)
            .await
            .map_err(|failure| failure.into_error()),
        Err(AgentError::StalePermission)
    );
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    assert_eq!(active.await.unwrap(), Ok(ExecutionOutcome::Cancelled));
    assert_eq!(
        audit
            .records
            .lock()
            .unwrap()
            .iter()
            .filter(|record| matches!(record, ExecutionAuditRecord::Cancelled(_)))
            .cloned()
            .collect::<Vec<_>>(),
        vec![ExecutionAuditRecord::Cancelled(cancellation)]
    );
    let choice: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root.path().join("permission-outcome")).unwrap(),
    )
    .unwrap();
    assert_eq!(choice["outcome"], "cancelled");
    assert!(!root.path().join("fixture.txt").exists());
    assert_gone(&root, "pid");
}

#[tokio::test(flavor = "current_thread")]
async fn close_cannot_overtake_an_admitted_answer() {
    let _slot = process_test_slot().await;
    let audit = Arc::new(AnswerAudit::default());
    let (root, opened, active, answer) = fixture("permission", audit.clone()).await;
    let mut answering = opened.session.answer_permission(answer.clone());
    let mut closing = opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()));
    // Neither poll yields to the worker: its queue contains the answer when the
    // independent close watch becomes ready. This fixes the interleaving exactly.
    let mut context = Context::from_waker(Waker::noop());
    assert!(answering.as_mut().poll(&mut context).is_pending());
    assert!(closing.as_mut().poll(&mut context).is_pending());
    assert_eq!(
        opened
            .session
            .answer_permission(answer.clone())
            .await
            .map_err(|failure| failure.into_error()),
        Err(AgentError::Closed)
    );
    let resolution = timeout(Duration::from_secs(5), answering)
        .await
        .unwrap()
        .unwrap();
    timeout(Duration::from_secs(5), closing)
        .await
        .unwrap()
        .into_result()
        .unwrap();
    assert!(active.await.unwrap().is_ok());
    let records = answers(&audit);
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].resolution(), &resolution);
    assert_eq!(records[0].resolution().attribution(), &answer.attribution);
    assert_eq!(records[1].delivery(), &PermissionAnswerDelivery::Written);
    assert!(root.path().join("fixture.txt").exists());
    assert_gone(&root, "pid");
}

#[tokio::test(flavor = "current_thread")]
async fn close_cannot_replace_an_admitted_explicit_cancellation() {
    let _slot = process_test_slot().await;
    let audit = Arc::new(AnswerAudit::default());
    let (root, opened, active, answer) = fixture("permission-stop", audit.clone()).await;
    let request = PermissionCancellationRequest {
        execution_id: answer.execution_id,
        id: answer.id,
        reason: PermissionCancellationReason::custom(
            CustomPermissionCancellationReason::new("guard", "Guard withdrew permission").unwrap(),
        ),
        actor: ActionContext::new("guard", "policy", "withdraw").unwrap(),
    };
    let mut cancelling = opened.session.cancel_permission(request.clone());
    let mut closing = opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()));
    let mut context = Context::from_waker(Waker::noop());
    assert!(cancelling.as_mut().poll(&mut context).is_pending());
    assert!(closing.as_mut().poll(&mut context).is_pending());
    assert_eq!(
        opened
            .session
            .cancel_permission(request.clone())
            .await
            .map_err(|failure| failure.into_error()),
        Err(AgentError::Closed)
    );
    let record = timeout(Duration::from_secs(5), cancelling)
        .await
        .unwrap()
        .unwrap();
    timeout(Duration::from_secs(5), closing)
        .await
        .unwrap()
        .into_result()
        .unwrap();
    assert_eq!(active.await.unwrap(), Ok(ExecutionOutcome::Cancelled));
    assert_eq!(
        record.request().state(),
        PermissionStateView::Cancelled {
            reason: &request.reason
        }
    );
    assert_eq!(record.origin(), &CancellationOrigin::Client(request.actor));
    let records = audit.records.lock().unwrap();
    assert_eq!(
        records
            .iter()
            .filter(|item| matches!(item, ExecutionAuditRecord::Cancelled(_)))
            .collect::<Vec<_>>(),
        vec![&ExecutionAuditRecord::Cancelled(record)]
    );
    assert!(!root.path().join("fixture.txt").exists());
    assert_gone(&root, "pid");
}

#[tokio::test(flavor = "current_thread")]
async fn close_drains_later_admitted_commands_after_audit_failure() {
    let _slot = process_test_slot().await;
    let audit = Arc::new(AnswerAudit {
        reject_call: Some(1),
        ..Default::default()
    });
    let (root, opened, active, answer) = fixture("permission", audit.clone()).await;
    let request = PermissionCancellationRequest {
        execution_id: answer.execution_id.clone(),
        id: answer.id.clone(),
        reason: PermissionCancellationReason::custom(
            CustomPermissionCancellationReason::new("guard.withdraw", "Guard withdrew the review")
                .unwrap(),
        ),
        actor: close_action(),
    };
    let mut answering = opened.session.answer_permission(answer);
    let mut cancelling = opened.session.cancel_permission(request);
    let mut closing = opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()));
    let mut context = Context::from_waker(Waker::noop());
    assert!(answering.as_mut().poll(&mut context).is_pending());
    assert!(cancelling.as_mut().poll(&mut context).is_pending());
    assert!(closing.as_mut().poll(&mut context).is_pending());
    assert_eq!(
        answering.await.map_err(|failure| failure.into_error()),
        Err(AgentError::AuditFailure)
    );
    // The second command is evaluated against the selected answer, not abandoned
    // behind the failed audit or rewritten as a lifecycle close cancellation.
    assert_eq!(
        cancelling.await.map_err(|failure| failure.into_error()),
        Err(AgentError::StalePermission)
    );
    assert_eq!(
        timeout(Duration::from_secs(5), closing)
            .await
            .unwrap()
            .into_result(),
        Err(AgentError::AuditFailure)
    );
    assert_eq!(active.await.unwrap(), Err(AgentError::AuditFailure));
    assert_eq!(answers(&audit).len(), 1);
    assert!(!root.path().join("fixture.txt").exists());
    assert_gone(&root, "pid");
}

#[tokio::test(flavor = "current_thread")]
async fn failed_answer_drains_a_distinct_admitted_cancellation_before_bulk_close() {
    let _slot = process_test_slot().await;
    for explicit_close in [false, true] {
        let audit = Arc::new(AnswerAudit {
            reject_call: Some(1),
            ..Default::default()
        });
        let (root, mut config, model) = test_acp_configuration("permission-pair", 16);
        config.execution_timeout = None;
        let binding = ClaudeAcpProvider::new(
            config,
            &model,
            TokenLimits::new(900, 100).unwrap(),
            audit.clone(),
        )
        .unwrap();
        let mut opened = binding.open(None).await.unwrap();
        let active = start(&opened, "write").await;
        assert!(matches!(next(&mut opened).await, ExecutionUpdate::Tool(_)));
        let mut ids = Vec::new();
        for _ in 0..2 {
            let ExecutionUpdate::PermissionRequested { id, .. } = next(&mut opened).await else {
                panic!("expected distinct pending review");
            };
            ids.push(id);
        }
        assert_ne!(ids[0], ids[1]);
        let answer = PermissionAnswer {
            execution_id: ExecutionId::new("write").unwrap(),
            id: ids[0].clone(),
            option_id: PermissionOptionId::new("deny-one").unwrap(),
            attribution: attribution(),
        };
        let cancellation = PermissionCancellationRequest {
            execution_id: answer.execution_id.clone(),
            id: ids[1].clone(),
            reason: PermissionCancellationReason::custom(
                CustomPermissionCancellationReason::new(
                    "guard.withdraw",
                    "Guard withdrew second review",
                )
                .unwrap(),
            ),
            actor: close_action(),
        };
        let mut answering = opened.session.answer_permission(answer.clone());
        let mut cancelling = opened.session.cancel_permission(cancellation.clone());
        let mut stale = opened.session.answer_permission(answer.clone());
        let mut closing = opened
            .session
            .shutdown(SessionCloseRequest::Explicit(close_action()));
        // Seal the exact admission order before the worker can dispatch anything.
        let mut context = Context::from_waker(Waker::noop());
        assert!(answering.as_mut().poll(&mut context).is_pending());
        assert!(cancelling.as_mut().poll(&mut context).is_pending());
        assert!(stale.as_mut().poll(&mut context).is_pending());
        if explicit_close {
            assert!(closing.as_mut().poll(&mut context).is_pending());
        }
        assert_eq!(
            answering.await.map_err(|failure| failure.into_error()),
            Err(AgentError::AuditFailure)
        );
        let recorded = cancelling
            .await
            .expect("admitted second review must retain its actor");
        assert_eq!(recorded.request().id(), &cancellation.id);
        assert_eq!(
            recorded.request().execution_id(),
            &cancellation.execution_id
        );
        assert_eq!(
            recorded.origin(),
            &CancellationOrigin::Client(cancellation.actor)
        );
        assert_eq!(
            recorded.request().state(),
            PermissionStateView::Cancelled {
                reason: &cancellation.reason
            }
        );
        assert_eq!(
            stale.await.map_err(|failure| failure.into_error()),
            Err(AgentError::StalePermission)
        );
        assert_eq!(active.await.unwrap(), Err(AgentError::AuditFailure));
        assert_eq!(closing.await.into_result(), Err(AgentError::AuditFailure));
        let records = audit.records.lock().unwrap();
        assert_eq!(
            records
                .iter()
                .filter(|record| matches!(record, ExecutionAuditRecord::Cancelled(_)))
                .count(),
            1
        );
        assert!(records.contains(&ExecutionAuditRecord::Cancelled(recorded)));
        let ExecutionAuditRecord::SessionClosed(closure) = records
            .iter()
            .find(|record| matches!(record, ExecutionAuditRecord::SessionClosed(_)))
            .unwrap()
        else {
            unreachable!()
        };
        assert_eq!(
            closure.closure().reason(),
            &if explicit_close {
                PermissionCancellationReason::session_closed()
            } else {
                PermissionCancellationReason::execution_failed()
            }
        );
        assert_eq!(
            closure.origin(),
            &if explicit_close {
                CancellationOrigin::Client(close_action())
            } else {
                CancellationOrigin::Runtime
            }
        );
        assert_gone(&root, "pid");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn admitted_answer_failures_retain_both_orders_and_confirmed_process_cleanup() {
    let _slot = process_test_slot().await;
    for (explicit_close, reject_call) in [false, true]
        .into_iter()
        .flat_map(|close| [1, 3].map(|call| (close, call)))
    {
        let audit = Arc::new(AnswerAudit {
            // Fail the first or second selection, with the other answer's wire write failing.
            reject_call: Some(reject_call),
            ..Default::default()
        });
        let (root, mut config, model) = test_acp_configuration("permission-pair-write-failure", 16);
        config.execution_timeout = None;
        let binding = ClaudeAcpProvider::new(
            config,
            &model,
            TokenLimits::new(900, 100).unwrap(),
            audit.clone(),
        )
        .unwrap();
        let mut opened = binding.open(None).await.unwrap();
        let active = start(&opened, "write").await;
        assert!(matches!(next(&mut opened).await, ExecutionUpdate::Tool(_)));
        let mut decisions = Vec::new();
        for _ in 0..2 {
            let ExecutionUpdate::PermissionRequested { id, .. } = next(&mut opened).await else {
                panic!("expected distinct pending review");
            };
            decisions.push(PermissionAnswer {
                execution_id: ExecutionId::new("write").unwrap(),
                id,
                option_id: PermissionOptionId::new("deny-one").unwrap(),
                attribution: attribution(),
            });
        }
        let mut first = opened.session.answer_permission(decisions[0].clone());
        let mut second = opened.session.answer_permission(decisions[1].clone());
        let mut closing = opened
            .session
            .shutdown(SessionCloseRequest::Explicit(close_action()));
        let mut context = Context::from_waker(Waker::noop());
        assert!(first.as_mut().poll(&mut context).is_pending());
        assert!(second.as_mut().poll(&mut context).is_pending());
        if explicit_close {
            assert!(closing.as_mut().poll(&mut context).is_pending());
        }
        let first_error = first
            .await
            .map_err(|failure| failure.into_error())
            .unwrap_err();
        let second_error = second
            .await
            .map_err(|failure| failure.into_error())
            .unwrap_err();
        let transport = if reject_call == 3 {
            first_error.clone()
        } else {
            second_error.clone()
        };
        assert!(matches!(transport, AgentError::Transport(_)));
        assert_eq!(
            if reject_call == 3 {
                &second_error
            } else {
                &first_error
            },
            &AgentError::AuditFailure
        );
        let combined = AgentError::MultipleOperationFailures {
            first_error: Box::new(first_error),
            subsequent_error: Box::new(second_error),
        };
        let combined = AgentError::OperationAndCleanupFailure {
            operation_error: Box::new(combined),
            cleanup_error: Box::new(AgentError::AuditFailure),
        };
        assert_eq!(active.await.unwrap(), Err(combined.clone()));
        let close_report = closing.await;
        assert!(close_report.is_confirmed());
        let close_error = close_report.into_result().unwrap_err();
        assert_eq!(close_error, combined);
        assert_gone(&root, "pid");
        let recorded = answers(&audit);
        assert_eq!(recorded.len(), 3);
        assert_eq!(recorded[0].resolution().request().id(), &decisions[0].id);
        assert_eq!(recorded[0].delivery(), &PermissionAnswerDelivery::Selected);
        assert_eq!(
            recorded[if reject_call == 3 { 1 } else { 2 }].delivery(),
            &PermissionAnswerDelivery::Failed(transport)
        );
        let second_selection = &recorded[if reject_call == 3 { 2 } else { 1 }];
        assert_eq!(
            second_selection.resolution().request().id(),
            &decisions[1].id
        );
        assert_eq!(
            second_selection.delivery(),
            &PermissionAnswerDelivery::Selected
        );
        assert_eq!(
            second_selection.resolution().attribution(),
            &decisions[1].attribution
        );
        assert_gone(&root, "pid");
    }
}
