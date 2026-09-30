//! Read-only access to the live aggregate's existing pending review collection.
#![deny(missing_docs)]
use super::{PermissionId, PermissionRequest};
use crate::domain::agent_execution::{executions::ExecutionId, sessions::ExecutionSessionId};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
};

pub(crate) type PendingPermissions = Arc<Mutex<HashMap<PermissionId, PermissionRequest>>>;

/// Failure to query a live review owner. These faults do not change durable history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionAuthorityError {
    /// The owning execution has ended or its aggregate was dropped.
    Unavailable,
    /// An unexpected panic poisoned the owning collection.
    Poisoned,
    /// A backend supplied a handle for another session or execution.
    IdentityMismatch,
}
/// A weak read handle issued by one live session aggregate for one execution.
/// It retains identities, never pending requests or authority after owner drop.
#[derive(Clone, Debug)]
pub struct PermissionAuthority {
    session: ExecutionSessionId,
    execution: ExecutionId,
    pending: Weak<Mutex<HashMap<PermissionId, PermissionRequest>>>,
}
impl PermissionAuthority {
    pub(crate) fn allocation_bytes(&self) -> usize {
        self.session
            .as_str()
            .len()
            .saturating_add(self.execution.as_str().len())
    }

    pub(crate) fn new(
        session: ExecutionSessionId,
        execution: ExecutionId,
        pending: &PendingPermissions,
    ) -> Self {
        Self {
            session,
            execution,
            pending: Arc::downgrade(pending),
        }
    }
    /// Provider context that issued this handle.
    pub fn session_id(&self) -> &ExecutionSessionId {
        &self.session
    }
    /// Exact execution whose pending collection this handle reads.
    pub fn execution_id(&self) -> &ExecutionId {
        &self.execution
    }
    /// Whether this review remains in the owning domain collection now.
    /// This does not reserve an answer; the command must still validate its owner.
    ///
    /// # Errors
    /// Reports dropped ownership or a poisoned collection without awaiting I/O.
    pub fn pending(&self, id: &PermissionId) -> Result<bool, PermissionAuthorityError> {
        let pending = self
            .pending
            .upgrade()
            .ok_or(PermissionAuthorityError::Unavailable)?;
        let pending = pending
            .lock()
            .map_err(|_| PermissionAuthorityError::Poisoned)?;
        Ok(pending.contains_key(id))
    }
}

#[cfg(test)]
#[path = "../../../../tests/domain/agent_execution/permissions/authority.rs"]
mod tests;
