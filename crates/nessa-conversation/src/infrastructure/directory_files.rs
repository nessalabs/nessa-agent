//! No-follow walks from a directory handle. A path is never resolved and then opened.

#![cfg(unix)]

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};

use crate::application::{DirectoryFiles, DirectoryRefusal, DirectorySession, MarkerBytes};
use crate::domain::{PathLeave, SandboxKind, WalkFacts, BINDING_LEN, MARKER_NAME};

/// The local machine's directories.
#[derive(Clone, Copy, Debug, Default)]
pub struct LocalDirectoryFiles;

impl DirectoryFiles for LocalDirectoryFiles {
    fn canonicalize(&self, path: &str) -> Result<String, DirectoryRefusal> {
        let absolute = absolute(path)?;
        let resolved = std::fs::canonicalize(&absolute).map_err(io)?;
        resolved
            .to_str()
            .map(str::to_owned)
            .ok_or(DirectoryRefusal::Io(std::io::ErrorKind::InvalidData))
    }

    fn absolute_spelling(&self, path: &str) -> Result<String, DirectoryRefusal> {
        absolute(path)
    }

    fn read_marker(&self, canonical: &str) -> Result<MarkerBytes, DirectoryRefusal> {
        let root = open_directory(canonical)?;
        match open_marker_file(root.as_raw_fd()) {
            Ok(file) => read_marker_bytes(&file),
            Err(DirectoryRefusal::Io(std::io::ErrorKind::NotFound)) => Ok(MarkerBytes::Absent),
            Err(DirectoryRefusal::Io(std::io::ErrorKind::InvalidInput)) => {
                Ok(MarkerBytes::Unusable)
            }
            Err(error) => Err(error),
        }
    }

    fn create_marker(
        &self,
        canonical: &str,
        bytes: &[u8; BINDING_LEN],
    ) -> Result<(), DirectoryRefusal> {
        let root = open_directory(canonical)?;
        let flags =
            libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_WRONLY | libc::O_CLOEXEC;
        let marker = c_name(MARKER_NAME)?;
        let fd = unsafe { libc::openat(root.as_raw_fd(), marker.as_ptr(), flags, 0o600) };
        if fd < 0 {
            let error = std::io::Error::last_os_error();
            // `O_EXCL` reports an existing symlink as `EEXIST`, not `ELOOP`.
            // The name was not opened, so it is the same boundary refusal.
            if error.raw_os_error() == Some(libc::EEXIST) && is_symlink(root.as_raw_fd(), &marker) {
                return Err(DirectoryRefusal::Outside {
                    leave: PathLeave::Symlink,
                    audit_recorded: true,
                });
            }
            return Err(no_follow_boundary(root.as_raw_fd(), &marker, error));
        }
        let file = unsafe { File::from_raw_fd(fd) };
        if let Err(error) = (&file).write_all(bytes) {
            let _ = remove_validated(root.as_raw_fd(), &file);
            return Err(io(error));
        }
        Ok(())
    }

