use crate::application::agent_execution::sessions::storage::{
    SessionSnapshot, SessionStorage, SessionStorageLease, StorageError, StorageFuture,
};
use crate::domain::agent_execution::sessions::SessionId;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

#[derive(Default)]
struct Entry {
    leased: bool,
    snapshot: Option<SessionSnapshot>,
}

/// Clones share snapshots and writer leases. Independent instances are isolated.
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
}
struct MemoryStore {
    id: SessionId,
    entries: Arc<Mutex<HashMap<String, Entry>>>,
}
impl SessionStorageLease for MemoryStore {
    fn load(&self) -> StorageFuture<'_, Option<SessionSnapshot>> {
        Box::pin(async move {
            let entries = self
                .entries
                .lock()
                .map_err(|_| StorageError::Io("memory storage lock poisoned".into()))?;
            Ok(entries
                .get(self.id.as_str())
                .and_then(|entry| entry.snapshot.clone()))
        })
    }
    fn save(&self, snapshot: SessionSnapshot) -> StorageFuture<'_, ()> {
        let validation = if snapshot.id != self.id {
            Err(StorageError::IdentityMismatch)
        } else {
            super::snapshot::validate(&snapshot)
        };
        if let Err(error) = validation {
            snapshot.discard_rejected_errors();
            return Box::pin(async move { Err(error) });
        }
        Box::pin(async move {
            let mut entries = self
                .entries
                .lock()
                .map_err(|_| StorageError::Io("memory storage lock poisoned".into()))?;
            entries
                .get_mut(self.id.as_str())
                .expect("leased entry exists")
                .snapshot = Some(snapshot);
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
