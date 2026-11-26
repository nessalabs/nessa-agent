//! Storage boundaries for receiver-keyed grants, separate from metadata.
use super::*;

#[test]
fn a_foreign_grant_version_refuses_before_opening_or_creating_metadata() {
    for metadata_exists in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let private = directory.path().join("conversations");
        nessa_local_storage::create_directory(&private).unwrap();
        let metadata = private.join("metadata.sqlite3");
        let grants = metadata.with_extension("grants.sqlite3");
        if metadata_exists {
            std::fs::write(&metadata, b"conversation file must not be opened").unwrap();
        }
        let old = nessa_local_database::Schema::new(
            "CREATE TABLE retained (id TEXT);\nPRAGMA user_version = 99;",
        )
        .unwrap();
        let connection = nessa_local_database::open(&grants, &old).unwrap();
        connection
            .execute("INSERT INTO retained VALUES ('keep')", [])
            .unwrap();
        drop(connection);
        let grant_bytes = std::fs::read(&grants).unwrap();
        let metadata_bytes = metadata_exists.then(|| std::fs::read(&metadata).unwrap());
        assert!(matches!(
            LocalConversationStore::open(&metadata),
            Err(crate::infrastructure::ConversationStoreOpenError::Grants {
                cause: nessa_local_database::OpenError::Version {
                    found: 99,
                    expected: 2
                },
                ..
            })
        ));
        assert_eq!(std::fs::read(&grants).unwrap(), grant_bytes);
        assert_eq!(metadata.exists(), metadata_exists);
        if let Some(bytes) = metadata_bytes {
            assert_eq!(std::fs::read(&metadata).unwrap(), bytes);
        }
    }
}

#[tokio::test]
async fn a_version_one_grant_file_keeps_its_rows_and_gains_the_log() {
    let opened = opened();
    let id = new_id();
    opened.store.create(owned(&id)).await.unwrap();
    assert!(grant(&opened.store, &id, "phone").await);
    drop(opened.store);
    let grants = opened.path.with_extension("grants.sqlite3");
    let connection = Connection::open(&grants).unwrap();
    let before: i64 = connection
        .query_row("SELECT count(*) FROM share_grants", [], |row| row.get(0))
        .unwrap();
    assert!(before >= 1);
    connection
        .execute_batch("DROP TABLE grant_log; DROP TABLE grant_streams; PRAGMA user_version = 1;")
        .unwrap();
    drop(connection);
    let store = LocalConversationStore::open(&opened.path).unwrap();
    assert!(store.held_grant(&id, "phone").await.unwrap().is_some());
    let connection = Connection::open(&grants).unwrap();
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 2);
    let after: i64 = connection
        .query_row("SELECT count(*) FROM share_grants", [], |row| row.get(0))
        .unwrap();
    assert_eq!(after, before);
    let streams: i64 = connection
        .query_row("SELECT count(*) FROM grant_streams", [], |row| row.get(0))
        .unwrap();
    assert_eq!(streams, 0);
}

#[test]
fn a_snapshot_refuses_a_foreign_grant_version_without_writing_either_file() {
    let opened = opened();
    let grants = opened.path.with_extension("grants.sqlite3");
    Connection::open(&grants)
        .unwrap()
        .execute_batch("PRAGMA user_version=999")
        .unwrap();
    let metadata_bytes = std::fs::read(&opened.path).unwrap();
    let grant_bytes = std::fs::read(&grants).unwrap();
    assert!(super::super::store::MetadataSnapshot::open(&opened.path).is_err());
    assert_eq!(std::fs::read(&opened.path).unwrap(), metadata_bytes);
    assert_eq!(std::fs::read(&grants).unwrap(), grant_bytes);
}