    fn release_created_marker(
        &self,
        canonical: &str,
        binding: &[u8; BINDING_LEN],
    ) -> Result<bool, DirectoryRefusal> {
        let root = match open_directory(canonical) {
            Ok(root) => root,
            Err(DirectoryRefusal::Io(std::io::ErrorKind::NotFound)) => return Ok(true),
            Err(error) => return Err(error),
        };
        let marker = match open_marker_file(root.as_raw_fd()) {
            Ok(file) => file,
            Err(DirectoryRefusal::Io(
                std::io::ErrorKind::NotFound | std::io::ErrorKind::InvalidInput,
            )) => return Ok(true),
            Err(error) => return Err(error),
        };
        let mut bytes = Vec::new();
        (&marker)
            .take(BINDING_LEN as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(io)?;
        if bytes.as_slice() != binding.as_slice() {
            return Ok(true);
        }
        remove_validated(root.as_raw_fd(), &marker)?;
        Ok(true)
    }

    fn unlink_marker(&self, canonical: &str) -> Result<(), DirectoryRefusal> {
        let root = open_directory(canonical)?;
        let marker = c_name(MARKER_NAME)?;
        let result = unsafe { libc::unlinkat(root.as_raw_fd(), marker.as_ptr(), 0) };
        if result == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::NotFound {
            return Ok(());
        }
        Err(io(error))
    }

    fn walk(&self, canonical: &str, relative: &str) -> Result<WalkFacts, DirectoryRefusal> {
        let root = open_directory(canonical)?;
        let (file, leave) = walk_from(root.as_raw_fd(), relative)?;
        let marker = file
            .as_ref()
            .is_some_and(|opened| is_marker(root.as_raw_fd(), opened.as_raw_fd()));
        Ok(WalkFacts { leave, marker })
    }

    fn read_inside(&self, canonical: &str, relative: &str) -> Result<Vec<u8>, DirectoryRefusal> {
        let root = open_directory(canonical)?;
        let (file, leave) = walk_from(root.as_raw_fd(), relative)?;
        if leave != PathLeave::Inside {
            return Err(DirectoryRefusal::Outside {
                leave,
                audit_recorded: true,
            });
        }
        let mut file = file.ok_or(DirectoryRefusal::Outside {
            leave: PathLeave::Missing,
            audit_recorded: true,
        })?;
        if is_marker(root.as_raw_fd(), file.as_raw_fd()) {
            return Err(DirectoryRefusal::MarkerName {
                audit_recorded: true,
            });
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).map_err(io)?;
        Ok(bytes)
    }

    fn write_inside(
        &self,
        canonical: &str,
        relative: &str,
        bytes: &[u8],
    ) -> Result<(), DirectoryRefusal> {
        let root = open_directory(canonical)?;
        let (existing, leave) = walk_from(root.as_raw_fd(), relative)?;
        if leave != PathLeave::Inside && leave != PathLeave::Missing {
            return Err(DirectoryRefusal::Outside {
                leave,
                audit_recorded: true,
            });
        }
        if existing
            .as_ref()
            .is_some_and(|opened| is_marker(root.as_raw_fd(), opened.as_raw_fd()))
        {
            return Err(DirectoryRefusal::MarkerName {
                audit_recorded: true,
            });
        }
        let parent = parent_file(root.as_raw_fd(), relative)?;
        let name = final_name(relative)?;
        let mut file = open_write_nofollow(parent.as_raw_fd(), &name)?;
        if is_marker(root.as_raw_fd(), file.as_raw_fd()) {
            return Err(DirectoryRefusal::MarkerName {
                audit_recorded: true,
            });
        }
        if unsafe { libc::ftruncate(file.as_raw_fd(), 0) } != 0 {
            return Err(io(std::io::Error::last_os_error()));
        }
        file.write_all(bytes).map_err(io)?;
        Ok(())
    }

    fn hides_marker(&self, _profile: SandboxKind) -> bool {
        // No sandbox on this machine is configured to exclude `.nessa-binding`.
        // The fact stays false until an enforcing sandbox reports it.
        false
    }

    fn root_leave(&self, canonical: &str, roots: &[String]) -> Result<PathLeave, DirectoryRefusal> {
        self.open_session(canonical)?.root_leave(canonical, roots)
    }

    fn open_session(&self, canonical: &str) -> Result<Box<dyn DirectorySession>, DirectoryRefusal> {
        Ok(Box::new(HeldDirectory {
            root: open_directory(canonical)?,
            marker: std::sync::Mutex::new(None),
        }))
    }
}

struct HeldDirectory {
    root: File,
    /// The marker file validated for this open. Later names are compared to
    /// this file, not looked up again by `.nessa-binding`.
    marker: std::sync::Mutex<Option<File>>,
}

impl DirectorySession for HeldDirectory {
    fn marker(&self) -> Result<MarkerBytes, DirectoryRefusal> {
        let mut held = self.marker.lock().expect("marker");
        if held.is_none() {
            match open_marker_file(self.root.as_raw_fd()) {
                Ok(file) => *held = Some(file),
                Err(DirectoryRefusal::Io(std::io::ErrorKind::NotFound)) => {
                    return Ok(MarkerBytes::Absent)
                }
                Err(DirectoryRefusal::Io(std::io::ErrorKind::InvalidInput)) => {
                    return Ok(MarkerBytes::Unusable)
                }
                Err(error) => return Err(error),
            }
        }
        let marker = held.as_ref().expect("the marker file is held");
        if unsafe { libc::lseek(marker.as_raw_fd(), 0, libc::SEEK_SET) } < 0 {
            return Err(io(std::io::Error::last_os_error()));
        }
        read_marker_bytes(marker)
    }

    fn walk(&self, relative: &str) -> Result<WalkFacts, DirectoryRefusal> {
        let (file, leave) = walk_from(self.root.as_raw_fd(), relative)?;
        let marker = file
            .as_ref()
            .is_some_and(|opened| self.same_marker(opened.as_raw_fd()));
        Ok(WalkFacts { leave, marker })
    }

    fn read(&self, relative: &str) -> Result<Vec<u8>, DirectoryRefusal> {
        let (file, leave) = walk_from(self.root.as_raw_fd(), relative)?;
        if leave != PathLeave::Inside {
            return Err(DirectoryRefusal::Outside {
                leave,
                audit_recorded: true,
            });
        }
        let mut file = file.ok_or(DirectoryRefusal::Outside {
            leave: PathLeave::Missing,
            audit_recorded: true,
        })?;
        if self.same_marker(file.as_raw_fd()) {
            return Err(DirectoryRefusal::MarkerName {
                audit_recorded: true,
            });
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).map_err(io)?;
        Ok(bytes)
    }

    fn write(&self, relative: &str, bytes: &[u8]) -> Result<(), DirectoryRefusal> {
        let (existing, leave) = walk_from(self.root.as_raw_fd(), relative)?;
        if leave != PathLeave::Inside && leave != PathLeave::Missing {
            return Err(DirectoryRefusal::Outside {
                leave,
                audit_recorded: true,
            });
        }
        if existing
            .as_ref()
            .is_some_and(|opened| self.same_marker(opened.as_raw_fd()))
        {
            return Err(DirectoryRefusal::MarkerName {
                audit_recorded: true,
            });
        }
        let parent = parent_file(self.root.as_raw_fd(), relative)?;
        let name = final_name(relative)?;
        let mut file = open_write_nofollow(parent.as_raw_fd(), &name)?;
        if self.same_marker(file.as_raw_fd()) {
            return Err(DirectoryRefusal::MarkerName {
                audit_recorded: true,
            });
        }
        if unsafe { libc::ftruncate(file.as_raw_fd(), 0) } != 0 {
            return Err(io(std::io::Error::last_os_error()));
        }
        file.write_all(bytes).map_err(io)?;
        Ok(())
    }

