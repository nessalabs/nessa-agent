//! What the identity retrofit asks of the world outside it.
use super::outcome::{Leftover, RetrofitSummary};
use crate::agents::domain::AgentId;
use crate::conversation::application::ConversationFuture;
use crate::conversation::domain::{ConversationApprovalMode, ConversationId};
use nessa_sdk::application::agent_execution::providers::ProviderIdentity;
use std::{future::Future, pin::Pin};

/// Every conversation this gateway holds a record of, deleted ones included.
pub trait RetrofitConversations: Send + Sync {
    /// Each conversation's identity, in no promised order.
    ///
    /// A row whose identity cannot be read is left out and counted, since it
    /// may be one of them. Only being unable to ask at all is an error.
    fn every_conversation(&self) -> ConversationFuture<'_, EveryConversation>;
}

/// What [`RetrofitConversations::every_conversation`] found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EveryConversation {
    /// Each conversation whose identity could be read.
    pub conversations: Vec<ConversationId>,
    /// Rows whose conversation could not be named.
    pub unreadable: usize,
}

/// The two identities a conversation's agent, model and approval mode resolve
/// to now, the way reopening resolves its provider.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestorationIdentities {
    /// What restoration compares today.
    pub current: ProviderIdentity,
    /// What the same provider had under the fingerprint that still hashed
    /// MCP servers; `None` for a provider that never had one.
    pub previous: Option<ProviderIdentity>,
}

/// Resolves a conversation's selection to its [`RestorationIdentities`].
///
/// `Ok(None)` is an agent configured nowhere, or not installed. A refusal is
/// a [`ConversationError`](crate::conversation::application::ConversationError):
/// `ModelUnavailable` and `ApprovalModeUnavailable` say the selection cannot
/// be opened here, `Unavailable` that the answer could not be had now (a
/// deadline, an unknown installation state).
pub trait RestorationIdentitySource: Send + Sync {
    fn identities<'a>(
        &'a self,
        agent: AgentId,
        model: &'a str,
        mode: ConversationApprovalMode,
    ) -> ConversationFuture<'a, Option<RestorationIdentities>>;
}

/// Why the retrofit changes a conversation's restoration identity. One cause,
/// named so a record says it rather than implying it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetrofitCause {
    /// The MCP server list left the restoration fingerprint (ADR 344).
    McpServersLeftRestorationIdentity,
}

/// Who started the retrofit. Nobody asks for it: the gateway runs it as it
/// starts, so no person is named.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetrofitInitiator {
    /// The gateway, while starting, before any conversation can be opened.
    SystemGatewayStart,
}

impl RetrofitCause {
    /// The cause this retrofit records, on each move and on its run summary.
    pub const OF_RUN: Self = Self::McpServersLeftRestorationIdentity;
}

impl RetrofitInitiator {
    /// The initiator this retrofit records, on each move and on its run
    /// summary.
    pub const OF_RUN: Self = Self::SystemGatewayStart;
}

/// One durable fact of the retrofit, handed to [`IdentityRetrofitAudit`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RetrofitAuditRecord {
    /// About to append the move; committed before the append is attempted.
    Rewriting(RetrofitMove),
    /// The writer acknowledged the move.
    Rewritten(RetrofitMove),
    /// The move was attempted after its intent was recorded, and not
    /// acknowledged, for `reason`.
    Left {
        /// The move that was attempted.
        attempted: RetrofitMove,
        /// Why it was left.
        reason: Leftover,
    },
    /// The run's counts and every conversation it left, with why.
    Summary(RetrofitSummary),
}

/// One conversation's restoration identity moving from `before` to `after`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetrofitMove {
    pub conversation_id: ConversationId,
    /// The identity its saved history restores under now.
    pub before: ProviderIdentity,
    /// The identity it will restore under.
    pub after: ProviderIdentity,
    pub cause: RetrofitCause,
    pub initiator: RetrofitInitiator,
}

/// Commits retrofit evidence durably before the retrofit reports anything.
///
/// A record repeated with the same content is acknowledged again; an
/// unavailable sink is [`ConversationError::Audit`](crate::conversation::application::ConversationError::Audit),
/// and the retrofit then neither appends for that conversation nor writes its
/// marker. Identities are credential-free fingerprints, so nothing secret is
/// recorded.
pub trait IdentityRetrofitAudit: Send + Sync {
    fn record(&self, record: RetrofitAuditRecord) -> ConversationFuture<'_, ()>;
}

/// The marker could not be read or written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MarkerUnavailable;

/// One marker answer.
pub type MarkerFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, MarkerUnavailable>> + Send + 'a>>;

/// Whether the retrofit has finished on this namespace, kept as a file.
pub trait IdentityRetrofitMarker: Send + Sync {
    /// Whether a run has finished with nothing transient left.
    fn done(&self) -> MarkerFuture<'_, bool>;
    /// Record, durably, that a run has.
    fn mark_done(&self) -> MarkerFuture<'_, ()>;
}
