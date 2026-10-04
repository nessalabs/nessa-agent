use super::view::{
    ConversationAnswerOption, ConversationApprovalModeChangeView, ConversationAsked,
    ConversationCapabilities, ConversationLifecycle, ConversationLifecyclePhase,
    ConversationMcpTool, ConversationMessage, ConversationMessageStatus, ConversationPart,
    ConversationPending, ConversationPendingMode, ConversationPermission,
    ConversationPermissionOption, ConversationPermissionOptionEffect, ConversationPermissionOrigin,
    ConversationQuestion, ConversationTool, ConversationTranscriptState, ConversationView,
    MAX_STRUCTURED_CONTENT_BYTES,
};
use super::{McpToolUis, NoMcpToolUis};
use crate::conversation::domain::ConversationId;
use nessa_sdk::application::agent_execution::{
    agents::AgentError,
    executions::{ExecutionEvent, ExecutionUpdate, SubmissionMode},
    sessions::{CommittedSession, CommittedStatus, InvocationRecord, SessionSnapshot},
};
use nessa_sdk::domain::agent_execution::tools::McpTool;
use nessa_sdk::domain::agent_execution::{
    executions::{ExecutionId, ExecutionOutcome, InvocationStage, MessageKind},
    permissions::{PermissionScope, ReviewDecline, ReviewDeclineReason, ReviewDeclineStage},
    questions::{AnswerShape, MAX_OPEN_QUESTIONS},
    tools::{ToolContentView, ToolKind, ToolStatus},
};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use uuid::Uuid;

const MAX_MESSAGES: usize = 24;
pub(super) const MAX_TEXT: usize = 8192;
const MAX_TOOLS: usize = 16;
const MAX_PERMISSIONS: usize = 16;
// Application transcript body; product catalog enrichment is serialized afterward.
pub(super) const MAX_VIEW_BYTES: usize = 60_000;
/// The largest single review the view offers, encoded. One review may not take
/// most of the view's budget from everything else.
const MAX_REVIEW_BYTES: usize = 16_000;
const REQUIRED_WORK_FAILURE: &str = "The turn could not complete all required work.";
const PROVIDER_FAILURE_PREFIX: &str = "The agent provider reported an error: ";

fn record_status(record: &InvocationRecord, restoring: bool) -> ConversationMessageStatus {
    match &record.result {
        Some(Ok(value)) => outcome(*value),
        Some(Err(_))
            if record
                .scheduling
                .last()
                .is_some_and(|event| event.stage == InvocationStage::Cancelled) =>
        {
            ConversationMessageStatus::Cancelled
        }
        Some(Err(_)) => ConversationMessageStatus::Failed,
        None => match record.scheduling.last().map(|event| event.stage) {
            Some(InvocationStage::Injected) => ConversationMessageStatus::Injected,
            Some(InvocationStage::Cancelled) => ConversationMessageStatus::Cancelled,
            _ if restoring => ConversationMessageStatus::Unresolved,
            _ => ConversationMessageStatus::Running,
        },
    }
}

pub(super) struct Projection {
    pub view: ConversationView,
    epoch: Uuid,
    revision: u64,
    committed_position: Option<(String, u64, u64, u64)>,
    resolved_permissions: HashSet<(String, String)>,
    /// Asks that have stopped waiting, so a replayed ask does not reopen one.
    answered_questions: HashSet<(String, String)>,
    terminal_executions: HashSet<String>,
    /// The calls that have a part in their turn, by execution and clipped tool
    /// id, so a call's later updates find it without scanning the turn's
    /// parts. A call goes in only when its part is pushed, and an execution
    /// leaves when its parts do: cleared for a replayed record, or its message
    /// let go.
    tool_parts: HashMap<String, HashSet<String>>,
    /// Exact local execution identity supplied by Agent for this replacement.
    /// It supplies action locality, never semantic completion or freshness.
    live_here: HashSet<String>,
    /// Where an MCP call's UI is looked up when the view is read.
    tool_uis: Arc<dyn McpToolUis>,
}
pub(super) fn clipped(value: &str, bytes: usize) -> String {
    let mut end = value.len().min(bytes);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}
