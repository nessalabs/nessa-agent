//! Invalid observations must not erase settlement released by provider cleanup.
use super::{actor, capabilities, request, MemoryStorage};
use crate::application::agent_execution::support::*;
use nessa_sdk::application::agent_execution::providers::HarnessHostIdentity;
use nessa_sdk::application::agent_execution::providers::NativeExecutionHandoff;
use std::{sync::atomic::AtomicBool, time::Duration};
use tokio::sync::{mpsc, watch};

struct SettlementProvider(Arc<SettlementBackend>);
struct SettlementBackend {
    events: mpsc::UnboundedSender<Result<ExecutionEvent, ObservationFailure>>,
    receiver: Mutex<Option<mpsc::UnboundedReceiver<Result<ExecutionEvent, ObservationFailure>>>>,
    closed: watch::Sender<bool>,
    outcome: Result<ExecutionOutcome, AgentError>,
    cleanup_report: Mutex<CleanupReport>,
    release_on_close: AtomicBool,
    delayed_settlement: bool,
    reader_error: Option<ObservationFailure>,
    closes: Mutex<Vec<SessionCloseRequest>>,
}
struct SettlementEvents(mpsc::UnboundedReceiver<Result<ExecutionEvent, ObservationFailure>>);
impl ExecutionEventStream for SettlementEvents {
    fn next(&mut self) -> ProviderObservationFuture<'_> {
        Box::pin(async {
            match self.0.recv().await {
                Some(Ok(event)) => Ok(Some(event)),
                Some(Err(error)) => Err(error),
                None => Ok(None),
            }
        })
    }
}
impl AgentProvider for SettlementProvider {
    fn harness_host_identity(&self) -> HarnessHostIdentity<'_> {
        HarnessHostIdentity::local()
    }
    fn identity(&self) -> ProviderIdentity {
        ProviderIdentity::new("observation-settlement", "fixture", "test").unwrap()
    }
    fn capabilities(&self) -> &EffectiveCapabilities {
        capabilities_ref()
    }
    fn open(&self, _request: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        Box::pin(async {
            Ok(OpenedProviderSession {
                session: ProviderSession::new(
                    ExecutionSessionId::new("settlement-context").unwrap(),
                    self.0.clone(),
                    capabilities(),
                ),
                events: Box::new(SettlementEvents(
                    self.0.receiver.lock().unwrap().take().unwrap(),
                )),
            })
        })
    }
}
impl ProviderSessionBackend for SettlementBackend {
    fn prepare_invocation(&self) -> ProviderOperationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn execute(
        &self,
        input: ExecutionRequest,
        handoff: NativeExecutionHandoff,
    ) -> ProviderExecutionFuture<'_> {
        Box::pin(async move {
            if let Err(error) = handoff() {
                return ProviderExecutionReply::Rejected(error);
            }

            let result: Result<ExecutionOutcome, AgentError> = {
                async move {
                    let terminal = ExecutionEvent::new(
                        input.execution_id,
                        ExecutionUpdate::Finished(ExecutionOutcome::Completed),
                    );
                    self.events.send(Ok(terminal.clone())).unwrap();
                    self.events
                        .send(match &self.reader_error {
                            Some(error) => Err(error.clone()),
                            None => Ok(terminal),
                        })
                        .unwrap();
                    let mut closed = self.closed.subscribe();
                    while !*closed.borrow_and_update() {
                        closed.changed().await.map_err(|_| AgentError::Closed)?;
                    }
                    if self.delayed_settlement {
                        // Confirmed cleanup may release a future that still needs another
                        // poll. One opportunistic poll is insufficient for this contract.
                        tokio::task::yield_now().await;
                    }
                    self.outcome.clone()
                }
            }
            .await;
            ProviderExecutionReply::Finished(ExecutionReport::new(
                Some(result),
                None,
                ProviderSessionState::Usable,
            ))
        })
    }
    fn answer_question(&self, _: QuestionAnswer) -> ProviderOperationFuture<'_, ()> {
        Box::pin(async {
            Err(ProviderOperationFailure::new(
                AgentError::Unsupported("this fixture asks nothing".into()),
                ProviderSessionState::Usable,
            ))
        })
    }
    fn answer_permission(
        &self,
        _: PermissionAnswer,
    ) -> ProviderOperationFuture<'_, PermissionResolution> {
        Box::pin(async {
            Err(ProviderOperationFailure::new(
                AgentError::StalePermission,
                ProviderSessionState::Usable,
            ))
        })
    }
    fn cancel_permission(
        &self,
        _: PermissionCancellationRequest,
    ) -> ProviderOperationFuture<'_, PermissionCancellation> {
        Box::pin(async {
            Err(ProviderOperationFailure::new(
                AgentError::StalePermission,
                ProviderSessionState::Usable,
            ))
        })
    }
    fn close(&self, request: SessionCloseRequest) -> CleanupFuture<'_> {
        Box::pin(async move {
            self.closes.lock().unwrap().push(request);
            if self.release_on_close.load(Ordering::SeqCst) {
                self.closed.send_replace(true);
            }
            self.cleanup_report.lock().unwrap().clone()
        })
    }
}

