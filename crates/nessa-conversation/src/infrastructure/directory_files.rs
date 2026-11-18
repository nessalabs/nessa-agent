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
        let text = resolved
            .to_str()
            .ok_or(DirectoryRefusal::Io(std::io::ErrorKind::InvalidData))?
            .to_owned();
        let _root = open_directory(&text)?;
        Ok(text)
    }

    fn read_marker(&self, canonical: &str) -> Result<MarkerBytes, DirectoryRefusal> {
        let root = open_directory(canonical)?;
        let marker = match open_at(root.as_raw_fd(), MARKER_NAME, false) {
            Ok(file) => file,
            Err(DirectoryRefusal::Io(std::io::ErrorKind::NotFound)) => {
                return Ok(MarkerBytes::Absent)
            }
            Err(error) => return Err(error),
        };
        let mut bytes = Vec::new();
        (&marker)
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
            return Err(io(std::io::Error::last_os_error()));
        }
        let mut file = unsafe { File::from_raw_fd(fd) };
        file.write_all(bytes).map_err(io)?;
        Ok(())
    }

    fn release_created_marker(
        &self,
        canonical: &str,
        binding: &[u8; BINDING_LEN],
    ) -> Result<bool, DirectoryRefusal> {
        match self.read_marker(canonical)? {
            MarkerBytes::Absent => Ok(true),
            MarkerBytes::Exact(bytes) if bytes == *binding => {
                self.unlink_marker(canonical)?;
                Ok(true)
            }
            MarkerBytes::Exact(_) | MarkerBytes::Other => Ok(true),
        }
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
            return Err(DirectoryRefusal::Outside { leave });
        }
        let mut file = file.ok_or(DirectoryRefusal::Outside {
            leave: PathLeave::Missing,
        })?;
        if is_marker(root.as_raw_fd(), file.as_raw_fd()) {
            return Err(DirectoryRefusal::MarkerName);
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
            return Err(DirectoryRefusal::Outside { leave });
        }
        if existing
            .as_ref()
            .is_some_and(|opened| is_marker(root.as_raw_fd(), opened.as_raw_fd()))
        {
            return Err(DirectoryRefusal::MarkerName);
        }
        let parent = parent_file(root.as_raw_fd(), relative)?;
        let name = final_name(relative)?;
        let flags = libc::O_CREAT | libc::O_RDWR | libc::O_NOFOLLOW | libc::O_CLOEXEC;
        let name = c_name(&name)?;
        let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags, 0o600) };
        if fd < 0 {
            return Err(io(std::io::Error::last_os_error()));
        }
        let mut file = unsafe { File::from_raw_fd(fd) };
        if is_marker(root.as_raw_fd(), file.as_raw_fd()) {
            return Err(DirectoryRefusal::MarkerName);
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
            match open_at(self.root.as_raw_fd(), MARKER_NAME, false) {
                Ok(file) => *held = Some(file),
                Err(DirectoryRefusal::Io(std::io::ErrorKind::NotFound)) => {
                    return Ok(MarkerBytes::Absent)
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
            return Err(DirectoryRefusal::Outside { leave });
        }
        let mut file = file.ok_or(DirectoryRefusal::Outside {
            leave: PathLeave::Missing,
        })?;
        if self.same_marker(file.as_raw_fd()) {
            return Err(DirectoryRefusal::MarkerName);
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).map_err(io)?;
        Ok(bytes)
    }

    fn write(&self, relative: &str, bytes: &[u8]) -> Result<(), DirectoryRefusal> {
        let (existing, leave) = walk_from(self.root.as_raw_fd(), relative)?;
        if leave != PathLeave::Inside && leave != PathLeave::Missing {
            return Err(DirectoryRefusal::Outside { leave });
        }
        if existing
            .as_ref()
            .is_some_and(|opened| self.same_marker(opened.as_raw_fd()))
        {
            return Err(DirectoryRefusal::MarkerName);
        }
        let parent = parent_file(self.root.as_raw_fd(), relative)?;
        let name = final_name(relative)?;
        let flags = libc::O_CREAT | libc::O_RDWR | libc::O_NOFOLLOW | libc::O_CLOEXEC;
        let name = c_name(&name)?;
        let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags, 0o600) };
        if fd < 0 {
            return Err(io(std::io::Error::last_os_error()));
        }
        let mut file = unsafe { File::from_raw_fd(fd) };
        if self.same_marker(file.as_raw_fd()) {
            return Err(DirectoryRefusal::MarkerName);
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
            let absolute = absolute(root)?;
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
            Err(DirectoryRefusal::Outside { leave }) => return Ok((None, leave)),
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
        return Err(DirectoryRefusal::Outside { leave });
    }
    file.ok_or(DirectoryRefusal::Outside {
        leave: PathLeave::Missing,
    })
}

fn final_name(relative: &str) -> Result<String, DirectoryRefusal> {
    relative
        .split('/')
        .rfind(|part| !part.is_empty() && *part != ".")
        .map(str::to_owned)
        .ok_or(DirectoryRefusal::Outside {
            leave: PathLeave::Missing,
        })
}

fn open_at(dir: RawFd, name: &str, directory: bool) -> Result<File, DirectoryRefusal> {
    if name == ".." {
        return Err(DirectoryRefusal::Outside {
            leave: PathLeave::DotDot,
        });
    }
    let mut flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
    if directory {
        flags |= libc::O_DIRECTORY;
    }
    let name = c_name(name)?;
    let fd = unsafe { libc::openat(dir, name.as_ptr(), flags) };
    if fd >= 0 {
        return Ok(unsafe { File::from_raw_fd(fd) });
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ELOOP) || is_symlink(dir, &name) {
        return Err(DirectoryRefusal::Outside {
            leave: PathLeave::Symlink,
        });
    }
    Err(io(error))
}

fn is_symlink(dir: RawFd, name: &std::ffi::CStr) -> bool {
    let mut info: libc::stat = unsafe { std::mem::zeroed() };
    let result = unsafe { libc::fstatat(dir, name.as_ptr(), &mut info, libc::AT_SYMLINK_NOFOLLOW) };
    result == 0 && (info.st_mode & libc::S_IFMT) == libc::S_IFLNK
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
            DirectoryRefusal::MarkerName
        ));
    }
}
