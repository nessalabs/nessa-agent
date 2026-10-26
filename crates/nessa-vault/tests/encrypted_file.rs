use nessa_vault::{
    application::Repository,
    domain::{EntryName, Failure, Passphrase},
    infrastructure::{EncryptedFileVault, OsEntropy},
};
use std::sync::Arc;
fn password() -> Passphrase {
    Passphrase::new("several words protect this local vault".into()).unwrap()
}
#[test]
fn corruption_and_a_second_setup_preserve_the_committed_vault() {
    let root = tempfile::tempdir().unwrap();
    let repository = EncryptedFileVault::new(root.path().join("vault"), Arc::new(OsEntropy));
    let key = repository.create(&password(), &[]).unwrap();
    let name = EntryName::new("inbox-key").unwrap();
    repository.write(&key, &name, b"secret").unwrap();
    let file = root.path().join("vault/entries.enc");
    let original = std::fs::read(&file).unwrap();
    assert!(matches!(
        repository.create(&password(), &[]),
        Err(Failure::AlreadyConfigured)
    ));
    assert_eq!(std::fs::read(&file).unwrap(), original);
    let mut damaged = original;
    *damaged.last_mut().unwrap() ^= 1;
    std::fs::write(&file, &damaged).unwrap();
    assert!(matches!(
        repository.unlock(&password()),
        Err(Failure::WrongPassphrase)
    ));
    assert_eq!(std::fs::read(&file).unwrap(), damaged);
    std::fs::write(&file, b"NESSAVAULT01").unwrap();
    assert!(matches!(
        repository.unlock(&password()),
        Err(Failure::Unavailable)
    ));
    assert_eq!(std::fs::read(&file).unwrap(), b"NESSAVAULT01");
}
#[test]
fn independent_repository_writers_keep_each_others_entries() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("vault");
    let repository = EncryptedFileVault::new(path.clone(), Arc::new(OsEntropy));
    repository.create(&password(), &[]).unwrap();
    let workers: Vec<_> = (0..4)
        .map(|i| {
            let path = path.clone();
            std::thread::spawn(move || {
                let repo = EncryptedFileVault::new(path, Arc::new(OsEntropy));
                let key = repo.unlock(&password()).unwrap();
                repo.write(
                    &key,
                    &EntryName::new(format!("entry-{i}")).unwrap(),
                    format!("secret-{i}").as_bytes(),
                )
                .unwrap();
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }
    let key = repository.unlock(&password()).unwrap();
    for i in 0..4 {
        assert_eq!(
            repository
                .read(&key, &EntryName::new(format!("entry-{i}")).unwrap())
                .unwrap()
                .unwrap()
                .as_slice(),
            format!("secret-{i}").as_bytes()
        );
    }
}
#[test]
fn entry_and_passphrase_limits_are_enforced_before_storage() {
    assert!(matches!(
        Passphrase::new("sixsix".into()),
        Err(Failure::PassphrasePolicy)
    ));
    assert!(matches!(
        Passphrase::new(" ".repeat(12)),
        Err(Failure::PassphrasePolicy)
    ));
    assert!(Passphrase::new("ä".repeat(12)).is_ok());
    assert!(matches!(
        Passphrase::new("a".repeat(1025)),
        Err(Failure::PassphrasePolicy)
    ));
    assert!(matches!(
        EntryName::new("../outside"),
        Err(Failure::InvalidEntry)
    ));
    let root = tempfile::tempdir().unwrap();
    let repository = EncryptedFileVault::new(root.path().join("vault"), Arc::new(OsEntropy));
    let key = repository.create(&password(), &[]).unwrap();
    assert_eq!(
        repository.write(&key, &EntryName::new("entry").unwrap(), &vec![0; 16385]),
        Err(Failure::Capacity)
    );
    for i in 0..32 {
        repository
            .write(
                &key,
                &EntryName::new(format!("entry-{i}")).unwrap(),
                b"test",
            )
            .unwrap();
    }
    assert_eq!(
        repository.write(&key, &EntryName::new("too-many").unwrap(), b"test"),
        Err(Failure::Capacity)
    );
}
