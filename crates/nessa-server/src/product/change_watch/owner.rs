use nessa_auth::application::session::AuthenticatedSession;
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::product_contract::generated::ChangeWatchErrorCode;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore};

/// The actual watch resource, separate from source-local and passive-read limits.
/// Charged against the gateway's total and against the principal it serves.
pub(in crate::product) struct WatchOwners {
    semaphore: Arc<Semaphore>,
    capacity: usize,
    principal_capacity: usize,
    // Entries exist only while a principal holds at least one owner, so the map
    // never holds more entries than there are owners.
    principals: Mutex<HashMap<WatchPrincipal, usize>>,
    changed: Notify,
    fault: OnceLock<WatchTaskFault>,
}

/// Who a watch is charged to: the authenticated session's organization and
/// principal, both identities minted when they began.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(in crate::product) struct WatchPrincipal {
    organization: OrganizationId,
    principal: PrincipalId,
}

impl WatchPrincipal {
    pub fn of(session: &AuthenticatedSession) -> Self {
        Self {
            organization: session.context().organization_id().clone(),
            principal: session.context().principal_id().clone(),
        }
    }
}

/// First actual task failure, retained independently of socket interest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchTaskFault {
    /// Original task unwound before its completion guard was marked completed.
    Panic,
    /// Original task future was dropped without ordinary completion.
    UnexpectedCancellation,
}

/// A task-local guard reports before its original resource ownership releases.
pub(in crate::product) struct WatchTaskGuard {
    permit: Arc<ProductWatchPermit>,
    completed: bool,
}
impl WatchTaskGuard {
    pub fn completed(mut self) {
        self.completed = true;
    }
}
impl Drop for WatchTaskGuard {
    fn drop(&mut self) {
        if !self.completed {
            let cause = if std::thread::panicking() {
                WatchTaskFault::Panic
            } else {
                WatchTaskFault::UnexpectedCancellation
            };
            let _ = self.permit.owners.fault.set(cause);
        }
    }
}

/// Non-cloneable original permit retained by a task/frame, not a cancellation flag.
pub(in crate::product) struct ProductWatchPermit {
    permit: Option<OwnedSemaphorePermit>,
    principal: WatchPrincipal,
    owners: Arc<WatchOwners>,
}

impl ProductWatchPermit {
    pub fn task(self: &Arc<Self>) -> WatchTaskGuard {
        WatchTaskGuard {
            permit: self.clone(),
            completed: false,
        }
    }
}

impl Drop for ProductWatchPermit {
    fn drop(&mut self) {
        {
            let mut principals = self
                .owners
                .principals
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if let Some(held) = principals.get_mut(&self.principal) {
                *held -= 1;
                if *held == 0 {
                    principals.remove(&self.principal);
                }
            }
        }
        drop(self.permit.take());
        self.owners.changed.notify_waiters();
    }
}

