//! The ports an MCP App's calls go through (#348): its conversation's own
//! MCP session, the resources held for it behind a ticket, and the audit of
//! every step. The rules are `mcp_servers::domain`'s; the flow is the
//! conversation service's.
use super::ConversationFuture;
use crate::conversation::domain::ConversationId;
use crate::product_contract::generated::MCP_RESOURCE_TICKET_MS;
use nessa_auth::domain::{OrganizationId, PrincipalId};
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

/// The protocol code a step ended an app's call with, as audit names it.
/// The wire answers with the same code (`tests/conversation/agreement.rs`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum McpAppCode {
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
    RemoteError,
    InvalidRequest,
    TemporarilyUnavailable,
    /// A message, while a turn runs or input waits.
    TurnRunning,
}
impl McpAppCode {
    /// Every code, for the tests that hold them to the protocol's.
    pub const ALL: [Self; 14] = [
        Self::AppUnknown,
        Self::ServerMismatch,
        Self::ToolNotForApp,
        Self::RequestTooLarge,
        Self::SessionUnavailable,
        Self::ApprovalDenied,
        Self::ApprovalExpired,
        Self::Cancelled,
        Self::ResultTooLarge,
        Self::TimedOut,
        Self::RemoteError,
        Self::InvalidRequest,
        Self::TemporarilyUnavailable,
        Self::TurnRunning,
    ];
    /// The protocol's name for it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AppUnknown => "mcp_app_unknown",
            Self::ServerMismatch => "mcp_server_mismatch",
            Self::ToolNotForApp => "mcp_tool_not_for_app",
            Self::RequestTooLarge => "mcp_request_too_large",
            Self::SessionUnavailable => "mcp_session_unavailable",
            Self::ApprovalDenied => "mcp_approval_denied",
            Self::ApprovalExpired => "mcp_approval_expired",
            Self::Cancelled => "mcp_cancelled",
            Self::ResultTooLarge => "mcp_result_too_large",
            Self::TimedOut => "mcp_timed_out",
            Self::RemoteError => "mcp_remote_error",
            Self::InvalidRequest => "invalid_request",
            Self::TemporarilyUnavailable => "temporarily_unavailable",
            Self::TurnRunning => "turn_running",
        }
    }
}

/// How a call that reached the server ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpAppOutcome {
    /// Answered within bounds; `is_error` as the server set it.
    Answered { is_error: bool, bytes: usize },
    /// Failed after it was sent, by the code it is refused with.
    Failed(McpAppCode),
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

/// One step of an app call's life. Each is recorded before the step's
/// effect is reported; a refusal and a withdrawal, as much as a success.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpAppAuditPhase {
    /// Refused before anything reached the server, by protocol code.
    Refused(McpAppCode),
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
    /// own record holds what it said.
    MessageSent { execution_id: String },
    /// The conversation refused the app's message: nothing reached the
    /// agent. Its code is the answer's.
    MessageNotSent { execution_id: String },
    /// The mount's context, `bytes` of it, is held for the next message, in
    /// place of what it held. `sequence` orders the conversation's updates:
    /// of a mount's, the highest recorded stands. A turn that carries it
    /// names this call's id (`AppModelContext::update`).
    ContextHeld { bytes: usize, sequence: u64 },
    /// What the mount gave before `sequence` is let go of.
    ContextCleared { sequence: u64 },
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

/// Why an app's call was refused, or failed once sent: one protocol code
/// each. [`Self::code`] is how audit names it.
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
}
impl McpAppError {
    /// The protocol code.
    pub fn code(&self) -> McpAppCode {
        match self {
            Self::AppUnknown => McpAppCode::AppUnknown,
            Self::ServerMismatch => McpAppCode::ServerMismatch,
            Self::ToolNotForApp => McpAppCode::ToolNotForApp,
            Self::RequestTooLarge => McpAppCode::RequestTooLarge,
            Self::SessionUnavailable => McpAppCode::SessionUnavailable,
            Self::ApprovalDenied => McpAppCode::ApprovalDenied,
            Self::ApprovalExpired => McpAppCode::ApprovalExpired,
            Self::Cancelled => McpAppCode::Cancelled,
            Self::ResultTooLarge => McpAppCode::ResultTooLarge,
            Self::TimedOut => McpAppCode::TimedOut,
            Self::Remote(_) => McpAppCode::RemoteError,
            Self::Busy => McpAppCode::TemporarilyUnavailable,
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
        }
    }
}

/// What an app's calls go through: the conversation's own MCP sessions, the
/// audit of every step, and the resources held behind tickets. A gateway
/// with no MCP servers has none, and no app to admit.
#[derive(Clone)]
pub struct McpAppPorts {
    pub apps: Arc<dyn McpApps>,
    pub audit: Arc<dyn McpAppAudit>,
    pub tickets: Arc<dyn ResourceTickets>,
}
