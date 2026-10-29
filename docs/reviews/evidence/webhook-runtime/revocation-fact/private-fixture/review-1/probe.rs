
#[cfg(unix)]
#[test]
fn independent_shared_parent_stays_untrusted_and_unmodified() {
    use std::os::unix::fs::PermissionsExt;
    use nessa_auth::adapters::local::LocalStoreError;
    let root = tempfile::tempdir().unwrap();
    let before = std::fs::metadata(root.path()).unwrap().permissions().mode() & 0o777;
    assert_eq!(before, 0o755, "probe process runs under umask022");
    let error = match LocalCredentialStore::open(root.path(), "auth.json", Arc::new(CedarPolicyEvaluator::new().unwrap())) {
        Err(error) => error,
        Ok(_) => panic!("shared temporary root must remain refused"),
    };
    match error {
        LocalStoreError::Io(error) => assert!(nessa_local_storage::is_unsafe_file(&error)),
        _ => panic!("expected typed private-storage refusal"),
    }
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    let private = root.path().join("private");
    nessa_local_storage::create_directory(&private).unwrap();
    assert_eq!(std::fs::metadata(root.path()).unwrap().permissions().mode() & 0o777, before);
    assert_eq!(std::fs::metadata(&private).unwrap().permissions().mode() & 0o777, 0o700);
    let store = Fixture::open(&private);
    drop(store);
    assert_eq!(std::fs::metadata(root.path()).unwrap().permissions().mode() & 0o777, before);
    assert!(!root.path().join("auth.json").exists());
}
