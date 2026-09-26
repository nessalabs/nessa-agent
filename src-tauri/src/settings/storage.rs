//! Atomic filesystem effects for desktop settings.
//!
//! `settings.rs` decides when defaults are allowed. This adapter only reads or
//! durably replaces one path, so failed reads and writes cannot become policy.

use super::repair::{SharedReadRepair, RECORDS};
use nessa_local_storage::SharedReadKind;
use std::{io, io::Read, io::Write, path::Path};

const MAX_SETTINGS_BYTES: u64 = 64 * 1024;

pub(crate) trait Storage: Send + Sync {
    fn read(&self, path: &Path) -> io::Result<String>;
    fn write(&self, path: &Path, bytes: &[u8]) -> io::Result<()>;
}

/// The real adapter. Built by [`FileStorage::repairing`] it makes one repair
/// before giving up on a read (ADR 221); the default repairs nothing.
#[derive(Default)]
pub(crate) struct FileStorage {
    repair: Option<SharedReadRepair>,
}

impl FileStorage {
    /// Reads that first make a shared-read settings file, or its folder,
    /// private, recording each change under `config_root`.
    pub(crate) fn repairing(config_root: &Path) -> Self {
        Self {
            repair: Some(SharedReadRepair::recording_in(config_root.join(RECORDS))),
        }
    }

    fn read_private(path: &Path) -> io::Result<String> {
        let file = nessa_local_storage::open(path, nessa_local_storage::OpenMode::ReadNonblocking)?;
        let mut bytes = Vec::new();
        file.take(MAX_SETTINGS_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_SETTINGS_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "settings file exceeds the size limit",
            ));
        }
        String::from_utf8(bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }
}

impl Storage for FileStorage {
    fn read(&self, path: &Path) -> io::Result<String> {
        let Some(repair) = &self.repair else {
            return Self::read_private(path);
        };
        // Reading never depended on the folder, so a folder that cannot be made
        // private is left as it was, silently: that is its steady state, and
        // writing still refuses it. Only an unexpected failure is worth a line.
        #[cfg(unix)]
        if let Some(folder) = path.parent() {
            match repair.repair(folder, SharedReadKind::Directory) {
                Ok(_) => {}
                Err(error)
                    if error.kind() == io::ErrorKind::NotFound
                        || nessa_local_storage::is_unsafe_file(&error) => {}
                Err(error) => eprintln!(
                    "[nessa] could not make {} private: {error}",
                    folder.display()
                ),
            }
        }
        match Self::read_private(path) {
            Err(refused) if nessa_local_storage::is_unsafe_file(&refused) => {
                match repair.repair(path, SharedReadKind::File) {
                    Ok(true) => Self::read_private(path),
                    Ok(false) => Err(refused),
                    Err(error) => {
                        eprintln!("[nessa] could not make {} private: {error}", path.display());
                        Err(refused)
                    }
                }
            }
            read => read,
        }
    }

    fn write(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        let parent = path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "settings path has no parent")
        })?;
        nessa_local_storage::create_directory(parent)?;
        let mut temporary = nessa_local_storage::PrivateTempFile::new_in(parent)?;
        temporary.as_file_mut().write_all(bytes)?;
        temporary.as_file_mut().flush()?;
        temporary.as_file().sync_all()?;
        temporary.persist(path)?;
        nessa_local_storage::sync_directory(parent)
    }
}

/// The whole settings file, in a map, with either effect made to fail on demand.
///
/// It lives beside the real adapter rather than inside one module's test block
/// because the decisions that read and write settings are spread across the
/// host — the first-run flag in `panel`, the quit policy in `tray`, the quit
/// policy again on exit — and each of them is tested against this one substitute
/// through [`super::SettingsStore`]. A second in-memory settings store would be
/// a second account of what a settings file does.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct MemoryStorage {
    pub(crate) files: std::sync::Mutex<std::collections::HashMap<std::path::PathBuf, Vec<u8>>>,
    /// Consumed by the next read, which fails with this kind instead.
    pub(crate) read_error: std::sync::Mutex<Option<io::ErrorKind>>,
    /// Consumed by the next write, the same way.
    pub(crate) write_error: std::sync::Mutex<Option<io::ErrorKind>>,
}

#[cfg(test)]
impl MemoryStorage {
    /// Puts bytes on the "disk" without going through the settings writer, so a
    /// test can start from a file this build cannot parse.
    pub(crate) fn put(&self, path: &Path, bytes: &[u8]) {
        self.files
            .lock()
            .unwrap()
            .insert(path.to_owned(), bytes.to_vec());
    }

