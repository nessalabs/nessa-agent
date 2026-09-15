//! Malformed identity histories must not replace valid committed evidence.
use super::*;

async fn reject_duplicate_ids(storage: &dyn SessionStorage) {
    let lease = storage.open(id("duplicate-probe")).await.unwrap();
    let original = snapshot("duplicate-probe");
    lease.save(original.clone()).await.unwrap();
    let mut invalid = original.clone();
    let mut duplicate = invalid.invocations[0].clone();
    duplicate.request.user_message = PromptText::new("different logical input").unwrap();
    invalid.invocations.push(duplicate);
    assert!(matches!(
        lease.save(invalid).await,
        Err(StorageError::Corrupt(_))
    ));
    assert_same(&lease.load().await.unwrap().unwrap(), &original);
}

#[tokio::test]
async fn robustness_duplicate_execution_ids_are_rejected_by_memory_storage() {
    reject_duplicate_ids(&InMemoryStorage::new()).await;
}

#[tokio::test]
async fn robustness_duplicate_execution_ids_are_rejected_by_file_storage() {
    let root = tempfile::tempdir().unwrap();
    private::create_directory(&root.path().join("private")).unwrap();
    reject_duplicate_ids(&LocalFileStorage::new(root.path().join("private")).unwrap()).await;
}

#[tokio::test]
async fn robustness_loading_duplicate_ids_preserves_corrupt_file_for_inspection() {
    let root = tempfile::tempdir().unwrap();
    private::create_directory(&root.path().join("private")).unwrap();
    let storage = LocalFileStorage::new(root.path().join("private")).unwrap();
    let lease = storage.open(id("duplicate-probe")).await.unwrap();
    lease.save(snapshot("duplicate-probe")).await.unwrap();
    let path = journal_path(&root.path().join("private"), "duplicate-probe");
    let mut json: serde_json::Value = snapshot_json(&std::fs::read(&path).unwrap()).unwrap();
    let duplicate = json["invocations"][0].clone();
    json["invocations"].as_array_mut().unwrap().push(duplicate);
    let corrupt = journal_bytes(&json).unwrap();
    std::fs::write(&path, &corrupt).unwrap();
    assert!(matches!(lease.load().await, Err(StorageError::Corrupt(_))));
    assert_eq!(std::fs::read(path).unwrap(), corrupt);
}
