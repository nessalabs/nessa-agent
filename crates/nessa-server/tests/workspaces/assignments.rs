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
        admission: super::allocation_tests::admission(),
    }
}

#[tokio::test]
async fn lifetime_refusal_keeps_the_actual_vm_target_after_reopen() {
    use crate::workspaces::domain::{AdmissionRefusal, AdmissionRejection, RefusalDisposition};
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("receipts");
    let store = FileAssignments(root.clone());
    store.save(intent()).await.unwrap();
    let refused = Assignment::Refused {
        thread: THREAD.into(),
        vm: "bx_fixture".into(),
        image: "fixture-image".into(),
        admission: super::allocation_tests::admission(),
        rejection: AdmissionRejection::new(AdmissionRefusal::LifetimeNotEnforced, None),
        disposition: RefusalDisposition::RollbackNewVm,
    };
    store.save(refused.clone()).await.unwrap();
    let reopened = FileAssignments(root.clone());
    assert_eq!(reopened.read(THREAD).await.unwrap(), Some(refused));
    let history: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join(format!("{THREAD}.json"))).unwrap())
            .unwrap();
    assert_eq!(
        history["transitions"][1]["rejection"]["cause"],
        "lifetime_not_enforced"
    );
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

#[tokio::test]
async fn receipt_publication_obeys_the_same_byte_bound_as_reopening() {
    for fill in ["x", "é"] {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("receipts");
        let store = FileAssignments(root.clone());
        store.save(intent()).await.unwrap();
        let mut oversized = receipt();
        if let Assignment::Assigned { vm, .. } = &mut oversized {
            *vm = fill.repeat(8193 / fill.len() + 1);
        }
        assert_eq!(store.save(oversized).await, Err(Failure::Storage));
        assert_eq!(store.read(THREAD).await.unwrap(), Some(intent()));
        store.save(receipt()).await.unwrap();
        let path = root.join(format!("{THREAD}.json"));
        let ordinary = std::fs::metadata(&path).unwrap().len() as usize;
        // Exactly the reader's bound is valid, including serialization overhead.
        let other = temporary.path().join("exact");
        let exact = FileAssignments(other.clone());
        exact.save(intent()).await.unwrap();
        let mut boundary = receipt();
        if let Assignment::Assigned { vm, .. } = &mut boundary {
            *vm = "x".repeat(8192 - ordinary + vm.len());
        }
        exact.save(boundary.clone()).await.unwrap();
        assert_eq!(
            std::fs::metadata(other.join(format!("{THREAD}.json")))
                .unwrap()
                .len(),
            8192
        );
        assert_eq!(exact.read(THREAD).await.unwrap(), Some(boundary));
    }
}

#[tokio::test]
async fn old_unverified_receipts_keep_the_target_without_an_upgrade_or_rewrite() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("receipts");
    let store = FileAssignments(root.clone());
    store.save(intent()).await.unwrap();
    let path = root.join(format!("{THREAD}.json"));
    let old = serde_json::json!({"cause":"thread_lease_requested", "transitions":[{"state":"provisioning","thread":THREAD},{"state":"assigned","thread":THREAD,"vm":"bx_fixture","image":"fixture-image","environment":"nessa"}]});
    let bytes = serde_json::to_vec(&old).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    let retained = store.read(THREAD).await.unwrap().unwrap();
    assert!(matches!(&retained, Assignment::Unverified {vm,..} if vm=="bx_fixture"));
    assert!(store.save(retained).await.is_err());
    assert_eq!(store.save(receipt()).await, Err(Failure::Storage));
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

#[tokio::test]
async fn refusal_is_terminal_and_a_restored_history_cannot_switch_vm_or_admission() {
    use crate::workspaces::domain::{AdmissionRefusal, RefusalDisposition};
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("receipts");
    let store = FileAssignments(root.clone());
    store.save(intent()).await.unwrap();
    store.save(receipt()).await.unwrap();
    let refused = Assignment::Refused {
        thread: THREAD.into(),
        vm: "bx_fixture".into(),
        image: "fixture-image".into(),
        admission: super::allocation_tests::admission(),
        disposition: RefusalDisposition::RollbackNewVm,
        rejection: crate::workspaces::domain::AdmissionRejection::new(
            AdmissionRefusal::WrongVersion,
            None,
        ),
    };
    store.save(refused.clone()).await.unwrap();
    let reopened = FileAssignments(root.clone());
    assert_eq!(reopened.read(THREAD).await.unwrap(), Some(refused.clone()));
    assert_eq!(reopened.save(receipt()).await, Err(Failure::Storage));
    let mut revised = refused;
    if let Assignment::Refused { rejection, .. } = &mut revised {
        *rejection = crate::workspaces::domain::AdmissionRejection::new(
            AdmissionRefusal::EnvironmentChanged,
            None,
        );
    }
    assert_eq!(reopened.save(revised).await, Err(Failure::Storage));
    let path = root.join(format!("{THREAD}.json"));
    let original = std::fs::read(&path).unwrap();
    for field in ["vm", "image", "environment"] {
        let mut history: serde_json::Value = serde_json::from_slice(&original).unwrap();
        history["transitions"][2][field] = serde_json::json!("other");
        std::fs::write(&path, serde_json::to_vec(&history).unwrap()).unwrap();
        assert_eq!(reopened.read(THREAD).await, Err(Failure::Storage));
    }
}
