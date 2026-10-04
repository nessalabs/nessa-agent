//! An MCP App's calls (#348): `mcp.callTool`, `mcp.readResource` and
//! `mcp.releaseApp`, through the conversation's own MCP sessions; and its
//! messages and model context (#390): `mcp.sendMessage`, submitted as the
//! person's turn written by the app, and `mcp.updateModelContext`, held for
//! the next message admitted while the conversation is idle. The rules are
//! `mcp_servers::domain::app_call`'s and the SDK's; this is their flow —
//! every step audited before its effect is reported, a destructive call and
//! every message held on a review the person answers, and each refusal one
//! protocol code (`docs/design/mcp-app-calls.md`).
//!
//! Each call runs on a task of its own, so that a caller going away cannot
//! leave a step unrecorded: a call already sent finishes and is recorded, and
//! a review still waiting is withdrawn and recorded as withdrawn. Nothing is
//! opened or issued for a mount released or a conversation ended: the
//! conversation's [`AppReviews`](super::super::app_reviews::AppReviews) lock
//! decides it.
use super::super::app_reviews::{
    new_review_id, AppReviews, ContextRefusal, ReviewAnswerer, ReviewAsk, ReviewEnd, ReviewRefusal,
    APP_REVIEW_DEADLINE,
};
use super::super::mcp_apps::{
    ContextDrop, HeldResource, McpAppAsk, McpAppAuditPhase, McpAppAuditRecord, McpAppCode,
    McpAppError, McpAppFailure, McpAppInitiator, McpAppOutcome, McpAppPorts, McpAppRef,
    McpAppWithdrawal, TicketRefusal,
};
use super::super::projection::{bound_view, bound_view_within, MAX_VIEW_BYTES};
use super::super::session_key::conversation_session;
use super::super::view::{ConversationPermission, ConversationTranscriptState, ConversationView};
use super::super::SubmittedMessage;
use super::{
    ConversationCaller, ConversationError, ConversationService, LiveConversation, Reach,
    SubmissionMode, SubmitFailure, Writer,
};
use crate::conversation::application::error_code;
use crate::conversation::domain::ConversationId;
use crate::mcp_servers::domain::{
    admit_app, admit_tool_call, AppCallAdmission, AppFacts, AppRefusal, ResourceTicketDigest,
    MAX_APP_RESULT_BYTES,
};
use nessa_sdk::domain::agent_execution::{
    executions::ExecutionId,
    prompts::{AppModelContext, McpAppSource},
    sessions::SessionId,
    tools::{McpTool, ToolCallId},
    ExecutionError,
};
use nessa_sdk::domain::common::value_objects::Sha256Digest;
use nessa_sdk::domain::mcp_apps::{UiCsp, UiPermissions, UiResourceUri};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{fmt, sync::Arc};
use tokio::sync::{oneshot, OwnedSemaphorePermit};
use uuid::Uuid;

/// The most MCP App calls running at once, across every caller. Past it a
/// call is refused `temporarily_unavailable` before anything is asked.
pub const MAX_APP_CALLS: usize = 32;
/// The most a resource's URI, CSP and domain may take, encoded: what a
/// `mcp.readResource` answer carries besides fixed-size fields, with room
/// left for those within one socket message.
pub const MAX_RESOURCE_META_BYTES: usize = 48 * 1024;

/// An app's `mcp.callTool`.
#[derive(Clone, Debug)]
pub struct McpAppCall {
    pub app: McpAppRef,
    pub server: String,
    pub tool: String,
    /// One JSON object, encoded; `None` is none.
    pub arguments_json: Option<String>,
}

/// An app's `mcp.readResource`.
#[derive(Clone, Debug)]
pub struct McpAppRead {
    pub app: McpAppRef,
    pub server: String,
    pub uri: String,
}

/// An app's `mcp.sendMessage` (`ui/message`): text for its conversation, as
/// the person.
#[derive(Clone, Debug)]
pub struct McpAppMessage {
    pub app: McpAppRef,
    pub server: String,
    pub text: String,
}

/// An app's `mcp.updateModelContext` (`ui/update-model-context`): what it
/// gives the model now. Neither part is no context: what it held is cleared.
#[derive(Clone, Debug)]
pub struct McpAppContextUpdate {
    pub app: McpAppRef,
    pub server: String,
    pub text: Option<String>,
    /// One JSON object, encoded.
    pub structured_content_json: Option<String>,
}

/// A resource read for an app: what its bytes are, and the ticket that
/// redeems them once.
#[derive(Clone)]
pub struct McpAppResource {
    pub uri: String,
    pub size: usize,
    /// Lowercase hex SHA-256 of the bytes.
    pub sha256: String,
    /// A secret: never logged, and left out of `Debug`.
    pub ticket: String,
    pub csp: UiCsp,
    pub permissions: UiPermissions,
    pub domain: Option<String>,
    pub prefers_border: Option<bool>,
}
impl fmt::Debug for McpAppResource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpAppResource")
            .field("uri", &self.uri)
            .field("size", &self.size)
            .field("sha256", &self.sha256)
            .field("ticket", &"<redacted>")
            .field("csp", &self.csp)
            .field("permissions", &self.permissions)
            .field("domain", &self.domain)
            .field("prefers_border", &self.prefers_border)
            .finish()
    }
}

impl ConversationService {
    /// Call `call.tool` on the app's own server, as the conversation's own
    /// session: the server's `CallToolResult`, re-encoded. A destructive
    /// tool waits for the person's answer to a review first.
    pub async fn call_app_tool(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
        call: McpAppCall,
    ) -> Result<String, ConversationError> {
        let running = self.app_call_permit()?;
        // Dropped with this future: the caller went.
        let (_present, gone) = oneshot::channel::<()>();
        let service = self.clone();
        tokio::spawn(async move {
            let _running = running;
            service.app_tool_call(id, caller, call, gone).await
        })
        .await
        .map_err(|_| ConversationError::Unavailable)?
    }

