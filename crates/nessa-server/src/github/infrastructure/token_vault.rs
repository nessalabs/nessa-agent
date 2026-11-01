//! GitHub sign-in in the injected encrypted vault entry. The passphrase-derived
//! key stays in the gateway process. A plaintext `sign-in.json` is not a store:
//! it is removed without being read, including while the vault is locked.
use crate::github::application::GithubSecret;
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
    fn read(&self, generation: u64) -> Result<Option<GithubSignIn>, StoreUnavailable> {
        let _lock = self.lock()?;
        // Delete any plaintext copy before answering, including when the vault
        // is locked. Its bytes are not imported. Nessa is alpha: there is no
        // migration of an older file.
        self.remove_legacy()?;
        let saved = self.secret.read(generation).map_err(|_| StoreUnavailable)?;
        saved.as_ref().map(|bytes| decode(bytes)).transpose()
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
        remove_plaintext(&self.directory)
    }
}

/// Remove `directory/sign-in.json` without reading it. A missing file or a
/// missing directory is success, so startup can call this more than once.
/// The credential lock is taken when the directory can be locked; a lock
/// failure does not skip the delete.
pub async fn remove_plaintext_sign_in(directory: PathBuf) -> Result<(), StoreUnavailable> {
    blocking(move || discard_at(&directory)).await
}

fn discard_at(directory: &std::path::Path) -> Result<(), StoreUnavailable> {
    if !directory.exists() {
        return Ok(());
    }
    let _lock = lock_if_possible(directory);
    remove_plaintext(directory)
}

fn lock_if_possible(directory: &std::path::Path) -> Option<std::fs::File> {
    create_directory(directory).ok()?;
    let lock = open(&directory.join("credential-lock"), OpenMode::OpenOrCreate).ok()?;
    lock.lock().ok()?;
    Some(lock)
}

