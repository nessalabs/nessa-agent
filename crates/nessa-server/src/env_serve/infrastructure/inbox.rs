//! Where a lease's fetched files arrive, and how each is placed in the
//! workspace (issue #701, slice D3b).
//!
//! ```text
//! <data>/environment/inbox/            swept when serving starts
//!   <lease's digest>/<fetch>.part      0600, written by the gateway over sftp
//!
//! destination(path) ──▶ the workspace's when relative ──▶ its last part a name?
//!   ──▶ its directory, every link followed ──▶ inside the workspace?
//!   ──▶ that directory opened without following a last link, the one checked?
//!   ──▶ the name opened in it without following a link:
//!         nothing ──▶ free;  exactly this file ──▶ already placed;  else occupied
//! place(part, destination) ──▶ the part opened once: size, SHA-256
//!   ──▶ its mode set to what a file this user makes gets (0666 less the umask)
//!   ──▶ the destination resolved again ──▶ link that opened file at the name
//!         (through /proc/self/fd where there is one, else by the part's path)
//!   ──▶ what is at the name now is that very file? else unlinked: mismatch
//!         something there ──▶ exactly this file? already placed : occupied
//!         another file system ──▶ a copy of the opened file beside it,
//!                                 checked as it is copied, then linked the same way
//!   ──▶ the part let go
//! ```
//!
//! Arrows are steps, in order. A destination is bounded as the outbox bounds
//! a path to publish: resolved inside the workspace, its last part never a
//! link, and everything after the check done through the directory that was
//! checked, never its path. A link never replaces anything: `link` refuses a
//! name that exists, whatever it is, so a file is only ever put where
//! nothing was. What is linked is the very file that was hashed: linked by
//! its descriptor where the system allows, and otherwise by path and then
//! held to that file by device and inode; the inbox is a directory only
//! this user may enter. A placed file has the mode this user's other new
//! files get, read from the umask once at start ([`note_umask`]).
use crate::env_serve::application::{ArtifactInbox, Destination, FetchedFile};
use nessa_protocol::lease::{FetchOutcome, FetchRefusal, MAX_ARTIFACT_PATH_BYTES};
use sha2::{Digest, Sha256};
use std::{
    ffi::{CString, OsStr},
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, Write},
    os::unix::{
        ffi::OsStrExt,
        fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        io::{AsRawFd, FromRawFd},
    },
    path::{Component, Path, PathBuf},
    sync::OnceLock,
};

/// This process's umask, read once while it was the only thread.
static PROCESS_UMASK: OnceLock<u32> = OnceLock::new();

/// Read this process's umask, for the mode of files the inbox places.
/// Reading it means setting it, so it is set back at once; called before
/// any other thread exists, so nothing else creates a file meanwhile.
pub(crate) fn note_umask() {
    PROCESS_UMASK.get_or_init(|| {
        // SAFETY: `umask` only swaps the process's mask; it is put back at
        // once, before any other thread runs.
        let mask = unsafe {
            let mask = libc::umask(0o077);
            libc::umask(mask);
            mask
        };
        #[allow(clippy::useless_conversion, reason = "mode_t is u16 on macOS")]
        u32::from(mask)
    });
}

/// The mode a placed file gets: what this user's other new files get, or
/// 0644 when the umask was never read.
fn placed_mode() -> u32 {
    PROCESS_UMASK
        .get()
        .map_or(0o644, |mask| 0o666 & !mask & 0o777)
}

/// How much is read or copied at a time.
const CHUNK: usize = 64 * 1024;

/// The inbox on this host's file system.
pub(crate) struct FileInbox {
    root: PathBuf,
    workspace: PathBuf,
}