    /// The bytes currently on the "disk", for asserting that a refused write
    /// left the person's file exactly as it was.
    pub(crate) fn get(&self, path: &Path) -> Option<Vec<u8>> {
        self.files.lock().unwrap().get(path).cloned()
    }
}

#[cfg(test)]
impl Storage for MemoryStorage {
    fn read(&self, path: &Path) -> io::Result<String> {
        if let Some(kind) = self.read_error.lock().unwrap().take() {
            return Err(io::Error::from(kind));
        }
        let files = self.files.lock().unwrap();
        let bytes = files
            .get(path)
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
        String::from_utf8(bytes.clone())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    fn write(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        if let Some(kind) = self.write_error.lock().unwrap().take() {
            return Err(io::Error::from(kind));
        }
        self.put(path, bytes);
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn temporary_directory(name: &str) -> std::path::PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "nessa-settings-{name}-{}-{unique}",
            std::process::id()
        ));
        nessa_local_storage::create_directory(&path).unwrap();
        path
    }

    #[test]
    fn private_regular_settings_read_and_fifo_rejection_are_bounded() {
        let root = temporary_directory("fifo");
        let regular = root.join("settings.json");
        let mut file =
            nessa_local_storage::open(&regular, nessa_local_storage::OpenMode::CreateNew).unwrap();
        file.write_all(br#"{"stopAgentsOnQuit":false}"#).unwrap();
        drop(file);
        assert_eq!(
            FileStorage::default().read(&regular).unwrap(),
            r#"{"stopAgentsOnQuit":false}"#
        );

        let fifo = root.join("settings.fifo");
        assert!(std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success());
        assert!(FileStorage::default().read(&fifo).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    /// The state found on 2026-09-26 (#221): the config folder `0755` and
    /// `settings.json` `0644`. The read repairs both, records four steps, and
    /// returns the person's settings. A file others can write stays refused.
    #[test]
    fn a_shared_read_config_folder_and_file_are_made_private_before_reading() {
        use std::os::unix::fs::PermissionsExt;
        let mode_of = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("so.nessa.app");
        std::fs::create_dir(&config).unwrap();
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = config.join("settings.json");
        std::fs::write(&path, br#"{"stopAgentsOnQuit":false}"#).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(FileStorage::default().read(&path).is_err());

        let storage = FileStorage::repairing(&config);
        assert_eq!(
            storage.read(&path).unwrap(),
            r#"{"stopAgentsOnQuit":false}"#
        );
        assert_eq!((mode_of(&config), mode_of(&path)), (0o700, 0o600));
        assert_eq!(
            std::fs::read_dir(config.join(super::super::repair::RECORDS))
                .unwrap()
                .count(),
            4
        );
        storage.write(&path, b"{}").unwrap();

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o664)).unwrap();
        assert!(nessa_local_storage::is_unsafe_file(
            &storage.read(&path).unwrap_err()
        ));
        assert_eq!(mode_of(&path), 0o664);
    }

    /// A folder others can write is not repairable, and reading never needed
    /// it private: the read goes ahead and the folder is left exactly as it was.
    #[test]
    fn a_folder_that_cannot_be_repaired_is_left_alone_and_read_as_before() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("so.nessa.app");
        std::fs::create_dir(&config).unwrap();
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o775)).unwrap();
        let path = config.join("settings.json");
        let mut file =
            nessa_local_storage::open(&path, nessa_local_storage::OpenMode::CreateNew).unwrap();
        file.write_all(b"{}").unwrap();
        drop(file);

        assert_eq!(FileStorage::repairing(&config).read(&path).unwrap(), "{}");
        assert_eq!(
            std::fs::metadata(&config).unwrap().permissions().mode() & 0o777,
            0o775
        );
        assert!(!config.join(super::super::repair::RECORDS).exists());
    }

    #[test]
    fn oversized_settings_are_rejected() {
        let root = temporary_directory("oversized");
        let path = root.join("settings.json");
        let mut file =
            nessa_local_storage::open(&path, nessa_local_storage::OpenMode::CreateNew).unwrap();
        file.write_all(&vec![b'x'; MAX_SETTINGS_BYTES as usize + 1])
            .unwrap();
        drop(file);
        assert_eq!(
            FileStorage::default().read(&path).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
