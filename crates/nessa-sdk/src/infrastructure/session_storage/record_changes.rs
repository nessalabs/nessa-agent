//! Producer-owned registrations; publication never waits for a reader.
//!
//! Interest is indexed by session, so a commit wakes only the registrations
//! on its own session and those on every session, however many are open.
//! There is no gateway-wide count: each registration belongs to a live
//! subscription, which its socket's own published limits already bound.

#![deny(missing_docs)]

use crate::{
    application::agent_execution::sessions::{
        committed_changes::{CommittedChangeRegistration, CommittedChangeSignal},
        ChangeWatchError, CommittedChangeWatch,
    },
    domain::agent_execution::sessions::SessionId,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError, Weak},
};

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
    by_session: HashMap<SessionId, Vec<Arc<CommittedChangeSignal>>>,
    // Interest in every session's commits.
    any: Vec<Arc<CommittedChangeSignal>>,
}
impl State {
    fn bucket(
        &mut self,
        target: &Option<SessionId>,
    ) -> Option<&mut Vec<Arc<CommittedChangeSignal>>> {
        match target {
            Some(target) => self.by_session.get_mut(target),
            None => Some(&mut self.any),
        }
    }
    fn every(&self) -> impl Iterator<Item = &Arc<CommittedChangeSignal>> {
        self.by_session.values().flatten().chain(&self.any)
    }
}
struct Registration {
    registry: Weak<Registry>,
    target: Option<SessionId>,
    signal: Arc<CommittedChangeSignal>,
}

impl RecordChanges {
    pub fn watch(&self, target: SessionId) -> Result<CommittedChangeWatch, ChangeWatchError> {
        self.register(Some(target))
    }
    /// Interest in every session's commits.
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
        let signal = Arc::new(CommittedChangeSignal::default());
        match &target {
            Some(session) => state.by_session.entry(session.clone()).or_default(),
            None => &mut state.any,
        }
        .push(signal.clone());
        let registration = Registration {
            registry: Arc::downgrade(&self.inner),
            target,
            signal: signal.clone(),
        };
        Ok(CommittedChangeWatch::new(signal, Box::new(registration)))
    }
    /// How many registrations are held now.
    pub fn count(&self) -> usize {
        let state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        state.every().count()
    }
    pub fn publish(&self, target: &SessionId) {
        // Only this session's and every-session interest is captured. Waking
        // outside the registry lock permits a waker to retire/register interest.
        let signals = {
            let state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            state
                .by_session
                .get(target)
                .into_iter()
                .flatten()
                .chain(&state.any)
                .cloned()
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
            state.every().cloned().collect::<Vec<_>>()
        };
        for signal in signals {
            signal.close();
        }
    }
}
impl CommittedChangeRegistration for Registration {}

impl Drop for Registration {
    fn drop(&mut self) {
        let Some(registry) = self.registry.upgrade() else {
            return;
        };
        let mut state = registry
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let Some(bucket) = state.bucket(&self.target) else {
            return;
        };
        bucket.retain(|signal| !Arc::ptr_eq(signal, &self.signal));
        // An emptied session's entry goes with its last registration.
        if bucket.is_empty() {
            if let Some(target) = &self.target {
                state.by_session.remove(target);
            }
        }
    }
}
impl Drop for Registry {
    fn drop(&mut self) {
        for signal in self
            .state
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .every()
        {
            signal.close();
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

    /// Interest in every session wakes for each one's commit and coalesces
    /// like any other registration.
    #[tokio::test]
    async fn interest_in_every_session_wakes_for_each() {
        let producer = RecordChanges::default();
        let mut any = producer.watch_any().unwrap();
        pending(&mut any);
        producer.publish(&target("one"));
        producer.publish(&target("two"));
        assert_eq!(ready(&mut any), ChangeWatchState::Dirty);
        pending(&mut any);
        producer.publish(&target("three"));
        assert_eq!(ready(&mut any), ChangeWatchState::Dirty);
    }

    /// #978: no fixed gateway-wide count. Ten thousand registrations are all
    /// admitted, and a commit wakes only its own session's and every-session
    /// interest.
    #[tokio::test]
    async fn registrations_scale_and_a_commit_wakes_only_its_own_session() {
        let producer = RecordChanges::default();
        let mut watches = (0..10_000)
            .map(|index| producer.watch(target(&format!("session-{index}"))).unwrap())
            .collect::<Vec<_>>();
        let mut any = producer.watch_any().unwrap();
        producer.publish(&target("session-42"));
        assert_eq!(ready(&mut watches[42]), ChangeWatchState::Dirty);
        assert_eq!(ready(&mut any), ChangeWatchState::Dirty);
        pending(&mut watches[41]);
        pending(&mut watches[43]);
        drop(watches);
        assert!(
            producer.inner.state.lock().unwrap().by_session.is_empty(),
            "a session's entry goes with its last registration"
        );
    }

    #[tokio::test]
    async fn cancelled_wait_keeps_the_registration_until_its_handle_drops() {
        let producer = RecordChanges::default();
        let key = target("owner");
        let mut watches = (0..2)
            .map(|_| producer.watch(key.clone()).unwrap())
            .collect::<Vec<_>>();
        pending(&mut watches[0]);
        producer.publish(&key);
        assert_eq!(ready(&mut watches[0]), ChangeWatchState::Dirty);
        let signal = Arc::downgrade(&producer.inner.state.lock().unwrap().by_session[&key][0]);
        drop(watches.remove(0));
        assert!(
            signal.upgrade().is_none(),
            "retired registration retains no signal owner"
        );
        assert_eq!(
            producer.inner.state.lock().unwrap().by_session[&key].len(),
            1
        );
        producer.publish(&key);
        assert_eq!(ready(&mut watches[0]), ChangeWatchState::Dirty);
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
