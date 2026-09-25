use nessa_local_database::{open, OpenError, Schema};
use std::path::PathBuf;

const DEFINITION: &str = "CREATE TABLE parents (id TEXT PRIMARY KEY) STRICT;
CREATE TABLE children (parent TEXT NOT NULL REFERENCES parents(id), note TEXT) STRICT;
PRAGMA user_version = 3;
";

fn schema() -> Schema {
    Schema::new(DEFINITION).unwrap()
}

fn private_directory() -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("private");
    nessa_local_storage::create_directory(&root).unwrap();
    (directory, root)
}

#[test]
fn an_empty_file_is_given_the_schema_at_its_version_and_reopened_as_it_is() {
    let (_directory, root) = private_directory();
    let path = root.join("store.sqlite3");
    let connection = open(&path, &schema()).unwrap();
    let version: u32 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 3);
    connection
        .execute("INSERT INTO parents (id) VALUES ('kept')", [])
        .unwrap();
    drop(connection);
    let reopened = open(&path, &schema()).unwrap();
    let kept: String = reopened
        .query_row("SELECT id FROM parents", [], |row| row.get(0))
        .unwrap();
    assert_eq!(kept, "kept");
}

#[test]
fn another_version_is_refused_and_left_as_it_was() {
    let (_directory, root) = private_directory();
    let path = root.join("store.sqlite3");
    drop(open(&path, &schema()).unwrap());
    let later =
        Schema::new("CREATE TABLE other (id TEXT) STRICT;\nPRAGMA user_version = 4;").unwrap();
    assert!(matches!(
        open(&path, &later),
        Err(OpenError::Version {
            found: 3,
            expected: 4
        })
    ));
    // Untouched: the version it had, and none of the refused schema.
    let connection = open(&path, &schema()).unwrap();
    let other: u32 = connection
        .query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name = 'other'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(other, 0);
}

#[test]
fn a_negative_version_is_another_version() {
    let (_directory, root) = private_directory();
    let path = root.join("store.sqlite3");
    drop(open(&path, &schema()).unwrap());
    rusqlite_connection(&path)
        .pragma_update(None, "user_version", -1)
        .unwrap();
    assert!(matches!(
        open(&path, &schema()),
        Err(OpenError::Version {
            found: -1,
            expected: 3
        })
    ));
}

#[test]
fn tables_without_a_version_are_refused_rather_than_given_a_schema() {
    let (_directory, root) = private_directory();
    let path = root.join("store.sqlite3");
    drop(nessa_local_storage::open(&path, nessa_local_storage::OpenMode::CreateNew).unwrap());
    rusqlite_connection(&path)
        .execute_batch("CREATE TABLE stray (id TEXT);")
        .unwrap();
    assert!(matches!(
        open(&path, &schema()),
        Err(OpenError::Version {
            found: 0,
            expected: 3
        })
    ));
}

#[test]
fn a_schema_states_its_version_once_in_its_definition() {
    assert_eq!(schema().version(), 3);
    for definition in [
        "CREATE TABLE t (id TEXT) STRICT;",
        "CREATE TABLE t (id TEXT) STRICT;\nPRAGMA user_version = 0;",
        "CREATE TABLE t (id TEXT) STRICT;\nPRAGMA user_version = three;",
        "PRAGMA user_version = 1;\nPRAGMA user_version = 2;",
        "CREATE TABLE t (id TEXT) STRICT;\nPRAGMA user_version = +1;",
        // Only ASCII spacing around the line.
        "CREATE TABLE t (id TEXT) STRICT;\n\u{85}PRAGMA user_version = 1;",
        "CREATE TABLE t (id TEXT) STRICT;\n\u{feff}PRAGMA user_version = 1;",
    ] {
        assert!(
            matches!(Schema::new(definition), Err(OpenError::UnversionedSchema)),
            "{definition}"
        );
    }
}

