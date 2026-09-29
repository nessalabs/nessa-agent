//! Two independent process-local receiver files exercise the real sync core.
//! This fixture uses Nessa's SQLite version, avoiding a second rusqlite in the
//! Cargo graph. It is receiver test data, never a production metadata mirror.
use super::*;
use crate::{
    agents::domain::AgentId,
    conversation::{
        application::{ConversationRepository, ConversationSummaries},
        domain::{
            Conversation, ConversationApprovalMode, ConversationDeletion, ConversationId,
            ConversationModelId, ConversationSummary,
        },
    },
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_local_database::rusqlite::{params, Connection, OptionalExtension};
use nessa_sync::replication::{
    catalogue::{
        apply_next_page, begin_or_resume, CataloguePagePlan, CatalogueProgress, CatalogueStore,
        CatalogueStoreError,
    },
    infrastructure::MemoryAuthorizer,
};
use std::{
    path::{Path, PathBuf},
    process::Command,
};
use uuid::Uuid;

fn sid(value: &str) -> Id {
    Id::new(value).unwrap()
}
fn owner() -> ConversationCaller {
    ConversationCaller {
        organization_id: OrganizationId::new("org").unwrap(),
        principal_id: PrincipalId::new("alice").unwrap(),
        surface_id: "panel".into(),
        action_id: "sync".into(),
    }
}
fn scope(receiver: &str, incarnation: &str) -> Scope {
    Scope::new(
        sid(receiver),
        sid("origin"),
        conversation_catalogue_stream(&owner()),
        sid(incarnation),
        conversation_catalogue_schema(),
        sid("epoch-1"),
    )
}
fn conversation(id: ConversationId, owner: &str) -> Conversation {
    Conversation::new(
        id,
        OrganizationId::new("org").unwrap(),
        PrincipalId::new(owner).unwrap(),
        "panel".into(),
        "create".into(),
        1,
        AgentId::Claude,
        ConversationModelId::new("model").unwrap(),
        ConversationApprovalMode::Ask,
    )
    .unwrap()
}
fn err(_: impl std::fmt::Debug) -> CatalogueStoreError {
    CatalogueStoreError::Failed
}
fn int(value: u64) -> Result<i64, CatalogueStoreError> {
    i64::try_from(value).map_err(err)
}
fn uint(value: i64) -> Result<u64, CatalogueStoreError> {
    u64::try_from(value).map_err(err)
}

struct SqliteReceiver {
    conn: Connection,
}
impl SqliteReceiver {
    fn open(path: &Path) -> Self {
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
    fn count(&self) -> i64 {
        self.conn
            .query_row("SELECT COUNT(*) FROM entries", [], |row| row.get(0))
            .unwrap()
    }
    fn payload(&self, id: &ConversationId) -> serde_json::Value {
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
    fn snapshot(conn: &Connection) -> Result<Option<CatalogueProgress>, CatalogueStoreError> {
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
                    sid(&receiver),
                    sid(&origin),
                    sid(&stream),
                    sid(&incarnation),
                    sid(&schema),
                    sid(&epoch),
                );
                let completed = uint(completed)?;
                let generation = uint(generation)?;
                let cursor = match (cursor_creation, cursor_id) {
                    (Some(creation), Some(id)) => Some(EntryKey {
                        creation: uint(creation)?,
                        id: sid(&id),
                    }),
                    (None, None) => None,
                    _ => return Err(CatalogueStoreError::Failed),
                };
                let active = boundary
                    .map(|boundary| {
                        Ok(CataloguePass {
                            scope: scope.clone(),
                            completed,
                            boundary: uint(boundary)?,
                            cursor,
                            generation,
                        })
                    })
                    .transpose()?;
                Ok(CatalogueProgress {
                    scope,
                    completed,
                    generation,
                    active,
                })
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
        if expected.as_ref().is_some_and(|p| p.scope != *scope) {
            return Err(CatalogueStoreError::ResetRequired);
        }
        let completed = expected.as_ref().map_or(0, |p| p.completed);
        let generation = expected.as_ref().map_or(1, |p| p.generation + 1);
        let active_boundary = if boundary == 0 {
            None
        } else {
            Some(int(boundary)?)
        };
        tx.execute("INSERT INTO progress VALUES(1,?1,?2,?3,?4,?5,?6,?7,?8,?9,NULL,NULL)
            ON CONFLICT(singleton) DO UPDATE SET completed=excluded.completed,generation=excluded.generation,boundary=excluded.boundary,cursor_creation=NULL,cursor_id=NULL",
            params![scope.receiver().as_str(),scope.origin().as_str(),scope.stream().as_str(),scope.incarnation().as_str(),scope.schema().as_str(),scope.access_epoch().as_str(),int(completed)?,int(generation)?,active_boundary]).map_err(err)?;
        tx.commit().map_err(err)?;
        Self::snapshot(&self.conn)?.ok_or(CatalogueStoreError::Failed)
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
        let tx = self.conn.transaction().map_err(err)?;
        if Self::snapshot(&tx)?.and_then(|p| p.active) != Some(plan.pass.clone()) {
            return Err(CatalogueStoreError::Stale);
        }
        for value in &plan.entries {
            let previous: Option<(i64, bool)> = tx
                .query_row(
                    "SELECT revision,deleted FROM entries WHERE id=?1",
                    [value.manifest.key.id.as_str()],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(err)?;
            if previous.is_some_and(|(_, deleted)| deleted && !value.manifest.deleted) {
                return Err(CatalogueStoreError::Fenced);
            }
            if previous
                .is_some_and(|(revision, _)| revision > int(value.manifest.revision).unwrap())
            {
                continue;
            }
            tx.execute("INSERT INTO entries VALUES(?1,?2,?3,?4,?5) ON CONFLICT(id) DO UPDATE SET revision=excluded.revision,deleted=excluded.deleted,payload=excluded.payload",
                params![value.manifest.key.id.as_str(),int(value.manifest.key.creation)?,int(value.manifest.revision)?,value.manifest.deleted,&value.payload]).map_err(err)?;
        }
        let completed = if plan.final_page {
            plan.pass.boundary
        } else {
            plan.pass.completed
        };
        let boundary = if plan.final_page {
            None
        } else {
            Some(int(plan.pass.boundary)?)
        };
        let cursor_creation = if plan.final_page {
            None
        } else {
            plan.next_cursor
                .as_ref()
                .map(|key| int(key.creation))
                .transpose()?
        };
        let cursor_id = if plan.final_page {
            None
        } else {
            plan.next_cursor.as_ref().map(|key| key.id.as_str())
        };
        tx.execute("UPDATE progress SET completed=?1,boundary=?2,cursor_creation=?3,cursor_id=?4 WHERE singleton=1", params![int(completed)?,boundary,cursor_creation,cursor_id]).map_err(err)?;
        tx.commit().map_err(err)?;
        Self::snapshot(&self.conn)?.ok_or(CatalogueStoreError::Failed)
    }
    fn reset(
        &mut self,
        scope: &Scope,
        expected: CatalogueProgress,
    ) -> Result<CatalogueProgress, CatalogueStoreError> {
        let tx = self.conn.transaction().map_err(err)?;
        if Self::snapshot(&tx)? != Some(expected.clone()) {
            return Err(CatalogueStoreError::Stale);
        }
        tx.execute("DELETE FROM entries WHERE deleted=0", [])
            .map_err(err)?;
        tx.execute("UPDATE progress SET receiver=?1,origin=?2,stream=?3,incarnation=?4,schema_id=?5,epoch=?6,completed=0,generation=?7,boundary=NULL,cursor_creation=NULL,cursor_id=NULL WHERE singleton=1",
            params![scope.receiver().as_str(),scope.origin().as_str(),scope.stream().as_str(),scope.incarnation().as_str(),scope.schema().as_str(),scope.access_epoch().as_str(),int(expected.generation+1)?]).map_err(err)?;
        tx.commit().map_err(err)?;
        Self::snapshot(&self.conn)?.ok_or(CatalogueStoreError::Failed)
    }
}

const CHILD: &str =
    "conversation::infrastructure::catalogue_source::receiver_tests::receiver_child";

#[tokio::test]
async fn receiver_child() {
    let Ok(source_path) = std::env::var("NESSA_259_CHILD_SOURCE") else {
        return;
    };
    let cache_path = PathBuf::from(std::env::var("NESSA_259_CHILD_CACHE").unwrap());
    let receiver = std::env::var("NESSA_259_CHILD_RECEIVER").unwrap();
    let store = Arc::new(
        super::super::store::LocalConversationStore::open(Path::new(&source_path)).unwrap(),
    );
    let head = store
        .head(&owner().organization_id, &owner().principal_id)
        .await
        .unwrap();
    let scope = scope(&receiver, &head.incarnation);
    let source =
        NessaCatalogueSource::new(store, owner(), scope.clone(), Handle::current()).unwrap();
    let stop_after_page = std::env::var_os("NESSA_259_CHILD_STOP_AFTER_PAGE").is_some();
    tokio::task::spawn_blocking(move || {
        let mut cache = SqliteReceiver::open(&cache_path);
        let mut source = source;
        let mut authorizer = MemoryAuthorizer::allowed(scope.clone());
        while let Some(mut pass) =
            begin_or_resume(&scope, &mut authorizer, &mut source, &mut cache).unwrap()
        {
            loop {
                let progress = apply_next_page(
                    &pass,
                    37,
                    128 * 1024,
                    &mut authorizer,
                    &mut source,
                    &mut cache,
                )
                .unwrap();
                if stop_after_page {
                    return;
                }
                match progress.active {
                    Some(next) => pass = next,
                    None => break,
                }
            }
        }
    })
    .await
    .unwrap();
}

fn child(source: &Path, cache: &Path, receiver: &str, stop_after_page: bool) {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .arg("--exact")
        .arg(CHILD)
        .arg("--nocapture")
        .env("NESSA_259_CHILD_SOURCE", source)
        .env("NESSA_259_CHILD_CACHE", cache)
        .env("NESSA_259_CHILD_RECEIVER", receiver);
    if stop_after_page {
        command.env("NESSA_259_CHILD_STOP_AFTER_PAGE", "1");
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[tokio::test]
async fn two_receiver_processes_reopen_independent_durable_progress() {
    let directory = tempfile::tempdir().unwrap();
    let private = directory.path().join("conversations");
    nessa_local_storage::create_directory(&private).unwrap();
    let source_path = private.join("metadata.sqlite3");
    let store = super::super::store::LocalConversationStore::open(&source_path).unwrap();
    let mut first = None;
    for index in 0..620 {
        let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
        if index == 0 {
            first = Some(id.clone());
        }
        let owner = if index < 600 { "alice" } else { "bob" };
        store.create(conversation(id, owner)).await.unwrap();
    }
    let left = directory.path().join("left.sqlite3");
    let right = directory.path().join("right.sqlite3");
    child(&source_path, &left, "receiver-left", true);
    assert_eq!(SqliteReceiver::open(&left).count(), 37);
    let mut paused_cache = SqliteReceiver::open(&left);
    let saved_pass = SqliteReceiver::snapshot(&paused_cache.conn)
        .unwrap()
        .unwrap()
        .active
        .unwrap();
    let delayed_pass = CataloguePass {
        cursor: None,
        ..saved_pass.clone()
    };
    let delayed_request = ManifestRequest {
        pass: delayed_pass.clone(),
        max_entries: 37,
    };
    assert_eq!(
        paused_cache.apply_page(CataloguePagePlan {
            pass: delayed_pass,
            manifest: ManifestPage {
                request: delayed_request,
                entries: vec![],
                has_more: false,
            },
            next_cursor: None,
            final_page: true,
            entries: vec![],
            unchanged: vec![],
        }),
        Err(CatalogueStoreError::Stale)
    );
    assert_eq!(paused_cache.count(), 37);
    child(&source_path, &right, "receiver-right", false);
    assert_eq!(SqliteReceiver::open(&right).count(), 600);
    let first = first.unwrap();
    store
        .record(
            &first,
            ConversationSummary::after_message(None, "changed while passing", None, 2),
        )
        .await
        .unwrap();
    let new_id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    store
        .create(conversation(new_id.clone(), "alice"))
        .await
        .unwrap();
    child(&source_path, &left, "receiver-left", false);
    assert_eq!(SqliteReceiver::open(&left).count(), 601);
    assert_eq!(
        SqliteReceiver::open(&left).payload(&first)["summary"]["preview"],
        "changed while passing"
    );
    assert_eq!(SqliteReceiver::open(&right).count(), 600);
    child(&source_path, &right, "receiver-right", false);
    assert_eq!(SqliteReceiver::open(&right).count(), 601);
    store
        .record_deletion(
            &first,
            ConversationDeletion::new(
                owner().organization_id,
                owner().principal_id,
                "panel".into(),
                "delete".into(),
                3,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    child(&source_path, &left, "receiver-left", false);
    let mut cache = SqliteReceiver::open(&left);
    let saved = SqliteReceiver::snapshot(&cache.conn).unwrap().unwrap();
    let changed = Scope::new(
        saved.scope.receiver().clone(),
        saved.scope.origin().clone(),
        saved.scope.stream().clone(),
        saved.scope.incarnation().clone(),
        saved.scope.schema().clone(),
        sid("epoch-2"),
    );
    let reset = cache.reset(&changed, saved).unwrap();
    assert_eq!(reset.completed, 0);
    assert_eq!(cache.count(), 1);
    assert_eq!(
        cache
            .cached_revision(&changed, &sid(&new_id.to_string()))
            .unwrap(),
        None
    );
    assert!(cache
        .cached_revision(&changed, &sid(&first.to_string()))
        .unwrap()
        .is_some());
}