    fn root_leave(&self, canonical: &str, roots: &[String]) -> Result<PathLeave, DirectoryRefusal> {
        if roots.is_empty() {
            return Ok(PathLeave::Outside);
        }
        let mut leave = PathLeave::Inside;
        for root in roots {
            // The grant stores the canonical directory. A sandbox root may name
            // that same directory through an ancestor symlink (`/var` on macOS
            // is `/private/var`). Resolve it before the prefix comparison.
            let absolute = absolute(root)?;
            let resolved = std::fs::canonicalize(&absolute).map_err(io)?;
            let absolute = resolved
                .to_str()
                .ok_or(DirectoryRefusal::Io(std::io::ErrorKind::InvalidData))?
                .to_owned();
            let Some(rest) = relative_inside(canonical, &absolute) else {
                return Ok(PathLeave::Outside);
            };
            if rest.is_empty() {
                if !same_directory(self.root.as_raw_fd(), &absolute)? {
                    return Ok(PathLeave::Outside);
                }
                continue;
            }
            let facts = self.walk(rest)?;
            if facts.leave != PathLeave::Inside {
                leave = facts.leave;
            }
        }
        Ok(leave)
    }
}

impl HeldDirectory {
    fn same_marker(&self, opened: RawFd) -> bool {
        let held = self.marker.lock().expect("marker");
        held.as_ref()
            .is_some_and(|marker| same_inode(marker.as_raw_fd(), opened))
    }
}

fn read_marker_bytes(marker: &File) -> Result<MarkerBytes, DirectoryRefusal> {
    let mut bytes = Vec::new();
    marker
        .take(BINDING_LEN as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(io)?;
    if bytes.len() == BINDING_LEN {
        let mut exact = [0; BINDING_LEN];
        exact.copy_from_slice(&bytes);
        Ok(MarkerBytes::Exact(exact))
    } else {
        Ok(MarkerBytes::Other)
    }
}

fn relative_inside<'a>(canonical: &str, absolute: &'a str) -> Option<&'a str> {
    let canonical = canonical.trim_end_matches('/');
    if canonical.is_empty() {
        return Some(absolute.trim_start_matches('/'));
    }
    if absolute == canonical {
        return Some("");
    }
    let mut prefix = String::from(canonical);
    prefix.push('/');
    absolute.strip_prefix(&prefix)
}

fn same_directory(root: RawFd, path: &str) -> Result<bool, DirectoryRefusal> {
    let opened = open_directory(path)?;
    Ok(same_inode(root, opened.as_raw_fd()))
}

fn same_inode(left: RawFd, right: RawFd) -> bool {
    let mut left_stat = unsafe { std::mem::zeroed() };
    let mut right_stat = unsafe { std::mem::zeroed() };
    let left_ok = unsafe { libc::fstat(left, &mut left_stat) } == 0;
    let right_ok = unsafe { libc::fstat(right, &mut right_stat) } == 0;
    left_ok
        && right_ok
        && left_stat.st_dev == right_stat.st_dev
        && left_stat.st_ino == right_stat.st_ino
}

fn absolute(path: &str) -> Result<String, DirectoryRefusal> {
    let path = if path.starts_with('/') {
        path.to_owned()
    } else {
        let current = std::env::current_dir().map_err(io)?;
        format!("{}/{}", current.display(), path)
    };
    let mut out = String::new();
    for component in path
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
    {
        if component == ".." {
            return Err(DirectoryRefusal::Outside {
                leave: PathLeave::DotDot,
                audit_recorded: true,
            });
        }
        out.push('/');
        out.push_str(component);
    }
    if out.is_empty() {
        out.push('/');
    }
    Ok(out)
}

