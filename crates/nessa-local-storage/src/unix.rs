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

    let mut parent = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
        .open(root)?;
    verify_directory_file(&parent)?;
    for component in components {
        let created = unsafe { libc::mkdirat(parent.as_raw_fd(), component.as_ptr(), 0o700) };
        if created != 0 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::AlreadyExists {
                return Err(error);
            }
        }
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
        parent = child;
    }
    Ok(())
}
pub fn sync_directory(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}
pub fn replace(from: &Path, to: &Path) -> io::Result<()> {
    fs::rename(from, to)
}
