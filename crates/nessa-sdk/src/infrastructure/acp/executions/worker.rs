use super::super::{
    fields,
    permissions::wire as permission_wire,
    profile::AcpProfile,
    sessions::{
        binding::{Command, Completion},
        cleanup::ProcessCleanup,
        AcpConfig,
    },
};
use super::{
    event_queue::{EventSender, QueueError},
    failure::{execution_finish_failure, requested_close_reason, retain_admitted_failure},
    steering::{self, PendingSteering},
    wire,
};
use crate::application::agent_execution::agents::{
    AgentError, AgentStartupContext, AgentStartupPhase, AgentStartupStep,
};
use crate::application::agent_execution::executions::{
    ExecutionAudit, ExecutionAuditRecord, ExecutionController, ExecutionEvent, ExecutionUpdate,
};

use crate::application::agent_execution::permissions::{
    CancellationOrigin, PermissionAnswerDelivery, PermissionAnswerRecord, PermissionCancellation,
    PermissionResolution, PermissionSelectionState,
};
use crate::application::agent_execution::providers::{
    CleanupReport, ExecutionReport, ObservationFailureCause, OperationCapabilities,
    ProviderExecutionReply, ProviderOperationFailure, ProviderSessionState, ResourceCleanup,
    SessionCloseRequest, SteeringOutcome,
};
use crate::domain::agent_execution::executions::{
    ExecutionId, ExecutionOutcome, MessageChunk, MessageId,
};
use crate::domain::agent_execution::permissions::{
    PermissionCancellationReason, PermissionCancellationReasonView, PermissionId,
};
use crate::domain::agent_execution::sessions::ExecutionSessionId;
use crate::domain::effective_capabilities::value_objects::EffectiveCapabilities;
use crate::infrastructure::{
    json_rpc::{self, Envelope, Reader, RpcId},
    process::ProcessScope,
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    future::{poll_fn, Future},
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    task::Poll,
    time::Duration,
};
use tokio::{
    sync::{mpsc, oneshot, watch},
    time::{timeout, Instant},
};

type ExecutionReply = oneshot::Sender<ProviderExecutionReply>;
enum DispatchReadiness {
    Ready,
    Interrupted(AgentError),
}
struct ActiveExecution {
    id: i64,
    execution_id: ExecutionId,
    reply: ExecutionReply,
    deadline: Option<Instant>,
}
struct Worker<P> {
    profile: P,
    audit: Arc<dyn ExecutionAudit>,
    cancellation_cause: Option<(PermissionCancellationReason, CancellationOrigin)>,
    scope: ProcessScope,
    reader: Reader<tokio::process::ChildStdout>,
    config: AcpConfig,
    capabilities: EffectiveCapabilities,
    commands: mpsc::Receiver<Command>,
    close_requested: watch::Receiver<Option<SessionCloseRequest>>,
    events: EventSender,
    sequence: i64,
    permission_sequence: Arc<AtomicU64>,
    active: Option<ActiveExecution>,
    steering: Option<PendingSteering>,
    steering_supported: bool,
    operation_capabilities: watch::Sender<OperationCapabilities>,
    permissions: HashMap<PermissionId, RpcId>,
    shutdown_deadline: Option<Instant>,
    closing: bool,
    deferred_outcome: Option<ExecutionOutcome>,
    provider_result: Option<Result<ExecutionOutcome, AgentError>>,
    audit_failure: Option<AgentError>,
    failure_cause: ObservationFailureCause,
}

#[allow(clippy::too_many_arguments)]
pub(in crate::infrastructure::acp) async fn run<P: AcpProfile>(
    mut scope: ProcessScope,
    profile: P,
    config: AcpConfig,
    capabilities: EffectiveCapabilities,
    commands: mpsc::Receiver<Command>,
    close_requested: watch::Receiver<Option<SessionCloseRequest>>,
    finished: watch::Sender<Option<Completion>>,
    events: EventSender,
    ready: oneshot::Sender<Result<ExecutionSessionId, AgentError>>,
    audit: Arc<dyn ExecutionAudit>,
    restore: Option<ExecutionSessionId>,
    permission_sequence: Arc<AtomicU64>,
    operation_capabilities: watch::Sender<OperationCapabilities>,
    recovery: Arc<ProcessCleanup>,
) {
    let reader = Reader::new(
        scope.stdout.take().expect("owned stdout"),
        config.max_frame_bytes,
    );
    let mut worker = Worker {
        profile,
        audit,
        cancellation_cause: None,
        scope,
        reader,
        config,
        capabilities,
        commands,
        close_requested,
        events,
        sequence: 0,
        permission_sequence,
        active: None,
        steering: None,
        steering_supported: false,
        operation_capabilities,
        permissions: HashMap::new(),
        shutdown_deadline: None,
        closing: false,
        deferred_outcome: None,
        provider_result: None,
        audit_failure: None,
        failure_cause: ObservationFailureCause::ExecutionFailed,
    };
    let mut execution = None;
    let startup = catch_worker_panic(worker.startup(restore, &mut execution)).await;
    let mut ready = Some(ready);
    let result = match startup {
        Ok(()) => {
            let execution = execution.as_mut().expect("startup established a context");
            if ready
                .take()
                .expect("startup sender")
                .send(Ok(execution.id().clone()))
                .is_err()
            {
                let request = worker
                    .close_requested
                    .borrow()
                    .clone()
                    .unwrap_or(SessionCloseRequest::SessionHandlesDropped);
                worker.cancellation_cause =
                    Some((requested_close_reason(&request, false), request.origin()));
                Err(AgentError::Closed)
            } else {
                catch_worker_panic(worker.drive(execution)).await
            }
        }
        Err(error) => Err(error),
    };
    // Worker, controller and reply ownership stay outside every caught phase.
    let mut execution_reply = None;
    let completed = catch_worker_panic(async {
        Ok(worker
            .finish(&mut execution, result, &mut execution_reply)
            .await)
    })
    .await;
    let WorkerResult {
        cleanup,
        settlement,
        failure: published_failure,
    } = match completed {
        Ok(completed) => completed,
        Err(error) => {
            worker.commands.close();
            let grace = worker
                .begin_shutdown_grace()
                .saturating_duration_since(Instant::now());
            let physical =
                catch_worker_panic(worker.scope.cleanup(grace, worker.config.kill_timeout)).await;
            if execution_reply.is_none() {
                execution_reply = worker.active.take().map(|active| active.reply);
            }
            let resources = match physical {
                Ok(outcome) => ResourceCleanup::Confirmed(outcome),
                Err(error) => ResourceCleanup::Unconfirmed(error),
            };
            let cleanup =
                CleanupReport::new(resources, worker.audit_failure.take().map_or(Ok(()), Err))
                    .with_operation_failure(Some(error.clone()));
            let settlement = ExecutionReport::new(
                worker.provider_result.take(),
                Some(error),
                ProviderSessionState::CleanupReported(cleanup.clone()),
            );
            WorkerResult {
                failure: settlement.clone().into_result().err(),
                cleanup,
                settlement,
            }
        }
    };
    if !cleanup.is_confirmed() {
        recovery.retain(worker.scope).await;
    }
    if let Some(ready) = ready {
        let _ = ready.send(Err(published_failure.clone().unwrap_or(AgentError::Closed)));
    }
    if let Some(pending) = worker.steering.take() {
        let _ = pending.reply.send(Err(ProviderOperationFailure::new(
            published_failure.clone().unwrap_or(AgentError::Closed),
            ProviderSessionState::CleanupReported(cleanup.clone()),
        )));
    }
    let _ = finished.send(Some(Completion {
        cleanup,
        failure: published_failure,
        failure_cause: worker.failure_cause,
    }));
    if let Some(reply) = execution_reply {
        let _ = reply.send(ProviderExecutionReply::Finished(settlement));
    }
}
// Startup budgets are shared by every step, so only the step that was waiting
// identifies what expired. Other failures keep their own typed meaning.
fn startup_deadline(
    error: AgentError,
    phase: AgentStartupPhase,
    context: AgentStartupContext,
) -> AgentError {
    match error {
        AgentError::Deadline => AgentError::StartupDeadline(AgentStartupStep::new(phase, context)),
        other => other,
    }
}
struct WorkerResult {
    cleanup: CleanupReport,
    settlement: ExecutionReport,
    failure: Option<AgentError>,
}