async fn assert_settlement_after_invalid_observation(
    queued: bool,
    cleanup_report: CleanupReport,
    outcome: Result<ExecutionOutcome, AgentError>,
    release_on_close: bool,
    reader_error: Option<ObservationFailure>,
) {
    let cleanup_error = cleanup_report.clone().into_result().err();
    let storage = MemoryStorage::default();
    let (events, receiver) = mpsc::unbounded_channel();
    let backend = Arc::new(SettlementBackend {
        events,
        receiver: Mutex::new(Some(receiver)),
        closed: watch::channel(false).0,
        outcome: outcome.clone(),
        cleanup_report: Mutex::new(cleanup_report.clone()),
        release_on_close: AtomicBool::new(release_on_close),
        delayed_settlement: cleanup_report.is_confirmed(),
        reader_error: reader_error.clone(),
        closes: Mutex::new(Vec::new()),
    });
    let agent = attached_agent(
        Arc::new(SettlementProvider(backend.clone())),
        storage.manager().await,
    )
    .await
    .unwrap();
    let executing = async {
        if queued {
            agent
                .enqueue(request("duplicate"), Submitter::owner(actor()))
                .await
                .unwrap()
                .wait()
                .await
        } else {
            agent
                .invoke(request("duplicate"), Submitter::owner(actor()))
                .await
        }
    };
    let result = tokio::time::timeout(Duration::from_secs(2), executing)
        .await
        .expect("uncertain cleanup cannot wait forever for unavailable settlement");
    let close_request = if reader_error
        .as_ref()
        .is_some_and(|failure| failure.cause() == ObservationFailureCause::DeadlineExceeded)
    {
        SessionCloseRequest::DeadlineExceeded
    } else {
        SessionCloseRequest::ExecutionFailed
    };
    let protocol = reader_error
        .map(ObservationFailure::into_error)
        .unwrap_or_else(|| AgentError::Protocol("duplicate terminal observation".into()));
    let error = match cleanup_error {
        Some(cleanup_error) => AgentError::OperationAndCleanupFailure {
            operation_error: Box::new(protocol),
            cleanup_error: Box::new(cleanup_error),
        },
        None => protocol,
    };
    let expected = Err(AgentError::ExecutionObservation {
        error: Box::new(error),
        execution_result: release_on_close.then(|| Box::new(outcome)),
    });
    assert_eq!(result, expected);
    let snapshot = storage.snapshot();
    assert_eq!(snapshot.invocations[0].result, Some(expected));
    assert_eq!(
        snapshot.invocations[0].events.len(),
        1,
        "only the first terminal observation is retained"
    );
    assert_eq!(
        snapshot.invocations[0].events[0].update(),
        &ExecutionUpdate::Finished(ExecutionOutcome::Completed)
    );
    assert_eq!(*backend.closes.lock().unwrap(), vec![close_request]);
    // End the fixture with confirmed cleanup even when the tested attempt was
    // deliberately uncertain, so no runtime-drop cleanup task is left pending.
    *backend.cleanup_report.lock().unwrap() =
        CleanupReport::confirmed(CloseOutcome { forced: false });
    backend.release_on_close.store(true, Ordering::SeqCst);
    let expected_close = if cleanup_report.is_confirmed() {
        cleanup_report.into_result()
    } else {
        Ok(CloseOutcome { forced: false })
    };
    assert_eq!(agent.close(actor()).await, expected_close);
    assert_eq!(agent.close(actor()).await, expected_close);
}

#[tokio::test]
async fn duplicate_terminal_retains_settlement_after_confirmed_or_audit_only_cleanup() {
    for queued in [false, true] {
        for cleanup in [
            CleanupReport::confirmed(CloseOutcome { forced: false }),
            cleaned_with_error(AgentError::AuditFailure),
            cleaned_with_error(AgentError::OperationAndCleanupFailure {
                operation_error: Box::new(AgentError::Provider {
                    code: -32044,
                    diagnostic: None,
                }),
                cleanup_error: Box::new(AgentError::AuditFailure),
            }),
        ] {
            for outcome in [
                Ok(ExecutionOutcome::Completed),
                Err(AgentError::Provider {
                    code: -32042,
                    diagnostic: None,
                }),
            ] {
                assert_settlement_after_invalid_observation(
                    queued,
                    cleanup.clone(),
                    outcome,
                    true,
                    None,
                )
                .await;
            }
        }
    }
}