#[tokio::test]
async fn legacy_metadata_grants_are_ignored_and_preserved_when_grant_file_is_recreated() {
    let opened = opened();
    let id = new_id();
    opened.store.create(owned(&id)).await.unwrap();
    assert!(grant(&opened.store, &id, "phone").await);
    let connection = raw(&opened.path);
    connection.execute_batch("INSERT INTO main.share_grant_changes SELECT * FROM grants.share_grant_changes;
        INSERT INTO main.share_grants SELECT principal_id,resource_kind,resource_id,receiver_id,credential_id,capabilities,policy,policy_revision,granted_at_ms FROM grants.share_grants;").unwrap();
    drop(connection);
    drop(opened.store);
    std::fs::remove_file(opened.path.with_extension("grants.sqlite3")).unwrap();
    let store = LocalConversationStore::open(&opened.path).unwrap();
    assert!(ConversationRepository::load(&store, &id)
        .await
        .unwrap()
        .is_some());
    assert_eq!(store.held_grant(&id, "phone").await.unwrap(), None);
    assert!(store.grants(&id).await.unwrap().grants.is_empty());
    let retained: i64 = raw(&opened.path)
        .query_row("SELECT count(*) FROM main.share_grants", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(retained, 1);
}

#[tokio::test]
async fn backup_identities_distinguish_equal_receiver_and_principal_subject_text() {
    let opened = opened();
    let id = new_id();
    opened.store.create(owned(&id)).await.unwrap();
    let receiver = grant_change(
        ShareGrantTransition::Grant(ShareAccess::read()),
        &id,
        Some("same-id"),
        "receiver-credential",
        "receiver",
    );
    assert!(opened.store.change(receiver).await.unwrap().applied);
    let mut principal = grant_change(
        ShareGrantTransition::Grant(ShareAccess::read()),
        &id,
        None,
        "principal-credential",
        "principal",
    );
    principal.principal_id = Some(PrincipalId::new("same-id").unwrap());
    assert!(opened.store.change(principal).await.unwrap().applied);
    let facts = opened.store.export_facts().await.unwrap();
    assert_eq!(facts.grants.len(), 2);
    let identities: std::collections::HashSet<_> = facts
        .grants
        .iter()
        .map(|row| row.identity.as_str())
        .collect();
    assert_eq!(identities.len(), 2);
    assert!(identities
        .iter()
        .any(|id| id.ends_with(":receiver:same-id")));
    assert!(identities
        .iter()
        .any(|id| id.ends_with(":principal:same-id")));
}

#[test]
fn the_current_version_does_not_accept_an_old_grant_shape() {
    let opened = opened();
    let grants = opened.path.with_extension("grants.sqlite3");
    let connection = Connection::open(&grants).unwrap();
    connection.execute_batch("DROP TABLE person_links").unwrap();
    drop(connection);
    let before = std::fs::read(&grants).unwrap();
    assert!(matches!(
        LocalConversationStore::open(&opened.path),
        Err(crate::infrastructure::ConversationStoreOpenError::Grants {
            cause: nessa_local_database::OpenError::Damaged(_),
            ..
        })
    ));
    assert!(super::super::store::MetadataSnapshot::open(&opened.path).is_err());
    assert_eq!(std::fs::read(&grants).unwrap(), before);
}

#[tokio::test]
async fn contradictory_revoke_selectors_cannot_choose_between_same_principal_receivers() {
    let opened = opened();
    let id = new_id();
    opened.store.create(owned(&id)).await.unwrap();
    let principal = PrincipalId::new("same-owner").unwrap();
    for receiver in ["phone", "tablet"] {
        let mut change = grant_change(
            ShareGrantTransition::Grant(ShareAccess::read()),
            &id,
            Some(receiver),
            &format!("{receiver}-credential"),
            receiver,
        );
        change.principal_id = Some(principal.clone());
        assert!(opened.store.change(change).await.unwrap().applied);
    }
    let before = opened.store.grants(&id).await.unwrap();
    let journaled = role_journal(&opened.path).len();
    for (receiver, credential, named) in [
        ("tablet", "phone-credential", None),
        ("tablet", "missing-credential", None),
        (
            "phone",
            "phone-credential",
            Some(PrincipalId::new("other").unwrap()),
        ),
    ] {
        let mut change = grant_change(
            ShareGrantTransition::Revoke,
            &id,
            Some(receiver),
            credential,
            &format!("bad-{credential}-{receiver}"),
        );
        change.principal_id = named;
        assert!(matches!(
            opened.store.change(change).await,
            Err(ConversationError::InvalidInput)
        ));
        assert_eq!(opened.store.grants(&id).await.unwrap(), before);
        assert_eq!(role_journal(&opened.path).len(), journaled);
    }
    assert!(revoke(&opened.store, &id, "phone").await);
    assert_eq!(opened.store.held_grant(&id, "phone").await.unwrap(), None);
    assert!(opened
        .store
        .held_grant(&id, "tablet")
        .await
        .unwrap()
        .is_some());
}

#[test]
fn a_refused_grant_upgrade_preserves_the_original_schema_and_file() {
    let opened = opened();
    let grants = opened.path.with_extension("grants.sqlite3");
    drop(opened.store);
    let connection = Connection::open(&grants).unwrap();
    connection
        .execute_batch(
            "DROP TABLE grant_streams; DROP TABLE grant_log;
         DROP TABLE person_links; PRAGMA user_version=1;",
        )
        .unwrap();
    drop(connection);
    let before = std::fs::read(&grants).unwrap();
    assert!(LocalConversationStore::open(&opened.path).is_err());
    assert_eq!(std::fs::read(&grants).unwrap(), before);
    let connection = Connection::open(&grants).unwrap();
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name='grant_log'",
                [],
                |r| r.get::<_, u32>(0)
            )
            .unwrap(),
        0
    );
}