    /// Read the app resource `read.uri` of the app's own server, and hold
    /// its bytes behind a ticket.
    pub async fn read_app_resource(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
        read: McpAppRead,
    ) -> Result<McpAppResource, ConversationError> {
        let running = self.app_call_permit()?;
        let service = self.clone();
        tokio::spawn(async move {
            let _running = running;
            service.app_resource_read(id, caller, read).await
        })
        .await
        .map_err(|_| ConversationError::Unavailable)?
    }

    /// Send `message.text` into the conversation as the person's turn,
    /// written by the app: the identity of the turn it became. Each message
    /// waits for the person to allow it, unless it is a retry of a turn the
    /// agent has already; one while a turn runs or input waits is refused
    /// `turn_running`.
    pub async fn send_app_message(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
        message: McpAppMessage,
    ) -> Result<String, ConversationError> {
        let running = self.app_call_permit()?;
        // Dropped with this future: the caller went.
        let (_present, gone) = oneshot::channel::<()>();
        let service = self.clone();
        tokio::spawn(async move {
            let _running = running;
            service.app_message(id, caller, message, gone).await
        })
        .await
        .map_err(|_| ConversationError::Unavailable)?
    }

    /// Hold what the app gives the model now, in place of what it gave
    /// before, until the next message admitted while the conversation is
    /// idle carries it; or clear it.
    pub async fn update_app_model_context(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
        update: McpAppContextUpdate,
    ) -> Result<(), ConversationError> {
        let running = self.app_call_permit()?;
        let service = self.clone();
        tokio::spawn(async move {
            let _running = running;
            service.app_model_context(id, caller, update).await
        })
        .await
        .map_err(|_| ConversationError::Unavailable)?
    }

    /// The caller tore the mount `app` down: withdraw its open reviews
    /// (their calls answer `mcp_cancelled`), let go of what was held for it,
    /// and open or issue nothing for it again — each recorded as the
    /// caller's. Idempotent; it never opens the conversation. A context it
    /// dropped whose drop cannot be recorded is dropped all the same, and the
    /// release answers [`ConversationError::Audit`]
    /// (`c14_a_drop_that_cannot_be_recorded_is_dropped_and_the_release_says_so`).
    pub async fn release_app(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
        app: McpAppRef,
    ) -> Result<(), ConversationError> {
        let _admission = self.admit().await?;
        caller.actor()?;
        self.check_view_access(&id, &caller).await?;
        let by = person(&super::answerer(&caller));
        // The conversation's apps, open or not: a release before the
        // conversation is open, or while it opens, is kept all the same.
        let tickets = self.inner.mcp_apps.as_ref().map(|ports| &ports.tickets);
        let dropped = self.apps_of(&id).release_app(&app, &by, || {
            if let Some(tickets) = tickets {
                tickets.release_app(&id, &app, &by);
            }
        });
        self.record_dropped(Dropped {
            updates: dropped,
            cause: ContextDrop::Released,
            by,
        })
        .await
    }

    /// Record each context `dropped` held as dropped unsent, against the
    /// record of the update that gave it. Each is dropped already, so a
    /// record that cannot be written does not stop the rest, or anything
    /// else: it is logged, the next is tried, and the answer says one failed.
    pub(super) async fn record_dropped(&self, dropped: Dropped) -> Result<(), ConversationError> {
        let Dropped { updates, cause, by } = dropped;
        let Some(ports) = &self.inner.mcp_apps else {
            return Ok(());
        };
        let mut recorded = Ok(());
        for update in updates {
            let record = McpAppAuditRecord {
                phase: McpAppAuditPhase::ContextDropped { cause },
                initiator: by.clone(),
                ..update
            };
            let (conversation_id, call_id) =
                (record.conversation_id.clone(), record.call_id.clone());
            if let Err(error) = ports.audit.record(record).await {
                tracing::error!(
                    %conversation_id,
                    %call_id,
                    ?cause,
                    ?error,
                    "an MCP App context's drop could not be audited; it is dropped all the same"
                );
                recorded = Err(error);
            }
        }
        recorded
    }

    /// Whether the conversation `id`, live now, has the turn `execution`
    /// already. Nothing is opened to answer it; a conversation not live
    /// answers no, and its message is asked about. (The app's call may have
    /// opened the conversation already, as #348's calls do; M10 holds only
    /// that a message admitted in one opening is not sent into another.)
    async fn holds_turn(&self, id: &ConversationId, execution: &str) -> bool {
        let slot = self.inner.conversations.lock().await.get(id).cloned();
        let Some(Ok(live)) = slot.as_ref().and_then(|slot| slot.value.get()) else {
            return false;
        };
        live.agent
            .session_manager()
            .snapshot()
            .await
            .is_some_and(|snapshot| {
                snapshot
                    .invocations
                    .iter()
                    .any(|record| record.request.execution_id.as_str() == execution)
            })
    }

    /// The conversation `id`'s apps, kept for it until it is deleted.
    pub(super) fn apps_of(&self, id: &ConversationId) -> Arc<AppReviews> {
        self.inner
            .apps
            .lock()
            .expect("conversations' apps")
            .entry(id.clone())
            .or_default()
            .clone()
    }

    /// The conversation `id` was deleted, and its agent's stop tried: its
    /// apps take no more work, ever, and keep nothing. Kept as that — made
    /// so if it had none in this run — not removed, so that a release or an
    /// opening racing the delete finds them deleted, and cannot build them
    /// afresh. The contexts it drops, `by` the person who deleted it.
    pub(super) fn close_apps_for_good(&self, id: &ConversationId, by: &McpAppInitiator) -> Dropped {
        let updates = self.apps_of(id).delete(|| {
            if let Some(ports) = &self.inner.mcp_apps {
                ports
                    .tickets
                    .release_conversation(id, &McpAppInitiator::System);
            }
        });
        Dropped {
            updates,
            cause: ContextDrop::ConversationEnded,
            by: by.clone(),
        }
    }

