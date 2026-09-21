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
// One owner: a temporary file is published by the type that reserved it, so
// that nothing can publish a name it did not create privately first.
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
    /// One move. When it returns, `destination` is this file — complete,
    /// single-linked, and readable by any other owner at once — and this
    /// temporary name no longer exists. There is no moment in between, which
    /// matters because the reader most likely to arrive during one is a
    /// concurrent writer that lost the race for this very name and is about to
    /// read the winner's record back to check that they agree.
    ///
    /// Returns `AlreadyExists` when another owner already published there,
    /// leaving that record untouched. This file is then released by `Drop`,
    /// exactly as an unpublished temporary is.
    ///
    /// # Errors
    /// Any platform publication failure, including a taken destination.
    pub fn publish(self, destination: &Path) -> io::Result<()> {
        publish_new(&self.path, destination)
    }
    /// Remove temporary files a killed process left in `directory`.
    ///
    /// A process that is killed between reserving a temporary and publishing it
    /// does not run [`Drop`], so its reservation stays on disk under a name
    /// nothing will ever claim. It is not a record and never was; one owner
    /// sweeps them up when it takes the directory. A concurrent writer's own
    /// temporary may be swept with them, and it sees its publish fail rather
    /// than lose data.
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

    /// The step `publish` is built out of, and the whole of what it promises.
    ///
    /// `publish_new` has to finish the publication, not begin it: the moment it
    /// returns, `destination` must be a complete record with one link and no
    /// second name anywhere — because another process is already reading that
    /// name, and there is nothing to wait for it to finish.
    ///
    /// It used to link the destination and leave the publisher to unlink its own
    /// name afterwards. Between those two calls the record had two links and
    /// `open` refused it as unsafe, so a reader that arrived in the gap was told
    /// the record could not be read. In `file_link_audit` that reader is the
    /// writer that just lost the race for the name; being refused made it
    /// refuse a legitimate submission, on a machine that happened to be busy.
    #[test]
    fn a_publication_is_complete_and_single_linked_the_moment_it_returns() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("private");
        create_directory(&directory).unwrap();
        let destination = directory.join("record");

        let mut temp = PrivateTempFile::new_in(&directory).unwrap();
        temp.as_file_mut().write_all(b"owner").unwrap();
        temp.as_file().sync_all().unwrap();
        let reserved = temp.path.clone();
        publish_new(&reserved, &destination).unwrap();

        // Read the way every other owner reads it, with no cleanup in between.
        let mut text = String::new();
        open(&destination, OpenMode::Read)
            .expect("a published record is readable at once")
            .read_to_string(&mut text)
            .unwrap();
        assert_eq!(text, "owner");
        assert!(!reserved.exists(), "the publisher's own name is gone");
    }
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

    /// The one failure here somebody can act on, said so they can.
    ///
    /// An exclusive rename is not universal: SMB and AFP answer `ENOTSUP`, and
    /// NFS, eCryptfs and many FUSE mounts answer `EINVAL` or `EOPNOTSUPP`. The
    /// link-and-unlink this replaced worked on all of them, so the blast radius
    /// is new, and the errno alone would reach a person as "conversation could
    /// not be created" with nothing to do about it.
    ///
    /// There is no fallback on purpose: falling back to the link would reopen
    /// the two-linked gap on exactly the volumes where a network round trip
    /// makes it widest. What a caller gets instead is a kind it can tell apart
    /// from a taken name, and a sentence naming the volume.
    ///
    /// Asserted through the kinds rather than by mounting one of those
    /// filesystems, which no test here can do: what is pinned is that the three
    /// outcomes stay distinguishable, since one is ordinary and one is fatal to
    /// every write this crate makes.
    #[cfg(unix)]
    #[test]
    fn a_volume_that_cannot_publish_is_told_apart_from_a_name_already_taken() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("private");
        create_directory(&directory).unwrap();
        let path = directory.join("record");

        let mut first = PrivateTempFile::new_in(&directory).unwrap();
        first.as_file_mut().write_all(b"owner").unwrap();
        first.publish(&path).unwrap();

        let mut second = PrivateTempFile::new_in(&directory).unwrap();
        second.as_file_mut().write_all(b"impostor").unwrap();
        let taken = second.publish(&path).unwrap_err();
        assert_eq!(
            taken.kind(),
            io::ErrorKind::AlreadyExists,
            "a taken name is the ordinary case and must not read as a broken volume"
        );

        // And a destination whose directory does not exist is neither: an
        // unexpected failure keeps the platform's own error rather than being
        // dressed up as one of the two a caller branches on.
        let mut third = PrivateTempFile::new_in(&directory).unwrap();
        third.as_file_mut().write_all(b"nowhere").unwrap();
        let elsewhere = third
            .publish(&directory.join("no-such/record"))
            .unwrap_err();
        assert_eq!(elsewhere.kind(), io::ErrorKind::NotFound);
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

        // A crash between reserving a temporary and publishing it leaves that
        // temporary behind — `Drop` does not run for a process that was killed.
        // It is nobody's record and the next owner releases it.
        let mut abandoned = PrivateTempFile::new_in(&directory).unwrap();
        abandoned
            .as_file_mut()
            .write_all(b"half a thought")
            .unwrap();
        let leftover = abandoned.path.clone();
        std::mem::forget(abandoned);
        assert!(leftover.exists());
        PrivateTempFile::clear_stale(&directory).unwrap();
        assert!(!leftover.exists());
        // The record beside it was never involved.
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
