//! Bounded producer-owned registrations; publication never waits for a reader.

#![deny(missing_docs)]

use crate::conversation::application::{
    catalogue_watch::{CatalogueChangeSignal, CatalogueWatchRegistration},
    CatalogueChangeWatch, CatalogueWatchError,
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError, Weak},
};

/// Maximum actual registrations in this producer, separate from gateway policy.
pub const MAX_CATALOGUE_CHANGE_WATCHES: usize = 64;

#[derive(Clone, Default)]
pub(super) struct CatalogueChanges {
    inner: Arc<Registry>,
}
#[derive(Default)]
struct Registry {
    state: Mutex<State>,
}
#[derive(Default)]
struct State {
    entries: Vec<Entry>,
}
struct Entry {
    target: (OrganizationId, PrincipalId),
    signal: Arc<CatalogueChangeSignal>,
    receiver: Option<String>,
}
struct Registration {
    registry: Weak<Registry>,
    signal: Arc<CatalogueChangeSignal>,
}

impl CatalogueChanges {
    pub fn watch(
        &self,
        target: (OrganizationId, PrincipalId),
    ) -> Result<CatalogueChangeWatch, CatalogueWatchError> {
        self.watch_for(target, None)
    }

    pub fn watch_for(
        &self,
        target: (OrganizationId, PrincipalId),
        receiver: Option<&str>,
    ) -> Result<CatalogueChangeWatch, CatalogueWatchError> {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if state.entries.len() == MAX_CATALOGUE_CHANGE_WATCHES {
            return Err(CatalogueWatchError::Capacity);
        }
        let signal = Arc::new(CatalogueChangeSignal::default());
        state.entries.push(Entry {
            target,
            receiver: receiver.map(str::to_owned),
            signal: signal.clone(),
        });
        let registration = Registration {
            registry: Arc::downgrade(&self.inner),
            signal: signal.clone(),
        };
        Ok(CatalogueChangeWatch::new(signal, Box::new(registration)))
    }
    /// Capture recipients while the store still owns its committed revision.
    /// Reader heads are the catalogue's authority; this registry selects no grants.
    pub fn capture(
        &self,
        target: &(OrganizationId, PrincipalId),
        mut changed: impl FnMut(&str) -> bool,
    ) -> CataloguePublication {
        let state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut heads = HashMap::new();
        CataloguePublication {
            signals: state
                .entries
                .iter()
                .filter(|entry| &entry.target == target)
                .filter(|entry| {
                    entry.receiver.as_deref().is_none_or(|receiver| {
                        *heads.entry(receiver).or_insert_with(|| changed(receiver))
                    })
                })
                .map(|entry| entry.signal.clone())
                .collect(),
        }
    }

    #[cfg(test)]
    pub fn publish(&self, target: &(OrganizationId, PrincipalId)) {
        self.capture(target, |_| true).publish();
    }
}

/// At most one signal per registered watch. The blocking worker publishes after
/// releasing the database mutex, even when its asynchronous caller disappeared.
#[derive(Default)]
pub(super) struct CataloguePublication {
    signals: Vec<Arc<CatalogueChangeSignal>>,
}
impl CataloguePublication {
    pub fn publish(self) {
        for signal in self.signals {
            signal.publish();
        }
    }
}

impl CatalogueWatchRegistration for Registration {}

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
    use crate::conversation::application::CatalogueWatchState;
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

    fn target(value: &str) -> (OrganizationId, PrincipalId) {
        (
            OrganizationId::new("org").unwrap(),
            PrincipalId::new(value).unwrap(),
        )
    }
    fn ready(watch: &mut CatalogueChangeWatch) -> CatalogueWatchState {
        let mut wait = Box::pin(watch.changed());
        match wait.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
            Poll::Ready(state) => state,
            Poll::Pending => panic!("expected a committed source notice"),
        }
    }

    fn pending(watch: &mut CatalogueChangeWatch) {
        let mut wait = Box::pin(watch.changed());
        assert!(matches!(
            wait.as_mut().poll(&mut Context::from_waker(Waker::noop())),
            Poll::Pending
        ));
        // Dropping this future cancels only this wait, not the registration.
    }

    #[tokio::test]
    async fn actual_waiter_is_woken_by_commit_and_final_publisher_close() {
        let producer = CatalogueChanges::default();
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
        assert_eq!(wait.await, CatalogueWatchState::Dirty);
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
        assert_eq!(wait.await, CatalogueWatchState::Closed);
    }

    #[tokio::test]
    async fn cancelled_wait_retains_capacity_until_actual_registration_drop() {
        let producer = CatalogueChanges::default();
        let key = target("owner");
        let mut watches = (0..MAX_CATALOGUE_CHANGE_WATCHES)
            .map(|_| producer.watch(key.clone()).unwrap())
            .collect::<Vec<_>>();
        pending(&mut watches[0]);
        assert!(matches!(
            producer.watch(key.clone()),
            Err(CatalogueWatchError::Capacity)
        ));
        producer.publish(&key);
        assert_eq!(ready(&mut watches[0]), CatalogueWatchState::Dirty);
        assert!(matches!(
            producer.watch(key.clone()),
            Err(CatalogueWatchError::Capacity)
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
        let producer = CatalogueChanges::default();
        let key = target("owner");
        let mut watched = producer.watch(key.clone()).unwrap();
        let mut other = producer.watch(target("other")).unwrap();
        pending(&mut watched);
        for _ in 0..100 {
            producer.publish(&key);
        }
        assert_eq!(ready(&mut watched), CatalogueWatchState::Dirty);
        pending(&mut watched);
        pending(&mut other);
        producer.publish(&key);
        assert_eq!(ready(&mut watched), CatalogueWatchState::Dirty);
        pending(&mut watched);
    }

    #[tokio::test]
    async fn final_publisher_drop_closes_waiters_after_the_last_retained_owner() {
        let producer = CatalogueChanges::default();
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
        assert_eq!(ready(&mut watched), CatalogueWatchState::Closed);
        assert_eq!(ready(&mut watched), CatalogueWatchState::Closed);
    }
    #[test]
    fn full_registry_checks_each_receiver_once_and_releases_only_on_drop() {
        let producer = CatalogueChanges::default();
        let key = target("owner");
        let mut watches: Vec<_> = (0..MAX_CATALOGUE_CHANGE_WATCHES - 1)
            .map(|_| producer.watch_for(key.clone(), Some("reader")).unwrap())
            .collect();
        let mut other = producer.watch_for(key.clone(), Some("other")).unwrap();
        let mut reads = 0;
        let publication = producer.capture(&key, |receiver| {
            reads += 1;
            receiver == "reader"
        });
        assert_eq!(reads, 2);
        // Cancel a wait while the physical registration still owns capacity.
        pending(&mut watches[0]);
        assert!(matches!(
            producer.watch(key.clone()),
            Err(CatalogueWatchError::Capacity)
        ));
        publication.publish();
        for watch in &mut watches {
            assert_eq!(ready(watch), CatalogueWatchState::Dirty);
            pending(watch);
        }
        pending(&mut other);
        drop(watches.pop());
        assert!(producer.watch(key).is_ok());
    }
}
