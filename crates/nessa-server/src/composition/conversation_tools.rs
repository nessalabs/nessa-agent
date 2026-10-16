//! Source-bound agent conversation tools over the gateway's existing MCP relay.
use crate::conversation::application::{
    error_code, event_error_code, project_events, validate_event_cursor, ConversationCaller,
    ConversationError, ConversationEventError, ConversationService, ReadOpening, RequestedAgent,
    RequestedConversation, SubmissionMode, SubmittedMessage,
};
use crate::mcp_servers::application::{BuiltInFuture, BuiltInServer};
use nessa_protocol::agents::AgentId;
use nessa_protocol::conversation::{domain::ConversationId, view::ConversationMessageStatus};
use nessa_sdk::application::agent_execution::{
    commands::{
        CreationFailure, CreationStage, MutationFailure, MutationOutcome, MutationReceipt,
        MutationStage,
    },
    sessions::{CommittedEventCursor, EventRead, EventReadLimits, EventStart},
};
use nessa_sdk::domain::agent_execution::{executions::Initiation, sessions::SessionId};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    sync::{Arc, OnceLock},
    time::Duration,
};
use tokio::{
    sync::{watch, Semaphore},
    time::{sleep, Instant},
};

pub(super) const SERVER_NAME: &str = "nessa-agents";
const MAX_WAIT_MS: u64 = 30_000;
const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 100;
const MAX_EVENT_READERS: usize = 4;

