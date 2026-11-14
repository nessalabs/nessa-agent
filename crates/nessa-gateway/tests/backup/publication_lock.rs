//! First-use ordering of the export reader and paired publication.

use std::fs::TryLockError;

use nessa_local_database::rusqlite::Connection;

use super::*;

#[test]
fn absent_receiver_parent_cut_excludes_first_publisher() {
    let namespace = tempfile::tempdir().unwrap();
    let access = namespace.path().join("receiver-access");
    let path = access.join("publication.lock");
    let reader = publication_lock(&path).unwrap();
    assert!(
        path.is_file(),
        "an absent receiver store still needs a stable lock"
    );
    let writer = open(&path, OpenMode::OpenOrCreate).unwrap();
    assert!(matches!(writer.try_lock(), Err(TryLockError::WouldBlock)));
    let registry = namespace.path().join("auth/credentials.v1.json");
    let receivers = access.join("receiver-access.sqlite3");
    assert!(matches!(
        read_credential_state(&registry),
        CredState::Absent
    ));
    assert!(matches!(
        open_receiver_state(&receivers),
        ReceiverState::Absent
    ));
    drop(reader);

    writer.try_lock().unwrap();
    let contender = open(&path, OpenMode::OpenOrCreate).unwrap();
    assert!(matches!(
        contender.try_lock(),
        Err(TryLockError::WouldBlock)
    ));
    fs::create_dir_all(registry.parent().unwrap()).unwrap();
    let credential = serde_json::json!({"credentials": [{
        "metadata": {
            "id": "synthetic-device", "principalId": "user:synthetic",
            "organizationId": "synthetic-org", "audienceId": "synthetic-gateway",
            "issuedAt": 1, "expiresAt": null, "revokedAt": null, "grants": []
        },
        "verifier": "SYNTHETIC-VERIFIER"
    }]});
    fs::write(&registry, serde_json::to_vec(&credential).unwrap()).unwrap();
    let database = Connection::open(&receivers).unwrap();
    database.execute_batch(
        "CREATE TABLE receiver_transitions(sequence INTEGER);
         CREATE TABLE receivers(receiver_id TEXT, credential_id TEXT, access_epoch INTEGER, active INTEGER);
         INSERT INTO receiver_transitions VALUES (1);
         INSERT INTO receivers VALUES ('synthetic-receiver', 'synthetic-device', 1, 1);",
    ).unwrap();
    drop(database);
    drop(writer);
    let (credentials, receivers) = snapshot_paired(&path, &registry, &receivers);
    let CredState::Read(credentials) = credentials else {
        panic!("the completed synthetic credential must be readable");
    };
    assert_eq!(credentials.credentials.len(), 1);
    assert_eq!(credentials.credentials[0].id, "synthetic-device");
    let ReceiverState::Open(receivers) = receivers else {
        panic!("the completed synthetic receiver must be readable");
    };
    let (_, rows, skipped) = receivers.read().unwrap();
    assert!(skipped.is_empty());
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].receiver_id, "synthetic-receiver");
    assert_eq!(rows[0].credential_id, credentials.credentials[0].id);
}

#[test]
fn a_publication_lock_parent_that_is_a_file_is_refused() {
    let namespace = tempfile::tempdir().unwrap();
    let parent = namespace.path().join("receiver-access");
    fs::write(&parent, b"not a directory").unwrap();
    assert!(publication_lock(&parent.join("publication.lock")).is_err());
}

#[cfg(unix)]
#[test]
fn a_publication_lock_parent_symlink_is_refused() {
    let namespace = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let parent = namespace.path().join("receiver-access");
    std::os::unix::fs::symlink(target.path(), &parent).unwrap();
    assert!(publication_lock(&parent.join("publication.lock")).is_err());
    assert!(!target.path().join("publication.lock").exists());
}
