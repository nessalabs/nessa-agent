//! Receipt persistence exercises the actual private directory boundary.
use crate::workspaces::{
    application::Assignments,
    domain::{Assignment, Failure},
    infrastructure::FileAssignments,
};
use std::os::unix::fs::PermissionsExt;

const THREAD: &str = "d49640a4-fd71-40a4-b3e9-a35e8753db78";
fn intent() -> Assignment {
    Assignment::Provisioning {
        thread: THREAD.into(),
    }
}
fn receipt() -> Assignment {
    Assignment::Assigned {
        thread: THREAD.into(),
        vm: "bx_fixture".into(),
        image: "fixture-image".into(),
        environment: "nessa".into(),
    }
}

#[tokio::test]
async fn reopening_keeps_both_intent_and_received_vm_in_private_storage() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("receipts");
    let store = FileAssignments(root.clone());
    assert_eq!(store.read(THREAD).await.unwrap(), None);
    store.save(intent()).await.unwrap();
    store.save(intent()).await.unwrap();
    store.save(receipt()).await.unwrap();
    store.save(receipt()).await.unwrap();
    let reopened = FileAssignments(root.clone());
    assert_eq!(reopened.read(THREAD).await.unwrap(), Some(receipt()));
    let path = root.join(format!("{THREAD}.json"));
    let history: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(history["transitions"].as_array().unwrap().len(), 2);
    assert_eq!(
        std::fs::metadata(root).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(reopened.save(intent()).await, Err(Failure::Storage));
}

#[tokio::test]
async fn missing_intent_different_vm_and_path_traversal_cannot_replace_a_receipt() {
    let temporary = tempfile::tempdir().unwrap();
    let store = FileAssignments(temporary.path().join("receipts"));
    assert_eq!(store.save(receipt()).await, Err(Failure::Storage));
    assert_eq!(store.read("../other").await, Err(Failure::InvalidInput));
    store.save(intent()).await.unwrap();
    store.save(receipt()).await.unwrap();
    let mut different = receipt();
    if let Assignment::Assigned { vm, .. } = &mut different {
        *vm = "bx_other".into();
    }
    assert_eq!(store.save(different).await, Err(Failure::Storage));
    assert_eq!(store.read(THREAD).await.unwrap(), Some(receipt()));
}

#[tokio::test]
async fn damaged_history_never_becomes_permission_to_allocate_again() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("receipts");
    let store = FileAssignments(root.clone());
    store.save(intent()).await.unwrap();
    let path = root.join(format!("{THREAD}.json"));
    for bytes in [
        b"{".as_slice(),
        b"{\"cause\":\"unknown\",\"transitions\":[]}".as_slice(),
        &[b'x'; 8193],
    ] {
        std::fs::write(&path, bytes).unwrap();
        assert_eq!(store.read(THREAD).await, Err(Failure::Storage));
        assert_eq!(store.save(intent()).await, Err(Failure::Storage));
    }
}

#[tokio::test]
async fn linked_receipt_is_refused_without_reading_or_overwriting_its_target() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("receipts");
    let store = FileAssignments(root.clone());
    store.save(intent()).await.unwrap();
    let path = root.join(format!("{THREAD}.json"));
    std::fs::remove_file(&path).unwrap();
    let target = temporary.path().join("unrelated");
    std::fs::write(&target, "preserve").unwrap();
    std::os::unix::fs::symlink(&target, &path).unwrap();
    assert_eq!(store.read(THREAD).await, Err(Failure::Storage));
    assert_eq!(store.save(intent()).await, Err(Failure::Storage));
    assert_eq!(std::fs::read_to_string(target).unwrap(), "preserve");
}
