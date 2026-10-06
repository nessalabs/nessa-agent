//! The ports an MCP App's calls go through (#348): its conversation's own
//! MCP session, the resources held for it behind a ticket, and the audit of
//! every step. The rules are `mcp_servers::domain`'s; the flow is the
//! conversation service's.
use super::ConversationFuture;
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_protocol::product_contract::generated::{ConversationErrorCode, MCP_RESOURCE_TICKET_MS};
use nessa_sdk::domain::agent_execution::sessions::SessionId;
use nessa_sdk::domain::mcp_apps::{ListedTool, UiResource, UiResourceUri};
use serde_json::Value;
use std::{future::Future, pin::Pin, sync::Arc};

/// One mount of an app: the tool call whose UI it is, in its conversation,
/// and the host's own id for this mount of it. Policy and audit name the app
/// by its tool call; reviews and tickets are kept per mount.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct McpAppRef {
    pub execution_id: String,
    pub tool_id: String,
    pub instance_id: String,
}

/// Why an app's conversation session could not answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpAppFailure {
    /// The conversation has no open session of that server.
    NoSession,
    /// The session ended before it answered.
    SessionEnded,
    /// The remote endpoint could not be reached. Nothing was retained.
    Unreachable,
    /// The remote endpoint refused the caller. The tool was not run.
    Unauthorized,
    /// The caller's scope was not enough. The call was not retried.
    InsufficientScope,
    /// No answer in time.
    TimedOut,
    /// The server answered with a JSON-RPC error.
    Remote { code: i64, message: String },
    /// The answer was larger than its bound.
    TooLarge,
    /// The resource is not an MCP App's HTML.
    NotAnApp,
    /// The answer was not of the protocol's shape.
    Malformed,
    /// The session has as many requests waiting as it takes.
    Busy,
}

/// A future an [`McpApps`] port answers with.
pub type McpAppFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, McpAppFailure>> + Send + 'a>>;

/// An SDK session's own newest session of each server, for an app's calls.
pub trait McpApps: Send + Sync {
    /// The tool `name` as that session last listed it, or `None`.
    fn listed_tool(
        &self,
        session: &SessionId,
        server: &str,
        name: &str,
    ) -> Result<Option<ListedTool>, McpAppFailure>;
    /// Call the tool: the server's `CallToolResult` as it gave it.
    fn call_tool<'a>(
        &'a self,
        session: &'a SessionId,
        server: &'a str,
        name: &'a str,
        arguments: Option<Value>,
    ) -> McpAppFuture<'a, Value>;
    /// Read the app resource `uri`.
    fn read_resource<'a>(
        &'a self,
        session: &'a SessionId,
        server: &'a str,
        uri: &'a UiResourceUri,
    ) -> McpAppFuture<'a, UiResource>;
}

/// What an app asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpAppAsk {
    CallTool {
        server: String,
        tool: String,
    },
    ReadResource {
        server: String,
        uri: String,
    },
    /// `ui/message`: a message in its conversation, as the person.
    SendMessage {
        server: String,
    },
    /// `ui/update-model-context`: what it gives the model now.
    UpdateModelContext {
        server: String,
    },
}

/// Who a step was taken by: the app, on behalf of the person whose
/// credential it runs under; a person, by an explicit command of theirs —
/// answering a review, releasing a mount, closing or deleting the
/// conversation; or the gateway itself, for a deadline, an automatic stop
/// or its own shutdown. The app is not authenticated beyond the credential —
/// that is what "on behalf of" says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpAppInitiator {
    App {
        principal_id: PrincipalId,
        surface_id: String,
    },
    Person {
        principal_id: PrincipalId,
        surface_id: String,
        request_id: String,
    },
    System,
}

/// Why a review was withdrawn before it was answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum McpAppWithdrawal {
    /// The app's request was cancelled, or its socket went.
    RequestCancelled,
    /// The app was torn down.
    AppTornDown,
    /// The conversation ended: closed, deleted, or its agent stopped.
    ConversationEnded,
}

