//! Private local storage shared by native adapters. No authentication policy lives here.
//! Existing unsafe files are rejected, never silently repaired.
use std::{
    fs::File,
    io,
    path::{Path, PathBuf},
};
#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as platform;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as platform;

pub use platform::{
    create_directory, open, replace, sync_directory, verify_directory, verify_file,
};

#[derive(Clone, Copy)]
pub enum OpenMode {
    Read,
    ReadWrite,
    OpenOrCreate,
    CreateNew,
}
fn unsafe_file() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "local storage must be private and owned by the current OS user",
    )
}

/// A temporary file protected at creation, before any secret bytes are written.
pub struct PrivateTempFile {
    file: File,
    path: PathBuf,
}
impl PrivateTempFile {
    pub fn new_in(parent: &Path) -> io::Result<Self> {
        verify_directory(parent)?;
        for _ in 0..10 {
            let mut random = [0u8; 16];
            getrandom::fill(&mut random).map_err(|e| io::Error::other(e.to_string()))?;
            let name: String = random.iter().map(|b| format!("{b:02x}")).collect();
            let path = parent.join(format!(".nessa-{name}.tmp"));
            match open(&path, OpenMode::CreateNew) {
                Ok(file) => return Ok(Self { file, path }),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not reserve private temporary file",
        ))
    }
    pub fn as_file(&self) -> &File {
        &self.file
    }
    pub fn as_file_mut(&mut self) -> &mut File {
        &mut self.file
    }
    pub fn persist(self, destination: &Path) -> io::Result<()> {
        replace(&self.path, destination)
    }
}
impl Drop for PrivateTempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    #[test]
    fn private_creation_reopen_replace_and_hardlink_rejection() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("private");
        create_directory(&directory).unwrap();
        let path = directory.join("token");
        open(&path, OpenMode::CreateNew)
            .unwrap()
            .write_all(b"first")
            .unwrap();
        assert!(open(&path, OpenMode::CreateNew).is_err());
        let mut temp = PrivateTempFile::new_in(&directory).unwrap();
        temp.as_file_mut().write_all(b"replacement").unwrap();
        temp.as_file().sync_all().unwrap();
        temp.persist(&path).unwrap();
        sync_directory(&directory).unwrap();
        let mut text = String::new();
        open(&path, OpenMode::Read)
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        assert_eq!(text, "replacement");
        std::fs::hard_link(&path, directory.join("alias")).unwrap();
        assert!(open(&path, OpenMode::Read).is_err());
    }
}