    /// `by` ended `live`'s conversation: withdraw its apps' reviews, let go
    /// of what was held for them, and open or issue nothing for them again.
    /// Idempotent. The contexts it drops are the caller's to record
    /// ([`Self::record_dropped`]), once whatever it is bounded by is done.
    pub(super) fn end_apps(
        &self,
        id: &ConversationId,
        live: &LiveConversation,
        by: &McpAppInitiator,
    ) -> Dropped {
        let updates = live.app_reviews.end(live.app_epoch, by, || {
            if let Some(ports) = &self.inner.mcp_apps {
                ports.tickets.release_conversation(id, by);
            }
        });
        Dropped {
            updates,
            cause: ContextDrop::ConversationEnded,
            by: by.clone(),
        }
    }

    /// One of the [`MAX_APP_CALLS`], for a call's task to hold until it ends.
    fn app_call_permit(&self) -> Result<OwnedSemaphorePermit, ConversationError> {
        self.inner
            .app_calls
            .clone()
            .try_acquire_owned()
            .map_err(|_| ConversationError::Unavailable)
    }

    /// Wait, up to `budget`, for every app call's task to end, so that each
    /// has recorded its last step: whether they all did.
    pub(super) async fn app_calls_finished(&self, budget: std::time::Duration) -> bool {
        let all = u32::try_from(MAX_APP_CALLS).expect("a small bound");
        tokio::time::timeout(budget, self.inner.app_calls.acquire_many(all))
            .await
            .is_ok_and(|permits| permits.is_ok())
    }

    async fn app_tool_call(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
        call: McpAppCall,
        mut gone: oneshot::Receiver<()>,
    ) -> Result<String, ConversationError> {
        let (opening, facts, ports) = self.app_opening(&id, &caller, &call.app).await?;
        let session = conversation_session(&id);
        let step = Step::new(
            &ports,
            &id,
            &caller,
            &call.app,
            McpAppAsk::CallTool {
                server: call.server.clone(),
                tool: call.tool.clone(),
            },
        );
        let arguments_text = call.arguments_json.as_deref();
        let admission = match admitted(&ports, &session, facts.as_ref(), &call, arguments_text) {
            Ok(admission) => admission,
            Err(error) => return Err(step.refuse(error).await),
        };
        if opening.admit(&call.app).is_err() {
            return Err(step.refuse_by_system(McpAppError::Cancelled).await);
        }
        let arguments = match arguments_text.map(serde_json::from_str::<Value>) {
            None => None,
            Some(Ok(value @ Value::Object(_))) => Some(value),
            Some(_) => {
                return Err(step
                    .refuse_as(McpAppCode::InvalidRequest, ConversationError::InvalidInput)
                    .await)
            }
        };
        match admission {
            AppCallAdmission::Send => step.record(McpAppAuditPhase::Admitted, None).await?,
            AppCallAdmission::Approve => {
                // What the person is shown is what is sent: the arguments as
                // parsed, encoded once — a duplicate key or a number past
                // what JSON numbers keep is shown as it will be sent.
                let shown = arguments
                    .as_ref()
                    .map_or_else(|| "{}".to_owned(), Value::to_string);
                let asked = Asked {
                    ask: ReviewAsk::RunTool,
                    app: &call.app,
                    server: &call.server,
                    tool: &call.tool,
                    shown: &shown,
                };
                self.approved(&opening, &step, asked, &mut gone).await?;
                // Allowed: the tool must still be what the person allowed.
                match admitted(&ports, &session, facts.as_ref(), &call, arguments_text) {
                    Ok(AppCallAdmission::Approve) => {}
                    Ok(AppCallAdmission::Send) => {
                        return Err(step.refuse(McpAppError::ToolNotForApp).await)
                    }
                    Err(error) => return Err(step.refuse(error).await),
                }
            }
        }
        // Checked once more just before it is handed to the session: the
        // mount released or the opening ended since it was admitted, and it
        // is not sent. One that lands after this finds it sent — it may wait
        // for room in the session's queue first — but never to a later
        // opening's session, which is chosen before that wait.
        if opening.admit(&call.app).is_err() {
            return Err(step.refuse_by_system(McpAppError::Cancelled).await);
        }
        let answer = ports
            .apps
            .call_tool(&session, &call.server, &call.tool, arguments)
            .await;
        let (outcome, answer) = match answer {
            // Too busy to take it: nothing was sent.
            Err(McpAppFailure::Busy) => return Err(step.refuse(McpAppError::Busy).await),
            Err(failure) => {
                let error = McpAppError::from(failure);
                (McpAppOutcome::Failed(error.code()), Err(error))
            }
            Ok(value) => {
                let text = value.to_string();
                // Measured as the wire carries it: one JSON string, every
                // quote and backslash in it escaped again.
                let carried =
                    serde_json::to_string(&text).map_or(usize::MAX, |carried| carried.len());
                if carried > MAX_APP_RESULT_BYTES {
                    let error = McpAppError::ResultTooLarge;
                    (McpAppOutcome::Failed(error.code()), Err(error))
                } else {
                    let is_error = value.get("isError").and_then(Value::as_bool) == Some(true);
                    let bytes = text.len();
                    (McpAppOutcome::Answered { is_error, bytes }, Ok(text))
                }
            }
        };
        step.record(McpAppAuditPhase::Completed(outcome), None)
            .await?;
        answer.map_err(ConversationError::McpApp)
    }

