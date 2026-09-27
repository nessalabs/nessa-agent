//! Provider identity decoding and custom-storage admission keep bounded exact values.
use super::custom_storage::assert_custom_retention_admission;
use super::*;

#[tokio::test]
async fn provider_identity_file_limits_preserve_exact_values_and_rejected_bytes() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("private");
    private::create_directory(&directory).unwrap();
    let storage = LocalFileStorage::new(directory.clone()).unwrap();
    let mut value = snapshot("provider-identity");
    value.provider =
        ProviderIdentity::new("é".repeat(128), "é".repeat(128), "é".repeat(2048)).unwrap();
    let lease = storage.open(value.id.clone()).await.unwrap();
    lease.save(value.clone()).await.unwrap();
    assert_same(&lease.load().await.unwrap().unwrap(), &value);
    assert_custom_retention_admission(value, true).await;
    let path = journal_path(&directory, "provider-identity");
    let original: serde_json::Value = snapshot_json(&std::fs::read(&path).unwrap()).unwrap();
    for field in ["name", "model_id", "context"] {
        for replacement in [
            format!("{}x", original["provider"][field].as_str().unwrap()),
            "bad\0value".into(),
        ] {
            let mut invalid = original.clone();
            invalid["provider"][field] = replacement.into();
            let bytes = journal_bytes(&invalid).unwrap();
            std::fs::write(&path, &bytes).unwrap();
            assert!(
                matches!(lease.load().await, Err(StorageError::Corrupt(_))),
                "{field}"
            );
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
    }
}