/// How a call that reached the server ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpAppOutcome {
    /// Answered within bounds; `is_error` as the server set it.
    Answered { is_error: bool, bytes: usize },
    /// Failed after it was sent, by the code it is refused with.
    Failed(ConversationErrorCode),
}

/// How a ticket ended unredeemed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TicketEnd {
    /// Its lifetime passed.
    Expired,
    /// Its app's mount was released.
    AppReleased,
    /// Its conversation ended: closed, deleted, stopped, or the gateway
    /// stopping.
    ConversationEnded,
}

/// Why a mount's context was dropped unsent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextDrop {
    /// Its mount was released.
    Released,
    /// The conversation's opening ended — closed, deleted, stopped, the
    /// gateway stopping — or another began.
    ConversationEnded,
    /// Its mount was released, or its opening ended, after the update was
    /// recorded and before it was held: it never was.
    NotHeld,
    /// A message admitted while the conversation was idle took it, and was
    /// then refused, or failed before the agent was asked: it went nowhere.
    /// The app may give it again.
    NotSent,
}

/// One step of an app call's life. Each is recorded before the step's
/// effect is reported; a refusal and a withdrawal, as much as a success.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpAppAuditPhase {
    /// Refused before anything reached the server, by protocol code.
    Refused(ConversationErrorCode),
    /// Admitted with nothing to wait for; sent next.
    Admitted,
    /// A destructive tool: the person is asked first.
    ApprovalRequested { permission_id: String },
    /// The person allowed it; sent next.
    Approved { permission_id: String },
    /// The person denied it.
    Denied { permission_id: String },
    /// Nobody answered within the review's deadline.
    Expired { permission_id: String },
    /// Withdrawn before it was answered.
    Withdrawn {
        permission_id: String,
        cause: McpAppWithdrawal,
    },
    /// The server's part ended.
    Completed(McpAppOutcome),
    /// A resource's bytes are held behind a ticket.
    TicketIssued {
        ticket_digest: String,
        size: usize,
        sha256: String,
    },
    /// The ticket was redeemed and the bytes served.
    TicketRedeemed { ticket_digest: String },
    /// The ticket ended unredeemed, and why.
    TicketEnded {
        ticket_digest: String,
        cause: TicketEnd,
    },
    /// The agent took the app's message as the turn `execution_id`, whose
    /// own record holds what it said. `code` is the answer's when the
    /// agent's evidence of taking it failed.
    MessageSent {
        execution_id: String,
        code: Option<ConversationErrorCode>,
    },
    /// The conversation refused the app's message, or its submission failed
    /// before it reached the agent: nothing did. `code` is the answer's.
    MessageNotSent {
        execution_id: String,
        code: ConversationErrorCode,
    },
    /// Whether the agent has the app's message is not known: the agent
    /// could not settle it, or the submission's own task failed once the
    /// agent was asked to take it. `code` is the answer's.
    MessageUnresolved {
        execution_id: String,
        code: ConversationErrorCode,
    },
    /// The mount's context, `bytes` of it, is held for the next message
    /// admitted while the conversation is idle, in place of what it held.
    /// The conversation's updates are recorded in the order they are held
    /// in, one at a time. A turn that carries it names this call's id
    /// (`AppModelContext::update_id`). Why one was never sent is read from
    /// the record of the update that replaced or cleared it, from its own
    /// [`Self::ContextDropped`], or from the record of the turn that
    /// carried it.
    ContextHeld { bytes: usize },
    /// What the mount held, if anything, is let go of unsent.
    ContextCleared,
    /// The context this update held, or was to hold, was dropped unsent,
    /// and why; taken by whoever released the mount, ended the opening or
    /// deleted the conversation, or by the system.
    ContextDropped { cause: ContextDrop },
}

/// Immutable evidence of one step: its target (the conversation, the app,
/// what it asked for), its initiator, and the step. The audit store assigns
/// when it was observed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpAppAuditRecord {
    pub conversation_id: ConversationId,
    pub organization_id: OrganizationId,
    /// The gateway's own identity for this one call, which its steps share:
    /// the app's request id is its own text, and may come again.
    pub call_id: String,
    /// The app's own request.
    pub request_id: String,
    pub app: McpAppRef,
    pub ask: McpAppAsk,
    pub initiator: McpAppInitiator,
    pub phase: McpAppAuditPhase,
}