    /// Ask the person, and wait for their answer: `Ok` once they allowed it.
    async fn approved(
        &self,
        opening: &Opening,
        step: &Step,
        asked: Asked<'_>,
        gone: &mut oneshot::Receiver<()>,
    ) -> Result<(), ConversationError> {
        let permission_id = new_review_id();
        if !super::super::app_reviews::fits(
            &permission_id,
            asked.ask,
            asked.app,
            asked.server,
            asked.tool,
            asked.shown,
        ) {
            return Err(step.refuse(McpAppError::RequestTooLarge).await);
        }
        step.record(
            McpAppAuditPhase::ApprovalRequested {
                permission_id: permission_id.clone(),
            },
            None,
        )
        .await?;
        let waiting = match opening.apps.open(
            opening.epoch,
            permission_id.clone(),
            asked.ask,
            asked.app,
            asked.server,
            asked.tool,
            asked.shown,
        ) {
            Ok(waiting) => waiting,
            Err(refusal) => {
                // Asked for and never shown: on record as what ended it.
                let (cause, error) = match refusal {
                    ReviewRefusal::Ended => (
                        McpAppWithdrawal::ConversationEnded,
                        ConversationError::McpApp(McpAppError::Cancelled),
                    ),
                    ReviewRefusal::Released => (
                        McpAppWithdrawal::AppTornDown,
                        ConversationError::McpApp(McpAppError::Cancelled),
                    ),
                    ReviewRefusal::Full | ReviewRefusal::TooLarge => (
                        McpAppWithdrawal::RequestCancelled,
                        ConversationError::Unavailable,
                    ),
                };
                step.record(
                    McpAppAuditPhase::Withdrawn {
                        permission_id,
                        cause,
                    },
                    Some(McpAppInitiator::System),
                )
                .await?;
                return Err(error);
            }
        };
        let ended = waiting.ended(APP_REVIEW_DEADLINE);
        tokio::pin!(ended);
        let end = tokio::select! {
            end = &mut ended => end,
            _ = &mut *gone => {
                // The caller went. Withdraw it, then take how it actually
                // ended: an answer that came first is what it ended with.
                opening.apps.withdraw(&permission_id);
                ended.await
            }
        };
        let (phase, initiator, error) = match end {
            ReviewEnd::Allowed(by) => (
                McpAppAuditPhase::Approved { permission_id },
                person(&by),
                None,
            ),
            ReviewEnd::Denied(by) => (
                McpAppAuditPhase::Denied { permission_id },
                person(&by),
                Some(McpAppError::ApprovalDenied),
            ),
            ReviewEnd::Expired => (
                McpAppAuditPhase::Expired { permission_id },
                McpAppInitiator::System,
                Some(McpAppError::ApprovalExpired),
            ),
            ReviewEnd::Withdrawn { cause, by } => (
                McpAppAuditPhase::Withdrawn {
                    permission_id,
                    cause,
                },
                by.unwrap_or_else(|| step.app_initiator()),
                Some(McpAppError::Cancelled),
            ),
        };
        step.record(phase, Some(initiator)).await?;
        error.map_or(Ok(()), |error| Err(ConversationError::McpApp(error)))
    }

    async fn app_message(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
        message: McpAppMessage,
        mut gone: oneshot::Receiver<()>,
    ) -> Result<String, ConversationError> {
        let (opening, seen, ports) = self.app_in_conversation(&id, &caller, &message.app).await?;
        let step = Step::new(
            &ports,
            &id,
            &caller,
            &message.app,
            McpAppAsk::SendMessage {
                server: message.server.clone(),
            },
        );
        if let Err(refusal) = admit_app(seen.as_ref().map(|(facts, _)| facts), &message.server) {
            return Err(step.refuse(refusal.into()).await);
        }
        let Some((_, tool)) = seen else {
            return Err(step.refuse(McpAppError::AppUnknown).await);
        };
        if message.text.trim().is_empty() {
            return Err(step
                .refuse_as(McpAppCode::InvalidRequest, ConversationError::InvalidInput)
                .await);
        }
        if message.text.len() > self.inner.limits.max_input_bytes {
            return Err(step.refuse(McpAppError::RequestTooLarge).await);
        }
        let Ok(sender) = app_source(&message.app, tool) else {
            return Err(step
                .refuse_as(McpAppCode::InvalidRequest, ConversationError::InvalidInput)
                .await);
        };
        // The same request again is the same turn: one the agent has already
        // is the agent's to settle, and nobody is asked again to send it — a
        // denial now could not take it back
        // (`m6_a_retry_of_a_turn_the_agent_has_is_not_asked_again`).
        let execution_id = message_execution(&id, &message.app, &caller.action_id);
        if opening.admit(&message.app).is_err() {
            return Err(step.refuse_by_system(McpAppError::Cancelled).await);
        }
        // One request at a time: while the same one is in review or being
        // sent, it is not asked about twice
        // (`m6b_the_same_request_while_it_is_in_flight_is_refused`). Free again
        // however this call ends.
        let Some(_in_flight) = opening.apps.start_message(&execution_id) else {
            return Err(step
                .ended(
                    McpAppAuditPhase::Refused(McpAppCode::TemporarilyUnavailable),
                    Some(McpAppInitiator::System),
                    ConversationError::Unavailable,
                )
                .await);
        };
        if self.holds_turn(&id, &execution_id).await {
            step.record(McpAppAuditPhase::Admitted, None).await?;
        } else {
            // Every message asks. What the person is shown is what is sent.
            let shown = serde_json::json!({ "text": message.text }).to_string();
            let asked = Asked {
                ask: ReviewAsk::SendMessage,
                app: &message.app,
                server: &message.server,
                tool: sender.tool().tool(),
                shown: &shown,
            };
            self.approved(&opening, &step, asked, &mut gone).await?;
        }
        // Checked once more under the conversation's submission lock, just
        // before it is enqueued (`submit_as`): a close or a delete that took
        // the lock first, a release, another opening, or the gateway stopping
        // refuses it there.
        let submitted = self
            .submit_as(
                id,
                caller,
                execution_id.clone(),
                SubmittedMessage {
                    text: message.text,
                    images: Vec::new(),
                    files: Vec::new(),
                },
                SubmissionMode::Queue,
                Writer::App {
                    sender,
                    apps: opening.apps.clone(),
                    epoch: opening.epoch,
                    mount: message.app.clone(),
                },
            )
            .await;
        let sent = |code| McpAppAuditPhase::MessageSent {
            execution_id: execution_id.clone(),
            code,
        };
        let not_sent = |error: &ConversationError| McpAppAuditPhase::MessageNotSent {
            execution_id: execution_id.clone(),
            code: error_code(error),
        };
        let unresolved = |error: &ConversationError| McpAppAuditPhase::MessageUnresolved {
            execution_id: execution_id.clone(),
            code: error_code(error),
        };
        let failure = match submitted {
            Ok(_) => {
                step.record(sent(None), None).await?;
                return Ok(execution_id);
            }
            Err(failure) => failure,
        };
        match failure {
            SubmitFailure::NotAsked(ConversationError::TurnRunning) => {
                return Err(step
                    .refuse_as(McpAppCode::TurnRunning, ConversationError::TurnRunning)
                    .await)
            }
            // Released, ended, deleted, reopened, or the gateway retiring,
            // before the agent was asked to take it: by that other command,
            // so the system's; nothing was sent (row M10). An opening that
            // failed is not one of these: it is the conversation's refusal
            // (M13), whatever it answers.
            SubmitFailure::Retired
            | SubmitFailure::NotAsked(
                ConversationError::McpApp(McpAppError::Cancelled) | ConversationError::Deleted,
            ) => return Err(step.refuse_by_system(McpAppError::Cancelled).await),
            _ => {}
        }
        // A task that failed is nobody's command: the system's.
        let by =
            matches!(failure, SubmitFailure::TaskFailed { .. }).then_some(McpAppInitiator::System);
        let reach = failure.reach();
        let error = failure.into_error();
        match reach {
            // Refused, or failed, before the agent was asked, or refused by
            // it: nothing reached it, and the answer says why (row M13).
            Reach::NotTaken => Err(step.ended(not_sent(&error), by, error).await),
            // Whether the agent has it is not known: it could not say, or the
            // submission's own task failed once it was asked. Said so, not
            // guessed (row M15).
            Reach::Unknown => Err(step.ended(unresolved(&error), by, error).await),
            // The agent has it; what failed is its evidence, which the record
            // and the answer say — unless its own record cannot be written
            // either, when the answer is that, and the evidence's failure is
            // kept in the log rather than lost (row M14).
            Reach::Taken => match step.record(sent(Some(error_code(&error))), by).await {
                Ok(()) => Err(error),
                Err(audit) => {
                    tracing::error!(
                        ?error,
                        "an app's message was taken without its evidence, and its own record could not be written"
                    );
                    Err(audit)
                }
            },
        }
    }

