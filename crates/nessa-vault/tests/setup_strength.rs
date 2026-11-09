use nessa_vault::{
    application::Repository,
    domain::{Failure, Passphrase},
    infrastructure::{EncryptedFileVault, OsEntropy},
};
use std::{io::Write, sync::Arc};

#[test]
fn weak_new_passphrases_are_refused_without_creating_a_file() {
    for input in [
        "legacyphrase",        // Twelve characters remain valid unlock input.
        "123456789012345",     // Public common-password list.
        "qwertyuiopasdfghjkl", // Public common-password list.
        "repeatablerepeatablerepeatable",
        "               a",
        "İİİİİİİaunique", // Lowercasing must not inflate the character floor.
    ] {
        let root = tempfile::tempdir().unwrap();
        let repository = EncryptedFileVault::new(root.path().join("vault"), Arc::new(OsEntropy));
        let passphrase = Passphrase::new(input.to_owned()).unwrap();
        assert!(matches!(
            repository.create(&passphrase, &[]),
            Err(Failure::PassphrasePolicy)
        ));
        assert!(!root.path().join("vault/entries.enc").exists());
    }
}

#[test]
fn unique_new_passphrase_creates_and_unlocks_without_changing_its_bytes() {
    let root = tempfile::tempdir().unwrap();
    let repository = EncryptedFileVault::new(root.path().join("vault"), Arc::new(OsEntropy));
    let input = "  cobalt orchard seven lanterns riverside  ";
    repository
        .create(&Passphrase::new(input.into()).unwrap(), &[])
        .unwrap();
    assert!(repository
        .unlock(&Passphrase::new(input.into()).unwrap())
        .is_ok());
    assert!(matches!(
        repository.unlock(&Passphrase::new(input.trim().into()).unwrap()),
        Err(Failure::WrongPassphrase)
    ));
}

#[test]
fn legacy_weak_passphrase_still_unlocks() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("vault");
    nessa_local_storage::create_directory(&directory).unwrap();
    nessa_local_storage::open(
        &directory.join("entries.enc"),
        nessa_local_storage::OpenMode::CreateNew,
    )
    .unwrap()
    .write_all(include_bytes!("fixtures/legacy-weak.enc"))
    .unwrap();
    let repository = EncryptedFileVault::new(directory, Arc::new(OsEntropy));
    assert!(repository
        .unlock(&Passphrase::new("aaaaaaaaaaaa".into()).unwrap())
        .is_ok());
}