#[tokio::test]
async fn uncertain_cleanup_captures_ready_settlement_without_waiting_for_unavailable_result() {
    for queued in [false, true] {
        for ready in [false, true] {
            assert_settlement_after_invalid_observation(
                queued,
                CleanupReport::unconfirmed(AgentError::CleanupUncertain),
                Err(AgentError::Provider {
                    code: -32043,
                    diagnostic: None,
                }),
                ready,
                None,
            )
            .await;
        }
    }
}

#[tokio::test]
async fn reader_deadline_retains_audit_failure_and_delayed_provider_settlement() {
    for queued in [false, true] {
        assert_settlement_after_invalid_observation(
            queued,
            cleaned_with_error(AgentError::AuditFailure),
            Err(AgentError::Provider {
                code: -32045,
                diagnostic: None,
            }),
            true,
            Some(ObservationFailure::new(
                AgentError::Deadline,
                ObservationFailureCause::DeadlineExceeded,
            )),
        )
        .await;
    }
}

struct PolicyStopAudit {
    records: Mutex<Vec<ExecutionAuditRecord>>,
}
impl ExecutionAudit for PolicyStopAudit {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        Box::pin(async move {
            self.records.lock().unwrap().push(record);
            Ok(())
        })
    }
}
struct PolicyStopProvider(Arc<PolicyStopBackend>);
struct PolicyStopBackend {
    events: mpsc::UnboundedSender<Result<ExecutionEvent, ObservationFailure>>,
    receiver: Mutex<Option<mpsc::UnboundedReceiver<Result<ExecutionEvent, ObservationFailure>>>>,
    closes: Mutex<Vec<SessionCloseRequest>>,
    report: ExecutionReport,
    terminal: ExecutionOutcome,
}
struct PolicyStopEvents(mpsc::UnboundedReceiver<Result<ExecutionEvent, ObservationFailure>>);
impl ExecutionEventStream for PolicyStopEvents {
    fn next(&mut self) -> ProviderObservationFuture<'_> {
        Box::pin(async {
            match self.0.recv().await {
                Some(Ok(event)) => Ok(Some(event)),
                Some(Err(error)) => Err(error),
                None => Ok(None),
            }
        })
    }
}
impl AgentProvider for PolicyStopProvider {
    fn harness_host_identity(&self) -> HarnessHostIdentity<'_> {
        HarnessHostIdentity::local()
    }
    fn identity(&self) -> ProviderIdentity {
        ProviderIdentity::new("policy-stop", "fixture", "test").unwrap()
    }
    fn capabilities(&self) -> &EffectiveCapabilities {
        capabilities_ref()
    }
    fn open(&self, _request: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        Box::pin(async {
            Ok(OpenedProviderSession {
                session: ProviderSession::new(
                    ExecutionSessionId::new("policy-stop").unwrap(),
                    self.0.clone(),
                    capabilities(),
                ),
                events: Box::new(PolicyStopEvents(
                    self.0.receiver.lock().unwrap().take().unwrap(),
                )),
            })
        })
    }
}
impl ProviderSessionBackend for PolicyStopBackend {
    fn prepare_invocation(&self) -> ProviderOperationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn execute(
        &self,
        input: ExecutionRequest,
        handoff: NativeExecutionHandoff,
    ) -> ProviderExecutionFuture<'_> {
        Box::pin(async move {
            if let Err(error) = handoff() {
                return ProviderExecutionReply::Rejected(error);
            }

            self.events
                .send(Ok(ExecutionEvent::new(
                    input.execution_id,
                    ExecutionUpdate::Finished(self.terminal),
                )))
                .unwrap();
            ProviderExecutionReply::Finished(self.report.clone())
        })
    }
    fn answer_question(&self, _: QuestionAnswer) -> ProviderOperationFuture<'_, ()> {
        Box::pin(async {
            Err(ProviderOperationFailure::new(
                AgentError::Unsupported("this fixture asks nothing".into()),
                ProviderSessionState::Usable,
            ))
        })
    }
    fn answer_permission(
        &self,
        _: PermissionAnswer,
    ) -> ProviderOperationFuture<'_, PermissionResolution> {
        Box::pin(async {
            Err(ProviderOperationFailure::new(
                AgentError::StalePermission,
                ProviderSessionState::Usable,
            ))
        })
    }
    fn cancel_permission(
        &self,
        _: PermissionCancellationRequest,
    ) -> ProviderOperationFuture<'_, PermissionCancellation> {
        Box::pin(async {
            Err(ProviderOperationFailure::new(
                AgentError::StalePermission,
                ProviderSessionState::Usable,
            ))
        })
    }
    fn close(&self, request: SessionCloseRequest) -> CleanupFuture<'_> {
        Box::pin(async move {
            self.closes.lock().unwrap().push(request);
            CleanupReport::confirmed(CloseOutcome { forced: false })
        })
    }
}