pub(crate) struct ConversationTools {
    service: OnceLock<ConversationService>,
    readers: Arc<Semaphore>,
}
impl ConversationTools {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            service: OnceLock::new(),
            readers: Arc::new(Semaphore::new(MAX_EVENT_READERS)),
        })
    }
    pub(crate) fn attach(&self, service: ConversationService) {
        let _ = self.service.set(service);
    }
}
impl BuiltInServer for ConversationTools {
    fn name(&self) -> &str {
        SERVER_NAME
    }
    fn configuration(&self) -> Vec<String> {
        vec!["conversation-tools/1".into()]
    }
    fn tools(&self) -> Vec<Value> {
        let identity = json!({
            "type": "string",
            "description": "Optional stable request identity for intentional retries. Reuse it with the same arguments to recover the original receipt.",
        });
        vec![
            json!({
                "name": "create_thread",
                "description": "Create an empty thread owned by the person whose turn is calling. Returns its threadId. Uses the configured default agent/model and asks for tool permission normally.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "requestId": identity,
                        "agent": {"type": "string", "enum": AgentId::ALL.iter().map(|agent| agent.name()).collect::<Vec<_>>()},
                        "model": {"type": "string"},
                        "environment": {"type": "string"},
                    },
                    "additionalProperties": false,
                },
                "annotations": {"readOnlyHint": false, "destructiveHint": false, "openWorldHint": false},
            }),
            json!({
                "name": "send_message",
                "description": "Queue a plain text message in another thread of the same owner. Returns the executionId and durable command status; accepted does not mean completed. Intentional retries should reuse requestId.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "threadId": {"type": "string"},
                        "message": {"type": "string"},
                        "requestId": identity,
                    },
                    "required": ["threadId", "message"],
                    "additionalProperties": false,
                },
                "annotations": {"readOnlyHint": false, "destructiveHint": false, "openWorldHint": false},
            }),
            json!({
                "name": "subscribe",
                "description": "Read a bounded batch of committed thread events and a cursor. Repeat with after to continue; omit after to start from now, or use replay:true after sending to include fast completions. Optionally wait for events. This call does not send a message, run an agent, or arrange a future model turn.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "threadId": {"type": "string"},
                        "after": {"type": "string"},
                        "replay": {"type": "boolean", "description": "Replay retained events from the beginning. Mutually exclusive with after. Use after send_message when no prior checkpoint exists."},
                        "waitMs": {"type": "integer", "minimum": 0, "maximum": MAX_WAIT_MS},
                        "limit": {"type": "integer", "minimum": 1, "maximum": MAX_LIMIT},
                    },
                    "required": ["threadId"],
                    "additionalProperties": false,
                },
                "annotations": {"readOnlyHint": true, "openWorldHint": false},
            }),
            json!({
                "name": "stop_thread",
                "description": "Stop one execution in a thread of the same owner. Defaults to the current active execution captured when called. A queued execution is withdrawn; an active execution is cancelled; a finished execution is already final. Does not delete or close the thread. Preserve executionId when retrying.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "threadId": {"type": "string"},
                        "executionId": {"type": "string"},
                        "requestId": identity,
                    },
                    "required": ["threadId"],
                    "additionalProperties": false,
                },
                "annotations": {"readOnlyHint": false, "destructiveHint": true, "openWorldHint": false},
            }),
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
        let source = ConversationId::new(session.as_str());
        let readers = self.readers.clone();
        Box::pin(async move {
            let (Some(service), Ok(source)) = (service, source) else {
                return failed("agent_tools_unavailable");
            };
            // At most four waiting reads across this server, leaving room in
            // the relay's eight-call connection bound for command/control calls.
            let _reader = if tool == "subscribe" {
                match readers.try_acquire_owned() {
                    Ok(permit) => Some(permit),
                    Err(_) => return failed("subscription_capacity"),
                }
            } else {
                None
            };
            match run(service, source, call, tool, arguments, stop).await {
                Ok(value) => answer(value),
                Err(code) => failed(&code),
            }
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Create {
    request_id: Option<String>,
    agent: Option<String>,
    model: Option<String>,
    environment: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Send {
    thread_id: String,
    message: String,
    request_id: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Stop {
    thread_id: String,
    execution_id: Option<String>,
    request_id: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Follow {
    thread_id: String,
    after: Option<String>,
    replay: Option<bool>,
    wait_ms: Option<u64>,
    limit: Option<usize>,
}
fn parse<T: serde::de::DeserializeOwned>(arguments: Value) -> Result<T, String> {
    serde_json::from_value(arguments).map_err(|_| "invalid_request".into())
}
fn thread(id: &str) -> Result<ConversationId, String> {
    ConversationId::new(id).map_err(|_| "invalid_request".into())
}
fn request(value: Option<String>, call: &str) -> Result<String, String> {
    let value = value.unwrap_or_else(|| call.into());
    if value.is_empty() || value.len() > 256 {
        return Err("invalid_request".into());
    }
    Ok(value)
}
/// Stable UUID defaults keep one logical retry bound to the original target.
fn identity(source: &ConversationId, request: &str, purpose: &str) -> String {
    let bytes =
        serde_json::to_vec(&(source.to_string(), request, purpose)).expect("strings serialize");
    let digest = Sha256::digest(bytes);
    let mut bytes = [0; 16];
    bytes.copy_from_slice(&digest[..16]);
    uuid::Uuid::from_bytes(bytes).to_string()
}
fn code(error: ConversationError) -> String {
    error_code(&error).as_str().into()
}
fn mutation(error: MutationFailure<ConversationError>) -> String {
    match error {
        MutationFailure::Target(error) => code(error),
        MutationFailure::Conflict => "request_conflict".into(),
        MutationFailure::Interrupted(_) => "command_unconfirmed".into(),
        MutationFailure::Storage(_) | MutationFailure::TaskFault(_) => "command_unavailable".into(),
    }
}
fn receipt(receipt: &MutationReceipt) -> Value {
    let stage = match receipt.stage() {
        MutationStage::Accepted => "accepted",
        MutationStage::Attempted => "attempted",
        MutationStage::Settled => "settled",
    };
    let outcome = receipt.outcome().map(|outcome| match outcome {
        MutationOutcome::Dispatched => "dispatched",
        MutationOutcome::Withdrawn => "withdrawn",
        MutationOutcome::Cancelled => "cancelled",
        MutationOutcome::AlreadyFinal => "alreadyFinal",
        MutationOutcome::NoEffect => "noEffect",
        MutationOutcome::RefusedAfterSave => "refusedAfterSave",
        MutationOutcome::UnavailableAfterSave => "unavailableAfterSave",
    });
    json!({"stage":stage,"outcome":outcome})
}
fn creation_stage(stage: CreationStage) -> &'static str {
    match stage {
        CreationStage::Accepted => "accepted",
        CreationStage::Attempted => "attempted",
        CreationStage::Ready => "ready",
    }
}

async fn run(
    service: ConversationService,
    source: ConversationId,
    call: String,
    tool: String,
    arguments: Value,
    stop: watch::Receiver<bool>,
) -> Result<Value, String> {
    match tool.as_str() {
        "create_thread" => {
            let args: Create = parse(arguments)?;
            let request = request(args.request_id, &call)?;
            let target = thread(&identity(&source, &request, "thread"))?;
            let action = identity(&source, &request, "create");
            let source = service
                .admit_agent_tool(source, call, tool, Some(target.clone()), action, stop)
                .await
                .map_err(code)?;
            let storage = service.commands().ok_or("command_unavailable")?;
            let agent = args
                .agent
                .as_deref()
                .map(|name| {
                    AgentId::parse(name)
                        .map(RequestedAgent::Known)
                        .ok_or_else(|| "invalid_request".to_owned())
                })
                .transpose()?;
            let receipt = service
                .create_command(
                    storage,
                    target.clone(),
                    source.caller(),
                    RequestedConversation {
                        agent,
                        model: args.model,
                        approval_mode: None,
                        environment: args.environment,
                    },
                )
                .await
                .map_err(|error| match error {
                    CreationFailure::Target(error) => code(error),
                    CreationFailure::Conflict => "request_conflict".into(),
                    CreationFailure::Interrupted(_) => "command_unconfirmed".into(),
                    CreationFailure::Storage(_) | CreationFailure::TaskFault(_) => {
                        "command_unavailable".into()
                    }
                })?;
            Ok(
                json!({"threadId":target.to_string(),"requestId":request,"stage":creation_stage(receipt.stage())}),
            )
        }
        "send_message" => {
            let args: Send = parse(arguments)?;
            let target = thread(&args.thread_id)?;
            let request = request(args.request_id, &call)?;
            let execution = identity(&source, &request, "execution");
            let action = identity(&source, &request, "send");
            let source = service
                .admit_agent_tool(source, call, tool, Some(target.clone()), action, stop)
                .await
                .map_err(code)?;
            let submitted = service
                .submit_command(
                    service.commands().ok_or("command_unavailable")?,
                    target.clone(),
                    source.caller(),
                    Initiation::owner(),
                    false,
                    execution.clone(),
                    SubmittedMessage {
                        text: args.message,
                        ..Default::default()
                    },
                    SubmissionMode::Queue,
                )
                .await
                .map_err(mutation)?;
            Ok(
                json!({"threadId":target.to_string(),"requestId":request,"executionId":execution,"command":receipt(&submitted.progress)}),
            )
        }
        "stop_thread" => {
            let args: Stop = parse(arguments)?;
            let target = thread(&args.thread_id)?;
            let request = request(args.request_id, &call)?;
            let action = identity(&source, &request, "stop");
            let source = service
                .admit_agent_tool(source, call, tool, Some(target.clone()), action, stop)
                .await
                .map_err(code)?;
            let caller = source.caller();
            let execution = match args.execution_id {
                Some(execution) => execution,
                None => {
                    let (view, _) = service
                        .read_at(target.clone(), caller.clone(), ReadOpening::LiveOnly)
                        .await
                        .map_err(code)?;
                    view.messages
                        .iter()
                        .rev()
                        .find(|message| {
                            matches!(
                                message.status,
                                ConversationMessageStatus::Running
                                    | ConversationMessageStatus::Injected
                            )
                        })
                        .map(|message| message.execution_id.clone())
                        .or_else(|| {
                            view.pending
                                .first()
                                .map(|pending| pending.execution_id.clone())
                        })
                        .ok_or("no_active_execution")?
                }
            };
            let stopped = service
                .stop_command(
                    service.commands().ok_or("command_unavailable")?,
                    target.clone(),
                    caller,
                    execution.clone(),
                )
                .await
                .map_err(mutation)?;
            Ok(
                json!({"threadId":target.to_string(),"requestId":request,"executionId":execution,"command":receipt(&stopped)}),
            )
        }
        "subscribe" => {
            let args: Follow = parse(arguments)?;
            let target = thread(&args.thread_id)?;
            let wait = args.wait_ms.unwrap_or(0);
            let limit = args.limit.unwrap_or(DEFAULT_LIMIT);
            if wait > MAX_WAIT_MS
                || !(1..=MAX_LIMIT).contains(&limit)
                || (args.after.is_some() && args.replay.unwrap_or(false))
            {
                return Err("invalid_request".into());
            }
            let action = identity(&source, &call, "subscribe");
            let source = service
                .admit_agent_tool(
                    source,
                    call,
                    tool,
                    Some(target.clone()),
                    action,
                    stop.clone(),
                )
                .await
                .map_err(code)?;
            follow(&service, target, source.caller(), args, stop).await
        }
        _ => Err("unknown_tool".into()),
    }
}
fn answer(value: Value) -> Value {
    let text = serde_json::to_string(&value).expect("JSON serializes");
    if text.len() > 32 * 1024 {
        return failed("tool_result_too_large");
    }
    json!({"content":[{"type":"text","text":text}],"structuredContent":value,"isError":false})
}
fn failed(code: &str) -> Value {
    let value = json!({"error":code});
    json!({"content":[{"type":"text","text":value.to_string()}],"structuredContent":value,"isError":true})
}

/// One retained reader per call; idle reads use a bounded timer, while the
/// committed cursor makes wake coalescing harmless. No provider is opened.
async fn follow(
    service: &ConversationService,
    target: ConversationId,
    caller: ConversationCaller,
    args: Follow,
    mut stop: watch::Receiver<bool>,
) -> Result<Value, String> {
    let wait_ms = args.wait_ms.unwrap_or(0);
    let limit = args.limit.unwrap_or(DEFAULT_LIMIT);
    let session = SessionId::new(&target.to_string()).map_err(|_| "invalid_request")?;
    let start = match args.after {
        Some(token) => EventStart::After(CommittedEventCursor::decode(&session, &token).map_err(
            |error| event_error_code(&ConversationEventError::Events(error)).to_owned(),
        )?),
        None if args.replay.unwrap_or(false) => EventStart::Beginning,
        None => EventStart::Now,
    };
    let limits =
        EventReadLimits::new(limit.min(16), 256 * 1024 * 1024).map_err(|_| "invalid_request")?;
    let mut reader = service
        .open_events(target.clone(), caller.clone(), start)
        .await
        .map_err(|error| event_error_code(&error).to_owned())?;
    let started = Instant::now();
    let deadline = started + Duration::from_millis(wait_ms);
    let prepare_deadline = started + Duration::from_secs(10);
    loop {
        if *stop.borrow() {
            return Err("cancelled".into());
        }
        service
            .authorize_events(&target, &caller)
            .await
            .map_err(code)?;
        let read = tokio::select! {
            biased;
            _ = stop.changed() => return Err("cancelled".into()),
            result = reader.read(limits) => result.map_err(|error| event_error_code(&ConversationEventError::Events(error)).to_owned())?,
        };
        match read {
            EventRead::Preparing => {
                if Instant::now() >= prepare_deadline {
                    return Err("event_source_preparing".into());
                }
                tokio::task::yield_now().await;
            }
            EventRead::Page(page) => {
                validate_event_cursor(&target, page.next())
                    .map_err(|error| event_error_code(&error).to_owned())?;
                validate_event_cursor(&target, page.captured_head())
                    .map_err(|error| event_error_code(&error).to_owned())?;
                let events = project_events(&target, page.events())
                    .map_err(|error| event_error_code(&error).to_owned())?;
                return Ok(
                    json!({"threadId":target.to_string(),"events":events,"cursor":page.next().encode(),"more":page.next()!=page.captured_head(),"timedOut":false}),
                );
            }
            EventRead::Idle(cursor) => {
                validate_event_cursor(&target, &cursor)
                    .map_err(|error| event_error_code(&error).to_owned())?;
                if Instant::now() >= deadline {
                    return Ok(
                        json!({"threadId":target.to_string(),"events":[],"cursor":cursor.encode(),"more":false,"timedOut":wait_ms>0}),
                    );
                }
                let remaining = deadline
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(250));
                tokio::select! {
                    _ = stop.changed() => return Err("cancelled".into()),
                    _ = sleep(remaining) => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_command_ids_are_stable_and_separate_source_operation_and_request() {
        let a = thread("00000000-0000-4000-8000-000000000001").unwrap();
        let b = thread("00000000-0000-4000-8000-000000000002").unwrap();
        assert_eq!(
            identity(&a, "request", "thread"),
            identity(&a, "request", "thread")
        );
        assert_ne!(
            identity(&a, "request", "thread"),
            identity(&b, "request", "thread")
        );
        assert_ne!(
            identity(&a, "request", "thread"),
            identity(&a, "request", "execution")
        );
        assert_ne!(
            identity(&a, "request", "thread"),
            identity(&a, "another", "thread")
        );
    }

    #[test]
    fn tool_schemas_and_parsing_expose_no_source_authority_or_permission_bypass() {
        let tools = ConversationTools::new().tools();
        assert_eq!(
            tools
                .iter()
                .map(|tool| tool["name"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["create_thread", "send_message", "subscribe", "stop_thread"]
        );
        for tool in tools {
            assert_eq!(tool["inputSchema"]["additionalProperties"], false);
        }
        assert!(parse::<Create>(json!({"ownerId":"other"})).is_err());
        assert!(parse::<Create>(json!({"approvalMode":"full"})).is_err());
        assert!(parse::<Send>(
            json!({"threadId":"target","message":"hello","sourceExecutionId":"forged"})
        )
        .is_err());
    }

    #[test]
    fn oversized_tool_results_fail_before_text_and_structured_content_are_duplicated() {
        let value = answer(json!({"value":"x".repeat(32*1024)}));
        assert_eq!(value["isError"], true);
        assert_eq!(value["structuredContent"]["error"], "tool_result_too_large");
        assert_eq!(answer(json!({"value":"small"}))["isError"], false);
    }

    #[tokio::test]
    async fn calls_before_service_attachment_fail_honestly() {
        let server = ConversationTools::new();
        let (_, stop) = watch::channel(false);
        let value = server
            .call(
                &SessionId::new("source").unwrap(),
                "call".into(),
                "create_thread".into(),
                json!({}),
                stop,
            )
            .await;
        assert_eq!(value["isError"], true);
        assert_eq!(
            value["structuredContent"]["error"],
            "agent_tools_unavailable"
        );
    }
}
