//! GitHub sign-in in the injected encrypted vault entry. A pre-existing private
//! JSON file is consumed only for a verified, crash-retryable one-time upgrade.
use crate::github::application::GithubSecret;
use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;
use zeroize::Zeroizing;

use async_trait::async_trait;
use nessa_local_storage::{create_directory, open, sync_directory, OpenMode};
use serde::{Deserialize, Serialize};

use crate::github::application::{GithubTokenStore, StoreUnavailable};
use crate::github::domain::{AccessToken, GithubAccount, GithubSignIn};

/// A stored sign-in is a few hundred bytes; anything larger is not one.
const FILE_LIMIT: u64 = 16_384;
const FILE_NAME: &str = "sign-in.json";

pub struct GithubTokenVault {
    directory: PathBuf,
    secret: Arc<dyn GithubSecret>,
}

impl GithubTokenVault {
    /// The sign-in in `directory`, created private by its first save.
    pub fn new(directory: PathBuf, secret: Arc<dyn GithubSecret>) -> Self {
        Self { directory, secret }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Stored {
    account_id: u64,
    login: String,
    token: String,
}

impl GithubTokenVault {
    fn legacy(&self) -> Result<Option<GithubSignIn>, StoreUnavailable> {
        let file = match open(&self.directory.join(FILE_NAME), OpenMode::Read) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(StoreUnavailable),
        };
        let mut bytes = Zeroizing::new(Vec::new());
        file.take(FILE_LIMIT + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| StoreUnavailable)?;
        if bytes.len() as u64 > FILE_LIMIT {
            return Err(StoreUnavailable);
        }
        decode(&bytes).map(Some)
    }

    fn read(&self, generation: u64) -> Result<Option<GithubSignIn>, StoreUnavailable> {
        let _lock = self.lock()?;
        let saved = self.secret.read(generation).map_err(|_| StoreUnavailable)?;
        let decoded = saved.as_ref().map(|bytes| decode(bytes)).transpose()?;
        match (decoded, self.legacy()?) {
            (current, None) => Ok(current),
            (Some(current), Some(legacy)) if current != legacy => Err(StoreUnavailable),
            (current, Some(legacy)) => {
                if current.is_none() {
                    self.write_verified(generation, &legacy)?;
                }
                self.remove_legacy()?;
                Ok(Some(legacy))
            }
        }
    }
    fn lock(&self) -> Result<std::fs::File, StoreUnavailable> {
        create_directory(&self.directory).map_err(|_| StoreUnavailable)?;
        let lock = open(
            &self.directory.join("credential-lock"),
            OpenMode::OpenOrCreate,
        )
        .map_err(|_| StoreUnavailable)?;
        lock.lock().map_err(|_| StoreUnavailable)?;
        Ok(lock)
    }
    fn write_verified(
        &self,
        generation: u64,
        sign_in: &GithubSignIn,
    ) -> Result<(), StoreUnavailable> {
        let bytes = Zeroizing::new(
            serde_json::to_vec(&Stored {
                account_id: sign_in.account.id,
                login: sign_in.account.login.clone(),
                token: sign_in.token.expose().to_owned(),
            })
            .map_err(|_| StoreUnavailable)?,
        );
        self.secret
            .write(generation, &bytes)
            .map_err(|_| StoreUnavailable)?;
        let receipt = self
            .secret
            .read(generation)
            .map_err(|_| StoreUnavailable)?
            .ok_or(StoreUnavailable)?;
        if receipt.as_slice() != bytes.as_slice() {
            return Err(StoreUnavailable);
        }
        Ok(())
    }
    fn write(&self, generation: u64, sign_in: &GithubSignIn) -> Result<(), StoreUnavailable> {
        let _lock = self.lock()?;
        self.write_verified(generation, sign_in)?;
        self.remove_legacy()
    }
    fn remove(&self, generation: u64) -> Result<(), StoreUnavailable> {
        let _lock = self.lock()?;
        // Delete any migration source first: an interrupted disconnect must not
        // resurrect it on next load. A failed protected delete remains unavailable.
        self.remove_legacy()?;
        self.secret.delete(generation).map_err(|_| StoreUnavailable)
    }
    fn remove_legacy(&self) -> Result<(), StoreUnavailable> {
        match std::fs::remove_file(self.directory.join(FILE_NAME)) {
            Ok(()) => sync_directory(&self.directory).map_err(|_| StoreUnavailable),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(StoreUnavailable),
        }
    }
}

fn decode(bytes: &[u8]) -> Result<GithubSignIn, StoreUnavailable> {
    if bytes.len() as u64 > FILE_LIMIT {
        return Err(StoreUnavailable);
    }
    let stored: Stored = serde_json::from_slice(bytes).map_err(|_| StoreUnavailable)?;
    Ok(GithubSignIn {
        account: GithubAccount {
            id: stored.account_id,
            login: stored.login,
        },
        token: AccessToken::new(stored.token).ok_or(StoreUnavailable)?,
    })
}

/// Runs `work` on a thread of its own, so the file system never holds up
/// the runtime.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, StoreUnavailable> + Send + 'static,
) -> Result<T, StoreUnavailable> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| StoreUnavailable)?
}

