use super::super::{
    fields,
    permissions::{wire as permission_wire, workspace::bounded_review_class},
    profile::AcpProfile,
    questions::wire as question_wire,
    sessions::{
        binding::{Command, Completion, DispatchedPrompt},
        cleanup::ProcessCleanup,
        forwarded::{attach_arguments, attach_forwarded},
        thought_level, AcpConfig,
    },
};
#[cfg(all(test, unix))]
use super::failure::MAX_RETAINED_CATEGORY_FACTS;
use super::{
    event_queue::{EventSender, QueueError},
    failure::{
        execution_finish_failure, requested_close_reason, retain_admitted_failure,
        AuditEffectPhase, EffectCorrelation, FactId, OperationEffectPhase, SettlementFacts,
        WorkerFailure,
    },
    prompt_content::{content_blocks, ImageBlocks},
    steering::{self, PendingSteering},
    wire,
};
use crate::application::agent_execution::agents::{
    AgentError, AgentStartupContext, AgentStartupPhase, AgentStartupStep, ProviderDiagnostic,
};
use crate::application::agent_execution::executions::{
    AdmittedQuestion, ExecutionAudit, ExecutionAuditRecord, ExecutionController, ExecutionEvent,
    ExecutionRequest, ExecutionUpdate, LateProviderResultRecord, NativeHooksLaunchCause,
    NativeHooksLaunchRecord, PermissionAuthoritySource,
};

use crate::application::agent_execution::permissions::{
    admit_configured_outcome, remember_effect, ApprovalAttribution, ApprovalBasis,
    ApprovalRuleReference, CancellationOrigin, ConfiguredRefusal, PermissionAnswer,
    PermissionAnswerDelivery, PermissionAnswerRecord, PermissionCancellation,
    PermissionCancellationRequest, PermissionResolution, PermissionSelectionState, QuestionAnswer,
    QuestionAnswerRecord, QuestionRefusalRecord, RefusedAsk, RememberRequest, ReviewDeclineRecord,
    ReviewNotice, StopKind,
};
use crate::application::agent_execution::providers::{
    ApprovalMode, CleanupReport, ExecutionReport, ImageInputRefusal, ObservationFailureCause,
    OperationCapabilities, ProviderExecutionReply, ProviderOperationCapabilities,
    ProviderOperationFailure, ProviderOperationResult, ProviderSessionState, ResourceCleanup,
    SessionCloseRequest, SteeringOutcome,
};
use crate::application::agent_execution::traces::{
    LocalTraceStore, PermissionFact, TraceRecorder, TurnIdentity,
};
use crate::domain::agent_execution::executions::{
    ExecutionId, ExecutionOutcome, MessageChunk, MessageId,
};
use crate::domain::agent_execution::permissions::{
    PermissionCancellationReason, PermissionCancellationReasonView, PermissionDeadlineSettlement,
    PermissionDecision, PermissionEffect, PermissionId, PermissionOfferPolicy, PermissionOptionId,
    PermissionScope, PermissionStateView, QuestionListing, RefusalOutcome, ReviewDecline,
    ReviewDeclineId, ReviewDeclineObservation, ReviewDeclineReason, ReviewDeclineStage,
    MAX_OPEN_REVIEWS, MAX_REVIEW_TOOL_NAME_BYTES,
};
use crate::domain::agent_execution::prompts::UserMessage;
use crate::domain::agent_execution::questions::{
    QuestionCancellation, QuestionId, QuestionRefusalReason, QuestionResponse,
};
use crate::domain::agent_execution::sessions::ExecutionSessionId;
use crate::domain::effective_capabilities::value_objects::EffectiveCapabilities;
use crate::infrastructure::clock::{within, Clock, ClockInstant, ClockSleep};
use crate::infrastructure::{
    json_rpc::{self, Envelope, Reader, RpcError, RpcId},
    process::{ProcessOutput, ProcessScope},
};
use serde_json::{json, Value};
use std::time::Duration;
use std::{
    collections::{HashMap, VecDeque},
    future::{poll_fn, Future},
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    task::Poll,
};

use tokio::sync::{mpsc, oneshot, watch, Semaphore};

/// What this client says about itself in `initialize`, on every connection
/// it opens to an agent, whatever the connection is for.
///
/// Questions are advertised only where `questions` says the configured agent can
/// ask: advertising to one that cannot leaves the model holding a tool whose
/// every use this binding would have to refuse. The form capability is an
/// object, as ACP's schema reads it: a boolean `true` fails that schema and is
/// read as absent, so an agent told `true` never asks at all.
pub(in crate::infrastructure::acp) fn initialize_params(questions: bool) -> Value {
    let mut capabilities =
        json!({"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false});
    if questions {
        capabilities["elicitation"] = json!({"form":{}});
    }
    json!({"protocolVersion":1,"clientInfo":{"name":"nessa-sdk","version":env!("CARGO_PKG_VERSION")},
        "clientCapabilities":capabilities})
}

/// Whether an agent's `initialize` answer is one this client works with: ACP
/// protocol 1, and whatever the profile requires of it.
pub(in crate::infrastructure::acp) fn check_initialize<P: AcpProfile>(
    profile: &P,
    init: &Value,
) -> Result<(), AgentError> {
    if init.get("protocolVersion").and_then(Value::as_u64) != Some(1) {
        return Err(json_rpc::protocol("requires ACP protocol 1"));
    }
    profile.validate_initialize(init)
}

/// Turn a provider's error response into this adapter's error, reporting what
/// the provider said on the way past.
///
/// The code is the decision fact. Provider text is retained and logged only as
/// bounded diagnostic context, so it cannot decide lifecycle behavior or grow
/// through an untrusted response.
pub(in crate::infrastructure::acp) fn provider_failure(phase: &str, error: RpcError) -> AgentError {
    let diagnostic = error.message.map(ProviderDiagnostic::new);
    match &diagnostic {
        Some(message) => {
            tracing::warn!(
                code = error.code,
                phase,
                message = message.as_str(),
                "provider refused"
            )
        }
        None => tracing::warn!(
            code = error.code,
            phase,
            "provider refused without a message"
        ),
    }
    // ACP protocol 1 assigns -32000 to Authentication required (the pinned
    // ACP schema), unlike a generic JSON-RPC provider code. Keep that meaning
    // at this adapter boundary; custom providers keep their generic errors.
    if error.code == -32000 {
        AgentError::AuthenticationRequired { diagnostic }
    } else {
        AgentError::Provider {
            code: error.code,
            diagnostic,
        }
    }
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
fn interruption(readiness: &Result<DispatchReadiness, WorkerFailure>) -> Option<&AgentError> {
    match readiness {
        Ok(DispatchReadiness::Ready) => None,
        Ok(DispatchReadiness::Interrupted(error)) => Some(error),
        Err(failure) => Some(failure.error()),
    }
}
/// A prompt a refusal already stopped, still waiting for the provider's reply.
///
/// The execution reply stays here until that reply arrives or the shutdown grace
/// expires. A later provider result for `prompt_id` is evidence, not a second
/// outcome for that execution.
struct StoppedPrompt {
    prompt_id: i64,
    execution_id: ExecutionId,
    session_id: ExecutionSessionId,
    reply: ExecutionReply,
    kind: StopKind,
}
/// Why later prompts on this generation cannot run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StopCleanup {
    /// No stop has required cleanup.
    None,
    /// A configured refusal closed the session.
    Policy,
    /// An end-turn stop's provider missed the reply grace.
    Deadline,
}
fn refusal_outcome(kind: StopKind) -> RefusalOutcome {
    match kind {
        StopKind::EndTurn => RefusalOutcome::EndTurn,
        StopKind::CloseSession => RefusalOutcome::CloseSession,
    }
}
/// A stopped prompt whose caller was already told the grace expired.
///
/// The provider may still answer. That reply is a late result, not a prompt
/// this worker is missing.
struct ExpiredStop {
    prompt_id: i64,
    execution_id: ExecutionId,
    session_id: ExecutionSessionId,
}
struct ActiveExecution {
    // Consumed by the first accepted, nonempty answer chunk; diagnostic only.
    first_response_started: Option<ClockInstant>,
    id: i64,
    execution_id: ExecutionId,
    reply: ExecutionReply,
    deadline: Option<ClockInstant>,
}
/// Ensures a selected local refusal gets a final publication attempt even when
/// the async operation is cancelled between selection and response settlement.
struct DeclineNoticePublication {
    events: EventSender,
    execution_id: ExecutionId,
    observation: ReviewDeclineObservation,
    selected: bool,
    write_attempted: bool,
    final_stage: Option<ReviewDeclineStage>,
    settled: bool,
}
impl DeclineNoticePublication {
    fn new(
        events: EventSender,
        execution_id: ExecutionId,
        observation: ReviewDeclineObservation,
    ) -> Self {
        Self {
            events,
            execution_id,
            observation,
            selected: false,
            write_attempted: false,
            final_stage: None,
            settled: false,
        }
    }
    fn event(&self, stage: ReviewDeclineStage) -> ExecutionEvent {
        let observation = if stage == ReviewDeclineStage::Selected {
            self.observation.clone()
        } else {
            self.observation
                .advance(stage)
                .expect("decline publication advances selection to a final stage")
        };
        ExecutionEvent::new(
            self.execution_id.clone(),
            ExecutionUpdate::ReviewDeclined(observation),
        )
    }
    fn publish_selected(&mut self) -> Result<(), QueueError> {
        if !self.selected {
            self.events
                .try_send(self.event(ReviewDeclineStage::Selected))?;
            self.selected = true;
        }
        Ok(())
    }
    fn begin_write(&mut self) {
        self.write_attempted = true;
    }
    fn settle(&mut self, stage: ReviewDeclineStage) -> Result<(), QueueError> {
        self.final_stage = Some(stage);
        self.publish_selected()?;
        self.events.try_send(self.event(stage))?;
        self.settled = true;
        Ok(())
    }
    /// Keep the selected notice as the panel's reason for a stop.
    ///
    /// The turn finishes immediately after that notice. A later delivery stage
    /// would be an observation after the finish, and the agent treats that as
    /// provider output following a terminal result. The write itself stays in
    /// the audit.
    fn keep_selected_notice(&mut self) {
        self.settled = true;
    }
}
impl Drop for DeclineNoticePublication {
    fn drop(&mut self) {
        if self.settled {
            return;
        }
        let stage = self.final_stage.unwrap_or(if self.write_attempted {
            ReviewDeclineStage::WriteUnconfirmed
        } else {
            ReviewDeclineStage::WriteNotAttempted
        });
        let result = self.publish_selected().and_then(|()| {
            let event = self.event(stage);
            self.events.try_send(event)
        });
        if let Err(error) = result {
            tracing::error!(
                execution_id = %self.execution_id.as_str(),
                decline_id = %self.observation.id().as_str(),
                ?error,
                "declined-review notice could not be finalized during operation drop"
            );
        }
    }
}
fn merge_failure(
    first: Result<(), WorkerFailure>,
    second: Result<(), WorkerFailure>,
) -> Result<(), WorkerFailure> {
    match (first, second) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(first), Err(second)) => Err(first.combine(second)),
        (Err(failure), Ok(())) | (Ok(()), Err(failure)) => Err(failure),
    }
}
/// The provider's prompt result, split the way the active turn records it.
///
/// A provider error object is the turn's result. A missing or unreadable result
/// is a protocol failure and is not recorded as that result.
enum PromptReply {
    Failed(AgentError),
    Outcome(ExecutionOutcome),
}
fn prompt_reply(message: &Envelope) -> Result<PromptReply, AgentError> {
    if let Some(error) = message.error.as_ref() {
        return Ok(PromptReply::Failed(provider_failure(
            "prompt",
            RpcError {
                code: error.code,
                message: error.message.clone(),
            },
        )));
    }
    let result = message
        .result
        .as_ref()
        .ok_or_else(|| json_rpc::protocol("missing prompt result"))?;
    Ok(PromptReply::Outcome(wire::outcome(result)?))
}
fn combine_decline_result(
    result: Result<(), AgentError>,
    notice: Result<(), AgentError>,
) -> Result<(), AgentError> {
    match (result, notice) {
        (Ok(()), Ok(())) => Ok(()),
        (Ok(()), Err(error)) => Err(error),
        (Err(error), Ok(())) => Err(error),
        (Err(error), Err(notice_error)) => Err(AgentError::MultipleOperationFailures {
            first_error: Box::new(error),
            subsequent_error: Box::new(notice_error),
        }),
    }
}
/// Retain the lifecycle fact a closed event queue proves before returning its
/// caller-facing diagnostic. A full queue is still open and proves no closure.
fn event_publication_result(
    cancellation_cause: &mut Option<(PermissionCancellationReason, CancellationOrigin)>,
    result: Result<(), QueueError>,
) -> Result<(), AgentError> {
    match result {
        Ok(()) => Ok(()),
        Err(QueueError::Closed) => {
            cancellation_cause.get_or_insert((
                PermissionCancellationReason::event_consumer_dropped(),
                CancellationOrigin::Runtime,
            ));
            Err(AgentError::Backpressure)
        }
        Err(QueueError::Full) => Err(AgentError::Backpressure),
    }
}
/// One review the deadline will answer if nobody else does first.
struct ArmedPermission {
    at: ClockInstant,
    settlement: PermissionDeadlineSettlement,
}

/// What a later answer needs from the request that opened the review.
struct PermissionContext {
    /// Execution that opened the review. An answer for another execution
    /// does not store a rule for this one.
    execution: ExecutionId,
    tool: String,
    /// The once allow option, when the review offered one that is not the
    /// harness's persistent choice and not a Nessa remember choice.
    once_allow: Option<PermissionOptionId>,
    /// The once deny option, on the same terms.
    once_deny: Option<PermissionOptionId>,
    /// The harness named one tool. A kind alone is not a project rule.
    specific: bool,
    /// The review was opened while permission-review hooks are holding
    /// remembered rules. Its deadline does not run until those hooks allow
    /// the tool and no rule answers it. A rule that answers it never starts
    /// that deadline.
    awaiting_consult: bool,
}

/// The once option a remember choice may store, when this review offered it
/// for this execution. Anything else is refused and nothing is stored.
fn accepted_remember<'a>(
    context: &'a PermissionContext,
    execution: &ExecutionId,
    effect: PermissionEffect,
) -> Result<&'a PermissionOptionId, AgentError> {
    if execution != &context.execution {
        return Err(AgentError::InvalidInput(
            "this review belongs to a different execution".into(),
        ));
    }
    match effect {
        PermissionEffect::Allow => context.once_allow.as_ref(),
        PermissionEffect::Deny => context.once_deny.as_ref(),
    }
    .ok_or_else(|| AgentError::InvalidInput("this review has no once choice to remember".into()))
}