fn open_directory(path: &str) -> Result<File, DirectoryRefusal> {
    // The owner named this path. An ancestor may be a symlink, as `/var` is
    // on macOS. Following those ancestors reaches the directory. Every name
    // inside it is opened from the resulting handle without following links.
    let absolute = absolute(path)?;
    let name = c_name(&absolute)?;
    let fd = unsafe {
        libc::open(
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(io(std::io::Error::last_os_error()));
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn walk_from(root: RawFd, relative: &str) -> Result<(Option<File>, PathLeave), DirectoryRefusal> {
    if relative.is_empty() || relative == "." {
        return Ok((None, PathLeave::Inside));
    }
    let mut current = root;
    let mut owned: Option<File> = None;
    let parts: Vec<&str> = relative
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    if parts.is_empty() {
        return Ok((None, PathLeave::Inside));
    }
    for (index, component) in parts.iter().enumerate() {
        if *component == ".." {
            return Ok((None, PathLeave::DotDot));
        }
        let last = index + 1 == parts.len();
        match open_at(current, component, !last) {
            Ok(file) => {
                current = file.as_raw_fd();
                owned = Some(file);
            }
            Err(DirectoryRefusal::Io(std::io::ErrorKind::NotFound)) => {
                return Ok((None, PathLeave::Missing));
            }
            Err(DirectoryRefusal::Outside { leave, .. }) => return Ok((None, leave)),
            Err(error) => return Err(error),
        }
    }
    Ok((owned, PathLeave::Inside))
}

fn parent_file(root: RawFd, relative: &str) -> Result<File, DirectoryRefusal> {
    let parts: Vec<&str> = relative
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    if parts.len() <= 1 {
        let fd = unsafe { libc::dup(root) };
        if fd < 0 {
            return Err(io(std::io::Error::last_os_error()));
        }
        return Ok(unsafe { File::from_raw_fd(fd) });
    }
    let parent = parts[..parts.len() - 1].join("/");
    let (file, leave) = walk_from(root, &parent)?;
    if leave != PathLeave::Inside {
        return Err(DirectoryRefusal::Outside {
            leave,
            audit_recorded: true,
        });
    }
    file.ok_or(DirectoryRefusal::Outside {
        leave: PathLeave::Missing,
        audit_recorded: true,
    })
}

fn final_name(relative: &str) -> Result<String, DirectoryRefusal> {
    relative
        .split('/')
        .rfind(|part| !part.is_empty() && *part != ".")
        .map(str::to_owned)
        .ok_or(DirectoryRefusal::Outside {
            leave: PathLeave::Missing,
            audit_recorded: true,
        })
}

fn open_marker_file(dir: RawFd) -> Result<File, DirectoryRefusal> {
    let file = match open_named(dir, MARKER_NAME) {
        Ok(file) => file,
        Err(error) => {
            // A socket's open fails with `ENXIO` before an fd exists to
            // `fstat`. A symlink fails with `ELOOP`. Any name that is not a
            // regular file is the same refusal as a FIFO; a missing name or a
            // transient open error stays that error.
            if marker_is_not_regular(dir) {
                return Err(DirectoryRefusal::Io(std::io::ErrorKind::InvalidInput));
            }
            return Err(error);
        }
    };
    if is_regular(&file)? {
        Ok(file)
    } else {
        Err(DirectoryRefusal::Io(std::io::ErrorKind::InvalidInput))
    }
}

/// The marker name exists and is not a regular file: a socket, FIFO, device,
/// directory, or symlink.
fn marker_is_not_regular(dir: RawFd) -> bool {
    let Ok(name) = c_name(MARKER_NAME) else {
        return false;
    };
    let mut info: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstatat(dir, name.as_ptr(), &mut info, libc::AT_SYMLINK_NOFOLLOW) } != 0 {
        return false;
    }
    mode_bits(info.st_mode) & mode_bits(libc::S_IFMT) != mode_bits(libc::S_IFREG)
}

fn open_named(dir: RawFd, name: &str) -> Result<File, DirectoryRefusal> {
    let flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK;
    let name = c_name(name)?;
    let fd = unsafe { libc::openat(dir, name.as_ptr(), flags) };
    if fd < 0 {
        return Err(io(std::io::Error::last_os_error()));
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn is_regular(file: &File) -> Result<bool, DirectoryRefusal> {
    Ok(file_kind(file)? == mode_bits(libc::S_IFREG))
}

fn is_walk_target(file: &File) -> Result<bool, DirectoryRefusal> {
    let kind = file_kind(file)?;
    Ok(kind == mode_bits(libc::S_IFREG) || kind == mode_bits(libc::S_IFDIR))
}

fn file_kind(file: &File) -> Result<u32, DirectoryRefusal> {
    let mut info = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(file.as_raw_fd(), &mut info) } != 0 {
        return Err(io(std::io::Error::last_os_error()));
    }
    Ok(mode_bits(info.st_mode) & mode_bits(libc::S_IFMT))
}

/// `mode_t` is `u32` on Linux and `u16` on macOS. Both fit in `u32`.
fn mode_bits(mode: impl Into<u32>) -> u32 {
    mode.into()
}

/// Move `.nessa-binding` aside, then delete it only when that file is the one
/// already validated. A different entry, including one that cannot be opened
/// (a symlink or a socket), is renamed back and not removed.
fn remove_validated(dir: RawFd, validated: &File) -> Result<(), DirectoryRefusal> {
    let aside = match reserve_aside(dir)? {
        Some(name) => name,
        None => return Ok(()),
    };
    let validated_stat = match file_stat(validated.as_raw_fd()) {
        Ok(stat) => stat,
        Err(error) => {
            restore_aside(dir, &aside)?;
            return Err(error);
        }
    };
    // `open` fails for a symlink (`ELOOP`) and a socket (`ENXIO`) before an fd
    // exists. The name's own identity, not a followed open, decides.
    let moved = match name_stat(dir, &aside) {
        Ok(stat) => stat,
        Err(error) => {
            restore_aside(dir, &aside)?;
            return Err(error);
        }
    };
    let same = moved.st_dev == validated_stat.st_dev && moved.st_ino == validated_stat.st_ino;
    let regular = mode_bits(moved.st_mode) & mode_bits(libc::S_IFMT) == mode_bits(libc::S_IFREG);
    if same && regular {
        return unlink_named(dir, &aside);
    }
    restore_aside(dir, &aside)
}

fn restore_aside(dir: RawFd, aside: &str) -> Result<(), DirectoryRefusal> {
    match rename_at(dir, aside, MARKER_NAME, true) {
        Ok(()) | Err(DirectoryRefusal::Io(std::io::ErrorKind::AlreadyExists)) => Ok(()),
        Err(error) => Err(error),
    }
}

fn file_stat(fd: RawFd) -> Result<libc::stat, DirectoryRefusal> {
    let mut info: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd, &mut info) } != 0 {
        return Err(io(std::io::Error::last_os_error()));
    }
    Ok(info)
}

fn name_stat(dir: RawFd, name: &str) -> Result<libc::stat, DirectoryRefusal> {
    let name = c_name(name)?;
    let mut info: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstatat(dir, name.as_ptr(), &mut info, libc::AT_SYMLINK_NOFOLLOW) } != 0 {
        return Err(io(std::io::Error::last_os_error()));
    }
    Ok(info)
}

