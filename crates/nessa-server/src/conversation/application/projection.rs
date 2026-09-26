use super::view::{
    ConversationAnswerOption, ConversationAsked, ConversationAttachment, ConversationCapabilities,
    ConversationLifecycle, ConversationLifecyclePhase, ConversationLinkedFile, ConversationMessage,
    ConversationMessageStatus, ConversationPart, ConversationPending, ConversationPendingMode,
    ConversationPermission, ConversationPermissionOption, ConversationQuestion, ConversationTool,
    ConversationView,
};
use nessa_sdk::application::agent_execution::{
    agents::AgentError,
    executions::{ExecutionEvent, ExecutionUpdate},
    sessions::{InvocationRecord, SessionSnapshot},
};
use nessa_sdk::domain::agent_execution::{
    executions::{ExecutionId, ExecutionOutcome, InvocationStage, MessageKind},
    permissions::{ReviewDecline, ReviewDeclineReason, ReviewDeclineStage},
    prompts::UserMessage,
    questions::{AnswerShape, MAX_OPEN_QUESTIONS},
    tools::{ToolContentView, ToolKind, ToolStatus},
};
use std::collections::HashSet;
use uuid::Uuid;

const MAX_MESSAGES: usize = 24;
pub(super) const MAX_TEXT: usize = 8192;
const MAX_TOOLS: usize = 16;
const MAX_PERMISSIONS: usize = 16;
const MAX_VIEW_BYTES: usize = 60_000;
const REQUIRED_WORK_FAILURE: &str = "The turn could not complete all required work.";
const PROVIDER_FAILURE_PREFIX: &str = "The agent provider reported an error: ";

