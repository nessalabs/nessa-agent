//! The one GitHub sign-in, kept in `<namespace>/github/sign-in.json`: a
//! private file in a private directory, replaced whole and synced, never
//! `config.json` and never a log. It is as private as the agents' own
//! sign-ins beside it, which the same user's processes can read.
use std::io::{Read, Write};
use std::path::PathBuf;

use async_trait::async_trait;
use nessa_local_storage::{create_directory, open, sync_directory, OpenMode, PrivateTempFile};
use serde::{Deserialize, Serialize};

use crate::github::application::{GithubTokenStore, StoreUnavailable};
use crate::github::domain::{AccessToken, GithubAccount, GithubSignIn};

/// A stored sign-in is a few hundred bytes; anything larger is not one.
const FILE_LIMIT: u64 = 16_384;
const FILE_NAME: &str = "sign-in.json";

pub struct GithubTokenFile {
    directory: PathBuf,
}

impl GithubTokenFile {
    /// The sign-in in `directory`, created private by its first save.
    pub fn new(directory: PathBuf) -> Self {
        Self { directory }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Stored {
    account_id: u64,
    login: String,
    token: String,
}

impl GithubTokenFile {
    fn read(&self) -> Result<Option<GithubSignIn>, StoreUnavailable> {
        let file = match open(&self.directory.join(FILE_NAME), OpenMode::Read) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(StoreUnavailable),
        };
        let mut bytes = Vec::new();
        file.take(FILE_LIMIT + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| StoreUnavailable)?;
        if bytes.len() as u64 > FILE_LIMIT {
            return Err(StoreUnavailable);
        }
        let stored: Stored = serde_json::from_slice(&bytes).map_err(|_| StoreUnavailable)?;
        Ok(Some(GithubSignIn {
            account: GithubAccount {
                id: stored.account_id,
                login: stored.login,
            },
            token: AccessToken::new(stored.token).ok_or(StoreUnavailable)?,
        }))
    }

    fn write(&self, sign_in: &GithubSignIn) -> Result<(), StoreUnavailable> {
        create_directory(&self.directory).map_err(|_| StoreUnavailable)?;
        let mut file = PrivateTempFile::new_in(&self.directory).map_err(|_| StoreUnavailable)?;
        serde_json::to_writer(
            file.as_file_mut(),
            &Stored {
                account_id: sign_in.account.id,
                login: sign_in.account.login.clone(),
                token: sign_in.token.expose().to_owned(),
            },
        )
        .map_err(|_| StoreUnavailable)?;
        file.as_file_mut()
            .write_all(b"\n")
            .and_then(|_| file.as_file().sync_all())
            .map_err(|_| StoreUnavailable)?;
        file.persist(&self.directory.join(FILE_NAME))
            .map_err(|_| StoreUnavailable)?;
        sync_directory(&self.directory).map_err(|_| StoreUnavailable)
    }

    fn remove(&self) -> Result<(), StoreUnavailable> {
        match std::fs::remove_file(self.directory.join(FILE_NAME)) {
            Ok(()) => sync_directory(&self.directory).map_err(|_| StoreUnavailable),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(StoreUnavailable),
        }
    }
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
impl GithubTokenStore for GithubTokenFile {
    async fn load(&self) -> Result<Option<GithubSignIn>, StoreUnavailable> {
        let store = Self::new(self.directory.clone());
        blocking(move || store.read()).await
    }
    async fn save(&self, sign_in: &GithubSignIn) -> Result<(), StoreUnavailable> {
        let store = Self::new(self.directory.clone());
        let sign_in = sign_in.clone();
        blocking(move || store.write(&sign_in)).await
    }
    async fn delete(&self) -> Result<(), StoreUnavailable> {
        let store = Self::new(self.directory.clone());
        blocking(move || store.remove()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    async fn a_saved_sign_in_reads_back_privately_and_is_deleted_once() {
        let root = tempfile::tempdir().unwrap();
        let store = GithubTokenFile::new(root.path().join("github"));
        assert_eq!(store.load().await, Ok(None));
        store.save(&sign_in("ghu_one")).await.unwrap();
        store.save(&sign_in("ghu_two")).await.unwrap();
        assert_eq!(store.load().await, Ok(Some(sign_in("ghu_two"))));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |path: PathBuf| std::fs::metadata(path).unwrap().permissions().mode();
            assert_eq!(mode(root.path().join("github/sign-in.json")) & 0o077, 0);
            assert_eq!(mode(root.path().join("github")) & 0o077, 0);
        }
        store.delete().await.unwrap();
        store.delete().await.unwrap();
        assert_eq!(store.load().await, Ok(None));
    }

    #[tokio::test]
    async fn a_file_that_is_not_a_sign_in_is_unavailable_never_signed_out() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("github");
        std::fs::create_dir(&directory).unwrap();
        let store = GithubTokenFile::new(directory.clone());
        for bytes in [b"{".to_vec(), vec![b' '; FILE_LIMIT as usize + 1]] {
            std::fs::write(directory.join(FILE_NAME), bytes).unwrap();
            assert_eq!(store.load().await, Err(StoreUnavailable));
        }
    }
}
