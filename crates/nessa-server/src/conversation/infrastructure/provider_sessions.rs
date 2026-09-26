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
pub(crate) trait DeletionOwner: Send + Sync {
    /// Wait, within the owner's own bound, for what it launched to be released.
    fn settled(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
    /// Whether what it launched may still hold a process, past that bound too.
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
pub(crate) struct LaunchedDeletions<T: ?Sized>(std::sync::Mutex<Vec<Arc<T>>>);
impl<T: ?Sized> Default for LaunchedDeletions<T> {
    fn default() -> Self {
        Self(std::sync::Mutex::new(Vec::new()))
    }
}
impl<T: DeletionOwner + ?Sized> LaunchedDeletions<T> {
    /// Keep `owner`, launched for one deletion.
    pub(crate) fn push(&self, owner: Arc<T>) {
        self.launched().push(owner);
    }
    /// Wait on each within its own bound, then let go of what is released.
    ///
    /// Nothing is removed while waiting, so `cleanup_outstanding` asked at the
    /// same time, by a retirement racing a shutdown, still sees every owner
    /// (`cleanup_outstanding_is_answered_while_settling`).
    pub(crate) async fn settled(&self) {
        let launched = self.launched().clone();
        for owner in &launched {
            owner.settled().await;
        }
        self.launched().retain(|owner| owner.cleanup_outstanding());
    }
    /// Whether any kept owner may still hold a process or what it launched.
    pub(crate) fn cleanup_outstanding(&self) -> bool {
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

    /// ADR 221: a retirement racing a shutdown asks while `settled` is still
    /// waiting; the owner being waited on is still counted.
    #[tokio::test]
    async fn cleanup_outstanding_is_answered_while_settling() {
        struct Slow {
            outstanding: AtomicBool,
            release: tokio::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
        }
        impl ProviderSessionEraser for Slow {
            fn erase(
                &self,
                _: ExecutionSessionId,
            ) -> ConversationFuture<'_, ProviderSessionErasure> {
                Box::pin(async { Ok(ProviderSessionErasure::NotSupported) })
            }
            fn settled(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
                Box::pin(async move {
                    if let Some(release) = self.release.lock().await.take() {
                        let _ = release.await;
                    }
                })
            }
            fn cleanup_outstanding(&self) -> bool {
                self.outstanding.load(Ordering::SeqCst)
            }
        }
        let (release, waiting) = tokio::sync::oneshot::channel();
        let slow = Arc::new(Slow {
            outstanding: AtomicBool::new(true),
            release: tokio::sync::Mutex::new(Some(waiting)),
        });
        let launched = Arc::new(LaunchedDeletions::<dyn ProviderSessionEraser>::default());
        launched.push(slow.clone());
        let settling = tokio::spawn({
            let launched = launched.clone();
            async move { launched.settled().await }
        });
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert!(launched.cleanup_outstanding(), "counted while settling");
        slow.outstanding.store(false, Ordering::SeqCst);
        release.send(()).unwrap();
        settling.await.unwrap();
        assert!(launched.launched().is_empty());
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
