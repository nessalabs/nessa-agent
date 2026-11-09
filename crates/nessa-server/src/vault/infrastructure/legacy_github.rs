use crate::{
    secrets::{SecretStore, SecretUnavailable},
    vault::application::{Migration, MigrationSnapshot},
};
use nessa_local_storage::{create_directory, open, sync_directory, OpenMode, PrivateTempFile};
use nessa_vault::domain::{EntryName, Failure, Secret};
use std::{
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};
/// OS stores are constructed lazily, only when a pre-vault namespace needs import.
pub struct LegacyGithubMigration {
    root: PathBuf,
    signin: Result<Arc<dyn SecretStore>, SecretUnavailable>,
    inbox: Result<Arc<dyn SecretStore>, SecretUnavailable>,
}
impl LegacyGithubMigration {
    pub fn new(
        root: PathBuf,
        signin: Result<Arc<dyn SecretStore>, SecretUnavailable>,
        inbox: Result<Arc<dyn SecretStore>, SecretUnavailable>,
    ) -> Self {
        Self {
            root,
            signin,
            inbox,
        }
    }
}
struct Snapshot {
    marker: PathBuf,
    entries: Vec<(EntryName, Secret)>,
    sources: Vec<Arc<dyn SecretStore>>,
    required: Vec<EntryName>,
    _locks: Vec<File>,
    done: bool,
}
fn exists(path: &Path) -> Result<bool, Failure> {
    match open(path, OpenMode::Read) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(Failure::MigrationUnavailable),
    }
}
impl Migration for LegacyGithubMigration {
    fn capture(&self) -> Result<Box<dyn MigrationSnapshot>, Failure> {
        let marker = self.root.join("vault/github-imported");
        if exists(&marker)? {
            return Ok(Box::new(Snapshot {
                marker,
                entries: vec![],
                sources: vec![],
                required: vec![],
                _locks: vec![],
                done: true,
            }));
        }
        let signin = exists(&self.root.join("github/credential-lock"))?;
        let inbox = self.root.join("github-relay/inbox.sqlite");
        let mut encrypted = false;
        match open(&inbox, OpenMode::Read) {
            Ok(mut file) => {
                if file
                    .metadata()
                    .map_err(|_| Failure::MigrationUnavailable)?
                    .len()
                    > 0
                {
                    let mut header = [0; 16];
                    encrypted =
                        file.read_exact(&mut header).is_err() || &header != b"SQLite format 3\0";
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => return Err(Failure::MigrationUnavailable),
        }
        let mut snapshot = Snapshot {
            marker,
            entries: vec![],
            sources: vec![],
            required: vec![],
            _locks: vec![],
            done: false,
        };
        // This ordering matches the old adapters; neither old path holds both locks.
        for (needed, purpose, lock, source) in [
            (
                signin,
                "github-sign-in",
                self.root.join("github/credential-lock"),
                &self.signin,
            ),
            (
                encrypted,
                "github-inbox-key",
                inbox.with_extension("key-lock"),
                &self.inbox,
            ),
        ] {
            if !needed {
                continue;
            }
            create_directory(lock.parent().ok_or(Failure::MigrationUnavailable)?)
                .map_err(|_| Failure::MigrationUnavailable)?;
            let held =
                open(&lock, OpenMode::OpenOrCreate).map_err(|_| Failure::MigrationUnavailable)?;
            held.lock().map_err(|_| Failure::MigrationUnavailable)?;
            snapshot._locks.push(held);
            let source = source
                .as_ref()
                .map_err(|_| Failure::MigrationUnavailable)?
                .clone();
            let name = EntryName::new(purpose)?;
            if purpose == "github-inbox-key" {
                snapshot.required.push(name.clone());
            }
            if let Some(bytes) = source.read().map_err(|_| Failure::MigrationUnavailable)? {
                snapshot.entries.push((name, bytes.into()));
                snapshot.sources.push(source);
            }
        }
        Ok(Box::new(snapshot))
    }
}
impl MigrationSnapshot for Snapshot {
    fn entries(&self) -> &[(EntryName, Secret)] {
        &self.entries
    }
    fn required(&self) -> &[EntryName] {
        &self.required
    }
    fn retire(&self) -> Result<(), Failure> {
        if self.done {
            return Ok(());
        }
        for (source, (_, expected)) in self.sources.iter().zip(&self.entries) {
            if source
                .read()
                .map_err(|_| Failure::MigrationUnavailable)?
                .as_ref()
                .map(|value| value.as_slice())
                != Some(expected.as_slice())
            {
                return Err(Failure::MigrationUnavailable);
            }
        }
        for source in &self.sources {
            source.delete().map_err(|_| Failure::MigrationUnavailable)?;
        }
        let directory = self.marker.parent().ok_or(Failure::MigrationUnavailable)?;
        create_directory(directory).map_err(|_| Failure::MigrationUnavailable)?;
        let mut marker =
            PrivateTempFile::new_in(directory).map_err(|_| Failure::MigrationUnavailable)?;
        marker
            .as_file_mut()
            .write_all(b"verified GitHub vault import\n")
            .map_err(|_| Failure::MigrationUnavailable)?;
        marker
            .as_file()
            .sync_all()
            .map_err(|_| Failure::MigrationUnavailable)?;
        marker
            .persist(&self.marker)
            .map_err(|_| Failure::MigrationUnavailable)?;
        sync_directory(directory).map_err(|_| Failure::MigrationUnavailable)
    }
}
