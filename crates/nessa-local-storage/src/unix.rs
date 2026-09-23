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
mod retained_directory;
pub(crate) use retained_directory::{
    validate_native_name, RetainedDirectory, RetainedDirectoryEntries,
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
pub fn open_temporary(path: &Path) -> io::Result<File> {
    open(path, OpenMode::CreateNew)
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
pub fn open_temporary_beneath(root: &Path, relative: &Path) -> io::Result<File> {
    open_beneath(root, relative, OpenMode::CreateNew)
}
pub fn verify_file(file: &File) -> io::Result<()> {
    if private_regular_file_metadata(file)?.nlink() != 1 {
        return Err(unsafe_file());
    }
    Ok(())
}
fn verify_identity_candidate(file: &File) -> io::Result<()> {
    // An already-open identity witness may have no name after unlink;
    // multiple names remain unsafe because they defeat name ownership.
    if private_regular_file_metadata(file)?.nlink() > 1 {
        return Err(unsafe_file());
    }
    Ok(())
}
fn private_regular_file_metadata(file: &File) -> io::Result<fs::Metadata> {
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err(unsafe_file());
    }
    Ok(metadata)
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
/// Remove a file named relative to an already-private root.
///
/// The leaf is unlinked through a handle on its parent, opened one verified
/// component at a time, so no symbolic link along the way can send the removal
/// somewhere else. `fs::remove_file` resolves the whole path in the kernel and
/// has no such anchor; a link planted at any directory above the leaf makes it
/// delete a file outside `root`.
pub fn remove_file_beneath(root: &Path, relative: &Path) -> io::Result<()> {
    unlink_beneath(root, relative, 0)
}

pub fn remove_reserved_beneath(_: &File, root: &Path, relative: &Path) -> io::Result<()> {
    remove_file_beneath(root, relative)
}

/// Remove an empty directory named relative to an already-private root.
///
/// The counterpart of [`remove_file_beneath`], and empty for the same reason
/// `rmdir` is: a directory with anything in it is one somebody still wants.
pub fn remove_directory_beneath(root: &Path, relative: &Path) -> io::Result<()> {
    unlink_beneath(root, relative, libc::AT_REMOVEDIR)
}

fn unlink_beneath(root: &Path, relative: &Path, flags: i32) -> io::Result<()> {
    let (parent, leaf) = open_parent_beneath(root, relative)?;
    let removed = unsafe { libc::unlinkat(parent.as_raw_fd(), leaf.as_ptr(), flags) };
    if removed != 0 {
        return Err(io::Error::last_os_error());
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
/// A rename that refuses to replace: one call, and afterwards `to` is the file
/// and `from` is gone. `AlreadyExists` when `to` is taken, so an interrupted or
/// repeated publish cannot overwrite a record another owner already published.
///
/// It is a rename rather than a link because a record has to be readable the
/// instant it exists. Linking the destination and unlinking the temporary
/// afterwards left a moment — two calls wide, and as long as the scheduler
/// cared to make it — when the name existed with two links, and
/// [`verify_file`] refuses a file with two links as unsafe. Every reader of
/// that name in that moment was told the record could not be read, including
/// the one reader who most needs it: a writer that lost the race for the name
/// and reads the winner's record back to check they agree. There is nothing for
/// such a reader to wait for and no way for it to tell a busy machine from a
/// tampered file, so the moment is removed instead of tolerated.
///
/// `RENAME_EXCL` and `RENAME_NOREPLACE` are the same guarantee under the two
/// kernels' names for it. Not every filesystem implements either: SMB and AFP
/// mounts answer `ENOTSUP`, and NFS, eCryptfs and many FUSE mounts answer
/// `EINVAL` or `EOPNOTSUPP`. That is the one failure here a person could
/// actually act on — move the data to a local volume — and the errno alone
/// does not say so, so it is named. There is deliberately no fallback to the
/// link-and-unlink this replaced: it would silently reopen the gap above on
/// exactly the volumes where a network round trip makes it widest.
///
/// # Errors
///
/// `AlreadyExists` when `to` is taken, [`io::ErrorKind::Unsupported`] with a
/// sentence naming the volume when the filesystem has no exclusive rename, and
/// any other platform failure unchanged.
pub fn publish_new(from: &Path, to: &Path) -> io::Result<()> {
    let source = CString::new(from.as_os_str().as_bytes()).map_err(|_| unsafe_file())?;
    let destination = CString::new(to.as_os_str().as_bytes()).map_err(|_| unsafe_file())?;
    #[cfg(target_vendor = "apple")]
    let renamed =
        unsafe { libc::renamex_np(source.as_ptr(), destination.as_ptr(), libc::RENAME_EXCL) };
    #[cfg(target_os = "linux")]
    let renamed = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            destination.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if renamed != 0 {
        let error = io::Error::last_os_error();
        // Compared rather than matched: Linux defines `ENOTSUP` and
        // `EOPNOTSUPP` as the same number and macOS does not, so as patterns
        // they are an unreachable arm on one platform and two necessary arms on
        // the other. This says what it means on both.
        let unsupported = error.raw_os_error().is_some_and(|code| {
            code == libc::ENOTSUP || code == libc::EOPNOTSUPP || code == libc::EINVAL
        });
        return Err(if unsupported {
            io::Error::new(
                io::ErrorKind::Unsupported,
                format!(
                    "the filesystem holding {} cannot publish a file without replacing one; \
                     local data has to live on a volume that can, not a network or FUSE mount",
                    to.display()
                ),
            )
        } else {
            error
        });
    }
    Ok(())
}

pub fn publish_new_beneath(root: &Path, from: &Path, to: &Path) -> io::Result<()> {
    let (from_parent, from_leaf) = open_parent_beneath(root, from)?;
    let (to_parent, to_leaf) = open_parent_beneath(root, to)?;
    #[cfg(target_vendor = "apple")]
    let renamed = unsafe {
        libc::renameatx_np(
            from_parent.as_raw_fd(),
            from_leaf.as_ptr(),
            to_parent.as_raw_fd(),
            to_leaf.as_ptr(),
            libc::RENAME_EXCL,
        )
    };
    #[cfg(target_os = "linux")]
    let renamed = unsafe {
        libc::renameat2(
            from_parent.as_raw_fd(),
            from_leaf.as_ptr(),
            to_parent.as_raw_fd(),
            to_leaf.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if renamed != 0 {
        let error = io::Error::last_os_error();
        let unsupported = error.raw_os_error().is_some_and(|code| {
            code == libc::ENOTSUP || code == libc::EOPNOTSUPP || code == libc::EINVAL
        });
        return Err(if unsupported {
            io::Error::new(
                io::ErrorKind::Unsupported,
                format!(
                    "the filesystem holding {} cannot publish a file without replacing one; local data has to live on a volume that can, not a network or FUSE mount",
                    root.join(to).display()
                ),
            )
        } else {
            error
        });
    }
    Ok(())
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
