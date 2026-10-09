use crate::application::agent_execution::sessions::{
    CommittedCompleteness, CommittedFreshness, CommittedSession, CommittedStatus, SessionLoad,
    SessionLoadState, SessionSaveBackend, SessionSaveGeneration, SessionSaveReceipt,
    SessionSaveUnit, SessionSnapshot, SessionStorage, SessionStorageLease, StorageError,
    StorageFuture,
};
use crate::domain::agent_execution::sessions::SessionId;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use uuid::Uuid;

struct Entry {
    leased: bool,
    snapshot: Option<SessionSnapshot>,
    revision: u64,
    incarnation: [u8; 16],
    receipt: Option<SessionSaveReceipt>,
    original: Option<SessionSnapshot>,
    units: Vec<SessionSaveUnit>,
}
impl Default for Entry {
    fn default() -> Self {
        Self {
            leased: false,
            snapshot: None,
            revision: 0,
            incarnation: *Uuid::new_v4().as_bytes(),
            receipt: None,
            original: None,
            units: Vec::new(),
        }
    }
}
impl Entry {
    fn binding(&self) -> SessionSaveGeneration {
        SessionSaveGeneration::new(
            SessionSaveBackend::Snapshot {
                incarnation: self.incarnation,
            },
            self.revision,
            self.revision,
        )
    }
}

