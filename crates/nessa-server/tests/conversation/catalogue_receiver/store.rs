//! Durable receiver cache shared by catalogue adapter and product-process tests.
use crate::conversation::domain::ConversationId;
use nessa_local_database::rusqlite::{params, Connection, OptionalExtension};
use nessa_sync::replication::{
    catalogue::{
        catalogue_progress_after_begin, catalogue_progress_after_page,
        catalogue_progress_after_reset, validate_catalogue_progress,
        validate_catalogue_revision_transition, CataloguePagePlan, CataloguePass,
        CatalogueProgress, CatalogueStore, CatalogueStoreError, CatalogueValidationError, EntryKey,
        ManifestEntry,
    },
    domain::{Id, Scope},
};
use std::{fmt::Debug, path::Path};
fn err(_: impl Debug) -> CatalogueStoreError {
    CatalogueStoreError::Failed
}
fn int(value: u64) -> Result<i64, CatalogueStoreError> {
    i64::try_from(value).map_err(err)
}
fn uint(value: i64) -> Result<u64, CatalogueStoreError> {
    u64::try_from(value).map_err(err)
}

pub(crate) struct SqliteReceiver {
    pub(crate) conn: Connection,
}
impl SqliteReceiver {
    pub(crate) fn open(path: &Path) -> Self {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch("CREATE TABLE IF NOT EXISTS progress (
            singleton INTEGER PRIMARY KEY CHECK(singleton=1), receiver TEXT NOT NULL, origin TEXT NOT NULL,
            stream TEXT NOT NULL, incarnation TEXT NOT NULL, schema_id TEXT NOT NULL, epoch TEXT NOT NULL,
            completed INTEGER NOT NULL, generation INTEGER NOT NULL, boundary INTEGER,
            cursor_creation INTEGER, cursor_id TEXT);
            CREATE TABLE IF NOT EXISTS entries (
            id TEXT PRIMARY KEY, creation INTEGER NOT NULL, revision INTEGER NOT NULL,
            deleted INTEGER NOT NULL, payload BLOB NOT NULL);").unwrap();
        Self { conn }
    }
    pub(crate) fn count(&self) -> i64 {
        self.conn
            .query_row("SELECT COUNT(*) FROM entries", [], |row| row.get(0))
            .unwrap()
    }
    pub(crate) fn payload(&self, id: &ConversationId) -> serde_json::Value {
        let bytes: Vec<u8> = self
            .conn
            .query_row(
                "SELECT payload FROM entries WHERE id=?1",
                [id.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }
    pub(crate) fn snapshot(
        conn: &Connection,
    ) -> Result<Option<CatalogueProgress>, CatalogueStoreError> {
        type Row = (
            String,
            String,
            String,
            String,
            String,
            String,
            i64,
            i64,
            Option<i64>,
            Option<i64>,
            Option<String>,
        );
        let row: Option<Row> = conn.query_row("SELECT receiver,origin,stream,incarnation,schema_id,epoch,completed,generation,boundary,cursor_creation,cursor_id FROM progress WHERE singleton=1", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?,r.get(8)?,r.get(9)?,r.get(10)?))).optional().map_err(err)?;
        row.map(
            |(
                receiver,
                origin,
                stream,
                incarnation,
                schema,
                epoch,
                completed,
                generation,
                boundary,
                cursor_creation,
                cursor_id,
            )| {
                let scope = Scope::new(
                    Id::new(&receiver).map_err(err)?,
                    Id::new(&origin).map_err(err)?,
                    Id::new(&stream).map_err(err)?,
                    Id::new(&incarnation).map_err(err)?,
                    Id::new(&schema).map_err(err)?,
                    Id::new(&epoch).map_err(err)?,
                );
                let completed = uint(completed)?;
                let generation = uint(generation)?;
                let cursor = match (cursor_creation, cursor_id) {
                    (Some(creation), Some(id)) => Some(EntryKey {
                        creation: uint(creation)?,
                        id: Id::new(&id).map_err(err)?,
                    }),
                    (None, None) => None,
                    _ => return Err(CatalogueStoreError::Failed),
                };
                if boundary.is_none() && cursor.is_some() {
                    return Err(CatalogueStoreError::Failed);
                }
                let active = boundary
                    .map(|boundary| -> Result<CataloguePass, CatalogueStoreError> {
                        Ok(CataloguePass {
                            scope: scope.clone(),
                            completed,
                            boundary: uint(boundary)?,
                            cursor,
                            generation,
                        })
                    })
                    .transpose()?;
                let progress = CatalogueProgress {
                    scope,
                    completed,
                    generation,
                    active,
                };
                validate_catalogue_progress(&progress).map_err(CatalogueStoreError::from)?;
                Ok(progress)
            },
        )
        .transpose()
    }
}

