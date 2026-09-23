//! Private local storage shared by native adapters. No authentication policy lives here.
//! Existing unsafe files are rejected, never silently repaired.
use std::{
    ffi::{OsStr, OsString},
    fs::File,
    io,
    path::{Component, Path, PathBuf},
};
mod retained_directory;
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
pub use retained_directory::{
    PrivateDirectory, PrivateDirectoryEntries, PrivateDirectoryEntry, PrivateDirectoryTempFile,
    PrivateFileIdentity, PrivateFileType, PrivatePublicationFailure, PrivatePublicationStage,
    PublishedPrivateFile,
};
// One owner: a temporary file is published by the type that reserved it, so
// that nothing can publish a name it did not create privately first.
pub(crate) use platform::publish_new;

const TEMPORARY_PREFIX: &str = ".nessa-";
const TEMPORARY_SUFFIX: &str = ".tmp";

/// Return whether a native name has the exact syntax used for private reservations.
///
/// A match proves syntax only. It does not prove that this process created the
/// entry or that the entry is safe to open, publish, or remove.
pub fn is_private_temporary_name(name: &OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    let Some(random) = name
        .strip_prefix(TEMPORARY_PREFIX)
        .and_then(|name| name.strip_suffix(TEMPORARY_SUFFIX))
    else {
        return false;
    };
    random.len() == 32
        && random
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn temporary_name() -> io::Result<OsString> {
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|error| io::Error::other(error.to_string()))?;
    let random: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(OsString::from(format!(
        "{TEMPORARY_PREFIX}{random}{TEMPORARY_SUFFIX}"
    )))
}

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
#[derive(Debug)]
struct UnsafeFile;

impl std::fmt::Display for UnsafeFile {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        output.write_str("local storage must be private and owned by the current OS user")
    }
}

impl std::error::Error for UnsafeFile {}

fn unsafe_file() -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, UnsafeFile)
}

