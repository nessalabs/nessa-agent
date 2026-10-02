//! Payloadless interest in committed changes; source reads remain authoritative.

#![deny(missing_docs)]

use nessa_auth::domain::{OrganizationId, PrincipalId};
#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{Arc, Mutex, PoisonError},
};
use tokio::sync::Notify;

/// Result of consuming a source change notice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatalogueWatchState {
    /// At least one local durable change occurred; recheck the current source.
    Dirty,
    /// The publisher closed; use a newly composed source or fallback reads.
    Closed,
    /// A notification callback unwound; use fallback reads or a new watch.
    NotificationFailed,
}

/// A bounded source registration was refused before allocating a watch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatalogueWatchError {
    /// The producer's published local registration limit is occupied.
    Capacity,
}

// Implemented by the producer guard whose Drop retires the actual slot.
pub(crate) trait CatalogueWatchRegistration: Send + Sync {}

/// One non-cloneable registration. Drop retires its producer slot synchronously.
///
/// Notices are advisory, process-local and coalesced; they contain no record,
/// cursor, permission or receiver progress. Register before the final authorized
/// head recheck. A commit before registration is found by that recheck; an
/// incomplete append or another process's mutation requires fallback reads.
/// The owning source tests cover capacity, cancellation and last-owner closure.
pub struct CatalogueChangeWatch {
    signal: Arc<CatalogueChangeSignal>,
    // The concrete producer guard owns unregister-on-drop, not the waiter.
    _registration: Box<dyn CatalogueWatchRegistration>,
}

impl CatalogueChangeWatch {
    pub(crate) fn new(
        signal: Arc<CatalogueChangeSignal>,
        registration: Box<dyn CatalogueWatchRegistration>,
    ) -> Self {
        Self {
            signal,
            _registration: registration,
        }
    }

    /// Wait for a coalesced notice, source closure, or notification failure.
    ///
    /// A cancelled pending wait preserves its registration and dirty bit.
    /// Multiple commits while dirty occupy one bit; a later commit after a
    /// consumed notice leaves a new notice. This wait does not authorize a read.
    pub async fn changed(&mut self) -> CatalogueWatchState {
        loop {
            // Notify::notified captures notify_waiters calls at creation, before
            // its first poll. Create it before the predicate check so closure
            // between check and await is retained; notify_one stores a permit
            // for this non-cloneable handle's single waiter.
            let notified = self.signal.ready.notified();
            {
                let mut state = self
                    .signal
                    .state
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                if let Some(terminal) = state.terminal {
                    return match terminal {
                        Terminal::Closed => CatalogueWatchState::Closed,
                        Terminal::NotificationFailed => CatalogueWatchState::NotificationFailed,
                    };
                }
                if state.dirty {
                    state.dirty = false;
                    return CatalogueWatchState::Dirty;
                }
            }
            #[cfg(test)]
            if self.signal.close_before_wait.swap(false, Ordering::SeqCst) {
                self.signal.close();
            }
            notified.await;
        }
    }
}

#[derive(Clone, Copy)]
enum Terminal {
    Closed,
    NotificationFailed,
}

#[derive(Default)]
struct SignalState {
    dirty: bool,
    terminal: Option<Terminal>,
}

#[derive(Default)]
pub(crate) struct CatalogueChangeSignal {
    state: Mutex<SignalState>,
    ready: Notify,
    #[cfg(test)]
    close_before_wait: AtomicBool,
}
impl CatalogueChangeSignal {
    pub(crate) fn publish(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.terminal.is_some() {
            return;
        }
        state.dirty = true;
        drop(state);
        // Notify may invoke a waker synchronously; hold no predicate lock.
        if catch_unwind(AssertUnwindSafe(|| self.ready.notify_one())).is_err() {
            self.state
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .terminal
                .get_or_insert(Terminal::NotificationFailed);
        }
    }
    pub(crate) fn close(&self) {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .terminal
            .get_or_insert(Terminal::Closed);
        // Closure is installed before the callback, so a callback unwind does
        // not replace it. Catch only notification, never persistence work.
        let _ = catch_unwind(AssertUnwindSafe(|| self.ready.notify_waiters()));
    }
}

/// Register advisory interest in one authenticated owner's catalogue.
/// The caller supplies its current authenticated owner and separately rechecks
/// head after registration; this port provides no authorization or source read.
pub trait WatchCatalogue: Send + Sync {
    /// Register before the final head recheck. Refuses full producer admission.
    fn watch(
        &self,
        organization: &OrganizationId,
        owner: &PrincipalId,
    ) -> Result<CatalogueChangeWatch, CatalogueWatchError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        future::Future,
        sync::Weak,
        task::{Context, Poll, Wake, Waker},
    };

    struct Registration;
    impl CatalogueWatchRegistration for Registration {}

    struct WakeChecksPredicate {
        signal: Weak<CatalogueChangeSignal>,
        unlocked: AtomicBool,
    }
    impl Wake for WakeChecksPredicate {
        fn wake(self: Arc<Self>) {
            self.unlocked.store(
                self.signal.upgrade().unwrap().state.try_lock().is_ok(),
                Ordering::SeqCst,
            );
        }
    }

    #[test]
    fn publication_unlocks_the_predicate_before_invoking_a_waker() {
        let signal = Arc::new(CatalogueChangeSignal::default());
        let observer = Arc::new(WakeChecksPredicate {
            signal: Arc::downgrade(&signal),
            unlocked: AtomicBool::new(false),
        });
        let waker = Waker::from(observer.clone());
        let mut watch = CatalogueChangeWatch::new(signal.clone(), Box::new(Registration));
        let mut wait = Box::pin(watch.changed());
        assert!(matches!(
            wait.as_mut().poll(&mut Context::from_waker(&waker)),
            Poll::Pending
        ));
        signal.publish();
        assert!(
            observer.unlocked.load(Ordering::SeqCst),
            "waker can reenter the signal predicate"
        );
        assert!(matches!(
            wait.as_mut().poll(&mut Context::from_waker(&waker)),
            Poll::Ready(CatalogueWatchState::Dirty)
        ));
    }

    struct PanickingWake;
    impl Wake for PanickingWake {
        fn wake(self: Arc<Self>) {
            panic!("notification callback");
        }
    }

    #[test]
    fn close_callback_failure_preserves_source_closed() {
        let signal = Arc::new(CatalogueChangeSignal::default());
        let mut watch = CatalogueChangeWatch::new(signal.clone(), Box::new(Registration));
        let waker = Waker::from(Arc::new(PanickingWake));
        let mut wait = Box::pin(watch.changed());
        assert!(matches!(
            wait.as_mut().poll(&mut Context::from_waker(&waker)),
            Poll::Pending
        ));
        signal.close();
        drop(wait);
        signal.publish();
        assert!(matches!(
            Box::pin(watch.changed())
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(CatalogueWatchState::Closed)
        ));
    }

    #[test]
    fn close_between_predicate_check_and_first_wait_poll_is_not_lost() {
        let signal = Arc::new(CatalogueChangeSignal::default());
        signal.close_before_wait.store(true, Ordering::SeqCst);
        let mut watch = CatalogueChangeWatch::new(signal, Box::new(Registration));
        let mut wait = Box::pin(watch.changed());
        // The private test control closes at the exact non-sticky notify_waiters
        // boundary, after the predicate check and before Notified's first poll.
        assert!(matches!(
            wait.as_mut().poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(CatalogueWatchState::Closed)
        ));
    }
}
