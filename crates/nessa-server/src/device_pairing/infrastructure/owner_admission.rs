//! Admission for owner commands, closed and drained at shutdown.
//!
//! ```text
//! admit --(lease)--> owner command's blocking job --(drop)--> drained
//! close: refuse later commands; drained: wait until every lease is back
//! ```
//! A lease is taken before a command does any store work and moves into its
//! blocking job, so a caller that stops waiting does not release it. This is
//! the one owner of "no owner command is running"; every owner command of
//! `GatewayPairing` is admitted here.
use std::sync::Arc;
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore, TryAcquireError};

/// How many owner commands may hold a lease at once. Owner commands are
/// already bounded by the product socket's request and control capacity; this
/// bound only has to be above that, never a second queue.
const OWNER_COMMAND_CAPACITY: usize = 1024;

/// Why a command was not admitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OwnerAdmissionRefusal {
    /// Shutdown has closed admission.
    Closed,
    /// Every lease is held, for example by commands whose callers left while
    /// their store work is still running.
    Full,
}

/// Owner-command admission for one gateway.
pub(crate) struct OwnerAdmission {
    leases: Arc<Semaphore>,
    drained: Arc<Notify>,
}

/// One admitted owner command. Dropping it returns the lease and wakes a
/// waiting drain.
pub(crate) struct OwnerLease {
    permit: Option<OwnedSemaphorePermit>,
    drained: Arc<Notify>,
}

impl OwnerAdmission {
    pub(crate) fn new() -> Self {
        Self {
            leases: Arc::new(Semaphore::new(OWNER_COMMAND_CAPACITY)),
            drained: Arc::new(Notify::new()),
        }
    }

    /// A lease for one command, or why none is given.
    pub(crate) fn admit(&self) -> Result<OwnerLease, OwnerAdmissionRefusal> {
        let permit = self
            .leases
            .clone()
            .try_acquire_owned()
            .map_err(|error| match error {
                TryAcquireError::Closed => OwnerAdmissionRefusal::Closed,
                TryAcquireError::NoPermits => OwnerAdmissionRefusal::Full,
            })?;
        Ok(OwnerLease {
            permit: Some(permit),
            drained: self.drained.clone(),
        })
    }

    /// Refuse every later command. Commands already admitted keep running.
    pub(crate) fn close(&self) {
        self.leases.close();
    }

    /// Wait until every admitted command has returned its lease. Call after
    /// `close`, or new commands can keep it waiting.
    pub(crate) async fn drained(&self) {
        loop {
            let notified = self.drained.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.leases.available_permits() == OWNER_COMMAND_CAPACITY {
                return;
            }
            notified.await;
        }
    }
}

impl Drop for OwnerLease {
    fn drop(&mut self) {
        drop(self.permit.take());
        self.drained.notify_waiters();
    }
}

#[cfg(test)]
#[path = "../../../tests/device_pairing/infrastructure/owner_admission.rs"]
mod tests;