/// Reports whether an I/O error proves that a local-storage object is unsafe.
///
/// Ordinary absence and operating-system availability failures are deliberately
/// distinct so callers may fall back only when no untrusted object was found.
pub fn is_unsafe_file(error: &io::Error) -> bool {
    if error
        .get_ref()
        .and_then(|cause| cause.downcast_ref::<UnsafeFile>())
        .is_some()
    {
        return true;
    }
    #[cfg(unix)]
    {
        matches!(error.raw_os_error(), Some(code) if code == libc::ELOOP || code == libc::ENOTDIR)
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// Create a private directory tree one anchored component at a time beneath a
/// trusted root.
///
/// The path must be non-empty, relative, and contain only normal components.
/// Each prefix is created through the platform's anchored `beneath` operation.
/// On Unix, its established parent is synced before descent and the final leaf
/// is synced before success. Windows has no directory-fsync equivalent, so the
/// same calls revalidate the private tree; a record publisher must separately
/// flush its file and use the platform's write-through move. This function does
/// not claim survival of arbitrary power loss.
pub fn create_private_directory_tree_beneath(root: &Path, directory: &Path) -> io::Result<()> {
    create_private_directory_tree_with(
        directory,
        |relative| create_directory_beneath(root, relative),
        |relative| sync_directory_beneath(root, relative),
    )
}

fn create_private_directory_tree_with(
    directory: &Path,
    mut create: impl FnMut(&Path) -> io::Result<()>,
    mut sync: impl FnMut(&Path) -> io::Result<()>,
) -> io::Result<()> {
    let mut relative = PathBuf::new();
    for component in directory.components() {
        let Component::Normal(name) = component else {
            return Err(unsafe_file());
        };
        let parent = relative.clone();
        relative.push(name);
        create(&relative)?;
        sync(&parent)?;
    }
    if relative.as_os_str().is_empty() {
        return Err(unsafe_file());
    }
    sync(&relative)
}

/// A temporary file protected at creation, before any secret bytes are written.
pub struct PrivateTempFile {
    file: Option<File>,
    path: PathBuf,
    beneath: Option<(PathBuf, PathBuf)>,
    cleanup_required: bool,
}
impl PrivateTempFile {
    pub fn new_in(parent: &Path) -> io::Result<Self> {
        verify_directory(parent)?;
        for _ in 0..10 {
            let path = parent.join(temporary_name()?);
            match platform::open_temporary(&path) {
                Ok(file) => {
                    return Ok(Self {
                        file: Some(file),
                        path,
                        beneath: None,
                        cleanup_required: true,
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
            let relative = directory.join(temporary_name()?);
            match platform::open_temporary_beneath(root, &relative) {
                Ok(file) => {
                    return Ok(Self {
                        file: Some(file),
                        path: root.join(&relative),
                        beneath: Some((root.to_path_buf(), relative)),
                        cleanup_required: true,
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
        self.file.as_ref().expect("temporary file is open")
    }
    pub fn as_file_mut(&mut self) -> &mut File {
        self.file.as_mut().expect("temporary file is open")
    }
    pub fn persist(mut self, destination: &Path) -> io::Result<()> {
        replace(&self.path, destination)?;
        self.cleanup_required = false;
        Ok(())
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
    pub fn publish(mut self, destination: &Path) -> io::Result<()> {
        publish_new(&self.path, destination)?;
        self.cleanup_required = false;
        Ok(())
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
    pub fn persist_beneath(mut self, destination: &Path) -> io::Result<()> {
        let (root, relative) = self.beneath.clone().ok_or_else(unsafe_file)?;
        match replace_beneath(&root, &relative, destination) {
            Ok(()) => {
                self.cleanup_required = false;
                Ok(())
            }
            Err(publication) => self.publication_failed(publication),
        }
    }

    /// Publish this file under a new relative name beneath the trusted root.
    ///
    /// The destination is never replaced. `AlreadyExists` means another owner
    /// already published that name. On any failure, anchored temporary cleanup
    /// is attempted and a cleanup failure is reported beside the publication
    /// failure.
    pub fn publish_new_beneath(mut self, destination: &Path) -> io::Result<()> {
        let (root, relative) = self.beneath.clone().ok_or_else(unsafe_file)?;
        match platform::publish_new_beneath(&root, &relative, destination) {
            Ok(()) => {
                self.cleanup_required = false;
                Ok(())
            }
            Err(publication) => self.publication_failed(publication),
        }
    }

    fn publication_failed(&mut self, publication: io::Error) -> io::Result<()> {
        match self.remove_reserved() {
            Ok(()) => {
                self.cleanup_required = false;
                drop(self.file.take());
                Err(publication)
            }
            Err(cleanup) => Err(io::Error::new(
                publication.kind(),
                format!(
                    "publication failed: {publication}; anchored temporary cleanup also failed: {cleanup}"
                ),
            )),
        }
    }

    fn remove_reserved(&self) -> io::Result<()> {
        match &self.beneath {
            Some((root, relative)) => platform::remove_reserved_beneath(
                self.file.as_ref().ok_or_else(unsafe_file)?,
                root,
                relative,
            ),
            None => std::fs::remove_file(&self.path),
        }
    }
}
impl Drop for PrivateTempFile {
    fn drop(&mut self) {
        if self.cleanup_required {
            // Drop cannot report cleanup failure. Anchored reservations never
            // fall back to an absolute path, so a swapped ancestor can retain
            // the original temporary but cannot redirect deletion elsewhere.
            let _ = self.remove_reserved();
        }
        drop(self.file.take());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn private_directory_tree_syncs_each_name_before_descent_and_the_leaf_before_success() {
        #[derive(Debug, PartialEq, Eq)]
        enum Step {
            Create(PathBuf),
            Sync(PathBuf),
        }
        let steps = std::cell::RefCell::new(Vec::new());

        create_private_directory_tree_with(
            Path::new("audit/refusals"),
            |path| {
                steps.borrow_mut().push(Step::Create(path.to_path_buf()));
                Ok(())
            },
            |path| {
                steps.borrow_mut().push(Step::Sync(path.to_path_buf()));
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(
            steps.into_inner(),
            vec![
                Step::Create(PathBuf::from("audit")),
                Step::Sync(PathBuf::new()),
                Step::Create(PathBuf::from("audit").join("refusals")),
                Step::Sync(PathBuf::from("audit")),
                Step::Sync(PathBuf::from("audit").join("refusals")),
            ]
        );
    }

    #[test]
    fn private_directory_tree_sync_failure_stops_before_creating_a_child() {
        let mut created = Vec::new();

        let error = create_private_directory_tree_with(
            Path::new("audit/refusals"),
            |path| {
                created.push(path.to_path_buf());
                Ok(())
            },
            |_| Err(io::Error::other("injected parent sync failure")),
        )
        .unwrap_err();

        assert_eq!(error.to_string(), "injected parent sync failure");
        assert_eq!(created, [PathBuf::from("audit")]);
    }

    #[test]
    fn anchored_publication_never_replaces_an_existing_record() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("root");
        create_directory(&root).unwrap();
        create_private_directory_tree_beneath(&root, Path::new("audit/refusals")).unwrap();
        let destination = Path::new("audit/refusals/record.json");

        let mut first = PrivateTempFile::new_beneath(&root, Path::new("audit/refusals")).unwrap();
        first.as_file_mut().write_all(b"first").unwrap();
        first.as_file().sync_all().unwrap();
        first.publish_new_beneath(destination).unwrap();

        let mut second = PrivateTempFile::new_beneath(&root, Path::new("audit/refusals")).unwrap();
        second.as_file_mut().write_all(b"second").unwrap();
        second.as_file().sync_all().unwrap();
        let error = second.publish_new_beneath(destination).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(root.join(destination)).unwrap(), b"first");
    }

    #[cfg(unix)]
    fn redirect_reservation_ancestry(
        temporary: &tempfile::TempDir,
        root: &Path,
        temp: &PrivateTempFile,
    ) -> (PathBuf, PathBuf) {
        use std::os::unix::fs::symlink;

        let relative = &temp.beneath.as_ref().unwrap().1;
        let held = root.join("held-audit");
        std::fs::rename(root.join("audit"), &held).unwrap();
        let outside = temporary.path().join("outside");
        create_directory(&outside).unwrap();
        create_directory(&outside.join("refusals")).unwrap();
        let marker = outside.join(relative.strip_prefix("audit").unwrap());
        std::fs::write(&marker, b"outside").unwrap();
        symlink(&outside, root.join("audit")).unwrap();

        assert_eq!(std::fs::read(&temp.path).unwrap(), b"outside");
        let control = outside.join("refusals/old-cleanup-control.tmp");
        std::fs::write(&control, b"control").unwrap();
        std::fs::remove_file(root.join("audit/refusals/old-cleanup-control.tmp")).unwrap();
        assert!(
            !control.exists(),
            "the negative control did not follow the swapped ancestry"
        );
        (held, marker)
    }

    #[cfg(unix)]
    #[test]
    fn anchored_drop_cannot_follow_replaced_ancestry_to_delete_outside() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("root");
        create_directory(&root).unwrap();
        create_private_directory_tree_beneath(&root, Path::new("audit/refusals")).unwrap();
        let mut temp = PrivateTempFile::new_beneath(&root, Path::new("audit/refusals")).unwrap();
        temp.as_file_mut().write_all(b"reserved").unwrap();
        let name = temp.path.file_name().unwrap().to_owned();
        let (held, marker) = redirect_reservation_ancestry(&temporary, &root, &temp);

        drop(temp);

        assert_eq!(std::fs::read(marker).unwrap(), b"outside");
        assert_eq!(
            std::fs::read(held.join("refusals").join(name)).unwrap(),
            b"reserved"
        );
    }

    #[cfg(unix)]
    #[test]
    fn failed_anchored_publication_reports_retained_cleanup_without_outside_delete() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("root");
        create_directory(&root).unwrap();
        create_private_directory_tree_beneath(&root, Path::new("audit/refusals")).unwrap();
        let mut temp = PrivateTempFile::new_beneath(&root, Path::new("audit/refusals")).unwrap();
        temp.as_file_mut().write_all(b"reserved").unwrap();
        let name = temp.path.file_name().unwrap().to_owned();
        let (held, marker) = redirect_reservation_ancestry(&temporary, &root, &temp);

        let error = temp
            .persist_beneath(Path::new("audit/refusals/record.json"))
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("anchored temporary cleanup also failed"),
            "{error}"
        );
        assert_eq!(std::fs::read(marker).unwrap(), b"outside");
        assert_eq!(
            std::fs::read(held.join("refusals").join(name)).unwrap(),
            b"reserved"
        );
    }

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
