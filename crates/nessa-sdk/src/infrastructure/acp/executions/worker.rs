use super::super::{
    fields,
    permissions::wire as permission_wire,
    profile::AcpProfile,
    sessions::{
        binding::{Command, Completion, DispatchedPrompt},
        cleanup::ProcessCleanup,
        AcpConfig,
    },
};
use super::{
    event_queue::{EventSender, QueueError},
    failure::{execution_finish_failure, requested_close_reason, retain_admitted_failure},
    prompt_content::{content_blocks, ImageBlocks},
    steering::{self, PendingSteering},
    wire,
};
use crate::application::agent_execution::agents::{
    AgentError, AgentStartupContext, AgentStartupPhase, AgentStartupStep,
};
use crate::application::agent_execution::executions::{
    ExecutionAudit, ExecutionAuditRecord, ExecutionController, ExecutionEvent, ExecutionRequest,
    ExecutionUpdate,
};

use crate::application::agent_execution::permissions::{
    CancellationOrigin, PermissionAnswer, PermissionAnswerDelivery, PermissionAnswerRecord,
    PermissionCancellation, PermissionCancellationRequest, PermissionResolution,
    PermissionSelectionState, ReviewDeclineRecord,
};
use crate::application::agent_execution::providers::{
    CleanupReport, ExecutionReport, ImageInputRefusal, ObservationFailureCause,
    OperationCapabilities, ProviderExecutionReply, ProviderOperationFailure,
    ProviderOperationResult, ProviderSessionState, ResourceCleanup, SessionCloseRequest,
    SteeringOutcome,
};
use crate::domain::agent_execution::executions::{
    ExecutionId, ExecutionOutcome, MessageChunk, MessageId,
};
use crate::domain::agent_execution::permissions::{
    PermissionCancellationReason, PermissionCancellationReasonView, PermissionId, ReviewDecline,
    ReviewDeclineReason,
};
use crate::domain::agent_execution::prompts::UserMessage;
use crate::domain::agent_execution::sessions::ExecutionSessionId;
use crate::domain::effective_capabilities::value_objects::EffectiveCapabilities;
use crate::infrastructure::{
    json_rpc::{self, Envelope, Reader, RpcError, RpcId},
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
};
use tokio::{
    sync::{mpsc, oneshot, watch},
    time::{timeout, Instant},
};

/// Turn a provider's error response into this adapter's error, reporting what
/// the provider said on the way past.
///
/// The error itself carries only the code, because the code is what decides
/// anything. The text is the operator's: `-32000` from a Codex that is not
/// signed in is indistinguishable from any other provider refusal, and
/// "Authentication required" is the entire answer. Logged once, here, so every
/// provider failure says as much as the provider said.
fn provider_failure(phase: &str, error: RpcError) -> AgentError {
    match &error.message {
        Some(message) => {
            tracing::warn!(code = error.code, phase, %message, "provider refused")
        }
        None => tracing::warn!(
            code = error.code,
            phase,
            "provider refused without a message"
        ),
    }
    AgentError::Provider { code: error.code }
}

