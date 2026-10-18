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
    /// A conversation and the snapshot adapter's reset incarnation; positions
    /// are snapshot revisions. The session remains part of every save receipt.
    /// Omitting it is a compile error:
    /// ```compile_fail
    /// use nessa_sdk::application::agent_execution::sessions::SessionSaveBackend;
    /// let _ = SessionSaveBackend::Snapshot { incarnation: [0; 16] };
    /// ```
    Snapshot {
        /// Conversation whose snapshot and save attempts this binding names.
        session: SessionId,
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
/// Snapshot construction accepts one revision, never two counters:
/// ```compile_fail
/// use nessa_sdk::application::agent_execution::sessions::SessionSaveGeneration;
/// use nessa_sdk::domain::agent_execution::sessions::SessionId;
/// let _ = SessionSaveGeneration::snapshot(SessionId::new("chat").unwrap(), [0;16], 4, 3);
/// ```
/// The former generic constructor cannot bypass backend-specific construction:
/// ```compile_fail
/// use nessa_sdk::application::agent_execution::sessions::{SessionSaveBackend, SessionSaveGeneration};
/// use nessa_sdk::domain::agent_execution::sessions::SessionId;
/// let backend = SessionSaveBackend::Snapshot { session: SessionId::new("chat").unwrap(), incarnation: [0;16] };
/// let _ = SessionSaveGeneration::new(backend, 4, 3);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionSaveGeneration {
    backend: SessionSaveBackend,
    counters: SaveCounters,
}
#[derive(Clone, Debug, PartialEq, Eq)]
enum SaveCounters {
    Record { base: u64, generation: u64 },
    Snapshot { revision: u64 },
}
impl SessionSaveGeneration {
    /// Bind an actual record stream's published physical `base` and durable save
    /// ordinal `generation`. These are distinct facts and need not be equal.
    /// `stream` and `incarnation` identify the adapter's actual stream/reset.
    /// This performs no I/O and does not confirm persistence or authorize work.
    /// ```
    /// use nessa_sdk::application::agent_execution::sessions::SessionSaveGeneration;
    /// use nessa_sync::replication::domain::Id;
    /// let binding = SessionSaveGeneration::record(Id::new("chat").unwrap(), [1;16], 8, 3);
    /// assert_eq!((binding.base(), binding.generation()), (8, 3));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn record(stream: Id, incarnation: [u8; 16], base: u64, generation: u64) -> Self {
        Self {
            backend: SessionSaveBackend::Record {
                stream,
                incarnation,
            },
            counters: SaveCounters::Record { base, generation },
        }
    }
    /// Bind `session` and the adapter's actual reset `incarnation` to its one
    /// snapshot `revision`. Both base and generation derive from that value.
    /// This performs no I/O; the lease still checks its current binding before
    /// accepting work. Retaining or cloning this value reserves no resources.
    /// ```
    /// use nessa_sdk::application::agent_execution::sessions::SessionSaveGeneration;
    /// use nessa_sdk::domain::agent_execution::sessions::SessionId;
    /// let binding = SessionSaveGeneration::snapshot(SessionId::new("chat")?, [1;16], 4);
    /// assert_eq!(binding.base(), binding.generation());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn snapshot(session: SessionId, incarnation: [u8; 16], revision: u64) -> Self {
        Self {
            backend: SessionSaveBackend::Snapshot {
                session,
                incarnation,
            },
            counters: SaveCounters::Snapshot { revision },
        }
    }
    /// Backend identity that must agree with the receiving lease.
    pub fn backend(&self) -> &SessionSaveBackend {
        &self.backend
    }
    /// Published record position or the snapshot's single revision.
    pub fn base(&self) -> u64 {
        match self.counters {
            SaveCounters::Record { base, .. } => base,
            SaveCounters::Snapshot { revision } => revision,
        }
    }
    /// Durable record save ordinal or the snapshot's single revision.
    pub fn generation(&self) -> u64 {
        match self.counters {
            SaveCounters::Record { generation, .. } => generation,
            SaveCounters::Snapshot { revision } => revision,
        }
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
    /// The snapshot that results from applying `units`, in order, to `prior`.
    ///
    /// An adapter that stores whole snapshots instead of records publishes
    /// this. `prior` must be the adapter's actual original published base,
    /// including on an extended retry. Every unit is checked by the same rules
    /// a record writer applies, so a snapshot adapter does not reimplement
    /// lifecycle or queue validation.
    ///
    /// This performs no I/O, mints no binding and authorizes no provider effect.
    ///
    /// # Errors
    /// Refuses an empty plan and a unit that does not apply.
    pub fn fold(
        prior: Option<SessionSnapshot>,
        units: &[Self],
    ) -> Result<SessionSnapshot, StorageError> {
        if units.is_empty() {
            return Err(StorageError::Corrupt("semantic save plan is empty".into()));
        }
        let mut continuation = Continuation::restore(prior)?;
        for unit in units {
            continuation.apply_unit(unit.changes())?;
        }
        continuation
            .snapshot
            .ok_or_else(|| StorageError::Corrupt("semantic save plan opened nothing".into()))
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
/// provider. The original caller must supply its full exact units to recover it;
/// [`Agent::prepare`](crate::application::agent_execution::agents::Agent::prepare)
/// is that caller for its own initialization save, which it derives again from
/// the prior publication.
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
        self.into_checked(id)
    }
    /// The same identity, binding and history checks as [`Self::into_published`]
    /// without its unfinished refusal. For an unfinished load this returns the
    /// prior publication and the original recovery binding; only an exact retry
    /// of the original plan may use them.
    pub(crate) fn into_checked(
        self,
        id: &SessionId,
    ) -> Result<(Option<SessionSnapshot>, SessionSaveGeneration), StorageError> {
        let matches_session = match self.binding.backend() {
            SessionSaveBackend::Record { stream, .. } => stream.as_str() == id.as_str(),
            SessionSaveBackend::Snapshot { session, .. } => session == id,
        };
        if !matches_session {
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
                "record binding contradicts its snapshot".into(),
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
            // Snapshot counters derive from its single stored revision.
            SessionSaveBackend::Snapshot { .. } => true,
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
