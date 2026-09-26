//! Reaches an agent's own store of sessions through that agent's SDK binding.
//!
//! One adapter for every agent: which agent is asked is the registry's
//! decision, and what an answer means is the binding's. This only carries the
//! binding's answer across, claiming nothing it did not.
use crate::conversation::{
    application::{ConversationError, ConversationFuture, ProviderSessionEraser},
    domain::ProviderSessionErasure,
};
use nessa_sdk::{
    application::agent_execution::providers::{ProviderSessionDeleter, ProviderSessionDeletion},
    domain::agent_execution::sessions::ExecutionSessionId,
};
use std::{future::Future, pin::Pin, sync::Arc};

/// A [`ProviderSessionEraser`] backed by one agent binding's
/// [`ProviderSessionDeleter`].
pub struct BindingSessionEraser {
    binding: Arc<dyn ProviderSessionDeleter>,
}
impl BindingSessionEraser {
    /// Ask `binding` whenever a session of its agent is to be erased.
    pub fn new(binding: Arc<dyn ProviderSessionDeleter>) -> Self {
        Self { binding }
    }
}
impl ProviderSessionEraser for BindingSessionEraser {
    fn erase(&self, session: ExecutionSessionId) -> ConversationFuture<'_, ProviderSessionErasure> {
        Box::pin(async move {
            let deletion = self
                .binding
                .delete_session(session)
                .await
                .map_err(ConversationError::Agent)?;
            Ok(match deletion {
                ProviderSessionDeletion::Deleted => ProviderSessionErasure::Deleted,
                ProviderSessionDeletion::Archived => ProviderSessionErasure::Archived,
                ProviderSessionDeletion::Acknowledged => ProviderSessionErasure::Acknowledged,
                ProviderSessionDeletion::NotListed => ProviderSessionErasure::NotListed,
                ProviderSessionDeletion::NotSupported => ProviderSessionErasure::NotSupported,
            })
        })
    }
    fn settled(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        ProviderSessionDeleter::settled(&*self.binding)
    }
    fn cleanup_outstanding(&self) -> bool {
        ProviderSessionDeleter::cleanup_outstanding(&*self.binding)
    }
}

/// What a deletion launched and may still be releasing: an SDK binding or an
/// eraser. Implemented for the two kinds [`LaunchedDeletions`] keeps.
pub trait DeletionOwner: Send + Sync {
    fn settled(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
    fn cleanup_outstanding(&self) -> bool;
}
impl<T: ProviderSessionDeleter> DeletionOwner for T {
    fn settled(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        ProviderSessionDeleter::settled(self)
    }
    fn cleanup_outstanding(&self) -> bool {
        ProviderSessionDeleter::cleanup_outstanding(self)
    }
}
impl DeletionOwner for dyn ProviderSessionEraser {
    fn settled(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        ProviderSessionEraser::settled(self)
    }
    fn cleanup_outstanding(&self) -> bool {
        ProviderSessionEraser::cleanup_outstanding(self)
    }
}

/// The bindings a wrapper's deletions launched, kept until each is released.
///
/// The one rule for a wrapper that makes a binding per deletion (ADR 221):
/// `settled` waits on each within its own bound, and whatever is still
/// outstanding after that stays here, so `cleanup_outstanding` keeps saying so
/// until its process is actually gone
/// (`launched_deletions_keep_what_is_still_outstanding`).
pub struct LaunchedDeletions<T: ?Sized>(std::sync::Mutex<Vec<Arc<T>>>);
impl<T: ?Sized> Default for LaunchedDeletions<T> {
    fn default() -> Self {
        Self(std::sync::Mutex::new(Vec::new()))
    }
}
impl<T: DeletionOwner + ?Sized> LaunchedDeletions<T> {
    /// Keep `owner`, launched for one deletion.
    pub fn push(&self, owner: Arc<T>) {
        self.launched().push(owner);
    }
    /// Wait on each within its own bound, keeping what is still outstanding.
    pub async fn settled(&self) {
        let launched = std::mem::take(&mut *self.launched());
        let mut still_outstanding = Vec::new();
        for owner in launched {
            owner.settled().await;
            if owner.cleanup_outstanding() {
                still_outstanding.push(owner);
            }
        }
        self.launched().extend(still_outstanding);
    }
    /// Whether any kept owner may still hold a process or what it launched.
    pub fn cleanup_outstanding(&self) -> bool {
        self.launched()
            .iter()
            .any(|owner| owner.cleanup_outstanding())
    }
    fn launched(&self) -> std::sync::MutexGuard<'_, Vec<Arc<T>>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod launched_deletions_tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct Owner(AtomicBool);
    impl ProviderSessionEraser for Owner {
        fn erase(&self, _: ExecutionSessionId) -> ConversationFuture<'_, ProviderSessionErasure> {
            Box::pin(async { Ok(ProviderSessionErasure::NotSupported) })
        }
        fn cleanup_outstanding(&self) -> bool {
            self.0.load(Ordering::SeqCst)
        }
    }

    /// ADR 221: settling waits on each launched owner and keeps whatever is
    /// still outstanding, so the wrapper keeps saying so until it is released.
    #[tokio::test]
    async fn launched_deletions_keep_what_is_still_outstanding() {
        let released = Arc::new(Owner(AtomicBool::new(false)));
        let stopping = Arc::new(Owner(AtomicBool::new(true)));
        let launched = LaunchedDeletions::<dyn ProviderSessionEraser>::default();
        launched.push(released.clone());
        launched.push(stopping.clone());
        assert!(launched.cleanup_outstanding());

        launched.settled().await;
        assert!(
            launched.cleanup_outstanding(),
            "the one still stopping is kept"
        );
        assert_eq!(launched.launched().len(), 1);

        stopping.0.store(false, Ordering::SeqCst);
        assert!(!launched.cleanup_outstanding());
        launched.settled().await;
        assert!(launched.launched().is_empty());
    }
}
