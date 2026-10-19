//! Source-bound agent conversation tools over the gateway's existing MCP relay.
use crate::conversation::application::{
    error_code, event_error_code, project_events, validate_event_cursor, AgentToolRequest,
    ConversationCaller, ConversationError, ConversationEventError, ConversationService,
    EventProgress, PaneConversation, ReportRegistration, RequestedAgent, RequestedConversation,
    SourcePermits, SubmissionMode, SubmittedMessage, ThreadReport, WindowReading,
    MAX_EVENT_READERS,
};
use crate::conversation::domain::{PaneContent, WindowShowing};
use crate::mcp_servers::application::{BuiltInFuture, BuiltInServer};
use nessa_protocol::agents::AgentId;
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::application::agent_execution::{
    commands::{
        CreationFailure, CreationStage, MutationFailure, MutationOutcome, MutationReceipt,
        MutationStage,
    },
    sessions::{
        CommittedEventCursor, EventRead, EventReadError, EventReadLimits, EventStart, NoteDelivery,
        NoteState,
    },
};
use nessa_sdk::domain::agent_execution::{executions::Initiator, sessions::SessionId};
use serde::{Deserialize, Deserializer};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    future::Future,
    sync::{Arc, OnceLock},
    time::Duration,
};
use tokio::{
    sync::watch,
    time::{sleep, sleep_until, Instant},
};

pub(super) const SERVER_NAME: &str = "nessa-agents";
const MAX_WAIT_MS: u64 = 30_000;
const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 100;
const SUBSCRIPTION_SETUP_TIMEOUT: Duration = Duration::from_secs(10);
/// The wait before retrying a read step every physical read worker refused.
const READ_RETRY: Duration = Duration::from_millis(25);