    async fn app_model_context(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
        update: McpAppContextUpdate,
    ) -> Result<(), ConversationError> {
        let (opening, seen, ports) = self.app_in_conversation(&id, &caller, &update.app).await?;
        let step = Step::new(
            &ports,
            &id,
            &caller,
            &update.app,
            McpAppAsk::UpdateModelContext {
                server: update.server.clone(),
            },
        );
        if let Err(refusal) = admit_app(seen.as_ref().map(|(facts, _)| facts), &update.server) {
            return Err(step.refuse(refusal.into()).await);
        }
        let Some((_, tool)) = seen else {
            return Err(step.refuse(McpAppError::AppUnknown).await);
        };
        // Held as given: the context itself is the one judge of what may be
        // held — one JSON object, within its bound, something at all.
        let structured = update.structured_content_json;
        let Ok(source) = app_source(&update.app, tool) else {
            return Err(step
                .refuse_as(McpAppCode::InvalidRequest, ConversationError::InvalidInput)
                .await);
        };
        // `None`: neither part, so what the mount held is cleared.
        let context = match AppModelContext::new(source, step.call_id(), update.text, structured) {
            Ok(context) => context,
            Err(ExecutionError::ValueTooLong { .. }) => {
                return Err(step.refuse(McpAppError::RequestTooLarge).await)
            }
            Err(_) => {
                return Err(step
                    .refuse_as(McpAppCode::InvalidRequest, ConversationError::InvalidInput)
                    .await)
            }
        };
        // One update of the conversation at a time, from its room to its
        // hold: so the order they are recorded in is the order they are held
        // in, and nothing takes the room this one was given
        // (`c8_two_updates_at_once_are_recorded_in_the_order_they_are_held`).
        let _one = opening.apps.one_update().await;
        match opening
            .apps
            .room(opening.epoch, &update.app, context.is_some())
        {
            Ok(()) => {}
            Err(ContextRefusal::Full) => {
                return Err(step
                    .refuse_as(
                        McpAppCode::TemporarilyUnavailable,
                        ConversationError::Unavailable,
                    )
                    .await)
            }
            Err(ContextRefusal::Gone(_)) => {
                return Err(step.refuse_by_system(McpAppError::Cancelled).await)
            }
        }
        let phase = match &context {
            Some(context) => McpAppAuditPhase::ContextHeld {
                bytes: context.content_bytes(),
            },
            None => McpAppAuditPhase::ContextCleared,
        };
        // On record before it is held: a context nobody can account for
        // never reaches the model, and one that could not be recorded
        // changes nothing (`c16_an_update_that_cannot_be_recorded_changes_nothing`).
        step.record(phase, None).await?;
        // A release or an end since came after it, and it is not held: that
        // is on record too, as the other command's
        // (`c17_a_release_between_the_record_and_the_hold_holds_nothing`).
        if opening
            .apps
            .hold(opening.epoch, &update.app, context, &step.record)
            .is_err()
        {
            step.record(
                McpAppAuditPhase::ContextDropped {
                    cause: ContextDrop::NotHeld,
                },
                Some(McpAppInitiator::System),
            )
            .await?;
        }
        Ok(())
    }