async fn assert_cleanup_close(
    name: &str,
    report: ExecutionReport,
    terminal: ExecutionOutcome,
    expected_result: Result<ExecutionOutcome, AgentError>,
    expected_close: SessionCloseRequest,
) {
    let storage = MemoryStorage::default();
    let (events, receiver) = mpsc::unbounded_channel();
    let backend = Arc::new(PolicyStopBackend {
        events,
        receiver: Mutex::new(Some(receiver)),
        closes: Mutex::new(Vec::new()),
        report: report.clone(),
        terminal,
    });
    let audit = Arc::new(PolicyStopAudit {
        records: Mutex::new(Vec::new()),
    });
    let agent = Agent::prepare(
        Arc::new(PolicyStopProvider(backend.clone())),
        storage.manager().await,
        audit.clone(),
    )
    .await
    .map_err(|error| error.cause().clone())
    .unwrap();
    let authorization = agent
        .authorize_attachment(AttachmentRequest::CallerRequested(close_action()))
        .unwrap();
    agent
        .start_attachment(authorization)
        .unwrap()
        .wait()
        .await
        .unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        agent.invoke(request(name), Submitter::owner(actor())),
    )
    .await
    .unwrap_or_else(|_| panic!("{name} did not settle"));
    assert_eq!(result, expected_result);
    assert_eq!(backend.closes.lock().unwrap().as_slice(), [expected_close]);
    let snapshot = storage.snapshot();
    assert_eq!(snapshot.invocations[0].result, Some(expected_result));
    assert_eq!(
        snapshot.invocations[0].provider_report.as_ref(),
        Some(&report)
    );
    let records = audit.records.lock().unwrap().clone();
    assert_no_failure_close(&records, &snapshot);
}

fn is_failure_close(reason: &PermissionCancellationReason) -> bool {
    matches!(
        reason.view(),
        PermissionCancellationReasonView::ExecutionFailed
            | PermissionCancellationReasonView::SessionFailed
    )
}

fn assert_no_failure_close(records: &[ExecutionAuditRecord], snapshot: &SessionSnapshot) {
    for record in records {
        match record {
            ExecutionAuditRecord::SessionClosed(closed) => {
                assert!(!is_failure_close(closed.closure().reason()), "{closed:?}");
            }
            ExecutionAuditRecord::Cancelled(cancelled) => {
                if let PermissionStateView::Cancelled { reason } = cancelled.request().state() {
                    assert!(!is_failure_close(reason), "{cancelled:?}");
                }
            }
            _ => {}
        }
    }
    for invocation in &snapshot.invocations {
        for event in &invocation.scheduling {
            assert_ne!(event.cause, SchedulingCause::ExecutionFailed, "{event:?}");
        }
        for stop in invocation
            .cancellation
            .iter()
            .chain(invocation.local_cancellation.iter())
        {
            assert_ne!(stop.cause, SchedulingCause::ExecutionFailed, "{stop:?}");
        }
    }
}

#[tokio::test]
async fn a_policy_stop_closes_once_as_a_policy_refusal() {
    assert_cleanup_close(
        "policy-stop",
        ExecutionReport::stopped_by_policy(ProviderSessionState::CleanupRequired),
        ExecutionOutcome::Cancelled,
        Ok(ExecutionOutcome::Cancelled),
        SessionCloseRequest::PolicyRefusal,
    )
    .await;
}

#[tokio::test]
async fn a_malformed_end_turn_reply_closes_as_an_execution_failure() {
    let report = ExecutionReport::new(
        None,
        Some(AgentError::Protocol("missing prompt result".into())),
        ProviderSessionState::CleanupRequired,
    );
    assert_cleanup_close(
        "malformed-end-turn",
        report,
        ExecutionOutcome::Cancelled,
        Err(AgentError::Protocol("missing prompt result".into())),
        SessionCloseRequest::ExecutionFailed,
    )
    .await;
}

#[tokio::test]
async fn a_missed_stop_grace_closes_once_as_a_deadline() {
    let report = ExecutionReport::new(
        None,
        Some(AgentError::Deadline),
        ProviderSessionState::CleanupRequired,
    );
    assert_cleanup_close(
        "missed-grace",
        report,
        ExecutionOutcome::Cancelled,
        Err(AgentError::Deadline),
        SessionCloseRequest::DeadlineExceeded,
    )
    .await;
}