fn reserve_aside(dir: RawFd) -> Result<Option<String>, DirectoryRefusal> {
    for _ in 0..32 {
        let aside = aside_name();
        match rename_at(dir, MARKER_NAME, &aside, true) {
            Ok(()) => return Ok(Some(aside)),
            Err(DirectoryRefusal::Io(std::io::ErrorKind::NotFound)) => return Ok(None),
            Err(DirectoryRefusal::Io(std::io::ErrorKind::AlreadyExists)) => continue,
            Err(error) => return Err(error),
        }
    }
    Err(DirectoryRefusal::Io(std::io::ErrorKind::AlreadyExists))
}

fn aside_name() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!(".nessa-cleanup-{}-{n}", std::process::id())
}

fn rename_at(dir: RawFd, from: &str, to: &str, noreplace: bool) -> Result<(), DirectoryRefusal> {
    let from = c_name(from)?;
    let to = c_name(to)?;
    let renamed = rename_exclusive(dir, &from, &to, noreplace);
    if renamed == 0 {
        Ok(())
    } else {
        Err(io(std::io::Error::last_os_error()))
    }
}

fn rename_exclusive(
    dir: RawFd,
    from: &std::ffi::CStr,
    to: &std::ffi::CStr,
    noreplace: bool,
) -> i32 {
    #[cfg(target_os = "linux")]
    {
        let flags = if noreplace { libc::RENAME_NOREPLACE } else { 0 };
        unsafe { libc::renameat2(dir, from.as_ptr(), dir, to.as_ptr(), flags) }
    }
    #[cfg(target_vendor = "apple")]
    {
        let flags = if noreplace { libc::RENAME_EXCL } else { 0 };
        unsafe { libc::renameatx_np(dir, from.as_ptr(), dir, to.as_ptr(), flags) }
    }
    #[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
    {
        let _ = noreplace;
        unsafe { libc::renameat(dir, from.as_ptr(), dir, to.as_ptr()) }
    }
}

fn unlink_named(dir: RawFd, name: &str) -> Result<(), DirectoryRefusal> {
    let name = c_name(name)?;
    if unsafe { libc::unlinkat(dir, name.as_ptr(), 0) } == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.kind() == std::io::ErrorKind::NotFound {
        return Ok(());
    }
    Err(io(error))
}

fn open_at(dir: RawFd, name: &str, directory: bool) -> Result<File, DirectoryRefusal> {
    if name == ".." {
        return Err(DirectoryRefusal::Outside {
            leave: PathLeave::DotDot,
            audit_recorded: true,
        });
    }
    let mut flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK;
    if directory {
        flags |= libc::O_DIRECTORY;
    }
    let name = c_name(name)?;
    let fd = unsafe { libc::openat(dir, name.as_ptr(), flags) };
    if fd >= 0 {
        let file = unsafe { File::from_raw_fd(fd) };
        if !directory && !is_walk_target(&file)? {
            return Err(DirectoryRefusal::Io(std::io::ErrorKind::InvalidInput));
        }
        return Ok(file);
    }
    let error = std::io::Error::last_os_error();
    // A socket or device fails open before there is an fd to classify.
    // The same refusal as a FIFO the open did return. A symlink is not one
    // of those: it is the boundary refusal below.
    if entry_is_special(dir, &name) {
        return Err(DirectoryRefusal::Io(std::io::ErrorKind::InvalidInput));
    }
    Err(no_follow_boundary(dir, &name, error))
}

/// `O_NOFOLLOW` rejected a symlink (`ELOOP`). On an intermediate component
/// opened with `O_DIRECTORY`, macOS reports that swap as `ENOTDIR`; the name
/// is a symlink, so it is the same refusal. Any other error stays I/O.
fn no_follow_boundary(
    dir: RawFd,
    name: &std::ffi::CStr,
    error: std::io::Error,
) -> DirectoryRefusal {
    let errno = error.raw_os_error();
    let swapped_link =
        errno == Some(libc::ELOOP) || (errno == Some(libc::ENOTDIR) && is_symlink(dir, name));
    if swapped_link {
        return DirectoryRefusal::Outside {
            leave: PathLeave::Symlink,
            audit_recorded: true,
        };
    }
    io(error)
}

