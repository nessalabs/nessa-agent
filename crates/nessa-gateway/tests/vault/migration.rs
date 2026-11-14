use crate::{
    secrets::{SecretStore, SecretUnavailable},
    vault::application::{verify_and_retire, Migration},
    vault::infrastructure::LegacyGithubMigration,
};
use nessa_vault::{
    application::{Clock, Vault},
    domain::{EntryName, Failure},
    infrastructure::{EncryptedFileVault, FileAudit, OsEntropy},
};
use std::io::Write;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
#[derive(Default)]
struct Old {
    value: Mutex<Option<Vec<u8>>>,
    reads: AtomicUsize,
}
impl SecretStore for Old {
    fn read(&self) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>, SecretUnavailable> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        Ok(self
            .value
            .lock()
            .unwrap()
            .clone()
            .map(zeroize::Zeroizing::new))
    }
    fn write(&self, bytes: &[u8]) -> Result<(), SecretUnavailable> {
        *self.value.lock().unwrap() = Some(bytes.to_vec());
        Ok(())
    }
    fn delete(&self) -> Result<(), SecretUnavailable> {
        *self.value.lock().unwrap() = None;
        Ok(())
    }
}
struct Time;
impl Clock for Time {
    fn seconds(&self) -> u64 {
        0
    }
}
fn vault(root: &std::path::Path) -> Arc<Vault> {
    Vault::new(
        Arc::new(EncryptedFileVault::new(
            root.join("vault"),
            Arc::new(OsEntropy),
        )),
        Arc::new(FileAudit::new(root.join("audit"), Arc::new(Time))),
        Arc::new(Time),
    )
}
fn password() -> nessa_vault::domain::Passphrase {
    nessa_vault::domain::Passphrase::new("a long disposable passphrase".into()).unwrap()
}
fn caller() -> nessa_vault::application::Caller {
    nessa_vault::application::Caller {
        principal: "owner".into(),
        credential: "panel".into(),
    }
}
#[test]
fn fresh_setup_and_subsequent_unlock_do_not_read_native_stores() {
    let root = tempfile::tempdir().unwrap();
    let old = Arc::new(Old::default());
    let migration =
        LegacyGithubMigration::new(root.path().to_path_buf(), Ok(old.clone()), Ok(old.clone()));
    let snapshot = migration.capture().unwrap();
    assert_eq!(old.reads.load(Ordering::SeqCst), 0);
    let vault = vault(root.path());
    let session = vault.session("first".into(), caller());
    vault
        .create(&session.lease(), password(), snapshot.entries())
        .unwrap();
    verify_and_retire(&vault, snapshot.as_ref()).unwrap();
    drop(snapshot);
    // These are now ordinary consumers' files, not proof of a pre-vault namespace.
    nessa_local_storage::create_directory(&root.path().join("github")).unwrap();
    nessa_local_storage::open(
        &root.path().join("github/credential-lock"),
        nessa_local_storage::OpenMode::CreateNew,
    )
    .unwrap();
    let again = migration.capture().unwrap();
    verify_and_retire(&vault, again.as_ref()).unwrap();
    assert_eq!(old.reads.load(Ordering::SeqCst), 0);
}
#[test]
fn existing_ciphertext_key_is_copied_verified_and_only_then_retired() {
    let root = tempfile::tempdir().unwrap();
    nessa_local_storage::create_directory(&root.path().join("github-relay")).unwrap();
    write_private(
        &root.path().join("github-relay/inbox.sqlite"),
        b"existing encrypted inbox bytes",
    );
    let old = Arc::new(Old::default());
    old.write(&[42; 32]).unwrap();
    let migration = LegacyGithubMigration::new(
        root.path().to_path_buf(),
        Err(SecretUnavailable::Unavailable),
        Ok(old.clone()),
    );
    let snapshot = migration.capture().unwrap();
    let vault = vault(root.path());
    let session = vault.session("import".into(), caller());
    vault
        .create(&session.lease(), password(), snapshot.entries())
        .unwrap();
    assert!(old.value.lock().unwrap().is_some());
    verify_and_retire(&vault, snapshot.as_ref()).unwrap();
    assert!(old.value.lock().unwrap().is_none());
    assert_eq!(
        vault
            .read(&EntryName::new("github-inbox-key").unwrap())
            .unwrap()
            .unwrap()
            .as_slice(),
        &[42; 32]
    );
}
#[test]
fn missing_required_key_or_unavailable_source_preserves_existing_data() {
    let root = tempfile::tempdir().unwrap();
    nessa_local_storage::create_directory(&root.path().join("github-relay")).unwrap();
    let path = root.path().join("github-relay/inbox.sqlite");
    write_private(&path, b"existing encrypted inbox bytes");
    let original = std::fs::read(&path).unwrap();
    let unavailable = LegacyGithubMigration::new(
        root.path().to_path_buf(),
        Err(SecretUnavailable::Unavailable),
        Err(SecretUnavailable::Unavailable),
    );
    assert!(matches!(
        unavailable.capture(),
        Err(Failure::MigrationUnavailable)
    ));
    assert_eq!(std::fs::read(&path).unwrap(), original);
    let empty = Arc::new(Old::default());
    let migration =
        LegacyGithubMigration::new(root.path().to_path_buf(), Ok(empty.clone()), Ok(empty));
    let snapshot = migration.capture().unwrap();
    let vault = vault(root.path());
    let session = vault.session("empty".into(), caller());
    vault.create(&session.lease(), password(), &[]).unwrap();
    assert_eq!(
        verify_and_retire(&vault, snapshot.as_ref()),
        Err(Failure::MigrationUnavailable)
    );
    assert_eq!(std::fs::read(&path).unwrap(), original);
}

fn write_private(path: &std::path::Path, bytes: &[u8]) {
    nessa_local_storage::open(path, nessa_local_storage::OpenMode::CreateNew)
        .unwrap()
        .write_all(bytes)
        .unwrap();
}

impl nessa_vault::application::ObservationClock for Time {
    fn unix_millis(&self) -> Result<u64, nessa_vault::domain::Failure> {
        Ok(1793030400000)
    }
}
