//! Adapter coordination for validated execution state and attributed evidence.
#![deny(missing_docs)]

use super::{
    limits::{validate_message_chunk, validate_observation_id},
    AdmittedQuestion, ExecutionAuditRecord, ExecutionEvent, ExecutionUpdate,
    PermissionAuthoritySource, SessionClosureRecord,
};
use crate::application::agent_execution::agents::AgentError;
use crate::application::agent_execution::permissions::{
    validate_hook_withdrawal, ApprovalAttribution, ApprovalBasis, ApprovalRuleReference,
    CancellationOrigin, PermissionAnswer, PermissionAnswerDelivery, PermissionCancellation,
    PermissionCancellationRequest, PermissionResolution, ReviewDeclineRecord,
};
use crate::application::agent_execution::tools::ToolReviewInput;
use crate::domain::agent_execution::questions::{AgentQuestion, QuestionId};
use crate::domain::agent_execution::{
    executions::*,
    permissions::*,
    sessions::{ExecutionSession, ExecutionSessionId, HistoricalToolUndo},
    tools::*,
    ExecutionError,
};
use std::collections::{HashMap, HashSet};

const MAX_TOOLS: usize = 4096;
pub(crate) const MAX_EXECUTION_PERMISSIONS: usize = 4096;
const MAX_RETAINED_BYTES: usize = 32 * 1024 * 1024;

/// Controller-owned admission charge for a historical review request.
/// The retention witness consumes this charge without inventing pending state.
#[derive(Clone, Copy, Debug)]
pub(crate) struct HistoricalReviewCost {
    bytes: usize,
}
impl HistoricalReviewCost {
    pub(crate) fn bytes(self) -> usize {
        self.bytes
    }
}
/// Moved touched-owner state; only a disposable historical witness consumes it.
pub(crate) struct RetentionControllerUndo {
    tool: HistoricalToolUndo,
    permission: Option<(ExecutionId, PermissionId)>,
    tool_bytes: usize,
}

/// Ephemeral execution coordination shared by session adapters. The aggregate
/// owns lifecycle decisions; this controller pairs review input with permissions
/// and projects accepted observations. It owns no transport or durable history.
#[derive(Debug)]
pub struct ExecutionController {
    authority: PermissionAuthoritySource,
    session: ExecutionSession,
    review_inputs: HashMap<PermissionId, (ToolReviewInput, usize)>,
    /// The deadline each pending review published, by review.
    published_deadlines: HashMap<PermissionId, PermissionReviewDeadline>,
    /// Reviews their own deadline settled, with the deadline they published,
    /// waiting for the one notice that says so. Taking an entry is what makes
    /// the notice once-only.
    deadline_settled: HashMap<PermissionId, PermissionReviewDeadline>,
    admitted_question_ids: HashSet<QuestionId>,
    retained_tool_bytes: usize,
    retained_review_bytes: usize,
    native_hooks: Option<bool>,
}
impl ExecutionController {
    /// Maximum distinct question admissions in one execution, including answered
    /// and cancelled asks. Compact identities remain reserved until settlement.
    pub const MAX_QUESTIONS_PER_EXECUTION: usize = 4096;

    /// Creates an idle live aggregate for provider context `id`, with empty tool
    /// and permission observations. This does not open a provider or load history.
    pub fn new(id: ExecutionSessionId) -> Self {
        Self::with_authority_source(id, PermissionAuthoritySource::default())
    }
    pub(crate) fn with_authority_source(
        id: ExecutionSessionId,
        authority: PermissionAuthoritySource,
    ) -> Self {
        Self {
            authority,
            session: ExecutionSession::new(id),
            review_inputs: HashMap::new(),
            published_deadlines: HashMap::new(),
            deadline_settled: HashMap::new(),
            admitted_question_ids: HashSet::new(),
            retained_tool_bytes: 0,
            retained_review_bytes: 0,
            native_hooks: None,
        }
    }
    /// Stamp every tool event with the hooks choice of the process start this
    /// controller belongs to. One execution runs on one process start, so the
    /// stamp is the choice each call ran under. `None` leaves events unstamped.
    pub(crate) fn launched_with(mut self, native_hooks: Option<bool>) -> Self {
        self.native_hooks = native_hooks;
        self
    }
    /// Read-only carrier for the handles this controller issues on successful begin.
    pub fn permission_authority_source(&self) -> PermissionAuthoritySource {
        self.authority.clone()
    }
    /// Whether this live attachment has already recorded its permanent closure.
    /// Adapters may skip repeated teardown recording; this does not confirm resource cleanup.
    pub fn is_closed(&self) -> bool {
        self.session.is_closed()
    }

