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
    use crate::application::agent_execution::{
        providers::ProviderIdentity,
        sessions::{records, ProviderContext, SessionChange},
    };
    use crate::domain::agent_execution::sessions::ExecutionSessionId;

    fn opening(id: &SessionId, name: &str) -> (SessionSaveUnit, SessionSnapshot) {
        let change = SessionChange::Opened {
            id: id.clone(),
            provider: ProviderIdentity::new(name, "model", "workspace").unwrap(),
            context: ProviderContext::Absent,
        };
        let snapshot = records::fold_changes(None, std::slice::from_ref(&change)).unwrap();
        (SessionSaveUnit::new(vec![change]).unwrap(), snapshot)
    }

    fn context(prior: &SessionSnapshot, name: &str) -> (SessionSaveUnit, SessionSnapshot) {
        let change = SessionChange::ProviderContext {
            before: prior.provider_context.clone(),
            after: ProviderContext::Recorded(ExecutionSessionId::new(name).unwrap()),
        };
        let snapshot = records::fold_changes(Some(prior), std::slice::from_ref(&change)).unwrap();
        (SessionSaveUnit::new(vec![change]).unwrap(), snapshot)
    }

    #[tokio::test]
    async fn actual_memory_binding_prefix_receipt_and_reset_keep_original_ownership() {
        let storage = InMemoryStorage::new();
        let id = SessionId::new("memory-owner").unwrap();
        let lease = storage.open(id.clone()).await.unwrap();
        let original = lease.load().await.unwrap().binding().clone();
        let other = InMemoryStorage::new();
        let foreign = other.open(id.clone()).await.unwrap().load().await.unwrap();
        let (first, snapshot) = opening(&id, "provider");
        assert!(matches!(
            lease
                .save_changes(
                    foreign.binding().clone(),
                    snapshot.clone(),
                    vec![first.clone()]
                )
                .await,
            Err(StorageError::IdentityMismatch)
        ));
        assert!(lease.load().await.unwrap().snapshot().is_none());
        let (wrong_session, wrong_snapshot) =
            opening(&SessionId::new("another").unwrap(), "provider");
        assert!(matches!(
            lease
                .save_changes(original.clone(), wrong_snapshot, vec![wrong_session])
                .await,
            Err(StorageError::IdentityMismatch)
        ));
        assert!(lease.load().await.unwrap().snapshot().is_none());
        let receipt = lease
            .save_changes(original.clone(), snapshot.clone(), vec![first.clone()])
            .await
            .unwrap();
        assert_eq!(
            lease
                .save_changes(original.clone(), snapshot.clone(), vec![first.clone()])
                .await
                .unwrap(),
            receipt
        );
        assert_eq!(lease.load().await.unwrap().binding(), receipt.next());
        let (changed_first, changed_snapshot) = opening(&id, "changed-provider");
        assert!(matches!(
            lease
                .save_changes(original.clone(), changed_snapshot, vec![changed_first])
                .await,
            Err(StorageError::Corrupt(_))
        ));
        assert_eq!(lease.load().await.unwrap().snapshot(), Some(&snapshot));
        let (suffix, extended) = context(&snapshot, "extended-context");
        let repartitioned = SessionSaveUnit::new(
            first
                .changes()
                .iter()
                .chain(suffix.changes())
                .cloned()
                .collect(),
        )
        .unwrap();
        assert!(matches!(
            lease
                .save_changes(original.clone(), extended.clone(), vec![repartitioned])
                .await,
            Err(StorageError::Corrupt(_))
        ));
        assert_eq!(lease.load().await.unwrap().binding(), receipt.next());
        let extension = lease
            .save_changes(
                original.clone(),
                extended.clone(),
                vec![first.clone(), suffix],
            )
            .await
            .unwrap();
        assert_eq!(extension.units(), 2);
        assert_eq!(lease.load().await.unwrap().snapshot(), Some(&extended));
        let (later, latest) = context(&extended, "later-context");
        assert!(matches!(
            lease
                .save_changes(receipt.next().clone(), latest.clone(), vec![later.clone()])
                .await,
            Err(StorageError::IdentityMismatch)
        ));
        assert_eq!(lease.load().await.unwrap().snapshot(), Some(&extended));
        let next_receipt = lease
            .save_changes(
                extension.next().clone(),
                latest.clone(),
                vec![later.clone()],
            )
            .await
            .unwrap();
        assert_eq!(
            lease
                .save_changes(extension.next().clone(), latest, vec![later])
                .await
                .unwrap(),
            next_receipt
        );
        lease.erase().await.unwrap();
        let reset = lease.load().await.unwrap();
        assert!(reset.snapshot().is_none());
        assert_ne!(reset.binding().backend(), original.backend());
        {
            // Observe cache release after the real public erase, never forge
            // a private entry or pretend its absence proves physical I/O.
            let entries = storage.entries.lock().unwrap();
            let entry = entries.get(id.as_str()).unwrap();
            assert!(entry.receipt.is_none());
            assert!(entry.original.is_none());
            assert!(entry.units.is_empty());
        }
        let result = lease
            .save_changes(original, snapshot.clone(), vec![first.clone()])
            .await;
        assert!(
            lease.load().await.unwrap().snapshot().is_none(),
            "old retry cannot replace reset state"
        );
        assert!(matches!(result, Err(StorageError::IdentityMismatch)));
        lease
            .save_changes(reset.binding().clone(), snapshot.clone(), vec![first])
            .await
            .unwrap();
        assert_eq!(lease.load().await.unwrap().snapshot(), Some(&snapshot));
    }

    #[tokio::test]
    async fn actual_memory_plan_preflight_refuses_before_replacing_published_evidence() {
        let storage = InMemoryStorage::new();
        let id = SessionId::new("memory-preflight").unwrap();
        let lease = storage.open(id.clone()).await.unwrap();
        let original = lease.load().await.unwrap().binding().clone();
        let (first, snapshot) = opening(&id, "provider");
        let (duplicate, _) = opening(&id, "provider");
        let result = lease
            .save_changes(
                original.clone(),
                snapshot.clone(),
                vec![first.clone(), duplicate],
            )
            .await;
        assert!(
            lease.load().await.unwrap().snapshot().is_none(),
            "invalid later unit cannot publish valid prefix"
        );
        assert!(matches!(result, Err(StorageError::Corrupt(_))));
        let (_, changed_candidate) = opening(&id, "changed-provider");
        let result = lease
            .save_changes(original.clone(), changed_candidate, vec![first.clone()])
            .await;
        assert!(
            lease.load().await.unwrap().snapshot().is_none(),
            "contradictory candidate cannot publish"
        );
        assert!(matches!(result, Err(StorageError::Corrupt(_))));
        let receipt = lease
            .save_changes(original, snapshot.clone(), vec![first])
            .await
            .unwrap();
        let result = lease
            .save_changes(receipt.next().clone(), snapshot.clone(), Vec::new())
            .await;
        assert_eq!(
            lease.load().await.unwrap().binding(),
            receipt.next(),
            "empty plan cannot advance revision before a refused receipt"
        );
        assert!(matches!(result, Err(StorageError::Corrupt(_))));
        let (suffix, extended) = context(&snapshot, "valid-context");
        lease
            .save_changes(receipt.next().clone(), extended.clone(), vec![suffix])
            .await
            .unwrap();
        assert_eq!(lease.load().await.unwrap().snapshot(), Some(&extended));
    }
}
