//! The one recorder of every MCP App context dropped unsent (#390): a
//! conversation's apps report each drop as they make it, and this writes it
//! to the MCP App audit on a task of its own, which composition stops after
//! the conversations.
//!
//! ```text
//!   AppReviews ──context_dropped──▶ channel ──▶ audit_context_drops ──record──▶ McpAppAudit
//!   (release, end, begin,           (unbounded)   (one task; a failure
//!    delete, not_sent)                             is logged, not answered)
//! ```
//!
//! Arrows are calls and sends. It is the twin of the resource tickets'
//! `audit_ticket_ends`: a drop, like a ticket's end, outlives the command
//! that caused it, so no command's future — cancellable, budgeted, or
//! unwinding — holds its record, and no command answers for it
//! (`docs/design/mcp-app-calls.md`, rows C15 and C15b).
use crate::conversation::application::{DroppedContexts, McpAppAudit, McpAppAuditRecord};
use std::sync::Arc;
use tokio::sync::{
    mpsc::{UnboundedReceiver, UnboundedSender},
    oneshot,
};

/// Each drop, onto a channel whose receiver records it
/// ([`audit_context_drops`]). Unbounded so that no drop is ever lost for want
/// of room: each held context is dropped at most once, so the channel holds
/// at most one record per context held and not yet audited. A receiver that
/// is gone loses the evidence, and says so.
impl DroppedContexts for UnboundedSender<McpAppAuditRecord> {
    fn context_dropped(&self, record: McpAppAuditRecord) {
        if let Err(lost) = self.send(record) {
            tracing::error!(
                conversation_id = %lost.0.conversation_id,
                call_id = %lost.0.call_id,
                phase = ?lost.0.phase,
                "an MCP App context's drop could not be audited: nothing receives it"
            );
        }
    }
}

/// Record each drop `drops` receives, in the order they were dropped — until
/// every sender is gone, or `stop` says the gateway is stopping, when every
/// drop already sent is recorded before it returns. Its caller bounds how
/// long that may take (composition's `AuditRecorder::finish`).
///
/// A record that cannot be committed is logged, by its conversation and its
/// call, and the next is tried: the context is already dropped, and the log
/// is all that is left to say it (row C15b).
pub async fn audit_context_drops(
    mut drops: UnboundedReceiver<McpAppAuditRecord>,
    audit: Arc<dyn McpAppAudit>,
    mut stop: oneshot::Receiver<()>,
) {
    loop {
        // Drops first: stopping is taken only once none is waiting, so every
        // drop the conversations' ends already sent is recorded before it.
        tokio::select! {
            biased;
            record = drops.recv() => match record {
                Some(record) => record_drop(audit.as_ref(), record).await,
                None => return,
            },
            _ = &mut stop => return,
        }
    }
}

async fn record_drop(audit: &dyn McpAppAudit, record: McpAppAuditRecord) {
    let (conversation_id, call_id, phase) = (
        record.conversation_id.clone(),
        record.call_id.clone(),
        record.phase.clone(),
    );
    if let Err(error) = audit.record(record).await {
        tracing::error!(
            %conversation_id,
            %call_id,
            ?phase,
            ?error,
            "an MCP App context's drop could not be audited; it is dropped all the same"
        );
    }
}

#[cfg(test)]
#[path = "../../../tests/conversation/context_drops.rs"]
mod tests;