#[test]
fn a_definition_that_does_not_set_the_version_it_states_is_refused_and_not_kept() {
    let (_directory, root) = private_directory();
    let path = root.join("store.sqlite3");
    // Stated on a line of its own, but inside a comment SQLite never runs.
    let commented =
        Schema::new("CREATE TABLE t (id TEXT) STRICT;\n/*\nPRAGMA user_version = 5;\n*/").unwrap();
    assert!(matches!(
        open(&path, &commented),
        Err(OpenError::Version {
            found: 0,
            expected: 5
        })
    ));
    // Nothing of it was kept: the file is still empty and takes a schema.
    assert_eq!(
        open(&path, &schema())
            .unwrap()
            .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
            .unwrap(),
        3
    );
}

#[test]
fn every_connection_enforces_keys_and_overwrites_what_it_deletes() {
    let (_directory, root) = private_directory();
    let path = root.join("store.sqlite3");
    drop(open(&path, &schema()).unwrap());
    let connection = open(&path, &schema()).unwrap();
    assert!(connection
        .execute(
            "INSERT INTO children (parent, note) VALUES ('nobody', 'x')",
            []
        )
        .is_err());
    let secure_delete: i64 = connection
        .pragma_query_value(None, "secure_delete", |row| row.get(0))
        .unwrap();
    assert_eq!(secure_delete, 1);
    let journal: String = connection
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .unwrap();
    assert_eq!(journal, "delete");
    let synchronous: i64 = connection
        .pragma_query_value(None, "synchronous", |row| row.get(0))
        .unwrap();
    assert_eq!(synchronous, 2);
    let fullfsync: i64 = connection
        .pragma_query_value(None, "fullfsync", |row| row.get(0))
        .unwrap();
    assert_eq!(fullfsync, 1);
    // What was deleted is not left readable in the file.
    let marker = "a-summary-somebody-deleted";
    connection
        .execute("INSERT INTO parents (id) VALUES (?1)", [marker])
        .unwrap();
    connection
        .execute("DELETE FROM parents WHERE id = ?1", [marker])
        .unwrap();
    drop(connection);
    let bytes = std::fs::read(&path).unwrap();
    assert!(!bytes
        .windows(marker.len())
        .any(|window| window == marker.as_bytes()));
}

#[test]
fn a_file_that_is_not_a_database_is_refused_and_left_as_it_was() {
    let (_directory, root) = private_directory();
    let path = root.join("store.sqlite3");
    let mut file =
        nessa_local_storage::open(&path, nessa_local_storage::OpenMode::CreateNew).unwrap();
    std::io::Write::write_all(&mut file, &[7; 4096]).unwrap();
    drop(file);
    assert!(matches!(
        open(&path, &schema()),
        Err(OpenError::Database(_))
    ));
    assert_eq!(std::fs::read(&path).unwrap(), [7; 4096]);
}

#[test]
fn a_file_left_in_write_ahead_mode_is_opened_in_the_rollback_journal() {
    let (_directory, root) = private_directory();
    let path = root.join("store.sqlite3");
    drop(open(&path, &schema()).unwrap());
    let other = rusqlite_connection(&path);
    let _: String = other
        .pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))
        .unwrap();
    drop(other);
    let connection = open(&path, &schema()).unwrap();
    let journal: String = connection
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .unwrap();
    assert_eq!(journal, "delete");
}

#[cfg(unix)]
#[test]
fn the_file_is_created_private_before_sqlite_opens_it() {
    use std::os::unix::fs::PermissionsExt;
    let (_directory, root) = private_directory();
    let path = root.join("store.sqlite3");
    drop(open(&path, &schema()).unwrap());
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    // Never through a link, and never in a directory others can reach.
    let linked = root.join("linked.sqlite3");
    std::os::unix::fs::symlink(&path, &linked).unwrap();
    assert!(matches!(open(&linked, &schema()), Err(OpenError::File(_))));
    let shared = _directory.path().join("shared");
    std::fs::create_dir(&shared).unwrap();
    std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(
        open(&shared.join("store.sqlite3"), &schema()),
        Err(OpenError::Directory(_))
    ));
}

fn rusqlite_connection(path: &std::path::Path) -> nessa_local_database::rusqlite::Connection {
    nessa_local_database::rusqlite::Connection::open(path).unwrap()
}
