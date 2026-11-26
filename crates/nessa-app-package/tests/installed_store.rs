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
    package_with_version(id, "1.0.0", contents)
}

fn package_with_version(id: &str, version: &str, contents: Vec<u8>) -> VerifiedPackage {
    let bytes = BTreeMap::from([
        ("index.html".into(), contents),
        ("static/style.css".into(), b"body{}".to_vec()),
    ]);
    let declarations = bytes.iter().map(|(path, data)| {
        json!({"path":path,"sizeBytes":data.len(),"sha256":hex::encode(Sha256::digest(data))})
    }).collect::<Vec<_>>();
    let manifest = json!({
        "schemaVersion":1, "appId":id, "packageVersion":version,
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
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch("DROP TRIGGER refuse_stylesheet")
        .unwrap();
    drop(connection);
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
    assert!(matches!(
        store.inventory(),
        Err(StoreError::AppIdentityMismatch)
    ));
}

#[test]
fn inventory_is_ordered_and_removal_matches_the_exact_id() {
    let (_fixture, path) = private_database();
    let left = package("calendar", b"calendar".to_vec());
    let right = package_with_version("calendar-work", "2.3.4", b"work".to_vec());
    let missing = ExtensionAppId::new("cal").unwrap();
    let mut store = InstalledAppStore::open(&path).unwrap();
    assert!(store.inventory().unwrap().is_empty());

    store.install(&right).unwrap();
    store.install(&left).unwrap();
    let inventory = store.inventory().unwrap();
    assert_eq!(
        inventory
            .iter()
            .map(|app| (app.app_id().as_str(), app.package_version()))
            .collect::<Vec<_>>(),
        [("calendar", "1.0.0"), ("calendar-work", "2.3.4")]
    );
    assert!(matches!(
        store.remove(&missing),
        Err(StoreError::NotInstalled)
    ));
    store.remove(left.manifest().app_id()).unwrap();
    assert!(matches!(
        store.remove(left.manifest().app_id()),
        Err(StoreError::NotInstalled)
    ));
    drop(store);

    let mut store = InstalledAppStore::open(&path).unwrap();
    assert!(store.load(left.manifest().app_id()).unwrap().is_none());
    let retained = store.load(right.manifest().app_id()).unwrap().unwrap();
    assert_eq!(retained.manifest().json(), right.manifest().json());
    assert_eq!(
        retained.assets().collect::<Vec<_>>(),
        right.assets().collect::<Vec<_>>()
    );
    let connection = Connection::open(&path).unwrap();
    let remaining_assets: i64 = connection
        .query_row(
            "SELECT count(*) FROM installed_app_assets WHERE app_id = ?1",
            [left.manifest().app_id().as_str()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(remaining_assets, 0);
}

#[test]
fn interrupted_removal_rolls_back_the_asset_delete() {
    let (_fixture, path) = private_database();
    let app = package("calendar", b"calendar".to_vec());
    let mut store = InstalledAppStore::open(&path).unwrap();
    store.install(&app).unwrap();
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER refuse_removal BEFORE DELETE ON installed_apps
         WHEN OLD.app_id = 'calendar'
         BEGIN SELECT RAISE(ABORT, 'fixture interruption'); END;",
        )
        .unwrap();
    drop(connection);

    assert!(matches!(
        store.remove(app.manifest().app_id()),
        Err(StoreError::Database(_))
    ));
    drop(store);
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch("DROP TRIGGER refuse_removal")
        .unwrap();
    drop(connection);
    let mut store = InstalledAppStore::open(&path).unwrap();
    let retained = store.load(app.manifest().app_id()).unwrap().unwrap();
    assert_eq!(
        retained.assets().collect::<Vec<_>>(),
        app.assets().collect::<Vec<_>>()
    );
}

#[test]
fn inventory_distinguishes_empty_corrupt_and_unavailable_storage() {
    let (_fixture, path) = private_database();
    let app = package("calendar", b"calendar".to_vec());
    let mut store = InstalledAppStore::open(&path).unwrap();
    assert!(store.inventory().unwrap().is_empty());
    store.install(&app).unwrap();
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE installed_apps SET manifest = ?1 WHERE app_id = ?2",
            params![b"{}".as_slice(), app.manifest().app_id().as_str()],
        )
        .unwrap();
    assert!(matches!(
        store.inventory(),
        Err(StoreError::InvalidPackage(PackageError::MalformedManifest))
    ));

    connection
        .execute_batch("DROP TABLE installed_app_assets; DROP TABLE installed_apps;")
        .unwrap();
    assert!(matches!(store.inventory(), Err(StoreError::Database(_))));
}

#[test]
fn opening_a_missing_private_parent_returns_unavailable() {
    let fixture = tempfile::tempdir().unwrap();
    let path = fixture.path().join("missing").join("installed.sqlite3");
    assert!(matches!(
        InstalledAppStore::open(&path),
        Err(StoreError::Open(_))
    ));
    assert!(!path.exists());
}

#[test]
fn inventory_refuses_an_oversized_stored_app_identity() {
    let (_fixture, path) = private_database();
    let store = InstalledAppStore::open(&path).unwrap();
    let connection = Connection::open(&path).unwrap();
    let id = "a".repeat(nessa_types::app_identity::MAX_EXTENSION_APP_ID_BYTES + 1);
    connection
        .execute(
            "INSERT INTO installed_apps (app_id, manifest) VALUES (?1, ?2)",
            params![id, b"{}".as_slice()],
        )
        .unwrap();
    assert!(matches!(
        store.inventory(),
        Err(StoreError::InvalidStoredAppId)
    ));
}

#[test]
fn inventory_and_load_refuse_oversized_stored_manifests() {
    let (_fixture, path) = private_database();
    let mut store = InstalledAppStore::open(&path).unwrap();
    let id = ExtensionAppId::new("calendar").unwrap();
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "INSERT INTO installed_apps (app_id, manifest) VALUES (?1, zeroblob(?2))",
            params![
                id.as_str(),
                nessa_app_package::MAX_MANIFEST_BYTES as i64 + 1
            ],
        )
        .unwrap();
    assert!(matches!(
        store.inventory(),
        Err(StoreError::InvalidPackage(PackageError::ManifestTooLarge))
    ));
    assert!(matches!(
        store.load(&id),
        Err(StoreError::InvalidPackage(PackageError::ManifestTooLarge))
    ));
}

#[test]
fn refuses_a_same_version_unrelated_database_before_changing_journal_mode() {
    let (_fixture, path) = private_database();
    let _private =
        nessa_local_storage::open(&path, nessa_local_storage::OpenMode::OpenOrCreate).unwrap();
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE unrelated (value TEXT); PRAGMA user_version=1; PRAGMA journal_mode=WAL;",
        )
        .unwrap();
    drop(connection);

    assert!(matches!(
        InstalledAppStore::open(&path),
        Err(StoreError::Open(nessa_local_database::OpenError::Damaged(
            _
        )))
    ));
    let connection = Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .pragma_query_value(None, "journal_mode", |row| row.get::<_, String>(0))
            .unwrap(),
        "wal"
    );
}
