//! Windows retained-directory authority built from top-down Win32 handles.
//!
//! Directory and mutation-capable file handles omit `FILE_SHARE_DELETE`,
//! pinning their names. Read-only handles and transient identity probes share
//! deletion so they can coexist with a publication handle; they acquire no
//! naming authority. Publication uses `FileRenameInfo` on the reservation
//! handle with a validated extended absolute destination. The retained chain
//! pins that destination from the caller-trusted root down; authority above the
//! root remains the caller's. Windows exposes no directory-fsync guarantee.

use super::{check, information, verify_acl, wide, User};
use crate::{
    retained_directory::{PrivateDirectoryEntry, PrivateFileIdentity, PrivateFileType},
    unsafe_file, verify_file, OpenMode,
};
use std::{
    ffi::{c_void, OsStr, OsString},
    fs::File,
    io,
    mem::{size_of, zeroed},
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        io::{AsRawHandle, FromRawHandle},
    },
    path::{Component, Path, PathBuf},
    ptr::{addr_of_mut, null, null_mut},
};
use windows_sys::Win32::{Foundation::*, Security::SECURITY_ATTRIBUTES, Storage::FileSystem::*};

struct DirectoryBinding {
    path: PathBuf,
    file: File,
    identity: PrivateFileIdentity,
}

pub struct RetainedDirectory {
    chain: Vec<DirectoryBinding>,
}

impl RetainedDirectory {
    pub fn open_beneath(root: &Path, directory: &Path) -> io::Result<Self> {
        if !root.is_absolute() {
            return Err(unsafe_file());
        }
        let components = normal_components(directory)?;
        let mut path = root.to_path_buf();
        let root_file = open_directory(&path, FILE_LIST_DIRECTORY | FILE_TRAVERSE)?;
        let mut chain = vec![DirectoryBinding {
            identity: file_identity(&root_file)?,
            file: root_file,
            path: path.clone(),
        }];
        let last = components.len() - 1;
        for (index, component) in components.into_iter().enumerate() {
            path.push(component);
            let file = open_directory(
                &path,
                FILE_LIST_DIRECTORY | FILE_TRAVERSE | if index == last { FILE_ADD_FILE } else { 0 },
            )?;
            chain.push(DirectoryBinding {
                identity: file_identity(&file)?,
                file,
                path: path.clone(),
            });
        }
        let retained = Self { chain };
        retained.verify_binding()?;
        Ok(retained)
    }

    pub fn entries(&self) -> io::Result<RetainedDirectoryEntries> {
        self.verify_binding()?;
        let file = open_directory(&self.directory().path, FILE_LIST_DIRECTORY)?;
        if file_identity(&file)? != self.directory().identity {
            return Err(unsafe_file());
        }
        Ok(RetainedDirectoryEntries {
            file,
            buffer: vec![0; 1024],
            offset: None,
            finished: false,
        })
    }

    pub fn open_file(&self, name: &OsStr, mode: OpenMode) -> io::Result<File> {
        self.verify_binding()?;
        let file = open_named(self.path(name)?, mode, 0)?;
        self.verify_binding()?;
        Ok(file)
    }

