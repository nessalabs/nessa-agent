//! Immutable save input and backend-issued recovery evidence.
#![deny(missing_docs)]

use super::{SessionChange, SessionSnapshot, StorageError};
use crate::application::agent_execution::sessions::records::continuation::Continuation;
use crate::domain::agent_execution::sessions::SessionId;
use nessa_sync::replication::domain::Id;

/// The capability supplied by a storage adapter, without invented record cursors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionSaveBackend {
    /// A canonical record stream and its actual reset incarnation.
    Record {
        /// Adapter-owned stream identity.
        stream: Id,
        /// Actual event-store incarnation, invalidated by Reset.
        incarnation: [u8; 16],
    },
    /// A snapshot adapter's own incarnation; positions are snapshot revisions.
    Snapshot {
        /// Minted by the snapshot owner, replaced on erasure.
        incarnation: [u8; 16],
    },
}

#[cfg(test)]
#[path = "../../../../../tests/application/agent_execution/sessions/storage/save.rs"]
mod tests;

/// Identity of a save supplied by the backend's load or acknowledged save.
///
/// This value is correlation evidence, not authority. The lease compares it
/// against its actual incarnation, base and generation before persistence.
/// Retain the original value on cancellation or a lost reply; equal changes
/// do not establish identity. Construction is for storage adapters, not callers
/// guessing a first or next generation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionSaveGeneration {
    backend: SessionSaveBackend,
    base: u64,
    generation: u64,
}
impl SessionSaveGeneration {
    /// Construct a binding from the adapter's actual current state.
    /// `base` is its published record position or snapshot revision;
    /// `generation` is its durable save ordinal (snapshot adapters use revisions).
    pub fn new(backend: SessionSaveBackend, base: u64, generation: u64) -> Self {
        Self {
            backend,
            base,
            generation,
        }
    }
    /// Backend identity that must agree with the receiving lease.
    pub fn backend(&self) -> &SessionSaveBackend {
        &self.backend
    }
    /// Published base at which this save began.
    pub fn base(&self) -> u64 {
        self.base
    }
    /// Original save ordinal within the backend incarnation.
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

/// One caller-selected indivisible semantic checkpoint.
///
/// Structural construction rejects an empty unit. The canonical session fold
/// validates its complete lifecycle/queue checkpoint at the storage boundary;
/// this type does not authorize provider effects or choose arbitrary byte cuts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionSaveUnit {
    changes: Box<[SessionChange]>,
}
impl SessionSaveUnit {
    /// Validate all unit checkpoints and the final candidate before persistence.
    /// `prior` must be the adapter's actual original published base, including
    /// on an extended retry. One canonical continuation validates the sequence;
    /// it does not clone the growing history at every unit boundary.
    ///
    /// This performs no I/O, mints no binding and authorizes no provider effect.
    /// Custom snapshot adapters use this same owner rather than reimplementing
    /// lifecycle or queue validation.
    ///
    /// # Errors
    /// Refuses an empty plan, an invalid intermediate unit or a final candidate
    /// that disagrees with the exact ordered decisions.
    pub fn validate_plan(
        prior: Option<&SessionSnapshot>,
        units: &[Self],
        observed: &SessionSnapshot,
    ) -> Result<(), StorageError> {
        if units.is_empty() {
            return Err(StorageError::Corrupt("semantic save plan is empty".into()));
        }
        let mut continuation = Continuation::restore(prior.cloned())?;
        for unit in units {
            continuation.apply_unit(unit.changes())?;
        }
        if continuation.snapshot.as_ref() != Some(observed) {
            return Err(StorageError::Corrupt(
                "semantic decisions disagree with observed session".into(),
            ));
        }
        Ok(())
    }
    /// Retain an explicit nonempty decision boundary in supplied order.
    ///
    /// # Errors
    /// Returns corruption for an empty boundary. Semantic and encoded-size
    /// validation belongs to the lease before any pending reconciliation.
    pub fn new(changes: Vec<SessionChange>) -> Result<Self, StorageError> {
        Self::check_changes(&changes)?;
        Ok(Self {
            changes: changes.into_boxed_slice(),
        })
    }
    /// Shared representation admission for a nonempty individual unit.
    pub(crate) fn check_changes(changes: &[SessionChange]) -> Result<(), StorageError> {
        if changes.is_empty() {
            return Err(StorageError::Corrupt("semantic save unit is empty".into()));
        }
        Ok(())
    }
    /// Exact ordered decisions; no mutable access to a retained retry boundary.
    pub fn changes(&self) -> &[SessionChange] {
        &self.changes
    }
}

/// Whether load supplies usable published state or unfinished recovery evidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionLoadState {
    /// Snapshot and binding describe a published save (or an empty backend).
    Published,
    /// Only the prior published snapshot is exposed; original units need retry.
    Unfinished,
}