#[test]
fn a_refused_current_grant_shape_preserves_wal_mode_and_contents() {
    let opened = opened();
    let grants = opened.path.with_extension("grants.sqlite3");
    drop(opened.store);
    let connection = Connection::open(&grants).unwrap();
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .unwrap();
    connection
        .execute_batch("DROP TABLE person_links; PRAGMA wal_checkpoint(TRUNCATE);")
        .unwrap();
    let before = std::fs::read(&grants).unwrap();
    assert!(LocalConversationStore::open(&opened.path).is_err());
    assert_eq!(std::fs::read(&grants).unwrap(), before);
    assert_eq!(
        connection
            .pragma_query_value(None, "journal_mode", |r| r.get::<_, String>(0))
            .unwrap(),
        "wal"
    );
}

#[tokio::test]
async fn compatibility_open_leaves_existing_v2_grants_in_place() {
    let opened = opened();
    let id = new_id();
    opened.store.create(owned(&id)).await.unwrap();
    assert!(grant(&opened.store, &id, "phone").await);
    drop(opened.store);
    let grants = opened.path.with_extension("grants.sqlite3");
    let before = std::fs::read(&grants).unwrap();
    let store = LocalConversationStore::open(&opened.path).unwrap();
    assert!(store.held_grant(&id, "phone").await.unwrap().is_some());
    drop(super::super::store::MetadataSnapshot::open(&opened.path).unwrap());
    drop(store);
    assert_eq!(std::fs::read(&grants).unwrap(), before);
    let database = Connection::open(&grants).unwrap();
    assert_eq!(grant_file_version(&database), 2);
    let directory_tables: i64 = database
        .query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name IN
             ('directory_grant_state','directory_receiver_log','directory_grant_evidence')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(directory_tables, 0);
}

fn grant_file_version(database: &Connection) -> i64 {
    database
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap()
}