    pub fn named_file_is(&self, name: &OsStr, file: &File) -> io::Result<bool> {
        verify_file(file)?;
        let path = self.path(name)?;
        let named = match open_identity_probe(&path, FILE_READ_ATTRIBUTES | READ_CONTROL) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error),
        };
        verify_file(&named)?;
        Ok(file_identity(&named)? == file_identity(file)?)
    }

    pub fn verify_binding(&self) -> io::Result<()> {
        for binding in &self.chain {
            let current = open_directory(&binding.path, FILE_LIST_DIRECTORY | FILE_TRAVERSE)?;
            if file_identity(&current)? != binding.identity {
                return Err(unsafe_file());
            }
            if file_identity(&binding.file)? != binding.identity {
                return Err(unsafe_file());
            }
        }
        Ok(())
    }

    pub fn sync(&self) -> io::Result<()> {
        // Win32 has no directory-fsync equivalent. The public operation wraps
        // this validation-only step in binding checks and makes no durability
        // claim for the directory entry.
        if file_identity(&self.directory().file)? == self.directory().identity {
            Ok(())
        } else {
            Err(unsafe_file())
        }
    }

    pub fn reserve_temp(&self, name: &OsStr) -> io::Result<File> {
        self.verify_binding()?;
        open_named(self.path(name)?, OpenMode::CreateNew, DELETE)
    }

    pub fn open_reserved(&self, name: &OsStr, authority: &File) -> io::Result<File> {
        let file = open_existing_with_sharing(
            &self.path(name)?,
            GENERIC_READ | GENERIC_WRITE | FILE_READ_ATTRIBUTES | READ_CONTROL,
            false,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        )?;
        verify_file(&file)?;
        if file_identity(&file)? != file_identity(authority)? {
            return Err(unsafe_file());
        }
        Ok(file)
    }

    pub fn publish_new(&self, _: &OsStr, to: &OsStr, file: &File) -> io::Result<()> {
        let name = wide(&self.path(to)?)?;
        let name_bytes = name
            .len()
            .checked_sub(1)
            .and_then(|length| length.checked_mul(size_of::<u16>()))
            .and_then(|length| u32::try_from(length).ok())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "destination is too long")
            })?;
        let bytes = std::mem::offset_of!(FILE_RENAME_INFO, FileName)
            .checked_add(name.len().checked_mul(size_of::<u16>()).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "destination is too long")
            })?)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "destination is too long")
            })?;
        let buffer_bytes = u32::try_from(bytes)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "destination is too long"))?;
        let words = bytes.div_ceil(size_of::<usize>());
        let mut buffer = vec![0usize; words];
        let rename = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
        unsafe {
            (*rename).Anonymous.ReplaceIfExists = false;
            (*rename).RootDirectory = null_mut();
            (*rename).FileNameLength = name_bytes;
            std::ptr::copy_nonoverlapping(
                name.as_ptr(),
                addr_of_mut!((*rename).FileName).cast::<u16>(),
                name.len(),
            );
            check(SetFileInformationByHandle(
                file.as_raw_handle(),
                FileRenameInfo,
                rename.cast(),
                buffer_bytes,
            ))
        }
    }

    pub fn remove_file(&self, name: &OsStr, file: &File) -> io::Result<()> {
        if !self.named_file_is(name, file)? {
            return Err(unsafe_file());
        }
        let deleting = open_existing(
            &self.path(name)?,
            DELETE | FILE_READ_ATTRIBUTES | READ_CONTROL,
            false,
        )
        .map_err(|error| {
            if error.raw_os_error() == Some(ERROR_SHARING_VIOLATION as i32) {
                io::Error::new(io::ErrorKind::PermissionDenied, error)
            } else {
                error
            }
        })?;
        verify_file(&deleting)?;
        if file_identity(&deleting)? != file_identity(file)? {
            return Err(unsafe_file());
        }
        mark_deleted_from_namespace(&deleting)
    }

    pub fn remove_reserved(&self, name: &OsStr, file: &File) -> io::Result<()> {
        if !self.named_file_is(name, file)? {
            return Err(unsafe_file());
        }
        mark_deleted_from_namespace(file)
    }

    pub fn file_identity(&self, file: &File) -> io::Result<PrivateFileIdentity> {
        file_identity(file)
    }

    fn directory(&self) -> &DirectoryBinding {
        self.chain.last().expect("retained directory has a root")
    }

    fn path(&self, name: &OsStr) -> io::Result<PathBuf> {
        if name.is_empty()
            || Path::new(name)
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(unsafe_file());
        }
        Ok(self.directory().path.join(name))
    }
}

fn mark_deleted_from_namespace(file: &File) -> io::Result<()> {
    let disposition = FILE_DISPOSITION_INFO_EX {
        Flags: FILE_DISPOSITION_FLAG_DELETE | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS,
    };
    unsafe {
        check(SetFileInformationByHandle(
            file.as_raw_handle(),
            FileDispositionInfoEx,
            (&raw const disposition).cast(),
            size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
        ))
    }
}

pub fn validate_native_name(name: &OsStr) -> io::Result<()> {
    let units: Vec<u16> = name.encode_wide().collect();
    if units.is_empty()
        || units.contains(&0)
        || super::ambiguous_component(name)
        || name.to_string_lossy().contains(':')
    {
        Err(unsafe_file())
    } else {
        Ok(())
    }
}

pub struct RetainedDirectoryEntries {
    file: File,
    buffer: Vec<u64>,
    offset: Option<usize>,
    finished: bool,
}

