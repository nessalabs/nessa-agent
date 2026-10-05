//! Payloadless interest in committed changes; source reads remain authoritative.

#![deny(missing_docs)]

#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::Notify;

use crate::application::agent_execution::caller_wake::contain_caller_wake;

/// Result of consuming a source change notice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeWatchState {
    /// At least one local durable change occurred; recheck the current source.
    Dirty,
    /// The publisher closed; use a newly composed source or fallback reads.
    Closed,
}

/// A bounded source registration was refused before allocating a watch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeWatchError {
    /// The producer's published local registration limit is occupied.
    Capacity,
    /// The publisher has closed its admission.
    Closed,
}

// Implemented by the producer guard whose Drop retires the actual slot.
pub(crate) trait CommittedChangeRegistration: Send + Sync {}

/// One non-cloneable registration. Drop retires its producer slot synchronously.
///
/// Notices are advisory, process-local and coalesced; they contain no record,
/// cursor, permission or receiver progress. Register before the final authorized
/// head recheck. A commit before registration is found by that recheck; an
/// incomplete append or another process's mutation requires fallback reads.
/// The owning source tests cover capacity, cancellation and last-owner closure.
pub struct CommittedChangeWatch {
    signal: Arc<CommittedChangeSignal>,
    // The concrete producer guard owns unregister-on-drop, not the waiter.
    _registration: Box<dyn CommittedChangeRegistration>,
}

impl CommittedChangeWatch {
    pub(crate) fn new(
        signal: Arc<CommittedChangeSignal>,
        registration: Box<dyn CommittedChangeRegistration>,
    ) -> Self {
        Self {
            signal,
            _registration: registration,
        }
    }

    /// Wait for a coalesced notice or source closure.
    ///
    /// A cancelled pending wait preserves its registration and dirty bit.
    /// Multiple commits while dirty occupy one bit; a later commit after a
    /// consumed notice leaves a new notice. A caller waker that panics loses
    /// that wake; the notice stays, and polling again returns it. This wait
    /// does not authorize a read.
    pub async fn changed(&mut self) -> ChangeWatchState {
        contain_caller_wake("committed change watch", async {
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
                    if state.closed {
                        return ChangeWatchState::Closed;
                    }
                    if state.dirty {
                        state.dirty = false;
                        return ChangeWatchState::Dirty;
                    }
                }
                #[cfg(test)]
                if self.signal.close_before_wait.swap(false, Ordering::SeqCst) {
                    self.signal.close();
                }
                notified.await;
            }
        })
        .await
    }
}

#[derive(Default)]
struct SignalState {
    dirty: bool,
    closed: bool,
}

#[derive(Default)]
pub(crate) struct CommittedChangeSignal {
    state: Mutex<SignalState>,
    ready: Notify,
    #[cfg(test)]
    close_before_wait: AtomicBool,
}
impl CommittedChangeSignal {
    pub(crate) fn publish(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.closed {
            return;
        }
        state.dirty = true;
        drop(state);
        // Notify may invoke a waker synchronously; hold no predicate lock.
        // A caller-waker panic is contained by `changed`, not here.
        self.ready.notify_one();
    }
    pub(crate) fn close(&self) {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .closed = true;
        // Closure is installed before the callback. A caller-waker panic is
        // contained by `changed`, so it cannot replace Closed or unwind this
        // publisher.
        self.ready.notify_waiters();
    }
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
    impl CommittedChangeRegistration for Registration {}

    struct WakeChecksPredicate {
        signal: Weak<CommittedChangeSignal>,
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
        let signal = Arc::new(CommittedChangeSignal::default());
        let observer = Arc::new(WakeChecksPredicate {
            signal: Arc::downgrade(&signal),
            unlocked: AtomicBool::new(false),
        });
        let waker = Waker::from(observer.clone());
        let mut watch = CommittedChangeWatch::new(signal.clone(), Box::new(Registration));
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
            Poll::Ready(ChangeWatchState::Dirty)
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
        let signal = Arc::new(CommittedChangeSignal::default());
        let mut watch = CommittedChangeWatch::new(signal.clone(), Box::new(Registration));
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
            Poll::Ready(ChangeWatchState::Closed)
        ));
    }

    #[test]
    fn close_between_predicate_check_and_first_wait_poll_is_not_lost() {
        let signal = Arc::new(CommittedChangeSignal::default());
        signal.close_before_wait.store(true, Ordering::SeqCst);
        let mut watch = CommittedChangeWatch::new(signal, Box::new(Registration));
        let mut wait = Box::pin(watch.changed());
        // The private test control closes at the exact non-sticky notify_waiters
        // boundary, after the predicate check and before Notified's first poll.
        assert!(matches!(
            wait.as_mut().poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(ChangeWatchState::Closed)
        ));
    }
}
