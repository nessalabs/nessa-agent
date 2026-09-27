//! Erasing a session's history happens under the lease that erases it, and
//! never opens a way to a second writer.
#[cfg(unix)]
use super::lock_path;
use super::{assert_same, id, journal_path, snapshot, SessionStorage, StorageError};
use nessa_local_storage as private;
use nessa_sdk::infrastructure::session_storage::{InMemoryStorage, LocalFileStorage};
use std::sync::Arc;

/// Both supplied adapters, each with a second handle onto the same sessions:
/// a clone for memory storage, an independent instance for files.
fn adapters(root: &std::path::Path) -> Vec<(Arc<dyn SessionStorage>, Arc<dyn SessionStorage>)> {
    private::create_directory(&root.join("private")).unwrap();
    let memory = InMemoryStorage::new();
    vec![
        (Arc::new(memory.clone()), Arc::new(memory)),
        (
            Arc::new(LocalFileStorage::new(root.join("private")).unwrap()),
            Arc::new(LocalFileStorage::new(root.join("private")).unwrap()),
        ),
    ]
}

#[tokio::test]
async fn erase_removes_history_under_the_held_lease_in_both_adapters() {
    let root = tempfile::tempdir().unwrap();
    for (storage, other) in adapters(root.path()) {
        let lease = storage.open(id("erased")).await.unwrap();
        lease.save(snapshot("erased")).await.unwrap();
        let kept = storage.open(id("kept")).await.unwrap();
        kept.save(snapshot("kept")).await.unwrap();

        lease.erase().await.unwrap();
        assert!(lease.load().await.unwrap().is_none());
        // Erasing what is already gone succeeds and changes nothing.
        lease.erase().await.unwrap();
        // Another session's history is not touched.
        assert_same(&kept.load().await.unwrap().unwrap(), &snapshot("kept"));

        // Still one writer: the erasing lease keeps the session until dropped,
        // through this handle and through another onto the same storage.
        assert!(matches!(
            storage.open(id("erased")).await,
            Err(StorageError::Busy)
        ));
        assert!(matches!(
            other.open(id("erased")).await,
            Err(StorageError::Busy)
        ));
        drop(lease);

        // The next owner finds no history, and may begin a new one.
        let reopened = other.open(id("erased")).await.unwrap();
        assert!(reopened.load().await.unwrap().is_none());
        assert!(matches!(
            storage.open(id("erased")).await,
            Err(StorageError::Busy)
        ));
        reopened.save(snapshot("erased")).await.unwrap();
        assert_same(
            &reopened.load().await.unwrap().unwrap(),
            &snapshot("erased"),
        );
    }
}

#[tokio::test]
async fn erasing_a_session_that_was_never_saved_succeeds() {
    let root = tempfile::tempdir().unwrap();
    for (storage, _) in adapters(root.path()) {
        let lease = storage.open(id("empty")).await.unwrap();
        lease.erase().await.unwrap();
        assert!(lease.load().await.unwrap().is_none());
    }
}

#[tokio::test]
async fn file_erase_removes_the_journal_and_keeps_the_lock_that_excludes_writers() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("private");
    let storage = LocalFileStorage::new(&directory).unwrap();
    let lease = storage.open(id("files")).await.unwrap();
    lease.save(snapshot("files")).await.unwrap();
    assert!(journal_path(&directory, "files").exists());

    lease.erase().await.unwrap();
    assert!(!journal_path(&directory, "files").exists());
    // The lock file is what the held lease has locked. Were it unlinked, a
    // second opener would create and lock a new one beside it.
    #[cfg(unix)]
    assert!(lock_path(&directory, "files").exists());
    assert!(matches!(
        LocalFileStorage::new(&directory)
            .unwrap()
            .open(id("files"))
            .await,
        Err(StorageError::Busy)
    ));

    // A save through the same lease after an erase begins a new history, and
    // does not resurrect the erased one.
    let mut fresh = snapshot("files");
    fresh.invocations.clear();
    lease.save(fresh.clone()).await.unwrap();
    assert_same(&lease.load().await.unwrap().unwrap(), &fresh);
}

// Unix only: the failure is made by moving the storage directory away while
// its lease is held, and Windows refuses to rename a directory with an open
// file in it.
#[cfg(unix)]
#[tokio::test]
async fn a_failed_file_erase_is_typed_and_leaves_the_history_whole() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("private");
    let storage = LocalFileStorage::new(&directory).unwrap();
    let lease = storage.open(id("stays")).await.unwrap();
    lease.save(snapshot("stays")).await.unwrap();
    // A directory that is no longer the private one this storage verified.
    let moved = root.path().join("moved");
    std::fs::rename(&directory, &moved).unwrap();
    assert!(matches!(lease.erase().await, Err(StorageError::Io(_))));
    std::fs::rename(&moved, &directory).unwrap();
    assert_same(&lease.load().await.unwrap().unwrap(), &snapshot("stays"));
    // Repeating the erase under the lease completes it.
    lease.erase().await.unwrap();
    assert!(lease.load().await.unwrap().is_none());
}

#[tokio::test]
async fn opening_an_existing_session_creates_nothing_for_one_that_never_was() {
    let root = tempfile::tempdir().unwrap();
    for (storage, other) in adapters(root.path()) {
        assert!(storage.open_existing(id("never")).await.unwrap().is_none());
        // Asking created nothing: a later ask still finds nothing.
        assert!(other.open_existing(id("never")).await.unwrap().is_none());

        drop(storage.open(id("opened")).await.unwrap());
        let lease = other
            .open_existing(id("opened"))
            .await
            .unwrap()
            .expect("a session that was opened exists");
        // It is the same exclusive lease an opening takes.
        assert!(matches!(
            storage.open_existing(id("opened")).await,
            Err(StorageError::Busy)
        ));
        assert!(matches!(
            storage.open(id("opened")).await,
            Err(StorageError::Busy)
        ));
        drop(lease);
    }
    #[cfg(unix)]
    assert!(!lock_path(&root.path().join("private"), "never").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn a_journal_whose_lock_is_gone_still_exists() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("private");
    let storage = LocalFileStorage::new(&directory).unwrap();
    let lease = storage.open(id("kept")).await.unwrap();
    lease.save(snapshot("kept")).await.unwrap();
    drop(lease);
    // Somebody removed the lease file and left the history.
    std::fs::remove_file(lock_path(&directory, "kept")).unwrap();
    let lease = storage
        .open_existing(id("kept"))
        .await
        .unwrap()
        .expect("a session with history exists whatever became of its lock");
    assert_same(&lease.load().await.unwrap().unwrap(), &snapshot("kept"));
    // And it is exclusive as any lease is.
    assert!(matches!(
        storage.open_existing(id("kept")).await,
        Err(StorageError::Busy)
    ));
}