/// Commits each step's evidence before the step's effect is reported.
pub trait McpAppAudit: Send + Sync {
    fn record(&self, record: McpAppAuditRecord) -> ConversationFuture<'_, ()>;
}

/// A resource's bytes, held for one redemption, and the audit record of the
/// `mcp.readResource` call that read them: its conversation, app, call and
/// initiator, whatever its phase. Each end of the ticket is recorded against
/// that call. Whose the bytes are is read from the record, so there is one
/// answer to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeldResource {
    pub record: McpAppAuditRecord,
    pub bytes: Arc<[u8]>,
}
impl HeldResource {
    /// The conversation the bytes are held for.
    pub fn conversation_id(&self) -> &ConversationId {
        &self.record.conversation_id
    }
    /// The mount of the app the bytes are held for.
    pub fn app(&self) -> &McpAppRef {
        &self.record.app
    }
    /// The record of this ticket's step `phase`, taken by `initiator`.
    pub fn audit_record(
        &self,
        phase: McpAppAuditPhase,
        initiator: McpAppInitiator,
    ) -> McpAppAuditRecord {
        McpAppAuditRecord {
            phase,
            initiator,
            ..self.record.clone()
        }
    }
}

/// Why no ticket was issued.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TicketRefusal {
    /// The conversation already holds as much as it may.
    Capacity,
    /// No random bytes could be had.
    Unavailable,
}

/// Where an app resource's bytes wait for their one redemption over HTTP.
/// A ticket is 256 random bits, valid for [`RESOURCE_TICKET_LIFETIME_MS`],
/// redeemable once; the store keeps only its digest.
///
/// A ticket is issued pending, and made redeemable once its issue is on
/// record. A pending ticket that ends — released, expired — is not reported:
/// its end is the issuer's to record, after its issue, from what
/// [`Self::activate`] answers. So no ticket's end is reported before its
/// issue is recorded.
pub trait ResourceTickets: Send + Sync {
    /// Hold `resource`, pending, and answer its ticket.
    fn issue(&self, resource: HeldResource) -> Result<String, TicketRefusal>;
    /// Make the pending `ticket` redeemable; or, when it ended first, how
    /// and by whom.
    fn activate(&self, ticket: &str) -> Result<(), (TicketEnd, McpAppInitiator)>;
    /// Let go of the pending `ticket`, unreported: its issue could not be
    /// recorded, so it was never handed out and has no history to end.
    fn discard(&self, ticket: &str);
    /// Let go of everything held for `conversation`, which `by` ended.
    fn release_conversation(&self, conversation: &ConversationId, by: &McpAppInitiator);
    /// Let go of everything held for the one mount `app` of
    /// `conversation`, which `by` released. Idempotent.
    fn release_app(&self, conversation: &ConversationId, app: &McpAppRef, by: &McpAppInitiator);
}

/// How long a resource ticket can be redeemed: the protocol's
/// `McpReadResourceResult.expiresInMs`, its one statement.
pub const RESOURCE_TICKET_LIFETIME_MS: u64 = MCP_RESOURCE_TICKET_MS;
/// The most a conversation may hold behind tickets at once.
pub const MAX_HELD_RESOURCE_BYTES: usize = 16 * 1024 * 1024;
/// The most tickets a conversation may hold at once, whatever their size.
pub const MAX_HELD_TICKETS: usize = 64;