/// Clones share snapshots and writer leases. Independent instances are isolated.
/// Erasing a session clears its snapshot and keeps its lease entry, so the
/// session stays excluded until the erasing lease is dropped.
/// [`SessionStorage::open_existing`] opens only a session with an entry, and
/// adds none.
#[derive(Clone, Default)]
pub struct InMemoryStorage {
    entries: Arc<Mutex<HashMap<String, Entry>>>,
}
impl InMemoryStorage {
    /// Creates an empty backend; clones share snapshots and exclusive leases.
    pub fn new() -> Self {
        Self::default()
    }
}
impl SessionStorage for InMemoryStorage {
    fn read_committed(&self, id: SessionId) -> StorageFuture<'_, Option<CommittedSession>> {
        Box::pin(async move {
            let entries = self
                .entries
                .lock()
                .map_err(|_| StorageError::Io("memory storage lock poisoned".into()))?;
            entries
                .get(id.as_str())
                .map(|entry| {
                    CommittedSession::new(
                        id.clone(),
                        Uuid::from_bytes(entry.incarnation).simple().to_string(),
                        entry.revision,
                        entry.revision,
                        entry.revision,
                        entry.snapshot.clone(),
                        CommittedStatus::new(
                            if entry.snapshot.is_some() {
                                CommittedCompleteness::Complete
                            } else {
                                CommittedCompleteness::CompleteEmpty
                            },
                            CommittedFreshness::Current,
                        ),
                    )
                })
                .transpose()
        })
    }
    fn open(&self, id: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
        Box::pin(async move {
            let mut entries = self
                .entries
                .lock()
                .map_err(|_| StorageError::Io("memory storage lock poisoned".into()))?;
            let entry = entries.entry(id.as_str().to_owned()).or_default();
            if entry.leased {
                return Err(StorageError::Busy);
            }
            entry.leased = true;
            Ok(Box::new(MemoryStore {
                id,
                entries: self.entries.clone(),
            }) as Box<dyn SessionStorageLease>)
        })
    }
    fn open_existing(
        &self,
        id: SessionId,
    ) -> StorageFuture<'_, Option<Box<dyn SessionStorageLease>>> {
        Box::pin(async move {
            let exists = self
                .entries
                .lock()
                .map_err(|_| StorageError::Io("memory storage lock poisoned".into()))?
                .contains_key(id.as_str());
            if !exists {
                return Ok(None);
            }
            self.open(id).await.map(Some)
        })
    }
}
struct MemoryStore {
    id: SessionId,
    entries: Arc<Mutex<HashMap<String, Entry>>>,
}
impl SessionStorageLease for MemoryStore {
    fn load(&self) -> StorageFuture<'_, SessionLoad> {
        Box::pin(async move {
            let entries = self
                .entries
                .lock()
                .map_err(|_| StorageError::Io("memory storage lock poisoned".into()))?;
            let entry = entries.get(self.id.as_str()).expect("leased entry exists");
            Ok(SessionLoad::new(
                entry.snapshot.clone(),
                entry.binding(),
                SessionLoadState::Published,
            ))
        })
    }
    fn save_changes(
        &self,
        binding: SessionSaveGeneration,
        snapshot: SessionSnapshot,
        units: Vec<SessionSaveUnit>,
    ) -> StorageFuture<'_, SessionSaveReceipt> {
        Box::pin(async move {
            let mut entries = self
                .entries
                .lock()
                .map_err(|_| StorageError::Io("memory storage lock poisoned".into()))?;
            let entry = entries
                .get_mut(self.id.as_str())
                .expect("leased entry exists");
            let retry = entry
                .receipt
                .as_ref()
                .is_some_and(|receipt| receipt.binding() == &binding);
            if !retry && binding != entry.binding() {
                return Err(StorageError::IdentityMismatch);
            }
            if snapshot.id != self.id {
                return Err(StorageError::IdentityMismatch);
            }
            let base = if retry {
                entry.original.as_ref()
            } else {
                entry.snapshot.as_ref()
            };
            SessionSaveUnit::validate_plan(base, &units, &snapshot)?;
            let mut hash = Sha256::new();
            for unit in &units {
                let bytes = super::snapshot::encode_semantic_batch(unit.changes())?;
                super::save_group::validate_unit_payload(&bytes)?;
                hash.update((bytes.len() as u64).to_be_bytes());
                hash.update(&bytes);
            }
            if retry && !units.starts_with(&entry.units) {
                return Err(StorageError::Corrupt("snapshot save prefix changed".into()));
            }
            let digest = hash.finalize().into();
            if retry && units == entry.units {
                return Ok(entry.receipt.clone().expect("matched receipt"));
            }
            let revision = entry
                .revision
                .checked_add(1)
                .ok_or_else(|| StorageError::Corrupt("memory revision exhausted".into()))?;
            if !retry {
                entry.original = entry.snapshot.clone();
            }
            entry.snapshot = Some(snapshot);
            entry.revision = revision;
            let receipt =
                SessionSaveReceipt::new(binding, entry.binding(), units.len() as u64, digest)?;
            entry.units = units;
            entry.receipt = Some(receipt.clone());
            Ok(receipt)
        })
    }
    fn erase(&self) -> StorageFuture<'_, ()> {
        Box::pin(async move {
            let mut entries = self
                .entries
                .lock()
                .map_err(|_| StorageError::Io("memory storage lock poisoned".into()))?;
            // The entry stays: it carries this lease's exclusion.
            if let Some(entry) = entries.get_mut(self.id.as_str()) {
                let next_revision = entry
                    .revision
                    .checked_add(1)
                    .ok_or_else(|| StorageError::Corrupt("memory revision exhausted".into()))?;
                entry.snapshot = None;
                entry.revision = next_revision;
                entry.incarnation = *Uuid::new_v4().as_bytes();
                entry.receipt = None;
                entry.original = None;
                entry.units.clear();
            }
            Ok(())
        })
    }
}
impl Drop for MemoryStore {
    fn drop(&mut self) {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(entry) = entries.get_mut(self.id.as_str()) {
            entry.leased = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn memory_storage_refuses_to_discard_unreadable_history() {
        let storage = InMemoryStorage::new();
        let error = storage
            .discard_unreadable(SessionId::new("unused").unwrap())
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            StorageError::Corrupt(detail) if detail == "this storage does not discard unreadable history"
        ));
    }
}
