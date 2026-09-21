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
    create_directory, create_directory_beneath, open, open_beneath, remove_directory_beneath,
    remove_file_beneath, replace, replace_beneath, sync_directory, sync_directory_beneath,
    verify_directory, verify_file,
};
// Publication post-conditions differ by platform: the Unix link leaves the
// writer's own name behind until it is released. Only `PrivateTempFile::publish`
// knows how to complete that, so it stays the single entry point.
pub(crate) use platform::publish_new;

const TEMPORARY_PREFIX: &str = ".nessa-";
const TEMPORARY_SUFFIX: &str = ".tmp";

#[derive(Clone, Copy)]
pub enum OpenMode {
    Read,
    /// Opens an existing file for reads without waiting on special-file peers.
    ///
    /// The platform still verifies that the opened handle is a private,
    /// single-linked regular file before returning it.
    ReadNonblocking,
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
    beneath: Option<(PathBuf, PathBuf)>,
}
impl PrivateTempFile {
    pub fn new_in(parent: &Path) -> io::Result<Self> {
        verify_directory(parent)?;
        for _ in 0..10 {
            let mut random = [0u8; 16];
            getrandom::fill(&mut random).map_err(|e| io::Error::other(e.to_string()))?;
            let name: String = random.iter().map(|b| format!("{b:02x}")).collect();
            let path = parent.join(format!("{TEMPORARY_PREFIX}{name}{TEMPORARY_SUFFIX}"));
            match open(&path, OpenMode::CreateNew) {
                Ok(file) => {
                    return Ok(Self {
                        file,
                        path,
                        beneath: None,
                    })
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not reserve private temporary file",
        ))
    }
    /// Reserve a private temporary file within `directory`, relative to a trusted root.
    pub fn new_beneath(root: &Path, directory: &Path) -> io::Result<Self> {
        for _ in 0..10 {
            let mut random = [0u8; 16];
            getrandom::fill(&mut random).map_err(|e| io::Error::other(e.to_string()))?;
            let name: String = random.iter().map(|b| format!("{b:02x}")).collect();
            let relative = directory.join(format!("{TEMPORARY_PREFIX}{name}{TEMPORARY_SUFFIX}"));
            match open_beneath(root, &relative, OpenMode::CreateNew) {
                Ok(file) => {
                    return Ok(Self {
                        file,
                        path: root.join(&relative),
                        beneath: Some((root.to_path_buf(), relative)),
                    });
                }
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
    /// Publish this file under `destination` only if that name is still unused.
    ///
    /// Returns `AlreadyExists` when another owner already published there,
    /// leaving that record untouched. Success means this temporary name has
    /// also been released, so the published file has a single link and passes
    /// private-file verification; a failure to release it is reported rather
    /// than discarded, and [`Self::clear_stale`] repairs it.
    ///
    /// # Errors
    /// Any platform publication failure, including a taken destination, and any
    /// failure to release this temporary name afterwards.
    pub fn publish(self, destination: &Path) -> io::Result<()> {
        publish_new(&self.path, destination)?;
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            // A move already consumed this name; a link did not.
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }
    /// Remove temporary files an interrupted publish left in `directory`.
    ///
    /// A publish interrupted between linking its destination and releasing its
    /// own name leaves a second link to an already complete record, which then
    /// fails private-file verification. One owner calls this when it takes the
    /// directory, before its first publish; a concurrent writer would see its
    /// own publish fail rather than lose data.
    ///
    /// # Errors
    /// The directory cannot be read, or a temporary file cannot be removed.
    pub fn clear_stale(directory: &Path) -> io::Result<()> {
        verify_directory(directory)?;
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if name.starts_with(TEMPORARY_PREFIX) && name.ends_with(TEMPORARY_SUFFIX) {
                std::fs::remove_file(entry.path())?;
            }
        }
        Ok(())
    }
    /// Atomically publish this temporary file to a path beneath the same trusted root.
    pub fn persist_beneath(self, destination: &Path) -> io::Result<()> {
        let (root, relative) = self.beneath.as_ref().ok_or_else(unsafe_file)?;
        replace_beneath(root, relative, destination)
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
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
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

    #[test]
    fn publication_never_replaces_a_taken_name_and_stale_temporaries_are_released() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("private");
        create_directory(&directory).unwrap();
        let path = directory.join("record");

        let mut temp = PrivateTempFile::new_in(&directory).unwrap();
        temp.as_file_mut().write_all(b"owner").unwrap();
        temp.as_file().sync_all().unwrap();
        temp.publish(&path).unwrap();
        sync_directory(&directory).unwrap();
        // The publisher released its own name, so the record is a private,
        // single-linked file its owner can read back.
        let mut text = String::new();
        open(&path, OpenMode::Read)
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        assert_eq!(text, "owner");

        let mut second = PrivateTempFile::new_in(&directory).unwrap();
        second.as_file_mut().write_all(b"impostor").unwrap();
        second.as_file().sync_all().unwrap();
        assert_eq!(
            second.publish(&path).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        text.clear();
        open(&path, OpenMode::Read)
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        assert_eq!(text, "owner");

        // A publish interrupted before it released its own name leaves the
        // record unreadable until the next owner clears that temporary.
        let leftover = directory.join(".nessa-interrupted.tmp");
        std::fs::hard_link(&path, &leftover).unwrap();
        assert!(open(&path, OpenMode::Read).is_err());
        PrivateTempFile::clear_stale(&directory).unwrap();
        assert!(!leftover.exists());
        text.clear();
        open(&path, OpenMode::Read)
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        assert_eq!(text, "owner");
    }

    #[cfg(unix)]
    #[test]
    fn nonblocking_reads_accept_private_files_and_reject_fifos_without_a_writer() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("private");
        create_directory(&directory).unwrap();
        let regular = directory.join("regular");
        open(&regular, OpenMode::CreateNew)
            .unwrap()
            .write_all(b"value")
            .unwrap();

        let mut text = String::new();
        open(&regular, OpenMode::ReadNonblocking)
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        assert_eq!(text, "value");

        let fifo = directory.join("fifo");
        assert!(std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success());
        assert!(open(&fifo, OpenMode::ReadNonblocking).is_err());
    }

    /// A removal named relative to a root stops at that root, however the name
    /// is reached. `fs::remove_file` resolves the whole path in the kernel, so
    /// a link planted at any directory above the leaf makes it delete
    /// somebody else's file; the `beneath` form opens one verified component
    /// at a time and cannot leave.
    #[cfg(unix)]
    #[test]
    fn removals_beneath_a_root_cannot_be_redirected_through_a_symbolic_link() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("root");
        create_directory(&root).unwrap();

        // Removing what really is beneath the root works, files and empty
        // directories alike.
        create_directory_beneath(&root, Path::new("agent/versions")).unwrap();
        let file = Path::new("agent/versions/opencode");
        open(&root.join(file), OpenMode::CreateNew).unwrap();
        remove_file_beneath(&root, file).unwrap();
        assert!(!root.join(file).exists());
        remove_directory_beneath(&root, Path::new("agent/versions")).unwrap();
        assert!(!root.join("agent/versions").exists());

        // Elsewhere, reachable only through a link inside the root.
        let outside = temporary.path().join("outside");
        create_directory(&outside).unwrap();
        let treasure = outside.join("keep");
        open(&treasure, OpenMode::CreateNew).unwrap();
        let victim = outside.join("directory");
        create_directory(&victim).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("agent/elsewhere")).unwrap();

        // What the unanchored call would have done, said out loud: the link is
        // followed and the file outside the root is the one that goes.
        assert!(std::fs::remove_file(root.join("agent/elsewhere/keep")).is_ok());
        assert!(!treasure.exists());
        open(&treasure, OpenMode::CreateNew).unwrap();

        assert!(remove_file_beneath(&root, Path::new("agent/elsewhere/keep")).is_err());
        assert!(treasure.exists(), "the file outside the root survived");
        assert!(remove_directory_beneath(&root, Path::new("agent/elsewhere/directory")).is_err());
        assert!(victim.exists(), "the directory outside the root survived");

        // And the link itself is not a directory to descend through, nor a
        // path outside the root to be handed in directly.
        assert!(remove_directory_beneath(&root, Path::new("agent/elsewhere")).is_err());
        assert!(remove_file_beneath(&root, Path::new("../outside/keep")).is_err());
        assert!(remove_file_beneath(&root, Path::new("")).is_err());
        assert!(treasure.exists());
    }

    #[cfg(unix)]
    #[test]
    fn existing_shared_directories_are_rejected_without_permission_repair() {
        let root = tempfile::tempdir().unwrap();
        for mode in [0o755, 0o750] {
            let directory = root.path().join(format!("shared-{mode:o}"));
            std::fs::create_dir(&directory).unwrap();
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(mode)).unwrap();

            assert!(verify_directory(&directory).is_err());
            assert!(create_directory(&directory).is_err());
            assert_eq!(
                std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
                mode
            );
        }

        let private = root.path().join("private");
        create_directory(&private).unwrap();
        assert_eq!(
            std::fs::metadata(private).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
}