impl Iterator for RetainedDirectoryEntries {
    type Item = io::Result<PrivateDirectoryEntry>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }
        loop {
            let offset = match self.offset {
                Some(offset) => offset,
                None => {
                    let loaded = unsafe {
                        GetFileInformationByHandleEx(
                            self.file.as_raw_handle(),
                            FileIdBothDirectoryInfo,
                            self.buffer.as_mut_ptr().cast(),
                            (self.buffer.len() * size_of::<u64>()) as u32,
                        )
                    };
                    if loaded == 0 {
                        let error = io::Error::last_os_error();
                        self.finished = true;
                        return if error.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
                            None
                        } else {
                            Some(Err(error))
                        };
                    }
                    0
                }
            };
            let info = unsafe {
                &*self
                    .buffer
                    .as_ptr()
                    .cast::<u8>()
                    .add(offset)
                    .cast::<FILE_ID_BOTH_DIR_INFO>()
            };
            self.offset = if info.NextEntryOffset == 0 {
                None
            } else {
                Some(offset + info.NextEntryOffset as usize)
            };
            let units = info.FileNameLength as usize / size_of::<u16>();
            let name = unsafe { std::slice::from_raw_parts(info.FileName.as_ptr(), units) };
            let name = OsString::from_wide(name);
            if name == OsStr::new(".") || name == OsStr::new("..") {
                continue;
            }
            let file_type = if info.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                PrivateFileType::Symlink
            } else if info.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
                PrivateFileType::Directory
            } else {
                PrivateFileType::RegularFile
            };
            return Some(Ok(PrivateDirectoryEntry::new(name, file_type)));
        }
    }
}

fn normal_components(path: &Path) -> io::Result<Vec<OsString>> {
    let components = path
        .components()
        .map(|component| match component {
            Component::Normal(name) => Ok(name.to_owned()),
            _ => Err(unsafe_file()),
        })
        .collect::<io::Result<Vec<_>>>()?;
    if components.is_empty() {
        return Err(unsafe_file());
    }
    Ok(components)
}

fn open_directory(path: &Path, access: u32) -> io::Result<File> {
    let file = open_existing(path, access | FILE_READ_ATTRIBUTES | READ_CONTROL, true)?;
    let info = information(&file)?;
    if info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
        return Err(unsafe_file());
    }
    verify_acl(&file)?;
    Ok(file)
}

fn open_existing(path: &Path, access: u32, directory: bool) -> io::Result<File> {
    open_existing_with_sharing(path, access, directory, FILE_SHARE_READ | FILE_SHARE_WRITE)
}

fn open_identity_probe(path: &Path, access: u32) -> io::Result<File> {
    // A reservation requests DELETE so it can rename and clean up by handle.
    // This short-lived probe must share that access; it acquires no mutation
    // authority and closes as soon as the two identities are compared.
    open_existing_with_sharing(
        path,
        access,
        false,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
    )
}

fn open_existing_with_sharing(
    path: &Path,
    access: u32,
    directory: bool,
    sharing: u32,
) -> io::Result<File> {
    unsafe {
        let handle = CreateFileW(
            wide(path)?.as_ptr(),
            access,
            sharing,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT
                | if directory {
                    FILE_FLAG_BACKUP_SEMANTICS
                } else {
                    0
                },
            null_mut(),
        );
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        Ok(File::from_raw_handle(handle))
    }
}

fn open_named(path: PathBuf, mode: OpenMode, additional_access: u32) -> io::Result<File> {
    let descriptor = User::current()?.descriptor(false)?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    let access = GENERIC_READ
        | additional_access
        | if matches!(mode, OpenMode::Read | OpenMode::ReadNonblocking) {
            0
        } else {
            GENERIC_WRITE
        };
    let disposition = match mode {
        OpenMode::Read | OpenMode::ReadNonblocking | OpenMode::ReadWrite => OPEN_EXISTING,
        OpenMode::OpenOrCreate => OPEN_ALWAYS,
        OpenMode::CreateNew => CREATE_NEW,
    };
    unsafe {
        // Read handles can coexist with the still-open publication handle,
        // which retains DELETE across rename. Mutation-capable handles omit
        // delete sharing and continue to pin stable lock names.
        let sharing = FILE_SHARE_READ
            | FILE_SHARE_WRITE
            | if matches!(mode, OpenMode::Read | OpenMode::ReadNonblocking) {
                FILE_SHARE_DELETE
            } else {
                0
            };
        let handle = CreateFileW(
            wide(&path)?.as_ptr(),
            access,
            sharing,
            &attributes,
            disposition,
            FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        );
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let file = File::from_raw_handle(handle);
        verify_file(&file)?;
        Ok(file)
    }
}

fn file_identity(file: &File) -> io::Result<PrivateFileIdentity> {
    let mut info = unsafe { zeroed::<FILE_ID_INFO>() };
    unsafe {
        check(GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileIdInfo,
            (&raw mut info).cast::<c_void>(),
            size_of::<FILE_ID_INFO>() as u32,
        ))?;
    }
    Ok(PrivateFileIdentity::from_windows(
        info.VolumeSerialNumber,
        info.FileId.Identifier,
    ))
}