pub(crate) struct ConversationTools {
    service: OnceLock<ConversationService>,
    readers: Arc<SourcePermits>,
}
impl ConversationTools {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            service: OnceLock::new(),
            readers: Arc::new(SourcePermits::new(MAX_EVENT_READERS)),
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
                "description": "Send plain text in another thread of the same owner. queued (default) starts a queued turn; steering uses the existing steering semantics; passive records a note without starting or steering a turn. When a queued or steering message's turn ends, a report with the thread's outcome and final reply is queued into this conversation as a new turn (reportsBack: true), so there is no need to poll for it. Retry with identical thread, text and mode using the same requestId. Passive notes and active submissions retain separate existing operation namespaces.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "threadId": {"type": "string"},
                        "message": {"type": "string"},
                        "mode": {"type": "string", "enum": ["queued", "steering", "passive"], "default": "queued"},
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
            json!({
                "name": "read_window",
                "description": "Read what the person's Nessa desktop windows show right now, as the window last said: whether the sidebar and session list are open, what fills the content region, and every pane left to right and top to bottom with its position in words (\"left\", \"right\", \"top left\"...), its share of the width and height, whether it is focused, and what it shows. A conversation pane gives its threadId with its title, agent and model, so \"the session on the right\" can be found and used with send_message or subscribe. The pane showing this conversation is marked you: true, and `you` says where this conversation is shown, if anywhere. No windows means no desktop window is open and publishing now.",
                "inputSchema": {
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false,
                },
                "annotations": {"readOnlyHint": true, "openWorldHint": false},
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
            // Bounded per source, not across the server, so the readers grow
            // with the agents.
            let _reader = if tool == "subscribe" {
                match readers.try_acquire(&source) {
                    Some(permit) => Some(permit),
                    None => return failed("subscription_capacity"),
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
    #[serde(default, deserialize_with = "supplied")]
    request_id: Option<String>,
    #[serde(default, deserialize_with = "supplied_agent")]
    agent: Option<AgentId>,
    #[serde(default, deserialize_with = "supplied")]
    model: Option<String>,
    #[serde(default, deserialize_with = "supplied")]
    environment: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Send {
    thread_id: String,
    message: String,
    #[serde(default)]
    mode: SendMode,
    #[serde(default, deserialize_with = "supplied")]
    request_id: Option<String>,
}
/// Decoded only from its published string, never serde's map form of a variant.
#[derive(Clone, Copy, Default, Deserialize)]
#[serde(try_from = "String")]
enum SendMode {
    #[default]
    Queued,
    Steering,
    Passive,
}
impl TryFrom<String> for SendMode {
    type Error = &'static str;
    fn try_from(name: String) -> Result<Self, Self::Error> {
        [Self::Queued, Self::Steering, Self::Passive]
            .into_iter()
            .find(|mode| mode.name() == name)
            .ok_or("unknown send mode")
    }
}
impl SendMode {
    fn name(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Steering => "steering",
            Self::Passive => "passive",
        }
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Stop {
    thread_id: String,
    #[serde(default, deserialize_with = "supplied")]
    execution_id: Option<String>,
    #[serde(default, deserialize_with = "supplied")]
    request_id: Option<String>,
}
/// `read_window` takes nothing; an object with any field is refused.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Window {}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Follow {
    thread_id: String,
    #[serde(default, deserialize_with = "supplied")]
    after: Option<String>,
    #[serde(default, deserialize_with = "supplied")]
    replay: Option<bool>,
    #[serde(default, deserialize_with = "supplied")]
    wait_ms: Option<u64>,
    #[serde(default, deserialize_with = "supplied")]
    limit: Option<usize>,
}
/// Absence alone selects a default; a supplied value must have its declared type.
fn supplied<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}
/// The published agent enum is a decode constraint, refused before admission.
fn supplied_agent<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<AgentId>, D::Error> {
    let name = String::deserialize(deserializer)?;
    AgentId::parse(&name)
        .map(Some)
        .ok_or_else(|| serde::de::Error::custom("unknown agent"))
}
/// Published input schemas are objects; serde would also decode a struct from an array.
fn parse<T: serde::de::DeserializeOwned>(arguments: Value) -> Result<T, String> {
    if !arguments.is_object() {
        return Err("invalid_request".into());
    }
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
                .admit_agent_tool(
                    source,
                    call,
                    tool,
                    Some(target.clone()),
                    AgentToolRequest {
                        logical: request.clone(),
                        command: action,
                    },
                    stop,
                )
                .await
                .map_err(code)?;
            let receipt = service
                .create_owned_thread(
                    source,
                    target.clone(),
                    RequestedConversation {
                        agent: args.agent.map(RequestedAgent::Known),
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
            let report = identity(&source, &request, "report");
            let report_action = identity(&source, &request, "report-send");
            let source = service
                .admit_agent_tool(
                    source,
                    call,
                    tool,
                    Some(target.clone()),
                    AgentToolRequest {
                        logical: request.clone(),
                        command: action,
                    },
                    stop,
                )
                .await
                .map_err(code)?;
            let mode = match args.mode {
                SendMode::Queued => SubmissionMode::Queue,
                SendMode::Steering => SubmissionMode::Steer,
                SendMode::Passive => {
                    let (note, state) = service
                        .leave_note(
                            target.clone(),
                            source.caller(),
                            Initiator::Owner,
                            args.message,
                            NoteDelivery::RecordOnly,
                        )
                        .await
                        .map_err(code)?;
                    let state = match state {
                        NoteState::Recorded => "recorded",
                        NoteState::Pending => "pending",
                        NoteState::Assigned(_) => "assigned",
                        NoteState::Retired(_) => "retired",
                    };
                    return Ok(
                        json!({"threadId":target.to_string(),"requestId":request,"mode":"passive","messageId":note.id.as_str(),"acceptedAtMs":note.accepted_at_ms,"deliveryState":state}),
                    );
                }
            };
            let storage = service.commands().ok_or("command_unavailable")?;
            let answer = ThreadReport {
                parent: source.conversation().clone(),
                caller: ConversationCaller {
                    action_id: report_action,
                    ..source.caller()
                },
                execution: report.clone(),
            };
            // Registered before the submit, so a turn that ends at once is
            // not missed; the look after it catches one that already had.
            let registration = service
                .report_thread_turn(target.clone(), execution.clone(), answer.clone())
                .await;
            let sending_caller = source.caller();
            let submitted = match service
                .submit_agent_command(
                    storage,
                    target.clone(),
                    source,
                    execution.clone(),
                    SubmittedMessage {
                        text: args.message,
                        ..Default::default()
                    },
                    mode,
                )
                .await
            {
                Ok(submitted) => submitted,
                Err(error) => {
                    // The thread's saved history decides whether a turn will
                    // finish; a report already waiting stays as it is.
                    if registration == ReportRegistration::Registered {
                        service
                            .settle_failed_thread_report(
                                target.clone(),
                                &execution,
                                &report,
                                &sending_caller,
                            )
                            .await;
                    }
                    return Err(mutation(error));
                }
            };
            // A concurrent retry under this request may have failed and let
            // the report go after this one registered: hold it again.
            let held = service
                .report_thread_turn(target.clone(), execution.clone(), answer)
                .await;
            let reports = registration.reports() || held.reports();
            if held.reports() {
                service.look_for_reports(target.clone(), false).await;
            }
            Ok(
                json!({"threadId":target.to_string(),"requestId":request,"mode":args.mode.name(),"executionId":execution,"command":receipt(&submitted.progress),"reportsBack":reports}),
            )
        }
        "stop_thread" => {
            let args: Stop = parse(arguments)?;
            let target = thread(&args.thread_id)?;
            let request = request(args.request_id, &call)?;
            let action = identity(&source, &request, "stop");
            let source = service
                .admit_agent_tool(
                    source,
                    call,
                    tool,
                    Some(target.clone()),
                    AgentToolRequest {
                        logical: request.clone(),
                        command: action,
                    },
                    stop,
                )
                .await
                .map_err(code)?;
            let caller = source.caller();
            let execution = match args.execution_id {
                Some(execution) => execution,
                None => service
                    .active_execution(target.clone(), caller.clone())
                    .await
                    .map_err(code)?
                    .ok_or("no_active_turn")?,
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
                || (args.after.is_some() && args.replay.is_some())
            {
                return Err("invalid_request".into());
            }
            let action = identity(&source, &call, "subscribe");
            let mut control = SubscriptionControl::new(stop, wait);
            let admission = service.admit_agent_tool(
                source,
                call.clone(),
                tool,
                Some(target.clone()),
                AgentToolRequest {
                    logical: call,
                    command: action,
                },
                control.stop.clone(),
            );
            let source = match control.step(admission).await {
                Ok(result) => result.map_err(code)?,
                Err(ended) => return control.finish(&target, ended),
            };
            follow(&service, target, source.caller(), args, control).await
        }
        "read_window" => {
            let _: Window = parse(arguments)?;
            let action = identity(&source, &call, "window");
            let source = service
                .admit_agent_tool(
                    source,
                    call.clone(),
                    tool,
                    None,
                    AgentToolRequest {
                        logical: call,
                        command: action,
                    },
                    stop,
                )
                .await
                .map_err(code)?;
            let reading = service.read_windows(&source.caller()).await.map_err(code)?;
            Ok(window_view(source.conversation(), &reading))
        }
        _ => Err("unknown_tool".into()),
    }
}

/// `read_window`, as the agent reads it: every window, the most recently
/// published first, every pane with its place in words, and where the asking
/// conversation is shown. Where it is shown more than once, `you` names the
/// focused pane if one is, else the first. A window showing the overview or a
/// widget over its panes has them `covered`, and so is `you` there. Windows
/// past what one tool result holds are left out and counted in `more`, the
/// ones showing the asking conversation kept first.
fn window_view(source: &ConversationId, reading: &WindowReading) -> Value {
    let mut you: Option<Value> = None;
    let mut views: Vec<(bool, Value)> = Vec::new();
    for (index, published) in reading.windows.iter().enumerate() {
        let window = &published.window;
        let covered = !matches!(window.showing(), WindowShowing::Panes);
        let mut mine_here = false;
        let panes: Vec<Value> = window
            .places()
            .into_iter()
            .map(|place| {
                let mut pane = json!({
                    "pane": place.number,
                    "position": place.position,
                    "column": place.column,
                    "row": place.row,
                    "widthPercent": place.column_width_percent,
                    "heightPercent": place.pane.height_percent(),
                    "focused": place.pane.focused(),
                    "kind": place.pane.content().kind(),
                });
                match place.pane.content() {
                    PaneContent::Conversation(id) => {
                        pane["threadId"] = json!(id.to_string());
                        let mine = id == source;
                        pane["you"] = json!(mine);
                        match reading.conversations.get(id) {
                            Some(PaneConversation::Shown {
                                title,
                                agent,
                                model,
                            }) => {
                                pane["title"] = json!(title);
                                pane["agent"] = json!(agent);
                                pane["model"] = json!(model);
                            }
                            _ => pane["available"] = json!(false),
                        }
                        let better = you
                            .as_ref()
                            .is_none_or(|held| held["focused"] == false && place.pane.focused());
                        if mine && better {
                            you = Some(json!({
                                "threadId": source.to_string(),
                                "shown": true,
                                "window": index + 1,
                                "pane": place.number,
                                "position": place.position,
                                "focused": place.pane.focused(),
                                "covered": covered,
                            }));
                        }
                        mine_here |= mine;
                    }
                    PaneContent::Draft => {}
                    PaneContent::Widget(widget) => {
                        pane["widget"] = json!({"plugin": widget.plugin(), "id": widget.id()});
                    }
                }
                pane
            })
            .collect();
        let mut view = json!({
            "window": index + 1,
            "arrangement": window.arrangement().name(),
            "sidebar": open(window.sidebar()),
            "sessionList": open(window.session_list()),
            "showing": window.showing().name(),
            "covered": covered,
            "panes": panes,
        });
        if let WindowShowing::Widget(widget) = window.showing() {
            view["widget"] = json!({"plugin": widget.plugin(), "id": widget.id()});
        }
        views.push((mine_here, view));
    }
    let you = you.unwrap_or_else(|| json!({"threadId": source.to_string(), "shown": false}));
    // Kept in order, the asking conversation's windows first, while they fit.
    let total = views.len();
    let mut windows = Vec::new();
    let mut room = WINDOW_BYTES.saturating_sub(you.to_string().len());
    for (_, view) in views
        .iter()
        .filter(|(mine, _)| *mine)
        .chain(views.iter().filter(|(mine, _)| !*mine))
    {
        let size = view.to_string().len() + 1;
        if size > room {
            continue;
        }
        room -= size;
        windows.push(view.clone());
    }
    windows.sort_by_key(|view| view["window"].as_u64());
    let more = total - windows.len();
    let mut value = json!({"you": you, "windows": windows});
    if more > 0 {
        value["more"] = json!(more);
    }
    value
}
/// What `read_window` lists, in bytes of JSON: room under the 32 KiB of text
/// a tool result may be (`answer`).
const WINDOW_BYTES: usize = 30 * 1024;
fn open(drawn: bool) -> &'static str {
    if drawn {
        "open"
    } else {
        "closed"
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

#[derive(Clone, Copy)]
enum SubscriptionEnd {
    Cancelled,
    Deadline,
}

/// Every awaited observation phase shares source cancellation and a deadline.
/// The preparation budget begins before source admission, not after reader open.
struct SubscriptionControl {
    stop: watch::Receiver<bool>,
    prepare_by: Instant,
    wait_by: Option<Instant>,
    wait_ms: u64,
    idle: Option<CommittedEventCursor>,
}
impl SubscriptionControl {
    fn new(stop: watch::Receiver<bool>, wait_ms: u64) -> Self {
        Self {
            stop,
            prepare_by: Instant::now() + SUBSCRIPTION_SETUP_TIMEOUT,
            wait_by: None,
            wait_ms,
            idle: None,
        }
    }
    async fn step<T>(&mut self, work: impl Future<Output = T>) -> Result<T, SubscriptionEnd> {
        self.step_until(work, self.wait_by.unwrap_or(self.prepare_by))
            .await
    }
    async fn step_until<T>(
        &mut self,
        work: impl Future<Output = T>,
        by: Instant,
    ) -> Result<T, SubscriptionEnd> {
        if *self.stop.borrow() {
            return Err(SubscriptionEnd::Cancelled);
        }
        tokio::select! {
            biased;
            _ = self.stop.changed() => Err(SubscriptionEnd::Cancelled),
            _ = sleep_until(by) => Err(SubscriptionEnd::Deadline),
            result = work => Ok(result),
        }
    }
    fn idle(&mut self, cursor: CommittedEventCursor) {
        self.idle = Some(cursor);
        let wait_ms = self.wait_ms;
        self.wait_by
            .get_or_insert_with(|| Instant::now() + Duration::from_millis(wait_ms));
    }
    fn finish(&self, target: &ConversationId, ended: SubscriptionEnd) -> Result<Value, String> {
        match ended {
            SubscriptionEnd::Cancelled => Err("cancelled".into()),
            SubscriptionEnd::Deadline => match &self.idle {
                Some(cursor) => Ok(
                    json!({"threadId":target.to_string(),"events":[],"cursor":cursor.encode(),"more":false,"timedOut":self.wait_ms>0}),
                ),
                None => Err("event_source_preparing".into()),
            },
        }
    }
}

/// One retained reader per call. Cancellation drops its local future; physical
/// SDK read ownership and admitted source audit delivery retain their own owners.
async fn follow(
    service: &ConversationService,
    target: ConversationId,
    caller: ConversationCaller,
    args: Follow,
    mut control: SubscriptionControl,
) -> Result<Value, String> {
    let value = observe(service, &target, &caller, args, &mut control).await?;
    // This is the publication admission point for Page, Idle, and a retained
    // checkpoint at the observation deadline. An expired wait cannot certify
    // current access; final authorization has a separate bounded attempt.
    match control
        .step_until(
            service.authorize_events(&target, &caller),
            Instant::now() + SUBSCRIPTION_SETUP_TIMEOUT,
        )
        .await
    {
        Ok(result) => result.map_err(code)?,
        Err(SubscriptionEnd::Cancelled) => return Err("cancelled".into()),
        Err(SubscriptionEnd::Deadline) => return Err(code(ConversationError::Unavailable)),
    }
    Ok(value)
}

async fn observe(
    service: &ConversationService,
    target: &ConversationId,
    caller: &ConversationCaller,
    args: Follow,
    control: &mut SubscriptionControl,
) -> Result<Value, String> {
    let limit = args.limit.unwrap_or(DEFAULT_LIMIT);
    let session = SessionId::new(target.to_string()).map_err(|_| "invalid_request")?;
    let start = match args.after {
        Some(token) => EventStart::After(CommittedEventCursor::decode(&session, &token).map_err(
            |error| event_error_code(&ConversationEventError::Events(error)).to_owned(),
        )?),
        None if args.replay.unwrap_or(false) => EventStart::Beginning,
        None => EventStart::Now,
    };
    let mut progress = EventProgress::new(&start);
    let limits =
        EventReadLimits::new(limit.min(16), 256 * 1024 * 1024).map_err(|_| "invalid_request")?;
    let opening = service.open_events(target.clone(), caller.clone(), start);
    let mut reader = match control.step(opening).await {
        Ok(result) => result.map_err(|error| event_error_code(&error).to_owned())?,
        Err(ended) => return control.finish(target, ended),
    };
    loop {
        match control.step(service.authorize_events(target, caller)).await {
            Ok(result) => result.map_err(code)?,
            Err(ended) => return control.finish(target, ended),
        }
        let read = match control.step(reader.read(limits)).await {
            // Every physical read worker is busy: a moment's wait, not a
            // refusal, within this call's own deadline.
            Ok(Err(EventReadError::Capacity)) => {
                if let Err(ended) = control.step(sleep(READ_RETRY)).await {
                    return control.finish(target, ended);
                }
                continue;
            }
            Ok(result) => result.map_err(|error| {
                event_error_code(&ConversationEventError::Events(error)).to_owned()
            })?,
            Err(ended) => return control.finish(target, ended),
        };
        progress
            .accept(&read)
            .map_err(|error| event_error_code(&ConversationEventError::Events(error)).to_owned())?;
        match read {
            EventRead::Preparing => tokio::task::yield_now().await,
            EventRead::Page(page) => {
                validate_event_cursor(target, page.next())
                    .map_err(|error| event_error_code(&error).to_owned())?;
                validate_event_cursor(target, page.captured_head())
                    .map_err(|error| event_error_code(&error).to_owned())?;
                let events = project_events(target, page.events())
                    .map_err(|error| event_error_code(&error).to_owned())?;
                return Ok(
                    json!({"threadId":target.to_string(),"events":events,"cursor":page.next().encode(),"more":page.next()!=page.captured_head(),"timedOut":false}),
                );
            }
            EventRead::Idle(cursor) => {
                validate_event_cursor(target, &cursor)
                    .map_err(|error| event_error_code(&error).to_owned())?;
                control.idle(cursor);
                if Instant::now() >= control.wait_by.expect("idle sets its deadline") {
                    return control.finish(target, SubscriptionEnd::Deadline);
                }
                if let Err(ended) = control.step(sleep(Duration::from_millis(250))).await {
                    return control.finish(target, ended);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Map;

    #[test]
    fn published_tool_arguments_preserve_omission_and_reject_null_and_unpublished_values() {
        fn decode(tool: &str, arguments: Value) -> Result<(), String> {
            match tool {
                "create_thread" => parse::<Create>(arguments).map(|_| ()),
                "send_message" => parse::<Send>(arguments).map(|_| ()),
                "stop_thread" => parse::<Stop>(arguments).map(|_| ()),
                "subscribe" => parse::<Follow>(arguments).map(|_| ()),
                "read_window" => parse::<Window>(arguments).map(|_| ()),
                _ => panic!("uncovered published tool: {tool}"),
            }
        }
        fn example(shape: &Value) -> Value {
            if let Some(value) = shape["enum"].as_array().and_then(|values| values.first()) {
                return value.clone();
            }
            match shape["type"].as_str() {
                Some("string") => json!("valid"),
                Some("boolean") => json!(true),
                Some("integer") => shape.get("minimum").cloned().unwrap_or(json!(0)),
                _ => panic!("uncovered published scalar: {shape}"),
            }
        }
        for tool in ConversationTools::new().tools() {
            let name = tool["name"].as_str().unwrap();
            let schema = &tool["inputSchema"];
            let properties = schema["properties"].as_object().unwrap();
            let required = schema["required"].as_array().cloned().unwrap_or_default();
            let mut base = Map::new();
            for field in &required {
                let field = field.as_str().unwrap();
                base.insert(field.into(), example(&properties[field]));
            }
            assert!(decode(name, Value::Object(base.clone())).is_ok(), "{name}");
            let positional = Value::Array(base.values().cloned().collect());
            for shape in [positional, Value::Null, json!("arguments")] {
                assert_eq!(
                    decode(name, shape.clone()),
                    Err("invalid_request".into()),
                    "non-object {name}: {shape}"
                );
            }
            for (field, shape) in properties {
                let mut arguments = base.clone();
                arguments.insert(field.clone(), example(shape));
                assert!(
                    decode(name, Value::Object(arguments.clone())).is_ok(),
                    "valid {name}.{field}"
                );
                // Serde's map form of a valid value, e.g. {"steering": null}.
                let valid = example(shape);
                let key = valid
                    .as_str()
                    .map_or_else(|| valid.to_string(), str::to_owned);
                arguments.insert(field.clone(), json!({ key: null }));
                assert_eq!(
                    decode(name, Value::Object(arguments.clone())),
                    Err("invalid_request".into()),
                    "object-shaped {name}.{field}"
                );
                if shape.get("enum").is_some() {
                    arguments.insert(field.clone(), json!("not-a-published-value"));
                    assert_eq!(
                        decode(name, Value::Object(arguments.clone())),
                        Err("invalid_request".into()),
                        "unpublished {name}.{field}"
                    );
                }
                arguments.insert(field.clone(), Value::Null);
                assert_eq!(
                    decode(name, Value::Object(arguments.clone())),
                    Err("invalid_request".into()),
                    "null {name}.{field}"
                );
                arguments.remove(field);
                assert_eq!(
                    decode(name, Value::Object(arguments)).is_ok(),
                    !required.iter().any(|value| value.as_str() == Some(field)),
                    "omitted {name}.{field}"
                );
            }
        }
    }

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
            [
                "create_thread",
                "send_message",
                "subscribe",
                "stop_thread",
                "read_window"
            ]
        );
        for tool in tools {
            assert_eq!(tool["inputSchema"]["additionalProperties"], false);
        }
        assert!(parse::<Create>(json!({"ownerId":"other"})).is_err());
        assert!(parse::<Create>(json!({"approvalMode":"full"})).is_err());
        assert!(
            parse::<Send>(json!({"threadId":"target","message":"hello","mode":"unknown"})).is_err()
        );
        assert_eq!(
            parse::<Send>(json!({"threadId":"target","message":"hello"}))
                .unwrap()
                .mode
                .name(),
            "queued"
        );
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

    #[test]
    fn read_window_says_each_pane_in_words_and_where_the_caller_is() {
        use crate::conversation::application::PublishedWindow;
        use crate::conversation::domain::{
            WidgetRef, WindowArrangement, WindowColumn, WindowPane, WorkspaceWindow,
        };
        let me = ConversationId::new("0b3b8f0e-6f43-4a51-9d55-0e7e4f7f8a11").unwrap();
        let other = ConversationId::new("1c4c9f1f-7054-4b62-8e66-1f8f5f8f9b22").unwrap();
        let hidden = ConversationId::new("2d5daf2f-8165-4c73-9f77-2f9f6f9fac33").unwrap();
        let window = WorkspaceWindow::new(
            WindowArrangement::Columns,
            false,
            true,
            WindowShowing::Panes,
            vec![
                WindowColumn::new(
                    40,
                    vec![
                        WindowPane::new(100, false, PaneContent::Conversation(other.clone()))
                            .unwrap(),
                    ],
                )
                .unwrap(),
                WindowColumn::new(
                    60,
                    vec![
                        WindowPane::new(50, true, PaneContent::Conversation(me.clone())).unwrap(),
                        WindowPane::new(25, false, PaneContent::Conversation(hidden.clone()))
                            .unwrap(),
                        WindowPane::new(
                            25,
                            false,
                            PaneContent::Widget(
                                WidgetRef::new("files".into(), "readme".into()).unwrap(),
                            ),
                        )
                        .unwrap(),
                    ],
                )
                .unwrap(),
            ],
        )
        .unwrap();
        let reading = WindowReading {
            windows: vec![PublishedWindow {
                id: "7d9f2a4e-1b3c-4d5e-8f60-718293a4b5c6".into(),
                revision: 3,
                window: Arc::new(window),
            }],
            conversations: std::collections::HashMap::from([
                (
                    me.clone(),
                    PaneConversation::Shown {
                        title: Some("hi".into()),
                        agent: Some("claude"),
                        model: "claude-opus-5".into(),
                    },
                ),
                (
                    other.clone(),
                    PaneConversation::Shown {
                        title: Some("Hey".into()),
                        agent: Some("codex"),
                        model: "gpt-6-astra".into(),
                    },
                ),
                (hidden.clone(), PaneConversation::Unavailable),
            ]),
        };
        let view = window_view(&me, &reading);
        assert_eq!(
            view["you"],
            json!({"threadId": me.to_string(), "shown": true, "window": 1, "pane": 2,
                   "position": "top right", "focused": true, "covered": false})
        );
        let window = &view["windows"][0];
        assert_eq!(window["sidebar"], "closed");
        assert_eq!(window["sessionList"], "open");
        assert_eq!(window["showing"], "panes");
        let panes = window["panes"].as_array().unwrap();
        assert_eq!(panes[0]["position"], "left");
        assert_eq!(panes[0]["title"], "Hey");
        assert_eq!(panes[0]["agent"], "codex");
        assert_eq!(panes[0]["you"], false);
        assert_eq!(panes[0]["widthPercent"], 40);
        assert_eq!(panes[1]["you"], true);
        assert_eq!(panes[2]["position"], "middle right");
        assert_eq!(panes[2]["available"], false);
        assert!(panes[2].get("title").is_none());
        assert_eq!(panes[3]["kind"], "widget");
        assert_eq!(
            panes[3]["widget"],
            json!({"plugin": "files", "id": "readme"})
        );

        let elsewhere = window_view(
            &ConversationId::new("3e6ebf3f-9276-4d84-8f88-3fafafafbd44").unwrap(),
            &reading,
        );
        assert_eq!(elsewhere["you"]["shown"], false);
    }

    #[test]
    fn read_window_names_the_focused_pane_says_what_is_covered_and_fits_one_result() {
        use crate::conversation::application::PublishedWindow;
        use crate::conversation::domain::{
            WindowArrangement, WindowColumn, WindowPane, WorkspaceWindow,
        };
        let me = ConversationId::new("0b3b8f0e-6f43-4a51-9d55-0e7e4f7f8a11").unwrap();
        let shown = |showing, focused| {
            Arc::new(
                WorkspaceWindow::new(
                    WindowArrangement::Columns,
                    true,
                    true,
                    showing,
                    vec![WindowColumn::new(
                        100,
                        vec![
                            WindowPane::new(100, focused, PaneContent::Conversation(me.clone()))
                                .unwrap(),
                        ],
                    )
                    .unwrap()],
                )
                .unwrap(),
            )
        };
        let published = |window| PublishedWindow {
            id: "7d9f2a4e-1b3c-4d5e-8f60-718293a4b5c6".into(),
            revision: 1,
            window,
        };
        // Shown unfocused in the first window, focused under the overview in the second.
        let reading = WindowReading {
            windows: vec![
                published(shown(WindowShowing::Panes, false)),
                published(shown(WindowShowing::Agents, true)),
            ],
            conversations: Default::default(),
        };
        let view = window_view(&me, &reading);
        assert_eq!(view["you"]["window"], 2);
        assert_eq!(view["you"]["covered"], true);
        assert_eq!(view["windows"][1]["covered"], true);
        assert!(view.get("more").is_none());

        // More windows than one result holds: the rest are counted, and the
        // window showing the caller is kept.
        let other = ConversationId::new("1c4c9f1f-7054-4b62-8e66-1f8f5f8f9b22").unwrap();
        let full = |content: &dyn Fn() -> PaneContent| {
            published(Arc::new(
                WorkspaceWindow::new(
                    WindowArrangement::Columns,
                    true,
                    true,
                    WindowShowing::Panes,
                    (0..8)
                        .map(|_| {
                            WindowColumn::new(
                                12,
                                (0..8)
                                    .map(|_| WindowPane::new(12, false, content()).unwrap())
                                    .collect(),
                            )
                            .unwrap()
                        })
                        .collect(),
                )
                .unwrap(),
            ))
        };
        let mut windows: Vec<_> = (0..6)
            .map(|_| full(&|| PaneContent::Conversation(other.clone())))
            .collect();
        windows.push(full(&|| PaneContent::Conversation(me.clone())));
        let view = window_view(
            &me,
            &WindowReading {
                windows,
                conversations: Default::default(),
            },
        );
        let listed = view["windows"].as_array().unwrap();
        assert!(view["more"].as_u64().unwrap() > 0);
        assert_eq!(listed.len() + view["more"].as_u64().unwrap() as usize, 7);
        assert_eq!(view["you"]["window"], 7);
        assert!(listed.iter().any(|window| window["window"] == 7));
        assert_eq!(answer(view)["isError"], false);
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

#[cfg(test)]
#[path = "../../tests/composition/conversation_tools_subscription.rs"]
mod subscription_tests;