// Poll a borrowed worker phase so unwinding cannot take its process or receipts.
async fn catch_worker_panic<T>(
    operation: impl Future<Output = Result<T, AgentError>>,
) -> Result<T, AgentError> {
    let mut operation = Box::pin(operation);
    let result = poll_fn(|context| {
        match catch_unwind(AssertUnwindSafe(|| operation.as_mut().poll(context))) {
            Ok(poll) => poll,
            Err(_) => Poll::Ready(Err(AgentError::Protocol(
                "ACP worker phase panicked".into(),
            ))),
        }
    })
    .await;
    match catch_unwind(AssertUnwindSafe(|| drop(operation))) {
        Ok(()) => result,
        Err(_) => Err(AgentError::Protocol(
            "ACP worker phase drop panicked".into(),
        )),
    }
}

impl<P: AcpProfile> Worker<P> {
    async fn finish(
        &mut self,
        execution: &mut Option<ExecutionController>,
        result: Result<(), AgentError>,
        execution_reply: &mut Option<ExecutionReply>,
    ) -> WorkerResult {
        // No command may enter a generation that has left its drive loop. The final
        // execution acknowledgement can wake a different runtime thread immediately.
        self.commands.close();
        let local_cancellation = self.closing && result.is_ok() && self.provider_result.is_none();
        self.begin_shutdown_grace();
        let mut failure = result.err();
        if self.closing && (failure.is_none() || failure == Some(AgentError::AuditFailure)) {
            failure = match self
                .cancellation_cause
                .as_ref()
                .map(|(reason, _)| reason.view())
            {
                Some(PermissionCancellationReasonView::DeadlineExceeded) => {
                    self.failure_cause = ObservationFailureCause::DeadlineExceeded;
                    Some(AgentError::Deadline)
                }
                Some(
                    PermissionCancellationReasonView::ExecutionFailed
                    | PermissionCancellationReasonView::SessionFailed,
                ) => Some(AgentError::Closed),
                _ => failure,
            };
        }
        self.closing = true;
        if let Some(execution) = execution.as_mut() {
            let (reason, origin) = self
                .cancellation_cause
                .get_or_insert_with(|| {
                    let reason = if self.deferred_outcome.is_some() || local_cancellation {
                        PermissionCancellationReason::session_closed()
                    } else if execution.active_execution_id().is_some() {
                        PermissionCancellationReason::execution_failed()
                    } else {
                        PermissionCancellationReason::session_failed()
                    };
                    (reason, CancellationOrigin::Runtime)
                })
                .clone();
            if let Err(error) = self.drain_admitted_permissions(execution).await {
                failure = Some(retain_admitted_failure(failure, error));
            }
            // The drive loop may have already closed this same generation and then
            // accepted its final provider result. Keep that original audit evidence;
            // do not reinterpret its execution cause after settlement released the ID.
            let closure = if execution.is_closed() {
                Ok(Vec::new())
            } else {
                match self.active.as_ref() {
                    Some(active) if execution.active_execution_id().is_some() => {
                        execution.close_execution(&active.execution_id, reason, origin)
                    }
                    _ => execution.close(reason, origin),
                }
            };
            match closure {
                Ok(records) => {
                    if let Err(error) = self.record_lifecycle(records).await {
                        // Audit delivery has its own report field. Do not duplicate it
                        // into an already retained initiating operation failure.
                        if error != AgentError::AuditFailure || failure.is_none() {
                            failure = Some(retain_admitted_failure(failure, error));
                        }
                    }
                }
                Err(error) => failure = Some(retain_admitted_failure(failure, error)),
            }
            if let Err(error) = self.send_cancellation(execution.id().as_str()).await {
                tracing::debug!(%error, "cooperative cancellation failed; physical cleanup still runs");
            }
        }
        let grace = self
            .begin_shutdown_grace()
            .saturating_duration_since(Instant::now());
        let physical = self.scope.cleanup(grace, self.config.kill_timeout).await;
        if let (Some(active), Some(execution)) = (self.active.take(), execution.as_mut()) {
            *execution_reply = Some(active.reply);
            let reason = self.cancellation_cause.as_ref().map_or(
                PermissionCancellationReason::execution_failed(),
                |(reason, _)| reason.clone(),
            );
            let domain_result = if local_cancellation && physical.is_ok() {
                Ok(ExecutionOutcome::Cancelled)
            } else {
                self.provider_result
                    .clone()
                    .unwrap_or_else(|| Err(failure.clone().unwrap_or(AgentError::Closed)))
                    .map_err(|_| reason)
            };
            let event = domain_result
                .as_ref()
                .ok()
                .map(|outcome| execution.finished_event(&active.execution_id, *outcome));
            match execution.finish_execution(&active.execution_id, domain_result) {
                Ok(records) => {
                    if let Err(error) = self.record_lifecycle(records).await {
                        // Audit delivery has its own report field. Do not duplicate it
                        // into an already retained initiating operation failure.
                        if error != AgentError::AuditFailure || failure.is_none() {
                            failure = Some(retain_admitted_failure(failure, error));
                        }
                    }
                }
                Err(error) => {
                    if let Some(error) = execution_finish_failure(execution, failure.clone(), error)
                    {
                        failure = Some(error);
                    }
                }
            }
            if failure.is_none() && physical.is_ok() {
                if let Some(Err(error)) =
                    event.map(|event| event.and_then(|event| self.emit(event)))
                {
                    failure = Some(error);
                }
            }
        }
        let resources = match physical {
            Ok(outcome) => ResourceCleanup::Confirmed(outcome),
            Err(error) => ResourceCleanup::Unconfirmed(error),
        };
        let cleanup = CleanupReport::new(resources, self.audit_failure.take().map_or(Ok(()), Err))
            .with_operation_failure(failure.clone());
        let settlement = if local_cancellation && cleanup.is_confirmed() && failure.is_none() {
            ExecutionReport::cancelled_locally(cleanup.clone())
        } else {
            ExecutionReport::new(
                self.provider_result.take(),
                failure.clone(),
                ProviderSessionState::CleanupReported(cleanup.clone()),
            )
        };
        let published_failure = if execution_reply.is_some() {
            settlement.clone().into_result().err()
        } else {
            cleanup.clone().into_result().err().or(failure)
        };
        WorkerResult {
            cleanup,
            settlement,
            failure: published_failure,
        }
    }
    fn pending_id(&self) -> Result<i64, AgentError> {
        self.sequence
            .checked_add(1)
            .ok_or_else(|| json_rpc::protocol("request ID exhausted"))
    }
    fn next_id(&mut self) -> Result<i64, AgentError> {
        self.sequence = self.pending_id()?;
        Ok(self.sequence)
    }
    async fn send_encoded(
        &mut self,
        bytes: Vec<u8>,
        deadline: Option<Instant>,
    ) -> Result<(), AgentError> {
        let stdin = self.scope.stdin.as_mut().ok_or(AgentError::Closed)?;
        let result = json_rpc::send_encoded(stdin, &bytes, Duration::from_secs(1), deadline).await;
        if result == Err(AgentError::Deadline) && !self.closing {
            self.failure_cause = ObservationFailureCause::DeadlineExceeded;
            self.cancellation_cause.get_or_insert((
                PermissionCancellationReason::deadline_exceeded(),
                CancellationOrigin::Runtime,
            ));
        }
        result
    }
    fn begin_shutdown_grace(&mut self) -> Instant {
        *self
            .shutdown_deadline
            .get_or_insert_with(|| Instant::now() + self.config.shutdown_grace)
    }
    async fn send_cancellation(&mut self, session_id: &str) -> Result<(), AgentError> {
        let deadline = self.begin_shutdown_grace();
        match tokio::time::timeout_at(deadline, self.cancel_wire_permissions()).await {
            Err(_) => return Ok(()),
            Ok(Err(AgentError::Deadline)) if Instant::now() >= deadline => return Ok(()),
            Ok(result) => result?,
        }
        let frame = json_rpc::encode(
            json_rpc::notification("session/cancel", json!({"sessionId": session_id})),
            self.config.max_frame_bytes,
        )?;
        match self.send_encoded(frame, Some(deadline)).await {
            // Expiry means cooperative grace ended, not that explicit close failed.
            Err(AgentError::Deadline) if Instant::now() >= deadline => Ok(()),
            result => result,
        }
    }
    fn current_deadline(&self) -> Option<Instant> {
        self.shutdown_deadline.or_else(|| {
            self.active
                .as_ref()
                .and_then(|active| active.deadline)
                .into_iter()
                .chain(self.steering.as_ref().map(|pending| pending.deadline))
                .min()
        })
    }
    async fn send(&mut self, value: Value) -> Result<(), AgentError> {
        self.send_before(value, self.current_deadline()).await
    }
    async fn send_before(
        &mut self,
        value: Value,
        deadline: Option<Instant>,
    ) -> Result<(), AgentError> {
        let frame = json_rpc::encode(value, self.config.max_frame_bytes)?;
        self.send_encoded(frame, deadline).await
    }