#[tokio::test]
async fn compatibility_open_preserves_activated_grants_without_a_directory_writer() {
    let opened = opened();
    let id = new_id();
    opened.store.create(owned(&id)).await.unwrap();
    assert!(grant(&opened.store, &id, "phone").await);
    let second = new_id();
    opened.store.create(owned(&second)).await.unwrap();
    let original_stream = opened.store.grant_stream("phone").await.unwrap();
    assert_eq!(original_stream.head, 1);
    let original_replay = opened.store.grant_log_after("phone", 0).await.unwrap();
    assert_eq!(original_replay.len(), 1);
    drop(opened.store);
    let grants = opened.path.with_extension("grants.sqlite3");
    let mut database = Connection::open(&grants).unwrap();
    let transaction = database.transaction().unwrap();
    transaction
        .execute_batch(include_str!(
            "../../src/infrastructure/store/grants_from_2.sql"
        ))
        .unwrap();
    transaction
        .execute_batch(
            "INSERT INTO directory_grant_state
                VALUES ('directory','fixture','/fixture','receiver','phone',1,'[]',1);
             INSERT INTO directory_receiver_log
                VALUES ('phone',1,'directory','fixture','/fixture',1,1);
             INSERT INTO directory_grant_evidence
                VALUES (1,'fixture-evidence','operator','request','set','fixture','/fixture',
                        'receiver','phone','{}','{}',0);",
        )
        .unwrap();
    transaction.commit().unwrap();
    assert_eq!(grant_file_version(&database), 3);
    let directory_rows = retained_directory_rows(&database);
    drop(database);
    let before = std::fs::read(&grants).unwrap();
    let store = LocalConversationStore::open(&opened.path).unwrap();
    assert!(store.held_grant(&id, "phone").await.unwrap().is_some());
    drop(super::super::store::MetadataSnapshot::open(&opened.path).unwrap());
    drop(store);
    assert_eq!(std::fs::read(&grants).unwrap(), before);

    let store = LocalConversationStore::open(&opened.path)
        .unwrap()
        .with_access_policy(crate::conversation_test_support::access_policy());
    assert_eq!(store.grant_stream("phone").await.unwrap(), original_stream);
    assert!(grant(&store, &second, "phone").await);
    let next = store.grant_stream("phone").await.unwrap();
    assert_eq!(next.incarnation, original_stream.incarnation);
    assert_eq!(next.oldest, original_stream.oldest);
    assert_eq!(next.head, original_stream.head + 1);
    let remaining = store
        .grant_log_after("phone", original_stream.head)
        .await
        .unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].position, next.head);
    assert_eq!(remaining[0].conversation_id, second.to_string());
    drop(store);

    let reopened = LocalConversationStore::open(&opened.path).unwrap();
    assert!(reopened.held_grant(&id, "phone").await.unwrap().is_some());
    assert!(reopened
        .held_grant(&second, "phone")
        .await
        .unwrap()
        .is_some());
    assert_eq!(reopened.grant_stream("phone").await.unwrap(), next);
    assert_eq!(
        reopened
            .grant_log_after("phone", original_stream.head)
            .await
            .unwrap(),
        remaining
    );
    let complete = reopened.grant_log_after("phone", 0).await.unwrap();
    assert_eq!(complete, [original_replay, remaining].concat());
    drop(reopened);
    let database = Connection::open(&grants).unwrap();
    assert_eq!(grant_file_version(&database), 3);
    assert_eq!(retained_directory_rows(&database), directory_rows);
}

// Keep all typed values (including the pending acknowledgement and its exact
// event contents), not only counts or identifiers, across the legacy write.
fn retained_directory_rows(
    database: &Connection,
) -> Vec<Vec<nessa_local_database::rusqlite::types::Value>> {
    [
        "SELECT * FROM directory_grant_state",
        "SELECT * FROM directory_receiver_log",
        "SELECT * FROM directory_grant_evidence",
    ]
    .iter()
    .map(|sql| {
        let mut statement = database.prepare(sql).unwrap();
        let columns = statement.column_count();
        let rows = statement
            .query_map([], |row| {
                (0..columns).map(|column| row.get(column)).collect()
            })
            .unwrap()
            .collect::<Result<Vec<Vec<_>>, _>>()
            .unwrap();
        assert_eq!(rows.len(), 1);
        rows.into_iter().next().unwrap()
    })
    .collect()
}

#[test]
fn a_v3_header_with_missing_directory_tables_is_refused_without_writes() {
    let opened = opened();
    drop(opened.store);
    let grants = opened.path.with_extension("grants.sqlite3");
    Connection::open(&grants)
        .unwrap()
        .execute_batch("PRAGMA user_version = 3")
        .unwrap();
    let metadata_before = std::fs::read(&opened.path).unwrap();
    let grant_before = std::fs::read(&grants).unwrap();
    assert!(matches!(
        LocalConversationStore::open(&opened.path),
        Err(crate::infrastructure::ConversationStoreOpenError::Grants {
            cause: nessa_local_database::OpenError::Damaged(_),
            ..
        })
    ));
    assert!(super::super::store::MetadataSnapshot::open(&opened.path).is_err());
    assert_eq!(std::fs::read(&opened.path).unwrap(), metadata_before);
    assert_eq!(std::fs::read(&grants).unwrap(), grant_before);
}