fn outcome(value: ExecutionOutcome) -> ConversationMessageStatus {
    match value {
        ExecutionOutcome::Completed => ConversationMessageStatus::Completed,
        ExecutionOutcome::Cancelled => ConversationMessageStatus::Cancelled,
        ExecutionOutcome::OutputLimit
        | ExecutionOutcome::RequestLimit
        | ExecutionOutcome::Refused => ConversationMessageStatus::Failed,
    }
}
fn decline_notice(decline: &ReviewDecline, delivery: ReviewDeclineStage) -> String {
    let target = decline
        .declared()
        .map(|tool| format!(" labeled {tool}"))
        .unwrap_or_default();
    let reason = match decline.reason() {
        ReviewDeclineReason::ToolNotReviewable => "that tool is not reviewable here",
        ReviewDeclineReason::UnreadableRequest => "the request could not be read safely",
        ReviewDeclineReason::UnusableOptions => "the request offered no usable choices",
    };
    let delivery = match delivery {
        ReviewDeclineStage::Selected => "Nessa has not confirmed writing the refusal to the agent.",
        ReviewDeclineStage::WriteConfirmed => "",
        ReviewDeclineStage::WriteUnconfirmed => {
            "Nessa could not confirm writing the refusal to the agent."
        }
        ReviewDeclineStage::WriteNotAttempted => {
            "Nessa did not attempt to write the refusal to the agent."
        }
    };
    format!("Nessa declined a tool review{target} because {reason}. {delivery}")
        .trim_end()
        .to_owned()
}
fn failure_notice(record: &InvocationRecord) -> Option<String> {
    let final_error = record.result.as_ref()?.as_ref().err()?;
    let report = record.provider_report.as_ref();
    let provider_error = report
        .and_then(|report| report.provider_result())
        .and_then(|result| result.as_ref().err());
    let Some(AgentError::Provider {
        diagnostic: Some(diagnostic),
        ..
    }) = provider_error
    else {
        return Some(REQUIRED_WORK_FAILURE.into());
    };
    let diagnostic = diagnostic.as_str().trim();
    if diagnostic.is_empty() {
        return Some(REQUIRED_WORK_FAILURE.into());
    }
    let mut notice = format!("{PROVIDER_FAILURE_PREFIX}{diagnostic}");
    let report_error = report.and_then(|report| report.clone().into_result().err());
    if report_error.as_ref() != provider_error || provider_error != Some(final_error) {
        if !matches!(notice.chars().last(), Some('.' | '!' | '?')) {
            notice.push('.');
        }
        notice.push(' ');
        notice.push_str(REQUIRED_WORK_FAILURE);
    }
    Some(notice)
}
/// Bound a retained snapshot through the same display owner as gateway reads.
///
/// The cache supplies the snapshot and status from one validated fold for `id`.
/// This mapping performs no semantic validation, source contact or dispatch.
/// The injected revision identity is transient presentation evidence; all action
/// capabilities are disabled. Display truncation does not change SDK completeness.
pub(crate) fn retained_view(
    id: &ConversationId,
    snapshot: Option<&SessionSnapshot>,
    status: CommittedStatus,
    revision: Uuid,
) -> ConversationView {
    let mut projection = Projection::from_snapshot_with_epoch(
        id.to_string(),
        ConversationCapabilities::read_only(),
        snapshot,
        HashSet::new(),
        revision,
    );
    projection.transcript_state(status.view_state().into());
    bound_view(projection.read())
}

impl Projection {
    pub fn new(
        id: String,
        capabilities: ConversationCapabilities,
        snapshot: Option<&SessionSnapshot>,
    ) -> Self {
        Self::from_snapshot(id, capabilities, snapshot, HashSet::new())
    }

    fn from_snapshot(
        id: String,
        capabilities: ConversationCapabilities,
        snapshot: Option<&SessionSnapshot>,
        live_here: HashSet<String>,
    ) -> Self {
        Self::from_snapshot_with_epoch(id, capabilities, snapshot, live_here, Uuid::new_v4())
    }

    fn from_snapshot_with_epoch(
        id: String,
        capabilities: ConversationCapabilities,
        snapshot: Option<&SessionSnapshot>,
        live_here: HashSet<String>,
        epoch: Uuid,
    ) -> Self {
        let mut projection = Self {
            epoch,
            revision: 0,
            committed_position: None,
            resolved_permissions: HashSet::new(),
            answered_questions: HashSet::new(),
            terminal_executions: HashSet::new(),
            tool_parts: HashMap::new(),
            live_here: live_here.clone(),
            tool_uis: Arc::new(NoMcpToolUis),
            view: ConversationView {
                selection: None,
                approval_mode_change: None,
                conversation_id: id,
                revision: String::new(),
                messages: Vec::new(),
                pending: Vec::new(),
                permissions: Vec::new(),
                questions: Vec::new(),
                tools: Vec::new(),
                runtime: None,
                capabilities,
                lifecycle: ConversationLifecycle {
                    phase: ConversationLifecyclePhase::Absent,
                    failure: None,
                    evidence_failure: None,
                },
                truncated: false,
                queue_complete: true,
                transcript_state: ConversationTranscriptState::NotLoaded,
                interaction_view_error: None,
                // Not the projection's: the service fills it from the summary.
                title: None,
            },
        };
        if let Some(snapshot) = snapshot {
            projection.view.truncated = snapshot.invocations.len() > MAX_MESSAGES;
            for record in
                &snapshot.invocations[..snapshot.invocations.len().saturating_sub(MAX_MESSAGES)]
            {
                if live_here.contains(record.request.execution_id.as_str()) {
                    projection.active_interactions(record);
                }
            }
            for record in snapshot
                .invocations
                .iter()
                .rev()
                .take(MAX_MESSAGES)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
            {
                projection.record(
                    record,
                    !live_here.contains(record.request.execution_id.as_str()),
                );
            }
        }
        projection.bump();
        projection
    }

