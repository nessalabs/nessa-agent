use super::*;
use std::{
    ffi::CString,
    fs::{self, OpenOptions},
    os::unix::{
        ffi::OsStrExt,
        fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
        io::{AsRawFd, FromRawFd},
    },
    path::Component,
};
pub fn open(path: &Path, mode: OpenMode) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(!matches!(mode, OpenMode::Read | OpenMode::ReadNonblocking))
        .create(matches!(mode, OpenMode::OpenOrCreate))
        .create_new(matches!(mode, OpenMode::CreateNew))
        .mode(0o600)
        .custom_flags(
            libc::O_NOFOLLOW
                | if matches!(mode, OpenMode::ReadNonblocking) {
                    libc::O_NONBLOCK
                } else {
                    0
                },
        );
    let file = options.open(path)?;
    verify_file(&file)?;
    Ok(file)
}
pub fn open_beneath(root: &Path, relative: &Path, mode: OpenMode) -> io::Result<File> {
    let (parent, leaf) = open_parent_beneath(root, relative)?;
    let mut flags = libc::O_CLOEXEC | libc::O_NOFOLLOW;
    flags |= match mode {
        OpenMode::Read => libc::O_RDONLY,
        OpenMode::ReadNonblocking => libc::O_RDONLY | libc::O_NONBLOCK,
        OpenMode::ReadWrite => libc::O_RDWR,
        OpenMode::OpenOrCreate => libc::O_RDWR | libc::O_CREAT,
        OpenMode::CreateNew => libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
    };
    let descriptor = unsafe { libc::openat(parent.as_raw_fd(), leaf.as_ptr(), flags, 0o600) };
    if descriptor < 0 {
        return Err(io::Error::last_os_error());
    }
    let file = unsafe { File::from_raw_fd(descriptor) };
    verify_file(&file)?;
    Ok(file)
}
pub fn verify_file(file: &File) -> io::Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err(unsafe_file());
    }
    Ok(())
}
pub fn verify_directory(path: &Path) -> io::Result<()> {
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
        .open(path)?;
    verify_directory_file(&directory)
}
fn verify_directory_file(directory: &File) -> io::Result<()> {
    let metadata = directory.metadata()?;
    if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
        return Err(unsafe_file());
    }
    Ok(())
}
fn relative_components(relative: &Path) -> io::Result<Vec<CString>> {
    let components = relative
        .components()
        .map(|component| match component {
            Component::Normal(name) => CString::new(name.as_bytes()).map_err(|_| unsafe_file()),
            _ => Err(unsafe_file()),
        })
        .collect::<io::Result<Vec<_>>>()?;
    if components.is_empty() {
        return Err(unsafe_file());
    }
    Ok(components)
}
fn open_root(root: &Path) -> io::Result<File> {
    let root = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_CLOEXEC)
        .open(root)?;
    verify_directory_file(&root)?;
    Ok(root)
}
fn open_child_directory(parent: &File, component: &CString) -> io::Result<File> {
    let descriptor = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            component.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if descriptor < 0 {
        return Err(io::Error::last_os_error());
    }
    let child = unsafe { File::from_raw_fd(descriptor) };
    verify_directory_file(&child)?;
    Ok(child)
}
fn open_parent_beneath(root: &Path, relative: &Path) -> io::Result<(File, CString)> {
    let mut components = relative_components(relative)?;
    let leaf = components.pop().ok_or_else(unsafe_file)?;
    let mut parent = open_root(root)?;
    for component in components {
        parent = open_child_directory(&parent, &component)?;
    }
    Ok((parent, leaf))
}
pub fn create_directory(path: &Path) -> io::Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    verify_directory(path)
}
/// Create a private relative directory tree beneath an already-private root.
///
/// Each component is opened relative to the verified parent handle, so a
/// symbolic link cannot redirect creation outside `root`.
pub fn create_directory_beneath(root: &Path, relative: &Path) -> io::Result<()> {
    let components = relative_components(relative)?;
    let mut parent = open_root(root)?;
    for component in components {
        let created = unsafe { libc::mkdirat(parent.as_raw_fd(), component.as_ptr(), 0o700) };
        if created != 0 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::AlreadyExists {
                return Err(error);
            }
        }
        parent = open_child_directory(&parent, &component)?;
    }
    Ok(())
}
pub fn sync_directory_beneath(root: &Path, relative: &Path) -> io::Result<()> {
    let directory = if relative.as_os_str().is_empty() {
        open_root(root)?
    } else {
        let mut directory = open_root(root)?;
        for component in relative_components(relative)? {
            directory = open_child_directory(&directory, &component)?;
        }
        directory
    };
    directory.sync_all()
}
pub fn sync_directory(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}
pub fn replace(from: &Path, to: &Path) -> io::Result<()> {
    fs::rename(from, to)
}
/// Publish `from` under the unused name `to`, never replacing an existing one.
///
/// `link` fails with `AlreadyExists` when `to` is taken, so an interrupted or
/// repeated publish cannot overwrite a record another owner already published.
/// The published name shares the temporary file's inode until that temporary
/// name is removed; until then the file has two links and fails private-file
/// verification, so callers must remove their temporary before reporting
/// success and clear temporaries a crash left behind.
pub fn publish_new(from: &Path, to: &Path) -> io::Result<()> {
    fs::hard_link(from, to)
}
pub fn replace_beneath(root: &Path, from: &Path, to: &Path) -> io::Result<()> {
    let (from_parent, from_leaf) = open_parent_beneath(root, from)?;
    let (to_parent, to_leaf) = open_parent_beneath(root, to)?;
    let result = unsafe {
        libc::renameat(
            from_parent.as_raw_fd(),
            from_leaf.as_ptr(),
            to_parent.as_raw_fd(),
            to_leaf.as_ptr(),
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
