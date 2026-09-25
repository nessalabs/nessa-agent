use crate::conversation::domain::ConversationId;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex as StdMutex, PoisonError, Weak},
};
use tokio::sync::{Mutex, OwnedMutexGuard};

/// One lock per conversation, so work on one conversation never waits behind
/// another's (`summary_writes_of_one_conversation_do_not_wait_for_another`).
///
/// A conversation's lock exists while somebody holds it or waits for it, and
/// is forgotten once nobody does: the table grows with the conversations in
/// use at once, not with every conversation there ever was
/// (`a_conversation_lock_nobody_holds_is_forgotten`).
#[derive(Default)]
pub(super) struct ConversationLocks {
    locks: StdMutex<HashMap<ConversationId, Weak<Mutex<()>>>>,
}
impl ConversationLocks {
    /// Wait for `id`'s lock and hold it until the guard is dropped.
    pub(super) async fn lock(&self, id: &ConversationId) -> OwnedMutexGuard<()> {
        self.entry(id).lock_owned().await
    }
    /// `id`'s lock if nobody holds it now, or `None` without waiting.
    pub(super) fn try_lock(&self, id: &ConversationId) -> Option<OwnedMutexGuard<()>> {
        self.entry(id).try_lock_owned().ok()
    }
    fn entry(&self, id: &ConversationId) -> Arc<Mutex<()>> {
        {
            // Nothing is awaited while this is held, so a poisoned table is
            // one whose last change finished: taking it back is safe.
            let mut locks = self.locks.lock().unwrap_or_else(PoisonError::into_inner);
            locks.retain(|_, lock| lock.strong_count() > 0);
            match locks.get(id).and_then(Weak::upgrade) {
                Some(lock) => lock,
                None => {
                    let lock = Arc::new(Mutex::new(()));
                    locks.insert(id.clone(), Arc::downgrade(&lock));
                    lock
                }
            }
        }
    }
    /// How many conversations have a lock somebody holds or waits for.
    #[cfg(test)]
    pub(super) fn in_use(&self) -> usize {
        let mut locks = self.locks.lock().unwrap_or_else(PoisonError::into_inner);
        locks.retain(|_, lock| lock.strong_count() > 0);
        locks.len()
    }
}