/// A lease read with its actual backend-owned save binding.
///
/// Unfinished state is not permission to restore a queue or initialize a
/// provider. The original caller must supply its full exact units to recover it.
#[derive(Clone, Debug)]
pub struct SessionLoad {
    snapshot: Option<SessionSnapshot>,
    binding: SessionSaveGeneration,
    state: SessionLoadState,
}
impl SessionLoad {
    /// Construct backend evidence. The caller still checks session/provider
    /// identity through the existing canonical saved-snapshot validation.
    pub fn new(
        snapshot: Option<SessionSnapshot>,
        binding: SessionSaveGeneration,
        state: SessionLoadState,
    ) -> Self {
        Self {
            snapshot,
            binding,
            state,
        }
    }
    /// Prior published history; unfinished unit state is excluded.
    pub fn snapshot(&self) -> Option<&SessionSnapshot> {
        self.snapshot.as_ref()
    }
    /// New-work binding if published; original recovery binding if unfinished.
    pub fn binding(&self) -> &SessionSaveGeneration {
        &self.binding
    }
    /// Whether ordinary restoration is permitted by this storage evidence.
    pub fn state(&self) -> SessionLoadState {
        self.state
    }
    /// Consume published evidence before restoration or provider initialization.
    ///
    /// # Errors
    /// Returns `Unresolved` for an unfinished save, without implying absence;
    /// identity mismatch for another session and corruption for invalid history.
    /// `id` is the identity for which this lease was acquired.
    pub fn into_published(
        self,
        id: &SessionId,
    ) -> Result<(Option<SessionSnapshot>, SessionSaveGeneration), StorageError> {
        if self.state == SessionLoadState::Unfinished {
            if let Some(snapshot) = self.snapshot {
                snapshot.discard_rejected_errors();
            }
            return Err(StorageError::Unresolved);
        }
        if matches!(self.binding.backend(), SessionSaveBackend::Record { stream, .. } if stream.as_str() != id.as_str())
        {
            if let Some(snapshot) = self.snapshot {
                snapshot.discard_rejected_errors();
            }
            return Err(StorageError::IdentityMismatch);
        }
        if matches!(self.binding.backend(), SessionSaveBackend::Record { .. })
            && ((self.snapshot.is_none() != (self.binding.base() == 0))
                || ((self.binding.base() == 0) != (self.binding.generation() == 0)))
        {
            if let Some(snapshot) = self.snapshot {
                snapshot.discard_rejected_errors();
            }
            return Err(StorageError::Corrupt(
                "published record binding contradicts its snapshot".into(),
            ));
        }
        if let Some(snapshot) = &self.snapshot {
            if let Err(error) = snapshot.check_saved(id) {
                self.snapshot
                    .expect("checked saved evidence")
                    .discard_rejected_errors();
                return Err(error);
            }
        }
        Ok((self.snapshot, self.binding))
    }
}

/// Acknowledgment of the exact completed plan, with the next actual binding.
///
/// Backend construction is not evidence by itself: the adapter returns this
/// only after its persistence contract confirms the completion terminal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionSaveReceipt {
    binding: SessionSaveGeneration,
    next: SessionSaveGeneration,
    units: u64,
    digest: [u8; 32],
}
impl SessionSaveReceipt {
    /// Construct from confirmed persistence, including exact ordered-unit digest.
    /// Snapshot adapters acknowledge their complete snapshot capability instead
    /// of pretending their revision is a physical event cursor.
    ///
    /// # Errors
    /// Refuses zero units, a foreign incarnation, or an impossible next binding.
    pub fn new(
        binding: SessionSaveGeneration,
        next: SessionSaveGeneration,
        units: u64,
        digest: [u8; 32],
    ) -> Result<Self, StorageError> {
        let valid_next = match binding.backend() {
            SessionSaveBackend::Record { .. } => {
                binding.generation().checked_add(1) == Some(next.generation())
            }
            SessionSaveBackend::Snapshot { .. } => {
                binding.base() == binding.generation() && next.base() == next.generation()
            }
        };
        if units == 0
            || binding.backend() != next.backend()
            || next.base() <= binding.base()
            || !valid_next
        {
            return Err(StorageError::Corrupt(
                "save receipt contradicts its backend binding".into(),
            ));
        }
        Ok(Self {
            binding,
            next,
            units,
            digest,
        })
    }
    /// Obtain next work only for the exact binding and complete unit count acknowledged.
    ///
    /// # Errors
    /// Returns identity mismatch for another attempt's receipt. This check does
    /// not prove persistence; the adapter still owns actual confirmation.
    pub fn next_for(
        &self,
        original: &SessionSaveGeneration,
        units: usize,
    ) -> Result<SessionSaveGeneration, StorageError> {
        if &self.binding != original || u64::try_from(units).ok() != Some(self.units) {
            return Err(StorageError::IdentityMismatch);
        }
        Ok(self.next.clone())
    }
    /// Original binding acknowledged, which does not acknowledge an extension.
    pub fn binding(&self) -> &SessionSaveGeneration {
        &self.binding
    }
    /// Actual new-work binding returned by the persistence owner.
    pub fn next(&self) -> &SessionSaveGeneration {
        &self.next
    }
    /// Number of explicit units in this completed plan.
    pub fn units(&self) -> u64 {
        self.units
    }
    /// Digest of the exact complete ordered unit plan, including boundaries.
    pub fn digest(&self) -> &[u8; 32] {
        &self.digest
    }
}