    async fn app_resource_read(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
        read: McpAppRead,
    ) -> Result<McpAppResource, ConversationError> {
        let (opening, facts, ports) = self.app_opening(&id, &caller, &read.app).await?;
        let session = conversation_session(&id);
        let step = Step::new(
            &ports,
            &id,
            &caller,
            &read.app,
            McpAppAsk::ReadResource {
                server: read.server.clone(),
                uri: read.uri.clone(),
            },
        );
        if let Err(refusal) = admit_app(facts.as_ref(), &read.server) {
            return Err(step.refuse(refusal.into()).await);
        }
        let Ok(uri) = UiResourceUri::new(read.uri.as_str()) else {
            return Err(step
                .refuse_as(McpAppCode::InvalidRequest, ConversationError::InvalidInput)
                .await);
        };
        if opening.admit(&read.app).is_err() {
            return Err(step.refuse_by_system(McpAppError::Cancelled).await);
        }
        step.record(McpAppAuditPhase::Admitted, None).await?;
        // Just before it is handed to the session, once more: see
        // `app_tool_call`.
        if opening.admit(&read.app).is_err() {
            return Err(step.refuse_by_system(McpAppError::Cancelled).await);
        }
        let resource = match ports.apps.read_resource(&session, &read.server, &uri).await {
            Ok(resource) => resource,
            // Too busy to take it: nothing was sent.
            Err(McpAppFailure::Busy) => return Err(step.refuse(McpAppError::Busy).await),
            Err(failure) => {
                let error = McpAppError::from(failure);
                return Err(step
                    .fail(error.code(), ConversationError::McpApp(error))
                    .await);
            }
        };
        // What the answer carries of the resource besides its ticket — its
        // URI, CSP and domain — must leave room in the response for the rest.
        let carried = serde_json::to_vec(&serde_json::json!({
            "uri": uri.as_str(),
            "connect": resource.csp().connect_domains(),
            "resource": resource.csp().resource_domains(),
            "frame": resource.csp().frame_domains(),
            "baseUri": resource.csp().base_uri_domains(),
            "domain": resource.domain(),
        }))
        .map_or(usize::MAX, |carried| carried.len());
        if carried > MAX_RESOURCE_META_BYTES {
            let error = McpAppError::ResultTooLarge;
            return Err(step
                .fail(error.code(), ConversationError::McpApp(error))
                .await);
        }
        let bytes: Arc<[u8]> = Arc::from(resource.html().as_bytes());
        let size = bytes.len();
        let sha256 = hex(&bytes);
        // Held pending, under the lock the mount's release and the
        // conversation's end take; the ticket's ends are recorded against
        // this call.
        let held = HeldResource {
            record: step.record.clone(),
            bytes,
        };
        let ticket = match opening
            .apps
            .issue(opening.epoch, &read.app, || ports.tickets.issue(held))
        {
            Ok(Ok(ticket)) => ticket,
            Ok(Err(TicketRefusal::Capacity | TicketRefusal::Unavailable)) => {
                return Err(step
                    .fail(
                        McpAppCode::TemporarilyUnavailable,
                        ConversationError::Unavailable,
                    )
                    .await);
            }
            // The mount released or the opening ended while it was read: by
            // that other command, so the system's.
            Err(_) => {
                let error = McpAppError::Cancelled;
                return Err(step
                    .ended(
                        McpAppAuditPhase::Completed(McpAppOutcome::Failed(error.code())),
                        Some(McpAppInitiator::System),
                        ConversationError::McpApp(error),
                    )
                    .await);
            }
        };
        let ticket_digest = ResourceTicketDigest::of(ticket.as_bytes()).to_hex();
        let recorded = async {
            step.record(
                McpAppAuditPhase::Completed(McpAppOutcome::Answered {
                    is_error: false,
                    bytes: size,
                }),
                None,
            )
            .await?;
            step.record(
                McpAppAuditPhase::TicketIssued {
                    ticket_digest: ticket_digest.clone(),
                    size,
                    sha256: sha256.clone(),
                },
                None,
            )
            .await
        };
        if let Err(error) = recorded.await {
            // Pending, so never handed out, and never ended on record either:
            // an end with no issue behind it is no history.
            ports.tickets.discard(&ticket);
            return Err(error);
        }
        if let Err((cause, by)) = ports.tickets.activate(&ticket) {
            // Let go of while its issue was being recorded: its end is this
            // call's to record, after its issue.
            step.record(
                McpAppAuditPhase::TicketEnded {
                    ticket_digest,
                    cause,
                },
                Some(by),
            )
            .await?;
            return Err(ConversationError::McpApp(McpAppError::Cancelled));
        }
        Ok(McpAppResource {
            uri: uri.as_str().to_owned(),
            size,
            sha256,
            ticket,
            csp: resource.csp().clone(),
            permissions: resource.permissions(),
            domain: resource.domain().map(str::to_owned),
            prefers_border: resource.prefers_border(),
        })
    }

    /// The opening of the conversation an app's call is in — opened, if it
    /// was not — what its view says of the app, and the ports the call goes
    /// through. The call keeps only its conversation's apps and the opening's
    /// epoch, never the live agent: a call that runs a minute must not hold
    /// the conversation's history, and so its reopening or its deletion,
    /// that long. Admission is held only this long too: a call can wait
    /// minutes on its review, and must not hold a shutdown back for it.
    async fn app_opening(
        &self,
        id: &ConversationId,
        caller: &ConversationCaller,
        app: &McpAppRef,
    ) -> Result<(Opening, Option<AppFacts>, McpAppPorts), ConversationError> {
        let (opening, facts, ports) = self.app_in_conversation(id, caller, app).await?;
        Ok((opening, facts.map(|(facts, _)| facts), ports))
    }

    /// [`Self::app_opening`], with the MCP tool the app's tool call was to,
    /// for what speaks in the conversation as the app.
    async fn app_in_conversation(
        &self,
        id: &ConversationId,
        caller: &ConversationCaller,
        app: &McpAppRef,
    ) -> Result<(Opening, Option<(AppFacts, McpTool)>, McpAppPorts), ConversationError> {
        let _admission = self.admit().await?;
        caller.actor()?;
        let live = self.resolve(id, caller).await?;
        // No MCP servers: no tool call has a UI, so there is no app.
        let ports = self
            .inner
            .mcp_apps
            .clone()
            .ok_or(ConversationError::McpApp(McpAppError::AppUnknown))?;
        let facts = self.app_facts(&live, app, &conversation_session(id)).await;
        let opening = Opening {
            apps: live.app_reviews.clone(),
            epoch: live.app_epoch,
        };
        Ok((opening, facts, ports))
    }