    /// Replace transcript fields from the SDK's committed snapshot. Live
    /// broadcast callbacks may prompt this read, but their provisional text or
    /// terminal status cannot become a replacement view. The SDK owns the
    /// lifecycle fold that produced `snapshot`; this method only bounds display.
    pub fn replace_committed(
        &mut self,
        committed: &CommittedSession,
        order: &[ExecutionId],
        active: Option<&ExecutionId>,
    ) -> bool {
        let incarnation = committed.incarnation();
        let position = committed.position();
        let downloaded = committed.downloaded();
        let observed_head = committed.observed_head();
        let snapshot = committed.snapshot();
        if let Some((current_incarnation, current_position, current_downloaded, current_head)) =
            &self.committed_position
        {
            if current_incarnation != incarnation
                || position < *current_position
                || downloaded < *current_downloaded
                || observed_head < *current_head
            {
                return false;
            }
        }
        let active = active
            .map(|id| id.as_str().to_owned())
            .into_iter()
            .collect();
        let mut next = Self::from_snapshot(
            self.view.conversation_id.clone(),
            self.view.capabilities.clone(),
            snapshot,
            active,
        )
        .with_tool_uis(self.tool_uis.clone());
        next.epoch = self.epoch;
        next.revision = self.revision;
        next.committed_position = Some((incarnation.into(), position, downloaded, observed_head));
        next.view.revision = self.view.revision.clone();
        next.view.transcript_state = self.view.transcript_state;
        next.view.selection = self.view.selection.clone();
        next.view.runtime = self.view.runtime.clone();
        next.view.lifecycle = self.view.lifecycle.clone();
        next.view.approval_mode_change = self.view.approval_mode_change.clone();
        next.view.title = self.view.title.clone();
        if let Some(snapshot) = snapshot {
            for id in order {
                let Some(record) = snapshot
                    .invocations
                    .iter()
                    .find(|record| record.request.execution_id == *id)
                else {
                    next.view.queue_complete = false;
                    continue;
                };
                let text = record.request.user_message.text_str();
                let mode = match record.submission {
                    SubmissionMode::Steering | SubmissionMode::BoundarySteering => {
                        ConversationPendingMode::Steering
                    }
                    SubmissionMode::Immediate | SubmissionMode::Queued => {
                        ConversationPendingMode::Queued
                    }
                };
                if next.view.pending.len() < 64 {
                    next.view.pending.push(ConversationPending {
                        execution_id: id.as_str().into(),
                        text: clipped(text, MAX_TEXT),
                        attachments: record
                            .request
                            .user_message
                            .images()
                            .iter()
                            .map(Into::into)
                            .collect(),
                        files: record
                            .request
                            .user_message
                            .files()
                            .iter()
                            .map(Into::into)
                            .collect(),
                        mode,
                    });
                } else {
                    next.view.queue_complete = false;
                    next.view.truncated = true;
                }
                if let Some(message) = next
                    .view
                    .messages
                    .iter_mut()
                    .find(|message| message.execution_id == id.as_str())
                {
                    message.status = ConversationMessageStatus::Queued;
                }
            }
        }
        let changed = serde_json::to_vec(&next.view).ok() != serde_json::to_vec(&self.view).ok();
        if changed {
            next.bump();
        }
        *self = next;
        true
    }
    pub fn transcript_state(&mut self, state: ConversationTranscriptState) {
        if self.view.transcript_state != state {
            self.view.transcript_state = state;
            self.bump();
        }
        if !matches!(
            state,
            ConversationTranscriptState::Complete | ConversationTranscriptState::CompleteEmpty
        ) {
            self.view.queue_complete = false;
            self.view.capabilities.queue = false;
            self.view.capabilities.steer = false;
            self.view.capabilities.permissions = false;
            self.view.permissions.clear();
            self.view.questions.clear();
            self.view.pending.clear();
        }
    }
    /// Look an MCP call's UI up in `tool_uis` when the view is read. A
    /// projection without one shows no UI.
    pub fn with_tool_uis(mut self, tool_uis: Arc<dyn McpToolUis>) -> Self {
        self.tool_uis = tool_uis;
        self
    }
    fn bump(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.view.revision = format!("{}:{}", self.epoch, self.revision);
    }
    pub fn lifecycle(&mut self, lifecycle: ConversationLifecycle) {
        if self.view.lifecycle != lifecycle {
            self.view.lifecycle = lifecycle;
            self.bump();
        }
    }
    /// Replace the coherent capability snapshot and revise the view only when it changed.
    pub fn capabilities(&mut self, capabilities: ConversationCapabilities) {
        let mut capabilities = capabilities;
        if !matches!(
            self.view.transcript_state,
            ConversationTranscriptState::Complete | ConversationTranscriptState::CompleteEmpty
        ) {
            capabilities.queue = false;
            capabilities.steer = false;
            capabilities.permissions = false;
        }
        if self.view.capabilities != capabilities {
            self.view.capabilities = capabilities;
            self.bump();
        }
    }
    fn ensure_message(&mut self, id: &str) -> usize {
        if let Some(index) = self
            .view
            .messages
            .iter()
            .position(|message| message.execution_id == id)
        {
            return index;
        }
        if self.view.messages.len() == MAX_MESSAGES {
            let evicted = self.view.messages.remove(0);
            self.tool_parts.remove(&evicted.execution_id);
            self.view.truncated = true;
        }
        self.view.messages.push(ConversationMessage {
            parts: Vec::new(),
            steering_offset: None,
            event_count: 0,
            execution_id: id.into(),
            user_text: String::new(),
            attachments: Vec::new(),
            files: Vec::new(),
            steering_target: None,
            status: ConversationMessageStatus::Running,
            error: None,
        });
        self.view.messages.len() - 1
    }

