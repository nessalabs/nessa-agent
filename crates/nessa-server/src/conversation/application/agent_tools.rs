//! Attribution and durable admission evidence for source-bound agent tools.
use super::{ConversationCaller, ConversationFuture};
use nessa_protocol::conversation::domain::ConversationId;

/// Detached admissions one source conversation retains at once, across all its
/// tools. A cancelled response does not free this place; other sources are not
/// bounded by it, so the gateway's total grows with its agents.
pub const MAX_AGENT_TOOL_ADMISSIONS: usize = 8;

/// One gateway-verified source turn. Tool arguments cannot construct this value.
pub struct AgentToolSource {
    pub(super) caller: ConversationCaller,
    pub(super) source: ConversationId,
    pub(super) execution: String,
}
impl AgentToolSource {
    pub fn conversation(&self) -> &ConversationId {
        &self.source
    }
    pub fn execution(&self) -> &str {
        &self.execution
    }
    pub fn caller(&self) -> ConversationCaller {
        self.caller.clone()
    }
}

/// Logical caller identity and the scoped identity used by command/note owners.
/// Keeping both preserves attribution without changing existing retry namespaces.
pub struct AgentToolRequest {
    pub logical: String,
    pub command: String,
}

/// A decision before an agent tool reaches a target command or read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentToolAuditRecord {
    pub source: ConversationId,
    pub execution: String,
    pub lease: String,
    pub call: String,
    pub request: String,
    pub command: String,
    pub tool: String,
    pub target: Option<ConversationId>,
    pub principal: String,
    pub surface: String,
    pub owner_initiated: bool,
    pub allowed: bool,
    pub policies: Vec<String>,
    pub reason: Option<String>,
}

/// Persist admission before effects, independently of response delivery.
pub trait AgentToolAudit: Send + Sync {
    fn record(&self, record: AgentToolAuditRecord) -> ConversationFuture<'_, ()>;
}
