use crate::{
    github_relay::{application::InboxKeySource, domain::Failure},
    secrets::SecretStore,
};
use nessa_local_database::DatabaseKey;
use std::{io::Read, path::PathBuf, sync::Arc};

/// The inbox key has a different lifetime from GitHub sign-in. A process-shared
/// lock prevents two starters from overwriting each other's newly minted key.
pub struct VaultInboxKey {
    path: PathBuf,
    secret: Arc<dyn SecretStore>,
}
impl VaultInboxKey {
    pub fn new(path: PathBuf, secret: Arc<dyn SecretStore>) -> Self {
        Self { path, secret }
    }
}
impl InboxKeySource for VaultInboxKey {
    fn load(&self) -> Result<Arc<DatabaseKey>, Failure> {
        let parent = self.path.parent().ok_or(Failure::Storage)?;
        nessa_local_storage::create_directory(parent).map_err(|_| Failure::Storage)?;
        let lock = nessa_local_storage::open(
            &self.path.with_extension("key-lock"),
            nessa_local_storage::OpenMode::OpenOrCreate,
        )
        .map_err(|_| Failure::Storage)?;
        lock.lock().map_err(|_| Failure::Storage)?;
        let bytes = match self.secret.read().map_err(secret_failure)? {
            Some(bytes) => bytes,
            None => {
                // Only a missing/empty/legacy plaintext inbox may mint a key.
                // Encrypted bytes without their existing key are not reset.
                match nessa_local_storage::open(&self.path, nessa_local_storage::OpenMode::Read) {
                    Ok(mut file) => {
                        let length = file.metadata().map_err(|_| Failure::Storage)?.len();
                        let mut header = [0; 16];
                        if length != 0
                            && (file.read_exact(&mut header).is_err()
                                || &header != b"SQLite format 3\0")
                        {
                            return Err(Failure::Storage);
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                    Err(_) => return Err(Failure::Storage),
                }
                let mut bytes = zeroize::Zeroizing::new(vec![0; 32]);
                getrandom::fill(&mut bytes).map_err(|_| Failure::Storage)?;
                self.secret.write(&bytes).map_err(secret_failure)?;
                let saved = self
                    .secret
                    .read()
                    .map_err(|_| Failure::Storage)?
                    .ok_or(Failure::Storage)?;
                if *saved != *bytes {
                    return Err(Failure::Storage);
                }
                bytes
            }
        };
        let key = Arc::new(DatabaseKey::new(
            bytes.as_slice().try_into().map_err(|_| Failure::Storage)?,
        ));
        Ok(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::SecretUnavailable;
    use std::sync::Mutex;
    #[derive(Default)]
    struct Memory(Mutex<Option<Vec<u8>>>);
    impl SecretStore for Memory {
        fn read(&self) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>, SecretUnavailable> {
            Ok(self.0.lock().unwrap().clone().map(zeroize::Zeroizing::new))
        }
        fn write(&self, b: &[u8]) -> Result<(), SecretUnavailable> {
            *self.0.lock().unwrap() = Some(b.to_vec());
            Ok(())
        }
        fn delete(&self) -> Result<(), SecretUnavailable> {
            *self.0.lock().unwrap() = None;
            Ok(())
        }
    }
    #[test]
    fn concurrent_initialization_has_one_key_and_a_missing_encrypted_key_is_not_replaced() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("private/inbox");
        let secret = Arc::new(Memory::default());
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let key = VaultInboxKey::new(path.clone(), secret.clone());
                let path = path.clone();
                std::thread::spawn(move || {
                    let key = key.load().unwrap();
                    let schema = nessa_local_database::Schema::new(
                        "CREATE TABLE x(value TEXT);\nPRAGMA user_version = 1;",
                    )
                    .unwrap();
                    nessa_local_database::open_encrypted(&path, &schema, &key).unwrap();
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let before = std::fs::read(&path).unwrap();
        secret.delete().unwrap();
        assert!(matches!(
            VaultInboxKey::new(path.clone(), secret.clone()).load(),
            Err(Failure::Storage)
        ));
        assert!(secret.read().unwrap().is_none());
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
    #[test]
    fn unavailable_vault_entry_does_not_create_a_database_or_file_secret() {
        struct Locked;
        impl SecretStore for Locked {
            fn read(&self) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>, SecretUnavailable> {
                Err(SecretUnavailable::Unavailable)
            }
            fn write(&self, _: &[u8]) -> Result<(), SecretUnavailable> {
                panic!("locked store must not write")
            }
            fn delete(&self) -> Result<(), SecretUnavailable> {
                panic!("not disconnecting")
            }
        }
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("private/inbox");
        assert!(matches!(
            VaultInboxKey::new(path.clone(), Arc::new(Locked)).load(),
            Err(Failure::Storage)
        ));
        assert!(!path.exists());
    }
}

fn secret_failure(problem: crate::secrets::SecretUnavailable) -> Failure {
    match problem {
        crate::secrets::SecretUnavailable::Locked => Failure::VaultLocked,
        crate::secrets::SecretUnavailable::Unavailable => Failure::Storage,
    }
}
