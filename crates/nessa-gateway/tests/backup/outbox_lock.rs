//! First-use ordering of the export reader and the outbox writer.

use std::fs::TryLockError;

use super::*;

#[test]
fn absent_parent_outbox_cut_excludes_first_writer() {
    for kind in [StoreKind::DeviceOutbox, StoreKind::EnvironmentOutbox] {
        let namespace = tempfile::tempdir().unwrap();
        let root = namespace.path().join("first-use/outbox");
        let reader = shared_outbox_lock(&root).unwrap();
        let path = root.with_extension("lock");
        assert!(path.is_file(), "an absent source still needs a stable lock");
        let writer = open(&path, OpenMode::OpenOrCreate).unwrap();
        assert!(matches!(writer.try_lock(), Err(TryLockError::WouldBlock)));
        assert!(!root.exists(), "locking must not create the outbox itself");
        let absent = read_outbox(kind, &root).unwrap();
        assert_eq!(absent.watermark().as_str(), "absent");
        assert!(absent.records().is_empty());
        drop(reader);

        writer.try_lock().unwrap();
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("row"), b"PARTIAL").unwrap();
        let contender = open(&path, OpenMode::OpenOrCreate).unwrap();
        assert!(matches!(
            contender.try_lock_shared(),
            Err(TryLockError::WouldBlock)
        ));
        fs::write(root.join("row"), b"COMPLETE").unwrap();
        drop(writer);
        let cut = read_outbox(kind, &root).unwrap();
        assert_eq!(cut.records().len(), 1);
        assert_eq!(
            cut.records()[0].public_bytes(),
            Some(b"COMPLETE".as_slice())
        );
    }
}

#[test]
fn an_outbox_lock_parent_that_is_a_file_is_refused() {
    let namespace = tempfile::tempdir().unwrap();
    let parent = namespace.path().join("environment");
    fs::write(&parent, b"not a directory").unwrap();
    assert!(shared_outbox_lock(&parent.join("outbox")).is_err());
}

#[cfg(unix)]
#[test]
fn an_outbox_lock_parent_symlink_is_refused() {
    let namespace = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let parent = namespace.path().join("environment");
    std::os::unix::fs::symlink(target.path(), &parent).unwrap();
    assert!(shared_outbox_lock(&parent.join("outbox")).is_err());
    assert!(!target.path().join("outbox.lock").exists());
}