    /// Identity of the provider context owned by this live aggregate.
    pub fn id(&self) -> &ExecutionSessionId {
        self.session.id()
    }
    /// Execution still owned by the aggregate, absent after settlement or while idle.
    /// Adapters use this authoritative correlation when selecting lifecycle causes.
    pub fn active_execution_id(&self) -> Option<&ExecutionId> {
        self.session.active_execution()
    }
    /// Admits `id` as the sole active execution, retaining it until settlement.
    /// Returns Busy for an active run, Closed for a retired aggregate, or
    /// InvalidInput when the identity was already admitted by this live attachment. Identity history grows linearly until aggregate drop;
    /// normal settlement releases observations but never makes an ID reusable.
    pub fn begin_execution(&mut self, id: ExecutionId) -> Result<(), AgentError> {
        self.session.begin_execution(id).map_err(domain_error)?;
        self.authority.publish(
            self.session
                .permission_authority()
                .expect("successful execution admission"),
        );
        self.retained_tool_bytes = 0;
        Ok(())
    }
    fn event(
        &self,
        execution: &ExecutionId,
        update: ExecutionUpdate,
    ) -> Result<ExecutionEvent, AgentError> {
        self.check_execution(execution)?;
        Ok(ExecutionEvent::new(execution.clone(), update))
    }
    /// Correlates observed `message` with the current execution without I/O.
    /// Supply the captured `execution` ID; a stale or inactive target returns an error.
    /// Returns Closed after session closure, or a protocol error without an active execution
    /// or when the chunk exceeds [`ExecutionEvent::MAX_MESSAGE_CHUNK_BYTES`].
    pub fn message_event(
        &self,
        execution: &ExecutionId,
        message: MessageChunk,
    ) -> Result<ExecutionEvent, AgentError> {
        validate_message_chunk(&message)?;
        if self.session.is_closed() {
            return Err(AgentError::Closed);
        }
        self.event(execution, ExecutionUpdate::Message(message))
    }
    /// Correlates the adapter-established `outcome` with the active execution.
    /// Supply the captured `execution` ID; a stale or inactive target returns an error.
    /// This constructs a projection without settling the aggregate or publishing
    /// evidence. Returns a protocol error when no execution is active.
    pub fn finished_event(
        &self,
        execution: &ExecutionId,
        outcome: ExecutionOutcome,
    ) -> Result<ExecutionEvent, AgentError> {
        self.event(execution, ExecutionUpdate::Finished(outcome))
    }
    /// Applies sparse `update` to the active execution's tool and creates its event.
    /// Supply the captured `execution` ID; a stale or inactive target returns an error.
    /// Returns a protocol error for an invalid domain transition or retention limit,
    /// and InvalidInput for an oversized tool identity. No event is published here.
    pub fn tool_event(
        &mut self,
        execution: &ExecutionId,
        update: ToolCallUpdate,
    ) -> Result<ExecutionEvent, AgentError> {
        self.validate_tool_retention(execution, &update)?;
        self.observe_tool(execution, update.clone())?;
        let event = self.event(execution, ExecutionUpdate::Tool(update))?;
        Ok(match self.native_hooks {
            Some(allowed) => event.with_native_hooks(allowed),
            None => event,
        })
    }
    /// Bind an adapter-minted ask identity and question to this active execution.
    ///
    /// Returns an immutable admission from which the adapter derives events and
    /// audit records. The controller reserves `id` until successful execution
    /// settlement, including after answer/cancellation; the adapter owns open-ask
    /// limits and delivery ordering. This method publishes nothing.
    ///
    /// # Errors
    /// Returns Closed after session closure, InvalidInput for an inactive/different
    /// execution or an already admitted ID, and Protocol at
    /// [`Self::MAX_QUESTIONS_PER_EXECUTION`] or for invalid event payload size.
    /// Rejection leaves identity history and earlier admissions unchanged.
    pub fn ask_question(
        &mut self,
        execution: &ExecutionId,
        id: QuestionId,
        question: AgentQuestion,
    ) -> Result<AdmittedQuestion, AgentError> {
        if self.session.is_closed() {
            return Err(AgentError::Closed);
        }
        self.check_execution(execution)?;
        if self.admitted_question_ids.contains(&id) {
            return Err(AgentError::InvalidInput(
                "question ID was already admitted".into(),
            ));
        }
        if self.admitted_question_ids.len() >= Self::MAX_QUESTIONS_PER_EXECUTION {
            return Err(AgentError::Protocol(
                "execution question limit exceeded".into(),
            ));
        }
        let admitted = AdmittedQuestion::new(self.id().clone(), execution.clone(), id, question);
        admitted.event().validate_payload_size()?;
        self.admitted_question_ids.insert(admitted.id().clone());
        Ok(admitted)
    }
    /// Capture a refusal for this controller's active execution before delivery
    /// or settlement. This records no audit, changes no lifecycle state and does
    /// not evaluate policy; the adapter supplies its already decided refusal.
    ///
    /// `execution` is the captured turn, `id` is the adapter-allocated decline
    /// identity, `decline` retains the tool/reason/outcome, and `rule` names the
    /// configured stopping rule. ID uniqueness remains the adapter's responsibility.
    ///
    /// # Errors
    /// Returns Closed for a closed session, InvalidInput for a foreign/inactive
    /// execution or a stopping outcome without its rule (or a continue outcome
    /// with one). The adapter owns decline ID allocation and audit ordering.
    /// Keep the returned record and use `with_delivery` for later write evidence,
    /// including after this execution finishes or another one begins.
    ///
    /// ```
    /// use nessa_sdk::application::agent_execution::executions::ExecutionController;
    /// use nessa_sdk::application::agent_execution::permissions::PermissionAnswerDelivery;
    /// use nessa_sdk::domain::agent_execution::{
    ///     executions::ExecutionId, sessions::ExecutionSessionId,
    ///     permissions::{ReviewDecline, ReviewDeclineId, ReviewDeclineReason, RefusalOutcome},
    /// };
    /// let mut controller = ExecutionController::new(ExecutionSessionId::new("session")?);
    /// let turn = ExecutionId::new("turn")?;
    /// controller.begin_execution(turn.clone())?;
    /// let selected = controller.decline_review(
    ///     &turn, ReviewDeclineId::new("1")?,
    ///     ReviewDecline::new(Some("Bash"), ReviewDeclineReason::ToolNotReviewable,
    ///         RefusalOutcome::Continue), None,
    /// )?;
    /// let written = selected.with_delivery(PermissionAnswerDelivery::Written);
    /// assert_eq!(written.session_id(), controller.id());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn decline_review(
        &self,
        execution: &ExecutionId,
        id: ReviewDeclineId,
        decline: ReviewDecline,
        rule: Option<ApprovalRuleReference>,
    ) -> Result<ReviewDeclineRecord, AgentError> {
        if self.session.is_closed() {
            return Err(AgentError::Closed);
        }
        self.check_execution(execution)?;
        ReviewDeclineRecord::new(
            self.id().clone(),
            execution.clone(),
            id,
            decline,
            PermissionAnswerDelivery::Selected,
            rule,
        )
        .map_err(|error| AgentError::InvalidInput(error.to_string()))
    }
    /// Admits review `id` for the observed `tool`, retaining its exact `input` and
    /// offered `options` until resolution. Produces the correlated review event.
    /// Supply the captured `execution` ID; a stale or inactive target returns an error.
    /// Rejects stale/duplicate identity, missing execution, or retention limits
    /// before admitting the review. Performs no provider response or audit I/O.
    pub fn request_permission(
        &mut self,
        execution: &ExecutionId,
        id: PermissionId,
        tool: ToolCallUpdate,
        input: ToolReviewInput,
        options: PermissionOptions,
        deadline: Option<crate::domain::agent_execution::permissions::PermissionReviewDeadline>,
    ) -> Result<ExecutionEvent, AgentError> {
        let (charged_bytes, review_bytes) =
            self.validate_review_retention(execution, &id, tool.id(), &input, &options)?;
        let tool_id = tool.id().clone();
        let request = PermissionRequest::new(
            id.clone(),
            execution.clone(),
            tool_id.clone(),
            options.clone(),
        );
        self.session
            .validate_permission_admission(&request)
            .map_err(domain_error)?;
        self.observe_tool(execution, tool)?;
        self.session
            .request_permission(request)
            .map_err(domain_error)?;
        let observation = self
            .session
            .tool(&tool_id)
            .expect("accepted observed tool")
            .observation()
            .clone();
        self.review_inputs
            .insert(id.clone(), (input.clone(), charged_bytes));
        self.retained_review_bytes = review_bytes;
        if let Some(deadline) = &deadline {
            self.published_deadlines
                .insert(id.clone(), deadline.clone());
        }
        self.event(
            execution,
            ExecutionUpdate::PermissionRequested {
                id,
                tool_id,
                observation,
                input,
                options,
                deadline,
            },
        )
    }

    /// What the session will know about `update`'s tool once it is applied:
    /// the observation already held for that tool, or none, with the update's
    /// fields over it. A permission frame may carry only the tool's id, the
    /// kind and paths having come in an earlier update, so a review is judged
    /// on this and not on the frame alone. Reads only; nothing is admitted.
    pub(crate) fn observation_after(&self, update: &ToolCallUpdate) -> ToolObservation {
        self.session
            .tool(update.id())
            .map(|tool| tool.observation().clone())
            .unwrap_or_default()
            .with_update(update.clone())
    }

    /// The lasting observation that `id`'s own deadline settled it.
    ///
    /// Does not answer the review. The caller first settles it here, with an
    /// answer attributed to the deadline or a cancellation for
    /// `deadline_exceeded`, and then emits this so a surface that was not
    /// watching the audit still sees who decided. The effect and reason are
    /// the ones the review published, not the caller's. A review that was not
    /// settled by its deadline, or whose notice was already made, returns
    /// [`AgentError::StalePermission`]; a stale execution returns the same
    /// error as any other event. Only the ACP worker emits it: a permission id
    /// from that worker shares the decimal sequence with a review decline, so
    /// it is a valid notice id.
    pub(crate) fn deadline_answered(
        &mut self,
        execution: &ExecutionId,
        id: &PermissionId,
    ) -> Result<ExecutionEvent, AgentError> {
        self.check_execution(execution)?;
        let deadline = self
            .deadline_settled
            .remove(id)
            .ok_or(AgentError::StalePermission)?;
        self.event(
            execution,
            ExecutionUpdate::DeadlineAnswered {
                id: id.clone(),
                effect: deadline.effect(),
                reason: deadline.recorded_reason(),
            },
        )
    }

    /// Keep `id`'s published deadline as settlement evidence when `by_deadline`,
    /// and let it go otherwise. A review answered by anyone else has no notice.
    fn settle_published_deadline(&mut self, id: &PermissionId, by_deadline: bool) {
        if let Some(deadline) = self.published_deadlines.remove(id) {
            if by_deadline {
                self.deadline_settled.insert(id.clone(), deadline);
            }
        }
    }

    pub(crate) fn validate_review_retention(
        &self,
        execution: &ExecutionId,
        id: &PermissionId,
        tool: &ToolCallId,
        input: &ToolReviewInput,
        options: &PermissionOptions,
    ) -> Result<(usize, usize), AgentError> {
        self.check_execution(execution)?;
        validate_observation_id(id.as_str())?;
        if self.session.permission_count() >= MAX_EXECUTION_PERMISSIONS {
            return Err(AgentError::Protocol(
                "execution permission limit exceeded".into(),
            ));
        }
        let charged_bytes =
            Self::historical_review_cost(execution, id, tool, input, options)?.bytes();
        let review_bytes = self.retained_review_bytes.saturating_add(charged_bytes);
        Self::validate_review_totals(self.review_inputs.len().saturating_add(1), review_bytes)?;
        Ok((charged_bytes, review_bytes))
    }
    pub(crate) fn historical_review_cost(
        execution: &ExecutionId,
        id: &PermissionId,
        tool: &ToolCallId,
        input: &ToolReviewInput,
        options: &PermissionOptions,
    ) -> Result<HistoricalReviewCost, AgentError> {
        Self::validate_review_payload(execution, id, tool, input, options)
            .map(|bytes| HistoricalReviewCost { bytes })
    }
    pub(crate) fn validate_review_totals(count: usize, bytes: usize) -> Result<(), AgentError> {
        if count > MAX_OPEN_REVIEWS {
            return Err(AgentError::Protocol(
                "pending permission limit exceeded".into(),
            ));
        }
        Self::validate_review_bytes(bytes)
    }
    pub(crate) fn retain_historical_tool(
        &mut self,
        execution: &ExecutionId,
        update: ToolCallUpdate,
    ) -> Result<RetentionControllerUndo, AgentError> {
        let total = self.validate_tool_retention(execution, &update)?;
        let tool = self
            .session
            .observe_tool_reversible(execution, update)
            .map_err(domain_error)?;
        let tool_bytes = std::mem::replace(&mut self.retained_tool_bytes, total);
        Ok(RetentionControllerUndo {
            tool,
            permission: None,
            tool_bytes,
        })
    }
    pub(crate) fn retain_historical_review(
        &mut self,
        execution: &ExecutionId,
        id: PermissionId,
        tool: ToolCallUpdate,
        input: &ToolReviewInput,
        options: &PermissionOptions,
    ) -> Result<(HistoricalReviewCost, RetentionControllerUndo), AgentError> {
        let (bytes, _) =
            self.validate_review_retention(execution, &id, tool.id(), input, options)?;
        let request = PermissionRequest::new(
            id.clone(),
            execution.clone(),
            tool.id().clone(),
            options.clone(),
        );
        self.session
            .validate_permission_admission(&request)
            .map_err(domain_error)?;
        let mut undo = self.retain_historical_tool(execution, tool)?;
        if let Err(error) = self.session.request_permission(request) {
            self.restore_historical_retention(undo);
            return Err(domain_error(error));
        }
        // This is the same minimum-retention witness as the full snapshot
        // validator: possible immediate release, never persisted answer evidence.
        self.session
            .answer_permission(execution, &id, options.choices()[0].id())
            .map_err(domain_error)?;
        undo.permission = Some((execution.clone(), id));
        Ok((HistoricalReviewCost { bytes }, undo))
    }
    pub(crate) fn restore_historical_retention(&mut self, undo: RetentionControllerUndo) {
        if let Some((execution, permission)) = undo.permission {
            self.session
                .restore_historical_permission_identity(&execution, &permission);
        }
        self.session.restore_historical_tool(undo.tool);
        self.retained_tool_bytes = undo.tool_bytes;
    }
    pub(crate) fn historical_retained_bytes(&self) -> usize {
        self.session
            .historical_retained_bytes()
            .saturating_add(self.authority.allocation_bytes())
            .saturating_add(self.retained_tool_bytes)
            .saturating_add(self.review_inputs.capacity().saturating_mul(
                std::mem::size_of::<(PermissionId, (ToolReviewInput, usize))>()
                    + std::mem::size_of::<usize>(),
            ))
            .saturating_add(
                self.admitted_question_ids.capacity().saturating_mul(
                    std::mem::size_of::<QuestionId>() + std::mem::size_of::<usize>(),
                ),
            )
    }
    pub(crate) fn validate_review_payload(
        execution: &ExecutionId,
        id: &PermissionId,
        tool: &ToolCallId,
        input: &ToolReviewInput,
        options: &PermissionOptions,
    ) -> Result<usize, AgentError> {
        validate_observation_id(id.as_str())?;
        validate_observation_id(tool.as_str())?;
        let bytes = input_bytes(input)
            .saturating_add(options.payload_bytes())
            .saturating_add(review_identity_bytes(execution, id, tool));
        Self::validate_review_bytes(bytes)?;
        Ok(bytes)
    }
    fn validate_review_bytes(bytes: usize) -> Result<(), AgentError> {
        if bytes > MAX_RETAINED_BYTES {
            return Err(AgentError::Protocol(
                "permission review retention limit exceeded".into(),
            ));
        }
        Ok(())
    }
    pub(crate) fn validate_tool_payload(bytes: usize) -> Result<(), AgentError> {
        if bytes > MAX_RETAINED_BYTES {
            return Err(AgentError::Protocol("tool retention limit exceeded".into()));
        }
        Ok(())
    }
    /// Resolves `answer` once against its execution, review, and offered option.
    /// Returns StalePermission if it cannot match a pending review. A hook basis
    /// paired with anything other than the request-scoped deny option, with
    /// an actor other than [`ApprovalAttribution::hook`] for this review, or
    /// with an outcome other than continue, returns InvalidInput and leaves
    /// the review pending. The runner withdraws a review it refuses with
    /// end-turn or close-session, so no answer carries those outcomes. The resolution
    /// retains this controller’s session, original input and attribution; the adapter must audit selection
    /// before attempting delivery and audit that delivery independently of waiters.
    ///
    /// This is the caller's answer, so it cannot claim the deadline decided:
    /// that is InvalidInput, and the review stays pending. Only the worker that
    /// armed the deadline settles it, through `settle_deadline`.
    pub fn answer_permission(
        &mut self,
        answer: PermissionAnswer,
    ) -> Result<PermissionResolution, AgentError> {
        if matches!(answer.attribution.basis(), ApprovalBasis::Deadline(_)) {
            return Err(AgentError::InvalidInput(
                "only the review's own deadline can answer it as a deadline".into(),
            ));
        }
        if let ApprovalBasis::Hook(refusal) = answer.attribution.basis() {
            if answer.attribution != ApprovalAttribution::hook(&answer.id, refusal.clone())? {
                return Err(AgentError::InvalidInput(
                    "a hook refusal is attributed to the permission-review hook".into(),
                ));
            }
            if refusal.outcome() != &RefusalOutcome::Continue {
                return Err(AgentError::InvalidInput(
                    "a hook answer records only continue; a terminal refusal withdraws the review"
                        .into(),
                ));
            }
            let decision = self
                .session
                .pending_option_decision(&answer.execution_id, &answer.id, &answer.option_id)
                .map_err(|_| AgentError::StalePermission)?;
            if decision.effect() != PermissionEffect::Deny
                || decision.scope() != &PermissionScope::request()
            {
                return Err(AgentError::InvalidInput(
                    "a hook refusal selects the request-scoped deny option".into(),
                ));
            }
        }
        self.resolve(answer, false)
    }
    /// The deadline the worker armed for review `id` selects `option`.
    ///
    /// `deadline` must be the one this review published, and its outcome the
    /// option's; anything else is InvalidInput and consumes nothing, so the
    /// review stays pending. Returns StalePermission when no pending review
    /// matches.
    pub(crate) fn settle_deadline(
        &mut self,
        execution: &ExecutionId,
        id: &PermissionId,
        option: &PermissionOptionId,
        deadline: PermissionReviewDeadline,
    ) -> Result<PermissionResolution, AgentError> {
        let decision = self
            .session
            .pending_option_decision(execution, id, option)
            .map_err(|_| AgentError::StalePermission)?;
        let published = self
            .published_deadlines
            .get(id)
            .is_some_and(|published| published.decides_as(&deadline));
        if !published || decision.effect() != deadline.effect() {
            return Err(AgentError::InvalidInput(
                "a deadline settlement must be the deadline this review published, \
                 selecting an option with its outcome"
                    .into(),
            ));
        }
        let answer = PermissionAnswer {
            attribution: ApprovalAttribution::deadline(id, deadline)?,
            execution_id: execution.clone(),
            id: id.clone(),
            option_id: option.clone(),
        };
        self.resolve(answer, true)
    }
    /// [`Self::settle_deadline`] for another crate's tests, which have no
    /// worker to arm a deadline. Production builds do not have it.
    #[cfg(feature = "test-support")]
    pub fn settle_deadline_for_test(
        &mut self,
        execution: &ExecutionId,
        id: &PermissionId,
        option: &PermissionOptionId,
        deadline: PermissionReviewDeadline,
    ) -> Result<PermissionResolution, AgentError> {
        self.settle_deadline(execution, id, option, deadline)
    }
    fn resolve(
        &mut self,
        answer: PermissionAnswer,
        by_deadline: bool,
    ) -> Result<PermissionResolution, AgentError> {
        let request = self
            .session
            .answer_permission(&answer.execution_id, &answer.id, &answer.option_id)
            .map_err(|_| AgentError::StalePermission)?;
        let (input, charged_bytes) = self
            .review_inputs
            .remove(&answer.id)
            .expect("controller retains the accepted review input");
        self.retained_review_bytes -= charged_bytes;
        self.settle_published_deadline(&answer.id, by_deadline);
        PermissionResolution::new(self.id().clone(), request, input, answer.attribution)
    }
    // Only the snapshot retention validator uses this on a disposable controller.
    // Answers are absent from observations: release the hypothetical pending
    // payload without manufacturing attribution or durable resolution evidence.
    /// Resolves explicit cancellation `input`, retaining its actor and exact reason.
    /// Returns InvalidInput for a non-Custom caller cause, or Closed/StalePermission
    /// when the live review cannot be matched. Rejection leaves pending state intact.
    /// The caller must deliver returned evidence to the mandatory audit port.
    pub fn cancel_review(
        &mut self,
        input: PermissionCancellationRequest,
    ) -> Result<PermissionCancellation, AgentError> {
        if !matches!(
            input.reason.view(),
            PermissionCancellationReasonView::Custom(_)
        ) {
            return Err(AgentError::InvalidInput(
                "explicit permission cancellation requires a custom caller reason".into(),
            ));
        }
        if self.session.is_closed() {
            return Err(AgentError::Closed);
        }
        if self.session.active_execution() != Some(&input.execution_id) {
            return Err(AgentError::StalePermission);
        }
        self.cancel_permission(
            &input.execution_id,
            &input.id,
            input.reason,
            CancellationOrigin::Client(input.actor),
        )?
        .ok_or(AgentError::StalePermission)
    }
    /// Cancels pending review `id` in the active execution for `reason` and `origin`.
    /// Supply the captured `execution` ID; a stale or inactive target returns an error.
    /// Returns InvalidInput for a contradictory cause/origin without changing state.
    /// Returns None if the live review is absent; the returned evidence includes
    /// the original review input. The adapter owns audit and provider effects.
    #[must_use = "record cancellation evidence before discarding it"]
    pub fn cancel_permission(
        &mut self,
        execution: &ExecutionId,
        id: &PermissionId,
        reason: PermissionCancellationReason,
        origin: CancellationOrigin,
    ) -> Result<Option<PermissionCancellation>, AgentError> {
        not_a_deadline(&origin)?;
        self.cancel_one(execution, id, reason, origin, false)
    }
    /// The deadline the worker armed for review `id` cancels it, because the
    /// agent offered no refusal to select.
    ///
    /// `deadline` must be the one this review published; anything else is
    /// InvalidInput and changes nothing. Returns None if the review is absent.
    #[must_use = "record cancellation evidence before discarding it"]
    pub(crate) fn cancel_deadline(
        &mut self,
        execution: &ExecutionId,
        id: &PermissionId,
        deadline: PermissionReviewDeadline,
    ) -> Result<Option<PermissionCancellation>, AgentError> {
        if !self
            .published_deadlines
            .get(id)
            .is_some_and(|published| published.decides_as(&deadline))
        {
            return Err(AgentError::InvalidInput(
                "a deadline cancellation must be the deadline this review published".into(),
            ));
        }
        self.cancel_one(
            execution,
            id,
            PermissionCancellationReason::deadline_exceeded(),
            CancellationOrigin::Deadline(deadline),
            true,
        )
    }
    /// [`Self::cancel_deadline`] for another crate's tests, which have no
    /// worker to arm a deadline. Production builds do not have it.
    #[cfg(feature = "test-support")]
    pub fn cancel_deadline_for_test(
        &mut self,
        execution: &ExecutionId,
        id: &PermissionId,
        deadline: PermissionReviewDeadline,
    ) -> Result<Option<PermissionCancellation>, AgentError> {
        self.cancel_deadline(execution, id, deadline)
    }
    fn cancel_one(
        &mut self,
        execution: &ExecutionId,
        id: &PermissionId,
        reason: PermissionCancellationReason,
        origin: CancellationOrigin,
        by_deadline: bool,
    ) -> Result<Option<PermissionCancellation>, AgentError> {
        origin.validate_reason(&reason)?;
        validate_hook_withdrawal(id, &reason, &origin)?;
        let request = self
            .session
            .cancel_permission(execution, id, reason)
            .map_err(domain_error)?;
        Ok(request.map(|request| {
            // A cancellation that is this review's own deadline settles it.
            self.settle_published_deadline(id, by_deadline);
            self.cancellation(request, origin)
        }))
    }
    fn cancellation(
        &mut self,
        request: PermissionRequest,
        origin: CancellationOrigin,
    ) -> PermissionCancellation {
        let (input, charged_bytes) = self
            .review_inputs
            .remove(request.id())
            .expect("accepted review input");
        self.retained_review_bytes -= charged_bytes;
        self.settle_published_deadline(request.id(), false);
        PermissionCancellation::new(self.id().clone(), request, input, origin)
    }
    /// Cancels pending reviews in `execution` with the lifecycle `reason` and `origin`.
    /// A contradictory cause/origin returns InvalidInput. A different or inactive
    /// execution also returns an error. Rejection leaves retained state unchanged.
    /// Retains resolved identities until execution settlement to prevent replay.
    /// Returns all mandatory evidence; this performs no audit or provider I/O.
    #[must_use = "record every cancellation, including lifecycle cleanup"]
    pub fn cancel_permissions(
        &mut self,
        execution: &ExecutionId,
        reason: PermissionCancellationReason,
        origin: CancellationOrigin,
    ) -> Result<Vec<PermissionCancellation>, AgentError> {
        not_a_deadline(&origin)?;
        origin.validate_reason(&reason)?;
        Ok(self
            .session
            .cancel_permissions(execution, reason)
            .map_err(domain_error)?
            .into_iter()
            .map(|request| self.cancellation(request, origin.clone()))
            .collect())
    }
    /// Closes this live aggregate and returns mandatory lifecycle audit evidence.
    /// `reason` identifies the lifecycle cause; `origin` retains verified caller
    /// attribution or the automatic initiator. The first close returns a session
    /// record even when idle, followed by every pending permission cancellation.
    /// Execution-owned callbacks use `close_execution` with their captured ID;
    /// this session-wide method rejects `ExecutionFailed`.
    /// Repeated close returns no records. Invalid cause/origin pairs return InvalidInput
    /// without changing state. This performs no provider/process I/O.
    #[must_use = "record every closure and cancellation before reporting shutdown"]
    pub fn close(
        &mut self,
        reason: PermissionCancellationReason,
        origin: CancellationOrigin,
    ) -> Result<Vec<ExecutionAuditRecord>, AgentError> {
        self.close_target(None, reason, origin)
    }

    /// Close only the attachment still running the captured `execution`.
    /// `reason` describes why that execution requires closure; `origin` retains
    /// caller attribution or its automatic initiator. A stale target or invalid
    /// cause returns an error before mutation. Retain all returned audit records.
    #[must_use = "record every closure and cancellation before reporting shutdown"]
    pub fn close_execution(
        &mut self,
        execution: &ExecutionId,
        reason: PermissionCancellationReason,
        origin: CancellationOrigin,
    ) -> Result<Vec<ExecutionAuditRecord>, AgentError> {
        self.close_target(Some(execution), reason, origin)
    }

    fn close_target(
        &mut self,
        execution: Option<&ExecutionId>,
        reason: PermissionCancellationReason,
        origin: CancellationOrigin,
    ) -> Result<Vec<ExecutionAuditRecord>, AgentError> {
        not_a_deadline(&origin)?;
        origin.validate_reason(&reason)?;
        let result = match execution {
            Some(execution) => self.session.close_execution(execution, reason),
            None => self.session.close(reason),
        };
        let (closure, permissions) = result.map_err(domain_error)?.into_parts();
        let mut records = Vec::new();
        if let Some(closure) = closure {
            records.push(ExecutionAuditRecord::SessionClosed(
                SessionClosureRecord::new(closure, origin.clone())?,
            ));
        }
        records.extend(permissions.into_iter().map(|request| {
            ExecutionAuditRecord::Cancelled(self.cancellation(request, origin.clone()))
        }));
        Ok(records)
    }
    /// Settles the matching `execution`, releasing retained tools, review identities,
    /// and admitted question identities. Returned question evidence remains valid.
    /// The terminal `result` is retained even without reviews. Any remaining permission
    /// is cancelled with its failure cause or ExecutionFinished and a runtime origin;
    /// every returned record must be audited. A stale or idle target returns an error
    /// before mutation. Supply the ID captured when the provider invocation began.
    /// This changes local state only; the adapter establishes external settlement.
    pub fn finish_execution(
        &mut self,
        execution: &ExecutionId,
        result: Result<ExecutionOutcome, PermissionCancellationReason>,
    ) -> Result<Vec<ExecutionAuditRecord>, AgentError> {
        let (finished, cancelled) = self
            .session
            .finish_execution(execution, result)
            .map_err(domain_error)?;
        let mut records = vec![ExecutionAuditRecord::Finished(finished)];
        records.extend(cancelled.into_iter().map(|request| {
            ExecutionAuditRecord::Cancelled(self.cancellation(request, CancellationOrigin::Runtime))
        }));
        self.retained_tool_bytes = 0;
        self.admitted_question_ids.clear();
        // A notice is about the execution that is ending; none can follow it.
        self.deadline_settled.clear();
        Ok(records)
    }
    fn check_execution(&self, execution: &ExecutionId) -> Result<(), AgentError> {
        if self.session.active_execution() != Some(execution) {
            return Err(AgentError::InvalidInput(
                "event targets a different or inactive execution".into(),
            ));
        }
        Ok(())
    }
    pub(crate) fn validate_tool_retention(
        &self,
        execution: &ExecutionId,
        update: &ToolCallUpdate,
    ) -> Result<usize, AgentError> {
        validate_observation_id(update.id().as_str())?;
        self.check_execution(execution)?;
        let previous = self.session.tool(update.id());
        if previous.is_none() && self.session.tool_count() >= MAX_TOOLS {
            return Err(AgentError::Protocol("tool count limit exceeded".into()));
        }
        let old_bytes = self.session.tool_payload_bytes(update.id());
        let new_bytes = self
            .session
            .tool_payload_bytes_after(execution, update)
            .map_err(domain_error)?;
        let total = self
            .retained_tool_bytes
            .saturating_sub(old_bytes)
            .saturating_add(new_bytes);
        Self::validate_tool_payload(total)?;
        Ok(total)
    }
    fn observe_tool(
        &mut self,
        execution: &ExecutionId,
        update: ToolCallUpdate,
    ) -> Result<(), AgentError> {
        let total = self.validate_tool_retention(execution, &update)?;
        self.session
            .observe_tool(execution, update)
            .map_err(domain_error)?;
        self.retained_tool_bytes = total;
        Ok(())
    }
}
// A pending request owns execution/permission/tool IDs. Both the aggregate's
// pending map and this controller's input map retain a separate permission key.
// The seen-ID history is retained after resolution and bounded separately.
fn review_identity_bytes(
    execution: &ExecutionId,
    permission: &PermissionId,
    tool: &ToolCallId,
) -> usize {
    execution
        .as_str()
        .len()
        .saturating_add(permission.as_str().len().saturating_mul(3))
        .saturating_add(tool.as_str().len())
}
fn input_bytes(input: &ToolReviewInput) -> usize {
    input
        .name
        .capacity()
        .saturating_add(input.arguments_json.capacity())
}
fn domain_error(error: ExecutionError) -> AgentError {
    match error {
        ExecutionError::SessionBusy => AgentError::Busy,
        ExecutionError::SessionClosed => AgentError::Closed,
        error @ (ExecutionError::InvalidExecutionFailureReason
        | ExecutionError::InvalidPermissionCancellationReason
        | ExecutionError::InvalidSessionClosureReason
        | ExecutionError::ReservedCancellationCode
        | ExecutionError::DuplicateExecution) => AgentError::InvalidInput(error.to_string()),
        error => AgentError::Protocol(error.to_string()),
    }
}

