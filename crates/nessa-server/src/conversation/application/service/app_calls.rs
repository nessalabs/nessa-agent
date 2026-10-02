//! An MCP App's calls (#348): `mcp.callTool`, `mcp.readResource` and
//! `mcp.releaseApp`, through the conversation's own MCP sessions. The rules
//! are `mcp_servers::domain::app_call`'s; this is their flow — every step
//! audited before its effect is reported, a destructive call held on a
//! review the person answers, and each refusal one protocol code.
//!
//! Each call runs on a task of its own, so that a caller going away cannot
//! leave a step unrecorded: a call already sent finishes and is recorded, and
//! a review still waiting is withdrawn and recorded as withdrawn.
use super::super::app_reviews::{new_review_id, ReviewEnd, ReviewRefusal, APP_REVIEW_DEADLINE};
use super::super::mcp_apps::{
    HeldResource, McpAppAsk, McpAppAuditPhase, McpAppAuditRecord, McpAppError, McpAppInitiator,
    McpAppOutcome, McpAppPorts, McpAppRef, McpAppWithdrawal, TicketRefusal,
};
use super::super::session_key::conversation_session;
use super::super::view::{ConversationPermission, ConversationView};
use super::{ConversationCaller, ConversationError, ConversationService, LiveConversation};
use crate::conversation::domain::ConversationId;
use crate::mcp_servers::domain::{
    admit_resource_read, admit_tool_call, AppCallAdmission, AppFacts, AppRefusal,
    MAX_APP_RESULT_BYTES,
};
use nessa_sdk::domain::agent_execution::{sessions::SessionId, tools::McpTool};
use nessa_sdk::domain::mcp_apps::{UiCsp, UiPermissions, UiResourceUri};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio::sync::oneshot;
use uuid::Uuid;

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

/// A resource read for an app: what its bytes are, and the ticket that
/// redeems them once.
#[derive(Clone, Debug)]
pub struct McpAppResource {
    pub uri: String,
    pub size: usize,
    /// Lowercase hex SHA-256 of the bytes.
    pub sha256: String,
    pub ticket: String,
    pub csp: UiCsp,
    pub permissions: UiPermissions,
    pub domain: Option<String>,
    pub prefers_border: Option<bool>,
}

impl ConversationService {
    /// Call `call.tool` on the app's own server, as the conversation's own
    /// session: the server's `CallToolResult`, encoded. A destructive tool
    /// waits for the person's answer to a review first.
    pub async fn call_app_tool(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
        call: McpAppCall,
    ) -> Result<String, ConversationError> {
        // Dropped with this future: the caller went.
        let (_present, gone) = oneshot::channel::<()>();
        let service = self.clone();
        tokio::spawn(async move { service.app_tool_call(id, caller, call, gone).await })
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
        let service = self.clone();
        tokio::spawn(async move { service.app_resource_read(id, caller, read).await })
            .await
            .map_err(|_| ConversationError::Unavailable)?
    }

    /// The host tore the mount `app` down: withdraw its open reviews (their
    /// calls answer `mcp_cancelled`) and let go of what was held for it.
    /// Idempotent; it never opens the conversation.
    pub async fn release_app(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
        app: McpAppRef,
    ) -> Result<(), ConversationError> {
        let _admission = self.admit().await?;
        caller.actor()?;
        self.check_view_access(&id, &caller).await?;
        let live = self
            .inner
            .conversations
            .lock()
            .await
            .get(&id)
            .and_then(|slot| slot.value.get())
            .and_then(|value| value.as_ref().ok())
            .cloned();
        if let Some(live) = live {
            live.app_reviews
                .withdraw_app(&app, McpAppWithdrawal::AppTornDown);
        }
        if let Some(ports) = &self.inner.mcp_apps {
            ports.tickets.release_app(&id, &app);
        }
        Ok(())
    }

