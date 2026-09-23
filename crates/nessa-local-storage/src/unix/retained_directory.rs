//! Unix retained-directory authority built from directory descriptors.
//!
//! Each enumeration opens `.` with `openat` to obtain an independent open-file
//! description. Publication and cleanup remain relative to the retained final
//! descriptor; cooperating callers hold their stable lock across the residual
//! leaf identity check and effect.

use super::{
    open_child_directory, open_root, relative_components, verify_directory_file,
    verify_identity_candidate,
};
use crate::{
    retained_directory::{PrivateDirectoryEntry, PrivateFileIdentity, PrivateFileType},
    unsafe_file, verify_file, OpenMode,
};
use std::{
    ffi::{CStr, CString, OsStr, OsString},
    fs::File,
    io,
    os::unix::{
        ffi::{OsStrExt, OsStringExt},
        fs::MetadataExt,
        io::{AsRawFd, FromRawFd},
    },
    path::{Path, PathBuf},
};

struct DirectoryBinding {
    file: File,
    identity: PrivateFileIdentity,
    name_in_parent: Option<CString>,
}

pub struct RetainedDirectory {
    root_path: PathBuf,
    chain: Vec<DirectoryBinding>,
}

impl RetainedDirectory {
    pub fn open_beneath(root: &Path, directory: &Path) -> io::Result<Self> {
        if !root.is_absolute() {
            return Err(unsafe_file());
        }
        let components = relative_components(directory)?;
        let root_file = open_root(root)?;
        let mut chain = vec![DirectoryBinding {
            identity: directory_identity(&root_file)?,
            file: root_file,
            name_in_parent: None,
        }];
        for component in components {
            let child = open_child_directory(&chain.last().expect("root exists").file, &component)?;
            chain.push(DirectoryBinding {
                identity: directory_identity(&child)?,
                file: child,
                name_in_parent: Some(component),
            });
        }
        let retained = Self {
            root_path: root.to_path_buf(),
            chain,
        };
        retained.verify_binding()?;
        Ok(retained)
    }

    pub fn entries(&self) -> io::Result<RetainedDirectoryEntries> {
        self.verify_binding()?;
        let dot = c".";
        let descriptor = unsafe {
            libc::openat(
                self.directory().as_raw_fd(),
                dot.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            )
        };
        if descriptor < 0 {
            return Err(io::Error::last_os_error());
        }
        // fdopendir owns this independently opened file description. `dup`
        // would share the directory offset and break parallel cursors.
        let directory = unsafe { libc::fdopendir(descriptor) };
        if directory.is_null() {
            let error = io::Error::last_os_error();
            unsafe {
                libc::close(descriptor);
            }
            return Err(error);
        }
        Ok(RetainedDirectoryEntries {
            directory,
            finished: false,
        })
    }

    pub fn open_file(&self, name: &OsStr, mode: OpenMode) -> io::Result<File> {
        self.verify_binding()?;
        let name = component(name)?;
        let mut flags = libc::O_CLOEXEC | libc::O_NOFOLLOW;
        flags |= match mode {
            OpenMode::Read => libc::O_RDONLY,
            OpenMode::ReadNonblocking => libc::O_RDONLY | libc::O_NONBLOCK,
            OpenMode::ReadWrite => libc::O_RDWR,
            OpenMode::OpenOrCreate => libc::O_RDWR | libc::O_CREAT,
            OpenMode::CreateNew => libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
        };
        let descriptor =
            unsafe { libc::openat(self.directory().as_raw_fd(), name.as_ptr(), flags, 0o600) };
        if descriptor < 0 {
            return Err(io::Error::last_os_error());
        }
        let file = unsafe { File::from_raw_fd(descriptor) };
        verify_file(&file)?;
        self.verify_binding()?;
        Ok(file)
    }

