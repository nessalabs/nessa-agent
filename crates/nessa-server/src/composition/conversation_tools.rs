//! Source-bound agent conversation tools over the gateway's existing MCP relay.
use crate::conversation::application::{
    error_code, event_error_code, project_events, validate_event_cursor, AgentToolRequest,
    ConversationCaller, ConversationError, ConversationEventError, ConversationService,
    EventProgress, ReportRegistration, RequestedAgent, RequestedConversation, SourcePermits,
    SubmissionMode, SubmittedMessage, ThreadEvent, ThreadReport, MAX_EVENT_READERS,
};
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
use nessa_sdk::domain::agent_execution::{
    executions::{Initiation, Initiator},
    sessions::SessionId,
};
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
/// The most of a thread's reply one subscribe answer carries, within the
/// answer's own 32 KiB with room for its events.
const MAX_REPLY_BYTES: usize = 16 * 1024;
const REPLY_CUT: &str = "\n[cut to fit]";

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
                "description": "Send plain text in another thread of the same owner. queued (default) starts a queued turn; steering uses the existing steering semantics; passive records a note without starting or steering a turn. When a queued or steering message's turn ends, a report with the thread's outcome and final reply is queued into this conversation as a new turn (reportsBack: true), so there is no need to poll for it: end your turn and the report wakes you. To wait for the reply inside this turn instead, call subscribe once with this threadId and the returned executionId; the report is then not sent. Retry with identical thread, text and mode using the same requestId. Passive notes and active submissions retain separate existing operation namespaces.",
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
                "description": "Wait for a thread's turn, or read its events. To get the reply to a message you sent, pass the executionId send_message returned: the call waits (up to waitMs, default and most 30000) and returns turn: {finished: true, outcome, reply} as soon as that turn ends, even if it already had. If turn.finished is false it is still running: call again the same way, or end your turn and its report wakes you. While waiting it reads past the thread's other events; its cursor continues after everything it read. Without executionId it returns the next committed events (metadata only, never empty while more are waiting) and a cursor; repeat with after to continue, omit after to start from now, or use replay:true to start from the beginning. A turn.ended event comes with turn: {outcome, reply}. This call does not send a message, run an agent, or arrange a future model turn.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "threadId": {"type": "string"},
                        "executionId": {"type": "string", "description": "Wait for this turn's end and return its reply. Mutually exclusive with replay."},
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
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Follow {
    thread_id: String,
    #[serde(default, deserialize_with = "supplied")]
    execution_id: Option<String>,
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
            let storage = service.commands().ok_or("command_unavailable")?;
            let receipt = service
                .create_command(
                    storage,
                    target.clone(),
                    source.caller(),
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
            let submitted = match service
                .submit_command(
                    storage,
                    target.clone(),
                    source.caller(),
                    Initiation::owner(),
                    false,
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
                                &source.caller(),
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
            // Waiting on a turn is the point of naming it: one call should do.
            let wait = args.wait_ms.unwrap_or(if args.execution_id.is_some() {
                MAX_WAIT_MS
            } else {
                0
            });
            let limit = args.limit.unwrap_or(DEFAULT_LIMIT);
            if wait > MAX_WAIT_MS
                || !(1..=MAX_LIMIT).contains(&limit)
                || (args.after.is_some() && args.replay.is_some())
                || (args.execution_id.is_some() && args.replay.is_some())
                || args
                    .execution_id
                    .as_ref()
                    .is_some_and(|execution| execution.is_empty() || execution.len() > 256)
            {
                return Err("invalid_request".into());
            }
            let action = identity(&source, &call, "subscribe");
            let mut control = SubscriptionControl::new(stop, wait, args.execution_id.clone());
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
            // While this call waits on the turn, its reply is this call's
            // to take, and the turn's report holds back.
            let _awaited = args.execution_id.as_ref().map(|execution| {
                service.await_thread_turn(&target, execution, source.conversation())
            });
            follow(
                &service,
                target,
                source.conversation().clone(),
                source.caller(),
                args,
                control,
            )
            .await
        }
        _ => Err("unknown_tool".into()),
    }
}
/// The most one tool answer's JSON text may take.
const MAX_ANSWER_BYTES: usize = 32 * 1024;
fn fits(value: &Value) -> bool {
    serde_json::to_string(value).is_ok_and(|text| text.len() <= MAX_ANSWER_BYTES)
}
fn answer(value: Value) -> Value {
    let text = serde_json::to_string(&value).expect("JSON serializes");
    if text.len() > MAX_ANSWER_BYTES {
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
    /// The furthest point read, where a call ended by its deadline resumes.
    checkpoint: Option<CommittedEventCursor>,
    /// The checkpoint is short of the head: more is waiting.
    behind: bool,
    /// The turn this call waits on, if it names one.
    execution: Option<String>,
}
impl SubscriptionControl {
    fn new(stop: watch::Receiver<bool>, wait_ms: u64, execution: Option<String>) -> Self {
        Self {
            stop,
            prepare_by: Instant::now() + SUBSCRIPTION_SETUP_TIMEOUT,
            wait_by: None,
            wait_ms,
            checkpoint: None,
            behind: false,
            execution,
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
    /// Everything up to `cursor` is read, and the wait for more begins.
    fn idle(&mut self, cursor: CommittedEventCursor) {
        self.checkpoint = Some(cursor);
        self.behind = false;
        let wait_ms = self.wait_ms;
        self.wait_by
            .get_or_insert_with(|| Instant::now() + Duration::from_millis(wait_ms));
    }
    /// Everything up to `cursor` is read, and more is waiting after it.
    fn read_to(&mut self, cursor: CommittedEventCursor) {
        self.checkpoint = Some(cursor);
        self.behind = true;
    }
    fn waited_out(&self) -> bool {
        self.wait_by.is_some_and(|by| Instant::now() >= by)
    }
    fn finish(&self, target: &ConversationId, ended: SubscriptionEnd) -> Result<Value, String> {
        match ended {
            SubscriptionEnd::Cancelled => Err("cancelled".into()),
            SubscriptionEnd::Deadline => match &self.checkpoint {
                Some(cursor) => {
                    let mut value = json!({"threadId":target.to_string(),"events":[],"cursor":cursor.encode(),"more":self.behind,"timedOut":self.wait_ms>0});
                    if let Some(execution) = &self.execution {
                        value["turn"] = json!({"executionId":execution,"finished":false});
                    }
                    Ok(value)
                }
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
    source: ConversationId,
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
    // A call waiting on the turn now holds its whole reply, in an answer
    // that will be sent: a report would only repeat it.
    if let Some(execution) = &control.execution {
        let turn = &value["turn"];
        if turn["finished"] == true && turn["replyCut"] == false && fits(&value) {
            service
                .claim_thread_report(&target, execution, &source)
                .await;
        }
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
    let mut looked = false;
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
                let mut events = project_events(target, page.events())
                    .map_err(|error| event_error_code(&error).to_owned())?;
                let next = page.next().clone();
                let caught_up = page.next() == page.captured_head();
                if let Some(execution) = control.execution.clone() {
                    // Look in its history first, for a turn that already ended;
                    // after that, only when a turn ends or is withdrawn.
                    let ends = !looked
                        || events.iter().any(|event| {
                            event.kind == "turn.ended"
                                || matches!(
                                    event.status,
                                    Some("cancelled" | "settled" | "settlement_updated")
                                )
                        });
                    looked = true;
                    if ends {
                        match control
                            .step(service.thread_turn_end(target, caller, &execution))
                            .await
                        {
                            Ok(Some(end)) => {
                                return Ok(answered(
                                    target,
                                    &events,
                                    &next,
                                    !caught_up,
                                    turn_value(&execution, end),
                                ))
                            }
                            Ok(None) => {}
                            Err(ended) => return control.finish(target, ended),
                        }
                    }
                    if caught_up {
                        control.idle(next);
                    } else {
                        control.read_to(next);
                    }
                    continue;
                }
                // A page of events this projection keeps out (message text,
                // thinking) is read past, never answered as an empty page.
                if events.is_empty() {
                    if !caught_up {
                        control.read_to(next);
                        continue;
                    }
                    control.idle(next);
                    if control.waited_out() {
                        return control.finish(target, SubscriptionEnd::Deadline);
                    }
                    continue;
                }
                // One ended turn per answer, with its reply; the rest follows
                // after its cursor.
                let Some(end) = events.iter().position(|event| event.kind == "turn.ended") else {
                    return Ok(
                        json!({"threadId":target.to_string(),"events":events,"cursor":next.encode(),"more":!caught_up,"timedOut":false}),
                    );
                };
                let more = end + 1 < events.len() || !caught_up;
                events.truncate(end + 1);
                let ended = &events[end];
                let execution = ended.execution_id.clone().unwrap_or_default();
                // Bounded on its own, as final authorization is: the page
                // is read, so a deadline here is no idle checkpoint.
                let reply = match control
                    .step_until(
                        service.thread_turn_end(target, caller, &execution),
                        Instant::now() + SUBSCRIPTION_SETUP_TIMEOUT,
                    )
                    .await
                {
                    Ok(found) => found,
                    Err(SubscriptionEnd::Cancelled) => return Err("cancelled".into()),
                    Err(SubscriptionEnd::Deadline) => {
                        return Err(code(ConversationError::Unavailable))
                    }
                };
                let outcome = ended.outcome.unwrap_or("completed");
                let turn = turn_value(&execution, reply.unwrap_or((outcome, None)));
                return Ok(
                    json!({"threadId":target.to_string(),"events":events,"cursor":ended.cursor,"more":more,"timedOut":false,"turn":turn}),
                );
            }
            EventRead::Idle(cursor) => {
                validate_event_cursor(target, &cursor)
                    .map_err(|error| event_error_code(&error).to_owned())?;
                if let Some(execution) = control.execution.clone() {
                    if !looked {
                        looked = true;
                        match control
                            .step(service.thread_turn_end(target, caller, &execution))
                            .await
                        {
                            Ok(Some(end)) => {
                                return Ok(answered(
                                    target,
                                    &[],
                                    &cursor,
                                    false,
                                    turn_value(&execution, end),
                                ))
                            }
                            Ok(None) => {}
                            Err(ended) => return control.finish(target, ended),
                        }
                    }
                }
                control.idle(cursor);
                if control.waited_out() {
                    return control.finish(target, SubscriptionEnd::Deadline);
                }
                if let Err(ended) = control.step(sleep(Duration::from_millis(250))).await {
                    return control.finish(target, ended);
                }
            }
        }
    }
}

fn answered(
    target: &ConversationId,
    events: &[ThreadEvent],
    cursor: &CommittedEventCursor,
    more: bool,
    turn: Value,
) -> Value {
    json!({"threadId":target.to_string(),"events":events,"cursor":cursor.encode(),"more":more,"timedOut":false,"turn":turn})
}

/// An ended turn as an agent reads it: its outcome and final reply, cut so
/// the reply as written in the answer's JSON fits `MAX_REPLY_BYTES`.
fn turn_value(execution: &str, (outcome, reply): (&'static str, Option<String>)) -> Value {
    let written = |text: &str| serde_json::to_string(text).map_or(usize::MAX, |json| json.len());
    let (reply, cut) = match reply {
        Some(mut reply) if written(&reply) > MAX_REPLY_BYTES => {
            while written(&reply) + REPLY_CUT.len() > MAX_REPLY_BYTES {
                let over = written(&reply) + REPLY_CUT.len() - MAX_REPLY_BYTES;
                let mut end = reply.len().saturating_sub(over.max(1));
                while !reply.is_char_boundary(end) {
                    end -= 1;
                }
                reply.truncate(end);
            }
            reply.push_str(REPLY_CUT);
            (Some(reply), true)
        }
        reply => (reply, false),
    };
    json!({"executionId":execution,"finished":true,"outcome":outcome,"reply":reply,"replyCut":cut})
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Map;

    #[test]
    fn a_long_reply_is_cut_so_it_fits_as_written_in_the_answer() {
        for reply in [
            "x".repeat(40_000),
            "\n".repeat(40_000),
            "é\"".repeat(20_000),
        ] {
            let turn = turn_value("run", ("completed", Some(reply.clone())));
            let carried = turn["reply"].as_str().unwrap();
            assert!(
                serde_json::to_string(carried).unwrap().len()
                    <= MAX_REPLY_BYTES + REPLY_CUT.len() * 2
            );
            assert!(carried.ends_with(REPLY_CUT));
            assert_eq!(turn["replyCut"], true);
            assert!(answer(json!({"turn": turn}))["isError"] == false);
        }
        let short = turn_value("run", ("completed", Some("hi".into())));
        assert_eq!(short["reply"], "hi");
        assert_eq!(short["replyCut"], false);
    }

    #[test]
    fn published_tool_arguments_preserve_omission_and_reject_null_and_unpublished_values() {
        fn decode(tool: &str, arguments: Value) -> Result<(), String> {
            match tool {
                "create_thread" => parse::<Create>(arguments).map(|_| ()),
                "send_message" => parse::<Send>(arguments).map(|_| ()),
                "stop_thread" => parse::<Stop>(arguments).map(|_| ()),
                "subscribe" => parse::<Follow>(arguments).map(|_| ()),
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
            ["create_thread", "send_message", "subscribe", "stop_thread"]
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