    /// What the conversation's view says of the tool call `app` names as
    /// itself: its MCP server, and whether its tool declared a UI; and the
    /// MCP tool it was to.
    async fn app_facts(
        &self,
        live: &LiveConversation,
        app: &McpAppRef,
        session: &SessionId,
    ) -> Option<(AppFacts, McpTool)> {
        let projection = live.projection.lock().await;
        let tool =
            projection.view.tools.iter().find(|tool| {
                tool.execution_id == app.execution_id && tool.tool_id == app.tool_id
            })?;
        let mcp = tool.mcp.as_ref()?;
        let call = McpTool::new(mcp.server.as_str(), mcp.tool.as_str()).ok()?;
        let facts = AppFacts {
            server: mcp.server.clone(),
            has_ui: self.inner.tool_uis.resource_uri(session, &call).is_some(),
        };
        Some((facts, call))
    }
}

/// What a review asks the person, and what it shows them.
struct Asked<'a> {
    ask: ReviewAsk,
    app: &'a McpAppRef,
    server: &'a str,
    tool: &'a str,
    shown: &'a str,
}

/// Contexts dropped unsent — the records of the updates that gave them —
/// why, and by whom: for [`ConversationService::record_dropped`].
#[must_use]
pub(super) struct Dropped {
    pub(super) updates: Vec<McpAppAuditRecord>,
    pub(super) cause: ContextDrop,
    pub(super) by: McpAppInitiator,
}

/// What an app call keeps of the conversation it is in: its apps, and the
/// opening it was admitted in.
struct Opening {
    apps: Arc<AppReviews>,
    epoch: u64,
}
impl Opening {
    /// Whether `app` may still be admitted, opened for or issued to in this
    /// opening.
    fn admit(&self, app: &McpAppRef) -> Result<(), ReviewRefusal> {
        self.apps.admit(self.epoch, app)
    }
}

/// The app `app` names, as the conversation's transcript keeps it: its tool
/// call, and the MCP tool that call was to.
fn app_source(app: &McpAppRef, tool: McpTool) -> Result<McpAppSource, ExecutionError> {
    McpAppSource::new(
        ExecutionId::new(app.execution_id.as_str())?,
        ToolCallId::new(app.tool_id.as_str())?,
        tool,
    )
}

/// The turn an app's message from the mount `app` becomes, for its request
/// `request_id` in conversation `id`: the same request again is the same
/// turn, and no two mounts' or conversations' requests share one. A digest,
/// length-prefixed, so no two inputs spell the same one.
fn message_execution(id: &ConversationId, app: &McpAppRef, request_id: &str) -> String {
    let mut digest = Sha256::new();
    for part in [
        id.to_string().as_str(),
        app.execution_id.as_str(),
        app.tool_id.as_str(),
        app.instance_id.as_str(),
        request_id,
    ] {
        digest.update((part.len() as u64).to_be_bytes());
        digest.update(part.as_bytes());
    }
    format!("app-{}", &hex_of(digest)[..32])
}

/// The policy's answer for `call` as the session lists its tool now.
fn admitted(
    ports: &McpAppPorts,
    session: &SessionId,
    facts: Option<&AppFacts>,
    call: &McpAppCall,
    arguments: Option<&str>,
) -> Result<AppCallAdmission, McpAppError> {
    admit_app(facts, &call.server)?;
    let listed = ports
        .apps
        .listed_tool(session, &call.server, &call.tool)
        .map_err(McpAppError::from)?;
    Ok(admit_tool_call(
        facts,
        &call.server,
        listed.as_ref(),
        arguments.map_or(0, str::len),
    )?)
}

/// A person, by an explicit command of theirs.
pub(super) fn person(by: &ReviewAnswerer) -> McpAppInitiator {
    McpAppInitiator::Person {
        principal_id: by.principal_id.clone(),
        surface_id: by.surface_id.clone(),
        request_id: by.request_id.clone(),
    }
}

/// One app call's audit: what every step of it shares.
struct Step {
    ports: McpAppPorts,
    record: McpAppAuditRecord,
}
impl Step {
    fn new(
        ports: &McpAppPorts,
        id: &ConversationId,
        caller: &ConversationCaller,
        app: &McpAppRef,
        ask: McpAppAsk,
    ) -> Self {
        Self {
            ports: ports.clone(),
            record: McpAppAuditRecord {
                conversation_id: id.clone(),
                organization_id: caller.organization_id.clone(),
                call_id: Uuid::new_v4().to_string(),
                request_id: caller.action_id.clone(),
                app: app.clone(),
                ask,
                initiator: McpAppInitiator::App {
                    principal_id: caller.principal_id.clone(),
                    surface_id: caller.surface_id.clone(),
                },
                phase: McpAppAuditPhase::Admitted,
            },
        }
    }

    /// The gateway's identity for this one call, which its records carry.
    fn call_id(&self) -> &str {
        &self.record.call_id
    }

    /// The app, on behalf of its caller.
    fn app_initiator(&self) -> McpAppInitiator {
        let McpAppAuditRecord { initiator, .. } = &self.record;
        initiator.clone()
    }

    /// Record `phase`, taken by `initiator` — the app, when `None`.
    async fn record(
        &self,
        phase: McpAppAuditPhase,
        initiator: Option<McpAppInitiator>,
    ) -> Result<(), ConversationError> {
        let mut record = self.record.clone();
        record.phase = phase;
        if let Some(initiator) = initiator {
            record.initiator = initiator;
        }
        self.ports.audit.record(record).await
    }

    /// Refuse the call as `error`, on record: the refusal, or the audit's own
    /// failure when the refusal could not be recorded.
    async fn refuse(&self, error: McpAppError) -> ConversationError {
        let code = error.code();
        self.refuse_as(code, ConversationError::McpApp(error)).await
    }