impl FileInbox {
    /// The inbox at `root` for files placed in `workspace`. Whatever an
    /// earlier serving process left in `root` is let go first: every lease
    /// it took files for has ended, as one process serves this data
    /// directory at a time.
    ///
    /// # Errors
    /// The inbox or the workspace could not be made ready.
    pub(crate) fn open(root: PathBuf, workspace: &Path) -> io::Result<Self> {
        match fs::remove_dir_all(&root) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&root)?;
        Ok(Self {
            root,
            workspace: fs::canonicalize(workspace)?,
        })
    }

    /// Named by the lease's digest, never by what the gateway sent: a
    /// lease is any text, and never a path.
    fn lease_directory(&self, lease: &str) -> PathBuf {
        self.root.join(hex(&Sha256::digest(lease.as_bytes())))
    }

    fn part(&self, lease: &str, fetch: u32) -> PathBuf {
        self.lease_directory(lease).join(format!("{fetch}.part"))
    }

    /// `destination`'s directory, opened as it was checked, and its name.
    fn resolve(&self, destination: &str) -> Result<Resolved, FetchRefusal> {
        if destination.is_empty() || destination.len() > MAX_ARTIFACT_PATH_BYTES {
            return Err(FetchRefusal::Invalid);
        }
        let asked = self.workspace.join(destination);
        let name = match asked.components().next_back() {
            Some(Component::Normal(name)) => name.to_owned(),
            // `..`, `.`, or the root: not a name a file can be put at.
            _ => return Err(FetchRefusal::OutsideWorkspace),
        };
        let parent = asked.parent().ok_or(FetchRefusal::OutsideWorkspace)?;
        let directory = match fs::canonicalize(parent) {
            Ok(directory) => directory,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(FetchRefusal::NoDirectory)
            }
            Err(_) => return Err(FetchRefusal::PlacingFailed),
        };
        if !directory.starts_with(&self.workspace) {
            return Err(FetchRefusal::OutsideWorkspace);
        }
        let checked = fs::metadata(&directory).map_err(|_| FetchRefusal::NoDirectory)?;
        if !checked.is_dir() {
            return Err(FetchRefusal::NoDirectory);
        }
        // Its last part is not followed, and what opened is the directory
        // that was checked: a link swapped in between is refused.
        let opened = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(&directory)
            .map_err(|_| FetchRefusal::OutsideWorkspace)?;
        let found = opened.metadata().map_err(|_| FetchRefusal::PlacingFailed)?;
        if !found.is_dir() || found.dev() != checked.dev() || found.ino() != checked.ino() {
            return Err(FetchRefusal::OutsideWorkspace);
        }
        let path = directory.join(&name);
        let path = path
            .to_str()
            .filter(|path| path.len() <= MAX_ARTIFACT_PATH_BYTES)
            .ok_or(FetchRefusal::Invalid)?
            .to_owned();
        Ok(Resolved {
            directory: opened,
            name: c_name(&name)?,
            path,
        })
    }

    /// Place `opened`, the checked part, at `resolved`, copying it beside
    /// the destination first when the two are on different file systems.
    /// What is linked is held to be `opened` itself.
    fn link(
        &self,
        part: &Path,
        opened: &mut File,
        resolved: &Resolved,
        file: &FetchedFile,
    ) -> Result<FetchOutcome, FetchRefusal> {
        let linked = match link_descriptor(opened, &resolved.directory, &resolved.name) {
            Some(linked) => linked,
            None => link_at(part, &resolved.directory, &resolved.name),
        };
        match linked {
            Ok(()) => same_file(resolved, opened).map(|()| FetchOutcome::Placed {
                path: resolved.path.clone(),
            }),
            Err(error) if error.raw_os_error() == Some(libc::EEXIST) => {
                taken(resolved, file).map(|()| FetchOutcome::AlreadyPlaced {
                    path: resolved.path.clone(),
                })
            }
            Err(error) if error.raw_os_error() == Some(libc::EXDEV) => {
                self.copy_beside(opened, resolved, file)
            }
            Err(_) => Err(FetchRefusal::PlacingFailed),
        }
    }

    /// Copy `part` to a new file beside the destination, checked as it is
    /// copied, and link that at the destination: a link within one
    /// directory, which never replaces anything either.
    fn copy_beside(
        &self,
        source: &mut File,
        resolved: &Resolved,
        file: &FetchedFile,
    ) -> Result<FetchOutcome, FetchRefusal> {
        let temporary = c_name(OsStr::new(&format!(
            ".nessa-fetch-{}",
            uuid::Uuid::new_v4().simple()
        )))?;
        let mut copy = open_at(
            &resolved.directory,
            &temporary,
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW,
            0o600,
        )
        .map_err(|_| FetchRefusal::PlacingFailed)?;
        let copied = (|| {
            source
                .seek(io::SeekFrom::Start(0))
                .map_err(|_| FetchRefusal::PlacingFailed)?;
            let (size, digest) = hashed(source, Some(&mut copy))?;
            if size != file.size || digest != file.digest {
                return Err(FetchRefusal::Mismatch);
            }
            copy.set_permissions(fs::Permissions::from_mode(placed_mode()))
                .map_err(|_| FetchRefusal::PlacingFailed)?;
            copy.sync_all().map_err(|_| FetchRefusal::PlacingFailed)?;
            match link_within(&resolved.directory, &temporary, &resolved.name) {
                Ok(()) => same_file(resolved, &copy).map(|()| FetchOutcome::Placed {
                    path: resolved.path.clone(),
                }),
                Err(error) if error.raw_os_error() == Some(libc::EEXIST) => taken(resolved, file)
                    .map(|()| FetchOutcome::AlreadyPlaced {
                        path: resolved.path.clone(),
                    }),
                Err(_) => Err(FetchRefusal::PlacingFailed),
            }
        })();
        if let Err(error) = unlink_at(&resolved.directory, &temporary) {
            tracing::warn!(%error, "a fetch's copy beside its destination could not be removed");
        }
        copied
    }
}