#[async_trait]
impl GithubTokenStore for GithubTokenVault {
    async fn load(&self, generation: u64) -> Result<Option<GithubSignIn>, StoreUnavailable> {
        let store = Self::new(self.directory.clone(), self.secret.clone());
        blocking(move || store.read(generation)).await
    }
    async fn save(&self, generation: u64, sign_in: &GithubSignIn) -> Result<(), StoreUnavailable> {
        let store = Self::new(self.directory.clone(), self.secret.clone());
        let sign_in = sign_in.clone();
        blocking(move || store.write(generation, &sign_in)).await
    }
    async fn delete(&self, generation: u64) -> Result<(), StoreUnavailable> {
        let store = Self::new(self.directory.clone(), self.secret.clone());
        blocking(move || store.remove(generation)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Memory(std::sync::Mutex<Option<Vec<u8>>>);
    impl GithubSecret for Memory {
        fn read(&self, _generation: u64) -> Result<Option<Zeroizing<Vec<u8>>>, StoreUnavailable> {
            Ok(self.0.lock().unwrap().clone().map(Zeroizing::new))
        }
        fn write(&self, _generation: u64, bytes: &[u8]) -> Result<(), StoreUnavailable> {
            *self.0.lock().unwrap() = Some(bytes.to_vec());
            Ok(())
        }
        fn delete(&self, _generation: u64) -> Result<(), StoreUnavailable> {
            *self.0.lock().unwrap() = None;
            Ok(())
        }
    }
    fn sign_in(token: &str) -> GithubSignIn {
        GithubSignIn {
            account: GithubAccount {
                id: 7,
                login: "octo".into(),
            },
            token: AccessToken::new(token.into()).unwrap(),
        }
    }

    #[tokio::test]
    async fn interrupted_migration_finishes_only_after_matching_encrypted_receipt() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("github");
        create_directory(&directory).unwrap();
        let secret = Arc::new(Memory::default());
        let store = GithubTokenVault::new(directory.clone(), secret.clone());
        let legacy = br#"{"accountId":7,"login":"octo","token":"ghu_one"}"#;
        let file = directory.join(FILE_NAME);
        let mut private = open(&file, OpenMode::OpenOrCreate).unwrap();
        std::io::Write::write_all(&mut private, legacy).unwrap();
        drop(private);
        assert_eq!(store.load(1).await, Ok(Some(sign_in("ghu_one"))));
        assert!(!file.exists());
        let mut private = open(&file, OpenMode::OpenOrCreate).unwrap();
        std::io::Write::write_all(&mut private, legacy).unwrap();
        drop(private);
        store.write_verified(1, &sign_in("ghu_two")).unwrap();
        assert_eq!(store.load(1).await, Err(StoreUnavailable));
        assert!(file.exists());
        store.write_verified(1, &sign_in("ghu_one")).unwrap();
        assert_eq!(store.load(1).await, Ok(Some(sign_in("ghu_one"))));
        assert!(!file.exists());
    }
    #[tokio::test]
    async fn locked_vault_entry_preserves_legacy_source_and_refuses_save_or_disconnect() {
        struct Locked;
        impl GithubSecret for Locked {
            fn read(
                &self,
                _generation: u64,
            ) -> Result<Option<Zeroizing<Vec<u8>>>, StoreUnavailable> {
                Err(StoreUnavailable)
            }
            fn write(&self, _generation: u64, _: &[u8]) -> Result<(), StoreUnavailable> {
                Err(StoreUnavailable)
            }
            fn delete(&self, _generation: u64) -> Result<(), StoreUnavailable> {
                Err(StoreUnavailable)
            }
        }
        let root = tempfile::tempdir().unwrap();
        let store = GithubTokenVault::new(root.path().join("github"), Arc::new(Locked));
        assert_eq!(store.load(1).await, Err(StoreUnavailable));
        assert_eq!(
            store.save(1, &sign_in("ghu_one")).await,
            Err(StoreUnavailable)
        );
        assert!(!root.path().join("github/sign-in.json").exists());
        assert_eq!(store.delete(1).await, Err(StoreUnavailable));
    }
    #[tokio::test]
    async fn a_saved_sign_in_reads_back_privately_and_is_deleted_once() {
        let root = tempfile::tempdir().unwrap();
        let store = GithubTokenVault::new(root.path().join("github"), Arc::new(Memory::default()));
        assert_eq!(store.load(1).await, Ok(None));
        store.save(1, &sign_in("ghu_one")).await.unwrap();
        store.save(1, &sign_in("ghu_two")).await.unwrap();
        assert_eq!(store.load(1).await, Ok(Some(sign_in("ghu_two"))));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |path: PathBuf| std::fs::metadata(path).unwrap().permissions().mode();
            assert!(!root.path().join("github/sign-in.json").exists());
            assert_eq!(mode(root.path().join("github")) & 0o077, 0);
        }
        store.delete(1).await.unwrap();
        store.delete(1).await.unwrap();
        assert_eq!(store.load(1).await, Ok(None));
    }

