//! Read-only carrier of the domain owner's last issued weak handle.
#![deny(missing_docs)]
use crate::domain::agent_execution::permissions::{PermissionAuthority, PermissionAuthorityError};
use std::sync::{Arc, Mutex};

/// A carrier for a domain-issued weak handle, with no review or lifecycle state.
/// Only successful controller admission publishes a handle. A stale carrier
/// confers nothing after its domain collection is emptied or dropped.
#[derive(Clone, Debug, Default)]
pub struct PermissionAuthoritySource(Arc<Mutex<Option<PermissionAuthority>>>);
impl PermissionAuthoritySource {
    pub(super) fn allocation_bytes(&self) -> usize {
        std::mem::size_of::<Mutex<Option<PermissionAuthority>>>()
            .saturating_add(2 * std::mem::size_of::<usize>())
            .saturating_add(self.0.lock().map_or(usize::MAX, |value| {
                value
                    .as_ref()
                    .map_or(0, PermissionAuthority::allocation_bytes)
            }))
    }

    /// Acquire the last issued handle without I/O, audit or command-queue waits.
    /// The caller must validate its exact session/execution before querying it.
    ///
    /// # Errors
    /// Reports unexpected carrier poison as a typed fault.
    pub fn read(&self) -> Result<Option<PermissionAuthority>, PermissionAuthorityError> {
        self.0
            .lock()
            .map(|value| value.clone())
            .map_err(|_| PermissionAuthorityError::Poisoned)
    }
    pub(super) fn publish(&self, authority: PermissionAuthority) {
        *self.0.lock().expect("permission authority carrier") = Some(authority);
    }
}