impl CatalogueStore for SqliteReceiver {
    fn progress(&mut self, _: &Scope) -> Result<Option<CatalogueProgress>, CatalogueStoreError> {
        Self::snapshot(&self.conn)
    }
    fn begin(
        &mut self,
        scope: &Scope,
        expected: Option<CatalogueProgress>,
        boundary: u64,
    ) -> Result<CatalogueProgress, CatalogueStoreError> {
        let tx = self.conn.transaction().map_err(err)?;
        if Self::snapshot(&tx)? != expected {
            return Err(CatalogueStoreError::Stale);
        }
        let progress = catalogue_progress_after_begin(scope, expected.as_ref(), boundary)
            .map_err(CatalogueStoreError::from)?;
        save_progress(&tx, &progress)?;
        tx.commit().map_err(err)?;
        Ok(progress)
    }
    fn cached_revision(
        &mut self,
        scope: &Scope,
        id: &Id,
    ) -> Result<Option<u64>, CatalogueStoreError> {
        if Self::snapshot(&self.conn)?.is_some_and(|p| p.scope != *scope) {
            return Err(CatalogueStoreError::ResetRequired);
        }
        let revision: Option<i64> = self
            .conn
            .query_row(
                "SELECT revision FROM entries WHERE id=?1",
                [id.as_str()],
                |r| r.get(0),
            )
            .optional()
            .map_err(err)?;
        revision.map(uint).transpose()
    }
    fn apply_page(
        &mut self,
        plan: CataloguePagePlan,
    ) -> Result<CatalogueProgress, CatalogueStoreError> {
        let progress = catalogue_progress_after_page(&plan).map_err(CatalogueStoreError::from)?;
        let tx = self.conn.transaction().map_err(err)?;
        if Self::snapshot(&tx)?.and_then(|p| p.active) != Some(plan.pass.clone()) {
            return Err(CatalogueStoreError::Stale);
        }
        for entry in &plan.unchanged {
            let cached = cached_entry(&tx, &entry.key.id)?
                .ok_or(CatalogueStoreError::Stale)?
                .0;
            validate_catalogue_revision_transition(entry, &cached).map_err(revision_error)?;
        }
        for value in &plan.entries {
            if let Some((cached, payload)) = cached_entry(&tx, &value.manifest.key.id)? {
                let (earlier, later) = if cached.revision > value.manifest.revision {
                    (&value.manifest, &cached)
                } else {
                    (&cached, &value.manifest)
                };
                validate_catalogue_revision_transition(earlier, later).map_err(revision_error)?;
                if cached.revision > value.manifest.revision {
                    continue;
                }
                if cached.revision == value.manifest.revision {
                    if payload != value.payload {
                        return Err(CatalogueStoreError::Conflict);
                    }
                    continue;
                }
            }
            tx.execute("INSERT INTO entries VALUES(?1,?2,?3,?4,?5) ON CONFLICT(id) DO UPDATE SET revision=excluded.revision,deleted=excluded.deleted,payload=excluded.payload",
                params![value.manifest.key.id.as_str(),int(value.manifest.key.creation)?,int(value.manifest.revision)?,value.manifest.deleted,&value.payload]).map_err(err)?;
        }
        save_progress(&tx, &progress)?;
        tx.commit().map_err(err)?;
        Ok(progress)
    }
    fn reset(
        &mut self,
        scope: &Scope,
        expected: CatalogueProgress,
    ) -> Result<CatalogueProgress, CatalogueStoreError> {
        let progress =
            catalogue_progress_after_reset(scope, &expected).map_err(CatalogueStoreError::from)?;
        let tx = self.conn.transaction().map_err(err)?;
        if Self::snapshot(&tx)? != Some(expected) {
            return Err(CatalogueStoreError::Stale);
        }
        tx.execute("DELETE FROM entries WHERE deleted=0", [])
            .map_err(err)?;
        save_progress(&tx, &progress)?;
        tx.commit().map_err(err)?;
        Ok(progress)
    }
}

fn save_progress(
    conn: &Connection,
    progress: &CatalogueProgress,
) -> Result<(), CatalogueStoreError> {
    let scope = &progress.scope;
    let boundary = progress
        .active
        .as_ref()
        .map(|pass| int(pass.boundary))
        .transpose()?;
    let cursor = progress
        .active
        .as_ref()
        .and_then(|pass| pass.cursor.as_ref());
    let creation = cursor.map(|key| int(key.creation)).transpose()?;
    conn.execute("INSERT INTO progress VALUES(1,?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
        ON CONFLICT(singleton) DO UPDATE SET receiver=excluded.receiver,origin=excluded.origin,stream=excluded.stream,incarnation=excluded.incarnation,schema_id=excluded.schema_id,epoch=excluded.epoch,completed=excluded.completed,generation=excluded.generation,boundary=excluded.boundary,cursor_creation=excluded.cursor_creation,cursor_id=excluded.cursor_id",
        params![scope.receiver().as_str(),scope.origin().as_str(),scope.stream().as_str(),scope.incarnation().as_str(),scope.schema().as_str(),scope.access_epoch().as_str(),int(progress.completed)?,int(progress.generation)?,boundary,creation,cursor.map(|key|key.id.as_str())]).map_err(err)?;
    Ok(())
}

