//! Bounded producer-owned registrations; publication never waits for a reader.

#![deny(missing_docs)]

use crate::{
    application::agent_execution::sessions::{
        committed_changes::{CommittedChangeRegistration, CommittedChangeSignal},
        ChangeWatchError, CommittedChangeWatch,
    },
    domain::agent_execution::sessions::SessionId,
};
use std::sync::{Arc, Mutex, PoisonError, Weak};

/// Maximum actual registrations in this producer, separate from gateway policy.
pub const MAX_RECORD_CHANGE_WATCHES: usize = 64;

#[derive(Clone, Default)]
pub(super) struct RecordChanges {
    inner: Arc<Registry>,
}
#[derive(Default)]
struct Registry {
    state: Mutex<State>,
}
#[derive(Default)]
struct State {
    closed: bool,
    entries: Vec<Entry>,
}
struct Entry {
    // `None` is interest in every session's commits.
    target: Option<SessionId>,
    signal: Arc<CommittedChangeSignal>,
}
struct Registration {
    registry: Weak<Registry>,
    signal: Arc<CommittedChangeSignal>,
}

impl RecordChanges {
    pub fn watch(&self, target: SessionId) -> Result<CommittedChangeWatch, ChangeWatchError> {
        self.register(Some(target))
    }
    /// Interest in every session's commits, under the same producer bound.
    pub fn watch_any(&self) -> Result<CommittedChangeWatch, ChangeWatchError> {
        self.register(None)
    }
    fn register(
        &self,
        target: Option<SessionId>,
    ) -> Result<CommittedChangeWatch, ChangeWatchError> {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if state.closed {
            return Err(ChangeWatchError::Closed);
        }
        if state.entries.len() == MAX_RECORD_CHANGE_WATCHES {
            return Err(ChangeWatchError::Capacity);
        }
        let signal = Arc::new(CommittedChangeSignal::default());
        state.entries.push(Entry {
            target,
            signal: signal.clone(),
        });
        let registration = Registration {
            registry: Arc::downgrade(&self.inner),
            signal: signal.clone(),
        };
        Ok(CommittedChangeWatch::new(signal, Box::new(registration)))
    }
    pub fn publish(&self, target: &SessionId) {
        // At most the published registration limit is captured. Waking outside
        // the registry lock permits a waker to retire/register source interest.
        let signals = {
            let state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            state
                .entries
                .iter()
                .filter(|entry| entry.target.as_ref().is_none_or(|own| own == target))
                .map(|entry| entry.signal.clone())
                .collect::<Vec<_>>()
        };
        for signal in signals {
            signal.publish();
        }
    }
    pub fn close(&self) {
        let signals = {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            state.closed = true;
            state
                .entries
                .iter()
                .map(|entry| entry.signal.clone())
                .collect::<Vec<_>>()
        };
        for signal in signals {
            signal.close();
        }
    }
}
impl CommittedChangeRegistration for Registration {}

