use nessa_local_database::{open, open_existing, rusqlite, OpenError, Schema};
use std::path::PathBuf;

const OLD: &str = "CREATE TABLE kept (value TEXT NOT NULL) STRICT;
INSERT INTO kept VALUES ('original');
PRAGMA user_version = 2;";
const CURRENT: &str = "CREATE TABLE kept (value TEXT NOT NULL) STRICT;
CREATE TABLE added (value TEXT NOT NULL) STRICT;
PRAGMA user_version = 3;";

fn upgrade(transaction: &rusqlite::Transaction<'_>) -> rusqlite::Result<()> {
    transaction.execute_batch(
        "CREATE TABLE added (value TEXT NOT NULL) STRICT;
         INSERT INTO added SELECT value FROM kept;
         PRAGMA user_version = 3;",
    )
}

fn file() -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let private = directory.path().join("private");
    nessa_local_storage::create_directory(&private).unwrap();
    let path = private.join("upgrade.sqlite3");
    drop(open(&path, &Schema::new(OLD).unwrap()).unwrap());
    (directory, path)
}

fn value(connection: &rusqlite::Connection, table: &str) -> String {
    connection
        .query_row(&format!("SELECT value FROM {table}"), [], |row| row.get(0))
        .unwrap()
}

#[test]
fn the_explicit_predecessor_upgrades_once_and_preserves_existing_rows() {
    let (_directory, path) = file();
    let schema = Schema::new(CURRENT).unwrap().upgrading_from(2, upgrade);
    let connection = open_existing(&path, &schema).unwrap();
    assert_eq!(value(&connection, "kept"), "original");
    assert_eq!(value(&connection, "added"), "original");
    connection
        .execute("UPDATE added SET value = 'after'", [])
        .unwrap();
    drop(connection);
    let reopened = open(&path, &schema).unwrap();
    assert_eq!(value(&reopened, "added"), "after");
    drop(reopened);
    assert!(matches!(
        open(&path, &Schema::new(OLD).unwrap()),
        Err(OpenError::Version {
            found: 3,
            expected: 2
        })
    ));
}

#[test]
fn a_failed_upgrade_rolls_back_and_can_retry() {
    let (_directory, path) = file();
    let schema = Schema::new(CURRENT)
        .unwrap()
        .upgrading_from(2, |transaction| {
            upgrade(transaction)?;
            transaction.execute("UPDATE kept SET value = 'must roll back'", [])?;
            Err(rusqlite::Error::InvalidQuery)
        });
    assert!(matches!(
        open(&path, &schema),
        Err(OpenError::Database(rusqlite::Error::InvalidQuery))
    ));
    let old = open(&path, &Schema::new(OLD).unwrap()).unwrap();
    assert_eq!(value(&old, "kept"), "original");
    assert_eq!(
        old.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name = 'added'",
            [],
            |row| row.get::<_, u32>(0)
        )
        .unwrap(),
        0
    );
    drop(old);
    drop(
        open(
            &path,
            &Schema::new(CURRENT).unwrap().upgrading_from(2, upgrade),
        )
        .unwrap(),
    );
}

#[test]
fn an_upgrade_that_does_not_set_the_target_version_is_rolled_back() {
    let (_directory, path) = file();
    let schema = Schema::new(CURRENT)
        .unwrap()
        .upgrading_from(2, |transaction| {
            transaction.execute_batch("CREATE TABLE added (value TEXT NOT NULL) STRICT;")
        });
    assert!(matches!(
        open(&path, &schema),
        Err(OpenError::Version {
            found: 2,
            expected: 3
        })
    ));
    let old = open(&path, &Schema::new(OLD).unwrap()).unwrap();
    assert_eq!(
        old.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name = 'added'",
            [],
            |row| row.get::<_, u32>(0)
        )
        .unwrap(),
        0
    );
}

#[test]
fn empty_current_and_unknown_files_do_not_invoke_an_upgrade() {
    let (directory, path) = file();
    let schema = Schema::new(CURRENT)
        .unwrap()
        .upgrading_from(1, |_| panic!("not the predecessor"));
    assert!(matches!(
        open(&path, &schema),
        Err(OpenError::Version {
            found: 2,
            expected: 3
        })
    ));
    let empty = directory.path().join("private/empty.sqlite3");
    drop(open(&empty, &schema).unwrap());
    drop(open(&empty, &schema).unwrap());
    let downgrade = Schema::new(OLD)
        .unwrap()
        .upgrading_from(3, |_| panic!("no downgrade"));
    assert!(matches!(
        open(&empty, &downgrade),
        Err(OpenError::Version {
            found: 3,
            expected: 2
        })
    ));
}