    async fn rpc(
        &mut self,
        method: &str,
        params: Value,
        deadline: Instant,
        mut execution: Option<&mut ExecutionController>,
    ) -> Result<Value, AgentError> {
        let id = self.next_id()?;
        let frame = json_rpc::encode(
            json_rpc::request(id, method, params),
            self.config.max_frame_bytes,
        )?;
        self.send_encoded(frame, Some(deadline)).await?;
        loop {
            if let Some(request) = self.close_requested.borrow().clone() {
                self.cancellation_cause =
                    Some((requested_close_reason(&request, false), request.origin()));
                return Err(AgentError::Closed);
            }
            let message = tokio::select! { biased;
                _ = self.close_requested.changed() => {
                    let request = self.close_requested.borrow().clone()
                        .unwrap_or(SessionCloseRequest::SessionHandlesDropped);
                    self.cancellation_cause = Some((requested_close_reason(&request, false), request.origin()));
                    return Err(AgentError::Closed);
                },
                _ = wait_for_deadline(Some(deadline)) => { self.failure_cause = ObservationFailureCause::DeadlineExceeded; self.cancellation_cause = Some((PermissionCancellationReason::deadline_exceeded(), CancellationOrigin::Runtime)); return Err(AgentError::Deadline); },
                message = self.reader.next() => message?,
            };
            if let Some(method) = message.method {
                if let Some(id) = message.id {
                    let response = if method == "session/request_permission" {
                        permission_wire::permission_cancel(&id)
                    } else {
                        json_rpc::unsupported(&id)
                    };
                    let frame = json_rpc::encode(response, self.config.max_frame_bytes)?;
                    self.send_encoded(frame, Some(deadline)).await?;
                } else if method == "session/request_permission" {
                    return Err(json_rpc::protocol(
                        "permission request requires an RPC identifier",
                    ));
                } else if method == "session/update" {
                    let execution = execution.as_deref_mut().ok_or_else(|| {
                        json_rpc::protocol("session update before startup context admission")
                    })?;
                    // Startup configuration cannot hide drift that the live
                    // update path rejects. This also checks session correlation
                    // and refuses execution output before a prompt is active.
                    self.update(
                        execution,
                        message
                            .params
                            .ok_or_else(|| json_rpc::protocol("missing session update params"))?,
                    )?;
                }
                continue;
            }
            if message.id != Some(RpcId::Number(id)) {
                return Err(json_rpc::protocol("unexpected startup response"));
            }
            if let Some(error) = message.error {
                return Err(AgentError::Provider { code: error.code });
            }
            return message
                .result
                .ok_or_else(|| json_rpc::protocol("missing response result"));
        }
    }
    async fn startup(
        &mut self,
        restore: Option<ExecutionSessionId>,
        execution: &mut Option<ExecutionController>,
    ) -> Result<(), AgentError> {
        let deadline = Instant::now() + self.config.startup_timeout;
        // Whether saved context is being restored is decided before any step
        // runs, so every step's deadline reports it. Reading it off the step
        // would call a restoration that expired during `initialize` new.
        let context = if restore.is_some() {
            AgentStartupContext::Restored
        } else {
            AgentStartupContext::New
        };
        let init = self.rpc("initialize", json!({"protocolVersion":1,"clientInfo":{"name":"nessa-sdk","version":env!("CARGO_PKG_VERSION")},
            "clientCapabilities":{"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false}}), deadline, None)
            .await
            .map_err(|error| startup_deadline(error, AgentStartupPhase::Initialize, context))?;
        if init.get("protocolVersion").and_then(Value::as_u64) != Some(1) {
            return Err(json_rpc::protocol("requires ACP protocol 1"));
        }
        self.profile.validate_initialize(&init)?;
        self.steering_supported = self.profile.supports_steering(&init);
        let mut params = self
            .profile
            .new_session_params(&self.config, &self.capabilities);
        let method = if let Some(id) = &restore {
            if !init
                .pointer("/agentCapabilities/sessionCapabilities/resume")
                .is_some_and(Value::is_object)
            {
                return Err(AgentError::Unsupported(
                    "provider does not support restoring a closed session".into(),
                ));
            }
            params
                .as_object_mut()
                .ok_or_else(|| json_rpc::protocol("session parameters must be an object"))?
                .insert("sessionId".into(), json!(id.as_str()));
            "session/resume"
        } else {
            "session/new"
        };
        let result = self
            .rpc(method, params, deadline, None)
            .await
            .map_err(|error| startup_deadline(error, AgentStartupPhase::Session, context))?;
        let id = if let Some(id) = &restore {
            id.clone()
        } else {
            ExecutionSessionId::new(fields::identifier(&result, "sessionId")?)
                .map_err(|error| json_rpc::protocol(&error.to_string()))?
        };
        // Retain the known context before later configuration can fail. Teardown
        // must audit its closure even when startup never publishes ready.
        let execution = execution.insert(ExecutionController::new(id));
        // Resume identifies its target in the request. ACP does not require
        // repeating that identity in the response; reject a conflicting extension
        // without losing the local closure evidence for the requested context.
        if restore.is_some()
            && result.get("sessionId").is_some()
            && fields::identifier(&result, "sessionId")? != execution.id().as_str()
        {
            return Err(json_rpc::protocol("provider resumed a different session"));
        }
        self.profile
            .verify_session(&result, &self.capabilities, false)?;
        if let Some(params) = self.profile.session_configuration(execution.id().as_str()) {
            let result = self
                .rpc(
                    "session/set_config_option",
                    params,
                    deadline,
                    Some(execution),
                )
                .await
                .map_err(|error| startup_deadline(error, AgentStartupPhase::Configure, context))?;
            self.profile
                .verify_session(&result, &self.capabilities, true)?;
        }
        self.operation_capabilities
            .send_replace(OperationCapabilities {
                native_steering: self.steering_supported,
                session_resume: init
                    .pointer("/agentCapabilities/sessionCapabilities/resume")
                    .is_some_and(Value::is_object),
            });
        Ok(())
    }

    async fn drive(&mut self, execution: &mut ExecutionController) -> Result<(), AgentError> {
        enum Input {
            Close,
            Command(Option<Command>),
            Message(Result<Envelope, AgentError>),
            Deadline,
            ConsumerGone,
        }
        loop {
            let deadline = self.current_deadline();
            let input = if self.close_requested.borrow().is_some() && !self.closing {
                Input::Close
            } else {
                tokio::select! { biased;
                    _ = self.close_requested.changed(), if !self.closing => Input::Close,
                    _ = self.events.closed() => Input::ConsumerGone,
                    _ = wait_for_deadline(deadline), if deadline.is_some() => Input::Deadline,
                    command = self.commands.recv(), if !self.closing => Input::Command(command),
                    message = self.reader.next() => Input::Message(message),
                }
            };
            match input {
                Input::Close | Input::Command(None) => {
                    self.closing = true;
                    self.begin_shutdown_grace();
                    let request = self
                        .close_requested
                        .borrow()
                        .clone()
                        .unwrap_or(SessionCloseRequest::SessionHandlesDropped);
                    let reason =
                        requested_close_reason(&request, execution.active_execution_id().is_some());
                    let origin = request.origin();
                    self.cancellation_cause = Some((reason.clone(), origin.clone()));
                    self.drain_admitted_permissions(execution).await?;
                    let records = match self.active.as_ref() {
                        Some(active) => {
                            execution.close_execution(&active.execution_id, reason, origin)
                        }
                        _ => execution.close(reason, origin),
                    }?;
                    self.record_lifecycle(records).await?;
                    self.send_cancellation(execution.id().as_str()).await?;
                    if self.active.is_none() {
                        return Ok(());
                    }
                }
                Input::ConsumerGone => {
                    self.cancellation_cause = Some((
                        PermissionCancellationReason::event_consumer_dropped(),
                        CancellationOrigin::Runtime,
                    ));
                    return Err(AgentError::Backpressure);
                }
                Input::Deadline => {
                    return if self.closing {
                        Ok(())
                    } else {
                        self.failure_cause = ObservationFailureCause::DeadlineExceeded;
                        self.cancellation_cause = Some((
                            PermissionCancellationReason::deadline_exceeded(),
                            CancellationOrigin::Runtime,
                        ));
                        Err(AgentError::Deadline)
                    };
                }
                Input::Command(Some(command)) => {
                    self.command(execution, command).await?;
                    if self.deferred_outcome.is_some() {
                        return Ok(());
                    }
                }
                Input::Message(message) => {
                    self.message(execution, message?, self.current_deadline())
                        .await?;
                    if self.deferred_outcome.is_some() || (self.closing && self.active.is_none()) {
                        return Ok(());
                    }
                }
            }
        }
    }
    async fn drain_admitted_permissions(
        &mut self,
        execution: &mut ExecutionController,
    ) -> Result<(), AgentError> {
        // Seal admission before awaiting any effects. Enqueue uses try_send (no
        // caller-held permits), so later requests cannot grow this queue. Receiving
        // until closed also joins a concurrent try_send already inside admission.
        // Its admitted decisions precede aggregate closure even though the close
        // notification arrived over an independently ordered watch channel.
        self.commands.close();
        let mut failure = None;
        while let Some(command) = self.commands.recv().await {
            match command {
                command @ (Command::Answer(..) | Command::CancelPermission(..)) => {
                    if let Err(error) = self.command(execution, command).await {
                        // Still attempt later admitted decisions and their audit
                        // evidence; each audit/write already has a deadline.
                        // Each admitted decision may fail independently. Keep all
                        // failures in admission order even when its caller vanished.
                        failure = Some(retain_admitted_failure(failure, error));
                    }
                }
                Command::ExecutionRequest(_, reply) => {
                    let _ = reply.send(ProviderExecutionReply::Rejected(AgentError::Closed));
                }
                Command::Steer(_, _, reply) => {
                    let _ = reply.send(Err(ProviderOperationFailure::new(
                        AgentError::Closed,
                        ProviderSessionState::CleanupRequired,
                    )));
                }
            }
        }
        failure.map_or(Ok(()), Err)
    }
    // Keep the selected command owned while applying ready provider evidence.
    // Batches yield without treating decoder fairness as an absence of ready messages.
    async fn drain_ready_before_dispatch(
        &mut self,
        execution: &mut ExecutionController,
        command: &Command,
        dispatch_deadline: Option<Instant>,
    ) -> Result<DispatchReadiness, AgentError> {
        loop {
            for _ in 0..32 {
                let caller_gone = match command {
                    Command::ExecutionRequest(_, reply) => reply.is_closed(),
                    Command::Steer(_, _, reply) => reply.is_closed(),
                    _ => false,
                };
                if self.close_requested.borrow().is_some()
                    || self.closing
                    || self.deferred_outcome.is_some()
                    || caller_gone
                {
                    return Ok(DispatchReadiness::Interrupted(AgentError::Closed));
                }
                if self.events.is_closed() {
                    return Ok(DispatchReadiness::Interrupted(AgentError::Backpressure));
                }
                if self
                    .current_deadline()
                    .is_some_and(|deadline| Instant::now() >= deadline)
                {
                    // The drive loop owns this already-established lifecycle deadline.
                    return Ok(DispatchReadiness::Interrupted(AgentError::Deadline));
                }
                if dispatch_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                    self.failure_cause = ObservationFailureCause::DeadlineExceeded;
                    self.cancellation_cause = Some((
                        PermissionCancellationReason::deadline_exceeded(),
                        CancellationOrigin::Runtime,
                    ));
                    return Err(AgentError::Deadline);
                }
                let next = poll_fn(|cx| {
                    // One bounded reader poll must distinguish input readiness from
                    // Tokio's task budget; explicit decoder yields remain observable.
                    let next = tokio::task::unconstrained(self.reader.next());
                    tokio::pin!(next);
                    Poll::Ready(match next.poll(cx) {
                        Poll::Ready(message) => Some(message),
                        Poll::Pending => None,
                    })
                })
                .await;
                match next {
                    Some(message) => {
                        let deadline = self
                            .current_deadline()
                            .into_iter()
                            .chain(dispatch_deadline)
                            .min();
                        self.message(execution, message?, deadline).await?;
                    }
                    None if self.reader.decoding_yielded() => {
                        // This Pending came from the decoder's explicit fairness
                        // yield while it still owns buffered bytes, rather than
                        // from an empty provider pipe. Finish that buffered work.
                        tokio::task::yield_now().await;
                    }
                    None if self.reader.frame_in_progress() => {
                        // Once the transport has observed bytes for the next
                        // frame, an empty poll is not a dispatch boundary. Await
                        // that frame or a lifecycle boundary; no scheduler-turn
                        // count is used to guess when the writer is finished.
                        let deadline = self
                            .current_deadline()
                            .into_iter()
                            .chain(dispatch_deadline)
                            .min();
                        let message = tokio::select! { biased;
                            _ = self.close_requested.changed() => {
                                return Ok(DispatchReadiness::Interrupted(AgentError::Closed));
                            }
                            _ = self.events.closed() => {
                                return Ok(DispatchReadiness::Interrupted(AgentError::Backpressure));
                            }
                            _ = wait_for_deadline(deadline), if deadline.is_some() => {
                                self.failure_cause = ObservationFailureCause::DeadlineExceeded;
                                self.cancellation_cause = Some((
                                    PermissionCancellationReason::deadline_exceeded(),
                                    CancellationOrigin::Runtime,
                                ));
                                return Err(AgentError::Deadline);
                            }
                            message = self.reader.next() => message,
                        };
                        self.message(execution, message?, deadline).await?;
                    }
                    None => {
                        // ChildStdout readiness notification can lag a completed
                        // provider write. Probe the nonblocking OS pipe before
                        // treating an async Pending as the dispatch boundary.
                        if self.reader.read_ready_os_bytes()? {
                            continue;
                        }
                        return Ok(DispatchReadiness::Ready);
                    }
                }
            }
            tokio::task::yield_now().await;
        }
    }
    async fn command(
        &mut self,
        execution: &mut ExecutionController,
        command: Command,
    ) -> Result<(), AgentError> {
        let dispatch_deadline = match &command {
            Command::ExecutionRequest(..) => self
                .config
                .execution_timeout
                .map(|limit| Instant::now() + limit),
            Command::Steer(..) => Some(Instant::now() + steering::RESPONSE_TIMEOUT),
            _ => None,
        };
        if matches!(&command, Command::ExecutionRequest(..) | Command::Steer(..)) {
            let readiness = self
                .drain_ready_before_dispatch(execution, &command, dispatch_deadline)
                .await;
            let error = match &readiness {
                Ok(DispatchReadiness::Ready) => None,
                Ok(DispatchReadiness::Interrupted(error)) | Err(error) => Some(error.clone()),
            };
            if let Some(error) = error {
                match command {
                    Command::ExecutionRequest(_, reply) => {
                        let report = if matches!(
                            &readiness,
                            Ok(DispatchReadiness::Interrupted(AgentError::Closed))
                        ) {
                            ProviderExecutionReply::Rejected(error)
                        } else {
                            ProviderExecutionReply::Finished(ExecutionReport::new(
                                None,
                                Some(error),
                                ProviderSessionState::CleanupRequired,
                            ))
                        };
                        let _ = reply.send(report);
                    }
                    Command::Steer(_, _, reply) => {
                        let _ = reply.send(Err(ProviderOperationFailure::new(
                            error,
                            ProviderSessionState::CleanupRequired,
                        )));
                    }
                    _ => unreachable!("only dispatch commands drain provider input"),
                }
                return readiness.map(|_| ());
            }
        }
        match command {
            Command::ExecutionRequest(input, reply) => {
                if reply.is_closed() {
                    return Ok(());
                }
                if self.steering.is_some() {
                    let _ = reply.send(ProviderExecutionReply::Rejected(AgentError::Busy));
                    return Ok(());
                }
                let validation = self.profile.validate_execution(&input, &self.capabilities);
                if let Err(error) = validation {
                    let _ = reply.send(ProviderExecutionReply::Rejected(error));
                    return Ok(());
                }
                let id = self.pending_id()?;
                let frame = match json_rpc::encode(
                    json_rpc::request(
                        id,
                        "session/prompt",
                        json!({"sessionId":execution.id().as_str(),"prompt":[{"type":"text","text":input.user_message.as_str()}]}),
                    ),
                    self.config.max_frame_bytes,
                ) {
                    Ok(frame) => frame,
                    Err(error) => {
                        let _ = reply.send(ProviderExecutionReply::Rejected(error));
                        return Ok(());
                    }
                };
                let execution_id = input.execution_id;
                if let Err(error) = execution.begin_execution(execution_id.clone()) {
                    let _ = reply.send(ProviderExecutionReply::Rejected(error));
                    return Ok(());
                }
                self.sequence = id;
                self.active = Some(ActiveExecution {
                    id,
                    execution_id,
                    reply,
                    deadline: dispatch_deadline,
                });
                self.provider_result = None;
                self.profile.begin_execution();
                self.send_encoded(
                    frame,
                    self.active.as_ref().and_then(|active| active.deadline),
                )
                .await?;
            }
            Command::Steer(target, input, reply) => {
                if reply.is_closed() {
                    return Ok(());
                }
                if self.close_requested.borrow().is_some() {
                    let _ = reply.send(Err(ProviderOperationFailure::new(
                        AgentError::Closed,
                        ProviderSessionState::CleanupRequired,
                    )));
                } else if !self.steering_supported {
                    let _ = reply.send(Err(ProviderOperationFailure::new(
                        AgentError::Unsupported("provider does not support steering".into()),
                        ProviderSessionState::Usable,
                    )));
                } else if self.active.as_ref().map(|active| &active.execution_id) != Some(&target) {
                    let _ = reply.send(Ok(SteeringOutcome::PromptRequired));
                } else if self.steering.is_some() {
                    let _ = reply.send(Err(ProviderOperationFailure::new(
                        AgentError::Busy,
                        ProviderSessionState::Usable,
                    )));
                } else if let Err(error) =
                    self.profile.validate_execution(&input, &self.capabilities)
                {
                    let _ = reply.send(Err(ProviderOperationFailure::new(
                        error,
                        ProviderSessionState::Usable,
                    )));
                } else {
                    let id = self.pending_id()?;
                    let frame = match json_rpc::encode(
                        json_rpc::request(
                            id,
                            "_session/steering",
                            json!({
                                "sessionId": execution.id().as_str(),
                                "prompt": [{"type":"text", "text":input.user_message.as_str()}],
                                "_meta": {"steering":{"idleBehavior":"promptRequired"}}
                            }),
                        ),
                        self.config.max_frame_bytes,
                    ) {
                        Ok(frame) => frame,
                        Err(error) => {
                            let _ = reply.send(Err(ProviderOperationFailure::new(
                                error,
                                ProviderSessionState::Usable,
                            )));
                            return Ok(());
                        }
                    };
                    self.sequence = id;
                    self.steering = Some(PendingSteering {
                        id,
                        reply,
                        deadline: dispatch_deadline.expect("steering has an operation deadline"),
                    });
                    let deadline = self
                        .active
                        .as_ref()
                        .and_then(|active| active.deadline)
                        .into_iter()
                        .chain(dispatch_deadline)
                        .min();
                    self.send_encoded(frame, deadline).await?;
                }
            }
            Command::CancelPermission(input, reply) => {
                // Queue admission transfers ownership to the worker. A dropped
                // reply waiter must not erase the decision or its audit evidence.
                let record = match execution.cancel_review(input) {
                    Ok(record) => record,
                    Err(error) => {
                        let _ = reply.send(Err(ProviderOperationFailure::permission_answer(
                            error,
                            ProviderSessionState::Usable,
                            PermissionSelectionState::Pending,
                        )));
                        return Ok(());
                    }
                };
                let wire_id = self
                    .permissions
                    .remove(record.request().id())
                    .expect("pending wire permission");
                if let Err(error) = self.record_cancellations(vec![record.clone()]).await {
                    let _ = reply.send(Err(ProviderOperationFailure::permission_answer(
                        error.clone(),
                        ProviderSessionState::CleanupRequired,
                        PermissionSelectionState::Consumed,
                    )));
                    return Err(error);
                }
                let delivery = self
                    .send(permission_wire::permission_cancel(&wire_id))
                    .await;
                let _ = reply.send(delivery.clone().map(|()| record).map_err(|error| {
                    ProviderOperationFailure::new(error, ProviderSessionState::CleanupRequired)
                }));
                delivery?;
            }
            Command::Answer(answer, reply) => {
                // Queue admission transfers ownership to the worker. A dropped
                // reply waiter must not erase the decision or its audit evidence.
                let permission_id = answer.id.clone();
                let option_id = answer.option_id.clone();
                let resolution = match execution.answer_permission(answer) {
                    Ok(resolution) => resolution,
                    Err(error) => {
                        let _ = reply.send(Err(ProviderOperationFailure::permission_answer(
                            error,
                            ProviderSessionState::Usable,
                            PermissionSelectionState::Pending,
                        )));
                        return Ok(());
                    }
                };
                if let Err(error) = self
                    .record_answer(resolution.clone(), PermissionAnswerDelivery::Selected)
                    .await
                {
                    let _ = reply.send(Err(ProviderOperationFailure::permission_answer(
                        error.clone(),
                        ProviderSessionState::CleanupRequired,
                        PermissionSelectionState::Consumed,
                    )));
                    return Err(error);
                }
                let wire_id = self
                    .permissions
                    .remove(&permission_id)
                    .expect("pending wire permission");
                let response = permission_wire::selected(&wire_id, option_id.as_str());
                let result = self.send(response).await;
                let delivery = match &result {
                    Ok(()) => PermissionAnswerDelivery::Written,
                    Err(error) => PermissionAnswerDelivery::Failed(error.clone()),
                };
                if let Err(error) = self.record_answer(resolution.clone(), delivery).await {
                    let error = match result {
                        Err(delivery_error) => {
                            AgentError::PermissionAnswerDeliveryAndAuditFailure {
                                delivery_error: Box::new(delivery_error),
                                cleanup_error: None,
                            }
                        }
                        Ok(()) => error,
                    };
                    let _ = reply.send(Err(ProviderOperationFailure::permission_answer(
                        error.clone(),
                        ProviderSessionState::CleanupRequired,
                        PermissionSelectionState::Consumed,
                    )));
                    return Err(error);
                }
                let _ = reply.send(result.clone().map(|()| resolution).map_err(|error| {
                    ProviderOperationFailure::permission_answer(
                        error,
                        ProviderSessionState::CleanupRequired,
                        PermissionSelectionState::Consumed,
                    )
                }));
                result?;
            }
        }
        Ok(())
    }
    async fn message(
        &mut self,
        execution: &mut ExecutionController,
        message: Envelope,
        response_deadline: Option<Instant>,
    ) -> Result<(), AgentError> {
        if let Some(method) = message.method {
            let params = message.params.unwrap_or(Value::Null);
            if let Some(id) = message.id {
                if method == "session/request_permission" {
                    let result = self
                        .permission(execution, id.clone(), params, response_deadline)
                        .await;
                    if result.is_err() && !self.permissions.values().any(|wire_id| *wire_id == id) {
                        let _ = self
                            .send_before(permission_wire::permission_cancel(&id), response_deadline)
                            .await;
                    }
                    result?;
                } else {
                    self.send_before(json_rpc::unsupported(&id), response_deadline)
                        .await?;
                }
            } else if method == "session/request_permission" {
                return Err(json_rpc::protocol(
                    "permission request requires an RPC identifier",
                ));
            } else if method == "session/update" {
                self.update(execution, params)?;
            } else if method == "$/cancel_request" {
                let wire_id: RpcId =
                    serde_json::from_value(params.get("requestId").cloned().unwrap_or(Value::Null))
                        .map_err(|_| json_rpc::protocol("invalid cancellation request ID"))?;
                if let Some(id) = self
                    .permissions
                    .iter()
                    .find_map(|(id, pending)| (pending == &wire_id).then(|| id.clone()))
                {
                    self.permissions.remove(&id);
                    let target = &self
                        .active
                        .as_ref()
                        .ok_or_else(|| {
                            json_rpc::protocol("permission cancellation has no active prompt")
                        })?
                        .execution_id;
                    if let Some(record) = execution.cancel_permission(
                        target,
                        &id,
                        PermissionCancellationReason::provider_withdrawal(),
                        CancellationOrigin::Provider,
                    )? {
                        self.record_cancellations(vec![record]).await?;
                    }
                    self.send_before(
                        permission_wire::permission_cancel(&wire_id),
                        response_deadline,
                    )
                    .await?;
                }
            }
            return Ok(());
        }
        if self
            .steering
            .as_ref()
            .is_some_and(|pending| message.id == Some(RpcId::Number(pending.id)))
        {
            let result = match message.error {
                Some(error) => Err(AgentError::Provider { code: error.code }),
                None => message
                    .result
                    .ok_or_else(|| json_rpc::protocol("missing steering result"))
                    .and_then(steering::outcome),
            };
            // Malformed or failed replies retire the worker: delivery may already
            // have happened, so a new prompt cannot be substituted automatically.
            let failure = result.as_ref().err().cloned();
            let pending = self.steering.take().expect("matched steering response");
            let _ = pending.reply.send(result.map_err(|error| {
                ProviderOperationFailure::new(error, ProviderSessionState::CleanupRequired)
            }));
            if let Some(error) = failure {
                return Err(error);
            }
            return Ok(());
        }
        let active = self
            .active
            .as_ref()
            .ok_or_else(|| json_rpc::protocol("response without active prompt"))?;
        if message.id != Some(RpcId::Number(active.id)) {
            return Err(json_rpc::protocol("response for a different prompt"));
        }
        if let Some(error) = message.error {
            // Keep the reply until teardown has recorded pending cancellations.
            let error = AgentError::Provider { code: error.code };
            self.provider_result = Some(Err(error.clone()));
            return Err(error);
        }
        let result = wire::outcome(
            &message
                .result
                .ok_or_else(|| json_rpc::protocol("missing prompt result"))?,
        )?;
        self.provider_result = Some(Ok(result));
        if result == ExecutionOutcome::Cancelled {
            // Keep pending reviews until correlated session teardown. Never publish
            // tool/process cancellation from protocol evidence alone.
            self.deferred_outcome = Some(result);
        } else {
            let target = &self
                .active
                .as_ref()
                .expect("validated active prompt")
                .execution_id;
            let event = execution.finished_event(target, result)?;
            let records = execution.finish_execution(target, Ok(result))?;
            self.record_lifecycle(records).await?;
            // Keep one absolute grace through cancellation and fallback teardown.
            // Successful normal completion leaves the reusable session without it.
            let previous_deadline = self.shutdown_deadline;
            let deadline = self.begin_shutdown_grace();
            match tokio::time::timeout_at(deadline, self.cancel_wire_permissions()).await {
                Ok(result) => result?,
                Err(_) => {
                    self.failure_cause = ObservationFailureCause::DeadlineExceeded;
                    self.cancellation_cause.get_or_insert((
                        PermissionCancellationReason::deadline_exceeded(),
                        CancellationOrigin::Runtime,
                    ));
                    return Err(AgentError::Deadline);
                }
            }
            self.emit(event)?;
            self.shutdown_deadline = previous_deadline;
            let active = self.active.take().expect("validated active prompt");
            let _ = active
                .reply
                .send(ProviderExecutionReply::Finished(ExecutionReport::new(
                    Some(Ok(result)),
                    None,
                    ProviderSessionState::Usable,
                )));
            self.provider_result = None;
        }
        Ok(())
    }
    fn emit(&mut self, event: ExecutionEvent) -> Result<(), AgentError> {
        match self.events.try_send(event) {
            Ok(()) => Ok(()),
            Err(QueueError::Closed) => {
                self.cancellation_cause.get_or_insert((
                    PermissionCancellationReason::event_consumer_dropped(),
                    CancellationOrigin::Runtime,
                ));
                Err(AgentError::Backpressure)
            }
            Err(QueueError::Full) => Err(AgentError::Backpressure),
        }
    }
    fn check_session(
        &self,
        execution: &ExecutionController,
        params: &Value,
    ) -> Result<(), AgentError> {
        if fields::identifier(params, "sessionId")? != execution.id().as_str() {
            return Err(json_rpc::protocol("message belongs to another session"));
        }
        Ok(())
    }
    fn update(
        &mut self,
        execution: &mut ExecutionController,
        params: Value,
    ) -> Result<(), AgentError> {
        self.check_session(execution, &params)?;
        if self.closing {
            // Closure froze and audited the live context. Drain every in-flight
            // session notification without changing output or configuration;
            // terminal responses and permission cancellation use separate paths.
            tracing::debug!(session_id = %execution.id().as_str(), "session update arrived during cleanup");
            return Ok(());
        }
        let update = params
            .get("update")
            .ok_or_else(|| json_rpc::protocol("missing session update"))?;
        let kind = fields::string(update, "sessionUpdate")?;
        match kind {
            "config_option_update" | "current_mode_update" => {
                return self.profile.verify_update(kind, update, &self.capabilities);
            }
            "agent_message_chunk" | "agent_thought_chunk" | "tool_call" | "tool_call_update" => {
                if self.active.is_none() {
                    return Err(json_rpc::protocol(
                        "execution update without an active prompt",
                    ));
                }
            }
            // Bounded advisory updates cannot change the immutable capability snapshot.
            _ => return Ok(()),
        }
        let target = self
            .active
            .as_ref()
            .expect("validated active prompt")
            .execution_id
            .clone();
        if kind == "agent_message_chunk" || kind == "agent_thought_chunk" {
            let content = update
                .get("content")
                .ok_or_else(|| json_rpc::protocol("missing message content"))?;
            if content.get("type").and_then(Value::as_str) != Some("text") {
                return Err(json_rpc::protocol("non-text output is not supported"));
            }
            let text = content
                .get("text")
                .and_then(Value::as_str)
                .ok_or_else(|| json_rpc::protocol("invalid text content"))?
                .to_owned();
            let mut chunk = if kind == "agent_message_chunk" {
                MessageChunk::text(text)
            } else {
                MessageChunk::thought(text)
            };
            if let Some(id) = update.get("messageId") {
                let id = id
                    .as_str()
                    .ok_or_else(|| json_rpc::protocol("invalid message identity"))?;
                let id = MessageId::new(id)
                    .map_err(|_| json_rpc::protocol("invalid message identity"))?;
                chunk = chunk.with_message_id(id);
            }
            self.emit(execution.message_event(&target, chunk)?)
        } else {
            if !self.config.tools_enabled {
                return Err(json_rpc::protocol("tool event in a text-only binding"));
            }
            let tool = self.profile.tool_call(update)?;
            self.emit(execution.tool_event(&target, tool)?)
        }
    }
    async fn permission(
        &mut self,
        execution: &mut ExecutionController,
        wire_id: RpcId,
        params: Value,
        response_deadline: Option<Instant>,
    ) -> Result<(), AgentError> {
        self.check_session(execution, &params)?;
        if self.closing || self.active.is_none() || !self.config.tools_enabled {
            return self
                .send_before(
                    permission_wire::permission_cancel(&wire_id),
                    response_deadline,
                )
                .await;
        }
        if self.permissions.len() >= 128 || self.permissions.values().any(|id| *id == wire_id) {
            return Err(json_rpc::protocol(
                "permission request limit or duplicate ID",
            ));
        }
        let tool = params
            .get("toolCall")
            .ok_or_else(|| json_rpc::protocol("missing permission tool"))?;
        let input = self.profile.tool_input(tool)?;
        let tool = self.profile.tool_call(tool)?;
        let options = permission_wire::permission_options(&params, &self.config.permissions)?;
        let sequence = self
            .permission_sequence
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .map_err(|_| json_rpc::protocol("permission ID exhausted"))?
            + 1;
        let id = sequence.to_string();
        let permission_id = PermissionId::new(id.clone())
            .map_err(|error| json_rpc::protocol(&error.to_string()))?;
        let target = &self
            .active
            .as_ref()
            .expect("validated active prompt")
            .execution_id;
        let event =
            execution.request_permission(target, permission_id.clone(), tool, input, options)?;
        self.permissions.insert(permission_id, wire_id);
        self.emit(event)
    }
    async fn record_audit(&mut self, record: ExecutionAuditRecord) -> Result<(), AgentError> {
        let result = catch_worker_panic(async {
            // The trait call itself may panic before returning its future.
            timeout(self.config.shutdown_grace, self.audit.record(record))
                .await
                .map_err(|_| AgentError::AuditFailure)?
        })
        .await;
        if result.is_err() {
            self.audit_failure = Some(AgentError::AuditFailure);
            Err(AgentError::AuditFailure)
        } else {
            Ok(())
        }
    }
    async fn record_answer(
        &mut self,
        resolution: PermissionResolution,
        delivery: PermissionAnswerDelivery,
    ) -> Result<(), AgentError> {
        let execution_id = resolution.request().execution_id().clone();
        let permission_id = resolution.request().id().clone();
        let session_id = resolution.session_id().clone();
        let record = PermissionAnswerRecord::new(resolution, delivery);
        match self
            .record_audit(ExecutionAuditRecord::Answered(record))
            .await
        {
            Ok(()) => Ok(()),
            _ => {
                tracing::error!(session_id = %session_id.as_str(), execution_id = %execution_id.as_str(), permission_id = %permission_id.as_str(), "permission answer audit delivery failed");
                self.audit_failure = Some(AgentError::AuditFailure);
                Err(AgentError::AuditFailure)
            }
        }
    }
    async fn record_lifecycle(
        &mut self,
        records: Vec<ExecutionAuditRecord>,
    ) -> Result<(), AgentError> {
        let mut permissions = Vec::new();
        let mut failure = None;
        for record in records {
            match record {
                ExecutionAuditRecord::Cancelled(record) => permissions.push(record),
                record => {
                    if self.record_audit(record).await.is_err() {
                        tracing::error!("execution lifecycle audit delivery failed");
                        self.audit_failure = Some(AgentError::AuditFailure);
                        failure = Some(AgentError::AuditFailure);
                    }
                }
            }
        }
        let cancellation_result = self.record_cancellations(permissions).await;
        if cancellation_result == Err(AgentError::AuditFailure) {
            failure = Some(AgentError::AuditFailure);
        }
        failure.map_or(cancellation_result, Err)
    }
    async fn record_cancellations(
        &mut self,
        records: Vec<PermissionCancellation>,
    ) -> Result<(), AgentError> {
        let mut failure = None;
        // Attempt every record before publishing UI updates. A full/dropped UI queue
        // must not prevent audit capture of later cancellations in this batch.
        // Each sink call gets its own bound; one timeout cannot consume the
        // delivery opportunity of a different durable record.
        for record in &records {
            if self
                .record_audit(ExecutionAuditRecord::Cancelled(record.clone()))
                .await
                .is_err()
            {
                tracing::error!(
                    session_id = %record.session_id().as_str(),
                    execution_id = %record.request().execution_id().as_str(),
                    permission_id = %record.request().id().as_str(),
                    "permission cancellation audit delivery failed"
                );
                self.audit_failure = Some(AgentError::AuditFailure);
                failure = Some(AgentError::AuditFailure);
            }
        }
        for record in records {
            if let Err(error) = self.emit(ExecutionEvent::new(
                record.request().execution_id().clone(),
                ExecutionUpdate::PermissionCancelled(record),
            )) {
                failure.get_or_insert(error);
            }
        }
        if let Some(error) = failure {
            Err(error)
        } else {
            Ok(())
        }
    }
    async fn cancel_wire_permissions(&mut self) -> Result<(), AgentError> {
        let permissions = std::mem::take(&mut self.permissions);
        for wire_id in permissions.into_values() {
            self.send(permission_wire::permission_cancel(&wire_id))
                .await?;
        }
        Ok(())
    }
}

/// No artificial far-future timestamp: no configured limit means no timer.
async fn wait_for_deadline(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

#[cfg(all(test, unix))]
#[path = "../../../../tests/infrastructure/acp/executions/worker.rs"]
mod tests;