impl Drop for Registration {
    fn drop(&mut self) {
        if let Some(registry) = self.registry.upgrade() {
            registry
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .entries
                .retain(|entry| !Arc::ptr_eq(&entry.signal, &self.signal));
        }
    }
}
impl Drop for Registry {
    fn drop(&mut self) {
        for entry in &self
            .state
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .entries
        {
            entry.signal.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::agent_execution::sessions::ChangeWatchState;
    use std::{
        future::Future,
        sync::atomic::{AtomicBool, Ordering},
        task::{Context, Poll, Wake, Waker},
    };

    #[derive(Default)]
    struct WakeObserved {
        woken: AtomicBool,
        registry: Weak<Registry>,
        lock_released: AtomicBool,
    }
    impl Wake for WakeObserved {
        fn wake(self: Arc<Self>) {
            self.lock_released.store(
                self.registry
                    .upgrade()
                    .is_none_or(|registry| registry.state.try_lock().is_ok()),
                Ordering::SeqCst,
            );
            self.woken.store(true, Ordering::SeqCst);
        }
    }

    fn target(value: &str) -> SessionId {
        SessionId::new(value).unwrap()
    }
    fn ready(watch: &mut CommittedChangeWatch) -> ChangeWatchState {
        let mut wait = Box::pin(watch.changed());
        match wait.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
            Poll::Ready(state) => state,
            Poll::Pending => panic!("expected a committed source notice"),
        }
    }

    fn pending(watch: &mut CommittedChangeWatch) {
        let mut wait = Box::pin(watch.changed());
        assert!(matches!(
            wait.as_mut().poll(&mut Context::from_waker(Waker::noop())),
            Poll::Pending
        ));
        // Dropping this future cancels only this wait, not the registration.
    }

    #[tokio::test]
    async fn actual_waiter_is_woken_by_commit_and_final_publisher_close() {
        let producer = RecordChanges::default();
        let key = target("owner");
        let mut watched = producer.watch(key.clone()).unwrap();
        let observed = Arc::new(WakeObserved {
            registry: Arc::downgrade(&producer.inner),
            ..WakeObserved::default()
        });
        let waker = Waker::from(observed.clone());
        let mut wait = Box::pin(watched.changed());
        assert!(matches!(
            wait.as_mut().poll(&mut Context::from_waker(&waker)),
            Poll::Pending
        ));
        producer.publish(&key);
        assert!(
            observed.woken.swap(false, Ordering::SeqCst),
            "publisher wakes the enabled waiter"
        );
        assert!(
            observed.lock_released.load(Ordering::SeqCst),
            "publication holds no registry lock while waking"
        );
        assert_eq!(wait.await, ChangeWatchState::Dirty);
        let mut wait = Box::pin(watched.changed());
        assert!(matches!(
            wait.as_mut().poll(&mut Context::from_waker(&waker)),
            Poll::Pending
        ));
        drop(producer);
        assert!(
            observed.woken.load(Ordering::SeqCst),
            "last publisher wakes the enabled waiter"
        );
        assert_eq!(wait.await, ChangeWatchState::Closed);
    }

    /// Interest in every session wakes for each one's commit, shares the one
    /// producer bound, and coalesces like any other registration.
    #[tokio::test]
    async fn interest_in_every_session_wakes_for_each_and_shares_the_bound() {
        let producer = RecordChanges::default();
        let mut any = producer.watch_any().unwrap();
        pending(&mut any);
        producer.publish(&target("one"));
        producer.publish(&target("two"));
        assert_eq!(ready(&mut any), ChangeWatchState::Dirty);
        pending(&mut any);
        producer.publish(&target("three"));
        assert_eq!(ready(&mut any), ChangeWatchState::Dirty);
        let _rest = (1..MAX_RECORD_CHANGE_WATCHES)
            .map(|_| producer.watch(target("one")).unwrap())
            .collect::<Vec<_>>();
        assert!(matches!(producer.watch_any(), Err(ChangeWatchError::Capacity)));
        drop(any);
        assert!(producer.watch_any().is_ok());
    }

    #[tokio::test]
    async fn cancelled_wait_retains_capacity_until_actual_registration_drop() {
        let producer = RecordChanges::default();
        let key = target("owner");
        let mut watches = (0..MAX_RECORD_CHANGE_WATCHES)
            .map(|_| producer.watch(key.clone()).unwrap())
            .collect::<Vec<_>>();
        pending(&mut watches[0]);
        assert!(matches!(
            producer.watch(key.clone()),
            Err(ChangeWatchError::Capacity)
        ));
        producer.publish(&key);
        assert_eq!(ready(&mut watches[0]), ChangeWatchState::Dirty);
        assert!(matches!(
            producer.watch(key.clone()),
            Err(ChangeWatchError::Capacity)
        ));
        let signal = Arc::downgrade(&producer.inner.state.lock().unwrap().entries[0].signal);
        drop(watches.remove(0));
        assert!(
            signal.upgrade().is_none(),
            "retired registration retains no signal owner"
        );
        assert!(producer.watch(key).is_ok());
    }

    #[tokio::test]
    async fn commits_coalesce_and_a_commit_after_consumption_leaves_another_notice() {
        let producer = RecordChanges::default();
        let key = target("owner");
        let mut watched = producer.watch(key.clone()).unwrap();
        let mut other = producer.watch(target("other")).unwrap();
        pending(&mut watched);
        for _ in 0..100 {
            producer.publish(&key);
        }
        assert_eq!(ready(&mut watched), ChangeWatchState::Dirty);
        pending(&mut watched);
        pending(&mut other);
        producer.publish(&key);
        assert_eq!(ready(&mut watched), ChangeWatchState::Dirty);
        pending(&mut watched);
    }

    #[tokio::test]
    async fn final_publisher_drop_closes_waiters_after_the_last_retained_owner() {
        let producer = RecordChanges::default();
        let retained = producer.clone();
        let registry = Arc::downgrade(&producer.inner);
        let mut watched = producer.watch(target("owner")).unwrap();
        drop(producer);
        pending(&mut watched);
        drop(retained);
        assert!(
            registry.upgrade().is_none(),
            "handle does not keep publisher alive"
        );
        assert_eq!(ready(&mut watched), ChangeWatchState::Closed);
        assert_eq!(ready(&mut watched), ChangeWatchState::Closed);
    }

    #[tokio::test]
    async fn closure_wakes_an_enabled_wait_and_refuses_new_admission() {
        let producer = RecordChanges::default();
        let mut watched = producer.watch(target("owner")).unwrap();
        let observed = Arc::new(WakeObserved {
            registry: Arc::downgrade(&producer.inner),
            ..WakeObserved::default()
        });
        let waker = Waker::from(observed.clone());
        let mut wait = Box::pin(watched.changed());
        assert!(matches!(
            wait.as_mut().poll(&mut Context::from_waker(&waker)),
            Poll::Pending
        ));
        producer.close();
        assert!(observed.woken.load(Ordering::SeqCst));
        assert!(
            observed.lock_released.load(Ordering::SeqCst),
            "admission close wakes outside the registry lock"
        );
        assert_eq!(wait.await, ChangeWatchState::Closed);
        assert!(matches!(
            producer.watch(target("owner")),
            Err(ChangeWatchError::Closed)
        ));
    }
}
