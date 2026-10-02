//! The ports an MCP App's calls go through (#348): its conversation's own
//! MCP session, the resources held for it behind a ticket, and the audit of
//! every step. The rules are `mcp_servers::domain`'s; the flow is the
//! conversation service's.
use super::ConversationFuture;
use crate::conversation::domain::ConversationId;
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

/// No MCP servers: no conversation has a session of any.
pub struct NoMcpApps;
impl McpApps for NoMcpApps {
    fn listed_tool(
        &self,
        _: &SessionId,
        _: &str,
        _: &str,
    ) -> Result<Option<ListedTool>, McpAppFailure> {
        Err(McpAppFailure::NoSession)
    }
    fn call_tool<'a>(
        &'a self,
        _: &'a SessionId,
        _: &'a str,
        _: &'a str,
        _: Option<Value>,
    ) -> McpAppFuture<'a, Value> {
        Box::pin(async { Err(McpAppFailure::NoSession) })
    }
    fn read_resource<'a>(
        &'a self,
        _: &'a SessionId,
        _: &'a str,
        _: &'a UiResourceUri,
    ) -> McpAppFuture<'a, UiResource> {
        Box::pin(async { Err(McpAppFailure::NoSession) })
    }
}

/// What an app asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpAppAsk {
    CallTool { server: String, tool: String },
    ReadResource { server: String, uri: String },
}

/// Who a step was taken by: the app, on behalf of the person whose
/// credential it runs under; the person, answering its review; or the
/// gateway itself, for a deadline or a cleanup. The app is not
/// authenticated beyond the credential — that is what "on behalf of" says.
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
    /// Failed after it was sent, by the protocol code it is refused with.
    Failed(&'static str),
}

/// One step of an app call's life. Each is recorded before the step's
/// effect is reported; a refusal and a withdrawal, as much as a success.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpAppAuditPhase {
    /// Refused before anything reached the server, by protocol code.
    Refused(&'static str),
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
    /// The ticket expired unredeemed, or its conversation ended first.
    TicketExpired { ticket_digest: String },
}

/// Immutable evidence of one step: its target (the conversation, the app,
/// what it asked for), its initiator, and the step. The audit store assigns
/// when it was observed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpAppAuditRecord {
    pub conversation_id: ConversationId,
    pub organization_id: OrganizationId,
    /// The app's own request, which the steps of one call share.
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

/// A resource's bytes, held for one redemption.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeldResource {
    pub conversation_id: ConversationId,
    pub app: McpAppRef,
    pub bytes: Arc<[u8]>,
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
pub trait ResourceTickets: Send + Sync {
    /// Hold `resource` and answer its ticket.
    fn issue(&self, resource: HeldResource) -> Result<String, TicketRefusal>;
    /// Let go of everything held for `conversation`: its tickets are
    /// refused from now on.
    fn release_conversation(&self, conversation: &ConversationId);
    /// Let go of everything held for the one mount `app` of
    /// `conversation`. Idempotent.
    fn release_app(&self, conversation: &ConversationId, app: &McpAppRef);
}

/// How long a resource ticket can be redeemed.
pub const RESOURCE_TICKET_LIFETIME_MS: u64 = 60_000;
/// The most a conversation may hold behind tickets at once.
pub const MAX_HELD_RESOURCE_BYTES: usize = 16 * 1024 * 1024;
