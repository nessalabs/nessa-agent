#![cfg(feature = "disposable-store")]

use std::{collections::BTreeMap, path::PathBuf};

use nessa_app_package::{
    verify_package, AssetIntegrityError, InstalledAppStore, PackageError, StoreError,
    VerifiedPackage,
};
use nessa_local_database::rusqlite::{params, Connection};
use nessa_types::app_identity::ExtensionAppId;
use serde_json::json;
use sha2::{Digest, Sha256};

fn private_database() -> (tempfile::TempDir, PathBuf) {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("private");
    nessa_local_storage::create_directory(&root).unwrap();
    (fixture, root.join("installed.sqlite3"))
}

fn package(id: &str, contents: Vec<u8>) -> VerifiedPackage {
    let bytes = BTreeMap::from([
        ("index.html".into(), contents),
        ("static/style.css".into(), b"body{}".to_vec()),
    ]);
    let declarations = bytes.iter().map(|(path, data)| {
        json!({"path":path,"sizeBytes":data.len(),"sha256":hex::encode(Sha256::digest(data))})
    }).collect::<Vec<_>>();
    let manifest = json!({
        "schemaVersion":1, "appId":id, "packageVersion":"1.0.0",
        "entrypoint":"index.html", "assets":declarations, "requestedCapabilities":["read"]
    });
    verify_package(serde_json::to_vec(&manifest).unwrap(), bytes).unwrap()
}

#[test]
fn installs_two_applications_and_reopens_the_same_verified_bytes() {
    let (_fixture, path) = private_database();
    let calendar = package("calendar", vec![b'x'; 65_537]);
    let notes = package("notes", b"notes".to_vec());
    let mut store = InstalledAppStore::open(&path).unwrap();
    assert!(store.load(calendar.manifest().app_id()).unwrap().is_none());
    store.install(&calendar).unwrap();
    store.install(&notes).unwrap();
    let replacement = package("calendar", b"replacement".to_vec());
    assert!(matches!(
        store.install(&replacement),
        Err(StoreError::AlreadyInstalled)
    ));
    drop(store);

    let mut store = InstalledAppStore::open(&path).unwrap();
    for expected in [calendar, notes] {
        let loaded = store.load(expected.manifest().app_id()).unwrap().unwrap();
        assert_eq!(loaded.manifest().json(), expected.manifest().json());
        assert_eq!(
            loaded.assets().collect::<Vec<_>>(),
            expected.assets().collect::<Vec<_>>()
        );
    }
}

#[test]
fn rejects_tampering_on_reopen_and_preserves_the_other_application() {
    let (_fixture, path) = private_database();
    let calendar = package("calendar", b"abc".to_vec());
    let notes = package("notes", b"notes".to_vec());
    let mut store = InstalledAppStore::open(&path).unwrap();
    store.install(&calendar).unwrap();
    store.install(&notes).unwrap();
    drop(store);

    for (tampered, reason) in [
        (b"axc".as_slice(), AssetIntegrityError::DigestMismatch),
        (b"abcd".as_slice(), AssetIntegrityError::LengthMismatch),
    ] {
        let connection = Connection::open(&path).unwrap();
        connection.execute("UPDATE installed_app_assets SET bytes = ?1 WHERE app_id = 'calendar' AND path = 'index.html'", [tampered]).unwrap();
        drop(connection);

        let mut store = InstalledAppStore::open(&path).unwrap();
        assert!(matches!(
            store.load(calendar.manifest().app_id()),
            Err(StoreError::InvalidPackage(PackageError::AssetIntegrity(error))) if error == reason
        ));
        assert_eq!(
            store
                .load(notes.manifest().app_id())
                .unwrap()
                .unwrap()
                .asset("index.html"),
            Some(b"notes".as_slice())
        );
    }
}

#[test]
fn rolls_back_a_failed_asset_insert_before_reopen() {
    let (_fixture, path) = private_database();
    let notes = package("notes", b"notes".to_vec());
    let calendar = package("calendar", b"calendar".to_vec());
    let mut store = InstalledAppStore::open(&path).unwrap();
    store.install(&notes).unwrap();
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER refuse_stylesheet BEFORE INSERT ON installed_app_assets
         WHEN NEW.app_id = 'calendar' AND NEW.path = 'static/style.css'
         BEGIN SELECT RAISE(ABORT, 'fixture interruption'); END;",
        )
        .unwrap();
    drop(connection);

    assert!(matches!(
        store.install(&calendar),
        Err(StoreError::Database(_))
    ));
    drop(store);
    let mut store = InstalledAppStore::open(&path).unwrap();
    assert!(store.load(calendar.manifest().app_id()).unwrap().is_none());
    assert!(store.load(notes.manifest().app_id()).unwrap().is_some());
    let connection = Connection::open(&path).unwrap();
    let stranded: i64 = connection
        .query_row(
            "SELECT count(*) FROM installed_app_assets WHERE app_id = ?1",
            ["calendar"],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stranded, 0);
}

#[test]
fn refuses_a_manifest_bound_to_a_different_app_identity() {
    let (_fixture, path) = private_database();
    let calendar = package("calendar", b"abc".to_vec());
    let notes = package("notes", b"abc".to_vec());
    let mut store = InstalledAppStore::open(&path).unwrap();
    store.install(&calendar).unwrap();
    drop(store);
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE installed_apps SET manifest = ?1 WHERE app_id = 'calendar'",
            params![notes.manifest().json()],
        )
        .unwrap();
    drop(connection);

    let mut store = InstalledAppStore::open(&path).unwrap();
    let id = ExtensionAppId::new("calendar").unwrap();
    assert!(matches!(
        store.load(&id),
        Err(StoreError::AppIdentityMismatch)
    ));
}