    pub fn injected(&mut self, id: &str, target: &str) {
        let index = self.ensure_message(id);
        self.view.messages[index].status = ConversationMessageStatus::Injected;
        self.view.messages[index].steering_target = Some(target.into());
        self.view.pending.retain(|value| value.execution_id != id);
        self.bump();
    }
    pub fn resolved_permission(&mut self, execution: &str, id: &str) {
        self.resolved_permissions
            .insert((execution.to_owned(), id.to_owned()));
        self.view
            .permissions
            .retain(|value| value.execution_id != execution || value.permission_id != id);
        self.bump();
    }

    /// An execution's message has stopped running, so nothing it asked or
    /// wanted reviewed is waiting any more. Removed here, on every path a
    /// status leaves running, rather than only hidden when the view is read:
    /// left in place, a stale entry still counted against the open limits and
    /// crowded out an ask somebody could answer.
    fn stop_waiting(&mut self, execution: &str) {
        self.live_here.remove(execution);
        self.view
            .permissions
            .retain(|permission| permission.execution_id != execution);
        self.view
            .questions
            .retain(|question| question.execution_id != execution);
    }

    /// Put one question into the view, unless it has already been answered or
    /// the execution that asked it is over.
    fn observe_question(&mut self, event: &ExecutionEvent) {
        let ExecutionUpdate::QuestionAsked { id, question } = event.update() else {
            return;
        };
        let execution = event.execution_id().as_str();
        if self
            .answered_questions
            .contains(&(execution.to_owned(), id.as_str().to_owned()))
            || self.terminal_executions.contains(execution)
            || self
                .view
                .questions
                .iter()
                .any(|open| open.question_id == id.as_str() && open.execution_id == execution)
        {
            return;
        }
        if self
            .view
            .questions
            .iter()
            .filter(|open| offered(&self.view, &open.execution_id))
            .count()
            >= MAX_OPEN_QUESTIONS
        {
            self.view.truncated = true;
            self.view.interaction_view_error = Some(
                "Some pending interactions exceed this display limit. Use Stop to cancel them."
                    .into(),
            );
            return;
        }
        let value = ConversationQuestion {
            execution_id: execution.to_owned(),
            question_id: id.as_str().to_owned(),
            message: question.message().to_owned(),
            questions: question
                .questions()
                .iter()
                .map(|asked| ConversationAsked {
                    key: asked.key().to_owned(),
                    prompt: asked.prompt().to_owned(),
                    header: asked.header().map(str::to_owned),
                    multi_select: asked.shape() == AnswerShape::Many,
                    free_text: asked.free_text(),
                    required: asked.required(),
                    options: asked
                        .options()
                        .iter()
                        .map(|option| ConversationAnswerOption {
                            value: option.value().to_owned(),
                            label: option.label().to_owned(),
                            description: option.description().map(str::to_owned),
                        })
                        .collect(),
                })
                .collect(),
        };
        self.view.questions.push(value);
    }
    fn observe_permission(&mut self, event: &ExecutionEvent) {
        let ExecutionUpdate::PermissionRequested {
            id: permission,
            tool_id,
            observation,
            input,
            options,
        } = event.update()
        else {
            return;
        };
        let execution = event.execution_id().as_str();
        if self
            .resolved_permissions
            .contains(&(execution.to_owned(), permission.as_str().to_owned()))
            || self.terminal_executions.contains(execution)
        {
            return;
        }
        if permission.as_str().len() > 256
            || tool_id.as_str().len() > 256
            || options.choices().len() > 64
            || options
                .choices()
                .iter()
                .any(|option| option.id().as_str().len() > 256 || option.label().len() > 2048)
            || input.name.len() > 256
            || input.arguments_json.len() > 32768
            || serde_json::from_str::<serde_json::Value>(&input.arguments_json).is_err()
        {
            self.view.interaction_view_error = Some("The complete tool input cannot be displayed safely; this review has no actionable choices in this view.".into());
            self.view.truncated = true;
            return;
        }
        // An option says what it decides (`effect`), not how far it reaches. Live
        // offers decide one request (`once_only`, and the ACP parser admits no
        // persistent kind); a restored one that reached further would read as
        // one that does not, so its review is not offered here.
        if options
            .choices()
            .iter()
            .any(|option| *option.decision().scope() != PermissionScope::request())
        {
            self.view.interaction_view_error = Some("This review offers a choice beyond this one request, which this view cannot show; this review has no actionable choices in this view.".into());
            self.view.truncated = true;
            return;
        }
        if let Some(tool) = self
            .view
            .tools
            .iter_mut()
            .find(|tool| tool.execution_id == execution && tool.tool_id == tool_id.as_str())
        {
            tool.input = input.arguments_json.clone();
        }
        let value = ConversationPermission {
            execution_id: execution.into(),
            permission_id: permission.as_str().into(),
            tool_id: tool_id.as_str().into(),
            title: clipped(
                observation.title().as_deref().unwrap_or("Tool permission"),
                512,
            ),
            tool_name: input.name.clone(),
            arguments_json: input.arguments_json.clone(),
            origin: ConversationPermissionOrigin::Harness,
            options: options
                .choices()
                .iter()
                .map(|option| ConversationPermissionOption {
                    id: option.id().as_str().into(),
                    label: option.label().into(),
                    effect: ConversationPermissionOptionEffect::from(option.decision().effect()),
                })
                .collect(),
        };
        let size = serde_json::to_vec(&value).map_or(usize::MAX, |bytes| bytes.len());
        let open = self
            .view
            .permissions
            .iter()
            .filter(|known| offered(&self.view, &known.execution_id))
            .count();
        if size > MAX_REVIEW_BYTES || open >= MAX_PERMISSIONS {
            self.view.interaction_view_error = Some("A pending tool review exceeds the display limit; no choices were silently removed.".into());
            self.view.truncated = true;
        } else if !self.view.permissions.iter().any(|known| {
            known.execution_id == execution && known.permission_id == value.permission_id
        }) {
            self.view.permissions.push(value);
        }
    }

