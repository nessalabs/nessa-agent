//! Candidate restore publication mechanics. Native qualification is owned by
//! `tests/windows_protected_directory.rs`; see the N01–N07 ordering table in
//! `docs/design/retained-directory-publication.md`. No path fallback is used.

use super::{check, information, validate_native_name, LocalAllocation, User};
use crate::{unsafe_file, verify_file};
use std::{
    ffi::OsStr,
    fs::File,
    io,
    mem::{offset_of, size_of},
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle},
    },
    ptr::{null, null_mut},
};
use windows_sys::Wdk::{
    Foundation::OBJECT_ATTRIBUTES,
    Storage::FileSystem::{
        NtCreateFile, FILE_CREATE, FILE_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_REPARSE_POINT,
        FILE_SYNCHRONOUS_IO_NONALERT,
    },
};
use windows_sys::Win32::{
    Foundation::*,
    Security::{Authorization::*, *},
    Storage::FileSystem::*,
    System::IO::{IO_STATUS_BLOCK, IO_STATUS_BLOCK_0},
};

/// Create an absent directory with temporary read/search-only user permissions.
///
/// The returned initial handle requests namespace mutation, DELETE and WRITE_DAC.
/// Native tests must qualify whether the OS permits child mutation through this
/// handle while refusing fresh writer handles. This is not an activation API.
pub fn create_protected_directory_at(parent: &File, name: &OsStr) -> io::Result<File> {
    let descriptor = User::current()?.descriptor_with_access(true, "FRFX")?;
    let file = relative(
        parent,
        name,
        FILE_LIST_DIRECTORY
            | FILE_TRAVERSE
            | FILE_ADD_FILE
            | FILE_ADD_SUBDIRECTORY
            | FILE_DELETE_CHILD
            | FILE_READ_ATTRIBUTES
            | READ_CONTROL
            | WRITE_DAC
            | DELETE
            | SYNCHRONIZE,
        FILE_CREATE,
        FILE_DIRECTORY_FILE,
        // The parent's DELETE_CHILD permission can otherwise grant a fresh
        // DELETE handle despite this directory's read-only DACL. Retain the
        // initial DELETE authority for our own handle rename, but do not share
        // it with a second opener. Native publication tests qualify that order.
        FILE_SHARE_READ,
        Some(&descriptor),
    )?;
    require_directory(&file)?;
    Ok(file)
}

/// Exclusively create a private file with temporary read-only user permissions.
/// The initial payload and DELETE authority omit write sharing. Native tests
/// require fresh write and DELETE opens to fail even after this handle closes.
pub fn create_protected_file_at(parent: &File, name: &OsStr) -> io::Result<File> {
    let descriptor = User::current()?.descriptor_with_access(false, "FR")?;
    let file = relative(
        parent,
        name,
        GENERIC_READ | GENERIC_WRITE | DELETE | WRITE_DAC | SYNCHRONIZE,
        FILE_CREATE,
        0,
        FILE_SHARE_READ | FILE_SHARE_DELETE,
        Some(&descriptor),
    )?;
    verify_file(&file)?;
    Ok(file)
}

/// Open one existing directory without following its final reparse point.
pub fn open_directory_at(parent: &File, name: &OsStr) -> io::Result<File> {
    let file = relative(
        parent,
        name,
        FILE_LIST_DIRECTORY | FILE_TRAVERSE | FILE_READ_ATTRIBUTES | READ_CONTROL | SYNCHRONIZE,
        FILE_OPEN,
        FILE_OPEN_REPARSE_POINT,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        None,
    )?;
    require_directory(&file)?;
    Ok(file)
}

/// Open a read-only private child without following its final reparse point.
pub fn open_file_at(parent: &File, name: &OsStr) -> io::Result<File> {
    let file = relative(
        parent,
        name,
        GENERIC_READ | SYNCHRONIZE,
        FILE_OPEN,
        FILE_OPEN_REPARSE_POINT,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        None,
    )?;
    verify_file(&file)?;
    Ok(file)
}

fn require_directory(file: &File) -> io::Result<()> {
    if information(file)?.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
        return Err(unsafe_file());
    }
    super::verify_acl(file)
}

fn name_units(name: &OsStr) -> io::Result<Vec<u16>> {
    validate_native_name(name)?;
    let units: Vec<u16> = name.encode_wide().collect();
    if name == OsStr::new(".")
        || name == OsStr::new("..")
        || units
            .iter()
            .any(|unit| *unit == b'/' as u16 || *unit == b'\\' as u16)
    {
        return Err(unsafe_file());
    }
    Ok(units)
}

