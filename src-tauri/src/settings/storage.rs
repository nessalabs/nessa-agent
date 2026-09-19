//! Atomic filesystem effects for desktop settings.
//!
//! `settings.rs` decides when defaults are allowed. This adapter only reads or
//! durably replaces one path, so failed reads and writes cannot become policy.

use std::{io, io::Read, io::Write, path::Path};

const MAX_SETTINGS_BYTES: u64 = 64 * 1024;

pub(crate) trait Storage: Send + Sync {
    fn read(&self, path: &Path) -> io::Result<String>;
    fn write(&self, path: &Path, bytes: &[u8]) -> io::Result<()>;
}

pub(crate) struct FileStorage;
impl Storage for FileStorage {
    fn read(&self, path: &Path) -> io::Result<String> {
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
            FileStorage.read(&regular).unwrap(),
            r#"{"stopAgentsOnQuit":false}"#
        );

        let fifo = root.join("settings.fifo");
        assert!(std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success());
        assert!(FileStorage.read(&fifo).is_err());
        std::fs::remove_dir_all(root).unwrap();
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
            FileStorage.read(&path).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