    // The exact active record can precede the recent message window. Read only
    // its interactions; its prompt, attachments and output are never copied.
    fn active_interactions(&mut self, record: &InvocationRecord) {
        if record_status(record, false) != ConversationMessageStatus::Running {
            return;
        }
        for event in &record.events {
            self.observe_interaction(event);
        }
    }
    fn observe_interaction(&mut self, event: &ExecutionEvent) {
        let execution = event.execution_id().as_str();
        match event.update() {
            ExecutionUpdate::PermissionRequested { .. } => self.observe_permission(event),
            ExecutionUpdate::QuestionAsked { .. } => self.observe_question(event),
            ExecutionUpdate::PermissionCancelled(cancellation) => {
                self.resolved_permission(execution, cancellation.request().id().as_str());
            }
            ExecutionUpdate::QuestionClosed { id } => {
                self.answered_questions
                    .insert((execution.to_owned(), id.as_str().to_owned()));
                self.view.questions.retain(|open| {
                    open.execution_id != execution || open.question_id != id.as_str()
                });
            }
            ExecutionUpdate::Finished(_) => {
                self.terminal_executions.insert(execution.to_owned());
                self.stop_waiting(execution);
            }
            _ => {}
        }
    }

    /// Render one observation from a validated committed SDK record.
    fn observe(&mut self, event: &ExecutionEvent) {
        let id = event.execution_id().as_str();
        let index = self.ensure_message(id);
        self.view
            .pending
            .retain(|pending| pending.execution_id != id);
        let offset = self.view.messages[index].event_count;
        self.view.messages[index].event_count += 1;
        let retained_text = self.view.messages[index]
            .parts
            .iter()
            .map(|part| part.text.len())
            .sum::<usize>();
        let available = (MAX_TEXT * 2).saturating_sub(retained_text);
        if matches!(event.update(), ExecutionUpdate::Message(chunk) if chunk.as_str().len() > available)
        {
            self.view.truncated = true;
        }
        let part = match event.update() {
            ExecutionUpdate::Message(chunk) => Some(ConversationPart {
                message_id: chunk.message_id().map(str::to_owned),
                offset,
                kind: if chunk.kind() == MessageKind::Text {
                    "text"
                } else {
                    "thought"
                }
                .into(),
                text: clipped(chunk.as_str(), available),
                tool_id: String::new(),
                notice_id: String::new(),
            }),
            // One part per tool call, where it was first seen: a call's later
            // updates change its `tools` entry, not the turn's parts.
            ExecutionUpdate::Tool(update) => {
                let tool_id = clipped(update.id().as_str(), 256);
                let seen = self
                    .tool_parts
                    .get(id)
                    .is_some_and(|calls| calls.contains(&tool_id));
                (!seen).then(|| ConversationPart {
                    message_id: None,
                    offset,
                    kind: "tool".into(),
                    text: String::new(),
                    tool_id,
                    notice_id: String::new(),
                })
            }
            ExecutionUpdate::ReviewDeclined(observation) => {
                let notice_id = observation.id().as_str();
                let text = decline_notice(observation.decline(), observation.stage());
                if let Some(position) = self.view.messages[index]
                    .parts
                    .iter()
                    .position(|part| part.kind == "local_notice" && part.notice_id == notice_id)
                {
                    let previous = self.view.messages[index].parts[position].text.len();
                    let budget =
                        (MAX_TEXT * 2).saturating_sub(retained_text.saturating_sub(previous));
                    let bounded = clipped(&text, budget);
                    if bounded.len() != text.len() {
                        self.view.truncated = true;
                    }
                    self.view.messages[index].parts[position].text = bounded;
                    None
                } else {
                    let bounded = clipped(&text, available);
                    if bounded.len() != text.len() {
                        self.view.truncated = true;
                    }
                    Some(ConversationPart {
                        message_id: None,
                        offset,
                        kind: "local_notice".into(),
                        text: bounded,
                        tool_id: String::new(),
                        notice_id: notice_id.into(),
                    })
                }
            }
            _ => None,
        };
        if let Some(part) = part {
            let message = &mut self.view.messages[index];
            if message.parts.len() < 512
                && message
                    .parts
                    .iter()
                    .map(|part| part.text.len())
                    .sum::<usize>()
                    + part.text.len()
                    <= MAX_TEXT * 2
            {
                if part.kind == "tool" {
                    self.tool_parts
                        .entry(id.to_owned())
                        .or_default()
                        .insert(part.tool_id.clone());
                }
                message.parts.push(part);
            } else {
                self.view.truncated = true;
            }
        }
        self.observe_interaction(event);
        match event.update() {
            ExecutionUpdate::Message(_) => {
                self.view.messages[index].status = ConversationMessageStatus::Running;
            }
            ExecutionUpdate::Finished(result) => {
                self.view.messages[index].status = outcome(*result);
            }
            ExecutionUpdate::QuestionAsked { .. } | ExecutionUpdate::PermissionRequested { .. } => {
                self.view.messages[index].status = ConversationMessageStatus::Running;
            }
            ExecutionUpdate::PermissionCancelled(_) | ExecutionUpdate::QuestionClosed { .. } => {}
            ExecutionUpdate::ReviewDeclined(_) => {
                self.view.messages[index].status = ConversationMessageStatus::Running;
            }
            ExecutionUpdate::Tool(update) => {
                self.view.messages[index].status = ConversationMessageStatus::Running;
                let position = self.view.tools.iter().position(|tool| {
                    tool.execution_id == id && tool.tool_id == update.id().as_str()
                });
                let tool = if let Some(position) = position {
                    &mut self.view.tools[position]
                } else {
                    if self.view.tools.len() == MAX_TOOLS {
                        self.view.tools.remove(0);
                        self.view.truncated = true;
                    }
                    self.view.tools.push(ConversationTool {
                        input: String::new(),
                        details: String::new(),
                        execution_id: id.into(),
                        tool_id: clipped(update.id().as_str(), 256),
                        title: "Tool".into(),
                        kind: String::new(),
                        status: "pending".into(),
                        mcp: None,
                        structured_content: None,
                    });
                    self.view.tools.last_mut().unwrap()
                };
                if let Some(title) = update.title() {
                    tool.title = clipped(title, 512);
                }
                // What a call does is the provider's own word for it. A panel
                // cannot read it off the title — "grep -l" and "Find `**/*`"
                // are both searches and neither says so.
                if let Some(kind) = update.kind() {
                    tool.kind = match kind {
                        ToolKind::Read => "read",
                        ToolKind::Edit => "edit",
                        ToolKind::Search => "search",
                        ToolKind::Fetch => "fetch",
                        ToolKind::Execute => "execute",
                        ToolKind::Think => "think",
                        ToolKind::Delete => "delete",
                        ToolKind::Move => "move",
                        ToolKind::SwitchMode => "switch_mode",
                        ToolKind::Other => "other",
                    }
                    .into();
                }
                if let Some(mcp) = update.mcp_tool() {
                    tool.mcp = Some(ConversationMcpTool {
                        server: mcp.server().into(),
                        tool: mcp.tool().into(),
                        // Filled when the view is read, from the tools as
                        // last listed: see `read_with_mode_change`.
                        resource_uri: None,
                    });
                }
                if let Some(content) = update.content() {
                    // The structured result is read apart from the text, so a
                    // long text cannot push it out; the last one reported is
                    // the result's. Replaced with the content, as details are.
                    // Past the bound it is left out, never cut: JSON cut short
                    // is not JSON, and its text is in the details either way.
                    let structured = content.iter().rev().find_map(|item| match item.view() {
                        ToolContentView::Structured(json) => Some(json),
                        _ => None,
                    });
                    tool.structured_content = structured
                        .filter(|json| json.len() <= MAX_STRUCTURED_CONTENT_BYTES)
                        .map(str::to_owned);
                    let mut details = String::new();
                    for item in content {
                        let text = match item.view() {
                            ToolContentView::Structured(_) => continue,
                            ToolContentView::Text(text) => clipped(text, 16384),
                            ToolContentView::Diff { path, old, new } => format!(
                                "File: {}\nBefore:\n{}\nAfter:\n{}",
                                path.as_str(),
                                clipped(old.unwrap_or("(not provided)"), 7000),
                                clipped(new, 7000)
                            ),
                        };
                        if !details.is_empty() {
                            details.push('\n');
                        }
                        details.push_str(&text);
                        if details.len() > 16000 {
                            details = clipped(&details, 16000);
                            details.push_str("\n[Output truncated]");
                            break;
                        }
                    }
                    tool.details = details;
                }
                if let Some(status) = update.status() {
                    tool.status = match status {
                        ToolStatus::Pending => "pending",
                        ToolStatus::Running => "running",
                        ToolStatus::Completed => "completed",
                        ToolStatus::Failed => "failed",
                    }
                    .into();
                }
            }
        }
        self.bump();
    }
    fn record(&mut self, record: &InvocationRecord, restoring: bool) {
        let id = record.request.execution_id.as_str();
        let index = self.ensure_message(id);
        self.view.messages[index].steering_target = record
            .scheduling
            .iter()
            .find(|event| event.stage == InvocationStage::Injected)
            .and_then(|event| event.target.as_ref())
            .map(|target| target.as_str().to_owned());
        self.view.messages[index].user_text =
            clipped(record.request.user_message.text_str(), MAX_TEXT);
        self.view.messages[index].attachments = record
            .request
            .user_message
            .images()
            .iter()
            .map(Into::into)
            .collect();
        self.view.messages[index].files = record
            .request
            .user_message
            .files()
            .iter()
            .map(Into::into)
            .collect();
        self.view.messages[index].parts.clear();
        self.tool_parts.remove(id);
        self.view.messages[index].event_count = 0;
        self.view.messages[index].steering_offset = record.target_event_offset;
        self.view.messages[index].status = ConversationMessageStatus::Running;
        self.view.messages[index].error = None;
        for event in &record.events {
            self.observe(event);
        }
        let index = self.ensure_message(id);
        self.view.messages[index].status = record_status(record, restoring);
        if self.view.messages[index].status == ConversationMessageStatus::Injected {
            if let Some(target) = self.view.messages[index].steering_target.clone() {
                self.injected(id, &target);
            }
        }
        self.view.messages[index].error = failure_notice(record);
        // An ask whose closure never reached storage — the gateway stopped
        // while it was open — would otherwise come back beside a settled or
        // unresolved message; the status is the authority, so it decides.
        if self.view.messages[index].status != ConversationMessageStatus::Running {
            self.stop_waiting(id);
        }
        self.view.pending.retain(|value| value.execution_id != id);
    }