pub(super) struct Projection {
    pub view: ConversationView,
    epoch: Uuid,
    revision: u64,
    lagged: bool,
    resolved_permissions: HashSet<(String, String)>,
    /// Asks that have stopped waiting, so a replayed ask does not reopen one.
    answered_questions: HashSet<(String, String)>,
    terminal_executions: HashSet<String>,
    /// Executions this process saw begin — admitted here, or observed live —
    /// and has not seen stop running. Only these can be waiting on anybody
    /// through this process, and a message missing from the view says nothing
    /// either way: one from before a restart and one pushed out by newer turns
    /// are both absent. This is what tells them apart during lag recovery.
    live_here: HashSet<String>,
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
impl Projection {
    pub fn new(
        id: String,
        capabilities: ConversationCapabilities,
        snapshot: Option<&SessionSnapshot>,
    ) -> Self {
        let mut projection = Self {
            epoch: Uuid::new_v4(),
            revision: 0,
            lagged: false,
            resolved_permissions: HashSet::new(),
            answered_questions: HashSet::new(),
            terminal_executions: HashSet::new(),
            live_here: HashSet::new(),
            view: ConversationView {
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
                permission_view_error: None,
                // Not the projection's: the service fills it from the summary.
                title: None,
            },
        };
        if let Some(snapshot) = snapshot {
            projection.view.truncated = snapshot.invocations.len() > MAX_MESSAGES;
            for record in snapshot
                .invocations
                .iter()
                .rev()
                .take(MAX_MESSAGES)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
            {
                projection.record(record, true);
            }
        }
        projection.bump();
        projection
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
            self.view.messages.remove(0);
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
    pub fn admitted(&mut self, id: &str, input: &UserMessage, mode: ConversationPendingMode) {
        self.live_here.insert(id.to_owned());
        let text = input.text_str();
        let attachments: Vec<ConversationAttachment> =
            input.images().iter().map(Into::into).collect();
        let files: Vec<ConversationLinkedFile> = input.files().iter().map(Into::into).collect();
        let existed = self
            .view
            .messages
            .iter()
            .any(|message| message.execution_id == id);
        let index = self.ensure_message(id);
        let message = &mut self.view.messages[index];
        message.user_text = clipped(text, MAX_TEXT);
        message.attachments = attachments.clone();
        message.files = files.clone();
        if !existed {
            // A retried submission can rebuild a message newer turns pushed
            // out. If its turn already has an ask or review open, it was
            // dispatched and is waiting — only a dispatched turn can have one —
            // and rebuilding it as queued hid exactly what it waits on.
            let waiting = self
                .view
                .questions
                .iter()
                .any(|question| question.execution_id == id)
                || self
                    .view
                    .permissions
                    .iter()
                    .any(|permission| permission.execution_id == id);
            let message = &mut self.view.messages[index];
            message.status = if waiting {
                ConversationMessageStatus::Running
            } else {
                ConversationMessageStatus::Queued
            };
        }
        let message = &mut self.view.messages[index];
        if !self
            .view
            .pending
            .iter()
            .any(|pending| pending.execution_id == id)
            && message.status == ConversationMessageStatus::Queued
        {
            if self.view.pending.len() < 64 {
                self.view.pending.push(ConversationPending {
                    execution_id: id.into(),
                    text: clipped(text, MAX_TEXT),
                    attachments,
                    files,
                    mode,
                });
            } else {
                self.view.truncated = true;
            }
        }
        self.bump();
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
    pub fn uncertain_permission(&mut self, execution: &str, id: &str) {
        self.resolved_permissions
            .insert((execution.to_owned(), id.to_owned()));
        self.view
            .permissions
            .retain(|value| value.execution_id != execution || value.permission_id != id);
        self.view.permission_view_error = Some(
            "The permission answer was interrupted after admission; this review is no longer actionable. Close the conversation if provider cleanup remains pending."
                .into(),
        );
        self.bump();
    }
    /// Rebuild actionable reviews from committed SDK evidence after observation lag.
    /// The lag fence remains active for output because broadcast updates have no
    /// durable cursor. Successful local answers are retained separately because
    /// their mandatory audit is not an execution observation.
    pub fn recover_permissions(&mut self, snapshot: Option<&SessionSnapshot>) {
        if !self.lagged {
            return;
        }
        let previous = serde_json::to_vec(&self.view.permissions).ok();
        let previous_questions = serde_json::to_vec(&self.view.questions).ok();
        let previous_error = self.view.permission_view_error.clone();
        self.view.permissions.clear();
        self.view.permission_view_error = None;
        let Some(snapshot) = snapshot else {
            if previous.as_deref() != Some(b"[]") {
                self.bump();
            }
            return;
        };
        for record in &snapshot.invocations {
            let execution = record.request.execution_id.as_str();
            if self.terminal_executions.contains(execution)
                || record.result.is_some()
                || record.scheduling.last().is_some_and(|event| {
                    matches!(
                        event.stage,
                        InvocationStage::Cancelled
                            | InvocationStage::Injected
                            | InvocationStage::Settled
                    )
                })
            {
                continue;
            }
            for event in &record.events {
                match event.update() {
                    ExecutionUpdate::PermissionRequested { .. } => {
                        if self.recovered_waiting(execution) {
                            self.observe_permission(event)
                        }
                    }
                    ExecutionUpdate::PermissionCancelled(cancellation) => {
                        let permission = cancellation.request().id().as_str();
                        self.resolved_permissions
                            .insert((execution.to_owned(), permission.to_owned()));
                        self.view.permissions.retain(|value| {
                            value.execution_id != execution || value.permission_id != permission
                        });
                    }
                    ExecutionUpdate::Finished(_) => {
                        self.view
                            .permissions
                            .retain(|permission| permission.execution_id != execution);
                        self.view
                            .questions
                            .retain(|question| question.execution_id != execution);
                    }
                    ExecutionUpdate::QuestionAsked { .. } => {
                        if self.recovered_waiting(execution) {
                            self.observe_question(event)
                        }
                    }
                    ExecutionUpdate::QuestionClosed { id } => {
                        self.answered_questions
                            .insert((execution.to_owned(), id.as_str().to_owned()));
                        self.view.questions.retain(|question| {
                            question.execution_id != execution
                                || question.question_id != id.as_str()
                        });
                    }
                    ExecutionUpdate::Message(_)
                    | ExecutionUpdate::Tool(_)
                    | ExecutionUpdate::ReviewDeclined(_) => {}
                }
            }
        }
        if self.view.permissions.is_empty() && self.view.permission_view_error.is_none() {
            self.view.permission_view_error = Some(
                "Live observations were missed; no current tool review can be recovered safely."
                    .into(),
            );
        }
        // Asks recovered or dropped here change the view as much as reviews do.
        if previous != serde_json::to_vec(&self.view.permissions).ok()
            || previous_questions != serde_json::to_vec(&self.view.questions).ok()
            || previous_error != self.view.permission_view_error
        {
            self.bump();
        }
    }

    /// An ask or review on record for an execution that has not settled was made
    /// by one that was dispatched and is waiting on it. Lag may have dropped the
    /// dispatch, leaving the message queued; the evidence says otherwise, so the
    /// message runs and leaves the queue, rather than the ask being hidden from
    /// the only person who could answer it.
    ///
    /// Returns whether the execution is waiting, so its evidence may be offered.
    /// A message present in the view decides by its status: queued or running
    /// is waiting, anything else — unresolved above all, a turn from before a
    /// restart that nothing here can answer for — is not. A message absent from
    /// the view decides by where the turn began: one this process saw begin
    /// was pushed out by newer turns and is still waiting, and is offered
    /// without its message, as a truncated view allows; any other is not. A
    /// message is never created here — recreating one put a dead turn back on
    /// screen as running and pushed a live one out to make room.
    fn recovered_waiting(&mut self, execution: &str) -> bool {
        let Some(index) = self
            .view
            .messages
            .iter()
            .position(|message| message.execution_id == execution)
        else {
            return self.live_here.contains(execution) && self.view.truncated;
        };
        match self.view.messages[index].status {
            ConversationMessageStatus::Running => true,
            ConversationMessageStatus::Queued => {
                self.view.messages[index].status = ConversationMessageStatus::Running;
                self.view
                    .pending
                    .retain(|item| item.execution_id != execution);
                true
            }
            _ => false,
        }
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
            return;
        }
        self.view.questions.push(ConversationQuestion {
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
        });
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
            self.view.permission_view_error = Some("The complete tool input cannot be displayed safely; this review has no actionable choices in this view.".into());
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
            options: options
                .choices()
                .iter()
                .map(|option| ConversationPermissionOption {
                    id: option.id().as_str().into(),
                    label: option.label().into(),
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
        if size > 16_000 || open >= MAX_PERMISSIONS {
            self.view.permission_view_error = Some("A pending tool review exceeds the display limit; no choices were silently removed.".into());
            self.view.truncated = true;
        } else if !self.view.permissions.iter().any(|known| {
            known.execution_id == execution && known.permission_id == value.permission_id
        }) {
            self.view.permissions.push(value);
        }
    }
    pub fn lagged(&mut self) {
        self.lagged = true;
        self.view.truncated = true;
        self.view.permissions.clear();
        for message in &mut self.view.messages {
            if matches!(
                message.status,
                ConversationMessageStatus::Running | ConversationMessageStatus::Queued
            ) {
                message.parts.clear();
            }
        }
        self.view.permission_view_error = Some(
            "Live observations were missed; pending reviews require a refreshed saved view.".into(),
        );
        self.bump();
    }
    /// Apply one live broadcast observation. Live updates carry no durable
    /// cursor, so the lag fence drops their text.
    pub fn event(&mut self, event: &ExecutionEvent) {
        // A live event says a turn began here, unless the view already shows
        // it stopped: an event buffered before a failed receipt and delivered
        // after it would otherwise revive a turn nothing is running any more.
        let id = event.execution_id().as_str();
        let stopped = self.terminal_executions.contains(id)
            || self.view.messages.iter().any(|message| {
                message.execution_id == id
                    && !matches!(
                        message.status,
                        ConversationMessageStatus::Queued | ConversationMessageStatus::Running
                    )
            });
        if !stopped {
            self.live_here.insert(id.to_owned());
        }
        self.observe(event, false);
    }
    /// Apply one observation. `authoritative` marks replay of a committed record
    /// rebuilt from the SDK snapshot: that text is proven to belong to this
    /// message, so the lag fence must not erase it.
    fn observe(&mut self, event: &ExecutionEvent, authoritative: bool) {
        let id = event.execution_id().as_str();
        // A completion watcher can restore the final snapshot before this
        // observer drains already-buffered chunks. Do not append those twice.
        if self.view.messages.iter().any(|message| {
            message.execution_id == id
                && matches!(
                    message.status,
                    ConversationMessageStatus::Completed
                        | ConversationMessageStatus::Cancelled
                        | ConversationMessageStatus::Failed
                        | ConversationMessageStatus::Injected
                )
        }) && !matches!(event.update(), ExecutionUpdate::PermissionCancelled(_))
        {
            return;
        }
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
            ExecutionUpdate::Message(chunk) if authoritative || !self.lagged => {
                Some(ConversationPart {
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
                })
            }
            ExecutionUpdate::Tool(update) => Some(ConversationPart {
                message_id: None,
                offset,
                kind: "tool".into(),
                text: String::new(),
                tool_id: clipped(update.id().as_str(), 256),
                notice_id: String::new(),
            }),
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
                message.parts.push(part);
            } else {
                self.view.truncated = true;
            }
        }
        match event.update() {
            ExecutionUpdate::Message(_) => {
                self.view.messages[index].status = ConversationMessageStatus::Running;
            }
            ExecutionUpdate::Finished(result) => {
                self.view.messages[index].status = outcome(*result);
                self.terminal_executions.insert(id.to_owned());
                self.stop_waiting(id);
            }
            ExecutionUpdate::PermissionCancelled(cancellation) => {
                self.resolved_permission(id, cancellation.request().id().as_str())
            }
            ExecutionUpdate::QuestionAsked { .. } => {
                // An agent waiting on an answer is still running: it has not
                // stopped, it has asked.
                self.view.messages[index].status = ConversationMessageStatus::Running;
                self.observe_question(event);
            }
            // `question`, not `id`: `id` here is the execution, and shadowing it
            // recorded the closure under (question, question) — so a replayed
            // ask missed its tombstone and reopened. An ask is identified by the
            // execution that asked and its own identity, together.
            ExecutionUpdate::QuestionClosed { id: question } => {
                self.answered_questions
                    .insert((id.to_owned(), question.as_str().to_owned()));
                self.view.questions.retain(|open| {
                    open.execution_id != id || open.question_id != question.as_str()
                });
            }
            ExecutionUpdate::PermissionRequested { .. } => {
                self.view.messages[index].status = ConversationMessageStatus::Running;
                if !self.lagged {
                    self.observe_permission(event);
                }
            }
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
                if let Some(content) = update.content() {
                    let mut details = String::new();
                    for item in content {
                        let text = match item.view() {
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
        self.view.messages[index].event_count = 0;
        self.view.messages[index].steering_offset = record.target_event_offset;
        self.view.messages[index].status = ConversationMessageStatus::Running;
        self.view.messages[index].error = None;
        for event in &record.events {
            self.observe(event, true);
        }
        let index = self.ensure_message(id);
        self.view.messages[index].status = match &record.result {
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
        };
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
    pub fn settled_all(&mut self, snapshot: Option<&SessionSnapshot>) {
        if let Some(snapshot) = snapshot {
            for record in snapshot
                .invocations
                .iter()
                .rev()
                .take(MAX_MESSAGES)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
            {
                if record.result.is_some()
                    || record.scheduling.last().is_some_and(|event| {
                        matches!(
                            event.stage,
                            InvocationStage::Cancelled
                                | InvocationStage::Injected
                                | InvocationStage::Settled
                        )
                    })
                {
                    self.record(record, false);
                }
            }
        }
        self.bump();
    }
    pub fn receipt_failed(&mut self, id: &str) {
        let index = self.ensure_message(id);
        if self.view.messages[index].status != ConversationMessageStatus::Cancelled {
            self.view.messages[index].status = ConversationMessageStatus::Failed;
        }
        self.stop_waiting(id);
        // Both receipt watchers call `settled` under this same projection lock
        // first. Preserve any diagnostic derived from the refreshed durable
        // record; absent snapshot evidence still receives the generic fallback.
        if self.view.messages[index].error.is_none() {
            self.view.messages[index].error = Some(REQUIRED_WORK_FAILURE.into());
        }
        self.bump();
    }
    pub fn settled(&mut self, id: &str, snapshot: Option<&SessionSnapshot>) {
        if let Some(record) = snapshot.and_then(|snapshot| {
            snapshot
                .invocations
                .iter()
                .find(|record| record.request.execution_id.as_str() == id)
        }) {
            self.record(record, false);
        }
        self.bump();
    }
    /// Read ordering comes from the SDK queue; this projection never schedules work.
    pub fn queue_order(&mut self, order: &[ExecutionId]) {
        let was_complete = self.view.queue_complete;
        let previous = self
            .view
            .pending
            .iter()
            .map(|item| item.execution_id.clone())
            .collect::<Vec<_>>();
        self.view
            .pending
            .retain(|item| order.iter().any(|id| id.as_str() == item.execution_id));
        self.view
            .pending
            .sort_by_key(|item| order.iter().position(|id| id.as_str() == item.execution_id));
        self.view.queue_complete = order.len() == self.view.pending.len();
        if was_complete != self.view.queue_complete
            || previous
                != self
                    .view
                    .pending
                    .iter()
                    .map(|item| item.execution_id.clone())
                    .collect::<Vec<_>>()
        {
            self.bump();
        }
    }
    pub fn read(&self) -> ConversationView {
        let mut view = self.view.clone();
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
        // Bound actual encoded bytes, including JSON escaping. Permissions are
        // atomic review units: never truncate an option or fabricate a choice.
        while serde_json::to_vec(&view).map_or(usize::MAX, |bytes| bytes.len()) > MAX_VIEW_BYTES {
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
            } else if !view.tools.is_empty() {
                view.tools.remove(0);
            } else if !view.pending.is_empty() {
                view.pending.remove(0);
                view.queue_complete = false;
            } else if !view.permissions.is_empty() {
                view.permissions.pop();
                view.permission_view_error = Some("Additional tool reviews exceed this bounded view; close the session to cancel all pending reviews.".into());
            } else {
                break;
            }
        }
        view
    }
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