#[test]
fn populated_unversioned_files_are_not_a_supported_predecessor() {
    let (_directory, path) = file();
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.pragma_update(None, "user_version", 0).unwrap();
    drop(connection);
    let schema = Schema::new(CURRENT).unwrap().upgrading_from(0, upgrade);
    assert!(matches!(
        open(&path, &schema),
        Err(OpenError::Version {
            found: 0,
            expected: 3
        })
    ));
    let connection = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(value(&connection, "kept"), "original");
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
            .unwrap(),
        0
    );
}

#[test]
fn concurrent_openers_recheck_the_version_under_the_write_lock() {
    let (_directory, path) = file();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let threads = (0..2)
        .map(|_| {
            let path = path.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let schema = Schema::new(CURRENT).unwrap().upgrading_from(2, upgrade);
                barrier.wait();
                let connection = open(&path, &schema).unwrap();
                assert_eq!(value(&connection, "added"), "original");
            })
        })
        .collect::<Vec<_>>();
    for thread in threads {
        thread.join().unwrap();
    }
}

#[test]
fn an_unwinding_upgrade_rolls_back_its_prepared_writes() {
    let (_directory, path) = file();
    let schema = Schema::new(CURRENT)
        .unwrap()
        .upgrading_from(2, |transaction| {
            upgrade(transaction)?;
            panic!("synthetic failure before commit");
        });
    assert!(std::panic::catch_unwind(|| open(&path, &schema)).is_err());
    let old = open(&path, &Schema::new(OLD).unwrap()).unwrap();
    assert_eq!(value(&old, "kept"), "original");
    assert_eq!(
        old.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name = 'added'",
            [],
            |row| row.get::<_, u32>(0)
        )
        .unwrap(),
        0
    );
}

#[test]
fn predecessor_integrity_is_checked_before_the_upgrade_runs() {
    let (_directory, path) = file();
    let connection = open(&path, &Schema::new(OLD).unwrap()).unwrap();
    let (size, page): (u32, u32) = connection.query_row(
        "SELECT (SELECT page_size FROM pragma_page_size), (SELECT rootpage FROM sqlite_schema WHERE name = 'kept')", [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    ).unwrap();
    drop(connection);
    let mut bytes = std::fs::read(&path).unwrap();
    let start = ((page - 1) * size) as usize;
    bytes[start..start + size as usize].fill(0xA5);
    std::fs::write(&path, &bytes).unwrap();
    let schema = Schema::new(CURRENT)
        .unwrap()
        .upgrading_from(2, |_| panic!("damaged predecessor must not run upgrade"));
    assert!(matches!(
        open(&path, &schema),
        Err(OpenError::Damaged(_) | OpenError::Unreadable(_))
    ));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

/// Invoked only by the process-loss fixture, on its isolated private database.
#[test]
fn synthetic_upgrade_process() {
    let Some(path) = std::env::var_os("NESSA_SYNTHETIC_UPGRADE_DATABASE") else {
        return;
    };
    let schema = Schema::new(CURRENT)
        .unwrap()
        .upgrading_from(2, |transaction| {
            upgrade(transaction)?;
            if std::env::var("NESSA_SYNTHETIC_UPGRADE_PHASE").as_deref() == Ok("before-commit") {
                // Skip Rust destructors: recovery must use SQLite's hot journal,
                // not Transaction::drop in this process.
                std::process::exit(17);
            }
            Ok(())
        });
    let _connection = open(std::path::Path::new(&path), &schema).unwrap();
    std::process::exit(18);
}

#[test]
fn process_loss_before_or_after_upgrade_commit_keeps_one_complete_version() {
    for (phase, code) in [("before-commit", 17), ("after-commit", 18)] {
        let (_directory, path) = file();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "synthetic_upgrade_process", "--nocapture"])
            .env("NESSA_SYNTHETIC_UPGRADE_DATABASE", &path)
            .env("NESSA_SYNTHETIC_UPGRADE_PHASE", phase)
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(code), "{result:?}");
        if phase == "before-commit" {
            let old = open(&path, &Schema::new(OLD).unwrap()).unwrap();
            assert_eq!(value(&old, "kept"), "original");
            assert_eq!(
                old.query_row(
                    "SELECT count(*) FROM sqlite_schema WHERE name = 'added'",
                    [],
                    |row| row.get::<_, u32>(0)
                )
                .unwrap(),
                0
            );
        } else {
            assert!(matches!(
                open(&path, &Schema::new(OLD).unwrap()),
                Err(OpenError::Version {
                    found: 3,
                    expected: 2
                })
            ));
        }
        let current = open(
            &path,
            &Schema::new(CURRENT).unwrap().upgrading_from(2, upgrade),
        )
        .unwrap();
        assert_eq!(value(&current, "kept"), "original");
        assert_eq!(value(&current, "added"), "original");
    }
}