    async fn refuse_as(&self, code: McpAppCode, error: ConversationError) -> ConversationError {
        self.ended(McpAppAuditPhase::Refused(code), None, error)
            .await
    }

    /// Refuse the call as `error`, on record as the system's: the release or
    /// end that refuses it came first, from another command.
    async fn refuse_by_system(&self, error: McpAppError) -> ConversationError {
        self.ended(
            McpAppAuditPhase::Refused(error.code()),
            Some(McpAppInitiator::System),
            ConversationError::McpApp(error),
        )
        .await
    }

    /// End the call after it reached the server, as `code`, on record.
    async fn fail(&self, code: McpAppCode, error: ConversationError) -> ConversationError {
        self.ended(
            McpAppAuditPhase::Completed(McpAppOutcome::Failed(code)),
            None,
            error,
        )
        .await
    }

    async fn ended(
        &self,
        phase: McpAppAuditPhase,
        initiator: Option<McpAppInitiator>,
        error: ConversationError,
    ) -> ConversationError {
        match self.record(phase, initiator).await {
            Ok(()) => error,
            Err(audit) => audit,
        }
    }
}

impl From<AppRefusal> for McpAppError {
    fn from(refusal: AppRefusal) -> Self {
        match refusal {
            AppRefusal::AppUnknown => Self::AppUnknown,
            AppRefusal::ServerMismatch => Self::ServerMismatch,
            AppRefusal::ToolNotForApp => Self::ToolNotForApp,
            AppRefusal::RequestTooLarge => Self::RequestTooLarge,
        }
    }
}

/// The view, bounded, with the app reviews open beside it.
///
/// - **What comes first.** The agent's own reviews and questions, exactly
///   as the view shows them with no app review open: an app's server must
///   not be able to hide or displace what the agent is asking.
/// - **What is shown.** The oldest app reviews that fit beside them, room
///   made out of the view's transcript, tool calls and queue — only as much
///   as the reviews shown, and the notice, need.
/// - **What is not.** Those that do not fit wait unseen, and the view says
///   so where that notice fits beside the agent's own and the view says
///   nothing more specific of its own.
/// - **The revision.** Replaced by a digest of it, every app review open,
///   shown or not, and how many are shown: any change in which are open or
///   shown changes it, so a window holding a revision holds what it showed.
///   The digest is no longer than the revision it replaces — the view needs
///   no room for it — and, all hex, never equal to a revision without apps.
///
/// Shown only in a view whose transcript is confirmed complete: the client
/// refuses one of unconfirmed history that offers any control.
pub(super) fn with_app_reviews(
    view: ConversationView,
    reviews: Vec<ConversationPermission>,
) -> ConversationView {
    let view = bound_view(view);
    if reviews.is_empty() || view.transcript_state != ConversationTranscriptState::Complete {
        return view;
    }
    let sizes: Vec<usize> = reviews
        .iter()
        .map(|review| serde_json::to_vec(review).map_or(usize::MAX, |bytes| bytes.len() + 1))
        .collect();
    let all = sizes
        .iter()
        .fold(0, |total: usize, size| total.saturating_add(*size));
    // What cannot be given up for them: the view with all but its
    // interactions given up. Where every one fits as it is, no need to look.
    let floor = if encoded_len(&view).saturating_add(all) <= MAX_VIEW_BYTES {
        encoded_len(&view)
    } else {
        encoded_len(&bound_view_within(view.clone(), 0, false))
    };
    // The notice, where the view has none of its own and it fits beside
    // the agent's own.
    let notice = Some(
        serde_json::to_vec(&serde_json::json!({ "interactionViewError": UNSHOWN_APP_REVIEWS }))
            .map_or(usize::MAX, |bytes| bytes.len()),
    )
    .filter(|&notice| {
        view.interaction_view_error.is_none() && floor.saturating_add(notice) <= MAX_VIEW_BYTES
    });
    // The most of them, oldest first, that fit on that floor.
    let room_for = |count: usize| {
        let unshown = if count < reviews.len() {
            notice.unwrap_or(0)
        } else {
            0
        };
        sizes[..count]
            .iter()
            .fold(unshown, |total, size| total.saturating_add(*size))
    };
    let shown = (0..=reviews.len())
        .rev()
        .find(|&count| floor.saturating_add(room_for(count)) <= MAX_VIEW_BYTES)
        .unwrap_or(0);
    let mut view = bound_view_within(view, MAX_VIEW_BYTES.saturating_sub(room_for(shown)), false);
    let mut digest = Sha256::new();
    digest.update((view.revision.len() as u64).to_be_bytes());
    digest.update(view.revision.as_bytes());
    for review in &reviews {
        digest.update((review.permission_id.len() as u64).to_be_bytes());
        digest.update(review.permission_id.as_bytes());
    }
    digest.update((shown as u64).to_be_bytes());
    let digits = view.revision.len().clamp(16, 64);
    view.revision = hex_of(digest)[..digits].to_owned();
    if shown < reviews.len() && notice.is_some() {
        view.interaction_view_error = Some(UNSHOWN_APP_REVIEWS.into());
    }
    view.permissions.extend(reviews.into_iter().take(shown));
    view
}

/// What a view says of the app reviews it leaves out: they wait, unseen,
/// until there is room or they end — no Stop of the agent's ends them.
const UNSHOWN_APP_REVIEWS: &str =
    "Some requests from apps do not fit this view. They wait until there is room, or expire.";

fn encoded_len(view: &ConversationView) -> usize {
    serde_json::to_vec(view).map_or(usize::MAX, |bytes| bytes.len())
}

fn hex_of(digest: Sha256) -> String {
    Sha256Digest::from_bytes(digest.finalize().into()).to_hex()
}

/// Lowercase hex SHA-256 of `bytes`.
fn hex(bytes: &[u8]) -> String {
    Sha256Digest::from_bytes(Sha256::digest(bytes).into()).to_hex()
}

#[cfg(test)]
#[path = "../../../../tests/conversation/app_calls.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../../tests/conversation/app_messages.rs"]
mod message_tests;