    pub fn read(&self) -> ConversationView {
        self.read_with_mode_change(None)
    }
    pub fn read_with_mode_change(
        &self,
        change: Option<ConversationApprovalModeChangeView>,
    ) -> ConversationView {
        let mut view = self.view.clone();
        if let Some(change) = change {
            let digest = Sha256::digest(
                format!(
                    "{}:{}:{:?}",
                    change.request_id, change.requested_mode, change.status
                )
                .as_bytes(),
            );
            view.revision = format!("{}:mode:{:x}", view.revision, digest);
            view.approval_mode_change = Some(change);
        }
        // Each MCP call's UI, from its server's tools as last listed: a tool's
        // declaration is the server's, not the call's, and no harness passes it
        // through ACP (ADR 344). Filled here, before the byte budget below, and
        // folded into the revision, so a window holding this revision holds
        // these URIs: a list read after the call still reaches the window.
        let mut drawn = Sha256::new();
        let mut any = false;
        // The conversation's own SDK session (`conversation_session`).
        let session = ConversationId::new(&view.conversation_id)
            .ok()
            .map(|id| super::conversation_session(&id));
        for tool in &mut view.tools {
            let Some(mcp) = &mut tool.mcp else { continue };
            mcp.resource_uri = McpTool::new(mcp.server.as_str(), mcp.tool.as_str())
                .ok()
                .zip(session.as_ref())
                .and_then(|(call, session)| self.tool_uis.resource_uri(session, &call))
                .map(|uri| uri.as_str().to_owned());
            if let Some(uri) = &mcp.resource_uri {
                any = true;
                for part in [&tool.execution_id, &tool.tool_id, uri] {
                    drawn.update((part.len() as u64).to_be_bytes());
                    drawn.update(part.as_bytes());
                }
            }
        }
        if any {
            view.revision = format!("{}:ui:{:x}", view.revision, drawn.finalize());
        }
        // A review or an ask is offered only while its execution is running:
        // nothing else is waiting on an answer, and the client refuses a view
        // that says otherwise — the whole conversation would fail to load.
        // Enforced here, where the view is handed out, rather than at each of
        // the many places a message's status changes; any one of them missing
        // it was enough to make a conversation unreadable. An execution whose
        // message is not in the view is left as the client allows: only in a
        // view that says it was truncated.
        let questions = view
            .questions
            .iter()
            .filter(|question| offered(&view, &question.execution_id))
            .cloned()
            .collect();
        let permissions = view
            .permissions
            .iter()
            .filter(|permission| offered(&view, &permission.execution_id))
            .cloned()
            .collect();
        view.questions = questions;
        view.permissions = permissions;
        view
    }
}

