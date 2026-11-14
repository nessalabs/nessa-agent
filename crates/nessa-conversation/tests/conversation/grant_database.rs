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
                    expected: 1
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
