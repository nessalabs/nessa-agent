//! Synthetic version-6 metadata, preserving cursors and grant evidence (#724).
use super::*;
use crate::conversation::application::{ShareGrantChange, ShareGrantReceipt};

const VERSION_6: &str = include_str!("fixtures/metadata-v6.sql");

fn retarget(
    id: &ConversationId,
    receiver: &str,
    credential: &str,
    request: &str,
) -> ShareGrantChange {
    let mut change = grant_change(
        ShareGrantTransition::Grant(ShareAccess::read()),
        id,
        Some(receiver),
        credential,
        request,
    );
    change.principal_id = Some(PrincipalId::new("one-grantee").unwrap());
    change
}

fn rows(connection: &Connection, table: &str) -> Vec<Vec<String>> {
    let mut statement = connection
        .prepare(&format!(
            "SELECT * FROM \"{}\" ORDER BY rowid",
            table.replace('"', "\"\"")
        ))
        .unwrap();
    let columns = statement.column_count();
    statement
        .query_map([], |row| {
            (0..columns)
                .map(|column| row.get_ref(column).map(|value| format!("{value:?}")))
                .collect()
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn snapshot(path: &Path) -> Vec<(String, Vec<Vec<String>>)> {
    let connection = raw(path);
    let mut tables = connection.prepare("SELECT name FROM sqlite_schema WHERE type = 'table' AND name <> 'catalogue_reader_floors' ORDER BY name").unwrap();
    tables
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(|table| {
            let table = table.unwrap();
            let kept = rows(&connection, &table);
            (table, kept)
        })
        .collect()
}

/// Copy real store-produced rows into the frozen version-6 schema, which has no
/// floor table. This is a genuine predecessor shape, not a version pragma on a
/// new database. The write path producing those rows is unchanged by migration.
fn version_6_copy(source: &Path) -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let private = directory.path().join("legacy");
    nessa_local_storage::create_directory(&private).unwrap();
    let path = private.join("metadata.sqlite3");
    let mut destination = nessa_local_database::open(
        &path,
        &nessa_local_database::Schema::new(VERSION_6).unwrap(),
    )
    .unwrap();
    let source = raw(source);
    let transaction = destination.transaction().unwrap();
    for table in [
        "catalogue_identity",
        "catalogue_owners",
        "conversations",
        "mode_requests",
        "deletions",
        "summaries",
        "share_grant_changes",
        "share_grants",
        "sqlite_sequence",
    ] {
        transaction
            .execute(&format!("DELETE FROM {table}"), [])
            .unwrap();
        let mut statement = source
            .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
            .unwrap();
        let columns = statement.column_count();
        let values = statement
            .query_map([], |row| {
                (0..columns)
                    .map(|column| {
                        row.get::<_, nessa_local_database::rusqlite::types::Value>(column)
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .unwrap();
        let placeholders = vec!["?"; columns].join(",");
        for value in values {
            transaction
                .execute(
                    &format!("INSERT INTO {table} VALUES ({placeholders})"),
                    nessa_local_database::rusqlite::params_from_iter(value.unwrap()),
                )
                .unwrap();
        }
    }
    transaction.commit().unwrap();
    drop(destination);
    (directory, path)
}

async fn reader_head(
    store: &LocalConversationStore,
    receiver: &str,
) -> crate::conversation::application::CatalogueHead {
    store
        .head(&org(), &alice(), &device(receiver))
        .await
        .unwrap()
}

fn reopen(path: &Path) -> LocalConversationStore {
    LocalConversationStore::open(path)
        .unwrap()
        .with_access_policy(crate::conversation_test_support::access_policy())
}

#[test]
fn new_metadata_has_no_legacy_floors() {
    let opened = opened();
    let connection = raw(&opened.path);
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
            .unwrap(),
        7
    );
    assert_eq!(rows(&connection, "catalogue_reader_floors").len(), 0);
}

#[tokio::test]
async fn metadata_upgrade_preserves_old_cursors_and_grant_receipts() {
    let opened = opened();
    let store = &opened.store;
    let (moved, kept_first, kept_second) = (new_id(), new_id(), new_id());
    for id in [&moved, &kept_first, &kept_second] {
        store.create(owned(id)).await.unwrap();
    }
    assert!(grant(store, &kept_first, "first").await);
    assert!(grant(store, &kept_second, "first").await);
    let mut receipt: Option<ShareGrantReceipt> = None;
    for receiver in ["retired", "first", "second", "third"] {
        receipt = Some(
            store
                .change(retarget(
                    &moved,
                    receiver,
                    &format!("credential-{receiver}"),
                    &format!("request-{receiver}"),
                ))
                .await
                .unwrap(),
        );
    }
    // The last retarget is exactly the legacy head exposed to all three
    // receivers. The corrected query already stops first at second's removal.
    let legacy = store.head(&org(), &alice(), &Reader::Owner).await.unwrap();
    assert!(reader_head(store, "first").await.revision < legacy.revision);
    let mut first_page = catalogue_request(
        alice(),
        &legacy.incarnation,
        0,
        reader_head(store, "first").await.revision,
        None,
        1,
    );
    first_page.reader = device("first");
    let first_page = store.page(first_page).await.unwrap();
    assert_eq!(first_page.entries.len(), 1);
    let saved_key = first_page.entries[0].key.clone();
    assert_eq!(saved_key.id, kept_first);

    // Same receiver in two other scopes must retain its own head, not this
    // owner's floor. The ordinary writer supplies authentic grant receipts.
    for (organization, owner) in [("org", "bob"), ("other-org", "alice")] {
        let id = new_id();
        store
            .create(owned_by(&id, organization, owner, 1))
            .await
            .unwrap();
        let mut change = grant_change(
            ShareGrantTransition::Grant(ShareAccess::read()),
            &id,
            Some("first"),
            "foreign-credential",
            &format!("foreign-{organization}-{owner}"),
        );
        change.initiator.organization_id = OrganizationId::new(organization).unwrap();
        change.initiator.principal_id = PrincipalId::new(owner).unwrap();
        assert!(store.change(change).await.unwrap().applied);
    }
    // A directory-kind history using an existing conversation ID is not a
    // conversation receiver, even though a join on resource_id alone matches.
    raw(&opened.path).execute("INSERT INTO share_grant_changes (principal_id, resource_kind, resource_id, receiver_id, credential_id, before, after, initiator, surface, request, changed_at_ms, revision) VALUES ('directory-principal', 'directory', ?1, 'directory-only', 'directory-credential', 'none', 'read', 'alice', 'desktop', 'directory-request', 7, 999)", params![moved.to_string()]).unwrap();
    let (_directory, path) = version_6_copy(&opened.path);
    let before = snapshot(&path);
    let store = reopen(&path);
    assert_eq!(
        snapshot(&path),
        before,
        "migration changed grant or replay evidence"
    );
    assert_eq!(reader_head(&store, "first").await, legacy);
    assert_eq!(reader_head(&store, "second").await, legacy);
    assert_eq!(reader_head(&store, "retired").await, legacy);
    assert_eq!(reader_head(&store, "third").await, legacy);
    assert_eq!(reader_head(&store, "directory-only").await.revision, 0);
    for (organization, owner) in [("org", "bob"), ("other-org", "alice")] {
        assert_eq!(
            store
                .head(
                    &OrganizationId::new(organization).unwrap(),
                    &PrincipalId::new(owner).unwrap(),
                    &device("first")
                )
                .await
                .unwrap()
                .revision,
            2
        );
    }
    let mut saved_page = catalogue_request(
        alice(),
        &legacy.incarnation,
        0,
        legacy.revision,
        Some(saved_key),
        1,
    );
    saved_page.reader = device("first");
    assert_eq!(
        store.page(saved_page.clone()).await.unwrap().entries[0]
            .key
            .id,
        kept_second
    );
    assert!(device_pass(&store, device("first"), legacy.revision)
        .await
        .is_empty());
    assert_eq!(
        store
            .change(retarget(
                &moved,
                "third",
                "credential-third",
                "request-third"
            ))
            .await
            .unwrap(),
        receipt.unwrap()
    );
    assert_eq!(
        snapshot(&path),
        before,
        "lost reply was not replayed unchanged"
    );
    let floors = rows(&raw(&path), "catalogue_reader_floors");
    assert_eq!(floors.len(), 6);
    let mut first = store.watch_reader(&org(), &alice(), "first").unwrap();
    let mut second = store.watch_reader(&org(), &alice(), "second").unwrap();
    let mut third = store.watch_reader(&org(), &alice(), "third").unwrap();
    assert!(
        store
            .change(retarget(
                &moved,
                "fourth",
                "credential-fourth",
                "request-fourth",
            ))
            .await
            .unwrap()
            .applied
    );
    catalogue_watch_pending(&mut first);
    catalogue_watch_pending(&mut second);
    assert_eq!(
        catalogue_watch_ready(&mut third),
        CatalogueWatchState::Dirty
    );
    assert_eq!(reader_head(&store, "first").await, legacy);
    assert_eq!(reader_head(&store, "second").await, legacy);
    drop(store);
    let store = reopen(&path);
    let mut first = store.watch_reader(&org(), &alice(), "first").unwrap();
    assert_eq!(
        rows(&raw(&path), "catalogue_reader_floors"),
        floors,
        "restart recaptured unrelated activity"
    );
    assert_eq!(reader_head(&store, "first").await, legacy);
    assert!(device_pass(&store, device("first"), legacy.revision)
        .await
        .is_empty());
    assert_eq!(
        store.page(saved_page).await.unwrap().entries[0].key.id,
        kept_second
    );
    // A legitimate visible write still moves past the preserved floor.
    store
        .record(&kept_second, said("visible after upgrade", 99))
        .await
        .unwrap();
    assert_eq!(
        catalogue_watch_ready(&mut first),
        CatalogueWatchState::Dirty
    );
    assert!(reader_head(&store, "first").await.revision > legacy.revision);
    assert_eq!(
        device_pass(&store, device("first"), legacy.revision).await,
        vec![(kept_second, reader_head(&store, "first").await.revision)]
    );
    assert_eq!(rows(&raw(&path), "catalogue_reader_floors"), floors);
}

#[tokio::test]
async fn failed_metadata_upgrade_rolls_back_and_can_retry() {
    let opened = opened();
    let id = new_id();
    opened.store.create(owned(&id)).await.unwrap();
    assert!(grant(&opened.store, &id, "phone").await);
    let (_directory, path) = version_6_copy(&opened.path);
    // A bad cell in synthetic predecessor history makes floor capture fail
    // after preparing the upgrade table/version. Never silently omit a receiver.
    raw(&path)
        .execute(
            "UPDATE share_grant_changes SET receiver_id = CAST(x'80' AS TEXT)",
            [],
        )
        .unwrap();
    let before = snapshot(&path);
    assert!(matches!(
        LocalConversationStore::open(&path),
        Err(nessa_local_database::OpenError::Database(_))
    ));
    assert_eq!(snapshot(&path), before);
    let connection = raw(&path);
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
            .unwrap(),
        6
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name = 'catalogue_reader_floors'",
                [],
                |row| row.get::<_, u32>(0)
            )
            .unwrap(),
        0
    );
    connection
        .execute("UPDATE share_grant_changes SET receiver_id = 'phone'", [])
        .unwrap();
    drop(connection);
    let store = reopen(&path);
    assert!(reader_head(&store, "phone").await.revision > 0);
}

#[tokio::test]
async fn metadata_upgrade_refuses_a_head_beyond_its_owner_counter() {
    let opened = opened();
    let id = new_id();
    opened.store.create(owned(&id)).await.unwrap();
    assert!(grant(&opened.store, &id, "phone").await);
    let (_directory, path) = version_6_copy(&opened.path);
    raw(&path)
        .execute("UPDATE share_grant_changes SET revision = 999", [])
        .unwrap();
    let before = snapshot(&path);
    assert!(matches!(
        LocalConversationStore::open(&path),
        Err(nessa_local_database::OpenError::Database(
            nessa_local_database::rusqlite::Error::InvalidQuery
        ))
    ));
    assert_eq!(snapshot(&path), before);
    assert_eq!(
        raw(&path)
            .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
            .unwrap(),
        6
    );
}

#[tokio::test]
async fn receiver_heads_ignore_regrant_after_revoke() {
    for noop_revoke in [false, true] {
        let opened = opened();
        let store = &opened.store;
        let id = new_id();
        store.create(owned(&id)).await.unwrap();
        assert!(
            store
                .change(retarget(&id, "first", "credential-one", "grant-one"))
                .await
                .unwrap()
                .applied
        );
        let mut first = store.watch_reader(&org(), &alice(), "first").unwrap();
        // Same receiver, new credential is a relevant transition, including after
        // an intervening capability/no-op journal row.
        assert!(
            !store
                .change(retarget(&id, "first", "credential-one", "noop"))
                .await
                .unwrap()
                .applied
        );
        catalogue_watch_pending(&mut first);
        let before = reader_head(store, "first").await;
        assert!(
            store
                .change(retarget(
                    &id,
                    "first",
                    "credential-two",
                    "credential-replace",
                ))
                .await
                .unwrap()
                .applied
        );
        assert!(reader_head(store, "first").await.revision > before.revision);
        assert_eq!(
            catalogue_watch_ready(&mut first),
            CatalogueWatchState::Dirty
        );
        let mut revoke = grant_change(
            ShareGrantTransition::Revoke,
            &id,
            None,
            "credential-two",
            "revoke",
        );
        revoke.principal_id = Some(PrincipalId::new("one-grantee").unwrap());
        assert!(store.change(revoke.clone()).await.unwrap().applied);
        assert_eq!(
            catalogue_watch_ready(&mut first),
            CatalogueWatchState::Dirty
        );
        let removed = reader_head(store, "first").await;
        if noop_revoke {
            revoke.initiator.action_id = "noop-revoke".into();
            assert!(!store.change(revoke).await.unwrap().applied);
            catalogue_watch_pending(&mut first);
        }
        for receiver in ["second", "third"] {
            assert!(
                store
                    .change(retarget(
                        &id,
                        receiver,
                        &format!("credential-{receiver}"),
                        &format!("regrant-{receiver}"),
                    ))
                    .await
                    .unwrap()
                    .applied
            );
            assert_eq!(reader_head(store, "first").await, removed);
            catalogue_watch_pending(&mut first);
        }
    }
}