type ExecutionReply = oneshot::Sender<ProviderExecutionReply>;
type SteeringReply = oneshot::Sender<ProviderOperationResult<SteeringOutcome>>;
enum DispatchReadiness {
    Ready,
    Interrupted(AgentError),
}
/// Who is waiting for the command about to be dispatched. Draining provider
/// input stops early when that caller has gone.
enum DispatchCaller<'a> {
    Execution(&'a ExecutionReply),
    Steering(&'a SteeringReply),
}
impl DispatchCaller<'_> {
    fn is_gone(&self) -> bool {
        match self {
            Self::Execution(reply) => reply.is_closed(),
            Self::Steering(reply) => reply.is_closed(),
        }
    }
}
/// What stopped a drain from reaching its dispatch boundary, if anything.
fn interruption(readiness: &Result<DispatchReadiness, AgentError>) -> Option<&AgentError> {
    match readiness {
        Ok(DispatchReadiness::Ready) => None,
        Ok(DispatchReadiness::Interrupted(error)) | Err(error) => Some(error),
    }
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
    /// The connected agent advertised `promptCapabilities.image` at initialize.
    /// Whether an image can actually be delivered also needs a byte source, so
    /// the published capability is this and `config.images` together.
    agent_accepts_images: bool,
    operation_capabilities: watch::Sender<OperationCapabilities>,
    permissions: HashMap<PermissionId, RpcId>,
    /// The review this worker answered without registering one — a refusal.
    ///
    /// The dispatcher answers any request whose handler failed and which it
    /// cannot find among the pending reviews, so that a provider is never left
    /// waiting on a question nobody will answer. A refusal is exactly that
    /// shape — answered, never registered — so without this it would be
    /// answered twice, and answering one request twice is its own protocol
    /// fault.
    declined: Option<RpcId>,
    shutdown_deadline: Option<Instant>,
    /// True once every request from `session_configuration` has been applied.
    ///
    /// The session's configuration requests are answered in order, and the
    /// provider may send `config_option_update` while they are still going out.
    /// Checking such a notification against the finished state would reject it
    /// for reporting the state this runtime had not asked for yet.
    configured: bool,
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
        config.max_incoming_frame_bytes,
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
        agent_accepts_images: false,
        operation_capabilities,
        permissions: HashMap::new(),
        declined: None,
        shutdown_deadline: None,
        configured: false,
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
        let allowed = json_rpc::write_allowance(bytes.len());
        let result = json_rpc::send_encoded(stdin, &bytes, allowed, deadline).await;
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
                return Err(provider_failure("startup", error));
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
        // Two budgets, because the two halves of startup are not ours in the
        // same way. Everything up to the child's first answer is the operating
        // system's: exec, its first-execution scan of a freshly written
        // executable, and the runtime's own boot. That is `launch_timeout`,
        // and it is generous. Protocol work afterwards is the provider
        // answering questions it is already running to answer, and keeps the
        // tighter `startup_timeout`.
        let spawn_deadline = Instant::now() + self.config.launch_timeout;
        // Whether saved context is being restored is decided before any step
        // runs, so every step's deadline reports it. Reading it off the step
        // would call a restoration that expired during `initialize` new.
        let context = if restore.is_some() {
            AgentStartupContext::Restored
        } else {
            AgentStartupContext::New
        };
        let init = self.rpc("initialize", json!({"protocolVersion":1,"clientInfo":{"name":"nessa-sdk","version":env!("CARGO_PKG_VERSION")},
            "clientCapabilities":{"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false}}), spawn_deadline, None)
            .await
            .map_err(|error| startup_deadline(error, AgentStartupPhase::Initialize, context))?;
        // The child has answered, so it is running and scanned. Start the
        // protocol budget here rather than carrying the remainder of a budget
        // that was sized for the operating system's work.
        let deadline = Instant::now() + self.config.startup_timeout;
        if init.get("protocolVersion").and_then(Value::as_u64) != Some(1) {
            return Err(json_rpc::protocol("requires ACP protocol 1"));
        }
        self.profile.validate_initialize(&init)?;
        self.steering_supported = self.profile.supports_steering(&init);
        self.agent_accepts_images =
            init.pointer("/agentCapabilities/promptCapabilities/image") == Some(&Value::Bool(true));
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
        // Applied in the profile's own order, because a provider can reject a
        // later selection that an earlier one has not made available yet. Only
        // the last response is checked as fully configured; the ones before it
        // are checked against what the profile has settled so far.
        let configuration = self.profile.session_configuration(execution.id().as_str());
        // Which makes this result the final state exactly when there is nothing
        // to apply after it. A profile is allowed to pin everything in its
        // session parameters and return no requests at all; for such a profile
        // the loop below never runs, so checking this leniently would mean
        // publishing ready having never held anything to the settled state —
        // with every test green, because the two profiles that exist today
        // return one request and two.
        self.profile
            .verify_session(&result, &self.capabilities, configuration.is_empty())?;
        let last = configuration.len().saturating_sub(1);
        for (step, params) in configuration.into_iter().enumerate() {
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
                .verify_session(&result, &self.capabilities, step == last)?;
        }
        self.configured = true;
        self.operation_capabilities
            .send_replace(OperationCapabilities {
                negotiated: true,
                native_steering: self.steering_supported,
                // Only an agent that said so receives an image, and only when
                // this process has somewhere to read the bytes from.
                image_input: self.config.images.is_some() && self.agent_accepts_images,
                session_resume: init
                    .pointer("/agentCapabilities/sessionCapabilities/resume")
                    .is_some_and(Value::is_object),
            });
        Ok(())
    }

    /// The ACP content blocks for one user message: its text, then its images
    /// in attachment order. Nothing has been written when this fails, so the
    /// caller rejects the input without a dispatch.
    ///
    /// This never waits. The session read, verified, and encoded `images`
    /// before it sent the command, because this task is the only one polling
    /// close, deadlines, consumer loss, and the agent's output. What stays here
    /// is the one fact only this task knows for certain after a restoration:
    /// whether the connected agent agreed to receive images. Admission gives
    /// the same typed answer when it already knows.
    fn prompt_blocks(
        &self,
        message: &UserMessage,
        images: ImageBlocks,
    ) -> Result<Vec<Value>, AgentError> {
        if !message.images().is_empty() {
            if self.config.images.is_none() {
                return Err(AgentError::ImageInputRefused(ImageInputRefusal::NotOffered));
            }
            if !self.agent_accepts_images {
                return Err(AgentError::ImageInputRefused(
                    ImageInputRefusal::AgentDoesNotAccept,
                ));
            }
        }
        content_blocks(message, images)
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
        caller: &DispatchCaller<'_>,
        dispatch_deadline: Option<Instant>,
    ) -> Result<DispatchReadiness, AgentError> {
        loop {
            for _ in 0..32 {
                if self.close_requested.borrow().is_some()
                    || self.closing
                    || self.deferred_outcome.is_some()
                    || caller.is_gone()
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
        match command {
            Command::ExecutionRequest(prompt, reply) => {
                self.dispatch_execution(execution, prompt, reply).await
            }
            Command::Steer(target, prompt, reply) => {
                self.dispatch_steering(execution, target, prompt, reply)
                    .await
            }
            Command::CancelPermission(input, reply) => {
                self.withdraw_permission(execution, input, reply).await
            }
            Command::Answer(answer, reply) => {
                self.answer_permission(execution, answer, reply).await
            }
        }
    }
    /// Send one `session/prompt` for `prompt`, after applying provider evidence
    /// that is already readable. Every refusal reaches `reply` and writes
    /// nothing; `prompt` carries the deadline the whole execution shares,
    /// counted from before its images were read.
    async fn dispatch_execution(
        &mut self,
        execution: &mut ExecutionController,
        prompt: DispatchedPrompt,
        reply: ExecutionReply,
    ) -> Result<(), AgentError> {
        let deadline = prompt.deadline;
        let readiness = self
            .drain_ready_before_dispatch(execution, &DispatchCaller::Execution(&reply), deadline)
            .await;
        if let Some(error) = interruption(&readiness) {
            let report = if matches!(
                &readiness,
                Ok(DispatchReadiness::Interrupted(AgentError::Closed))
            ) {
                ProviderExecutionReply::Rejected(error.clone())
            } else {
                ProviderExecutionReply::Finished(ExecutionReport::new(
                    None,
                    Some(error.clone()),
                    ProviderSessionState::CleanupRequired,
                ))
            };
            let _ = reply.send(report);
            return readiness.map(|_| ());
        }
        if reply.is_closed() {
            return Ok(());
        }
        if self.steering.is_some() {
            let _ = reply.send(ProviderExecutionReply::Rejected(AgentError::Busy));
            return Ok(());
        }
        let DispatchedPrompt {
            input, mut images, ..
        } = prompt;
        // Held until these bytes have left as a frame, so the session's
        // in-flight image budget covers exactly what is still retained.
        let _budget = images.take_charge();
        if let Err(error) = self.profile.validate_execution(&input, &self.capabilities) {
            let _ = reply.send(ProviderExecutionReply::Rejected(error));
            return Ok(());
        }
        let blocks = match self.prompt_blocks(&input.user_message, images) {
            Ok(blocks) => blocks,
            Err(error) => {
                let _ = reply.send(ProviderExecutionReply::Rejected(error));
                return Ok(());
            }
        };
        let id = self.pending_id()?;
        let frame = match json_rpc::encode(
            json_rpc::request(
                id,
                "session/prompt",
                json!({"sessionId":execution.id().as_str(),"prompt":blocks}),
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
            deadline,
        });
        self.provider_result = None;
        self.profile.begin_execution();
        self.send_encoded(frame, deadline).await
    }
    /// Send one `_session/steering` request for `prompt`, after applying
    /// provider evidence that is already readable.
    ///
    /// `prompt` carries the deadline the whole steering call shares, counted
    /// from before its images were read; the acknowledgement is awaited until
    /// that instant plus the extra write time this frame's size is allowed.
    async fn dispatch_steering(
        &mut self,
        execution: &mut ExecutionController,
        target: ExecutionId,
        prompt: DispatchedPrompt,
        reply: SteeringReply,
    ) -> Result<(), AgentError> {
        let readiness = self
            .drain_ready_before_dispatch(
                execution,
                &DispatchCaller::Steering(&reply),
                prompt.deadline,
            )
            .await;
        if let Some(error) = interruption(&readiness) {
            let _ = reply.send(Err(ProviderOperationFailure::new(
                error.clone(),
                ProviderSessionState::CleanupRequired,
            )));
            return readiness.map(|_| ());
        }
        if reply.is_closed() {
            return Ok(());
        }
        if let Some(answer) = self.steering_without_sending(&target, &prompt.input) {
            let _ = reply.send(answer);
            return Ok(());
        }
        let DispatchedPrompt {
            input,
            mut images,
            deadline,
        } = prompt;
        // Held until these bytes have left as a frame, as for an execution.
        let _budget = images.take_charge();
        let blocks = match self.prompt_blocks(&input.user_message, images) {
            Ok(blocks) => blocks,
            Err(error) => {
                let _ = reply.send(Err(ProviderOperationFailure::new(
                    error,
                    ProviderSessionState::Usable,
                )));
                return Ok(());
            }
        };
        let id = self.pending_id()?;
        let frame = match json_rpc::encode(
            json_rpc::request(
                id,
                "_session/steering",
                json!({
                    "sessionId": execution.id().as_str(),
                    "prompt": blocks,
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
        // The acknowledgement deadline covers the write too. A frame carrying
        // images gets the same extra time its write does; a text frame keeps
        // exactly what is left of the fixed steering deadline.
        let acknowledged_by = deadline.expect("steering has an operation deadline")
            + json_rpc::large_frame_allowance(frame.len());
        self.sequence = id;
        self.steering = Some(PendingSteering {
            id,
            reply,
            deadline: acknowledged_by,
        });
        let write_deadline = self
            .active
            .as_ref()
            .and_then(|active| active.deadline)
            .into_iter()
            .chain(Some(acknowledged_by))
            .min();
        self.send_encoded(frame, write_deadline).await
    }
    /// The answer a steering request gets without being sent, if it gets one:
    /// a closing context, an agent that does not steer, a target that is not
    /// the running execution, one steering already outstanding, or input the
    /// profile refuses.
    fn steering_without_sending(
        &self,
        target: &ExecutionId,
        input: &ExecutionRequest,
    ) -> Option<ProviderOperationResult<SteeringOutcome>> {
        if self.close_requested.borrow().is_some() {
            return Some(Err(ProviderOperationFailure::new(
                AgentError::Closed,
                ProviderSessionState::CleanupRequired,
            )));
        }
        if !self.steering_supported {
            return Some(Err(ProviderOperationFailure::new(
                AgentError::Unsupported("provider does not support steering".into()),
                ProviderSessionState::Usable,
            )));
        }
        if self.active.as_ref().map(|active| &active.execution_id) != Some(target) {
            return Some(Ok(SteeringOutcome::PromptRequired));
        }
        if self.steering.is_some() {
            return Some(Err(ProviderOperationFailure::new(
                AgentError::Busy,
                ProviderSessionState::Usable,
            )));
        }
        match self.profile.validate_execution(input, &self.capabilities) {
            Ok(()) => None,
            Err(error) => Some(Err(ProviderOperationFailure::new(
                error,
                ProviderSessionState::Usable,
            ))),
        }
    }
    /// Withdraw one pending review on the caller's behalf: record the decision,
    /// then tell the agent. Queue admission transferred ownership to this
    /// worker, so a dropped reply waiter must not erase either step.
    async fn withdraw_permission(
        &mut self,
        execution: &mut ExecutionController,
        input: PermissionCancellationRequest,
        reply: oneshot::Sender<ProviderOperationResult<PermissionCancellation>>,
    ) -> Result<(), AgentError> {
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
        delivery
    }
    /// Answer one pending review with the caller's selection: record it, send
    /// it, then record what delivery did. Queue admission transferred ownership
    /// to this worker, so a dropped reply waiter must not erase any of that.
    async fn answer_permission(
        &mut self,
        execution: &mut ExecutionController,
        answer: PermissionAnswer,
        reply: oneshot::Sender<ProviderOperationResult<PermissionResolution>>,
    ) -> Result<(), AgentError> {
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
                Err(delivery_error) => AgentError::PermissionAnswerDeliveryAndAuditFailure {
                    delivery_error: Box::new(delivery_error),
                    cleanup_error: None,
                },
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
        result
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
                    let answered = self.declined.take().is_some_and(|wire_id| wire_id == id);
                    if result.is_err()
                        && !answered
                        && !self.permissions.values().any(|wire_id| *wire_id == id)
                    {
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
                Some(error) => Err(provider_failure("steering", error)),
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
            let error = provider_failure("prompt", error);
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
                return self.profile.verify_update(
                    kind,
                    update,
                    &self.capabilities,
                    self.configured,
                );
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
        // A review this binding cannot put to a host is one tool's answer, not
        // the execution's ending. Each of these used to leave the turn dead and
        // the caller with nothing on screen; the agent is now told no, and goes
        // on to say so in its own words.
        let call = match params.get("toolCall") {
            Some(call) => call,
            None => {
                return self
                    .decline_review(
                        execution,
                        wire_id,
                        &params,
                        ReviewDeclineReason::UnreadableRequest,
                        response_deadline,
                    )
                    .await
            }
        };
        // `Unsupported` is the profile saying it will not review this tool;
        // anything else is it saying it could not read the request. Flattening
        // the two would file a refusal under the wrong reason, and the reason
        // is most of what the record is for.
        let input = match self.profile.permission_input(&params) {
            Ok(input) => input,
            Err(error) => {
                return self
                    .decline_review(
                        execution,
                        wire_id,
                        &params,
                        decline_reason(&error),
                        response_deadline,
                    )
                    .await
            }
        };
        let tool = match self.profile.tool_call(call) {
            Ok(tool) => tool,
            Err(error) => {
                return self
                    .decline_review(
                        execution,
                        wire_id,
                        &params,
                        decline_reason(&error),
                        response_deadline,
                    )
                    .await
            }
        };
        let options = match permission_wire::permission_options(&params, &self.config.permissions) {
            Ok(options) => options,
            Err(_) => {
                return self
                    .decline_review(
                        execution,
                        wire_id,
                        &params,
                        ReviewDeclineReason::UnusableOptions,
                        response_deadline,
                    )
                    .await
            }
        };
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
    /// Answer one review "no" without offering it, and leave the turn running.
    ///
    /// The agent asked to use a tool this binding will not put to a host, or
    /// asked in a frame that could not be described to one. Either way the
    /// answer is about that tool: the execution continues, and the agent —
    /// having been told no rather than cut off — is the one that explains it to
    /// whoever is reading.
    ///
    /// A refusal is delivered the way a selection is, so it is recorded the
    /// same way: the local decision first, then what the wire did with it.
    /// Where the provider offered a rejection to choose, that is the answer,
    /// because a chosen "no" is a denial the agent can act on. Where no such
    /// choice could be read, the review is cancelled instead — the weaker
    /// statement, and the honest one.
    async fn decline_review(
        &mut self,
        execution: &ExecutionController,
        wire_id: RpcId,
        params: &Value,
        reason: ReviewDeclineReason,
        response_deadline: Option<Instant>,
    ) -> Result<(), AgentError> {
        let decline = ReviewDecline::new(declared_tool_name(params), reason);
        let session_id = execution.id().clone();
        let Some(execution_id) = self
            .active
            .as_ref()
            .map(|active| active.execution_id.clone())
        else {
            // Without an active execution there is nothing to correlate the
            // refusal with, and the caller's own guard has already answered.
            return self
                .send_before(
                    permission_wire::permission_cancel(&wire_id),
                    response_deadline,
                )
                .await;
        };
        tracing::warn!(
            session_id = %session_id.as_str(),
            execution_id = %execution_id.as_str(),
            tool = decline.tool().unwrap_or("<unnamed>"),
            reason = ?decline.reason(),
            "tool review declined; the agent is told no and the execution continues"
        );
        let record = |delivery| {
            ExecutionAuditRecord::ReviewDeclined(ReviewDeclineRecord::new(
                session_id.clone(),
                execution_id.clone(),
                decline.clone(),
                delivery,
            ))
        };
        // The decision is evidence before the wire sees it. An unrecordable
        // decision still has to reach the agent, so the refusal is sent either
        // way and the audit failure is reported after it.
        let decided = self
            .record_audit(record(PermissionAnswerDelivery::Selected))
            .await;
        let response = match rejection_option(params) {
            Some(option) => permission_wire::selected(&wire_id, &option),
            None => permission_wire::permission_cancel(&wire_id),
        };

        // Answered, and deliberately not registered: a refusal has no pending
        // review to register. The dispatcher is told so it does not answer the
        // same request a second time when this returns an audit failure.
        self.declined = Some(wire_id);
        let delivery = self.send_before(response, response_deadline).await;
        let observed = match &delivery {
            Ok(()) => PermissionAnswerDelivery::Written,
            Err(error) => PermissionAnswerDelivery::Failed(error.clone()),
        };
        let written = self.record_audit(record(observed)).await;
        decided.and(written).and(delivery)
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

/// Which of the two things a profile's refusal was saying.
///
/// `Unsupported` is "this binding does not review that tool"; everything else
/// is "this binding could not read the request". They lead to the same answer
/// on the wire and to different records, which is the point of keeping them
/// apart.
fn decline_reason(error: &AgentError) -> ReviewDeclineReason {
    match error {
        AgentError::Unsupported(_) => ReviewDeclineReason::ToolNotReviewable,
        _ => ReviewDeclineReason::UnreadableRequest,
    }
}

/// The provider's own name for the tool under review, as its frame gives it.
///
/// Either place the provider names it counts. A permission request carries the
/// name on its `toolCall`; the profile metadata that a tool-call update uses
/// may carry it instead, and a refusal should not go unnamed over which field
/// a provider chose. Whether the name is worth retaining is
/// [`ReviewDecline`]'s decision, not this one's.
fn declared_tool_name(params: &Value) -> Option<&str> {
    params
        .pointer("/toolCall/name")
        .or_else(|| params.pointer("/toolCall/_meta/claudeCode/toolName"))
        .and_then(Value::as_str)
}

/// A rejection the provider offered, if it offered one that says "no" once.
///
/// Read from the frame rather than through the offer policy, because this is
/// the path taken when that policy could not be applied. Only `reject_once`
/// qualifies: a persistent refusal would answer for reviews this binding has
/// not seen, which is not a decision it may make on a host's behalf.
fn rejection_option(params: &Value) -> Option<String> {
    params
        .get("options")
        .and_then(Value::as_array)?
        .iter()
        .find(|option| option.get("kind").and_then(Value::as_str) == Some("reject_once"))
        .and_then(|option| option.get("optionId"))
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty() && id.len() <= 256)
        .map(str::to_owned)
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
