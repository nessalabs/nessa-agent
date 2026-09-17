use super::view::{
    ConversationCapabilities, ConversationMessage, ConversationMessageStatus, ConversationPart,
    ConversationPending, ConversationPendingMode, ConversationPermission,
    ConversationPermissionOption, ConversationTool, ConversationView,
};
use nessa_sdk::application::agent_execution::{
    executions::{ExecutionEvent, ExecutionUpdate},
    sessions::{InvocationRecord, SessionSnapshot},
};
use nessa_sdk::domain::agent_execution::{
    executions::{ExecutionId, ExecutionOutcome, InvocationStage, MessageKind},
    tools::{ToolContentView, ToolStatus},
};
use std::collections::HashSet;
use uuid::Uuid;

const MAX_MESSAGES: usize = 24;
const MAX_TEXT: usize = 8192;
const MAX_TOOLS: usize = 16;
const MAX_PERMISSIONS: usize = 16;
const MAX_VIEW_BYTES: usize = 60_000;

pub(super) struct Projection {
    pub view: ConversationView,
    epoch: Uuid,
    revision: u64,
    lagged: bool,
    resolved_permissions: HashSet<(String, String)>,
    terminal_executions: HashSet<String>,
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
            terminal_executions: HashSet::new(),
            view: ConversationView {
                conversation_id: id,
                revision: String::new(),
                messages: Vec::new(),
                pending: Vec::new(),
                permissions: Vec::new(),
                tools: Vec::new(),
                runtime: None,
                capabilities,
                truncated: false,
                queue_complete: true,
                permission_view_error: None,
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
            steering_target: None,
            status: ConversationMessageStatus::Running,
            error: None,
        });
        self.view.messages.len() - 1
    }
    pub fn admitted(&mut self, id: &str, text: &str, mode: ConversationPendingMode) {
        let existed = self
            .view
            .messages
            .iter()
            .any(|message| message.execution_id == id);
        let index = self.ensure_message(id);
        let message = &mut self.view.messages[index];
        message.user_text = clipped(text, MAX_TEXT);
        if !existed {
            message.status = ConversationMessageStatus::Queued;
        }
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
                    ExecutionUpdate::PermissionRequested { .. } => self.observe_permission(event),
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
                    }
                    ExecutionUpdate::Message(_) | ExecutionUpdate::Tool(_) => {}
                }
            }
        }
        if self.view.permissions.is_empty() && self.view.permission_view_error.is_none() {
            self.view.permission_view_error = Some(
                "Live observations were missed; no current tool review can be recovered safely."
                    .into(),
            );
        }
        if previous != serde_json::to_vec(&self.view.permissions).ok()
            || previous_error != self.view.permission_view_error
        {
            self.bump();
        }
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
        if size > 16_000 || self.view.permissions.len() >= MAX_PERMISSIONS {
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
        let available = (MAX_TEXT * 2).saturating_sub(
            self.view.messages[index]
                .parts
                .iter()
                .map(|part| part.text.len())
                .sum::<usize>(),
        );
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
                })
            }
            ExecutionUpdate::Tool(update) => Some(ConversationPart {
                message_id: None,
                offset,
                kind: "tool".into(),
                text: String::new(),
                tool_id: clipped(update.id().as_str(), 256),
            }),
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
                self.view
                    .permissions
                    .retain(|permission| permission.execution_id != id);
            }
            ExecutionUpdate::PermissionCancelled(cancellation) => {
                self.resolved_permission(id, cancellation.request().id().as_str())
            }
            ExecutionUpdate::PermissionRequested { .. } => {
                self.view.messages[index].status = ConversationMessageStatus::Running;
                if !self.lagged {
                    self.observe_permission(event);
                }
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
                        status: "pending".into(),
                    });
                    self.view.tools.last_mut().unwrap()
                };
                if let Some(title) = update.title() {
                    tool.title = clipped(title, 512);
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
            clipped(record.request.user_message.as_str(), MAX_TEXT);
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
        if record.result.as_ref().is_some_and(Result::is_err) {
            self.view.messages[index].error = Some("The agent operation failed. Retry with the same submission identity to inspect its saved result.".into());
        }
        if record.result.is_some() || restoring {
            self.view
                .permissions
                .retain(|permission| permission.execution_id != id);
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
        self.view.messages[index].error = Some("The invocation could not complete all required work. Its provider result and audit evidence remain saved separately.".into());
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
