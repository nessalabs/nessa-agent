//! Producer-owned registrations; publication never waits for a reader.
//!
//! Interest is indexed by owner, so a commit wakes only that owner's
//! registrations, however many are open, and a registration leaves in
//! constant time. There is no gateway-wide count:
//! each registration belongs to a live list subscription, which its socket's
//! own published limit already bounds.

#![deny(missing_docs)]

use crate::conversation::application::{
    catalogue_watch::{CatalogueChangeSignal, CatalogueWatchRegistration},
    CatalogueChangeWatch,
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError, Weak},
};

type Owner = (OrganizationId, PrincipalId);

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
    next: u64,
    // Each registration's signal under its own id, so one leaves in constant time.
    by_owner: HashMap<Owner, HashMap<u64, Arc<CatalogueChangeSignal>>>,
}
struct Registration {
    registry: Weak<Registry>,
    target: Owner,
    id: u64,
}

impl CatalogueChanges {
    pub fn watch(&self, target: Owner) -> CatalogueChangeWatch {
        let signal = Arc::new(CatalogueChangeSignal::default());
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let id = state.next;
        state.next += 1;
        state
            .by_owner
            .entry(target.clone())
            .or_default()
            .insert(id, signal.clone());
        drop(state);
        let registration = Registration {
            registry: Arc::downgrade(&self.inner),
            target,
            id,
        };
        CatalogueChangeWatch::new(signal, Box::new(registration))
    }
    pub fn publish(&self, target: &Owner) {
        // Only this owner's interest is captured. Waking outside the registry
        // lock permits a waker to retire/register source interest.
        let signals = self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .by_owner
            .get(target)
            .map(|bucket| bucket.values().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        for signal in signals {
            signal.publish();
        }
    }
}
impl CatalogueWatchRegistration for Registration {}

impl Drop for Registration {
    fn drop(&mut self) {
        let Some(registry) = self.registry.upgrade() else {
            return;
        };
        let mut state = registry
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let Some(bucket) = state.by_owner.get_mut(&self.target) else {
            return;
        };
        bucket.remove(&self.id);
        // An emptied owner's entry goes with its last registration.
        if bucket.is_empty() {
            state.by_owner.remove(&self.target);
        }
    }
}
impl Drop for Registry {
    fn drop(&mut self) {
        for signal in self
            .state
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .by_owner
            .values()
            .flat_map(HashMap::values)
        {
            signal.close();
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
        let mut watched = producer.watch(key.clone());
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
    async fn cancelled_wait_keeps_the_registration_until_its_handle_drops() {
        let producer = CatalogueChanges::default();
        let key = target("owner");
        let mut watches = (0..2)
            .map(|_| producer.watch(key.clone()))
            .collect::<Vec<_>>();
        pending(&mut watches[0]);
        producer.publish(&key);
        assert_eq!(ready(&mut watches[0]), CatalogueWatchState::Dirty);
        let signal = Arc::downgrade(&producer.inner.state.lock().unwrap().by_owner[&key][&0]);
        drop(watches.remove(0));
        assert!(
            signal.upgrade().is_none(),
            "retired registration retains no signal owner"
        );
        producer.publish(&key);
        assert_eq!(ready(&mut watches[0]), CatalogueWatchState::Dirty);
    }

    /// #978: no fixed gateway-wide count, and a commit wakes only its owner.
    #[tokio::test]
    async fn registrations_scale_and_a_commit_wakes_only_its_owner() {
        let producer = CatalogueChanges::default();
        let mut watches = (0..10_000)
            .map(|index| producer.watch(target(&format!("owner-{index}"))))
            .collect::<Vec<_>>();
        producer.publish(&target("owner-42"));
        assert_eq!(ready(&mut watches[42]), CatalogueWatchState::Dirty);
        pending(&mut watches[41]);
        pending(&mut watches[43]);
        drop(watches);
        assert!(
            producer.inner.state.lock().unwrap().by_owner.is_empty(),
            "an owner's entry goes with its last registration"
        );
    }

    #[tokio::test]
    async fn commits_coalesce_and_a_commit_after_consumption_leaves_another_notice() {
        let producer = CatalogueChanges::default();
        let key = target("owner");
        let mut watched = producer.watch(key.clone());
        let mut other = producer.watch(target("other"));
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
        let mut watched = producer.watch(target("owner"));
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
}
