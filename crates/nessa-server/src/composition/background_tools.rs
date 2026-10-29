//! The background command MCP boundary; session identity comes from the relay.
use crate::conversation::{
    application::{
        background_tasks::{
            BackgroundCallMatcher, BackgroundRecord, BackgroundTaskError, BACKGROUND_SERVER,
            BACKGROUND_START,
        },
        AgentToolRequest, ConversationService,
    },
    domain::{BackgroundCommand, BackgroundTaskId},
};
use crate::mcp_servers::application::{BuiltInFuture, BuiltInServer};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::domain::agent_execution::sessions::SessionId;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::{Arc, OnceLock};
use tokio::sync::watch;

pub(super) struct BackgroundTools {
    service: OnceLock<ConversationService>,
}
impl BackgroundTools {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            service: OnceLock::new(),
        })
    }
    pub(super) fn attach(&self, service: ConversationService) {
        let _ = self.service.set(service);
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Start {
    task_id: String,
    command: String,
    timeout_seconds: Option<u64>,
}
impl Start {
    fn validated(self) -> Result<(BackgroundTaskId, BackgroundCommand), BackgroundTaskError> {
        Ok((
            BackgroundTaskId::new(self.task_id).map_err(|_| BackgroundTaskError::InvalidInput)?,
            BackgroundCommand::new(
                self.command,
                self.timeout_seconds
                    .unwrap_or(BackgroundCommand::DEFAULT_TIMEOUT_SECONDS),
            )
            .map_err(|_| BackgroundTaskError::InvalidInput)?,
        ))
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Task {
    task_id: String,
}

pub(super) struct Arguments;
// Claude reports the argument object; Codex reports its MCP envelope. Decode
// both current provider representations here, before comparing domain values.
#[derive(Deserialize)]
#[serde(untagged)]
enum ObservedInput {
    Arguments(Start),
    Envelope(McpInput),
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct McpInput {
    server: String,
    tool: String,
    arguments: Start,
}
impl ObservedInput {
    fn arguments(self) -> Option<Start> {
        match self {
            Self::Arguments(arguments) => Some(arguments),
            Self::Envelope(input)
                if input.server == BACKGROUND_SERVER && input.tool == BACKGROUND_START =>
            {
                Some(input.arguments)
            }
            Self::Envelope(_) => None,
        }
    }
}
impl BackgroundCallMatcher for Arguments {
    fn matches(
        &self,
        arguments: &str,
        task: &BackgroundTaskId,
        command: &BackgroundCommand,
    ) -> bool {
        serde_json::from_str::<ObservedInput>(arguments)
            .ok()
            .and_then(ObservedInput::arguments)
            .and_then(|a| a.validated().ok())
            .is_some_and(|(id, request)| &id == task && &request == command)
    }
}
impl BuiltInServer for BackgroundTools {
    fn name(&self) -> &str {
        BACKGROUND_SERVER
    }
    fn configuration(&self) -> Vec<String> {
        vec!["background-tasks".into()]
    }
    fn tools(&self) -> Vec<Value> {
        let id = json!({"type":"string", "minLength":BackgroundTaskId::MIN_CHARACTERS, "maxLength":BackgroundTaskId::MAX_CHARACTERS, "pattern":BackgroundTaskId::PATTERN, "description":"Choose a stable task identity. Reuse it with the same command and timeout to recover the original task after a lost reply; choose a new identity to run again."});
        vec![
            json!({"name":BACKGROUND_START, "description":"Run a shell command in this local conversation's workspace without waiting for it to finish. Returns the task identity; completion automatically continues this session with the outcome. The existing tool row tracks the command until it ends. Use ordinary shell execution when you want the current call to wait for the result. A background task survives this agent's idle parking, but does not restart after gateway interruption. Remote workspaces are unsupported.", "inputSchema":{"type":"object", "properties":{"taskId":id, "command":{"type":"string", "minLength":1, "maxLength":BackgroundCommand::MAX_BYTES}, "timeoutSeconds":{"type":"integer", "minimum":1, "maximum":BackgroundCommand::MAX_TIMEOUT_SECONDS, "default":BackgroundCommand::DEFAULT_TIMEOUT_SECONDS}}, "required":["taskId","command"], "additionalProperties":false}, "annotations":{"readOnlyHint":false,"destructiveHint":true,"openWorldHint":true}}),
            json!({"name":"status", "description":"Read a background task's current state and retained terminal output. Completion arrives automatically; this operation does not arrange another notification.", "inputSchema":{"type":"object", "properties":{"taskId":id}, "required":["taskId"], "additionalProperties":false}, "annotations":{"readOnlyHint":true,"openWorldHint":false}}),
            json!({"name":"cancel", "description":"Request cancellation of this conversation's background task. The same original tool row settles when process cleanup finishes. Cancelling a terminal task preserves its outcome.", "inputSchema":{"type":"object", "properties":{"taskId":id}, "required":["taskId"], "additionalProperties":false}, "annotations":{"readOnlyHint":false,"destructiveHint":true,"openWorldHint":false}}),
        ]
    }
    fn call(
        &self,
        session: &SessionId,
        call: String,
        tool: String,
        arguments: Value,
        stop: watch::Receiver<bool>,
    ) -> BuiltInFuture {
        let service = self.service.get().cloned();
        let conversation = ConversationId::new(session.as_str());
        Box::pin(async move {
            let result = async {
                let service = service.ok_or(BackgroundTaskError::Unavailable)?;
                let conversation = conversation.map_err(|_| BackgroundTaskError::InvalidInput)?;
                if tool == BACKGROUND_START {
                    let (task, command) = serde_json::from_value::<Start>(arguments)
                        .map_err(|_| BackgroundTaskError::InvalidInput)?
                        .validated()?;
                    return service
                        .start_background_task(conversation, call, task, command, stop)
                        .await;
                }
                if tool != "status" && tool != "cancel" {
                    return Err(BackgroundTaskError::InvalidInput);
                }
                let task = serde_json::from_value::<Task>(arguments)
                    .map_err(|_| BackgroundTaskError::InvalidInput)?;
                let task = BackgroundTaskId::new(task.task_id)
                    .map_err(|_| BackgroundTaskError::InvalidInput)?;
                let source = service
                    .admit_agent_tool(
                        conversation,
                        call,
                        format!("background.{tool}"),
                        None,
                        AgentToolRequest {
                            logical: task.as_str().into(),
                            command: format!("background-{tool}-{}", task.as_str()),
                        },
                        stop,
                    )
                    .await
                    .map_err(|_| BackgroundTaskError::Unavailable)?;
                if tool == "status" {
                    service.background_task(&source, &task).await
                } else {
                    service.cancel_background_task(&source, &task).await
                }
            }
            .await;
            match result {
                Ok(record) => {
                    let result = render(&record);
                    json!({"isError":false,"content":[{"type":"text","text":result.to_string()}],"structuredContent":result}).into()
                }
                Err(error) => {
                    json!({"isError":true,"content":[{"type":"text","text":error_text(&error)}]})
                        .into()
                }
            }
        })
    }
}
fn render(record: &BackgroundRecord) -> Value {
    let outcome = record.lifecycle.outcome();
    json!({"taskId":record.id.as_str(),"state":if outcome.is_some() {"finished"} else if record.lifecycle.stop().is_some() {"stopping"} else {"running"},"outcome":outcome.map(|o| json!({"cause":format!("{:?}",o.cause),"exitCode":o.exit_code,"signal":o.signal,"cleanupVerified":o.cleanup_verified,"stdout":o.stdout,"stderr":o.stderr,"omittedBytes":o.omitted_bytes,"outputComplete":o.output_complete,"error":o.error}))})
}
fn error_text(error: &BackgroundTaskError) -> &'static str {
    match error {
        BackgroundTaskError::InvalidInput => "Invalid background task arguments.",
        BackgroundTaskError::Conflict => "This task identity already names a different request. Reuse its original command and timeout, or choose a new task identity.",
        BackgroundTaskError::NotFound => "No such background task in this conversation.",
        BackgroundTaskError::Capacity => "Background task capacity is full. Running tasks, undelivered outcomes and unconfirmed cleanup retain capacity.",
        BackgroundTaskError::UnsupportedEnvironment => "Background commands currently require a local workspace; this conversation's remote workspace is unsupported.",
        BackgroundTaskError::UnobservedCall => "The native tool call could not be identified uniquely. No command was started. Retry with the same task identity.",
        BackgroundTaskError::Storage => "The background task record could not be committed. Retry with the same task identity to recover any accepted task.",
        BackgroundTaskError::CleanupUnverified | BackgroundTaskError::CleanupIncomplete(_) => "Process cleanup could not be confirmed. The original outcome and ownership evidence are retained.",
        BackgroundTaskError::Unavailable => "Background tasks are unavailable for this call.",
    }
}

#[cfg(test)]
#[path = "../../tests/composition/background_tools.rs"]
mod tests;