struct Worker<P> {
    profile: P,
    audit: Arc<dyn ExecutionAudit>,
    cancellation_cause: Option<(PermissionCancellationReason, CancellationOrigin)>,
    scope: ProcessScope,
    reader: Reader<ProcessOutput>,
    config: AcpConfig,
    capabilities: EffectiveCapabilities,
    commands: mpsc::Receiver<Command>,
    close_requested: watch::Receiver<Option<SessionCloseRequest>>,
    events: EventSender,
    sequence: i64,
    permission_sequence: Arc<AtomicU64>,
    question_sequence: Arc<AtomicU64>,
    active: Option<ActiveExecution>,
    steering: Option<PendingSteering>,
    steering_supported: bool,
    /// The connected agent advertised `promptCapabilities.image` at initialize.
    /// Whether an image can actually be delivered also needs a byte source, so
    /// the published capability is this and `config.images` together.
    agent_accepts_images: bool,
    operation_capabilities: watch::Sender<ProviderOperationCapabilities>,
    permission_authority: PermissionAuthoritySource,
    permissions: HashMap<PermissionId, RpcId>,
    /// Tool name and harness-persistent option ids for each open review, so a
    /// remember choice can be stored and answered as the once option.
    permission_context: HashMap<PermissionId, PermissionContext>,
    /// Reviews waiting on a person, and the moment the deadline answers them.
    armed_permissions: HashMap<PermissionId, ArmedPermission>,
    /// When set, a remembered rule is not read until the invocation's
    /// permission-review hooks allow the tool. A direct session execution
    /// leaves this clear and may answer an owner turn immediately.
    hold_remembered_allows: Arc<AtomicBool>,
    /// One path classification at a time. A read that outlasts its wait keeps
    /// the permit until the read returns, so the next review does not start
    /// another (`a_classification_still_running_is_not_started_again`).
    path_classification: Arc<Semaphore>,
    /// One session identity claimed by advisory updates racing session startup.
    /// Advisory payloads are not retained and conflicting identities fail startup.
    startup_advisory_session: Option<ExecutionSessionId>,
    /// Questions the agent is waiting on, by the identity an answer names.
    ///
    /// Held beside the wire request because answering one means writing a
    /// response to it, and beside what was asked because an answer is only an
    /// answer if it chooses what that question offered.
    questions: AskedQuestions,
    /// The review or ask this worker answered without registering one — a
    /// refusal.
    ///
    /// The dispatcher answers any request whose handler failed and which it
    /// cannot find among the pending reviews or open asks, so that a provider
    /// is never left waiting on a request nobody will answer. A refusal is
    /// exactly that shape — answered, never registered — so without this it
    /// would be answered twice, and answering one request twice is its own
    /// protocol fault.
    ///
    /// One request at a time is all this has to remember, because the
    /// dispatcher takes it as soon as that request's handler returns. A
    /// provider that reuses one identifier for two requests is already outside
    /// JSON-RPC, and is not something this could hold enough state to repair.
    declined: Option<RpcId>,
    /// Prompts a refusal stopped whose provider replies have not arrived.
    ///
    /// Keyed by prompt id so a second stop cannot drop the first. The session
    /// is not reported usable, and the next prompt is not dispatched, until
    /// each one is answered or its grace expires.
    stopped_prompts: Vec<StoppedPrompt>,
    /// Stopped prompts already settled because their reply grace expired.
    expired_stops: Vec<ExpiredStop>,
    /// Prompts accepted while a stop is still waiting on its provider reply.
    held_prompts: VecDeque<(DispatchedPrompt, ExecutionReply)>,
    /// When an unanswered stopped prompt must be settled.
    ///
    /// This is the same instant `finish_turn_wire` used for that stop. A later
    /// stop does not move it, so the wait is never a second grace.
    stop_reply_deadline: Option<ClockInstant>,
    /// Why this generation cannot take another turn.
    ///
    /// [`StopCleanup::Policy`] is a configured session close.
    /// [`StopCleanup::Deadline`] is an end-turn stop whose provider missed the
    /// grace. Either refuses later prompts. A missed grace is not a policy refusal.
    stop_cleanup: StopCleanup,
    shutdown_deadline: Option<ClockInstant>,
    /// True once every request from `session_configuration` has been applied.
    ///
    /// The session's configuration requests are answered in order, and the
    /// provider may send `config_option_update` while they are still going out.
    /// Checking such a notification against the finished state would reject it
    /// for reporting the state this runtime had not asked for yet.
    configured: bool,
    closing: bool,
    /// Set when this turn's end was written: `session/cancel`, or a refusal
    /// that stops the turn (the deadline's, or a person's Codex `cancel`).
    /// A later `Cancelled` prompt result is that turn ending. Any other
    /// cancelled result is session teardown, which disarms deadlines.
    turn_cancel_requested: Option<ExecutionId>,
    deferred_outcome: Option<ExecutionOutcome>,
    /// Commands a deadline took from the queue and has not run yet. They were
    /// admitted, so a batch that stops early leaves them here for teardown to
    /// answer, as it would have found them in the queue.
    carried: VecDeque<Command>,
    provider_result: Option<Result<ExecutionOutcome, AgentError>>,
    settlement_facts: SettlementFacts,
    correlation_sequence: u64,
    failure_cause: ObservationFailureCause,
    /// Assigned when this process was started. Claude and Codex record it
    /// with the hooks choice of this launch.
    launch_id: u64,
    /// Why this process was started, recorded with its hooks choice.
    launch_cause: NativeHooksLaunchCause,
    /// The conversation this process was opened for, when the open named one.
    session_key: Option<String>,
    /// Spans for the turns this process runs. Recording never fails the turn.
    traces: TraceRecorder,
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
    question_sequence: Arc<AtomicU64>,
    operation_capabilities: watch::Sender<ProviderOperationCapabilities>,
    permission_authority: PermissionAuthoritySource,
    recovery: Arc<ProcessCleanup>,
    launch_id: u64,
    launch_cause: NativeHooksLaunchCause,
    session_key: Option<String>,
    hold_remembered_allows: Arc<AtomicBool>,
    traces: Arc<LocalTraceStore>,
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
        question_sequence,
        active: None,
        steering: None,
        steering_supported: false,
        agent_accepts_images: false,
        operation_capabilities,
        permission_authority,
        permissions: HashMap::new(),
        permission_context: HashMap::new(),
        armed_permissions: HashMap::new(),
        hold_remembered_allows,
        path_classification: Arc::new(Semaphore::new(1)),
        startup_advisory_session: None,
        questions: AskedQuestions::default(),
        declined: None,
        stopped_prompts: Vec::new(),
        expired_stops: Vec::new(),
        held_prompts: VecDeque::new(),
        stop_reply_deadline: None,
        stop_cleanup: StopCleanup::None,
        shutdown_deadline: None,
        configured: false,
        closing: false,
        turn_cancel_requested: None,
        deferred_outcome: None,
        carried: VecDeque::new(),
        provider_result: None,
        settlement_facts: SettlementFacts::new(),
        correlation_sequence: 0,
        failure_cause: ObservationFailureCause::ExecutionFailed,
        launch_id,
        launch_cause,
        session_key,
        traces: TraceRecorder::new(traces.clone(), traces.capture()),
    };
    let mut execution = None;
    let startup_started = worker.config.clock.now();
    let startup = catch_worker_panic(worker.startup(restore, &mut execution)).await;
    tracing::info!(target: "nessa_sdk::timing", phase = "protocol_startup",
        elapsed_ms = worker.config.clock.now().saturating_duration_since(startup_started).as_secs_f64() * 1000.0,
        outcome = if startup.is_ok() { "success" } else { "error" },
        "agent protocol startup finished");
    let mut ready = Some(ready);
    let result = match startup {
        Ok(()) => {
            let execution = execution.as_mut().expect("startup established a context");
            tracing::info!(target: "nessa_sdk::timing", session_id = execution.id().as_str(), "agent protocol ready");
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
                Err(worker.record_failure(OperationEffectPhase::Teardown, AgentError::Closed))
            } else {
                catch_worker_panic(worker.drive(execution)).await
            }
        }
        Err(error) => Err(error),
    };
    let result = result
        .map_err(|failure| worker.ensure_failure_covered(OperationEffectPhase::Worker, failure));
    // Worker, controller and reply ownership stay outside every caught phase.
    let mut execution_reply = None;
    let mut unanswered = Vec::new();
    let completed: Result<WorkerResult, AgentError> = catch_worker_panic(async {
        Ok::<WorkerResult, AgentError>(
            worker
                .finish(&mut execution, result, &mut execution_reply, &recovery)
                .await,
        )
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
                .saturating_duration_since(worker.config.clock.now());
            let physical =
                catch_worker_panic(worker.scope.cleanup(grace, worker.config.kill_timeout)).await;
            if execution_reply.is_none() {
                execution_reply = worker.active.take().map(|active| active.reply);
            }
            unanswered = worker.take_unanswered_replies();
            let resources = match physical {
                Ok(outcome) => recovery.confirm_physical(outcome).await.resources().clone(),
                Err(error) => ResourceCleanup::Unconfirmed(error),
            };
            let _ = worker.retain_operation(OperationEffectPhase::Worker, error.clone());
            let projection =
                std::mem::replace(&mut worker.settlement_facts, SettlementFacts::new()).finalize();
            let settlement = ExecutionReport::finalized_provider(
                worker.provider_result.take(),
                resources,
                None,
                projection,
            );
            let ProviderSessionState::CleanupReported(cleanup) = settlement.session_state() else {
                unreachable!("finalized worker panic always reports cleanup")
            };
            let cleanup = cleanup.clone();
            WorkerResult {
                failure: settlement.clone().into_result().err(),
                cleanup,
                settlement,
            }
        }
    };
    if recovery.needs_resource().await {
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
    for reply in execution_reply.into_iter().chain(unanswered) {
        let _ = reply.send(ProviderExecutionReply::Finished(settlement.clone()));
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
async fn catch_worker_panic<T, E>(operation: impl Future<Output = Result<T, E>>) -> Result<T, E>
where
    E: From<AgentError>,
{
    let mut operation = Box::pin(operation);
    let result = poll_fn(|context| {
        match catch_unwind(AssertUnwindSafe(|| operation.as_mut().poll(context))) {
            Ok(poll) => poll,
            Err(_) => Poll::Ready(Err(AgentError::Protocol(
                "ACP worker phase panicked".into(),
            )
            .into())),
        }
    })
    .await;
    match catch_unwind(AssertUnwindSafe(|| drop(operation))) {
        Ok(()) => result,
        Err(_) => Err(AgentError::Protocol("ACP worker phase drop panicked".into()).into()),
    }
}

impl<P: AcpProfile> Worker<P> {
    async fn finish(
        &mut self,
        execution: &mut Option<ExecutionController>,
        result: Result<(), WorkerFailure>,
        execution_reply: &mut Option<ExecutionReply>,
        recovery: &ProcessCleanup,
    ) -> WorkerResult {
        // A16: the grant stays with the provider session when this worker
        // leaves. Drop arguments no update took before the next generation
        // of that open can attach them. A turn that ends with the worker
        // still running already dropped them in `message`.
        self.discard_unclaimed_arguments();
        // No command may enter a generation that has left its drive loop. The final
        // execution acknowledgement can wake a different runtime thread immediately.
        self.commands.close();
        let local_cancellation = self.closing && result.is_ok() && self.provider_result.is_none();
        self.begin_shutdown_grace();
        let initial_has_operation = result.as_ref().err().is_some_and(|failure| {
            failure.validate(&self.settlement_facts);
            self.settlement_facts
                .coverage_has_operation(failure.coverage())
        });
        let mut failure = result.err().map(WorkerFailure::into_error);
        if self.closing && !initial_has_operation {
            failure = match self
                .cancellation_cause
                .as_ref()
                .map(|(reason, _)| reason.view())
            {
                Some(PermissionCancellationReasonView::DeadlineExceeded) => {
                    self.failure_cause = ObservationFailureCause::DeadlineExceeded;
                    let error = AgentError::Deadline;
                    let _ = self.retain_operation(OperationEffectPhase::Teardown, error.clone());
                    Some(error)
                }
                Some(
                    PermissionCancellationReasonView::ExecutionFailed
                    | PermissionCancellationReasonView::SessionFailed,
                ) => {
                    let error = AgentError::Closed;
                    let _ = self.retain_operation(OperationEffectPhase::Teardown, error.clone());
                    Some(error)
                }
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
                let error = error.into_error();
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
                        let error = error.into_error();
                        failure = Some(retain_admitted_failure(failure, error));
                    }
                }
                Err(error) => {
                    let _ = self.retain_operation(OperationEffectPhase::Teardown, error.clone());
                    failure = Some(retain_admitted_failure(failure, error));
                }
            }
            if let Err(error) = self.send_cancellation(execution.id().as_str()).await {
                tracing::debug!(%error, "cooperative cancellation failed; physical cleanup still runs");
            }
        }
        let grace = self
            .begin_shutdown_grace()
            .saturating_duration_since(self.config.clock.now());
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
                        let error = error.into_error();
                        failure = Some(retain_admitted_failure(failure, error));
                    }
                }
                Err(error) => {
                    let retained = error.clone();
                    if let Some(error) = execution_finish_failure(execution, failure.clone(), error)
                    {
                        let _ = self.retain_operation(OperationEffectPhase::Teardown, retained);
                        failure = Some(error);
                    }
                }
            }
            if failure.is_none() && physical.is_ok() {
                if let Some(Err(error)) =
                    event.map(|event| event.and_then(|event| self.emit(event)))
                {
                    let _ =
                        self.retain_operation(OperationEffectPhase::EventDelivery, error.clone());
                    failure = Some(error);
                }
            }
            self.traces.abandon(self.trace_now());
        }
        let resources = match physical {
            Ok(outcome) => recovery.confirm_physical(outcome).await.resources().clone(),
            Err(error) => ResourceCleanup::Unconfirmed(error),
        };
        let projection =
            std::mem::replace(&mut self.settlement_facts, SettlementFacts::new()).finalize();
        let settlement = if local_cancellation {
            ExecutionReport::finalized_local_cancellation(resources, None, projection)
        } else {
            ExecutionReport::finalized_provider(
                self.provider_result.take(),
                resources,
                None,
                projection,
            )
        };
        let ProviderSessionState::CleanupReported(cleanup) = settlement.session_state() else {
            unreachable!("finalized worker settlement always reports cleanup")
        };
        let cleanup = cleanup.clone();
        // The execution reply already carries an expected authentication refusal.
        // Finalized facts and cleanup establish whether another failure exists;
        // ending this stream cleanly avoids inventing a second observation fault.
        let authentication_refusal_only = execution_reply.is_some()
            && settlement.provider_result().is_some_and(|result| {
                result
                    .as_ref()
                    .err()
                    .is_some_and(AgentError::authentication_required)
            })
            && settlement.failure().is_none()
            && cleanup.clone().into_result().is_ok();
        let mut unanswered = self.take_unanswered_replies();
        if execution_reply.is_none() {
            *execution_reply = unanswered.pop();
        }
        for reply in unanswered {
            let _ = reply.send(ProviderExecutionReply::Finished(settlement.clone()));
        }
        let published_failure = if authentication_refusal_only {
            None
        } else if execution_reply.is_some() {
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
        deadline: Option<ClockInstant>,
    ) -> Result<(), AgentError> {
        let stdin = self.scope.stdin.as_mut().ok_or(AgentError::Closed)?;
        let allowed = json_rpc::write_allowance(bytes.len());
        let result =
            json_rpc::send_encoded(&*self.config.clock, stdin, &bytes, allowed, deadline).await;
        if result == Err(AgentError::Deadline) && !self.closing {
            self.failure_cause = ObservationFailureCause::DeadlineExceeded;
            self.cancellation_cause.get_or_insert((
                PermissionCancellationReason::deadline_exceeded(),
                CancellationOrigin::Runtime,
            ));
        }
        result
    }
    fn begin_shutdown_grace(&mut self) -> ClockInstant {
        *self
            .shutdown_deadline
            .get_or_insert_with(|| self.config.clock.now() + self.config.shutdown_grace)
    }
    async fn send_cancellation(&mut self, session_id: &str) -> Result<(), AgentError> {
        let deadline = self.begin_shutdown_grace();
        let clock = self.config.clock.clone();
        let sent = within(&*clock, deadline, self.cancel_wire_requests()).await;
        // Outside the grace, not under it: a deadline ends what the provider is
        // told, never what is recorded about the asks it was told about.
        let closed = self
            .close_ended_questions(QuestionCancellation::SessionEnded)
            .await;
        match sent {
            None => return closed,
            Some(Err(AgentError::Deadline)) if clock.now() >= deadline => return closed,
            Some(Err(error)) => return closed.and(Err(error)),
            Some(Ok(())) => {}
        }
        let frame = json_rpc::encode(
            json_rpc::notification("session/cancel", json!({"sessionId": session_id})),
            self.config.max_frame_bytes,
        )?;
        let cancelled = match self.send_encoded(frame, Some(deadline)).await {
            // Expiry means cooperative grace ended, not that explicit close failed.
            Err(AgentError::Deadline) if self.config.clock.now() >= deadline => Ok(()),
            result => result,
        };
        cancelled.and(closed)
    }
    fn current_deadline(&self) -> Option<ClockInstant> {
        // An absolute shutdown grace is the deadline. A sooner steering,
        // prompt, or held-prompt timer must not cut it short, and must not
        // start another one.
        if let Some(deadline) = self.shutdown_deadline {
            return Some(deadline);
        }
        let held = self
            .held_prompts
            .iter()
            .filter_map(|(prompt, _)| prompt.deadline)
            .min();
        [
            self.stop_reply_deadline,
            self.active.as_ref().and_then(|active| active.deadline),
            self.steering.as_ref().map(|pending| pending.deadline),
            held,
        ]
        .into_iter()
        .flatten()
        .min()
    }
    /// A deadline that ends this worker, apart from a stopped reply's grace
    /// and a held prompt's own bound.
    fn operation_deadline_due(&self) -> bool {
        let now = self.config.clock.now();
        self.shutdown_deadline
            .is_some_and(|deadline| now >= deadline)
            || self
                .active
                .as_ref()
                .and_then(|active| active.deadline)
                .is_some_and(|deadline| now >= deadline)
            || self
                .steering
                .as_ref()
                .is_some_and(|pending| now >= pending.deadline)
    }
    async fn send(&mut self, value: Value) -> Result<(), AgentError> {
        self.send_before(value, self.current_deadline()).await
    }
    async fn send_before(
        &mut self,
        value: Value,
        deadline: Option<ClockInstant>,
    ) -> Result<(), AgentError> {
        let frame = json_rpc::encode(value, self.config.max_frame_bytes)?;
        self.send_encoded(frame, deadline).await
    }

    async fn rpc(
        &mut self,
        method: &str,
        params: Value,
        deadline: ClockInstant,
        execution: Option<&mut ExecutionController>,
    ) -> Result<Value, AgentError> {
        let started = self.config.clock.now();
        tracing::info!(target: "nessa_sdk::timing", phase = method, "agent startup phase started");
        let result = self.rpc_exchange(method, params, deadline, execution).await;
        tracing::info!(target: "nessa_sdk::timing", phase = method,
            elapsed_ms = self.config.clock.now().saturating_duration_since(started).as_secs_f64() * 1000.0,
            outcome = if result.is_ok() { "success" } else { "error" },
            "agent startup phase finished");
        result
    }

    async fn rpc_exchange(
        &mut self,
        method: &str,
        params: Value,
        deadline: ClockInstant,
        mut execution: Option<&mut ExecutionController>,
    ) -> Result<Value, AgentError> {
        let id = self.next_id()?;
        let frame = json_rpc::encode(
            json_rpc::request(id, method, params),
            self.config.max_frame_bytes,
        )?;
        let write_started = self.config.clock.now();
        let sent = self.send_encoded(frame, Some(deadline)).await;
        let write_finished = self.config.clock.now();
        tracing::info!(target: "nessa_sdk::timing", phase = method, request_id = id,
            elapsed_ms = write_finished.saturating_duration_since(write_started).as_secs_f64() * 1000.0,
            outcome = if sent.is_ok() { "success" } else { "error" },
            "agent startup request write finished");
        sent?;
        let response_started = write_finished;
        let mut first_frame = true;
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
                _ = wait_for_deadline(&*self.config.clock, Some(deadline)) => { self.failure_cause = ObservationFailureCause::DeadlineExceeded; self.cancellation_cause = Some((PermissionCancellationReason::deadline_exceeded(), CancellationOrigin::Runtime)); return Err(AgentError::Deadline); },
                message = self.reader.next() => message?,
            };
            if first_frame {
                tracing::info!(target: "nessa_sdk::timing", phase = method, request_id = id,
                    elapsed_ms = self.config.clock.now().saturating_duration_since(response_started).as_secs_f64() * 1000.0,
                    "agent startup first frame received");
                first_frame = false;
            }
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
                    let params = message
                        .params
                        .ok_or_else(|| json_rpc::protocol("missing session update params"))?;
                    if let Some(execution) = execution.as_deref_mut() {
                        // Startup configuration cannot hide drift that the live
                        // update path rejects. This also checks session correlation
                        // and refuses execution output before a prompt is active.
                        self.update(execution, params)?;
                    } else {
                        self.accept_startup_advisory(&params)?;
                    }
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
    ) -> Result<(), WorkerFailure> {
        // Two budgets, because the two halves of startup are not ours in the
        // same way. Everything up to the child's first answer is the operating
        // system's: exec, its first-execution scan of a freshly written
        // executable, and the runtime's own boot. That is `launch_timeout`,
        // and it is generous. Protocol work afterwards is the provider
        // answering questions it is already running to answer, and keeps the
        // tighter `startup_timeout`.
        if let Some((harness, allowed)) = self.profile.native_hooks_launch() {
            self.record_audit(
                ExecutionAuditRecord::NativeHooksLaunch(NativeHooksLaunchRecord::new(
                    self.session_key.clone(),
                    self.launch_id,
                    harness,
                    allowed,
                    self.launch_cause,
                    restore.is_some(),
                )),
                AuditEffectPhase::Lifecycle,
            )
            .await?;
        }
        let spawn_deadline = self.config.clock.now() + self.config.launch_timeout;
        // Whether saved context is being restored is decided before any step
        // runs, so every step's deadline reports it. Reading it off the step
        // would call a restoration that expired during `initialize` new.
        let context = if restore.is_some() {
            AgentStartupContext::Restored
        } else {
            AgentStartupContext::New
        };
        let questions_enabled = self.config.tools_enabled && self.profile.supports_questions();
        let init = self
            .rpc(
                "initialize",
                initialize_params(questions_enabled),
                spawn_deadline,
                None,
            )
            .await
            .map_err(|error| startup_deadline(error, AgentStartupPhase::Initialize, context))?;
        // The child has answered, so it is running and scanned. Start the
        // protocol budget here rather than carrying the remainder of a budget
        // that was sized for the operating system's work.
        let deadline = self.config.clock.now() + self.config.startup_timeout;
        check_initialize(&self.profile, &init)?;
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
                )
                .into());
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
        let execution = execution.insert(
            ExecutionController::with_authority_source(id, self.permission_authority.clone())
                .launched_with(
                    self.profile
                        .native_hooks_launch()
                        .map(|(_, allowed)| allowed),
                ),
        );
        if self
            .startup_advisory_session
            .take()
            .is_some_and(|claimed| &claimed != execution.id())
        {
            return Err(json_rpc::protocol("startup advisory belongs to another session").into());
        }
        // Resume identifies its target in the request. ACP does not require
        // repeating that identity in the response; reject a conflicting extension
        // without losing the local closure evidence for the requested context.
        if restore.is_some()
            && result.get("sessionId").is_some()
            && fields::identifier(&result, "sessionId")? != execution.id().as_str()
        {
            return Err(json_rpc::protocol("provider resumed a different session").into());
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
        // with every test green, because every profile that exists today
        // returns at least one request (the mode, after a model and an effort
        // level where it selects them).
        self.profile
            .verify_session(&result, &self.capabilities, configuration.is_empty())?;
        let last = configuration.len().saturating_sub(1);
        // The response held to the final state: what the agent offers is read
        // from it once every selection has been applied.
        let mut settled = result;
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
            settled = result;
        }
        // An agent may only list its effort option once its model is selected
        // (Codex does), so the levels are read from the settled response.
        let effort_levels =
            thought_level::offered(&settled, &self.capabilities, self.profile.effort_option())?;
        self.configured = true;
        // Native hook suppression is published with the other negotiated facts.
        // Unsupported does not fail this open or the same publish on
        // restoration. Codex holds that with
        // `codex_startup_and_restoration_continue_when_native_hook_suppression_is_unsupported`.
        let profile_capabilities = self.profile.operation_capabilities(&init);
        let published = ProviderOperationCapabilities {
            negotiated: true,
            native_steering: self.steering_supported,
            // Only an agent that said so receives an image, and only when
            // this process has somewhere to read the bytes from.
            image_input: self.config.images.is_some() && self.agent_accepts_images,
            session_resume: init
                .pointer("/agentCapabilities/sessionCapabilities/resume")
                .is_some_and(Value::is_object),
            supports_questions: questions_enabled,
            effort_levels,
            ..profile_capabilities
        };
        self.operation_capabilities.send_replace(published);
        admit_configured_outcome(
            &self.config.refusal,
            OperationCapabilities::resolve(published),
        )?;
        Ok(())
    }

    /// The ACP content blocks for one user message, in [`content_blocks`]'
    /// order: the block of what apps gave the model first, when there is one,
    /// then the message itself. Nothing has been written when this fails, so
    /// the caller rejects the input without a dispatch.
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

    async fn drive(&mut self, execution: &mut ExecutionController) -> Result<(), WorkerFailure> {
        enum Input {
            Close,
            Command(Option<Command>),
            Message(Result<Envelope, AgentError>),
            Deadline,
            PermissionDeadline,
            ConsumerGone,
        }
        loop {
            let deadline = self.current_deadline();
            let permission_deadline = self.next_permission_deadline();
            let input = if self.close_requested.borrow().is_some() && !self.closing {
                Input::Close
            } else {
                // A due permission deadline is ahead of unrelated commands. An
                // answer or cancellation of that review already in the queue is
                // taken before it settles. A provider frame stays behind it, so
                // a turn end that is also ready is read after the deadline.
                // `an_answer_already_waiting_at_the_deadline_beats_the_deadline`
                // and `an_unrelated_command_already_queued_does_not_hold_a_due_deadline`.
                tokio::select! { biased;
                    _ = self.close_requested.changed(), if !self.closing => Input::Close,
                    _ = self.events.closed() => Input::ConsumerGone,
                    _ = wait_for_deadline(&*self.config.clock, deadline), if deadline.is_some() => Input::Deadline,
                    _ = wait_for_deadline(&*self.config.clock, permission_deadline), if permission_deadline.is_some() => Input::PermissionDeadline,
                    command = self.commands.recv(), if !self.closing => Input::Command(command),
                    message = self.reader.next() => Input::Message(message),
                }
            };
            match input {
                Input::Close | Input::Command(None) => {
                    // `cancel_wire_requests`, called from the cancellation
                    // below, clears armed deadlines before this branch returns
                    // to the wait. The shutdown grace can outlast a review, and
                    // the deadline must not answer during it.
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
                    }
                    .map_err(|error| self.record_failure(OperationEffectPhase::Teardown, error))?;
                    self.record_lifecycle(records).await?;
                    if let Err(error) = self.send_cancellation(execution.id().as_str()).await {
                        return Err(self.record_failure(OperationEffectPhase::Teardown, error));
                    }
                    if self.active.is_none() {
                        return Ok(());
                    }
                }
                Input::ConsumerGone => {
                    self.cancellation_cause = Some((
                        PermissionCancellationReason::event_consumer_dropped(),
                        CancellationOrigin::Runtime,
                    ));
                    return Err(self.record_failure(
                        OperationEffectPhase::EventDelivery,
                        AgentError::Backpressure,
                    ));
                }
                Input::Deadline => {
                    if self.closing {
                        return Ok(());
                    }
                    // A held prompt's bound can pass while the stopped turn is
                    // still inside its grace. That prompt is refused here and
                    // is not dispatched; the wake is not itself a worker failure.
                    self.refuse_expired_held_prompts();
                    if self.stop_reply_due() {
                        self.expire_stopped_prompts();
                        self.flush_held_prompts(execution).await?;
                        continue;
                    }
                    if !self.operation_deadline_due() {
                        continue;
                    }
                    self.failure_cause = ObservationFailureCause::DeadlineExceeded;
                    self.cancellation_cause = Some((
                        PermissionCancellationReason::deadline_exceeded(),
                        CancellationOrigin::Runtime,
                    ));
                    return Err(
                        self.record_failure(OperationEffectPhase::Worker, AgentError::Deadline)
                    );
                }
                Input::Command(Some(command)) => {
                    self.command(execution, command).await?;
                    if self.deferred_outcome.is_some() {
                        return Ok(());
                    }
                }
                Input::PermissionDeadline => {
                    // Answers and cancellations of a due review were admitted
                    // before this wait returned. They decide it. The snapshot
                    // is what the queue held as the deadline won, taken before
                    // anything here is awaited: each command taken frees a
                    // slot, and a command admitted into it came after the
                    // deadline, so it is not taken here
                    // (`a_command_admitted_after_the_deadline_is_not_in_its_snapshot`).
                    // Anything else in that snapshot waits until those reviews
                    // are settled
                    // (`an_unrelated_command_already_queued_does_not_hold_a_due_deadline`).
                    // Taken commands wait in `carried`, not a local, so an
                    // early return still leaves them for teardown
                    // (`an_admitted_command_outlives_a_deadline_batch_that_stops`).
                    // `now` is fixed with the snapshot. A review that falls
                    // due while this batch awaits may have an answer admitted
                    // after the snapshot, so it waits for its own wake and
                    // its own snapshot
                    // (`a_review_due_during_another_settlement_keeps_its_queued_answer`).
                    let now = self.config.clock.now();
                    let admitted = self.commands.len();
                    let (due, later): (Vec<_>, Vec<_>) =
                        admitted_before_deadline(&mut self.commands, admitted)
                            .into_iter()
                            .partition(|command| self.command_answers_due_review(command, now));
                    let answers = due.len();
                    self.carried.extend(due);
                    self.carried.extend(later);
                    for _ in 0..answers {
                        let command = self.carried.pop_front().expect("carried answer");
                        self.command(execution, command).await?;
                        if self.deferred_outcome.is_some() {
                            return Ok(());
                        }
                    }
                    self.settle_due_permissions(execution, now).await?;
                    while let Some(command) = self.carried.pop_front() {
                        self.command(execution, command).await?;
                        if self.deferred_outcome.is_some() {
                            return Ok(());
                        }
                    }
                }
                Input::Message(message) => {
                    let message = message.map_err(|error| {
                        self.record_failure(OperationEffectPhase::Worker, error)
                    })?;
                    self.message(execution, message, self.current_deadline())
                        .await?;
                    self.flush_held_prompts(execution).await?;
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
    ) -> Result<(), WorkerFailure> {
        // Seal admission before awaiting any effects. Enqueue uses try_send (no
        // caller-held permits), so later requests cannot grow this queue. Receiving
        // until closed also joins a concurrent try_send already inside admission.
        // Its admitted decisions precede aggregate closure even though the close
        // notification arrived over an independently ordered watch channel.
        self.commands.close();
        let mut failure: Option<WorkerFailure> = None;
        // A deadline batch that stopped early holds commands admitted before
        // those still queued, so they are answered first.
        loop {
            let command = match self.carried.pop_front() {
                Some(command) => command,
                None => match self.commands.recv().await {
                    Some(command) => command,
                    None => break,
                },
            };
            match command {
                command @ (Command::Answer(..)
                | Command::CancelPermission(..)
                | Command::AnswerQuestion(..)) => {
                    if let Err(error) = self.command(execution, command).await {
                        // Still attempt later admitted decisions and their audit
                        // evidence; each audit/write already has a deadline.
                        // Each admitted decision may fail independently. Keep all
                        // failures in admission order even when its caller vanished.
                        failure = Some(match failure {
                            Some(previous) => previous.combine(error),
                            None => error,
                        });
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
                Command::SetApprovalMode(_, reply) | Command::SetEffortLevel(_, reply) => {
                    let _ = reply.send(Err(AgentError::Closed));
                }
                Command::CancelTurn(_, reply) => {
                    let _ = reply.send(Err(ProviderOperationFailure::new(
                        AgentError::Closed,
                        ProviderSessionState::CleanupRequired,
                    )));
                }
                Command::ConsultRemembered(_, reply) => {
                    let _ = reply.send(None);
                }
                Command::ReleaseConsult(_, reply) => {
                    let _ = reply.send(Ok(()));
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
        dispatch_deadline: Option<ClockInstant>,
    ) -> Result<DispatchReadiness, WorkerFailure> {
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
                    .is_some_and(|deadline| self.config.clock.now() >= deadline)
                {
                    // The drive loop owns this already-established lifecycle deadline.
                    return Ok(DispatchReadiness::Interrupted(AgentError::Deadline));
                }
                if dispatch_deadline.is_some_and(|deadline| self.config.clock.now() >= deadline) {
                    self.failure_cause = ObservationFailureCause::DeadlineExceeded;
                    self.cancellation_cause = Some((
                        PermissionCancellationReason::deadline_exceeded(),
                        CancellationOrigin::Runtime,
                    ));
                    return Err(AgentError::Deadline.into());
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
                            _ = wait_for_deadline(&*self.config.clock, deadline), if deadline.is_some() => {
                                self.failure_cause = ObservationFailureCause::DeadlineExceeded;
                                self.cancellation_cause = Some((
                                    PermissionCancellationReason::deadline_exceeded(),
                                    CancellationOrigin::Runtime,
                                ));
                                return Err(AgentError::Deadline.into());
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
    ) -> Result<(), WorkerFailure> {
        match command {
            Command::SetEffortLevel(level, reply) => {
                self.set_effort_level(execution, level, reply).await
            }
            Command::SetApprovalMode(mode, reply) => {
                self.set_approval_mode(execution, mode, reply).await
            }
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
            Command::ConsultRemembered(id, reply) => {
                let _ = reply.send(self.consult_remembered(&id));
                Ok(())
            }
            Command::ReleaseConsult(id, reply) => {
                let due = self.release_consult(&id);
                if due {
                    if let Some(armed) = self.armed_permissions.remove(&id) {
                        self.settle_permission(execution, id, armed).await?;
                    }
                }
                let _ = reply.send(Ok(()));
                Ok(())
            }
            Command::AnswerQuestion(answer, reply) => self.answer_question(answer, reply).await,
            Command::CancelTurn(turn, reply) => self.cancel_turn(execution, turn, reply).await,
        }
    }
    async fn cancel_turn(
        &mut self,
        execution: &mut ExecutionController,
        turn: ExecutionId,
        reply: tokio::sync::oneshot::Sender<ProviderOperationResult<()>>,
    ) -> Result<(), WorkerFailure> {
        if self.closing {
            let _ = reply.send(Err(ProviderOperationFailure::new(
                AgentError::Closed,
                ProviderSessionState::CleanupRequired,
            )));
            return Ok(());
        }
        let active = self
            .active
            .as_ref()
            .map(|active| active.execution_id.clone());
        if active.as_ref() != Some(&turn) {
            let _ = reply.send(Err(ProviderOperationFailure::new(
                AgentError::Unsupported("turn is not active".into()),
                ProviderSessionState::Usable,
            )));
            return Ok(());
        }
        let frame = match json_rpc::encode(
            json_rpc::notification(
                "session/cancel",
                json!({"sessionId": execution.id().as_str()}),
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
        let result = self.send_encoded(frame, None).await.map_err(|error| {
            ProviderOperationFailure::new(error, ProviderSessionState::CleanupRequired)
        });
        if result.is_ok() {
            self.turn_cancel_requested = Some(turn);
        }
        let _ = reply.send(result);
        Ok(())
    }
    async fn set_approval_mode(
        &mut self,
        execution: &mut ExecutionController,
        mode: ApprovalMode,
        reply: tokio::sync::oneshot::Sender<Result<(), AgentError>>,
    ) -> Result<(), WorkerFailure> {
        if self.active.is_some() || !self.permissions.is_empty() {
            let _ = reply.send(Err(AgentError::Busy));
            return Ok(());
        }
        let params = match self
            .profile
            .change_approval_mode(execution.id().as_str(), mode)
        {
            Ok(params) => params,
            Err(error) => {
                let _ = reply.send(Err(error));
                return Ok(());
            }
        };
        let deadline = self.config.clock.now() + super::steering::RESPONSE_TIMEOUT;
        let result = self
            .rpc(
                "session/set_config_option",
                params,
                deadline,
                Some(execution),
            )
            .await
            .and_then(|result| {
                self.profile
                    .verify_session(&result, &self.capabilities, true)
            });
        let _ = reply.send(result.clone());
        result.map_err(WorkerFailure::from)
    }
    /// Apply and verify one reasoning effort level on an idle session, as
    /// [`Self::set_approval_mode`] does for a preset.
    async fn set_effort_level(
        &mut self,
        execution: &mut ExecutionController,
        level: crate::domain::model_metadata::value_objects::EffortLevel,
        reply: tokio::sync::oneshot::Sender<Result<(), AgentError>>,
    ) -> Result<(), WorkerFailure> {
        if self.active.is_some() || !self.permissions.is_empty() {
            let _ = reply.send(Err(AgentError::Busy));
            return Ok(());
        }
        let params = match self
            .profile
            .change_effort_level(execution.id().as_str(), level)
        {
            Ok(params) => params,
            Err(error) => {
                let _ = reply.send(Err(error));
                return Ok(());
            }
        };
        let deadline = self.config.clock.now() + super::steering::RESPONSE_TIMEOUT;
        let result = self
            .rpc(
                "session/set_config_option",
                params,
                deadline,
                Some(execution),
            )
            .await
            .and_then(|result| {
                self.profile
                    .verify_session(&result, &self.capabilities, true)
            });
        let _ = reply.send(result.clone());
        result.map_err(WorkerFailure::from)
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
    ) -> Result<(), WorkerFailure> {
        if self.stop_cleanup != StopCleanup::None {
            let _ = reply.send(ProviderExecutionReply::Finished(self.cleanup_refusal()));
            return Ok(());
        }
        if !self.stopped_prompts.is_empty() {
            self.held_prompts.push_back((prompt, reply));
            return Ok(());
        }
        let deadline = prompt.deadline;
        self.traces.accept_usage_before_turn();
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
            self.traces.release_usage_before_turn();
            return readiness.map(|_| ());
        }
        if reply.is_closed() {
            self.traces.release_usage_before_turn();
            return Ok(());
        }
        if self.steering.is_some() {
            let _ = reply.send(ProviderExecutionReply::Rejected(AgentError::Busy));
            self.traces.release_usage_before_turn();
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
            self.traces.release_usage_before_turn();
            return Ok(());
        }
        let blocks = match self.prompt_blocks(&input.user_message, images) {
            Ok(blocks) => blocks,
            Err(error) => {
                let _ = reply.send(ProviderExecutionReply::Rejected(error));
                self.traces.release_usage_before_turn();
                return Ok(());
            }
        };
        let id = match self.pending_id() {
            Ok(id) => id,
            Err(error) => {
                self.traces.release_usage_before_turn();
                return Err(error.into());
            }
        };
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
                self.traces.release_usage_before_turn();
                return Ok(());
            }
        };
        let user_text = if self.traces.captures_content() {
            input.user_message.text_str().to_owned()
        } else {
            String::new()
        };
        let execution_id = input.execution_id;
        if let Err(error) = execution.begin_execution(execution_id.clone()) {
            let _ = reply.send(ProviderExecutionReply::Rejected(error));
            self.traces.release_usage_before_turn();
            return Ok(());
        }
        self.sequence = id;
        tracing::info!(target: "nessa_sdk::timing", session_id = execution.id().as_str(),
            execution_id = execution_id.as_str(), "agent prompt dispatch started");
        self.active = Some(ActiveExecution {
            first_response_started: Some(self.config.clock.now()),
            id,
            execution_id: execution_id.clone(),
            reply,
            deadline,
        });
        self.provider_result = None;
        self.profile.begin_execution();
        if let Err(error) = self.send_encoded(frame, deadline).await {
            self.traces.release_usage_before_turn();
            return Err(WorkerFailure::from(error));
        }
        self.begin_trace(&execution_id, &user_text);
        Ok(())
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
    ) -> Result<(), WorkerFailure> {
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
        self.send_encoded(frame, write_deadline)
            .await
            .map_err(Into::into)
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
    ) -> Result<(), WorkerFailure> {
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
        self.armed_permissions.remove(record.request().id());
        if let Err(error) = self.record_cancellations(vec![record.clone()]).await {
            let _ = reply.send(Err(ProviderOperationFailure::permission_answer(
                error.error().clone(),
                ProviderSessionState::CleanupRequired,
                PermissionSelectionState::Consumed,
            )));
            return Err(error);
        }
        let delivery = self
            .send(permission_wire::permission_cancel(&wire_id))
            .await;
        let failure = if let Err(error) = &delivery {
            let correlation = self.next_correlation();
            Some(WorkerFailure::new(
                error.clone(),
                self.retain_operation(
                    OperationEffectPhase::PermissionDelivery(correlation),
                    error.clone(),
                ),
            ))
        } else {
            None
        };
        let _ = reply.send(delivery.clone().map(|()| record).map_err(|error| {
            ProviderOperationFailure::new(error, ProviderSessionState::CleanupRequired)
        }));
        match (delivery, failure) {
            (Ok(()), None) => Ok(()),
            (Err(_), Some(failure)) => Err(failure),
            _ => unreachable!("delivery failure and retained fact agree"),
        }
    }
    /// A Nessa remember choice stores a project rule, then answers this review
    /// with the harness's once wire option of the same effect. The logical
    /// selection and Explicit attribution stay as submitted for receipt and
    /// audit correlation. A refusal leaves the review pending.
    fn remember_and_map_answer(
        &self,
        answer: &PermissionAnswer,
    ) -> Result<PermissionOptionId, AgentError> {
        let Some(effect) = remember_effect(answer.option_id.as_str()) else {
            return Ok(answer.option_id.clone());
        };
        let context = self
            .permission_context
            .get(&answer.id)
            .ok_or(AgentError::StalePermission)?;
        // The once option and the execution are checked before anything is
        // stored. A choice that was filtered out, never offered, or that names
        // another execution leaves no rule behind.
        let once = accepted_remember(context, &answer.execution_id, effect)?.clone();
        if !context.specific {
            return Err(AgentError::InvalidInput(
                "this review does not name one tool".into(),
            ));
        }
        let source = self
            .profile
            .native_hooks_launch()
            .map(|(name, _)| name)
            .ok_or_else(|| {
                AgentError::InvalidInput("this harness does not remember permissions".into())
            })?;
        let conversation = self.session_key.as_deref().unwrap_or("");
        if !self
            .config
            .permission_rules
            .offers_remember(conversation, &answer.execution_id)
        {
            return Err(AgentError::InvalidInput(
                "only the owner can remember a permission".into(),
            ));
        }
        let caller = self
            .config
            .permission_rules
            .answerer(
                conversation,
                &answer.execution_id,
                answer.attribution.actor().request_id(),
            )
            .ok_or_else(|| {
                AgentError::InvalidInput("the caller of this answer was not verified".into())
            })?;
        let workspace = self.config.workspace.to_str().unwrap_or("");
        let tool = context.tool.clone();
        self.config
            .permission_rules
            .remember(RememberRequest {
                workspace,
                source,
                tool: &tool,
                effect,
                granted_by: answer.attribution.actor(),
                caller,
                conversation_id: conversation,
                execution_id: &answer.execution_id,
            })
            .map_err(|error| match error {
                crate::application::agent_execution::permissions::RememberError::Refused => {
                    AgentError::InvalidInput("only the owner can remember a permission".into())
                }
                crate::application::agent_execution::permissions::RememberError::Unavailable => {
                    AgentError::InvalidInput("the permission rule could not be stored".into())
                }
            })?;
        Ok(once)
    }
    /// Answer one pending review with the caller's selection: record it, send
    /// it, then record what delivery did. Queue admission transferred ownership
    /// to this worker, so a dropped reply waiter must not erase any of that.
    async fn answer_permission(
        &mut self,
        execution: &mut ExecutionController,
        answer: PermissionAnswer,
        reply: oneshot::Sender<ProviderOperationResult<PermissionResolution>>,
    ) -> Result<(), WorkerFailure> {
        let permission_id = answer.id.clone();
        let answered_execution = answer.execution_id.clone();
        let wire_option = match self.remember_and_map_answer(&answer) {
            Ok(option) => option,
            Err(error) => {
                let _ = reply.send(Err(ProviderOperationFailure::permission_answer(
                    error,
                    ProviderSessionState::Usable,
                    PermissionSelectionState::Pending,
                )));
                return Ok(());
            }
        };
        // The rule and its audit already committed inside `remember`. A later
        // failure to record this answer does not remove that grant.
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
        if let PermissionStateView::Answered { decision, .. } = resolution.request().state() {
            self.traces.permission(
                self.trace_now(),
                PermissionFact::Answered {
                    id: resolution.request().id().as_str().to_owned(),
                    tool_id: resolution.request().tool_id().as_str().to_owned(),
                    effect: decision.effect().as_str().to_owned(),
                    decider: super::gen_ai::permission_decider(resolution.attribution().basis())
                        .to_owned(),
                },
            );
        }
        self.armed_permissions.remove(&permission_id);
        let correlation = self.next_correlation();
        if let Err(error) = self
            .record_answer(
                resolution.clone(),
                PermissionAnswerDelivery::Selected,
                correlation,
            )
            .await
        {
            let _ = reply.send(Err(ProviderOperationFailure::permission_answer(
                error.error().clone(),
                ProviderSessionState::CleanupRequired,
                PermissionSelectionState::Consumed,
            )));
            return Err(error);
        }
        let wire_id = self
            .permissions
            .remove(&permission_id)
            .expect("pending wire permission");
        self.permission_context.remove(&permission_id);
        let response = permission_wire::selected(&wire_id, wire_option.as_str());
        let result = self.send(response).await;
        // Codex ends the turn when the person picks `cancel`. The cancelled
        // result that follows is that turn ending. `decline` does not stop
        // the turn, so the same cancelled result after it is unrequested and
        // the session is torn down. Without the stop check, every answer
        // would be treated as that turn ending. Only a refusal stops a turn:
        // an allow that carries the stopping id is still an allow
        // (`an_allow_named_cancel_then_an_unasked_cancel_ends_the_session`).
        let refused = matches!(
            resolution.request().state(),
            PermissionStateView::Answered { decision, .. }
                if decision.effect() == PermissionEffect::Deny
        );
        if result.is_ok()
            && refused
            && self
                .profile
                .deadline_deny_preference()
                .stops_turn(wire_option.as_str())
            && self
                .active
                .as_ref()
                .is_some_and(|active| active.execution_id == answered_execution)
        {
            self.turn_cancel_requested = Some(answered_execution);
        }
        let delivery_failure = result.as_ref().err().map(|error| {
            WorkerFailure::new(
                error.clone(),
                self.retain_operation(
                    OperationEffectPhase::PermissionDelivery(correlation),
                    error.clone(),
                ),
            )
        });
        let delivery = match &result {
            Ok(()) => PermissionAnswerDelivery::Written,
            Err(error) => PermissionAnswerDelivery::Failed(error.clone()),
        };
        if let Err(audit_failure) = self
            .record_answer(resolution.clone(), delivery, correlation)
            .await
        {
            let failure = match delivery_failure {
                Some(delivery_failure) => delivery_failure.combine(audit_failure).with_error(
                    AgentError::PermissionAnswerDeliveryAndAuditFailure {
                        delivery_error: Box::new(
                            result.expect_err("retained delivery failure has failed result"),
                        ),
                        cleanup_error: None,
                    },
                ),
                None => audit_failure,
            };
            let _ = reply.send(Err(ProviderOperationFailure::permission_answer(
                failure.error().clone(),
                ProviderSessionState::CleanupRequired,
                PermissionSelectionState::Consumed,
            )));
            return Err(failure);
        }
        let _ = reply.send(result.clone().map(|()| resolution).map_err(|error| {
            ProviderOperationFailure::permission_answer(
                error,
                ProviderSessionState::CleanupRequired,
                PermissionSelectionState::Consumed,
            )
        }));
        match (result, delivery_failure) {
            (Ok(()), None) => Ok(()),
            (Err(_), Some(failure)) => Err(failure),
            _ => unreachable!("delivery failure and retained fact agree"),
        }
    }
    async fn message(
        &mut self,
        execution: &mut ExecutionController,
        message: Envelope,
        response_deadline: Option<ClockInstant>,
    ) -> Result<(), WorkerFailure> {
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
                } else if method == "elicitation/create" {
                    // A question the agent is waiting on. Like a review it is
                    // answered, never ignored: a request this client leaves
                    // open is a turn that never ends.
                    let result = self
                        .question(execution, id.clone(), params, response_deadline)
                        .await;
                    let answered = self.declined.take().is_some_and(|wire_id| wire_id == id);
                    if result.is_err()
                        && !answered
                        && !self.questions.values().any(|open| open.wire_id == id)
                    {
                        let _ = self
                            .send_before(question_wire::cancelled(&id), response_deadline)
                            .await;
                    }
                    result?;
                } else {
                    self.send_before(json_rpc::unsupported(&id), response_deadline)
                        .await?;
                }
            } else if method == "session/request_permission" {
                return Err(
                    json_rpc::protocol("permission request requires an RPC identifier").into(),
                );
            } else if method == "elicitation/create" {
                return Err(json_rpc::protocol("question requires an RPC identifier").into());
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
                    self.armed_permissions.remove(&id);
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
                } else if let Some(id) = self
                    .questions
                    .iter()
                    .find_map(|(id, open)| (open.wire_id == wire_id).then(|| id.clone()))
                {
                    // A provider taking its question back ends the ask exactly
                    // as a teardown does, for a different recorded reason.
                    // Left open, a surface would keep offering it, and a later
                    // answer would be written into a request nobody is waiting on.
                    let open = self.questions.remove(&id).expect("question found above");
                    let delivery = self.send(question_wire::cancelled(&open.wire_id)).await;
                    let closed = self
                        .close_question(
                            open,
                            QuestionCancellation::ProviderWithdrawal,
                            delivery.clone(),
                        )
                        .await;
                    delivery.and(closed)?;
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
                return Err(error.into());
            }
            return Ok(());
        }
        if let Some(index) = self
            .stopped_prompts
            .iter()
            .position(|stopped| message.id == Some(RpcId::Number(stopped.prompt_id)))
        {
            let stopped = self.stopped_prompts.remove(index);
            return self.complete_stopped_prompt(stopped, &message).await;
        }
        if let Some(index) = self
            .expired_stops
            .iter()
            .position(|expired| message.id == Some(RpcId::Number(expired.prompt_id)))
        {
            let expired = self.expired_stops.remove(index);
            return self
                .record_late_reply(expired.session_id, expired.execution_id, &message)
                .await;
        }
        let active = self
            .active
            .as_ref()
            .ok_or_else(|| json_rpc::protocol("response without active prompt"))?;
        if message.id != Some(RpcId::Number(active.id)) {
            return Err(json_rpc::protocol("response for a different prompt").into());
        }
        match prompt_reply(&message)? {
            PromptReply::Failed(error) => {
                // Keep the reply until teardown has recorded pending cancellations.
                self.provider_result = Some(Err(error.clone()));
                let id = self.settlement_facts.record_provider_result();
                return Err(WorkerFailure::new(error, [id]));
            }
            PromptReply::Outcome(result) => {
                if let Some(value) = message.result.as_ref() {
                    self.traces.usage(super::usage::reported_usage(value));
                }
                self.provider_result = Some(Ok(result));
                let active_turn = self
                    .active
                    .as_ref()
                    .expect("validated active prompt")
                    .execution_id
                    .clone();
                let turn_cancel_requested =
                    self.turn_cancel_requested.take().as_ref() == Some(&active_turn);
                if result == ExecutionOutcome::Cancelled && !turn_cancel_requested {
                    // The provider ended the prompt without a cancel we sent.
                    // The cancelled result is session teardown. `cancel_wire_requests`,
                    // called from that teardown, clears armed deadlines before the
                    // shutdown grace, so none answers a request the provider
                    // stopped waiting on.
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
                    let _grace = self.finish_turn_wire().await?;
                    self.emit(event)?;
                    self.discard_unclaimed_arguments();
                    let active = self.active.take().expect("validated active prompt");
                    let _ =
                        active
                            .reply
                            .send(ProviderExecutionReply::Finished(ExecutionReport::new(
                                Some(Ok(result)),
                                None,
                                ProviderSessionState::Usable,
                            )));
                    self.provider_result = None;
                }
            }
        }
        Ok(())
    }
    /// Arguments still waiting were not claimed by an update of the execution
    /// that just ended. Drop them before the session accepts another turn.
    fn discard_unclaimed_arguments(&self) {
        if let Some(forwarded) = self.config.stand_ins.forwarded() {
            forwarded.discard_arguments();
        }
    }
    fn emit(&mut self, event: ExecutionEvent) -> Result<(), AgentError> {
        let at = self.trace_now();
        super::gen_ai::observe_event(&mut self.traces, at, &event);
        let result = self.events.try_send(event);
        event_publication_result(&mut self.cancellation_cause, result)
    }
    fn trace_now(&self) -> Duration {
        self.config.clock.now().since_origin()
    }
    fn begin_trace(&mut self, execution_id: &ExecutionId, user_text: &str) {
        let (agent_name, native_hooks) = match self.profile.native_hooks_launch() {
            Some((name, allowed)) => (name, Some(allowed)),
            None => ("agent", None),
        };
        let conversation = self
            .session_key
            .clone()
            .filter(|key| !key.is_empty())
            .unwrap_or_else(|| execution_id.as_str().to_owned());
        self.traces.begin_turn(
            self.trace_now(),
            TurnIdentity::new(
                conversation,
                execution_id.as_str(),
                self.capabilities.model().provider().as_str(),
                agent_name,
                self.capabilities.model().model_id(),
                native_hooks,
            ),
            user_text,
        );
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
    fn accept_startup_advisory(&mut self, params: &Value) -> Result<(), AgentError> {
        let session = ExecutionSessionId::new(fields::identifier(params, "sessionId")?)
            .map_err(|error| json_rpc::protocol(&error.to_string()))?;
        let update = params
            .get("update")
            .ok_or_else(|| json_rpc::protocol("missing session update"))?;
        if fields::string(update, "sessionUpdate")? != "available_commands_update" {
            return Err(json_rpc::protocol(
                "non-advisory session update before startup context admission",
            ));
        }
        if self
            .startup_advisory_session
            .as_ref()
            .is_some_and(|claimed| claimed != &session)
        {
            return Err(json_rpc::protocol(
                "conflicting session identities in startup advisories",
            ));
        }
        self.startup_advisory_session = Some(session);
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
            "usage_update" => {
                self.traces.usage(super::usage::reported_usage(update));
                return Ok(());
            }
            "agent_message_chunk" | "agent_thought_chunk" | "tool_call" | "tool_call_update" => {
                // A stopped turn's updates have no prompt id. While a stop is
                // unresolved, or its grace has expired and the late reply has
                // not arrived, they are dropped so they cannot fail the worker.
                if !self.stopped_prompts.is_empty() || !self.expired_stops.is_empty() {
                    return Ok(());
                }
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
            let is_answer = kind == "agent_message_chunk" && !text.is_empty();
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
            self.emit(execution.message_event(&target, chunk)?)?;
            if is_answer {
                if let Some(started) = self
                    .active
                    .as_mut()
                    .and_then(|active| active.first_response_started.take())
                {
                    tracing::info!(target: "nessa_sdk::timing",
                        metric = "agent_first_text_response_ms",
                        session_id = execution.id().as_str(), execution_id = target.as_str(),
                        elapsed_ms = self.config.clock.now().saturating_duration_since(started).as_secs_f64() * 1000.0,
                        "agent first text response");
                }
            }
            Ok(())
        } else {
            if !self.config.tools_enabled {
                return Err(json_rpc::protocol("tool event in a text-only binding"));
            }
            let forwarded = self.config.stand_ins.forwarded();
            let tool = attach_arguments(
                attach_forwarded(self.profile.tool_call(update)?, forwarded),
                forwarded,
            );
            let name = self.profile.traced_tool_name(tool.id().as_str());
            let arguments = self
                .traces
                .captures_content()
                .then(|| self.profile.traced_tool_arguments(tool.id().as_str()))
                .flatten();
            let fact =
                super::gen_ai::tool_fact(name, arguments, &tool, self.traces.captures_content());
            let event = execution.tool_event(&target, tool)?;
            self.traces.tool(self.trace_now(), fact);
            self.emit(event)
        }
    }
    async fn permission(
        &mut self,
        execution: &mut ExecutionController,
        wire_id: RpcId,
        params: Value,
        response_deadline: Option<ClockInstant>,
    ) -> Result<(), WorkerFailure> {
        self.check_session(execution, &params)?;
        if self.closing
            || self.active.is_none()
            || !self.stopped_prompts.is_empty()
            || !self.config.tools_enabled
        {
            return self
                .send_before(
                    permission_wire::permission_cancel(&wire_id),
                    response_deadline,
                )
                .await
                .map_err(Into::into);
        }
        if self.permissions.len() >= MAX_OPEN_REVIEWS
            || self.permissions.values().any(|id| *id == wire_id)
        {
            return Err(json_rpc::protocol("permission request limit or duplicate ID").into());
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
            // A name longer than a review may carry could not be shown, so
            // it is refused here rather than left waiting unseen.
            Ok(input) if input.name.len() > MAX_REVIEW_TOOL_NAME_BYTES => {
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
            Ok(input) => input,
            Err(error) => {
                self.profile.note_declined_permission(&params);
                return self
                    .decline_review(
                        execution,
                        wire_id,
                        &params,
                        decline_reason(&error),
                        response_deadline,
                    )
                    .await;
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
        let execution_id = self
            .active
            .as_ref()
            .expect("validated active prompt")
            .execution_id
            .clone();
        let source = self.profile.native_hooks_launch().map(|(name, _)| name);
        // A project rule is one tool. A kind such as Codex `execute` is every
        // command of that kind, so it is not offered and does not settle one.
        let specific = self.profile.permission_names_a_tool(&params);
        let remember = source.is_some()
            && specific
            && self
                .config
                .permission_rules
                .offers_remember(self.session_key.as_deref().unwrap_or(""), &execution_id);
        let admitted = match permission_wire::admit(
            &params,
            &self.config.permissions,
            if source.is_some() {
                permission_wire::PersistentKinds::Keep
            } else {
                permission_wire::PersistentKinds::Drop
            },
            remember,
        ) {
            Ok(admitted) => admitted,
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
        let options = admitted.options;
        let persistent = admitted.persistent;
        #[allow(deprecated, reason = "Rust 1.89 MSRV; try_update requires Rust 1.95")]
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
        let asked = self
            .active
            .as_ref()
            .expect("validated active prompt")
            .execution_id
            .clone();
        let observed = execution.observation_after(&tool);
        let class = bounded_review_class(
            &*self.config.clock,
            &self.path_classification,
            *observed.kind(),
            observed.locations().clone(),
            self.config.workspace.clone(),
            Arc::clone(&self.config.workspace_paths),
        )
        .await;
        let settlement = self.config.permission_deadlines.settlement(
            class,
            &options,
            &self.profile.deadline_deny_preference(),
        );
        // One reading of the clock arms the deadline and dates the card, so
        // the two cannot differ by whatever happens between two readings.
        let armed_at = self.config.clock.now();
        // A wall clock past what a client can read gives no moment a card
        // could show, so the review waits for a person as it would with no
        // deadline, rather than be decided at a time nobody was told.
        let promised = settlement
            .deadline()
            .clone()
            .promise(unix_deadline_ms(
                &*self.config.clock,
                armed_at,
                settlement.deadline().wait(),
            ))
            .ok();
        let armed = promised.is_some();
        let tool_name = input.name.clone();
        let workspace = self.config.workspace.to_str().unwrap_or("");
        // A review hook is the shared-folder and Drive grant check. While it
        // is set, the rule is not read here. The invocation consults it only
        // after that check allows the tool. An unpaired member is not an
        // owner turn; without that hook, no rule answers the review.
        let hold = self.hold_remembered_allows.load(Ordering::Acquire);
        let owner_turn = self
            .config
            .permission_rules
            .offers_remember(self.conversation_scope(), &execution_id);
        let ruled = if !hold && owner_turn {
            if specific { source } else { None }.and_then(|source| {
                self.config.permission_rules.settle(
                    crate::application::agent_execution::permissions::PermissionCaller::Owner,
                    workspace,
                    source,
                    &tool_name,
                )
            })
        } else {
            None
        };
        let once_allow =
            permission_wire::once_option(&options, PermissionEffect::Allow, &persistent);
        let once_deny = permission_wire::once_option(&options, PermissionEffect::Deny, &persistent);
        let once = ruled.as_ref().and_then(|rule| match rule.effect() {
            PermissionEffect::Allow => once_allow.clone(),
            PermissionEffect::Deny => once_deny.clone(),
        });
        // A live rule answers with the harness once option. The rule's own
        // id, revision, and grantor are the audit basis. A deny is sent
        // before the review is announced: it does not start the tool. An
        // allow is sent the same way unless this invocation is holding
        // allows until its permission-review hooks decide
        // (`a_review_hook_refusal_stops_a_remembered_allow_before_the_tool_starts`).
        // Those hooks are the per-turn tool policy. Sending the allow first
        // would start the tool before they can refuse it.
        if let Some((rule, once)) = ruled.zip(once) {
            let event = execution.request_permission(
                &asked,
                permission_id.clone(),
                tool,
                input,
                options,
                None,
            )?;
            self.permissions.insert(permission_id.clone(), wire_id);
            self.permission_context.insert(
                permission_id.clone(),
                PermissionContext {
                    execution: asked.clone(),
                    tool: tool_name,
                    once_allow,
                    once_deny,
                    specific,
                    awaiting_consult: false,
                },
            );
            let answer = PermissionAnswer {
                attribution: ApprovalAttribution::new(
                    rule.reference().granted_by().clone(),
                    ApprovalBasis::Rule(rule.reference().clone()),
                ),
                execution_id: asked,
                id: permission_id.clone(),
                option_id: once,
            };
            // This path runs only when no review hook is holding the rule.
            // A held rule is consulted later, after that hook permits the tool.
            let (reply, _waiting) = oneshot::channel();
            let answered = self.answer_permission(execution, answer, reply).await;
            self.emit(event).map_err(WorkerFailure::from)?;
            return answered;
        }
        let event = execution.request_permission(
            &asked,
            permission_id.clone(),
            tool,
            input,
            options,
            promised,
        )?;
        self.permissions.insert(permission_id.clone(), wire_id);
        self.permission_context.insert(
            permission_id.clone(),
            PermissionContext {
                execution: asked.clone(),
                tool: tool_name.clone(),
                once_allow,
                once_deny,
                specific,
                awaiting_consult: hold,
            },
        );
        if armed {
            let at = armed_at + settlement.deadline().wait();
            self.armed_permissions
                .insert(permission_id.clone(), ArmedPermission { at, settlement });
        }
        self.emit(event).map_err(WorkerFailure::from)?;
        // A held review is not a question for a person yet. The notice waits
        // until the hooks allow the tool and no remembered rule answers it.
        if hold {
            return Ok(());
        }
        if let Some(conversation_id) = self.session_key.clone().filter(|id| !id.is_empty()) {
            self.config.review_notices.waiting(&ReviewNotice {
                conversation_id,
                execution_id: asked,
                permission_id,
                tool: tool_name,
            });
        }
        Ok(())
    }

    fn conversation_scope(&self) -> &str {
        self.session_key.as_deref().unwrap_or("")
    }

    /// Read a remembered rule for a review whose permission-review hooks have
    /// already allowed the tool. This is the consultation those hooks gate:
    /// it is not called when they refuse. `None` when no rule answers, and
    /// nothing is sent. The caller is the turn the service armed: an owner
    /// turn is the owner, and any other turn is a grant.
    fn consult_remembered(&self, id: &PermissionId) -> Option<PermissionAnswer> {
        let context = self.permission_context.get(id)?;
        if !context.specific {
            return None;
        }
        let source = self.profile.native_hooks_launch().map(|(name, _)| name)?;
        let workspace = self.config.workspace.to_str().unwrap_or("");
        let caller = if self
            .config
            .permission_rules
            .offers_remember(self.conversation_scope(), &context.execution)
        {
            crate::application::agent_execution::permissions::PermissionCaller::Owner
        } else {
            crate::application::agent_execution::permissions::PermissionCaller::Grant
        };
        let rule = self
            .config
            .permission_rules
            .settle(caller, workspace, source, &context.tool)?;
        let once = match rule.effect() {
            PermissionEffect::Allow => context.once_allow.clone(),
            PermissionEffect::Deny => context.once_deny.clone(),
        }?;
        Some(PermissionAnswer {
            attribution: ApprovalAttribution::new(
                rule.reference().granted_by().clone(),
                ApprovalBasis::Rule(rule.reference().clone()),
            ),
            execution_id: context.execution.clone(),
            id: id.clone(),
            option_id: once,
        })
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
        execution: &mut ExecutionController,
        wire_id: RpcId,
        params: &Value,
        reason: ReviewDeclineReason,
        response_deadline: Option<ClockInstant>,
    ) -> Result<(), WorkerFailure> {
        if let ConfiguredRefusal::Stop { kind, rule } = self.config.refusal.clone() {
            return self
                .stop_declined_turn(
                    execution,
                    wire_id,
                    params,
                    reason,
                    (kind, rule),
                    response_deadline,
                )
                .await;
        }
        let decline =
            ReviewDecline::new(declared_tool_name(params), reason, RefusalOutcome::Continue);
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
                .await
                .map_err(Into::into);
        };
        let decline_id = self.allocate_decline_id()?;
        let selected =
            execution.decline_review(&execution_id, decline_id.clone(), decline.clone(), None)?;
        let mut publication =
            self.begin_selected_notice(execution_id.clone(), decline_id.clone(), decline.clone());
        tracing::warn!(
            session_id = %session_id.as_str(),
            execution_id = %execution_id.as_str(),
            tool = decline.declared().unwrap_or("<unnamed>"),
            reason = ?decline.reason(),
            "tool review declined; the agent is told no and the execution continues"
        );
        let correlation = self.next_correlation();
        // The decision is evidence before the wire sees it. An unrecordable
        // decision still has to reach the agent, so the refusal is sent either
        // way and the audit failure is reported after it.
        let decided = self
            .record_audit(
                ExecutionAuditRecord::ReviewDeclined(selected.clone()),
                AuditEffectPhase::PermissionSelection,
            )
            .await;
        let response = match rejection_option(
            params,
            &self.config.permissions,
            &self.profile.deadline_deny_preference(),
        ) {
            Some(option) => permission_wire::selected(&wire_id, &option),
            None => permission_wire::permission_cancel(&wire_id),
        };

        // Answered, and deliberately not registered: a refusal has no pending
        // review to register. The dispatcher is told so it does not answer the
        // same request a second time when this returns an audit failure.
        self.declined = Some(wire_id);
        publication.begin_write();
        let delivery = self.send_before(response, response_deadline).await;
        let delivery_failure = delivery.as_ref().err().map(|error| {
            WorkerFailure::new(
                error.clone(),
                self.retain_operation(
                    OperationEffectPhase::PermissionDelivery(correlation),
                    error.clone(),
                ),
            )
        });
        let observed = match &delivery {
            Ok(()) => PermissionAnswerDelivery::Written,
            Err(error) => PermissionAnswerDelivery::Failed(error.clone()),
        };
        let notice_delivery = if delivery.is_ok() {
            ReviewDeclineStage::WriteConfirmed
        } else {
            ReviewDeclineStage::WriteUnconfirmed
        };
        let final_notice = publication.settle(notice_delivery);
        // The caller-visible fact follows the observed write immediately. A
        // slow or cancelled final audit must not relabel a confirmed write as
        // unconfirmed through the publication guard's drop fallback.
        let written = self
            .record_audit(
                ExecutionAuditRecord::ReviewDeclined(selected.with_delivery(observed)),
                AuditEffectPhase::PermissionDelivery(correlation),
            )
            .await;
        // Three ways this can fail and three different things a reader needs to
        // know, so a transport cause is never replaced by a generic audit one.
        // An answered review keeps them apart the same way.
        let audit_failure = merge_failure(decided, written).err();
        let mut failure = match (delivery_failure, audit_failure) {
            (Some(delivery), Some(audit)) => {
                let delivery_error = delivery.error().clone();
                Some(delivery.combine(audit).with_error(
                    AgentError::PermissionAnswerDeliveryAndAuditFailure {
                        delivery_error: Box::new(delivery_error),
                        cleanup_error: None,
                    },
                ))
            }
            (Some(failure), None) | (None, Some(failure)) => Some(failure),
            (None, None) => None,
        };
        // A successful final publication necessarily retried and published the
        // selection first, so an earlier full queue has recovered.
        let notice = event_publication_result(&mut self.cancellation_cause, final_notice);
        if let Err(error) = notice {
            let notice = self.record_failure(OperationEffectPhase::EventDelivery, error);
            failure = Some(match failure {
                Some(previous) => {
                    let combined = combine_decline_result(
                        Err(previous.error().clone()),
                        Err(notice.error().clone()),
                    )
                    .expect_err("two failed effects produce a failure");
                    previous.combine(notice).with_error(combined)
                }
                None => notice,
            });
        }
        failure.map_or(Ok(()), Err)
    }
    /// Stop the execution for a refusal, then tell the provider.
    ///
    /// The notice and the cancelled finish happen before any await. The
    /// execution reply stays parked until the provider answers that prompt or
    /// the shutdown grace expires, so the session is not reported usable
    /// before then. Audit failure after the stop fails the operation. Telling
    /// the provider does not: that notice is bounded and must not delay the
    /// cleanup that already ran.
    async fn stop_declined_turn(
        &mut self,
        execution: &mut ExecutionController,
        wire_id: RpcId,
        params: &Value,
        reason: ReviewDeclineReason,
        stop: (StopKind, ApprovalRuleReference),
        response_deadline: Option<ClockInstant>,
    ) -> Result<(), WorkerFailure> {
        let (kind, rule) = stop;
        let decline = ReviewDecline::new(declared_tool_name(params), reason, refusal_outcome(kind));
        let session_id = execution.id().clone();
        let Some(active) = self.active.as_ref() else {
            return self
                .send_before(
                    permission_wire::permission_cancel(&wire_id),
                    response_deadline,
                )
                .await
                .map_err(Into::into);
        };
        let execution_id = active.execution_id.clone();
        let prompt_id = active.id;
        let decline_id = self.allocate_decline_id()?;
        let selected = execution.decline_review(
            &execution_id,
            decline_id.clone(),
            decline.clone(),
            Some(rule),
        )?;
        let mut publication =
            self.begin_selected_notice(execution_id.clone(), decline_id.clone(), decline.clone());
        publication.keep_selected_notice();
        let event = execution.finished_event(&execution_id, ExecutionOutcome::Cancelled)?;
        let records = execution.finish_execution(&execution_id, Ok(ExecutionOutcome::Cancelled))?;
        let emitted = self.emit(event);
        let active = self.active.take().expect("validated active prompt");
        self.stopped_prompts.push(StoppedPrompt {
            prompt_id,
            execution_id: execution_id.clone(),
            session_id: session_id.clone(),
            reply: active.reply,
            kind,
        });
        if kind == StopKind::CloseSession {
            self.stop_cleanup = StopCleanup::Policy;
        }
        self.declined = Some(wire_id.clone());
        self.discard_unclaimed_arguments();
        tracing::warn!(
            session_id = %session_id.as_str(),
            execution_id = %execution_id.as_str(),
            tool = decline.declared().unwrap_or("<unnamed>"),
            reason = ?decline.reason(),
            ?kind,
            "tool review declined and the execution stopped"
        );
        let cleaned = self.finish_turn_wire().await;
        if let Ok(deadline) = cleaned {
            if self.stop_reply_deadline.is_none() {
                self.stop_reply_deadline = Some(deadline);
            }
        }
        let response = match rejection_option(
            params,
            &self.config.permissions,
            &self.profile.deadline_deny_preference(),
        ) {
            Some(option) => permission_wire::selected(&wire_id, &option),
            None => permission_wire::permission_cancel(&wire_id),
        };
        let delivery = self.send_before(response, response_deadline).await;
        let cancel = json_rpc::encode(
            json_rpc::notification("session/cancel", json!({"sessionId": session_id.as_str()})),
            self.config.max_frame_bytes,
        );
        let notified = match cancel {
            Ok(frame) => self.send_encoded(frame, response_deadline).await,
            Err(error) => Err(error),
        };
        if let Err(error) = &notified {
            tracing::warn!(%error, "provider stop notification failed after the local stop");
        }
        if let Err(error) = &delivery {
            tracing::warn!(%error, "provider refusal delivery failed after the local stop");
        }
        let observed = match &delivery {
            Ok(()) => PermissionAnswerDelivery::Written,
            Err(error) => PermissionAnswerDelivery::Failed(error.clone()),
        };
        let audited = self.record_decline_audits(selected, observed).await;
        let lifecycle = self.record_lifecycle(records).await;
        let recorded = merge_failure(audited, lifecycle);
        let recorded = match emitted {
            Ok(()) => recorded,
            Err(error) => {
                let notice = self.record_failure(OperationEffectPhase::EventDelivery, error);
                merge_failure(recorded, Err(notice))
            }
        };
        merge_failure(cleaned.map(|_| ()), recorded)
    }
    fn allocate_decline_id(&mut self) -> Result<ReviewDeclineId, AgentError> {
        #[allow(deprecated, reason = "Rust 1.89 MSRV; try_update requires Rust 1.95")]
        let sequence = self
            .permission_sequence
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .map_err(|_| json_rpc::protocol("review identity exhausted"))?
            + 1;
        ReviewDeclineId::new(sequence.to_string())
            .map_err(|error| json_rpc::protocol(&error.to_string()))
    }
    fn begin_selected_notice(
        &self,
        execution_id: ExecutionId,
        decline_id: ReviewDeclineId,
        decline: ReviewDecline,
    ) -> DeclineNoticePublication {
        let mut publication = DeclineNoticePublication::new(
            self.events.clone(),
            execution_id,
            ReviewDeclineObservation::selected(decline_id, decline),
        );
        if let Err(error) = publication.publish_selected() {
            tracing::error!(?error, "declined-review selection notice was not published");
        }
        publication
    }
    /// Selection and delivery audits for one decline, both attempted.
    async fn record_decline_audits(
        &mut self,
        selected: ReviewDeclineRecord,
        observed: PermissionAnswerDelivery,
    ) -> Result<(), WorkerFailure> {
        let decided = self
            .record_audit(
                ExecutionAuditRecord::ReviewDeclined(selected.clone()),
                AuditEffectPhase::PermissionSelection,
            )
            .await;
        let correlation = self.next_correlation();
        let written = self
            .record_audit(
                ExecutionAuditRecord::ReviewDeclined(selected.with_delivery(observed)),
                AuditEffectPhase::PermissionDelivery(correlation),
            )
            .await;
        merge_failure(decided, written)
    }
    /// Cancel outstanding wire asks and close questions, under the shutdown grace.
    ///
    /// The same cleanup a turn uses when the provider finishes it. On success
    /// the returned instant is that grace, and the previous shutdown deadline
    /// is restored.
    ///
    /// Keep one absolute grace through cancellation and fallback teardown.
    /// Successful normal completion leaves the reusable session without it.
    async fn finish_turn_wire(&mut self) -> Result<ClockInstant, WorkerFailure> {
        let previous_deadline = self.shutdown_deadline;
        let deadline = self.begin_shutdown_grace();
        let clock = self.config.clock.clone();
        let sent = within(&*clock, deadline, self.cancel_wire_requests()).await;
        let closed = self
            .close_ended_questions(QuestionCancellation::ExecutionFinished)
            .await;
        match sent {
            Some(result) => {
                let result = result.and(closed).map_err(WorkerFailure::from);
                if result.is_ok() {
                    self.shutdown_deadline = previous_deadline;
                }
                result.map(|()| deadline)
            }
            None => {
                self.failure_cause = ObservationFailureCause::DeadlineExceeded;
                self.cancellation_cause.get_or_insert((
                    PermissionCancellationReason::deadline_exceeded(),
                    CancellationOrigin::Runtime,
                ));
                Err(AgentError::Deadline.into())
            }
        }
    }
    fn stop_reply_due(&self) -> bool {
        self.stop_reply_deadline.is_some_and(|deadline| {
            !self.stopped_prompts.is_empty() && self.config.clock.now() >= deadline
        })
    }
    /// The provider did not answer a stopped prompt within the grace.
    ///
    /// Each parked execution is reported cleanup-required. No provider result
    /// is invented for the reply that never arrived. An end-turn stop missed
    /// its grace, so that report is a deadline. A configured session close
    /// stays a policy refusal.
    fn expire_stopped_prompts(&mut self) {
        self.stop_reply_deadline = None;
        for stopped in std::mem::take(&mut self.stopped_prompts) {
            let report = match stopped.kind {
                StopKind::CloseSession => {
                    self.stop_cleanup = StopCleanup::Policy;
                    ExecutionReport::stopped_by_policy(ProviderSessionState::CleanupRequired)
                }
                StopKind::EndTurn => {
                    if self.stop_cleanup != StopCleanup::Policy {
                        self.stop_cleanup = StopCleanup::Deadline;
                    }
                    ExecutionReport::new(
                        None,
                        Some(AgentError::Deadline),
                        ProviderSessionState::CleanupRequired,
                    )
                }
            };
            self.expired_stops.push(ExpiredStop {
                prompt_id: stopped.prompt_id,
                execution_id: stopped.execution_id.clone(),
                session_id: stopped.session_id.clone(),
            });
            let _ = stopped.reply.send(ProviderExecutionReply::Finished(report));
        }
    }
    /// The report for a prompt refused because this generation cannot continue.
    fn cleanup_refusal(&self) -> ExecutionReport {
        match self.stop_cleanup {
            StopCleanup::Deadline => ExecutionReport::new(
                None,
                Some(AgentError::Deadline),
                ProviderSessionState::CleanupRequired,
            ),
            StopCleanup::Policy | StopCleanup::None => {
                ExecutionReport::stopped_by_policy(ProviderSessionState::CleanupRequired)
            }
        }
    }
    /// Release prompts that were accepted while a stop was unresolved.
    ///
    /// One is dispatched when the session can take another turn. Cleanup
    /// rejects the rest. Nothing is released while a stopped prompt is still
    /// waiting on its provider.
    async fn flush_held_prompts(
        &mut self,
        execution: &mut ExecutionController,
    ) -> Result<(), WorkerFailure> {
        if !self.stopped_prompts.is_empty() {
            return Ok(());
        }
        if self.stop_cleanup != StopCleanup::None {
            let report = self.cleanup_refusal();
            for (_, reply) in std::mem::take(&mut self.held_prompts) {
                let _ = reply.send(ProviderExecutionReply::Finished(report.clone()));
            }
            return Ok(());
        }
        // An update for the turn that was just released must not start the next
        // one, and must not answer that next one with Busy.
        if self.active.is_some() || self.steering.is_some() {
            return Ok(());
        }
        if let Some((prompt, reply)) = self.held_prompts.pop_front() {
            self.dispatch_execution(execution, prompt, reply).await?;
        }
        Ok(())
    }
    /// Record the provider reply for a prompt this worker already stopped.
    ///
    /// The reply is read before any session report. A result or a provider
    /// error keeps the local stop's report, sent before the late-result audit
    /// so a stalled audit cannot relabel the session. Anything else is a
    /// protocol failure. An end-turn stop answers the stopped execution and
    /// anything held behind it with that failure, so the session closes as an
    /// execution failure. A close-session stop stays a policy refusal. The
    /// error fails the worker the way a live prompt's malformed reply does.
    async fn complete_stopped_prompt(
        &mut self,
        stopped: StoppedPrompt,
        message: &Envelope,
    ) -> Result<(), WorkerFailure> {
        let reply = match prompt_reply(message) {
            Ok(reply) => reply,
            Err(error) => {
                self.fail_stopped_reply(stopped, error.clone());
                return Err(error.into());
            }
        };
        if stopped.kind == StopKind::CloseSession {
            self.stop_cleanup = StopCleanup::Policy;
        }
        let session_state = if self.stop_cleanup != StopCleanup::None {
            ProviderSessionState::CleanupRequired
        } else {
            ProviderSessionState::Usable
        };
        let _ = stopped.reply.send(ProviderExecutionReply::Finished(
            ExecutionReport::stopped_by_policy(session_state),
        ));
        if self.stopped_prompts.is_empty() {
            self.stop_reply_deadline = None;
        }
        self.record_parsed_late_reply(stopped.session_id, stopped.execution_id, reply)
            .await
    }
    /// Refuse a stopped prompt whose reply is not a terminal provider result.
    ///
    /// An end-turn stop answers this prompt, and every prompt held behind it,
    /// with the protocol error and cleanup required. That report is a provider
    /// failure, so the agent closes the session as an execution failure. A
    /// close-session stop stays a policy refusal. Prompts held behind that
    /// stop are answered only once nothing stopped is still waiting.
    fn fail_stopped_reply(&mut self, stopped: StoppedPrompt, error: AgentError) {
        let closes_session = stopped.kind == StopKind::CloseSession;
        let report = if closes_session {
            if self.stop_cleanup == StopCleanup::None {
                self.stop_cleanup = StopCleanup::Policy;
            }
            ExecutionReport::stopped_by_policy(ProviderSessionState::CleanupRequired)
        } else {
            ExecutionReport::new(None, Some(error), ProviderSessionState::CleanupRequired)
        };
        let _ = stopped
            .reply
            .send(ProviderExecutionReply::Finished(report.clone()));
        if !closes_session {
            for (_, reply) in std::mem::take(&mut self.held_prompts) {
                let _ = reply.send(ProviderExecutionReply::Finished(report.clone()));
            }
        }
        if self.stopped_prompts.is_empty() {
            self.stop_reply_deadline = None;
            if closes_session {
                let report = self.cleanup_refusal();
                for (_, reply) in std::mem::take(&mut self.held_prompts) {
                    let _ = reply.send(ProviderExecutionReply::Finished(report.clone()));
                }
            }
        }
    }
    /// Answer held prompts whose own execution bound has passed.
    ///
    /// The report is the one a dispatch uses when its deadline passes. The
    /// prompt is not sent. Prompts that are still inside their bound stay held.
    fn refuse_expired_held_prompts(&mut self) {
        let now = self.config.clock.now();
        let mut kept = VecDeque::new();
        for (prompt, reply) in std::mem::take(&mut self.held_prompts) {
            if prompt.deadline.is_some_and(|deadline| now >= deadline) {
                let _ = reply.send(ProviderExecutionReply::Finished(ExecutionReport::new(
                    None,
                    Some(AgentError::Deadline),
                    ProviderSessionState::CleanupRequired,
                )));
            } else {
                kept.push_back((prompt, reply));
            }
        }
        self.held_prompts = kept;
    }
    async fn record_late_reply(
        &mut self,
        session_id: ExecutionSessionId,
        execution_id: ExecutionId,
        message: &Envelope,
    ) -> Result<(), WorkerFailure> {
        let reply = prompt_reply(message)?;
        self.record_parsed_late_reply(session_id, execution_id, reply)
            .await
    }
    async fn record_parsed_late_reply(
        &mut self,
        session_id: ExecutionSessionId,
        execution_id: ExecutionId,
        reply: PromptReply,
    ) -> Result<(), WorkerFailure> {
        let result = match reply {
            PromptReply::Outcome(outcome) => Ok(outcome),
            PromptReply::Failed(error) => Err(error),
        };
        self.record_audit(
            ExecutionAuditRecord::LateProviderResult(LateProviderResultRecord::new(
                session_id,
                execution_id,
                result,
            )),
            AuditEffectPhase::LateProviderResult,
        )
        .await
    }
    fn take_unanswered_replies(&mut self) -> Vec<ExecutionReply> {
        let mut replies = Vec::new();
        replies.extend(
            std::mem::take(&mut self.stopped_prompts)
                .into_iter()
                .map(|stopped| stopped.reply),
        );
        replies.extend(
            std::mem::take(&mut self.held_prompts)
                .into_iter()
                .map(|(_, reply)| reply),
        );
        replies
    }
    fn next_correlation(&mut self) -> EffectCorrelation {
        let correlation = EffectCorrelation(self.correlation_sequence);
        self.correlation_sequence = self
            .correlation_sequence
            .checked_add(1)
            .expect("one worker cannot exhaust effect correlations");
        correlation
    }
    fn retain_operation(
        &mut self,
        phase: OperationEffectPhase,
        error: AgentError,
    ) -> Option<FactId> {
        self.settlement_facts.record_operation(phase, error)
    }
    fn record_failure(&mut self, phase: OperationEffectPhase, error: AgentError) -> WorkerFailure {
        WorkerFailure::new(
            error.clone(),
            self.settlement_facts.record_operation(phase, error),
        )
    }
    fn ensure_failure_covered(
        &mut self,
        phase: OperationEffectPhase,
        failure: WorkerFailure,
    ) -> WorkerFailure {
        failure.validate(&self.settlement_facts);
        if failure.coverage().is_empty() {
            self.record_failure(phase, failure.into_error())
        } else {
            failure
        }
    }
    /// The next identity in this binding's question sequence.
    ///
    /// Asks that are admitted and asks that are refused draw from the one
    /// sequence, so no two requests an audit names can share an identity.
    fn mint_question_id(&self) -> Result<QuestionId, AgentError> {
        #[allow(deprecated, reason = "Rust 1.89 MSRV; try_update requires Rust 1.95")]
        let sequence = self
            .question_sequence
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .map_err(|_| json_rpc::protocol("question ID exhausted"))?
            + 1;
        QuestionId::new(sequence.to_string())
            .map_err(|error| json_rpc::protocol(&error.to_string()))
    }
    /// Refuse one ask before anybody is asked, and leave the evidence of why.
    ///
    /// Mirrors a refused review: the decision is recorded before the wire sees
    /// it, the refusal is sent whether or not that record could be made — an
    /// agent left waiting is worse than an unrecorded refusal, which is
    /// reported — and the write is recorded after. The dispatcher is told the
    /// request is answered, so an audit failure returned from here is not
    /// answered a second time.
    async fn refuse_question(
        &mut self,
        execution: &ExecutionController,
        wire_id: RpcId,
        reason: QuestionRefusalReason,
        refused: Option<RefusedAsk>,
        response_deadline: Option<ClockInstant>,
    ) -> Result<(), WorkerFailure> {
        let session_id = execution.id().clone();
        let execution_id = self
            .active
            .as_ref()
            .expect("a refusal is correlated with the running execution")
            .execution_id
            .clone();
        tracing::warn!(
            session_id = %session_id.as_str(),
            execution_id = %execution_id.as_str(),
            ?reason,
            "agent question refused; the agent is told and the execution continues"
        );
        // Its decision and its write are two records; this is what pairs them.
        let id = self.mint_question_id()?;
        let selected = match refused {
            Some(refused) => QuestionRefusalRecord::for_room(
                session_id.clone(),
                execution_id.clone(),
                id,
                refused,
                PermissionAnswerDelivery::Selected,
            ),
            None => QuestionRefusalRecord::new(
                session_id.clone(),
                execution_id.clone(),
                id,
                reason,
                PermissionAnswerDelivery::Selected,
            ),
        }
        .map_err(|error| json_rpc::protocol(&error.to_string()))?;
        let record = |delivery| {
            ExecutionAuditRecord::QuestionRefused(selected.clone().with_delivery(delivery))
        };
        let correlation = self.next_correlation();
        let decided = self
            .record_audit(
                record(PermissionAnswerDelivery::Selected),
                AuditEffectPhase::PermissionSelection,
            )
            .await;
        self.declined = Some(wire_id.clone());
        let delivery = self
            .send_before(question_wire::cancelled(&wire_id), response_deadline)
            .await;
        let observed = match &delivery {
            Ok(()) => PermissionAnswerDelivery::Written,
            Err(error) => PermissionAnswerDelivery::Failed(error.clone()),
        };
        let written = self
            .record_audit(
                record(observed),
                AuditEffectPhase::PermissionDelivery(correlation),
            )
            .await;
        let audit = match (decided, written) {
            (Ok(()), Ok(())) => None,
            (Err(first), Err(subsequent)) => Some(first.combine(subsequent)),
            (Err(failure), Ok(())) | (Ok(()), Err(failure)) => Some(failure),
        };
        self.delivery_and_audit(correlation, &delivery, audit)
    }
    /// One decision's outcome from its write and its audit, kept apart the way
    /// an answered or refused review keeps them: a transport cause is never
    /// replaced by a generic audit one, and a failed write is retained as the
    /// operation fact its audit pairs with. Questions share the review phases,
    /// because each is the same shape of effect — a local decision, then a
    /// write to the agent — and the settlement pairs them the same way.
    fn delivery_and_audit(
        &mut self,
        correlation: EffectCorrelation,
        delivery: &Result<(), AgentError>,
        audit: Option<WorkerFailure>,
    ) -> Result<(), WorkerFailure> {
        let delivery_failure = delivery.as_ref().err().map(|error| {
            WorkerFailure::new(
                error.clone(),
                self.retain_operation(
                    OperationEffectPhase::PermissionDelivery(correlation),
                    error.clone(),
                ),
            )
        });
        match (delivery_failure, audit) {
            (Some(delivery), Some(audit)) => {
                let delivery_error = delivery.error().clone();
                Err(delivery.combine(audit).with_error(
                    AgentError::PermissionAnswerDeliveryAndAuditFailure {
                        delivery_error: Box::new(delivery_error),
                        cleanup_error: None,
                    },
                ))
            }
            (Some(failure), None) | (None, Some(failure)) => Err(failure),
            (None, None) => Ok(()),
        }
    }
    /// Take one question the agent is asking, and show it to whoever can answer.
    ///
    /// The ask is held here until it is answered: the agent is waiting on this
    /// request, so the wire identity is kept beside what was asked, and the
    /// answer is checked against it rather than against whatever is open.
    async fn question(
        &mut self,
        execution: &mut ExecutionController,
        wire_id: RpcId,
        params: Value,
        response_deadline: Option<ClockInstant>,
    ) -> Result<(), WorkerFailure> {
        self.check_session(execution, &params)?;
        // With no execution there is nothing to correlate a refusal with, so
        // the agent is told and nothing is recorded — as a refused review is.
        // A question that arrives while a stopped prompt is still unanswered
        // belongs to that stopped turn.
        if self.active.is_none() || !self.stopped_prompts.is_empty() {
            return self
                .send_before(question_wire::cancelled(&wire_id), response_deadline)
                .await
                .map_err(Into::into);
        }
        // A second request under an identity that is still open is outside
        // JSON-RPC. Answering it would answer the first, which is still being
        // offered, so it is refused as a review's duplicate is: as a protocol
        // fault, whose teardown ends the first ask with its evidence.
        if self.questions.values().any(|open| open.wire_id == wire_id) {
            return Err(json_rpc::protocol("question reuses an open request's ID").into());
        }
        // Each refusal below is a decision with an effect — the agent abandons
        // the tool call that asked — so each is recorded, not only sent.
        // Nothing is read once the session is ending: nobody could answer.
        if self.closing {
            return self
                .refuse_question(
                    execution,
                    wire_id,
                    QuestionRefusalReason::SessionEnding,
                    None,
                    response_deadline,
                )
                .await;
        }
        let question = match question_wire::question(&params) {
            Ok(question) => question,
            // A question this binding cannot put to anybody is answered rather
            // than left hanging, and the execution goes on — the same rule a
            // refused review follows.
            Err(error) => {
                let reason = match error {
                    AgentError::Unsupported(_) => QuestionRefusalReason::Unsupported,
                    _ => QuestionRefusalReason::UnreadableQuestion,
                };
                return self
                    .refuse_question(execution, wire_id, reason, None, response_deadline)
                    .await;
            }
        };
        // Room is decided from what is open, and a refusal for room keeps both
        // sides of that comparison: the ask, and what was already there.
        let candidate = RefusedAsk::new(
            question,
            self.questions.values().map(|open| open.admitted.question()),
        )
        .map_err(|error| json_rpc::protocol(&error.to_string()))?;
        if let Some(reason) = candidate.reason() {
            return self
                .refuse_question(
                    execution,
                    wire_id,
                    reason,
                    Some(candidate),
                    response_deadline,
                )
                .await;
        }
        let question = candidate.into_ask();
        let id = self.mint_question_id()?;
        let execution_id = self
            .active
            .as_ref()
            .expect("validated active prompt")
            .execution_id
            .clone();
        let admitted = execution.ask_question(&execution_id, id, question)?;
        let event = admitted.event();
        let listing = execution.question_listing();
        self.questions.insert(
            admitted.id().clone(),
            OpenQuestion {
                wire_id,
                admitted,
                cancel_delivery: None,
            },
            listing,
        );
        self.emit(event).map_err(Into::into)
    }

    /// Answer one question, and tell the agent what it was answered with.
    ///
    /// A choice that the question did not offer is refused here rather than
    /// sent: the agent acts on what comes back, so what comes back is what it
    /// asked for. Evidence keeps the local decision apart from the write, as an
    /// answered review does.
    async fn answer_question(
        &mut self,
        answer: QuestionAnswer,
        reply: oneshot::Sender<ProviderOperationResult<()>>,
    ) -> Result<(), WorkerFailure> {
        let Some(OpenQuestion {
            wire_id, admitted, ..
        }) = self.questions.get(&answer.id).cloned()
        else {
            let _ = reply.send(Err(ProviderOperationFailure::new(
                AgentError::InvalidInput("no question is waiting for this answer".into()),
                ProviderSessionState::Usable,
            )));
            return Ok(());
        };
        if self
            .active
            .as_ref()
            .is_none_or(|active| active.execution_id != answer.execution_id)
        {
            let _ = reply.send(Err(ProviderOperationFailure::new(
                AgentError::InvalidInput(
                    "the answered question belongs to another execution".into(),
                ),
                ProviderSessionState::Usable,
            )));
            return Ok(());
        }
        // The answer is validated where it is recorded, against the ask it is
        // recorded beside, so an answer this refuses never becomes evidence and
        // one it accepts can only be an answer to that ask.
        let selected = match QuestionAnswerRecord::chosen(
            admitted.clone(),
            answer.choices,
            answer.actor.clone(),
            PermissionAnswerDelivery::Selected,
        ) {
            Ok(selected) => selected,
            Err(error) => {
                let _ = reply.send(Err(ProviderOperationFailure::new(
                    AgentError::InvalidInput(error.to_string()),
                    ProviderSessionState::Usable,
                )));
                return Ok(());
            }
        };
        let response = selected.response().clone();
        let record = |delivery| {
            ExecutionAuditRecord::QuestionAnswered(selected.clone().with_delivery(delivery))
        };
        // The decision is evidence before the wire sees it, and an unrecordable
        // decision is not sent: an answered review returns here rather than
        // acting unaudited, and an answered question is the same decision.
        let correlation = self.next_correlation();
        if let Err(error) = self
            .record_audit(
                record(PermissionAnswerDelivery::Selected),
                AuditEffectPhase::PermissionSelection,
            )
            .await
        {
            let _ = reply.send(Err(ProviderOperationFailure::new(
                error.error().clone(),
                ProviderSessionState::CleanupRequired,
            )));
            // Still open: nothing was sent, so the teardown this failure starts
            // is what tells the agent, closes the ask and records why. Taken
            // out before the audit, it was ended by nobody.
            return Err(error);
        }
        // Released only once the decision is on record, as a review's wire
        // identity is: from here on this path owns its ending.
        self.questions.remove(&answer.id);
        let written = match &response {
            QuestionResponse::Answered(accepted) => {
                question_wire::accepted(&wire_id, admitted.question(), accepted)
            }
            QuestionResponse::Declined => question_wire::declined(&wire_id),
            // A host answers or declines; cancelling is not something it can
            // submit. Written as what it means on the wire all the same, so the
            // match is total rather than trusting that it cannot happen.
            QuestionResponse::Cancelled(_) => question_wire::cancelled(&wire_id),
        };
        let delivery = self.send(written).await;
        // Closure is how a surface that did not answer learns the ask is over,
        // so a queue that cannot take it is a failure to report, not to ignore:
        // dropping it leaves an actionable ask whose later answers are refused.
        let closed = self.emit(admitted.closed_event());
        let observed = match &delivery {
            Ok(()) => PermissionAnswerDelivery::Written,
            Err(error) => PermissionAnswerDelivery::Failed(error.clone()),
        };
        let recorded = self
            .record_audit(
                record(observed),
                AuditEffectPhase::PermissionDelivery(correlation),
            )
            .await;
        let mut outcome = self.delivery_and_audit(correlation, &delivery, recorded.err());
        if let Err(error) = closed {
            let unobserved = self.record_failure(OperationEffectPhase::EventDelivery, error);
            outcome = Err(match outcome {
                Ok(()) => unobserved,
                Err(previous) => previous.combine(unobserved),
            });
        }
        let _ = reply.send(match &outcome {
            Ok(()) => Ok(()),
            Err(failure) => Err(ProviderOperationFailure::new(
                failure.error().clone(),
                ProviderSessionState::CleanupRequired,
            )),
        });
        outcome
    }

    async fn record_audit(
        &mut self,
        record: ExecutionAuditRecord,
        phase: AuditEffectPhase,
    ) -> Result<(), WorkerFailure> {
        let result = catch_worker_panic(async {
            // The trait call itself may panic before returning its future.
            let clock = &*self.config.clock;
            within(
                clock,
                clock.now() + self.config.shutdown_grace,
                self.audit.record(record),
            )
            .await
            .ok_or(AgentError::AuditFailure)?
        })
        .await;
        if result.is_err() {
            Err(WorkerFailure::new(
                AgentError::AuditFailure,
                self.settlement_facts.record_audit(phase),
            ))
        } else {
            Ok(())
        }
    }
    /// Drop the notice for this review. The conversation is the session key
    /// the notice was published under. Without one, nothing was published.
    fn clear_review(&self, execution: &ExecutionId, permission: &PermissionId) {
        let Some(conversation_id) = self.session_key.clone().filter(|id| !id.is_empty()) else {
            return;
        };
        self.config
            .review_notices
            .clear(&conversation_id, execution, permission);
    }
    async fn record_answer(
        &mut self,
        resolution: PermissionResolution,
        delivery: PermissionAnswerDelivery,
        correlation: EffectCorrelation,
    ) -> Result<(), WorkerFailure> {
        let execution_id = resolution.request().execution_id().clone();
        let permission_id = resolution.request().id().clone();
        let session_id = resolution.session_id().clone();
        // The review has been selected, including when the audit of that
        // selection fails afterwards. A notice left up would still be asking.
        if delivery == PermissionAnswerDelivery::Selected {
            self.clear_review(&execution_id, &permission_id);
        }
        let phase = if delivery == PermissionAnswerDelivery::Selected {
            AuditEffectPhase::PermissionSelection
        } else {
            AuditEffectPhase::PermissionDelivery(correlation)
        };
        let record = PermissionAnswerRecord::new(resolution, delivery);
        match self
            .record_audit(ExecutionAuditRecord::Answered(record), phase)
            .await
        {
            Ok(()) => Ok(()),
            Err(failure) => {
                tracing::error!(session_id = %session_id.as_str(), execution_id = %execution_id.as_str(), permission_id = %permission_id.as_str(), "permission answer audit delivery failed");
                Err(failure)
            }
        }
    }
    async fn record_lifecycle(
        &mut self,
        records: Vec<ExecutionAuditRecord>,
    ) -> Result<(), WorkerFailure> {
        let mut permissions = Vec::new();
        let mut failure: Option<WorkerFailure> = None;
        for record in records {
            match record {
                ExecutionAuditRecord::Cancelled(record) => permissions.push(record),
                record => {
                    if let Err(error) = self.record_audit(record, AuditEffectPhase::Lifecycle).await
                    {
                        tracing::error!("execution lifecycle audit delivery failed");
                        failure = Some(match failure {
                            Some(previous) => previous.combine(error),
                            None => error,
                        });
                    }
                }
            }
        }
        match (failure, self.record_cancellations(permissions).await) {
            (Some(first), Err(subsequent)) => Err(first.combine(subsequent)),
            (Some(error), Ok(())) | (None, Err(error)) => Err(error),
            (None, Ok(())) => Ok(()),
        }
    }
    async fn record_cancellations(
        &mut self,
        records: Vec<PermissionCancellation>,
    ) -> Result<(), WorkerFailure> {
        let mut failure: Option<WorkerFailure> = None;
        for record in &records {
            self.permission_context.remove(record.request().id());
            self.clear_review(record.request().execution_id(), record.request().id());
        }
        // Attempt every record before publishing UI updates. A full/dropped UI queue
        // must not prevent audit capture of later cancellations in this batch.
        // Each sink call gets its own bound; one timeout cannot consume the
        // delivery opportunity of a different durable record.
        for record in &records {
            if let Err(error) = self
                .record_audit(
                    ExecutionAuditRecord::Cancelled(record.clone()),
                    AuditEffectPhase::Cancellation,
                )
                .await
            {
                tracing::error!(
                    session_id = %record.session_id().as_str(),
                    execution_id = %record.request().execution_id().as_str(),
                    permission_id = %record.request().id().as_str(),
                    "permission cancellation audit delivery failed"
                );
                failure = Some(match failure {
                    Some(previous) => previous.combine(error),
                    None => error,
                });
            }
        }
        for record in records {
            if let Err(error) = self.emit(ExecutionEvent::new(
                record.request().execution_id().clone(),
                ExecutionUpdate::PermissionCancelled(record),
            )) {
                let operation = self.record_failure(OperationEffectPhase::EventDelivery, error);
                failure = Some(match failure {
                    Some(previous) => previous.combine(operation),
                    None => operation,
                });
            }
        }
        if let Some(error) = failure {
            Err(error)
        } else {
            Ok(())
        }
    }
    /// Tell the provider that nothing it is waiting on will be answered.
    ///
    /// Only the wire is touched here, because only this runs under the grace
    /// deadline, and a deadline drops this future wherever it has got to. Each
    /// ask's result is kept on the ask as it is sent, so what reached the
    /// provider outlives the future that sent it. Asks go first: a failed
    /// review cancellation returns early, and must not decide whether an ask
    /// is told. The first failure to tell the provider anything is returned.
    async fn cancel_wire_requests(&mut self) -> Result<(), AgentError> {
        // The turn these reviews belonged to is over. A deadline that fired
        // afterwards would answer a request the provider is no longer waiting on.
        // Clear the waiting notice before the context that names it is dropped,
        // so a cancelled review does not leave the prompt up
        // (`cancelling_the_wires_clears_the_waiting_notice`).
        let waiting: Vec<(ExecutionId, PermissionId)> = self
            .permission_context
            .iter()
            .map(|(id, context)| (context.execution.clone(), id.clone()))
            .collect();
        for (execution, id) in &waiting {
            self.clear_review(execution, id);
        }
        self.armed_permissions.clear();
        self.permission_context.clear();
        let asked: Vec<QuestionId> = self.questions.keys().cloned().collect();
        let mut failure = None;
        for id in asked {
            let Some(wire_id) = self.questions.get(&id).map(|open| open.wire_id.clone()) else {
                continue;
            };
            let delivery = self.send(question_wire::cancelled(&wire_id)).await;
            if let Err(error) = &delivery {
                failure.get_or_insert(error.clone());
            }
            if let Some(open) = self.questions.get_mut(&id) {
                open.cancel_delivery = Some(delivery);
            }
        }
        let permissions = std::mem::take(&mut self.permissions);
        for wire_id in permissions.into_values() {
            self.send(permission_wire::permission_cancel(&wire_id))
                .await?;
        }
        failure.map_or(Ok(()), Err)
    }

    /// Close every open ask for `cause`, and record that it ended.
    ///
    /// A question nobody will answer is ended for the same reason a pending
    /// review is: the agent is waiting on it. Run after the deadline-bound
    /// sends, never under them, so a deadline cannot drop an ask's evidence
    /// with it. An ask whose cancellation was never sent — the deadline came
    /// first — is recorded as not delivered, because it was not.
    async fn close_ended_questions(
        &mut self,
        cause: QuestionCancellation,
    ) -> Result<(), AgentError> {
        let questions = self.questions.take();
        let mut failure = None;
        for mut open in questions.into_values() {
            let delivery = open
                .cancel_delivery
                .take()
                .unwrap_or(Err(AgentError::Deadline));
            if let Err(error) = self.close_question(open, cause, delivery).await {
                failure.get_or_insert(error);
            }
        }
        failure.map_or(Ok(()), Err)
    }

    /// Close one ask that nobody answered, and leave the evidence of why.
    ///
    /// `delivery` is what telling the provider came to, and is recorded, not
    /// returned: whoever sent it reports it, since only the sender knows whether
    /// a deadline there was a failure or the end of a grace. Both steps here are
    /// attempted whatever the other did: the close is observed, so a surface
    /// still showing the ask stops offering it; and the cause is recorded,
    /// because an ask ending is a transition like any other and a teardown is
    /// not an exception to that. The first local failure is returned.
    async fn close_question(
        &mut self,
        open: OpenQuestion,
        cause: QuestionCancellation,
        delivery: Result<(), AgentError>,
    ) -> Result<(), AgentError> {
        let closed = open.admitted.closed_event();
        let observed = self.emit(closed);
        let recorded = self
            .record_audit(
                ExecutionAuditRecord::QuestionAnswered(QuestionAnswerRecord::ended(
                    open.admitted,
                    cause,
                    match &delivery {
                        Ok(()) => PermissionAnswerDelivery::Written,
                        Err(error) => PermissionAnswerDelivery::Failed(error.clone()),
                    },
                )),
                AuditEffectPhase::Cancellation,
            )
            .await
            // The failure is already retained as a settlement fact; what is
            // returned here only tells the teardown that something failed.
            .map_err(WorkerFailure::into_error);
        observed.and(recorded)
    }
}

/// One ask this worker is holding open for an answer.
///
/// Named rather than a tuple because each part answers a different question:
/// which provider request the agent is waiting on, which execution asked — so
/// an answer or a cancellation can name it after that execution is no longer
/// active — and what was asked, which an answer is checked against.
/// The asks this worker holds open, and the listing each is shown through:
/// one owner, so a question leaves the listing exactly when it leaves here.
#[derive(Default)]
struct AskedQuestions {
    open: HashMap<QuestionId, OpenQuestion>,
    listings: HashMap<QuestionId, QuestionListing>,
}
impl AskedQuestions {
    fn insert(&mut self, id: QuestionId, open: OpenQuestion, listing: Option<QuestionListing>) {
        if let Some(listing) = listing {
            self.listings.insert(id.clone(), listing);
        }
        self.open.insert(id, open);
    }
    fn remove(&mut self, id: &QuestionId) -> Option<OpenQuestion> {
        if let Some(listing) = self.listings.remove(id) {
            listing.unlist(id);
        }
        self.open.remove(id)
    }
    fn take(&mut self) -> HashMap<QuestionId, OpenQuestion> {
        for (id, listing) in self.listings.drain() {
            listing.unlist(&id);
        }
        std::mem::take(&mut self.open)
    }
    fn get(&self, id: &QuestionId) -> Option<&OpenQuestion> {
        self.open.get(id)
    }
    #[cfg(test)]
    fn len(&self) -> usize {
        self.open.len()
    }
    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.open.is_empty()
    }
    fn get_mut(&mut self, id: &QuestionId) -> Option<&mut OpenQuestion> {
        self.open.get_mut(id)
    }
    fn values(&self) -> std::collections::hash_map::Values<'_, QuestionId, OpenQuestion> {
        self.open.values()
    }
    fn keys(&self) -> std::collections::hash_map::Keys<'_, QuestionId, OpenQuestion> {
        self.open.keys()
    }
    fn iter(&self) -> std::collections::hash_map::Iter<'_, QuestionId, OpenQuestion> {
        self.open.iter()
    }
}

#[derive(Clone)]
struct OpenQuestion {
    wire_id: RpcId,
    admitted: AdmittedQuestion,
    /// What telling the provider this ask is over came to, once a teardown
    /// has tried. Kept here because that attempt runs under a deadline that
    /// can drop it, and the record made afterwards must say what happened.
    cancel_delivery: Option<Result<(), AgentError>>,
}

/// Which of the two things a profile's refusal was saying.
///
/// `Unsupported` is "this binding does not review that tool" — including a
/// profile that will not review it because the name is not usable as an
/// identity, which is the same refusal reached one step earlier. Everything
/// else
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

/// A rejection the provider offered, if it offered one this host allows.
///
/// Read from the frame rather than through the offered options, because this is
/// the path taken when those could not be built. Two limits still apply. Only
/// `reject_once` qualifies: a persistent refusal would answer for reviews this
/// binding has not seen. And the configured policy must permit refusing a
/// single request at all — a host that never offers "deny" has not authorized
/// this binding to answer one on its behalf, and such a review is cancelled
/// instead. The identity is validated as a domain option identity, not by a
/// second rule invented here.
fn rejection_option(
    params: &Value,
    policy: &PermissionOfferPolicy,
    deny_preference: &crate::domain::agent_execution::permissions::DeadlineDenyPreference,
) -> Option<String> {
    let deny = PermissionDecision::new(PermissionEffect::Deny, PermissionScope::request());
    if !policy.decisions().contains(&deny) {
        return None;
    }
    let ids: Vec<PermissionOptionId> = params
        .get("options")
        .and_then(Value::as_array)?
        .iter()
        .filter(|option| option.get("kind").and_then(Value::as_str) == Some("reject_once"))
        .filter_map(|option| option.get("optionId").and_then(Value::as_str))
        .filter_map(|id| PermissionOptionId::new(id).ok())
        .collect();
    deny_preference
        .select(ids.iter())
        .map(|(id, _)| id.as_str().to_owned())
}

/// Unix milliseconds when a review armed now, waiting `wait`, comes due.
///
/// `clock` is the binding's clock. The countdown a card shows and the moment
/// this worker decides are the same timeline.
fn unix_deadline_ms(clock: &dyn Clock, armed_at: ClockInstant, wait: std::time::Duration) -> u64 {
    let wait_ms = crate::domain::agent_execution::permissions::wait_ms(wait);
    clock.unix_ms_at(armed_at).saturating_add(wait_ms)
}

impl<P: AcpProfile> Worker<P> {
    /// An answer or a cancellation of a review whose deadline has already
    /// passed. That command was admitted before this wait, so it decides the
    /// review. Any other command does not.
    fn command_answers_due_review(&self, command: &Command, now: ClockInstant) -> bool {
        let id = match command {
            Command::Answer(answer, _) => &answer.id,
            Command::CancelPermission(request, _) => &request.id,
            _ => return false,
        };
        self.armed_permissions
            .get(id)
            .is_some_and(|armed| now >= armed.at)
    }

    fn next_permission_deadline(&self) -> Option<ClockInstant> {
        self.armed_permissions
            .iter()
            .filter(|(id, _)| {
                self.permission_context
                    .get(*id)
                    .is_none_or(|context| !context.awaiting_consult)
            })
            .map(|(_, armed)| armed.at)
            .min()
    }

    /// The permission-review hooks allowed the tool and no remembered rule
    /// answered it. The deadline may run, and the person is told the review
    /// is waiting. A review that was not held, or that is already gone, is
    /// left as it is.
    fn release_consult(&mut self, id: &PermissionId) -> bool {
        let Some(context) = self.permission_context.get_mut(id) else {
            return false;
        };
        if !context.awaiting_consult {
            return false;
        }
        context.awaiting_consult = false;
        let execution_id = context.execution.clone();
        let tool = context.tool.clone();
        // The wait starts when the review can be shown. Time spent behind the
        // hook is not part of the person's window, and a hook that outlasts
        // the original instant does not settle the review as it is released
        // (`a_held_review_keeps_its_full_window_after_the_hook`).
        let now = self.config.clock.now();
        if let Some(armed) = self.armed_permissions.get_mut(id) {
            armed.at = now + armed.settlement.deadline().wait();
        }
        let due = self
            .armed_permissions
            .get(id)
            .is_some_and(|armed| now >= armed.at);
        if !due {
            if let Some(conversation_id) = self.session_key.clone().filter(|id| !id.is_empty()) {
                self.config.review_notices.waiting(&ReviewNotice {
                    conversation_id,
                    execution_id,
                    permission_id: id.clone(),
                    tool,
                });
            }
        }
        due
    }

    /// Answer every review whose deadline had passed at `now` and which nobody
    /// else has answered. An audit failure is retained and does not stop the
    /// turn. A failure to tell the provider, or to publish the notice, does.
    async fn settle_due_permissions(
        &mut self,
        execution: &mut ExecutionController,
        now: ClockInstant,
    ) -> Result<(), WorkerFailure> {
        let mut due: Vec<(ClockInstant, PermissionId)> = self
            .armed_permissions
            .iter()
            .filter(|(id, armed)| {
                now >= armed.at
                    && self
                        .permission_context
                        .get(*id)
                        .is_none_or(|context| !context.awaiting_consult)
            })
            .map(|(id, armed)| (armed.at, id.clone()))
            .collect();
        due.sort_by(|(left_at, left_id), (right_at, right_id)| {
            left_at
                .cmp(right_at)
                .then_with(|| left_id.as_str().cmp(right_id.as_str()))
        });
        for (_, id) in due {
            let Some(armed) = self.armed_permissions.remove(&id) else {
                continue;
            };
            self.settle_permission(execution, id, armed).await?;
        }
        Ok(())
    }

    async fn settle_permission(
        &mut self,
        execution: &mut ExecutionController,
        id: PermissionId,
        armed: ArmedPermission,
    ) -> Result<(), WorkerFailure> {
        let Some(execution_id) = self
            .active
            .as_ref()
            .map(|active| active.execution_id.clone())
        else {
            if let Some(wire_id) = self.permissions.remove(&id) {
                self.permission_context.remove(&id);
                let _ = self
                    .send(permission_wire::permission_cancel(&wire_id))
                    .await;
            }
            return Ok(());
        };
        let deadline = armed.settlement.deadline().clone();
        let stops_turn = deadline.stops_turn();
        let reason = deadline.recorded_reason();
        let mut delivery_correlation = None;
        let sent = if let Some(option_id) = armed.settlement.option().cloned() {
            let resolution = match execution.settle_deadline(
                &execution_id,
                &id,
                &option_id,
                deadline,
            ) {
                Ok(resolution) => resolution,
                Err(error) => {
                    tracing::warn!(%error, permission_id = %id.as_str(), "permission deadline found the review already resolved");
                    // A review resolved without its deadline being disarmed is
                    // a bug elsewhere; nothing was decided, so nothing is published.
                    return Ok(());
                }
            };
            let correlation = self.next_correlation();
            delivery_correlation = Some(correlation);
            if let Err(failure) = self
                .record_answer(
                    resolution.clone(),
                    PermissionAnswerDelivery::Selected,
                    correlation,
                )
                .await
            {
                // Retained on the settlement facts. An audit failure must not
                // stop the turn: the provider is still answered below.
                let _ = failure;
            }
            let wire_id = self
                .permissions
                .remove(&id)
                .expect("pending wire permission");
            self.permission_context.remove(&id);
            let sent = self
                .send(permission_wire::selected_because(
                    &wire_id,
                    option_id.as_str(),
                    reason.as_str(),
                ))
                .await;
            let delivery = match &sent {
                Ok(()) => PermissionAnswerDelivery::Written,
                Err(error) => PermissionAnswerDelivery::Failed(error.clone()),
            };
            if let Err(failure) = self.record_answer(resolution, delivery, correlation).await {
                // Same as the selection audit: kept, and not a reason to skip
                // the notice or to fail the turn when the write itself succeeded.
                let _ = failure;
            }
            sent
        } else {
            if let Some(record) = execution
                .cancel_deadline(&execution_id, &id, deadline)
                .map_err(|error| self.record_failure(OperationEffectPhase::Worker, error))?
            {
                // The review is settled, including when its audit fails later.
                self.permission_context.remove(&id);
                self.clear_review(&execution_id, &id);
                if let Err(failure) = self
                    .record_audit(
                        ExecutionAuditRecord::Cancelled(record.clone()),
                        AuditEffectPhase::Cancellation,
                    )
                    .await
                {
                    // The cancellation is already the local fact. Losing the
                    // audit record does not change that the review was cancelled.
                    let _ = failure;
                }
                // The history must carry the cancellation before the provider
                // acts on it: a later notice without it would say the deadline
                // answered a review nothing settled.
                self.emit(ExecutionEvent::new(
                    record.request().execution_id().clone(),
                    ExecutionUpdate::PermissionCancelled(record),
                ))
                .map_err(|error| self.record_failure(OperationEffectPhase::EventDelivery, error))?;
            }
            let wire_id = self
                .permissions
                .remove(&id)
                .expect("pending wire permission");
            self.send(permission_wire::permission_cancel(&wire_id))
                .await
        };
        if let Err(error) = sent {
            let correlation = delivery_correlation.unwrap_or_else(|| self.next_correlation());
            return Err(
                self.record_failure(OperationEffectPhase::PermissionDelivery(correlation), error)
            );
        }
        // The notice says the deadline decided, so it follows the write: an
        // answer the provider never received decided nothing.
        if let Ok(event) = execution.deadline_answered(&execution_id, &id) {
            self.emit(event)
                .map_err(|error| self.record_failure(OperationEffectPhase::EventDelivery, error))?;
        }
        // Codex ends the turn when the refusal it was given stops the turn.
        // The cancelled result that follows is that turn ending. Without this,
        // it is an unrequested cancel and the session is torn down.
        if stops_turn {
            self.turn_cancel_requested = Some(execution_id);
        }
        Ok(())
    }
}

/// No artificial far-future timestamp: no configured limit means no timer.
fn wait_for_deadline(clock: &dyn Clock, deadline: Option<ClockInstant>) -> ClockSleep {
    match deadline {
        Some(deadline) => clock.sleep_until(deadline),
        None => Box::pin(std::future::pending()),
    }
}

/// The first `admitted` commands in `queue`: what it held when the deadline
/// won, counted then, and nothing admitted after.
///
/// Taking a command frees its slot, and a sender waiting on a full queue
/// fills it at once, so draining until the queue is empty could take a
/// command that arrived after the deadline.
fn admitted_before_deadline<T>(queue: &mut mpsc::Receiver<T>, admitted: usize) -> Vec<T> {
    let mut ready = Vec::with_capacity(admitted);
    while ready.len() < admitted {
        match queue.try_recv() {
            Ok(command) => ready.push(command),
            Err(_) => break,
        }
    }
    ready
}

#[cfg(test)]
mod remember_tests {
    use super::{accepted_remember, PermissionContext};
    use crate::application::agent_execution::agents::AgentError;
    use crate::domain::agent_execution::executions::ExecutionId;
    use crate::domain::agent_execution::permissions::{PermissionEffect, PermissionOptionId};

    fn context(once_allow: bool, once_deny: bool) -> PermissionContext {
        PermissionContext {
            execution: ExecutionId::new("turn").unwrap(),
            tool: "Read".into(),
            once_allow: once_allow.then(|| PermissionOptionId::new("allow-once").unwrap()),
            once_deny: once_deny.then(|| PermissionOptionId::new("deny-once").unwrap()),
            specific: true,
            awaiting_consult: false,
        }
    }

    #[test]
    fn a_remember_choice_needs_the_once_option_of_its_own_execution() {
        let review = context(true, false);
        let execution = ExecutionId::new("turn").unwrap();
        assert_eq!(
            accepted_remember(&review, &execution, PermissionEffect::Allow)
                .unwrap()
                .as_str(),
            "allow-once"
        );
        assert!(matches!(
            accepted_remember(&review, &execution, PermissionEffect::Deny),
            Err(AgentError::InvalidInput(text)) if text.contains("no once choice")
        ));
        let other = ExecutionId::new("other").unwrap();
        assert!(matches!(
            accepted_remember(&review, &other, PermissionEffect::Allow),
            Err(AgentError::InvalidInput(text)) if text.contains("different execution")
        ));
        let unoffered = context(false, false);
        assert!(matches!(
            accepted_remember(&unoffered, &execution, PermissionEffect::Allow),
            Err(AgentError::InvalidInput(text)) if text.contains("no once choice")
        ));
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::admitted_before_deadline;
    use tokio::sync::mpsc;

    #[test]
    fn a_command_admitted_after_the_deadline_is_not_in_its_snapshot() {
        let (sender, mut queue) = mpsc::channel(4);
        sender.try_send("before").unwrap();
        sender.try_send("also before").unwrap();
        let admitted = queue.len();
        // Admitted after the count: a sender that was waiting for the slot
        // the first taken command frees arrives the same way.
        sender.try_send("after").unwrap();
        assert_eq!(
            admitted_before_deadline(&mut queue, admitted),
            vec!["before", "also before"]
        );
        assert_eq!(queue.try_recv().unwrap(), "after");
    }
}

#[cfg(all(test, unix))]
#[path = "../../../../tests/infrastructure/acp/executions/worker.rs"]
mod tests;