    #[tokio::test]
    async fn a_file_that_is_not_a_sign_in_is_unavailable_never_signed_out() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("github");
        std::fs::create_dir(&directory).unwrap();
        let store = GithubTokenVault::new(directory.clone(), Arc::new(Memory::default()));
        for bytes in [b"{".to_vec(), vec![b' '; FILE_LIMIT as usize + 1]] {
            std::fs::write(directory.join(FILE_NAME), bytes).unwrap();
            assert_eq!(store.load(1).await, Err(StoreUnavailable));
        }
    }
    #[tokio::test]
    async fn an_old_codec_save_or_delete_cannot_touch_a_new_vault_generation() {
        use nessa_vault::{
            application::{Clock, ObservationClock, Vault},
            domain::{EntryName, Failure, Passphrase},
            infrastructure::{EncryptedFileVault, FileAudit, OsEntropy},
        };
        struct Time;
        impl Clock for Time {
            fn seconds(&self) -> u64 {
                0
            }
        }
        impl ObservationClock for Time {
            fn unix_millis(&self) -> Result<u64, Failure> {
                Ok(1793030400000)
            }
        }
        let root = tempfile::tempdir().unwrap();
        let vault = Vault::new(
            Arc::new(EncryptedFileVault::new(
                root.path().join("vault"),
                Arc::new(OsEntropy),
            )),
            Arc::new(FileAudit::new(root.path().join("audit"), Arc::new(Time))),
            Arc::new(Time),
        );
        let caller = nessa_vault::application::Caller {
            principal: "owner".into(),
            credential: "desktop".into(),
        };
        let first = vault.session("first".into(), caller.clone());
        vault
            .create(
                &first.lease(),
                Passphrase::new("disposable long passphrase".into()).unwrap(),
                &[],
            )
            .unwrap();
        let old = vault.unlocked_generation().unwrap();
        let store = GithubTokenVault::new(
            root.path().join("github"),
            Arc::new(crate::secrets::VaultSecret::new(
                vault.clone(),
                EntryName::new("github-sign-in").unwrap(),
            )),
        );
        store.save(old, &sign_in("ghu_old")).await.unwrap();
        drop(first);
        let second = vault.session("second".into(), caller);
        vault
            .unlock(
                &second.lease(),
                Passphrase::new("disposable long passphrase".into()).unwrap(),
            )
            .unwrap();
        let current = vault.unlocked_generation().unwrap();
        store.save(current, &sign_in("ghu_current")).await.unwrap();
        assert_eq!(store.load(old).await, Err(StoreUnavailable));
        assert_eq!(
            store.save(old, &sign_in("ghu_stale")).await,
            Err(StoreUnavailable)
        );
        assert_eq!(store.delete(old).await, Err(StoreUnavailable));
        assert_eq!(store.load(current).await, Ok(Some(sign_in("ghu_current"))));
    }
}