/// What a view bounded to its size says of the interactions it left out.
const UNSHOWN_INTERACTIONS: &str =
    "Some pending interactions exceed this display limit. Use Stop to cancel them.";

/// Bound the complete service result after its metadata and authority additions.
pub(super) fn bound_view(view: ConversationView) -> ConversationView {
    bound_view_within(view, MAX_VIEW_BYTES, true)
}

/// As [`bound_view`], within `limit` bytes: the view that leaves room for
/// what is added beside it after. Its interactions — the agent's reviews
/// and questions — are given up for that room only when `interactions`
/// says so; otherwise it stops short of them, over `limit` if need be.
pub(super) fn bound_view_within(
    mut view: ConversationView,
    limit: usize,
    interactions: bool,
) -> ConversationView {
    // The binding bounds actual admitted ACP asks. Custom backends can
    // retain other valid histories, so the encoded display owner gives up
    // whole interactions with an explicit notice rather than cutting choices.
    while serde_json::to_vec(&view).map_or(usize::MAX, |bytes| bytes.len()) > limit {
        // The structured result is optional display data. Preserve the history
        // and its text before giving up a message or interaction.
        if let Some(tool) = view
            .tools
            .iter_mut()
            .find(|tool| tool.structured_content.is_some())
        {
            tool.structured_content = None;
            continue;
        }
        // Only the interactions are left, and they are not to be given up:
        // nothing is removed, so the view is not truncated either.
        if !interactions && !gives_up_anything_but_interactions(&view) {
            break;
        }
        view.truncated = true;
        if view.messages.len() > 1 {
            view.messages.remove(0);
        } else if view
            .messages
            .first()
            .is_some_and(|message| !message.parts.is_empty())
        {
            view.messages[0].parts.pop();
        } else if let Some(message) = view
            .messages
            .first_mut()
            .filter(|message| !message.user_text.is_empty())
        {
            message.user_text = clipped(&message.user_text, message.user_text.len() / 2);
        } else if let Some(message) = view
            .messages
            .first_mut()
            .filter(|message| !message.files.is_empty() || !message.attachments.is_empty())
        {
            // What the message linked is shown by name, and a path can be
            // long; past the budget the view says it left some out rather
            // than break its bound. Whole interactions yield afterward
            // with an explicit display-limit notice.
            if message.files.pop().is_none() {
                message.attachments.pop();
            }
        } else if !view.tools.is_empty() {
            view.tools.remove(0);
        } else if !view.pending.is_empty() {
            view.pending.remove(0);
            view.queue_complete = false;
        } else if !view.permissions.is_empty() {
            view.permissions.pop();
            view.interaction_view_error = Some(UNSHOWN_INTERACTIONS.into());
        } else if !view.questions.is_empty() {
            view.questions.pop();
            view.interaction_view_error = Some(UNSHOWN_INTERACTIONS.into());
        } else {
            break;
        }
    }
    view
}

/// Whether `view` still has anything a bound may give up before its
/// interactions: its transcript, tool calls or queue.
fn gives_up_anything_but_interactions(view: &ConversationView) -> bool {
    view.messages.len() > 1
        || view.messages.first().is_some_and(|message| {
            !message.parts.is_empty()
                || !message.user_text.is_empty()
                || !message.files.is_empty()
                || !message.attachments.is_empty()
        })
        || !view.tools.is_empty()
        || !view.pending.is_empty()
}

/// Whether `view` may offer an ask or review from `execution`: only while its
/// message is running, or — where the message is no longer in the view — only
/// in a view that says it was truncated. The client's own rule, in one place,
/// used both to decide what is offered and what counts against the limits.
fn offered(view: &ConversationView, execution: &str) -> bool {
    view.messages
        .iter()
        .find(|message| message.execution_id == execution)
        .map_or(view.truncated, |message| {
            message.status == ConversationMessageStatus::Running
        })
}