/// Why an app's call was refused, or failed once sent. [`Self::code`] is the
/// protocol's [`ConversationErrorCode`], which audit records and the wire
/// answers with (`each_app_refusal_is_on_the_wire_by_the_code_audit_names_it_with`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpAppError {
    AppUnknown,
    ServerMismatch,
    ToolNotForApp,
    RequestTooLarge,
    SessionUnavailable,
    ApprovalDenied,
    ApprovalExpired,
    Cancelled,
    ResultTooLarge,
    TimedOut,
    /// The server's JSON-RPC error, or `None` for an answer that is no MCP
    /// answer at all.
    Remote(Option<(i64, String)>),
    /// The session cannot take another request now; nothing was sent.
    Busy,
    /// The remote endpoint could not be reached. Nothing was retained.
    Unreachable,
    /// The remote endpoint refused the caller. The tool was not run.
    Unauthorized,
    /// The caller's scope was not enough. The call was not retried.
    InsufficientScope,
}
impl McpAppError {
    /// The protocol code audit records and the wire answers with.
    pub fn code(&self) -> ConversationErrorCode {
        match self {
            Self::AppUnknown => ConversationErrorCode::McpAppUnknown,
            Self::ServerMismatch => ConversationErrorCode::McpServerMismatch,
            Self::ToolNotForApp => ConversationErrorCode::McpToolNotForApp,
            Self::RequestTooLarge => ConversationErrorCode::McpRequestTooLarge,
            Self::SessionUnavailable => ConversationErrorCode::McpSessionUnavailable,
            Self::ApprovalDenied => ConversationErrorCode::McpApprovalDenied,
            Self::ApprovalExpired => ConversationErrorCode::McpApprovalExpired,
            Self::Cancelled => ConversationErrorCode::McpCancelled,
            Self::ResultTooLarge => ConversationErrorCode::McpResultTooLarge,
            Self::TimedOut => ConversationErrorCode::McpTimedOut,
            Self::Remote(_) => ConversationErrorCode::McpRemoteError,
            Self::Busy => ConversationErrorCode::TemporarilyUnavailable,
            Self::Unreachable => ConversationErrorCode::McpUnreachable,
            Self::Unauthorized => ConversationErrorCode::McpUnauthorized,
            Self::InsufficientScope => ConversationErrorCode::McpInsufficientScope,
        }
    }
}
impl From<McpAppFailure> for McpAppError {
    fn from(failure: McpAppFailure) -> Self {
        match failure {
            McpAppFailure::NoSession | McpAppFailure::SessionEnded => Self::SessionUnavailable,
            McpAppFailure::TimedOut => Self::TimedOut,
            McpAppFailure::Remote { code, message } => Self::Remote(Some((code, message))),
            McpAppFailure::TooLarge => Self::ResultTooLarge,
            // A resource that is not an app's: read, and changing nothing.
            McpAppFailure::NotAnApp => Self::AppUnknown,
            McpAppFailure::Malformed => Self::Remote(None),
            McpAppFailure::Busy => Self::Busy,
            McpAppFailure::Unreachable => Self::Unreachable,
            McpAppFailure::Unauthorized => Self::Unauthorized,
            McpAppFailure::InsufficientScope => Self::InsufficientScope,
        }
    }
}

/// Told of each held context dropped unsent, so it can be audited: the
/// [`McpAppAuditPhase::ContextDropped`] record, built by the conversation's
/// apps at the moment they removed it — released, its opening ended, a new
/// opening begun, the conversation deleted, or taken by a submission that
/// then went nowhere — against the update that held it, by whoever dropped
/// it. The twin of the resource tickets' `TicketEvents`.
///
/// Called once per drop, synchronously, after the apps have let go of the
/// context and outside their lock. It must not block: an implementation
/// that has to await an audit store hands the record on (composition's
/// channel and its recorder, `audit_context_drops`). The apps keep no
/// record of what they reported; the receiver is the evidence's only holder
/// from then on. No command that dropped a context waits for its record or
/// answers by it: a drop whose record fails is logged by the receiver
/// (`docs/design/mcp-app-calls.md`, row C15b).
pub trait DroppedContexts: Send + Sync {
    fn context_dropped(&self, record: McpAppAuditRecord);
}

/// What an app's calls go through: the conversation's own MCP sessions, the
/// audit of every step, the resources held behind tickets, and where the
/// drops of held contexts are reported. A gateway with no MCP servers has
/// none, and no app to admit.
#[derive(Clone)]
pub struct McpAppPorts {
    pub apps: Arc<dyn McpApps>,
    pub audit: Arc<dyn McpAppAudit>,
    pub tickets: Arc<dyn ResourceTickets>,
    pub dropped: Arc<dyn DroppedContexts>,
}