fn relative(
    parent: &File,
    name: &OsStr,
    access: u32,
    disposition: u32,
    options: u32,
    sharing: u32,
    descriptor: Option<&LocalAllocation>,
) -> io::Result<File> {
    let mut name = name_units(name)?;
    let length = name
        .len()
        .checked_mul(2)
        .and_then(|length| u16::try_from(length).ok())
        .ok_or_else(unsafe_file)?;
    let mut unicode = UNICODE_STRING {
        Length: length,
        MaximumLength: length,
        Buffer: name.as_mut_ptr(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: parent.as_raw_handle(),
        ObjectName: &mut unicode,
        // No multi-component lookup; preserve the caller's native case spelling.
        Attributes: 0,
        SecurityDescriptor: descriptor.map_or(null(), |descriptor| descriptor.0.cast()),
        SecurityQualityOfService: null_mut(),
    };
    let mut status = IO_STATUS_BLOCK {
        Anonymous: IO_STATUS_BLOCK_0 { Status: 0 },
        Information: 0,
    };
    let mut handle = null_mut();
    let result = unsafe {
        NtCreateFile(
            &mut handle,
            access,
            &attributes,
            &mut status,
            null(),
            FILE_ATTRIBUTE_NORMAL,
            sharing,
            disposition,
            options | FILE_SYNCHRONOUS_IO_NONALERT,
            null(),
            0,
        )
    };
    if result < 0 {
        return Err(io::Error::from_raw_os_error(
            unsafe { RtlNtStatusToDosError(result) } as i32,
        ));
    }
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return Err(unsafe_file());
    }
    Ok(unsafe { File::from_raw_handle(handle) })
}

/// Exclusively rename the exact retained object to one relative destination.
/// Requires DELETE on `source`; an occupied destination is left intact.
pub fn rename_handle_at(source: &File, destination: &File, name: &OsStr) -> io::Result<()> {
    let name = name_units(name)?;
    let name_bytes = name.len().checked_mul(2).ok_or_else(unsafe_file)?;
    let bytes = offset_of!(FILE_RENAME_INFO, FileName)
        .checked_add(name_bytes)
        .ok_or_else(unsafe_file)?;
    let mut buffer = vec![0usize; bytes.div_ceil(size_of::<usize>())];
    let rename = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    unsafe {
        (*rename).Anonymous.ReplaceIfExists = false;
        (*rename).RootDirectory = destination.as_raw_handle();
        (*rename).FileNameLength = u32::try_from(name_bytes).map_err(|_| unsafe_file())?;
        std::ptr::copy_nonoverlapping(
            name.as_ptr(),
            std::ptr::addr_of_mut!((*rename).FileName).cast(),
            name.len(),
        );
        check(SetFileInformationByHandle(
            source.as_raw_handle(),
            FileRenameInfo,
            rename.cast(),
            u32::try_from(bytes).map_err(|_| unsafe_file())?,
        ))
    }
}

/// Remove the exact retained object from its namespace. Directories must be empty.
pub fn remove_handle(file: &File) -> io::Result<()> {
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

/// Release temporary namespace protection through the retained WRITE_DAC handle.
/// The caller owns the ordering: call only after successful live publication.
pub fn release_directory_writes(file: &File) -> io::Result<()> {
    release_writes(file, true)
}

/// Release temporary file restrictions through the retained WRITE_DAC handle.
/// The caller owns the ordering: call only after successful live publication.
pub fn release_file_writes(file: &File) -> io::Result<()> {
    release_writes(file, false)
}

fn release_writes(file: &File, directory: bool) -> io::Result<()> {
    let descriptor = User::current()?.descriptor(directory)?;
    let mut present = 0;
    let mut defaulted = 0;
    let mut acl = null_mut();
    unsafe {
        check(GetSecurityDescriptorDacl(
            descriptor.0,
            &mut present,
            &mut acl,
            &mut defaulted,
        ))?;
        if present == 0 || acl.is_null() {
            return Err(unsafe_file());
        }
        let result = SetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            acl,
            null_mut(),
        );
        if result != 0 {
            return Err(io::Error::from_raw_os_error(result as i32));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::fs::OpenOptionsExt;

    #[test]
    fn an_ancestor_delete_child_handle_cannot_acquire_delete_on_the_held_root() {
        let root = tempfile::tempdir().unwrap();
        // Acquire the ancestor rights before the protected child exists. They
        // remain granted even if the child receives a restrictive creation ACL.
        let ancestor = std::fs::OpenOptions::new()
            .access_mode(FILE_LIST_DIRECTORY | FILE_ADD_SUBDIRECTORY | FILE_DELETE_CHILD)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(root.path())
            .unwrap();
        let directory = create_protected_directory_at(&ancestor, OsStr::new("sealed")).unwrap();
        let error = relative(
            &ancestor,
            OsStr::new("sealed"),
            DELETE | SYNCHRONIZE,
            FILE_OPEN,
            FILE_OPEN_REPARSE_POINT,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
        )
        .expect_err("pre-acquired ancestor DELETE_CHILD must not defeat root sharing protection");
        assert_eq!(error.raw_os_error(), Some(ERROR_SHARING_VIOLATION as i32));
        assert!(root.path().join("sealed").is_dir());
        remove_handle(&directory).unwrap();
    }
}