impl WatchOwners {
    pub fn new(capacity: usize, principal_capacity: usize) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(capacity)),
            capacity,
            principal_capacity,
            principals: Mutex::new(HashMap::new()),
            changed: Notify::new(),
            fault: OnceLock::new(),
        }
    }

    /// Take one owner for `principal`: refused as closed once shutdown began,
    /// and as capacity when the gateway or this principal is at its limit.
    pub fn try_acquire(
        self: &Arc<Self>,
        principal: &WatchPrincipal,
    ) -> Result<ProductWatchPermit, ChangeWatchErrorCode> {
        let mut principals = self
            .principals
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if self.semaphore.is_closed() {
            return Err(ChangeWatchErrorCode::WatchClosed);
        }
        let held = principals.get(principal).copied().unwrap_or(0);
        if held >= self.principal_capacity {
            return Err(ChangeWatchErrorCode::WatchCapacity);
        }
        let permit = self.semaphore.clone().try_acquire_owned().map_err(|_| {
            if self.semaphore.is_closed() {
                ChangeWatchErrorCode::WatchClosed
            } else {
                ChangeWatchErrorCode::WatchCapacity
            }
        })?;
        principals.insert(principal.clone(), held + 1);
        Ok(ProductWatchPermit {
            permit: Some(permit),
            principal: principal.clone(),
            owners: self.clone(),
        })
    }

    pub fn close(&self) {
        self.semaphore.close();
        self.changed.notify_waiters();
    }

    pub async fn closed(&self) {
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.semaphore.is_closed() {
                return;
            }
            notified.await;
        }
    }

    pub async fn drain(&self) -> Result<(), WatchTaskFault> {
        self.close();
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.semaphore.available_permits() == self.capacity {
                return self.fault.get().copied().map_or(Ok(()), Err);
            }
            notified.await;
        }
    }

    #[cfg(test)]
    pub fn is_closed(&self) -> bool {
        self.semaphore.is_closed()
    }

    #[cfg(test)]
    pub fn available_permits(&self) -> usize {
        self.semaphore.available_permits()
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use futures_util::poll;
    use std::task::Poll;
    use tokio::sync::oneshot::Sender;

    pub(in crate::product) fn principal(name: &str) -> WatchPrincipal {
        WatchPrincipal {
            organization: OrganizationId::new("organization").unwrap(),
            principal: PrincipalId::new(name).unwrap(),
        }
    }

    /// Row R4b: one principal cannot take more than its limit, whatever the
    /// gateway has left, and other principals are unaffected.
    #[test]
    fn one_principal_cannot_take_more_than_its_limit_and_others_are_unaffected() {
        let owners = Arc::new(WatchOwners::new(4, 2));
        let first = owners.try_acquire(&principal("a")).unwrap();
        let _second = owners.try_acquire(&principal("a")).unwrap();
        assert_eq!(
            owners.try_acquire(&principal("a")).err(),
            Some(ChangeWatchErrorCode::WatchCapacity)
        );
        assert_eq!(owners.available_permits(), 2);
        let _other = owners.try_acquire(&principal("b")).unwrap();
        drop(first);
        let _again = owners.try_acquire(&principal("a")).unwrap();
        assert_eq!(owners.available_permits(), 1);
        owners.close();
        assert_eq!(
            owners.try_acquire(&principal("c")).err(),
            Some(ChangeWatchErrorCode::WatchClosed)
        );
    }

    #[tokio::test]
    async fn closed_interest_and_cancelled_drain_do_not_release_original_task_owner() {
        let owners = Arc::new(WatchOwners::new(1, 1));
        let permit = owners.try_acquire(&principal("a")).unwrap();
        let (release, held) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            let _original = permit;
            held.await.unwrap();
        });
        let mut drain = Box::pin(owners.drain());
        assert!(matches!(poll!(&mut drain), Poll::Pending));
        assert!(matches!(
            owners.try_acquire(&principal("a")),
            Err(ChangeWatchErrorCode::WatchClosed)
        ));
        assert_eq!(owners.available_permits(), 0);
        // Loss of the drain observer cannot abort the actual admitted task.
        drop(drain);
        assert_eq!(owners.available_permits(), 0);
        release.send(()).unwrap();
        task.await.unwrap();
        owners.drain().await.unwrap();
        assert_eq!(owners.available_permits(), 1);
    }
    #[tokio::test]
    async fn fault_survives_observer_loss_and_is_reported_before_last_permit_release() {
        struct Finished(Option<Sender<()>>);
        impl Drop for Finished {
            fn drop(&mut self) {
                let _ = self.0.take().unwrap().send(());
            }
        }
        let owners = Arc::new(WatchOwners::new(1, 1));
        let original = Arc::new(owners.try_acquire(&principal("a")).unwrap());
        let guard = original.task();
        let (finished, observed) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let _finished = Finished(Some(finished));
            let _original_task = guard;
            panic!("injected watch authority panic");
        });
        drop(task); // Actual worker continues without its JoinHandle observer.
        observed.await.unwrap();
        assert_eq!(owners.fault.get(), Some(&WatchTaskFault::Panic));
        assert_eq!(owners.available_permits(), 0);
        let mut drain = Box::pin(owners.drain());
        assert!(matches!(poll!(&mut drain), Poll::Pending));
        drop(original);
        assert_eq!(drain.await, Err(WatchTaskFault::Panic));
    }

    #[tokio::test]
    async fn unpolled_task_cancellation_is_a_distinct_fault_and_first_cause_wins() {
        let owners = Arc::new(WatchOwners::new(1, 1));
        let owner = Arc::new(owners.try_acquire(&principal("a")).unwrap());
        let guard = owner.task();
        let task = tokio::spawn(async move {
            let _original_task = guard;
            std::future::pending::<()>().await;
        });
        task.abort(); // Test injects an unexpected abort; production never does this.
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(
            owners.fault.get(),
            Some(&WatchTaskFault::UnexpectedCancellation)
        );
        assert_eq!(owners.available_permits(), 0);
        let guard = owner.task();
        let task = tokio::spawn(async move {
            let _original_task = guard;
            panic!("later cause cannot overwrite cancellation");
        });
        assert!(task.await.unwrap_err().is_panic());
        drop(owner);
        assert_eq!(
            owners.drain().await,
            Err(WatchTaskFault::UnexpectedCancellation)
        );
    }

    #[tokio::test]
    async fn completed_task_and_ordinary_interest_closure_do_not_report_a_fault() {
        let owners = Arc::new(WatchOwners::new(1, 1));
        let owner = Arc::new(owners.try_acquire(&principal("a")).unwrap());
        let guard = owner.task();
        let task = tokio::spawn(async move {
            guard.completed();
        });
        task.await.unwrap();
        owners.close();
        drop(owner);
        assert_eq!(owners.drain().await, Ok(()));
    }
}