    async fn app_tool_call(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
        call: McpAppCall,
        mut gone: oneshot::Receiver<()>,
    ) -> Result<String, ConversationError> {
        let (live, ports) = self.app_conversation(&id, &caller).await?;
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
        let facts = self.app_facts(&live, &call.app, &session).await;
        if let Err(refusal) = admit_resource_read(facts.as_ref(), &call.server) {
            return Err(step.refuse(refusal.into()).await);
        }
        let listed = match ports.apps.listed_tool(&session, &call.server, &call.tool) {
            Ok(listed) => listed,
            Err(failure) => return Err(step.refuse(failure.into()).await),
        };
        let arguments_text = call.arguments_json.as_deref();
        let admission = match admit_tool_call(
            facts.as_ref(),
            &call.server,
            listed.as_ref(),
            arguments_text.map_or(0, str::len),
        ) {
            Ok(admission) => admission,
            Err(refusal) => return Err(step.refuse(refusal.into()).await),
        };
        let arguments = match arguments_text.map(serde_json::from_str::<Value>) {
            None => None,
            Some(Ok(value @ Value::Object(_))) => Some(value),
            Some(_) => {
                return Err(step
                    .refuse_as("invalid_request", ConversationError::InvalidInput)
                    .await)
            }
        };
        match admission {
            AppCallAdmission::Send => step.record(McpAppAuditPhase::Admitted, None).await?,
            AppCallAdmission::Approve => {
                let shown = arguments_text.unwrap_or("{}");
                self.approved(&live, &step, &call, shown, &mut gone).await?
            }
        }
        let answer = ports
            .apps
            .call_tool(&session, &call.server, &call.tool, arguments)
            .await;
        let (outcome, answer) = match answer {
            Err(failure) => {
                let error = McpAppError::from(failure);
                (McpAppOutcome::Failed(error.code()), Err(error))
            }
            Ok(value) => {
                let text = value.to_string();
                if text.len() > MAX_APP_RESULT_BYTES {
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
        live: &LiveConversation,
        step: &Step,
        call: &McpAppCall,
        arguments_json: &str,
        gone: &mut oneshot::Receiver<()>,
    ) -> Result<(), ConversationError> {
        let permission_id = new_review_id();
        step.record(
            McpAppAuditPhase::ApprovalRequested {
                permission_id: permission_id.clone(),
            },
            None,
        )
        .await?;
        let waiting = match live.app_reviews.open(
            permission_id.clone(),
            &call.app,
            &call.server,
            &call.tool,
            arguments_json,
        ) {
            Ok(waiting) => waiting,
            Err(refusal) => {
                // Asked for and never shown: on record as what ended it.
                let (cause, error) = match refusal {
                    ReviewRefusal::Ended => (
                        McpAppWithdrawal::ConversationEnded,
                        ConversationError::McpApp(McpAppError::Cancelled),
                    ),
                    ReviewRefusal::Full => (
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
                live.app_reviews
                    .withdraw(&permission_id, McpAppWithdrawal::RequestCancelled);
                ended.await
            }
        };
        let person = |by: &super::ReviewAnswerer| McpAppInitiator::Person {
            principal_id: by.principal_id.clone(),
            surface_id: by.surface_id.clone(),
            request_id: by.request_id.clone(),
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
            ReviewEnd::Withdrawn(cause) => (
                McpAppAuditPhase::Withdrawn {
                    permission_id,
                    cause,
                },
                match cause {
                    McpAppWithdrawal::ConversationEnded => McpAppInitiator::System,
                    McpAppWithdrawal::RequestCancelled | McpAppWithdrawal::AppTornDown => {
                        step.app_initiator()
                    }
                },
                Some(McpAppError::Cancelled),
            ),
        };
        step.record(phase, Some(initiator)).await?;
        error.map_or(Ok(()), |error| Err(ConversationError::McpApp(error)))
    }

    async fn app_resource_read(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
        read: McpAppRead,
    ) -> Result<McpAppResource, ConversationError> {
        let (live, ports) = self.app_conversation(&id, &caller).await?;
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
        let facts = self.app_facts(&live, &read.app, &session).await;
        if let Err(refusal) = admit_resource_read(facts.as_ref(), &read.server) {
            return Err(step.refuse(refusal.into()).await);
        }
        let Ok(uri) = UiResourceUri::new(read.uri.as_str()) else {
            return Err(step
                .refuse_as("invalid_request", ConversationError::InvalidInput)
                .await);
        };
        step.record(McpAppAuditPhase::Admitted, None).await?;
        let resource = match ports.apps.read_resource(&session, &read.server, &uri).await {
            Ok(resource) => resource,
            Err(failure) => {
                let error = McpAppError::from(failure);
                step.record(
                    McpAppAuditPhase::Completed(McpAppOutcome::Failed(error.code())),
                    None,
                )
                .await?;
                return Err(ConversationError::McpApp(error));
            }
        };
        let bytes: Arc<[u8]> = Arc::from(resource.html().as_bytes());
        let size = bytes.len();
        let sha256 = hex(&Sha256::digest(&bytes));
        let issued = ports.tickets.issue(HeldResource {
            conversation_id: id.clone(),
            app: read.app.clone(),
            bytes,
        });
        let ticket = match issued {
            Ok(ticket) => ticket,
            Err(TicketRefusal::Capacity | TicketRefusal::Unavailable) => {
                step.record(
                    McpAppAuditPhase::Completed(McpAppOutcome::Failed("temporarily_unavailable")),
                    None,
                )
                .await?;
                return Err(ConversationError::Unavailable);
            }
        };
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
                ticket_digest: hex(&Sha256::digest(ticket.as_bytes())),
                size,
                sha256: sha256.clone(),
            },
            None,
        )
        .await?;
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

    /// The conversation an app's call is in, open, and the ports it goes
    /// through. Admission is held only this long: a call can wait minutes on
    /// its review, and must not hold a shutdown back for it.
    async fn app_conversation(
        &self,
        id: &ConversationId,
        caller: &ConversationCaller,
    ) -> Result<(Arc<LiveConversation>, McpAppPorts), ConversationError> {
        let _admission = self.admit().await?;
        caller.actor()?;
        let live = self.resolve(id, caller).await?;
        // No MCP servers: no tool call has a UI, so there is no app.
        let ports = self
            .inner
            .mcp_apps
            .clone()
            .ok_or(ConversationError::McpApp(McpAppError::AppUnknown))?;
        Ok((live, ports))
    }

    /// What the conversation's view says of the tool call `app` names as
    /// itself: its MCP server, and whether its tool declared a UI.
    async fn app_facts(
        &self,
        live: &LiveConversation,
        app: &McpAppRef,
        session: &SessionId,
    ) -> Option<AppFacts> {
        let projection = live.projection.lock().await;
        let tool =
            projection.view.tools.iter().find(|tool| {
                tool.execution_id == app.execution_id && tool.tool_id == app.tool_id
            })?;
        let mcp = tool.mcp.as_ref()?;
        let call = McpTool::new(mcp.server.as_str(), mcp.tool.as_str()).ok()?;
        Some(AppFacts {
            server: mcp.server.clone(),
            has_ui: self.inner.tool_uis.resource_uri(session, &call).is_some(),
        })
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

    async fn refuse_as(&self, code: &'static str, error: ConversationError) -> ConversationError {
        match self.record(McpAppAuditPhase::Refused(code), None).await {
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

/// The view, with the app reviews open beside the agent's: their
/// identities folded into its revision, so a window holding this revision
/// holds these reviews.
pub(super) fn with_app_reviews(
    mut view: ConversationView,
    reviews: Vec<ConversationPermission>,
) -> ConversationView {
    if reviews.is_empty() {
        return view;
    }
    let mut digest = Sha256::new();
    for review in &reviews {
        digest.update((review.permission_id.len() as u64).to_be_bytes());
        digest.update(review.permission_id.as_bytes());
    }
    view.revision = format!("{}:app:{}", view.revision, hex(&digest.finalize()));
    view.permissions.extend(reviews);
    view
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