/// Only the worker that armed a review's deadline cancels as it, one review at
/// a time, through [`ExecutionController::cancel_deadline`].
fn not_a_deadline(origin: &CancellationOrigin) -> Result<(), AgentError> {
    if matches!(origin, CancellationOrigin::Deadline(_)) {
        return Err(AgentError::InvalidInput(
            "only the review's own deadline can cancel it as a deadline".into(),
        ));
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::agent_execution::permissions::{
        ActionContext, ApprovalAttribution, ApprovalBasis,
    };

    use crate::domain::agent_execution::permissions::CustomPermissionCancellationReason;

    #[test]
    fn a_launched_controller_stamps_tool_events_and_nothing_else() {
        use crate::domain::agent_execution::tools::{ToolCallId, ToolCallUpdate};
        let execution = ExecutionId::new("turn").unwrap();
        let tool =
            || ToolCallUpdate::new(ToolCallId::new("t").unwrap(), None, None, None, None, None);
        for (launched, stamp) in [
            (Some(true), Some(true)),
            (Some(false), Some(false)),
            (None, None),
        ] {
            let mut controller =
                ExecutionController::new(ExecutionSessionId::new("session").unwrap())
                    .launched_with(launched);
            controller.begin_execution(execution.clone()).unwrap();
            assert_eq!(
                controller
                    .tool_event(&execution, tool())
                    .unwrap()
                    .native_hooks(),
                stamp
            );
            let message = controller
                .message_event(&execution, MessageChunk::text("hi"))
                .unwrap();
            assert_eq!(message.native_hooks(), None);
        }
    }

    #[test]
    fn cause_origin_validation_preserves_pending_state_and_retained_budgets() {
        let reasons = [
            (
                PermissionCancellationReason::provider_withdrawal(),
                [false, true, false],
            ),
            (
                PermissionCancellationReason::session_closed(),
                [true, false, true],
            ),
            (
                PermissionCancellationReason::session_failed(),
                [false, false, true],
            ),
            (
                PermissionCancellationReason::execution_finished(),
                [false, false, true],
            ),
            (
                PermissionCancellationReason::execution_failed(),
                [false, false, true],
            ),
            (
                PermissionCancellationReason::deadline_exceeded(),
                [false, false, true],
            ),
            (
                PermissionCancellationReason::event_consumer_dropped(),
                [false, false, true],
            ),
            (
                PermissionCancellationReason::session_handles_dropped(),
                [false, false, true],
            ),
            (
                PermissionCancellationReason::custom(
                    CustomPermissionCancellationReason::new("guard", "guard withdrew review")
                        .unwrap(),
                ),
                [true, false, false],
            ),
        ];
        let origins = [
            CancellationOrigin::Client(ActionContext::new("guard", "host", "withdraw").unwrap()),
            CancellationOrigin::Provider,
            CancellationOrigin::Runtime,
        ];
        for (reason, allowed) in reasons {
            for (origin, allowed) in origins.iter().zip(allowed) {
                for operation in 0..3 {
                    let mut controller = controller();
                    request(&mut controller, "permission", "original").unwrap();
                    let bytes = controller.retained_review_bytes;
                    let result = match operation {
                        0 => controller
                            .cancel_permission(
                                &ExecutionId::new("execution").unwrap(),
                                &PermissionId::new("permission").unwrap(),
                                reason.clone(),
                                origin.clone(),
                            )
                            .map(|record| assert!(record.is_some())),
                        1 => controller
                            .cancel_permissions(
                                &ExecutionId::new("execution").unwrap(),
                                reason.clone(),
                                origin.clone(),
                            )
                            .map(|records| assert_eq!(records.len(), 1)),
                        _ => controller
                            .close_execution(
                                &ExecutionId::new("execution").unwrap(),
                                reason.clone(),
                                origin.clone(),
                            )
                            .map(|records| assert_eq!(records.len(), 2)),
                    };
                    let closure_cause = !matches!(
                        reason.view(),
                        PermissionCancellationReasonView::ProviderWithdrawal
                            | PermissionCancellationReasonView::ExecutionFinished
                            | PermissionCancellationReasonView::Custom(_)
                    );
                    let terminal_cancellation = matches!(
                        reason.view(),
                        PermissionCancellationReasonView::ExecutionFinished
                            | PermissionCancellationReasonView::ExecutionFailed
                            | PermissionCancellationReasonView::SessionFailed
                            | PermissionCancellationReasonView::SessionClosed
                            | PermissionCancellationReasonView::EventConsumerDropped
                            | PermissionCancellationReasonView::SessionHandlesDropped
                    );
                    let valid_lifecycle = if operation == 2 {
                        closure_cause
                    } else {
                        !terminal_cancellation
                    };
                    if allowed && valid_lifecycle {
                        assert!(result.is_ok(), "{reason:?} {origin:?}");
                        assert_eq!(controller.retained_review_bytes, 0);
                    } else {
                        assert!(matches!(result, Err(AgentError::InvalidInput(_))));
                        assert!(!controller.session.is_closed());
                        assert_eq!(controller.retained_review_bytes, bytes);
                        let resolution =
                            controller.answer_permission(answer("permission")).unwrap();
                        assert_eq!(resolution.input(), &input());
                    }
                }
                let mut request = PermissionRequest::new(
                    PermissionId::new("permission").unwrap(),
                    ExecutionId::new("execution").unwrap(),
                    ToolCallId::new("tool").unwrap(),
                    options("Allow".into()),
                );
                request.cancel(reason.clone()).unwrap();
                assert_eq!(
                    PermissionCancellation::from_record(
                        ExecutionSessionId::new("session").unwrap(),
                        request,
                        input(),
                        origin.clone()
                    )
                    .is_ok(),
                    allowed
                );
                let mut controller = controller();
                if let Ok(result) = controller
                    .session
                    .close_execution(&ExecutionId::new("execution").unwrap(), reason.clone())
                {
                    let (closure, _) = result.into_parts();
                    assert_eq!(
                        SessionClosureRecord::new(closure.unwrap(), origin.clone()).is_ok(),
                        allowed
                    );
                }
            }
        }
    }

    fn controller() -> ExecutionController {
        let mut controller = ExecutionController::new(ExecutionSessionId::new("session").unwrap());
        controller
            .begin_execution(ExecutionId::new("execution").unwrap())
            .unwrap();
        controller
    }
    fn tool(title: Option<String>) -> ToolCallUpdate {
        ToolCallUpdate::new(
            ToolCallId::new("tool").unwrap(),
            title,
            None,
            None,
            None,
            None,
        )
    }
    fn options(label: String) -> PermissionOptions {
        PermissionOptions::new(
            vec![PermissionOption::new(
                PermissionOptionId::new("allow").unwrap(),
                label,
                PermissionDecision::new(PermissionEffect::Allow, PermissionScope::request()),
            )
            .unwrap()],
            &PermissionOfferPolicy::once_only(),
        )
        .unwrap()
    }
    fn input() -> ToolReviewInput {
        ToolReviewInput {
            name: "Write".into(),
            arguments_json: "reviewed input".into(),
        }
    }
    fn request(
        controller: &mut ExecutionController,
        id: &str,
        title: &str,
    ) -> Result<ExecutionEvent, AgentError> {
        let execution = controller.active_execution_id().unwrap().clone();
        controller.request_permission(
            &execution,
            PermissionId::new(id).unwrap(),
            tool(Some(title.into())),
            input(),
            options("Allow".into()),
            None,
        )
    }
    fn answer(id: &str) -> PermissionAnswer {
        PermissionAnswer {
            execution_id: ExecutionId::new("execution").unwrap(),
            id: PermissionId::new(id).unwrap(),
            option_id: PermissionOptionId::new("allow").unwrap(),
            attribution: ApprovalAttribution::new(
                ActionContext::new("actor", "surface", "answer").unwrap(),
                ApprovalBasis::Explicit,
            ),
        }
    }
    #[test]
    fn a_deadline_notice_is_made_once_and_only_for_a_review_its_deadline_settled() {
        use crate::domain::agent_execution::permissions::{
            PermissionToolClass, STANDARD_ALLOW_REASON, STANDARD_DENY_REASON,
        };
        use std::time::Duration;
        let execution = ExecutionId::new("execution").unwrap();
        let permission = PermissionId::new("permission").unwrap();
        let deadline = |effect, reason| {
            PermissionReviewDeadline::new(
                PermissionToolClass::Other,
                Duration::from_secs(30),
                effect,
                reason,
            )
            .unwrap()
        };
        let allow = deadline(PermissionEffect::Allow, STANDARD_ALLOW_REASON);
        let deny = deadline(PermissionEffect::Deny, STANDARD_DENY_REASON);
        let pending = |published: Option<PermissionReviewDeadline>| {
            let mut controller = controller();
            controller
                .request_permission(
                    &execution,
                    permission.clone(),
                    tool(None),
                    input(),
                    options("Allow".into()),
                    published.map(|deadline| deadline.promise(1_000).unwrap()),
                )
                .unwrap();
            controller
        };
        let allow_option = PermissionOptionId::new("allow").unwrap();
        let settle = |controller: &mut ExecutionController, deadline| {
            controller.settle_deadline(&execution, &permission, &allow_option, deadline)
        };
        let stale = |controller: &mut ExecutionController| {
            matches!(
                controller.deadline_answered(&execution, &permission),
                Err(AgentError::StalePermission)
            )
        };

        let mut unsettled = pending(Some(allow.clone()));
        assert!(stale(&mut unsettled), "a pending review has no notice");

        let mut answered = pending(Some(allow.clone()));
        settle(&mut answered, allow.clone()).unwrap();
        let event = answered.deadline_answered(&execution, &permission).unwrap();
        assert_eq!(
            event.update(),
            &ExecutionUpdate::DeadlineAnswered {
                id: permission.clone(),
                effect: PermissionEffect::Allow,
                reason: allow.recorded_reason(),
            },
            "the notice repeats the published deadline"
        );
        assert!(stale(&mut answered), "the notice is made once");

        let mut explicit = pending(Some(allow.clone()));
        explicit.answer_permission(answer("permission")).unwrap();
        assert!(
            stale(&mut explicit),
            "a person's answer has no deadline notice"
        );

        // A caller's answer cannot claim the deadline decided, even the
        // published one; a settlement that is not the published deadline, or
        // whose outcome is not the selected option's, is refused too. Neither
        // consumes anything: the review stays pending and has no notice.
        let mut claimed = pending(Some(allow.clone()));
        assert!(matches!(
            claimed.answer_permission(PermissionAnswer {
                attribution: ApprovalAttribution::deadline(&permission, allow.clone()).unwrap(),
                ..answer("permission")
            }),
            Err(AgentError::InvalidInput(_))
        ));
        assert!(claimed.answer_permission(answer("permission")).is_ok());
        assert!(stale(&mut claimed), "a refused claim makes no notice");
        let refused = |published: Option<PermissionReviewDeadline>, claimed| {
            let mut controller = pending(published);
            let refused = matches!(
                settle(&mut controller, claimed),
                Err(AgentError::InvalidInput(_))
            );
            let still_pending = controller.answer_permission(answer("permission")).is_ok();
            refused && still_pending && stale(&mut controller)
        };
        assert!(
            refused(
                Some(allow.clone()),
                deadline(PermissionEffect::Allow, "a different sentence")
            ),
            "another reason is not the published deadline"
        );
        assert!(
            refused(
                Some(allow.clone()),
                PermissionReviewDeadline::new(
                    PermissionToolClass::ReadOnly,
                    Duration::from_secs(8),
                    PermissionEffect::Allow,
                    STANDARD_ALLOW_REASON,
                )
                .unwrap()
            ),
            "another class and wait is not the published deadline"
        );
        assert!(
            refused(Some(deny.clone()), deny.clone()),
            "a deny deadline cannot select the allow option"
        );
        assert!(
            refused(None, allow.clone()),
            "a review that published no deadline cannot be answered by one"
        );

        let mut timed_out = pending(Some(deny.clone()));
        let _ = timed_out
            .cancel_permission(
                &execution,
                &permission,
                PermissionCancellationReason::deadline_exceeded(),
                CancellationOrigin::Runtime,
            )
            .unwrap()
            .unwrap();
        assert!(
            stale(&mut timed_out),
            "an execution timeout is not the review's deadline"
        );
        // A caller cannot cancel as the deadline, even with its reason and
        // the published deadline; the review stays pending.
        let mut claimed = pending(Some(deny.clone()));
        assert!(matches!(
            claimed.cancel_permission(
                &execution,
                &permission,
                PermissionCancellationReason::deadline_exceeded(),
                CancellationOrigin::Deadline(deny.clone()),
            ),
            Err(AgentError::InvalidInput(_))
        ));
        assert!(matches!(
            claimed.cancel_permissions(
                &execution,
                PermissionCancellationReason::deadline_exceeded(),
                CancellationOrigin::Deadline(deny.clone()),
            ),
            Err(AgentError::InvalidInput(_))
        ));
        assert!(matches!(
            claimed.close(
                PermissionCancellationReason::session_closed(),
                CancellationOrigin::Deadline(deny.clone()),
            ),
            Err(AgentError::InvalidInput(_))
        ));
        assert!(matches!(
            claimed.cancel_deadline(&execution, &permission, allow.clone()),
            Err(AgentError::InvalidInput(_))
        ));
        assert!(claimed.answer_permission(answer("permission")).is_ok());
        assert!(
            stale(&mut claimed),
            "a refused deadline cancel makes no notice"
        );

        let mut cancelled = pending(Some(deny.clone()));
        let record = cancelled
            .cancel_deadline(&execution, &permission, deny.clone())
            .unwrap()
            .unwrap();
        assert_eq!(record.origin(), &CancellationOrigin::Deadline(deny.clone()));
        let event = cancelled
            .deadline_answered(&execution, &permission)
            .unwrap();
        assert_eq!(
            event.update(),
            &ExecutionUpdate::DeadlineAnswered {
                id: permission.clone(),
                effect: PermissionEffect::Deny,
                reason: deny.recorded_reason(),
            }
        );

        let mut withdrawn = pending(Some(deny.clone()));
        let _ = withdrawn
            .cancel_permission(
                &execution,
                &permission,
                PermissionCancellationReason::provider_withdrawal(),
                CancellationOrigin::Provider,
            )
            .unwrap()
            .unwrap();
        assert!(
            stale(&mut withdrawn),
            "a review cancelled for another reason was not settled by its deadline"
        );

        let mut finished = pending(Some(deny));
        let _ = finished
            .cancel_permission(
                &execution,
                &permission,
                PermissionCancellationReason::deadline_exceeded(),
                CancellationOrigin::Runtime,
            )
            .unwrap();
        let _ = finished
            .finish_execution(&execution, Ok(ExecutionOutcome::Completed))
            .unwrap();
        assert!(finished.deadline_settled.is_empty() && finished.published_deadlines.is_empty());
    }

    #[test]
    fn a_hook_answer_with_a_terminal_outcome_is_rejected_and_stays_pending() {
        use crate::application::agent_execution::permissions::HookRefusal;
        let mut controller = controller();
        let execution = controller.active_execution_id().unwrap().clone();
        let deny = PermissionOptions::new(
            vec![PermissionOption::new(
                PermissionOptionId::new("deny").unwrap(),
                String::from("Deny"),
                PermissionDecision::new(PermissionEffect::Deny, PermissionScope::request()),
            )
            .unwrap()],
            &PermissionOfferPolicy::once_only(),
        )
        .unwrap();
        let id = PermissionId::new("permission").unwrap();
        controller
            .request_permission(&execution, id.clone(), tool(None), input(), deny, None)
            .unwrap();
        let hook_answer = |outcome| PermissionAnswer {
            execution_id: execution.clone(),
            id: id.clone(),
            option_id: PermissionOptionId::new("deny").unwrap(),
            attribution: ApprovalAttribution::hook(
                &id,
                HookRefusal::applied(Some(0), "no", outcome),
            )
            .unwrap(),
        };
        for terminal in [RefusalOutcome::EndTurn, RefusalOutcome::CloseSession] {
            assert!(matches!(
                controller.answer_permission(hook_answer(terminal)),
                Err(AgentError::InvalidInput(_))
            ));
        }
        controller
            .answer_permission(hook_answer(RefusalOutcome::Continue))
            .expect("a continue is still an answer");
    }
    #[test]
    fn permission_authority_is_scoped_and_weak() {
        let mut controller = controller();
        let source = controller.permission_authority_source();
        let first = source.read().unwrap().unwrap();
        let permission = PermissionId::new("permission").unwrap();
        assert_eq!(first.session_id(), controller.id());
        assert_eq!(
            first.execution_id(),
            controller.active_execution_id().unwrap()
        );
        assert!(!first.pending(&permission).unwrap());
        request(&mut controller, "permission", "original").unwrap();
        assert!(first.pending(&permission).unwrap());
        controller.answer_permission(answer("permission")).unwrap();
        assert!(!first.pending(&permission).unwrap());
        controller
            .finish_execution(
                &ExecutionId::new("execution").unwrap(),
                Ok(ExecutionOutcome::Completed),
            )
            .unwrap();
        assert_eq!(
            first.pending(&permission),
            Err(PermissionAuthorityError::Unavailable)
        );
        controller
            .begin_execution(ExecutionId::new("next").unwrap())
            .unwrap();
        request(&mut controller, "permission", "new execution").unwrap();
        let next = source.read().unwrap().unwrap();
        assert!(next.pending(&permission).unwrap());
        assert_eq!(
            first.pending(&permission),
            Err(PermissionAuthorityError::Unavailable)
        );
        controller
            .cancel_permissions(
                &ExecutionId::new("next").unwrap(),
                PermissionCancellationReason::provider_withdrawal(),
                CancellationOrigin::Provider,
            )
            .unwrap();
        assert!(!next.pending(&permission).unwrap());
        drop(controller);
        assert_eq!(
            next.pending(&permission),
            Err(PermissionAuthorityError::Unavailable)
        );
    }

    #[test]
    fn rejected_reviews_preserve_the_observation_and_exact_pending_input() {
        let mut controller = controller();
        request(&mut controller, "permission", "original").unwrap();
        assert!(request(&mut controller, "permission", "unaccepted").is_err());
        assert_eq!(
            controller
                .session
                .tool(&ToolCallId::new("tool").unwrap())
                .unwrap()
                .observation()
                .title()
                .as_deref(),
            Some("original")
        );
        let resolution = controller.answer_permission(answer("permission")).unwrap();
        assert_eq!(resolution.input(), &input());
        assert_eq!(controller.retained_review_bytes, 0);
        assert!(request(&mut controller, "permission", "replayed").is_err());
        assert_eq!(
            controller
                .close(
                    PermissionCancellationReason::session_closed(),
                    CancellationOrigin::Runtime
                )
                .unwrap()
                .len(),
            1
        );
        assert!(request(&mut controller, "other", "after close").is_err());
        assert_eq!(
            controller
                .session
                .tool(&ToolCallId::new("tool").unwrap())
                .unwrap()
                .observation()
                .title()
                .as_deref(),
            Some("original")
        );
    }
    #[test]
    fn permission_event_retains_snapshot_after_later_tool_updates() {
        let mut controller = controller();
        let event = request(&mut controller, "permission", "reviewed title").unwrap();
        controller
            .tool_event(
                &ExecutionId::new("execution").unwrap(),
                tool(Some("later title".into())),
            )
            .unwrap();
        let ExecutionUpdate::PermissionRequested {
            id,
            tool_id,
            observation,
            input: reviewed_input,
            ..
        } = event.update()
        else {
            panic!("expected permission review");
        };
        assert_eq!(event.execution_id().as_str(), "execution");
        assert_eq!(id.as_str(), "permission");
        assert_eq!(tool_id.as_str(), "tool");
        assert_eq!(observation.title().as_deref(), Some("reviewed title"));
        assert_eq!(reviewed_input, &input());
        assert_eq!(
            controller
                .session
                .tool(tool_id)
                .unwrap()
                .observation()
                .title()
                .as_deref(),
            Some("later title")
        );
        let resolution = controller.answer_permission(answer("permission")).unwrap();
        assert_eq!(resolution.request().tool_id(), tool_id);
        assert_eq!(resolution.input(), reviewed_input);
    }
    #[test]
    fn long_tool_identity_copies_count_at_admission_replacement_and_review_release() {
        let id = ToolCallId::new("t".repeat(256)).unwrap();
        let update = |title| ToolCallUpdate::new(id.clone(), title, None, None, None, None);
        let mut controller = controller();
        let identity_bytes = "execution".len() + 2 * id.as_str().len();
        assert!(controller
            .tool_event(
                &ExecutionId::new("execution").unwrap(),
                update(Some("x".repeat(MAX_RETAINED_BYTES - identity_bytes + 1)))
            )
            .is_err());
        assert_eq!(controller.session.tool_count(), 0);
        controller
            .tool_event(
                &ExecutionId::new("execution").unwrap(),
                update(Some("x".repeat(MAX_RETAINED_BYTES - identity_bytes))),
            )
            .unwrap();
        assert_eq!(controller.retained_tool_bytes, MAX_RETAINED_BYTES);
        controller
            .tool_event(&ExecutionId::new("execution").unwrap(), update(None))
            .unwrap();
        assert_eq!(controller.retained_tool_bytes, MAX_RETAINED_BYTES);
        assert!(controller
            .tool_event(
                &ExecutionId::new("execution").unwrap(),
                update(Some("x".repeat(MAX_RETAINED_BYTES - identity_bytes + 1)))
            )
            .is_err());
        assert_eq!(controller.retained_tool_bytes, MAX_RETAINED_BYTES);
        controller
            .tool_event(
                &ExecutionId::new("execution").unwrap(),
                update(Some(String::new())),
            )
            .unwrap();
        assert_eq!(controller.retained_tool_bytes, identity_bytes);
        for release in 0..5 {
            let target = controller.active_execution_id().unwrap().clone();
            let choices = options("Allow".into());
            let permission = PermissionId::new(format!("review-{release}")).unwrap();
            let overhead = id.as_str().len()
                + 3 * permission.as_str().len()
                + controller
                    .session
                    .active_execution()
                    .unwrap()
                    .as_str()
                    .len()
                + choices.payload_bytes()
                + "Read".len();
            let review = |extra| ToolReviewInput {
                name: "Read".into(),
                arguments_json: "x".repeat(MAX_RETAINED_BYTES - overhead + extra),
            };
            assert!(controller
                .request_permission(
                    &target,
                    permission.clone(),
                    update(None),
                    review(1),
                    choices.clone(),
                    None,
                )
                .is_err());
            assert_eq!(controller.retained_review_bytes, 0);
            controller
                .request_permission(
                    &target,
                    permission.clone(),
                    update(None),
                    review(0),
                    choices,
                    None,
                )
                .unwrap();
            assert_eq!(controller.retained_review_bytes, MAX_RETAINED_BYTES);
            match release {
                0 => {
                    controller
                        .answer_permission(answer(permission.as_str()))
                        .unwrap();
                }
                1 => {
                    assert!(controller
                        .cancel_permission(
                            &target,
                            &permission,
                            PermissionCancellationReason::provider_withdrawal(),
                            CancellationOrigin::Provider
                        )
                        .unwrap()
                        .is_some());
                }
                2 => {
                    assert_eq!(
                        controller
                            .cancel_permissions(
                                &target,
                                PermissionCancellationReason::deadline_exceeded(),
                                CancellationOrigin::Runtime
                            )
                            .unwrap()
                            .len(),
                        1
                    );
                }
                3 => {
                    let _evidence = controller
                        .finish_execution(&target, Ok(ExecutionOutcome::Completed))
                        .unwrap();
                    assert_eq!(controller.retained_tool_bytes, 0);
                    controller
                        .begin_execution(ExecutionId::new("next-execution").unwrap())
                        .unwrap();
                }
                _ => {
                    let records = controller
                        .close(
                            PermissionCancellationReason::session_closed(),
                            CancellationOrigin::Runtime,
                        )
                        .unwrap();
                    assert_eq!(records.len(), 2);
                }
            }
            assert_eq!(controller.retained_review_bytes, 0);
        }
    }

    #[test]
    fn tool_budget_retains_sparse_fields_and_releases_replaced_or_finished_content() {
        let mut controller = controller();
        let identity_bytes = controller
            .session
            .tool_payload_bytes_after(controller.session.active_execution().unwrap(), &tool(None))
            .unwrap();
        controller
            .tool_event(
                &ExecutionId::new("execution").unwrap(),
                tool(Some("x".repeat(MAX_RETAINED_BYTES - identity_bytes))),
            )
            .unwrap();
        controller
            .tool_event(&ExecutionId::new("execution").unwrap(), tool(None))
            .unwrap();
        assert_eq!(controller.retained_tool_bytes, MAX_RETAINED_BYTES);
        let other = ToolCallUpdate::new(
            ToolCallId::new("other").unwrap(),
            Some("y".into()),
            None,
            None,
            None,
            None,
        );
        assert!(controller
            .tool_event(&ExecutionId::new("execution").unwrap(), other.clone())
            .is_err());
        assert_eq!(controller.session.tool_count(), 1);
        controller
            .tool_event(
                &ExecutionId::new("execution").unwrap(),
                tool(Some(String::new())),
            )
            .unwrap();
        controller
            .tool_event(&ExecutionId::new("execution").unwrap(), other)
            .unwrap();
        controller
            .finish_execution(
                &ExecutionId::new("execution").unwrap(),
                Ok(ExecutionOutcome::Completed),
            )
            .unwrap();
        assert_eq!(controller.retained_tool_bytes, 0);
        assert_eq!(controller.session.tool_count(), 0);
    }
    #[test]
    fn oversized_options_are_rejected_before_tool_or_permission_mutation() {
        let mut controller = controller();
        assert!(controller
            .request_permission(
                &ExecutionId::new("execution").unwrap(),
                PermissionId::new("p").unwrap(),
                tool(Some("unaccepted".into())),
                input(),
                options("x".repeat(MAX_RETAINED_BYTES)),
                None,
            )
            .is_err());
        assert_eq!(controller.session.tool_count(), 0);
        assert_eq!(controller.session.permission_count(), 0);
        assert_eq!(controller.retained_review_bytes, 0);
    }
    #[test]
    fn cancellation_releases_only_its_review_and_execution_caps_include_resolved_ids() {
        let mut controller = controller();
        request(&mut controller, "first", "tool").unwrap();
        request(&mut controller, "second", "tool").unwrap();
        let both = controller.retained_review_bytes;
        assert!(matches!(controller
                .cancel_permission(
                    &ExecutionId::new("execution").unwrap(),
                    &PermissionId::new("first").unwrap(),
                    PermissionCancellationReason::provider_withdrawal(),
                    CancellationOrigin::Provider
                )
                .unwrap()
                .unwrap()
                .request()
                .state(),
            PermissionStateView::Cancelled { reason } if reason.view() == PermissionCancellationReasonView::ProviderWithdrawal
        ));
        assert!(controller.retained_review_bytes < both);
        assert_eq!(
            controller.answer_permission(answer("first")),
            Err(AgentError::StalePermission)
        );
        controller.answer_permission(answer("second")).unwrap();
        assert_eq!(controller.retained_review_bytes, 0);
        for index in 2..MAX_EXECUTION_PERMISSIONS {
            let id = index.to_string();
            request(&mut controller, &id, "tool").unwrap();
            controller.answer_permission(answer(&id)).unwrap();
        }
        assert!(request(&mut controller, "over-limit", "tool").is_err());
        controller
            .finish_execution(
                &ExecutionId::new("execution").unwrap(),
                Ok(ExecutionOutcome::Completed),
            )
            .unwrap();
        controller
            .begin_execution(ExecutionId::new("next-execution").unwrap())
            .unwrap();
        request(&mut controller, "first", "tool").unwrap();
        assert_eq!(
            controller
                .cancel_permissions(
                    &ExecutionId::new("next-execution").unwrap(),
                    PermissionCancellationReason::deadline_exceeded(),
                    CancellationOrigin::Runtime
                )
                .unwrap()
                .len(),
            1
        );
        assert_eq!(controller.retained_review_bytes, 0);
    }
    #[test]
    fn controller_rejects_reused_execution_identity_without_releasing_new_run() {
        let mut controller = controller();
        controller
            .finish_execution(
                &ExecutionId::new("execution").unwrap(),
                Ok(ExecutionOutcome::Completed),
            )
            .unwrap();
        assert!(matches!(
            controller.begin_execution(ExecutionId::new("execution").unwrap()),
            Err(AgentError::InvalidInput(_))
        ));
        assert_eq!(controller.session.active_execution(), None);
        controller
            .begin_execution(ExecutionId::new("next-execution").unwrap())
            .unwrap();
        request(&mut controller, "review", "tool").unwrap();
        let bytes = controller.retained_review_bytes;
        assert!(matches!(
            controller.finish_execution(
                &ExecutionId::new("next-execution").unwrap(),
                Err(PermissionCancellationReason::session_closed())
            ),
            Err(AgentError::InvalidInput(_))
        ));
        assert_eq!(
            controller.session.active_execution().unwrap().as_str(),
            "next-execution"
        );
        assert!(!controller.session.is_closed());
        assert_eq!(controller.retained_review_bytes, bytes);
        assert_eq!(
            controller
                .close(
                    PermissionCancellationReason::session_closed(),
                    CancellationOrigin::Runtime
                )
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            controller
                .finish_execution(
                    &ExecutionId::new("next-execution").unwrap(),
                    Err(PermissionCancellationReason::session_closed())
                )
                .unwrap()
                .len(),
            1
        );
    }
    #[test]
    fn stale_execution_close_preserves_later_run_and_review_accounting() {
        let mut controller = controller();
        controller
            .finish_execution(
                &ExecutionId::new("execution").unwrap(),
                Ok(ExecutionOutcome::Completed),
            )
            .unwrap();
        let next = ExecutionId::new("next").unwrap();
        controller.begin_execution(next.clone()).unwrap();
        request(&mut controller, "permission", "original").unwrap();
        let bytes = controller.retained_review_bytes;
        assert!(controller
            .finish_execution(
                &ExecutionId::new("execution").unwrap(),
                Ok(ExecutionOutcome::Completed)
            )
            .is_err());
        assert_eq!(controller.active_execution_id(), Some(&next));
        assert_eq!(controller.retained_review_bytes, bytes);
        assert!(controller
            .close_execution(
                &ExecutionId::new("execution").unwrap(),
                PermissionCancellationReason::execution_failed(),
                CancellationOrigin::Runtime,
            )
            .is_err());
        assert_eq!(controller.active_execution_id(), Some(&next));
        assert!(!controller.session.is_closed());
        assert_eq!(controller.retained_review_bytes, bytes);
        assert!(controller
            .close(
                PermissionCancellationReason::execution_failed(),
                CancellationOrigin::Runtime
            )
            .is_err());
        let records = controller
            .close_execution(
                &next,
                PermissionCancellationReason::execution_failed(),
                CancellationOrigin::Runtime,
            )
            .unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(controller.retained_review_bytes, 0);
    }

    #[test]
    fn only_the_hook_records_a_hook_refusal_withdrawal() {
        use crate::domain::agent_execution::permissions::RefusalOutcome;
        let execution = ExecutionId::new("execution").unwrap();
        let review = PermissionId::new("permission").unwrap();
        let session = ExecutionSessionId::new("session").unwrap();
        let reason = || {
            PermissionCancellationReason::custom(
                CustomPermissionCancellationReason::hook_refusal(
                    &session,
                    &execution,
                    &review,
                    RefusalOutcome::EndTurn,
                    "hook refused",
                )
                .unwrap(),
            )
        };
        let person =
            CancellationOrigin::Client(ActionContext::new("person", "app", "permission").unwrap());
        let hook = CancellationOrigin::Client(
            ActionContext::new("permission-review-hook", "0", "permission").unwrap(),
        );
        let mut controller = controller();
        request(&mut controller, "permission", "original").unwrap();
        for forged in [
            person.clone(),
            CancellationOrigin::Client(
                ActionContext::new("permission-review-hook", "00", "permission").unwrap(),
            ),
            CancellationOrigin::Client(
                ActionContext::new("permission-review-hook", "0", "other").unwrap(),
            ),
        ] {
            assert!(matches!(
                controller.cancel_permission(&execution, &review, reason(), forged),
                Err(AgentError::InvalidInput(_))
            ));
        }
        let withdrawn = controller
            .cancel_permission(&execution, &review, reason(), hook.clone())
            .unwrap()
            .expect("the hook withdraws its own review");
        // Saved evidence is held to the same rule when it is loaded again.
        let restore = |origin| {
            let mut saved = PermissionRequest::new(
                review.clone(),
                execution.clone(),
                ToolCallId::new("tool").unwrap(),
                options("Allow".into()),
            );
            saved.cancel(reason()).unwrap();
            PermissionCancellation::from_record(
                withdrawn.session_id().clone(),
                saved,
                withdrawn.input().clone(),
                origin,
            )
        };
        assert!(matches!(restore(person), Err(AgentError::InvalidInput(_))));
        assert_eq!(restore(hook).unwrap(), withdrawn);
    }
}