fn open_write_nofollow(dir: RawFd, name: &str) -> Result<File, DirectoryRefusal> {
    let flags = libc::O_CREAT | libc::O_RDWR | libc::O_NOFOLLOW | libc::O_CLOEXEC;
    let name = c_name(name)?;
    let fd = unsafe { libc::openat(dir, name.as_ptr(), flags, 0o600) };
    if fd < 0 {
        return Err(no_follow_boundary(
            dir,
            &name,
            std::io::Error::last_os_error(),
        ));
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn entry_is_special(dir: RawFd, name: &std::ffi::CStr) -> bool {
    let mut info: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstatat(dir, name.as_ptr(), &mut info, libc::AT_SYMLINK_NOFOLLOW) } != 0 {
        return false;
    }
    let kind = mode_bits(info.st_mode) & mode_bits(libc::S_IFMT);
    kind != mode_bits(libc::S_IFREG)
        && kind != mode_bits(libc::S_IFDIR)
        && kind != mode_bits(libc::S_IFLNK)
}

fn is_symlink(dir: RawFd, name: &std::ffi::CStr) -> bool {
    let mut info: libc::stat = unsafe { std::mem::zeroed() };
    let result = unsafe { libc::fstatat(dir, name.as_ptr(), &mut info, libc::AT_SYMLINK_NOFOLLOW) };
    result == 0 && mode_bits(info.st_mode) & mode_bits(libc::S_IFMT) == mode_bits(libc::S_IFLNK)
}

fn is_marker(root: RawFd, opened: RawFd) -> bool {
    let Ok(marker) = open_at(root, MARKER_NAME, false) else {
        return false;
    };
    let mut marker_stat = unsafe { std::mem::zeroed() };
    let mut opened_stat = unsafe { std::mem::zeroed() };
    let marker_ok = unsafe { libc::fstat(marker.as_raw_fd(), &mut marker_stat) } == 0;
    let opened_ok = unsafe { libc::fstat(opened, &mut opened_stat) } == 0;
    marker_ok
        && opened_ok
        && marker_stat.st_dev == opened_stat.st_dev
        && marker_stat.st_ino == opened_stat.st_ino
}

fn c_name(name: &str) -> Result<std::ffi::CString, DirectoryRefusal> {
    std::ffi::CString::new(name).map_err(|_| DirectoryRefusal::Io(std::io::ErrorKind::InvalidInput))
}

fn io(error: std::io::Error) -> DirectoryRefusal {
    DirectoryRefusal::Io(error.kind())
}

#[cfg(test)]
mod marker_identity {
    use std::os::unix::fs::FileTypeExt;

    use super::*;

    #[test]
    fn a_renamed_marker_is_still_the_marker() {
        let root = tempfile::tempdir().unwrap();
        let shared = root.path().join("shared");
        std::fs::create_dir(&shared).unwrap();
        let path = shared.to_str().unwrap();
        LocalDirectoryFiles
            .create_marker(path, &[9; BINDING_LEN])
            .unwrap();
        let session = LocalDirectoryFiles.open_session(path).unwrap();
        assert!(matches!(session.marker().unwrap(), MarkerBytes::Exact(_)));
        std::fs::rename(shared.join(MARKER_NAME), shared.join("alias")).unwrap();
        assert!(matches!(
            session.read("alias").unwrap_err(),
            DirectoryRefusal::MarkerName {
                audit_recorded: true
            }
        ));
    }

    #[test]
    fn a_fifo_marker_is_refused_without_blocking_or_being_removed() {
        let root = tempfile::tempdir().unwrap();
        let shared = root.path().join("shared");
        std::fs::create_dir(&shared).unwrap();
        let marker = shared.join(MARKER_NAME);
        let name = std::ffi::CString::new(marker.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let started = std::time::Instant::now();
        let marker_bytes = LocalDirectoryFiles
            .read_marker(shared.to_str().unwrap())
            .unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        assert!(matches!(
            marker_bytes,
            crate::application::MarkerBytes::Unusable
        ));
        assert!(marker.exists());
        assert!(LocalDirectoryFiles
            .release_created_marker(shared.to_str().unwrap(), &[1; BINDING_LEN])
            .unwrap());
        assert!(marker.exists());
    }

    #[test]
    fn a_socket_marker_is_unusable_and_left_in_place() {
        let root = tempfile::tempdir().unwrap();
        let shared = root.path().join("shared");
        std::fs::create_dir(&shared).unwrap();
        let marker = shared.join(MARKER_NAME);
        let _listener = std::os::unix::net::UnixListener::bind(&marker).unwrap();
        let started = std::time::Instant::now();
        let marker_bytes = LocalDirectoryFiles
            .read_marker(shared.to_str().unwrap())
            .unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        assert!(matches!(
            marker_bytes,
            crate::application::MarkerBytes::Unusable
        ));
        assert!(marker.exists());
    }

    #[test]
    fn a_socket_inside_the_directory_is_refused_without_blocking() {
        let root = tempfile::tempdir().unwrap();
        let shared = root.path().join("shared");
        std::fs::create_dir(&shared).unwrap();
        let path = shared.to_str().unwrap();
        LocalDirectoryFiles
            .create_marker(path, &[4; BINDING_LEN])
            .unwrap();
        let socket = shared.join("socket");
        let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let session = LocalDirectoryFiles.open_session(path).unwrap();
        session.marker().unwrap();
        let started = std::time::Instant::now();
        let error = session.walk("socket").unwrap_err();
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        assert!(matches!(
            error,
            DirectoryRefusal::Io(std::io::ErrorKind::InvalidInput)
        ));
        assert!(socket.exists());
    }

    #[test]
    fn a_fifo_inside_the_directory_does_not_block_the_walk() {
        let root = tempfile::tempdir().unwrap();
        let shared = root.path().join("shared");
        std::fs::create_dir(&shared).unwrap();
        let path = shared.to_str().unwrap();
        LocalDirectoryFiles
            .create_marker(path, &[4; BINDING_LEN])
            .unwrap();
        let pipe = shared.join("pipe");
        let name = std::ffi::CString::new(pipe.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let session = LocalDirectoryFiles.open_session(path).unwrap();
        session.marker().unwrap();
        let started = std::time::Instant::now();
        let error = session.walk("pipe").unwrap_err();
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        assert!(matches!(
            error,
            DirectoryRefusal::Io(std::io::ErrorKind::InvalidInput)
        ));
        assert!(pipe.exists());
    }

    fn aside_names(dir: &std::path::Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(".nessa-cleanup-"))
            .collect()
    }

    #[test]
    fn cleanup_puts_back_a_symlink_or_socket_it_moved_aside() {
        let root = tempfile::tempdir().unwrap();
        let shared = root.path().join("shared");
        std::fs::create_dir(&shared).unwrap();
        let marker = shared.join(MARKER_NAME);
        std::fs::write(&marker, [9; BINDING_LEN]).unwrap();
        let directory = open_directory(shared.to_str().unwrap()).unwrap();
        let validated = open_named(directory.as_raw_fd(), MARKER_NAME).unwrap();
        std::fs::remove_file(&marker).unwrap();
        std::os::unix::fs::symlink("elsewhere", &marker).unwrap();
        remove_validated(directory.as_raw_fd(), &validated).unwrap();
        assert_eq!(
            std::fs::read_link(&marker).unwrap(),
            std::path::Path::new("elsewhere")
        );
        assert!(aside_names(&shared).is_empty());

        std::fs::remove_file(&marker).unwrap();
        let _listener = std::os::unix::net::UnixListener::bind(&marker).unwrap();
        remove_validated(directory.as_raw_fd(), &validated).unwrap();
        assert!(std::fs::metadata(&marker).unwrap().file_type().is_socket());
        assert!(aside_names(&shared).is_empty());
    }

    #[test]
    fn cleanup_does_not_replace_an_existing_aside_file() {
        let root = tempfile::tempdir().unwrap();
        let shared = root.path().join("shared");
        std::fs::create_dir(&shared).unwrap();
        let path = shared.to_str().unwrap();
        LocalDirectoryFiles
            .create_marker(path, &[3; BINDING_LEN])
            .unwrap();
        let taken = aside_name();
        let n: u64 = taken.rsplit('-').next().unwrap().parse().unwrap();
        let occupied = format!(".nessa-cleanup-{}-{}", std::process::id(), n + 1);
        std::fs::write(shared.join(&occupied), b"user-data").unwrap();
        assert!(LocalDirectoryFiles
            .release_created_marker(path, &[3; BINDING_LEN])
            .unwrap());
        assert_eq!(std::fs::read(shared.join(&occupied)).unwrap(), b"user-data");
        assert!(!shared.join(MARKER_NAME).exists());
    }

    #[test]
    fn a_write_open_of_a_symlink_is_a_boundary_and_leaves_the_target() {
        let root = tempfile::tempdir().unwrap();
        let shared = root.path().join("shared");
        std::fs::create_dir(&shared).unwrap();
        let secret = root.path().join("secret.txt");
        std::fs::write(&secret, b"secret").unwrap();
        std::os::unix::fs::symlink(&secret, shared.join("note")).unwrap();
        let directory = open_directory(shared.to_str().unwrap()).unwrap();
        let error = open_write_nofollow(directory.as_raw_fd(), "note").unwrap_err();
        assert!(matches!(
            error,
            DirectoryRefusal::Outside {
                leave: PathLeave::Symlink,
                ..
            }
        ));
        assert_eq!(std::fs::read(&secret).unwrap(), b"secret");
    }

    #[test]
    fn a_read_open_of_a_symlink_is_a_boundary_and_an_intermediate_swap_is_too() {
        let root = tempfile::tempdir().unwrap();
        let shared = root.path().join("shared");
        std::fs::create_dir(&shared).unwrap();
        let secret = root.path().join("secret.txt");
        std::fs::write(&secret, b"secret").unwrap();
        std::os::unix::fs::symlink(&secret, shared.join("note")).unwrap();
        let path = shared.to_str().unwrap();
        let error = LocalDirectoryFiles.read_inside(path, "note").unwrap_err();
        assert!(matches!(
            error,
            DirectoryRefusal::Outside {
                leave: PathLeave::Symlink,
                ..
            }
        ));
        assert_eq!(std::fs::read(&secret).unwrap(), b"secret");
        // An intermediate component opened with `O_DIRECTORY|O_NOFOLLOW`.
        // Linux returns `ELOOP`. macOS returns `ENOTDIR` when that component
        // is a symlink to a file; the classifier covers both.
        std::os::unix::fs::symlink(&secret, shared.join("file-link")).unwrap();
        let through_file = LocalDirectoryFiles
            .read_inside(path, "file-link/leaf")
            .unwrap_err();
        assert!(matches!(
            through_file,
            DirectoryRefusal::Outside {
                leave: PathLeave::Symlink,
                ..
            }
        ));
        assert_eq!(std::fs::read(&secret).unwrap(), b"secret");
    }

    #[test]
    fn enotdir_is_a_boundary_only_when_the_component_is_a_symlink() {
        let root = tempfile::tempdir().unwrap();
        let shared = root.path().join("shared");
        std::fs::create_dir(&shared).unwrap();
        std::fs::write(shared.join("file"), b"x").unwrap();
        std::os::unix::fs::symlink("elsewhere", shared.join("link")).unwrap();
        let directory = open_directory(shared.to_str().unwrap()).unwrap();
        let link = c_name("link").unwrap();
        let file = c_name("file").unwrap();
        let swapped = no_follow_boundary(
            directory.as_raw_fd(),
            &link,
            std::io::Error::from_raw_os_error(libc::ENOTDIR),
        );
        assert!(matches!(
            swapped,
            DirectoryRefusal::Outside {
                leave: PathLeave::Symlink,
                ..
            }
        ));
        let ordinary = no_follow_boundary(
            directory.as_raw_fd(),
            &file,
            std::io::Error::from_raw_os_error(libc::ENOTDIR),
        );
        assert!(matches!(
            ordinary,
            DirectoryRefusal::Io(std::io::ErrorKind::NotADirectory)
        ));
        let transient = no_follow_boundary(
            directory.as_raw_fd(),
            &link,
            std::io::Error::from_raw_os_error(libc::EIO),
        );
        assert!(matches!(transient, DirectoryRefusal::Io(_)));
    }

    #[test]
    fn creating_a_marker_does_not_follow_a_symlink_at_its_name() {
        let root = tempfile::tempdir().unwrap();
        let shared = root.path().join("shared");
        std::fs::create_dir(&shared).unwrap();
        let secret = root.path().join("secret.txt");
        std::fs::write(&secret, b"secret").unwrap();
        std::os::unix::fs::symlink(&secret, shared.join(MARKER_NAME)).unwrap();
        let error = LocalDirectoryFiles
            .create_marker(shared.to_str().unwrap(), &[2; BINDING_LEN])
            .unwrap_err();
        assert!(matches!(
            error,
            DirectoryRefusal::Outside {
                leave: PathLeave::Symlink,
                ..
            }
        ));
        assert_eq!(std::fs::read(&secret).unwrap(), b"secret");
        assert!(shared
            .join(MARKER_NAME)
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn unlinking_a_marker_name_does_not_remove_a_symlink_target() {
        let root = tempfile::tempdir().unwrap();
        let shared = root.path().join("shared");
        std::fs::create_dir(&shared).unwrap();
        let secret = root.path().join("secret.txt");
        std::fs::write(&secret, b"secret").unwrap();
        std::os::unix::fs::symlink(&secret, shared.join(MARKER_NAME)).unwrap();
        LocalDirectoryFiles
            .unlink_marker(shared.to_str().unwrap())
            .unwrap();
        assert!(!shared.join(MARKER_NAME).exists());
        assert_eq!(std::fs::read(&secret).unwrap(), b"secret");
        std::os::unix::fs::symlink(&secret, shared.join("aside-link")).unwrap();
        let directory = open_directory(shared.to_str().unwrap()).unwrap();
        unlink_named(directory.as_raw_fd(), "aside-link").unwrap();
        assert!(!shared.join("aside-link").exists());
        assert_eq!(std::fs::read(&secret).unwrap(), b"secret");
    }

    #[test]
    fn renaming_a_symlink_does_not_replace_its_target() {
        let root = tempfile::tempdir().unwrap();
        let shared = root.path().join("shared");
        std::fs::create_dir(&shared).unwrap();
        let secret = root.path().join("secret.txt");
        std::fs::write(&secret, b"secret").unwrap();
        std::os::unix::fs::symlink(&secret, shared.join("from")).unwrap();
        let directory = open_directory(shared.to_str().unwrap()).unwrap();
        rename_at(directory.as_raw_fd(), "from", "to", true).unwrap();
        assert!(!shared.join("from").exists());
        assert_eq!(std::fs::read_link(shared.join("to")).unwrap(), secret);
        assert_eq!(std::fs::read(&secret).unwrap(), b"secret");
    }

    #[test]
    fn opening_a_symlink_marker_does_not_read_its_target() {
        let root = tempfile::tempdir().unwrap();
        let shared = root.path().join("shared");
        std::fs::create_dir(&shared).unwrap();
        let secret = root.path().join("secret.txt");
        std::fs::write(&secret, b"secret-bytes-that-are-not-the-marker").unwrap();
        std::os::unix::fs::symlink(&secret, shared.join(MARKER_NAME)).unwrap();
        let directory = open_directory(shared.to_str().unwrap()).unwrap();
        let error = open_marker_file(directory.as_raw_fd()).unwrap_err();
        assert!(matches!(
            error,
            DirectoryRefusal::Io(std::io::ErrorKind::InvalidInput)
        ));
        assert_eq!(
            std::fs::read(&secret).unwrap(),
            b"secret-bytes-that-are-not-the-marker"
        );
    }

    #[test]
    fn a_failed_marker_write_removes_the_file_it_created() {
        // The file-size limit is process-wide. Apply it in a child so the
        // other tests in this process can still write.
        if std::env::var_os("NESSA_MARKER_WRITE_LIMIT").is_none() {
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .arg("a_failed_marker_write_removes_the_file_it_created")
                .arg("--exact")
                .env("NESSA_MARKER_WRITE_LIMIT", "1")
                .status()
                .unwrap();
            assert!(status.success());
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let shared = root.path().join("shared");
        std::fs::create_dir(&shared).unwrap();
        let mut old = unsafe { std::mem::zeroed() };
        assert_eq!(unsafe { libc::getrlimit(libc::RLIMIT_FSIZE, &mut old) }, 0);
        let limited = libc::rlimit {
            rlim_cur: 0,
            rlim_max: old.rlim_max,
        };
        unsafe { libc::signal(libc::SIGXFSZ, libc::SIG_IGN) };
        assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_FSIZE, &limited) }, 0);
        let result = LocalDirectoryFiles.create_marker(shared.to_str().unwrap(), &[1; BINDING_LEN]);
        unsafe { libc::setrlimit(libc::RLIMIT_FSIZE, &old) };
        unsafe { libc::signal(libc::SIGXFSZ, libc::SIG_DFL) };
        assert!(result.is_err());
        assert!(!shared.join(MARKER_NAME).exists());
    }
}