/// A destination: the directory it is in, opened, and its name there.
struct Resolved {
    directory: File,
    name: CString,
    /// Absolute, for the gateway.
    path: String,
}

impl ArtifactInbox for FileInbox {
    fn destination(
        &self,
        destination: &str,
        file: &FetchedFile,
    ) -> Result<Destination, FetchRefusal> {
        let resolved = self.resolve(destination)?;
        match open_at(
            &resolved.directory,
            &resolved.name,
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK,
            0,
        ) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Destination::Free {
                path: resolved.path,
            }),
            _ => taken(&resolved, file).map(|()| Destination::AlreadyPlaced {
                path: resolved.path,
            }),
        }
    }

    fn open_part(&self, lease: &str, fetch: u32) -> Result<String, FetchRefusal> {
        let directory = self.lease_directory(lease);
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&directory)
            .map_err(|_| FetchRefusal::PlacingFailed)?;
        let part = self.part(lease, fetch);
        let path = part
            .to_str()
            .filter(|path| path.len() <= MAX_ARTIFACT_PATH_BYTES)
            .ok_or(FetchRefusal::PlacingFailed)?
            .to_owned();
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&part)
            .map_err(|_| FetchRefusal::PlacingFailed)?;
        Ok(path)
    }

    fn place(
        &self,
        lease: &str,
        fetch: u32,
        destination: &str,
        file: &FetchedFile,
    ) -> FetchOutcome {
        let part = self.part(lease, fetch);
        let placed = (|| {
            let mut opened = open_nofollow(&part).map_err(|_| FetchRefusal::PlacingFailed)?;
            let (size, digest) = hashed(&mut opened, None)?;
            if size != file.size || digest != file.digest {
                return Err(FetchRefusal::Mismatch);
            }
            opened
                .set_permissions(fs::Permissions::from_mode(placed_mode()))
                .map_err(|_| FetchRefusal::PlacingFailed)?;
            let resolved = self.resolve(destination)?;
            let outcome = self.link(&part, &mut opened, &resolved, file)?;
            let _ = resolved.directory.sync_all();
            Ok(outcome)
        })();
        self.discard(lease, fetch);
        placed.unwrap_or_else(|reason| FetchOutcome::Refused { reason })
    }

    fn discard(&self, lease: &str, fetch: u32) {
        if let Err(error) = fs::remove_file(self.part(lease, fetch)) {
            if error.kind() != io::ErrorKind::NotFound {
                tracing::warn!(lease, fetch, %error, "a fetch's part could not be removed");
            }
        }
    }

    fn close(&self, lease: &str) {
        match fs::remove_dir_all(self.lease_directory(lease)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                tracing::warn!(lease, %error, "a lease's fetched parts could not be removed");
            }
        }
    }
}

/// Whether what was just linked at `resolved` is `linked` itself, by device
/// and inode. When it is not (the part was swapped between its check and
/// the link), what was linked is taken back and the fetch is a mismatch.
fn same_file(resolved: &Resolved, linked: &File) -> Result<(), FetchRefusal> {
    let ours = linked.metadata().map_err(|_| FetchRefusal::PlacingFailed)?;
    let there = open_at(
        &resolved.directory,
        &resolved.name,
        libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK,
        0,
    )
    .and_then(|there| there.metadata());
    match there {
        Ok(there) if there.dev() == ours.dev() && there.ino() == ours.ino() => Ok(()),
        _ => {
            if let Err(error) = unlink_at(&resolved.directory, &resolved.name) {
                tracing::warn!(%error, "a fetch linked as another file could not be taken back");
            }
            Err(FetchRefusal::Mismatch)
        }
    }
}

