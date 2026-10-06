//! In-memory ownership rows for tests and for a process that has not opened SQLite.
#![deny(missing_docs)]

use std::sync::Mutex;

use async_trait::async_trait;

use super::ports::{OwnershipStore, PortFailure};
use crate::domain::agent_execution::subagents::OwnershipSnapshot;

/// Stores one snapshot in the process. A failure flag lets tests refuse or lose a write.
#[derive(Debug)]
pub struct MemoryOwnershipStore {
    snapshot: Mutex<OwnershipSnapshot>,
    failure: Mutex<Option<PortFailure>>,
}

impl MemoryOwnershipStore {
    /// An empty store that accepts writes.
    pub fn new() -> Self {
        Self {
            snapshot: Mutex::new(OwnershipSnapshot::default()),
            failure: Mutex::new(None),
        }
    }

    /// The next write returns `failure` instead of retaining the snapshot.
    /// [`PortFailure::Uncertain`] leaves the previous snapshot in place.
    pub fn fail_next_write(&self, failure: PortFailure) {
        *self.failure.lock().expect("ownership store") = Some(failure);
    }
}

impl Default for MemoryOwnershipStore {
    fn default() -> Self {
        Self::new()
    }
}

/// A fixed number of live child slots. `try_reserve` fails at zero until `release`.
#[derive(Debug)]
pub struct LiveCapacity {
    remaining: Mutex<usize>,
}

impl LiveCapacity {
    /// `slots` is how many children may be reserved at once.
    pub fn new(slots: usize) -> Self {
        Self {
            remaining: Mutex::new(slots),
        }
    }
}

impl super::ports::LiveRoom for LiveCapacity {
    fn try_reserve(&self) -> bool {
        let mut remaining = self.remaining.lock().expect("live capacity");
        if *remaining == 0 {
            return false;
        }
        *remaining -= 1;
        true
    }

    fn release(&self) {
        *self.remaining.lock().expect("live capacity") += 1;
    }
}

#[async_trait]
impl OwnershipStore for MemoryOwnershipStore {
    async fn write(&self, snapshot: &OwnershipSnapshot) -> Result<(), PortFailure> {
        if let Some(failure) = self.failure.lock().expect("ownership store").take() {
            return Err(failure);
        }
        *self.snapshot.lock().expect("ownership store") = snapshot.clone();
        Ok(())
    }

    async fn read(&self) -> Result<OwnershipSnapshot, PortFailure> {
        Ok(self.snapshot.lock().expect("ownership store").clone())
    }
}