fn cached_entry(
    conn: &Connection,
    id: &Id,
) -> Result<Option<(ManifestEntry, Vec<u8>)>, CatalogueStoreError> {
    let saved: Option<(i64, i64, bool, Vec<u8>)> = conn
        .query_row(
            "SELECT creation,revision,deleted,payload FROM entries WHERE id=?1",
            [id.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(err)?;
    saved
        .map(|(creation, revision, deleted, payload)| {
            Ok((
                ManifestEntry {
                    key: EntryKey {
                        creation: uint(creation)?,
                        id: id.clone(),
                    },
                    revision: uint(revision)?,
                    deleted,
                },
                payload,
            ))
        })
        .transpose()
}

fn revision_error(error: CatalogueValidationError) -> CatalogueStoreError {
    match error {
        CatalogueValidationError::DeletionFence => CatalogueStoreError::Fenced,
        _ => CatalogueStoreError::Conflict,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nessa_sync::replication::catalogue::{ManifestPage, ManifestRequest, ResolvedEntry};

    fn scope() -> Scope {
        let id = Id::new("scope").unwrap();
        Scope::new(
            id.clone(),
            id.clone(),
            id.clone(),
            id.clone(),
            id.clone(),
            id,
        )
    }

    #[test]
    fn unchanged_older_live_descriptor_keeps_newer_deletion_and_later_resurrection_is_fenced() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = SqliteReceiver::open(&directory.path().join("cache.sqlite3"));
        let scope = scope();
        let progress = store.begin(&scope, None, 3).unwrap();
        let key = EntryKey {
            creation: 1,
            id: Id::new("entry").unwrap(),
        };
        store
            .conn
            .execute(
                "INSERT INTO entries VALUES(?1,1,2,1,X'')",
                [key.id.as_str()],
            )
            .unwrap();
        let older = ManifestEntry {
            key: key.clone(),
            revision: 1,
            deleted: false,
        };
        let plan = CataloguePagePlan::new(
            ManifestPage {
                request: ManifestRequest {
                    pass: progress.active.unwrap(),
                    max_entries: 1,
                },
                entries: vec![older.clone()],
                has_more: false,
            },
            vec![],
            vec![older],
        )
        .unwrap();
        let completed = store.apply_page(plan).unwrap();
        assert_eq!(completed.completed, 3);
        assert!(
            cached_entry(&store.conn, &key.id)
                .unwrap()
                .unwrap()
                .0
                .deleted
        );
        let progress = store.begin(&scope, Some(completed), 4).unwrap();
        let later = ManifestEntry {
            key: key.clone(),
            revision: 4,
            deleted: false,
        };
        let plan = CataloguePagePlan::new(
            ManifestPage {
                request: ManifestRequest {
                    pass: progress.active.clone().unwrap(),
                    max_entries: 1,
                },
                entries: vec![later.clone()],
                has_more: false,
            },
            vec![ResolvedEntry {
                manifest: later,
                payload: vec![1],
            }],
            vec![],
        )
        .unwrap();
        assert_eq!(store.apply_page(plan), Err(CatalogueStoreError::Fenced));
        assert_eq!(store.progress(&scope).unwrap(), Some(progress));
        let saved = cached_entry(&store.conn, &key.id).unwrap().unwrap();
        assert!(saved.0.deleted);
        assert_eq!(saved.0.revision, 2);
        assert!(saved.1.is_empty());
    }

    #[test]
    fn contradictory_public_page_plan_changes_neither_progress_nor_entries() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = SqliteReceiver::open(&directory.path().join("cache.sqlite3"));
        let scope = scope();
        let progress = store.begin(&scope, None, 2).unwrap();
        let mut plan = CataloguePagePlan::new(
            ManifestPage {
                request: ManifestRequest {
                    pass: progress.active.clone().unwrap(),
                    max_entries: 1,
                },
                entries: vec![],
                has_more: false,
            },
            vec![],
            vec![],
        )
        .unwrap();
        plan.next_cursor = Some(EntryKey {
            creation: 1,
            id: Id::new("foreign").unwrap(),
        });
        assert_eq!(store.apply_page(plan), Err(CatalogueStoreError::Conflict));
        assert_eq!(store.progress(&scope).unwrap(), Some(progress));
        assert_eq!(store.count(), 0);
    }

    #[test]
    fn malformed_saved_identity_or_active_generation_is_typed_failed_after_reopen() {
        for statement in [
            "UPDATE progress SET receiver=''",
            "UPDATE progress SET generation=0",
            "UPDATE progress SET boundary=NULL,cursor_creation=1,cursor_id='entry'",
        ] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("cache.sqlite3");
            let mut store = SqliteReceiver::open(&path);
            store.begin(&scope(), None, 2).unwrap();
            store.conn.execute(statement, []).unwrap();
            drop(store);
            let mut reopened = SqliteReceiver::open(&path);
            assert_eq!(
                reopened.progress(&scope()),
                Err(CatalogueStoreError::Failed)
            );
        }
    }
}