/// Whether what is at `resolved` is exactly `file`: `Ok` when it is,
/// `Occupied` when anything else is there, a link included.
fn taken(resolved: &Resolved, file: &FetchedFile) -> Result<(), FetchRefusal> {
    let mut there = open_at(
        &resolved.directory,
        &resolved.name,
        libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK,
        0,
    )
    .map_err(|_| FetchRefusal::Occupied)?;
    let metadata = there.metadata().map_err(|_| FetchRefusal::Occupied)?;
    if !metadata.is_file() || metadata.len() != file.size {
        return Err(FetchRefusal::Occupied);
    }
    match hashed(&mut there, None) {
        Ok((size, digest)) if size == file.size && digest == file.digest => Ok(()),
        _ => Err(FetchRefusal::Occupied),
    }
}

/// The length and SHA-256 of all of `source`, as it is read, copying it
/// into `copy` when one is given.
fn hashed(source: &mut File, mut copy: Option<&mut File>) -> Result<(u64, String), FetchRefusal> {
    let mut hash = Sha256::new();
    let mut size = 0u64;
    let mut buffer = vec![0; CHUNK];
    loop {
        let read = match source.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(FetchRefusal::PlacingFailed),
        };
        size += read as u64;
        hash.update(&buffer[..read]);
        if let Some(copy) = copy.as_deref_mut() {
            copy.write_all(&buffer[..read])
                .map_err(|_| FetchRefusal::PlacingFailed)?;
        }
    }
    Ok((size, hex(&hash.finalize())))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn open_nofollow(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}

fn c_name(name: &OsStr) -> Result<CString, FetchRefusal> {
    CString::new(name.as_bytes()).map_err(|_| FetchRefusal::Invalid)
}

/// `openat(directory, name, flags, mode)`, close-on-exec.
fn open_at(
    directory: &File,
    name: &CString,
    flags: libc::c_int,
    mode: libc::c_uint,
) -> io::Result<File> {
    // SAFETY: `name` is a NUL-terminated string that outlives the call, and
    // `directory` is an open descriptor; a descriptor returned is owned by
    // nothing else and is handed to the `File` alone.
    let descriptor = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_CLOEXEC,
            mode,
        )
    };
    if descriptor < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: just opened, and owned by nothing else.
    Ok(unsafe { File::from_raw_fd(descriptor) })
}

/// `linkat(AT_FDCWD, from, directory, name, 0)`: never follows `from`'s
/// last link, never replaces anything at `name`.
fn link_at(from: &Path, directory: &File, name: &CString) -> io::Result<()> {
    let from = CString::new(from.as_os_str().as_bytes())?;
    // SAFETY: both strings are NUL-terminated and outlive the call, and
    // `directory` is an open descriptor.
    let linked = unsafe {
        libc::linkat(
            libc::AT_FDCWD,
            from.as_ptr(),
            directory.as_raw_fd(),
            name.as_ptr(),
            0,
        )
    };
    if linked == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Link the open `file` itself at `name` in `directory`, through
/// `/proc/self/fd`, so no path can stand in for it. `None` where that is not
/// to be had (no `/proc`, or a system that refuses it): link by path then.
#[cfg(target_os = "linux")]
fn link_descriptor(file: &File, directory: &File, name: &CString) -> Option<io::Result<()>> {
    let from = CString::new(format!("/proc/self/fd/{}", file.as_raw_fd())).ok()?;
    // SAFETY: as `link_at`; `file` is open for the whole call.
    let linked = unsafe {
        libc::linkat(
            libc::AT_FDCWD,
            from.as_ptr(),
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::AT_SYMLINK_FOLLOW,
        )
    };
    if linked == 0 {
        return Some(Ok(()));
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::EEXIST | libc::EXDEV) => Some(Err(error)),
        _ => None,
    }
}

#[cfg(not(target_os = "linux"))]
fn link_descriptor(_file: &File, _directory: &File, _name: &CString) -> Option<io::Result<()>> {
    None
}

/// `linkat(directory, from, directory, to, 0)`.
fn link_within(directory: &File, from: &CString, to: &CString) -> io::Result<()> {
    // SAFETY: as `link_at`.
    let linked = unsafe {
        libc::linkat(
            directory.as_raw_fd(),
            from.as_ptr(),
            directory.as_raw_fd(),
            to.as_ptr(),
            0,
        )
    };
    if linked == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// `unlinkat(directory, name, 0)`.
fn unlink_at(directory: &File, name: &CString) -> io::Result<()> {
    // SAFETY: as `link_at`.
    let removed = unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) };
    if removed == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(test)]
#[path = "../../../tests/env_serve/inbox.rs"]
mod tests;