fn remove_plaintext(directory: &std::path::Path) -> Result<(), StoreUnavailable> {
    match std::fs::remove_file(directory.join(FILE_NAME)) {
        Ok(()) => sync_directory(directory).map_err(|_| StoreUnavailable),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(StoreUnavailable),
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
    async fn discard_plaintext(&self) -> Result<(), StoreUnavailable> {
        remove_plaintext_sign_in(self.directory.clone()).await
    }
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
    async fn a_plaintext_sign_in_is_removed_and_not_adopted() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("github");
        create_directory(&directory).unwrap();
        let store = GithubTokenVault::new(directory.clone(), Arc::new(Memory::default()));
        let file = directory.join(FILE_NAME);
        let mut private = open(&file, OpenMode::OpenOrCreate).unwrap();
        std::io::Write::write_all(
            &mut private,
            br#"{"accountId":7,"login":"octo","token":"ghu_one"}"#,
        )
        .unwrap();
        drop(private);
        assert_eq!(store.load(1).await, Ok(None));
        assert!(!file.exists());
        store.save(1, &sign_in("ghu_kept")).await.unwrap();
        assert_eq!(store.load(1).await, Ok(Some(sign_in("ghu_kept"))));
        assert!(!file.exists());
    }
    #[tokio::test]
    async fn locked_vault_removes_plaintext_and_refuses_save_or_disconnect() {
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
        let directory = root.path().join("github");
        create_directory(&directory).unwrap();
        let file = directory.join(FILE_NAME);
        std::fs::write(
            &file,
            br#"{"accountId":7,"login":"octo","token":"ghu_one"}"#,
        )
        .unwrap();
        let store = GithubTokenVault::new(directory, Arc::new(Locked));
        assert_eq!(store.load(1).await, Err(StoreUnavailable));
        assert!(
            !file.exists(),
            "a locked vault must not leave the plaintext token"
        );
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
    async fn a_plaintext_file_is_removed_even_when_it_is_not_a_sign_in() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("github");
        create_directory(&directory).unwrap();
        let store = GithubTokenVault::new(directory.clone(), Arc::new(Memory::default()));
        let file = directory.join(FILE_NAME);
        for bytes in [b"{".to_vec(), vec![b' '; FILE_LIMIT as usize + 1]] {
            std::fs::write(&file, bytes).unwrap();
            assert_eq!(store.load(1).await, Ok(None));
            assert!(!file.exists());
        }
    }
    #[tokio::test]
    async fn an_agent_command_that_knows_the_path_cannot_read_the_token() {
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
        let token = "ghu_NsA1195AgentMustNotReadThisTokenValue";
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
        let session = vault.session("sign-in".into(), caller);
        vault
            .create(
                &session.lease(),
                Passphrase::new("disposable long passphrase".into()).unwrap(),
                &[],
            )
            .unwrap();
        let generation = vault.unlocked_generation().unwrap();
        let directory = root.path().join("github");
        let store = GithubTokenVault::new(
            directory.clone(),
            Arc::new(crate::secrets::VaultSecret::new(
                vault.clone(),
                EntryName::new("github-sign-in").unwrap(),
            )),
        );
        let plaintext = directory.join(FILE_NAME);
        create_directory(&directory).unwrap();
        std::fs::write(&plaintext, format!(r#"{{"token":"{token}"}}"#)).unwrap();
        store.save(generation, &sign_in(token)).await.unwrap();
        assert!(!plaintext.exists());
        let vault_file = root.path().join("vault").join("entries.enc");
        assert!(vault_file.is_file());
        let output = agent_command_output(root.path(), &plaintext, &vault_file);
        assert!(
            output.contains("NESSA_AGENT_COMMAND_RAN"),
            "the agent command did not run"
        );
        assert!(
            output.contains("SIGNIN_ABSENT"),
            "the plaintext sign-in path was still readable"
        );
        assert!(
            output.contains("VAULT_OPENED"),
            "the command did not open the vault file it was given"
        );
        assert!(
            output.contains("SWEEP_DONE"),
            "the command did not finish reading the namespace"
        );
        assert!(
            !output.contains(token),
            "the agent command obtained the token bytes"
        );
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

    /// The program the managed shell runs (`/bin/bash --noprofile --norc -c`),
    /// pointed at the sign-in path and the vault file, then at every file under
    /// the namespace. On Windows the same-user process is `powershell.exe`.
    fn agent_command_output(
        root: &std::path::Path,
        signin: &std::path::Path,
        vault: &std::path::Path,
    ) -> String {
        #[cfg(unix)]
        let output = {
            let quote = |path: &std::path::Path| {
                format!("'{}'", path.display().to_string().replace('\'', r"'\''"))
            };
            let script = format!(
            "echo NESSA_AGENT_COMMAND_RAN\n\
             if [ -f {signin} ]; then echo SIGNIN_OPENED; cat {signin}; else echo SIGNIN_ABSENT; fi\n\
             if [ -f {vault} ]; then echo VAULT_OPENED; cat {vault}; else echo VAULT_ABSENT; fi\n\
             echo SWEEP\n\
             find {root} -type f -exec cat {{}} +\n\
             echo SWEEP_DONE\n",
            signin = quote(signin),
            vault = quote(vault),
            root = quote(root),
        );
            std::process::Command::new("/bin/bash")
                .args(["--noprofile", "--norc", "-c", &script])
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .current_dir(std::env::temp_dir())
                .output()
                .expect("managed shell program")
        };
        #[cfg(windows)]
        let output = {
            let script = "\
$enc = [System.Text.Encoding]::GetEncoding(28591)
function Dump([string]$path) {
  [Console]::Out.Write($enc.GetString([System.IO.File]::ReadAllBytes($path)))
}
Write-Output NESSA_AGENT_COMMAND_RAN
if (Test-Path -LiteralPath $env:NESSA_SIGNIN) { Write-Output SIGNIN_OPENED; Dump $env:NESSA_SIGNIN } else { Write-Output SIGNIN_ABSENT }
if (Test-Path -LiteralPath $env:NESSA_VAULT) { Write-Output VAULT_OPENED; Dump $env:NESSA_VAULT } else { Write-Output VAULT_ABSENT }
Write-Output SWEEP
Get-ChildItem -LiteralPath $env:NESSA_ROOT -Recurse -File | ForEach-Object { Dump $_.FullName }
Write-Output SWEEP_DONE
";
            std::process::Command::new("powershell.exe")
                .args(["-NoProfile", "-NonInteractive", "-Command", script])
                .env("NESSA_SIGNIN", signin)
                .env("NESSA_VAULT", vault)
                .env("NESSA_ROOT", root)
                .current_dir(std::env::temp_dir())
                .output()
                .expect("windows agent command")
        };
        let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
        text.push_str(&String::from_utf8_lossy(&output.stderr));
        text
    }
}
