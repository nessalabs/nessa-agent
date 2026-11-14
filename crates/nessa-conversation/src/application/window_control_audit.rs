//! Evidence of every change an agent or a person's CLI asked a desktop
//! window to make (`control_window`).
//!
//! Each change is recorded twice under one operation id: `Requested`, before
//! the window is asked (a failure refuses the change), and then what the
//! window said, or why it was never asked. A refusal by policy is recorded
//! too, so the audit shows each attempt, allowed or not.
use super::{ConversationCaller, ConversationFuture};
use crate::domain::WindowAction;
use nessa_protocol::conversation::domain::ConversationId;

/// Who asked: an agent's tool call, acting as the person whose turn it is,
/// or the person's own session (a CLI, an MCP client of the gateway).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WindowControlVia {
    Agent {
        source: ConversationId,
        execution: String,
        call: String,
    },
    Session,
}

/// One step of a window change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WindowControlOutcome {
    /// Allowed and about to be sent; recorded before the window is asked.
    Requested,
    /// The window did it.
    Applied { revision: u64 },
    /// It was so already.
    Unchanged { revision: u64 },
    /// Refused before the window was asked, or by the window, for `code`.
    Refused { code: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowControlAuditRecord {
    /// The gateway's own id for this change, shared by its records.
    pub operation_id: String,
    pub caller: ConversationCaller,
    pub via: WindowControlVia,
    /// The window asked, once one was chosen.
    pub window_id: Option<String>,
    pub action: WindowAction,
    /// The Cedar policies that allowed it, when they did.
    pub policies: Vec<String>,
    pub outcome: WindowControlOutcome,
}

/// Commits each record before the step it records is reported.
pub trait WindowControlAudit: Send + Sync {
    fn record(&self, record: WindowControlAuditRecord) -> ConversationFuture<'_, ()>;
}
