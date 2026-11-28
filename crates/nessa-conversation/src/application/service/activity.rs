//! What runs on this gateway now, whoever started it (`server.activity`).
//!
//! Every other read is scoped to its caller's own conversations, so a list
//! asked with one credential misses work started with another. This one
//! answers for the whole gateway, so a cloud box is never put to sleep under
//! a turn somebody else started (#1427).
//!
//! ```text
//! each conversation slot:
//!   still opening ──▶ running (an opening is work under way)
//!   open, agent busy or an app call waiting on a review ──▶ running
//!   open and idle ──▶ not counted
//! background commands ──▶ counted by their owner (any conversation)
//! ```
use super::ConversationService;
use std::sync::Arc;

/// What runs on this gateway now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GatewayActivity {
    /// Conversations opening, or open with a turn running, queued, or waiting
    /// on a review.
    pub running_conversations: usize,
    /// Background commands an agent started that have not ended.
    pub background_commands: usize,
}

impl GatewayActivity {
    /// Whether anything at all is under way.
    pub fn busy(&self) -> bool {
        self.running_conversations > 0 || self.background_commands > 0
    }
}

impl ConversationService {
    /// What runs on this gateway now, for every principal.
    ///
    /// The slots are copied out and the lock let go before any agent is
    /// asked, so a slow agent never holds up an opening.
    pub async fn activity(&self) -> GatewayActivity {
        let slots: Vec<_> = self
            .inner
            .conversations
            .lock()
            .await
            .values()
            .map(Arc::clone)
            .collect();
        let mut running_conversations = 0;
        for slot in slots {
            let busy = match slot.value.get() {
                // Still opening: something is about to run there.
                None => true,
                // An opening that failed holds nothing that runs.
                Some(Err(_)) => false,
                Some(Ok(live)) => {
                    !live.app_reviews.entries().is_empty() || !live.agent.is_idle().await
                }
            };
            if busy {
                running_conversations += 1;
            }
        }
        let background_commands = self
            .inner
            .background_tasks
            .get()
            .map_or(0, |tasks| tasks.running_count());
        GatewayActivity {
            running_conversations,
            background_commands,
        }
    }
}