    pub fn named_file_is(&self, name: &OsStr, file: &File) -> io::Result<bool> {
        verify_identity_candidate(file)?;
        let name = component(name)?;
        let mut status = std::mem::MaybeUninit::<libc::stat>::uninit();
        let result = unsafe {
            libc::fstatat(
                self.directory().as_raw_fd(),
                name.as_ptr(),
                status.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if result != 0 {
            let error = io::Error::last_os_error();
            return if error.kind() == io::ErrorKind::NotFound {
                Ok(false)
            } else {
                Err(error)
            };
        }
        let status = unsafe { status.assume_init() };
        let identity = self.file_identity(file)?;
        Ok(file_type(status.st_mode) == PrivateFileType::RegularFile
            && identity == identity_from_status(&status))
    }

    pub fn verify_binding(&self) -> io::Result<()> {
        let current_root = open_root(&self.root_path)?;
        if directory_identity(&current_root)? != self.chain[0].identity {
            return Err(unsafe_file());
        }
        for index in 1..self.chain.len() {
            let parent = &self.chain[index - 1];
            let child = &self.chain[index];
            verify_directory_file(&parent.file)?;
            verify_directory_file(&child.file)?;
            let name = child.name_in_parent.as_ref().expect("child has a name");
            let mut status = std::mem::MaybeUninit::<libc::stat>::uninit();
            let result = unsafe {
                libc::fstatat(
                    parent.file.as_raw_fd(),
                    name.as_ptr(),
                    status.as_mut_ptr(),
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            };
            if result != 0 {
                return Err(io::Error::last_os_error());
            }
            let status = unsafe { status.assume_init() };
            if file_type(status.st_mode) != PrivateFileType::Directory
                || identity_from_status(&status) != child.identity
            {
                return Err(unsafe_file());
            }
        }
        Ok(())
    }

    pub fn sync(&self) -> io::Result<()> {
        self.directory().sync_all()
    }

    pub fn reserve_temp(&self, name: &OsStr) -> io::Result<File> {
        self.open_file(name, OpenMode::CreateNew)
    }

    pub fn publish_new(&self, from: &OsStr, to: &OsStr, _: &File) -> io::Result<()> {
        let from = component(from)?;
        let to = component(to)?;
        #[cfg(target_vendor = "apple")]
        let renamed = unsafe {
            libc::renameatx_np(
                self.directory().as_raw_fd(),
                from.as_ptr(),
                self.directory().as_raw_fd(),
                to.as_ptr(),
                libc::RENAME_EXCL,
            )
        };
        #[cfg(target_os = "linux")]
        let renamed = unsafe {
            libc::renameat2(
                self.directory().as_raw_fd(),
                from.as_ptr(),
                self.directory().as_raw_fd(),
                to.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if renamed != 0 {
            return Err(exclusive_rename_error(io::Error::last_os_error()));
        }
        Ok(())
    }

    pub fn remove_reserved(&self, name: &OsStr, file: &File) -> io::Result<()> {
        if !self.named_file_is(name, file)? {
            return Err(unsafe_file());
        }
        let name = component(name)?;
        let result = unsafe { libc::unlinkat(self.directory().as_raw_fd(), name.as_ptr(), 0) };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub fn file_identity(&self, file: &File) -> io::Result<PrivateFileIdentity> {
        let metadata = file.metadata()?;
        Ok(PrivateFileIdentity::from_u64s(
            metadata.dev(),
            metadata.ino(),
        ))
    }

    fn directory(&self) -> &File {
        &self
            .chain
            .last()
            .expect("retained directory has a root")
            .file
    }
}

pub fn validate_native_name(name: &OsStr) -> io::Result<()> {
    component(name).map(|_| ())
}

pub struct RetainedDirectoryEntries {
    directory: *mut libc::DIR,
    finished: bool,
}

impl Iterator for RetainedDirectoryEntries {
    type Item = io::Result<PrivateDirectoryEntry>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }
        loop {
            set_errno(0);
            let entry = unsafe { libc::readdir(self.directory) };
            if entry.is_null() {
                self.finished = true;
                let code = errno();
                return if code == 0 {
                    None
                } else {
                    Some(Err(io::Error::from_raw_os_error(code)))
                };
            }
            let entry = unsafe { &*entry };
            let name = unsafe { CStr::from_ptr(entry.d_name.as_ptr()) }.to_bytes();
            if name == b"." || name == b".." {
                continue;
            }
            let name = OsString::from_vec(name.to_vec());
            let kind = match entry.d_type {
                libc::DT_REG => PrivateFileType::RegularFile,
                libc::DT_DIR => PrivateFileType::Directory,
                libc::DT_LNK => PrivateFileType::Symlink,
                libc::DT_UNKNOWN => match entry_type(self.directory, &name) {
                    Ok(kind) => kind,
                    Err(error) => return Some(Err(error)),
                },
                _ => PrivateFileType::Other,
            };
            return Some(Ok(PrivateDirectoryEntry::new(name, kind)));
        }
    }
}

impl Drop for RetainedDirectoryEntries {
    fn drop(&mut self) {
        unsafe {
            libc::closedir(self.directory);
        }
    }
}

fn component(name: &OsStr) -> io::Result<CString> {
    CString::new(name.as_bytes()).map_err(|_| unsafe_file())
}

fn directory_identity(file: &File) -> io::Result<PrivateFileIdentity> {
    let metadata = file.metadata()?;
    Ok(PrivateFileIdentity::from_u64s(
        metadata.dev(),
        metadata.ino(),
    ))
}

#[cfg(target_os = "linux")]
fn identity_from_status(status: &libc::stat) -> PrivateFileIdentity {
    PrivateFileIdentity::from_u64s(status.st_dev, status.st_ino)
}

#[cfg(target_vendor = "apple")]
fn identity_from_status(status: &libc::stat) -> PrivateFileIdentity {
    // Match the standard library's Darwin `MetadataExt::dev` projection so
    // identities from `fstatat` and `File::metadata` have one representation.
    PrivateFileIdentity::from_u64s(status.st_dev as u64, status.st_ino)
}

fn file_type(mode: libc::mode_t) -> PrivateFileType {
    match mode & libc::S_IFMT {
        libc::S_IFREG => PrivateFileType::RegularFile,
        libc::S_IFDIR => PrivateFileType::Directory,
        libc::S_IFLNK => PrivateFileType::Symlink,
        _ => PrivateFileType::Other,
    }
}

fn entry_type(directory: *mut libc::DIR, name: &OsStr) -> io::Result<PrivateFileType> {
    let name = component(name)?;
    let mut status = std::mem::MaybeUninit::<libc::stat>::uninit();
    let result = unsafe {
        libc::fstatat(
            libc::dirfd(directory),
            name.as_ptr(),
            status.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(file_type(unsafe { status.assume_init() }.st_mode))
}

fn exclusive_rename_error(error: io::Error) -> io::Error {
    let unsupported = error.raw_os_error().is_some_and(|code| {
        code == libc::ENOTSUP || code == libc::EOPNOTSUPP || code == libc::EINVAL
    });
    if unsupported {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "the filesystem cannot publish a file without replacing one; local data has to live on a volume that supports exclusive rename",
        )
    } else {
        error
    }
}

#[cfg(target_os = "linux")]
fn errno() -> i32 {
    unsafe { *libc::__errno_location() }
}

#[cfg(target_vendor = "apple")]
fn errno() -> i32 {
    unsafe { *libc::__error() }
}

#[cfg(target_os = "linux")]
fn set_errno(value: i32) {
    unsafe {
        *libc::__errno_location() = value;
    }
}

#[cfg(target_vendor = "apple")]
fn set_errno(value: i32) {
    unsafe {
        *libc::__error() = value;
    }
}

#[cfg(all(test, target_vendor = "apple"))]
mod tests {
    use super::*;

    #[test]
    fn apple_stat_identity_matches_metadata_projection_across_dev_t_values() {
        for (device, inode) in [(0, 0), (17, 19), (-17, 23), (i32::MIN, u64::MAX)] {
            let mut status = unsafe { std::mem::zeroed::<libc::stat>() };
            status.st_dev = device;
            status.st_ino = inode;

            assert_eq!(
                identity_from_status(&status),
                PrivateFileIdentity::from_u64s(device as u64, inode)
            );
        }
    }
}
