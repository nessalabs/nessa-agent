//! Wakes a reader of a conversation's view when a live fact that is not a
//! committed record changed: the agent's opening, a slot let go, the summary
//! (the view's title), an approval-mode change, an MCP App's pending reviews.
//! Committed records wake readers through the SDK's own watch; this owns only
//! what the records do not say.
//!
//! A change is a counter bump on a `tokio::sync::watch` value, so publishing
//! never waits for a reader and many changes before a read collapse into one.
//! A reader subscribes **before** it reads: a change during or after the read
//! then leaves its receiver changed, and nothing is missed
//! (`a_change_after_subscribing_is_seen_and_changes_collapse`).

use nessa_protocol::conversation::domain::ConversationId;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError},
};
use tokio::sync::watch;

/// One signal per conversation that somebody follows. A conversation nobody
/// follows has no entry; its entry goes once its last receiver has.
#[derive(Default)]
pub struct LiveChanges {
    senders: Mutex<HashMap<ConversationId, watch::Sender<u64>>>,
}

impl LiveChanges {
    /// Follow `id`'s live facts. The receiver starts as seen.
    pub fn subscribe(&self, id: &ConversationId) -> watch::Receiver<u64> {
        let mut senders = self.senders.lock().unwrap_or_else(PoisonError::into_inner);
        // Entries nobody receives any more go here, so the map is bounded by
        // the receivers alive plus this one.
        senders.retain(|_, sender| sender.receiver_count() > 0);
        senders
            .entry(id.clone())
            .or_insert_with(|| watch::channel(0).0)
            .subscribe()
    }

    /// Something live about `id` changed. Never waits.
    pub fn publish(&self, id: &ConversationId) {
        let senders = self.senders.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(sender) = senders.get(id) {
            sender.send_modify(|generation| *generation = generation.wrapping_add(1));
        }
    }

    /// A publisher bound to one conversation, for an owner that knows only
    /// its own conversation's changes (an MCP App's reviews).
    pub fn publisher(self: &Arc<Self>, id: &ConversationId) -> LiveChangePublisher {
        let changes = self.clone();
        let id = id.clone();
        Arc::new(move || changes.publish(&id))
    }

    #[cfg(test)]
    fn entries(&self) -> usize {
        self.senders
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }
}

/// Says one conversation's live facts changed.
pub type LiveChangePublisher = Arc<dyn Fn() + Send + Sync>;

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u8) -> ConversationId {
        ConversationId::new(&format!("00000000-0000-4000-8000-0000000000{n:02}")).unwrap()
    }

    #[tokio::test]
    async fn a_change_after_subscribing_is_seen_and_changes_collapse() {
        let changes = LiveChanges::default();
        let mut receiver = changes.subscribe(&id(1));
        assert!(!receiver.has_changed().unwrap());
        changes.publish(&id(2));
        assert!(!receiver.has_changed().unwrap(), "another conversation's change");
        changes.publish(&id(1));
        changes.publish(&id(1));
        receiver.changed().await.unwrap();
        assert!(!receiver.has_changed().unwrap(), "two changes are one wake");
    }

    #[test]
    fn an_entry_goes_with_its_last_receiver() {
        let changes = LiveChanges::default();
        let first = changes.subscribe(&id(1));
        drop(changes.subscribe(&id(2)));
        assert_eq!(changes.entries(), 2);
        let _third = changes.subscribe(&id(3));
        assert_eq!(changes.entries(), 2, "the second's entry went");
        drop(first);
        changes.publish(&id(1));
        let _again = changes.subscribe(&id(3));
        assert_eq!(changes.entries(), 1);
    }
}
