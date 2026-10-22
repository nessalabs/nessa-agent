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
mod closed_file;
mod relative;
pub use closed_file::{
    open_verified_directory_at, open_verified_file_at, reacquire_exclusive_file, seal_file_writes,
    NativeObjectId,
};
pub use relative::{
    open_relative_directory, open_relative_file, relative_entry_kind, relative_names,
    rename_relative_entry_at, RelativeEntryKind,
};

use windows_sys::Wdk::{
    Foundation::OBJECT_ATTRIBUTES,
    Storage::FileSystem::{
        FileAccessInformation, FileRenameInformation, NtCreateFile, NtQueryInformationFile,
        NtSetInformationFile, NtSetSecurityObject, FILE_ACCESS_INFORMATION, FILE_CREATE,
        FILE_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_REPARSE_POINT, FILE_RENAME_INFORMATION,
        FILE_SYNCHRONOUS_IO_NONALERT,
    },
};
use windows_sys::Win32::{
    Foundation::*,
    Security::{Authorization::*, *},
    Storage::FileSystem::*,
    System::{
        SystemServices::MAXIMUM_ALLOWED,
        IO::{IO_STATUS_BLOCK, IO_STATUS_BLOCK_0},
    },
};

const DIRECTORY_METADATA_ACCESS: u32 = FILE_LIST_DIRECTORY
    | FILE_TRAVERSE
    | FILE_READ_ATTRIBUTES
    | READ_CONTROL
    | WRITE_DAC
    | SYNCHRONIZE;
const DIRECTORY_BUILD_ACCESS: u32 =
    DIRECTORY_METADATA_ACCESS | FILE_ADD_FILE | FILE_ADD_SUBDIRECTORY;

fn require_directory_access(file: &File, required: u32) -> io::Result<()> {
    let mut granted = FILE_ACCESS_INFORMATION::default();
    let mut status = IO_STATUS_BLOCK {
        Anonymous: IO_STATUS_BLOCK_0 { Status: 0 },
        Information: 0,
    };
    let result = unsafe {
        NtQueryInformationFile(
            file.as_raw_handle(),
            &mut status,
            (&mut granted as *mut FILE_ACCESS_INFORMATION).cast(),
            size_of::<FILE_ACCESS_INFORMATION>() as u32,
            FileAccessInformation,
        )
    };
    if result < 0 {
        return Err(io::Error::from_raw_os_error(
            unsafe { RtlNtStatusToDosError(result) } as i32,
        ));
    }
    if granted.AccessFlags & required != required {
        return Err(io::Error::from_raw_os_error(ERROR_ACCESS_DENIED as i32));
    }
    Ok(())
}

/// Create an absent root with temporary read/search and addition permissions.
///
/// Its user ACL omits DELETE_CHILD from creation. The initial handle does not
/// share DELETE, protecting this root even when its parent grants DELETE_CHILD.
/// WRITE sharing permits the native rename destination-parent internal open;
/// seal every copied directory before final identity and content validation.
pub fn create_protected_directory_at(parent: &File, name: &OsStr) -> io::Result<File> {
    create_directory_at(parent, name, FILE_SHARE_READ | FILE_SHARE_WRITE)
}

/// Create one append-only child directory. Its parent must also omit DELETE_CHILD.
/// Close descendant handles after sealing and final validation, before root rename.
pub fn create_protected_subdirectory_at(parent: &File, name: &OsStr) -> io::Result<File> {
    create_directory_at(
        parent,
        name,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
    )
}

fn create_directory_at(parent: &File, name: &OsStr, sharing: u32) -> io::Result<File> {
    let descriptor = append_descriptor()?;
    let file = relative(
        parent,
        name,
        MAXIMUM_ALLOWED | DIRECTORY_BUILD_ACCESS | DELETE,
        FILE_CREATE,
        FILE_DIRECTORY_FILE,
        sharing,
        Some(&descriptor),
    )?;
    require_directory(&file)?;
    require_directory_access(&file, DIRECTORY_BUILD_ACCESS | DELETE)?;
    Ok(file)
}

fn append_descriptor() -> io::Result<LocalAllocation> {
    let access = FILE_GENERIC_READ | FILE_GENERIC_EXECUTE | FILE_ADD_FILE | FILE_ADD_SUBDIRECTORY;
    User::current()?.descriptor_with_access(true, &format!("0x{access:08x}"))
}

/// Original maximum-access handle for an existing private directory.
///
/// The original maximum-access handle admits the required build/control rights
/// without delete sharing. Parent ACL isolation additionally depends on native
/// full-descriptor updates preserving inheritance-model controls; the MAX open
/// alone did not establish isolation in the 303 native qualification receipt.
#[derive(Debug)]
pub struct MaxAccessDirectory {
    file: File,
}

impl MaxAccessDirectory {
    /// Acquire the original maximum-access handle without delete sharing.
    /// Original required ADD/control rights are checked before any ACL change.
    pub fn open_at(parent: &File, name: &OsStr) -> io::Result<Self> {
        // relative() uses synchronous native I/O, so request SYNCHRONIZE with
        // MAXIMUM_ALLOWED. Role admission is checked before any ACL effect.
        let file = relative(
            parent,
            name,
            MAXIMUM_ALLOWED | SYNCHRONIZE,
            FILE_OPEN,
            FILE_OPEN_REPARSE_POINT,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
        )?;
        require_directory(&file)?;
        require_directory_access(&file, DIRECTORY_BUILD_ACCESS)?;
        Ok(Self { file })
    }

    /// Read this guard's directory authority; ACL methods retain the original handle.
    pub fn file(&self) -> &File {
        &self.file
    }

    /// Permit additions during fetch without changing existing child ACLs.
    pub fn append(&self) -> io::Result<()> {
        set_directory_descriptor(&self.file, append_descriptor()?)?;
        require_directory(&self.file)
    }

    /// Deny further additions before final target validation.
    pub fn seal(&self) -> io::Result<()> {
        set_directory_descriptor(
            &self.file,
            User::current()?.descriptor_with_access(true, "FRFX")?,
        )?;
        require_directory(&self.file)
    }

    /// Restore this same directory's captured ACL with exact control readback.
    pub fn restore(&self, original: &DirectoryAcl) -> io::Result<()> {
        original.restore_with(&self.file, |descriptor| unsafe {
            set_directory_security(&self.file, descriptor)
        })
    }
}

/// Exact protected DACL of one retained directory, for temporary fetch guards.
/// The private storage contract requires a protected current-user-owned DACL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectoryAcl {
    words: Vec<usize>,
    length: usize,
    control: SECURITY_DESCRIPTOR_CONTROL,
    volume: u32,
    index: u64,
}

impl DirectoryAcl {
    /// Capture the ACL bytes and bind restoration to this directory's identity.
    pub fn capture(file: &File) -> io::Result<Self> {
        require_directory(file)?;
        let identity = information(file)?;
        let mut acl = null_mut();
        let mut descriptor = null_mut();
        let result = unsafe {
            GetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                &mut acl,
                null_mut(),
                &mut descriptor,
            )
        };
        if result != 0 {
            return Err(io::Error::from_raw_os_error(result as i32));
        }
        let _descriptor = LocalAllocation(descriptor);
        let mut control = 0;
        let mut revision = 0;
        unsafe {
            check(GetSecurityDescriptorControl(
                descriptor,
                &mut control,
                &mut revision,
            ))?;
        }
        if acl.is_null() {
            return Err(unsafe_file());
        }
        let length = unsafe { (*acl).AclSize as usize };
        if length < size_of::<ACL>() {
            return Err(unsafe_file());
        }
        let mut words = vec![0usize; length.div_ceil(size_of::<usize>())];
        unsafe {
            std::ptr::copy_nonoverlapping(
                acl.cast::<u8>(),
                words.as_mut_ptr().cast::<u8>(),
                length,
            );
        }
        Ok(Self {
            words,
            length,
            control,
            volume: identity.dwVolumeSerialNumber,
            index: (u64::from(identity.nFileIndexHigh) << 32) | u64::from(identity.nFileIndexLow),
        })
    }

    /// Restore the original ACL/control through an original maximum-access directory handle.
    /// `file` must come from a protected-directory constructor or verified directory
    /// reopen in this module. Existing parents use MaxAccessDirectory::restore.
    pub fn restore(&self, file: &File) -> io::Result<()> {
        self.restore_with(file, |descriptor| unsafe {
            set_directory_security(file, descriptor)
        })
    }

    fn restore_with(
        &self,
        file: &File,
        assign: impl FnOnce(PSECURITY_DESCRIPTOR) -> io::Result<()>,
    ) -> io::Result<()> {
        let identity = information(file)?;
        let index = (u64::from(identity.nFileIndexHigh) << 32) | u64::from(identity.nFileIndexLow);
        if identity.dwVolumeSerialNumber != self.volume || index != self.index {
            return Err(unsafe_file());
        }
        // A stack descriptor references the retained exact ACL bytes. The
        // original MAX handle is required for parent-only directory assignment.
        let mut descriptor = SECURITY_DESCRIPTOR::default();
        unsafe {
            check(InitializeSecurityDescriptor(
                (&mut descriptor as *mut SECURITY_DESCRIPTOR).cast(),
                1,
            ))?;
            check(SetSecurityDescriptorDacl(
                (&mut descriptor as *mut SECURITY_DESCRIPTOR).cast(),
                1,
                self.words.as_ptr().cast::<ACL>().cast_mut(),
                i32::from(self.control & SE_DACL_DEFAULTED != 0),
            ))?;
            let dacl_control =
                SE_DACL_PROTECTED | SE_DACL_AUTO_INHERIT_REQ | SE_DACL_AUTO_INHERITED;
            check(SetSecurityDescriptorControl(
                (&mut descriptor as *mut SECURITY_DESCRIPTOR).cast(),
                dacl_control,
                self.control & dacl_control,
            ))?;
            assign((&mut descriptor as *mut SECURITY_DESCRIPTOR).cast())?;
        }
        let restored = Self::capture(file)?;
        if restored != *self {
            return Err(io::Error::other(format!(
                "original directory ACL was not restored (dacl_equal={}, control={:#06x}->{:#06x}, acl_bytes={}->{})",
                restored.words == self.words && restored.length == self.length,
                self.control, restored.control, self.length, restored.length
            )));
        }
        Ok(())
    }
}

/// Exclusively create a private file with temporary read-only user permissions.
/// The initial payload and DELETE authority omit write sharing. Native tests
/// require competing write/deletion operations to fail after this handle closes.
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
/// Windows refuses directory rename while descendant handles remain open.
pub fn rename_handle_at(source: &File, destination: &File, name: &OsStr) -> io::Result<()> {
    let native_name = name;
    let name = name_units(name)?;
    let name_bytes = name.len().checked_mul(2).ok_or_else(unsafe_file)?;
    let bytes = offset_of!(FILE_RENAME_INFORMATION, FileName)
        .checked_add(name_bytes)
        .ok_or_else(unsafe_file)?
        .max(size_of::<FILE_RENAME_INFORMATION>());
    let mut buffer = vec![0usize; bytes.div_ceil(size_of::<usize>())];
    let rename = buffer.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();
    unsafe {
        (*rename).Anonymous.ReplaceIfExists = false;
        (*rename).RootDirectory = destination.as_raw_handle();
        (*rename).FileNameLength = u32::try_from(name_bytes).map_err(|_| unsafe_file())?;
        std::ptr::copy_nonoverlapping(
            name.as_ptr(),
            std::ptr::addr_of_mut!((*rename).FileName).cast(),
            name.len(),
        );
        let mut status = IO_STATUS_BLOCK {
            Anonymous: IO_STATUS_BLOCK_0 { Status: 0 },
            Information: 0,
        };
        let result = NtSetInformationFile(
            source.as_raw_handle(),
            &mut status,
            rename.cast(),
            u32::try_from(bytes).map_err(|_| unsafe_file())?,
            FileRenameInformation,
        );
        if result < 0 {
            let code = RtlNtStatusToDosError(result);
            // An occupied target with an exclusive writer can produce a sharing
            // error before the kernel reports name collision. Observe only that
            // retained-relative target; a missing/unreadable target keeps the
            // original failure, including unrelated sharing conflicts.
            if code == ERROR_SHARING_VIOLATION
                && matches!(relative_entry_kind(destination, native_name), Ok(Some(_)))
            {
                Err(io::Error::from_raw_os_error(ERROR_ALREADY_EXISTS as i32))
            } else {
                Err(io::Error::from_raw_os_error(code as i32))
            }
        } else {
            Ok(())
        }
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
/// Requires the original MAX handle from a protected constructor or verified
/// directory reopen; use MaxAccessDirectory for pre-existing foreign parents.
pub fn release_directory_writes(file: &File) -> io::Result<()> {
    set_directory_descriptor(file, User::current()?.descriptor(true)?)?;
    require_directory(file)
}

/// Refuse further additions before final whole-tree identity/content validation.
/// Native tests exercise real creation/rename through pre-acquired ADD handles.
/// Requires an original MAX handle from a protected constructor or verified
/// directory reopen; ownership alone does not establish child ACL protection.
pub fn seal_directory_writes(file: &File) -> io::Result<()> {
    set_directory_descriptor(file, User::current()?.descriptor_with_access(true, "FRFX")?)?;
    require_directory(file)
}

/// Release temporary file restrictions through the retained WRITE_DAC handle.
/// The caller owns the ordering: call only after successful live publication.
pub fn release_file_writes(file: &File) -> io::Result<()> {
    set_descriptor(file, User::current()?.descriptor(false)?)
}

fn directory_control(file: &File) -> io::Result<SECURITY_DESCRIPTOR_CONTROL> {
    let mut descriptor = null_mut();
    let code = unsafe {
        GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
            &mut descriptor,
        )
    };
    if code != 0 {
        return Err(io::Error::from_raw_os_error(code as i32));
    }
    let _owned = LocalAllocation(descriptor);
    let mut control = 0;
    let mut revision = 0;
    unsafe {
        check(GetSecurityDescriptorControl(
            descriptor,
            &mut control,
            &mut revision,
        ))?;
    }
    Ok(control)
}

fn set_directory_descriptor(file: &File, descriptor: LocalAllocation) -> io::Result<()> {
    // D:P alone drops AUTO_INHERITED model metadata. Preserve the current
    // model on temporary changes; AUTO_INHERIT_REQ would request propagation
    // and is deliberately absent. Exact restoration bypasses this preparation.
    let current = directory_control(file)?;
    if current & SE_DACL_PROTECTED == 0 {
        return Err(unsafe_file());
    }
    unsafe {
        check(SetSecurityDescriptorControl(
            descriptor.0,
            SE_DACL_PROTECTED | SE_DACL_AUTO_INHERITED | SE_DACL_AUTO_INHERIT_REQ,
            current & (SE_DACL_PROTECTED | SE_DACL_AUTO_INHERITED),
        ))?;
        set_directory_security(file, descriptor.0)
    }
}

// Directory-only full-SD assignment: temporary callers supply current model
// controls; restore supplies the exact captured controls. Files do not enter
// this path, so its fallible preparation/readback cannot escape an FRFW window.
unsafe fn set_directory_security(file: &File, descriptor: PSECURITY_DESCRIPTOR) -> io::Result<()> {
    let before = directory_control(file)?;
    let mut input = 0;
    let mut revision = 0;
    unsafe {
        check(GetSecurityDescriptorControl(
            descriptor,
            &mut input,
            &mut revision,
        ))?;
    }
    let status =
        unsafe { NtSetSecurityObject(file.as_raw_handle(), DACL_SECURITY_INFORMATION, descriptor) };
    if status < 0 {
        return Err(io::Error::from_raw_os_error(
            unsafe { RtlNtStatusToDosError(status) } as i32,
        ));
    }
    let after = directory_control(file)?;
    let model = SE_DACL_PROTECTED | SE_DACL_AUTO_INHERITED;
    if input & model != after & model {
        return Err(io::Error::other(format!("directory ACL control changed (before={before:#06x}, input={input:#06x}, after={after:#06x})")));
    }
    Ok(())
}

fn set_descriptor(file: &File, descriptor: LocalAllocation) -> io::Result<()> {
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
        set_native_descriptor(file, descriptor.0)?;
    }
    Ok(())
}

// File-only mechanics. Directory full-SD assignment separately preserves
// inheritance-model metadata and never enters the writable-file window.
unsafe fn set_native_descriptor(file: &File, descriptor: PSECURITY_DESCRIPTOR) -> io::Result<()> {
    // Protection is carried in the full descriptor itself. Neither bc6's
    // high-bit selector nor 09c's low selector established foreign-child
    // preservation; production uses this path only for regular files.
    // File descriptors are prepared before writable permission windows;
    // directory model preparation/readback never enters this file-only path.
    let status =
        unsafe { NtSetSecurityObject(file.as_raw_handle(), DACL_SECURITY_INFORMATION, descriptor) };
    if status < 0 {
        return Err(io::Error::from_raw_os_error(
            unsafe { RtlNtStatusToDosError(status) } as i32,
        ));
    }
    // Do not put fallible readback after mutation here: FRFW callers must
    // reach their unconditional FR reseal before any further fallible work.
    // Directory entrypoints and exact restoration own protected-ACL readback.
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/support/windows_token.rs"]
mod token;

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::fs::OpenOptionsExt;

    fn full_parent(path: &std::path::Path) -> File {
        std::fs::OpenOptions::new()
            .access_mode(FILE_LIST_DIRECTORY | FILE_ADD_FILE | FILE_ADD_SUBDIRECTORY)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
            .unwrap()
    }

    fn delete_child(parent: &File, name: &OsStr) -> io::Result<File> {
        relative(
            parent,
            name,
            DELETE | SYNCHRONIZE,
            FILE_OPEN,
            FILE_OPEN_REPARSE_POINT,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
        )
    }

    fn acl_receipt(file: &File) -> (Vec<u8>, SECURITY_DESCRIPTOR_CONTROL) {
        let mut acl = null_mut();
        let mut descriptor = null_mut();
        let code = unsafe {
            GetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                &mut acl,
                null_mut(),
                &mut descriptor,
            )
        };
        assert_eq!(code, 0);
        let _owned = LocalAllocation(descriptor);
        assert!(!acl.is_null());
        let mut control = 0;
        let mut revision = 0;
        unsafe {
            check(GetSecurityDescriptorControl(
                descriptor,
                &mut control,
                &mut revision,
            ))
            .unwrap();
        }
        let bytes = unsafe {
            std::slice::from_raw_parts(acl.cast::<u8>(), (*acl).AclSize as usize).to_vec()
        };
        (bytes, control)
    }

    fn foreign_children(path: &std::path::Path) -> Vec<File> {
        std::fs::write(path.join("other-chat"), b"other chat unchanged").unwrap();
        std::fs::create_dir(path.join("foreign-directory")).unwrap();
        std::fs::write(
            path.join("foreign-directory/grandchild"),
            b"other chat unchanged",
        )
        .unwrap();
        foreign_handles(path)
    }

    fn foreign_handles(path: &std::path::Path) -> Vec<File> {
        [
            path.join("other-chat"),
            path.join("foreign-directory"),
            path.join("foreign-directory/grandchild"),
        ]
        .iter()
        .map(|path| {
            std::fs::OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
                .open(path)
                .unwrap()
        })
        .collect()
    }

    fn inject_foreign_children(owned: &std::path::Path) -> Vec<File> {
        // Native 303 showed that moving an entry can protect it. Establish the
        // actual required state under this root instead: ordinary creation,
        // then explicit disposable-fixture ACL setup through held handles.
        drop(foreign_children(owned));
        for (name, directory) in [
            ("foreign-directory", true),
            ("other-chat", false),
            ("foreign-directory/grandchild", false),
        ] {
            let handle = std::fs::OpenOptions::new()
                .access_mode(GENERIC_READ | WRITE_DAC)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
                .open(owned.join(name))
                .unwrap();
            let descriptor = User::current().unwrap().descriptor(directory).unwrap();
            let mut acl = null_mut();
            let mut present = 0;
            let mut defaulted = 0;
            unsafe {
                check(GetSecurityDescriptorDacl(
                    descriptor.0,
                    &mut present,
                    &mut acl,
                    &mut defaulted,
                ))
                .unwrap();
            }
            assert_ne!(present, 0);
            assert!(!acl.is_null());
            let status = unsafe {
                SetSecurityInfo(
                    handle.as_raw_handle(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION | UNPROTECTED_DACL_SECURITY_INFORMATION,
                    null_mut(),
                    null_mut(),
                    acl,
                    null_mut(),
                )
            };
            assert_eq!(status, 0);
            assert_eq!(
                acl_receipt(&handle).1 & SE_DACL_PROTECTED,
                0,
                "test setup must produce a genuinely unprotected foreign object"
            );
        }
        for name in ["other-chat", "foreign-directory/grandchild"] {
            std::fs::write(owned.join(name), b"other chat unchanged").expect(
                "unprotected foreign fixture has actual full existing-file write authority",
            );
        }
        foreign_handles(owned)
    }

    fn acl_shape(bytes: &[u8]) -> Vec<(u8, u8, u32)> {
        let mut entries = Vec::new();
        let mut offset = 8usize;
        while entries.len() < 16 && offset + 8 <= bytes.len() {
            let size = u16::from_le_bytes([bytes[offset + 2], bytes[offset + 3]]) as usize;
            if size < 8 || size > bytes.len() - offset {
                break;
            }
            entries.push((
                bytes[offset],
                bytes[offset + 1],
                u32::from_le_bytes([
                    bytes[offset + 4],
                    bytes[offset + 5],
                    bytes[offset + 6],
                    bytes[offset + 7],
                ]),
            ));
            offset += size;
        }
        entries
    }

    fn diagnose_foreign_difference(
        before: &[(Vec<u8>, SECURITY_DESCRIPTOR_CONTROL)],
        after: &[(Vec<u8>, SECURITY_DESCRIPTOR_CONTROL)],
    ) {
        for (index, (old, new)) in before.iter().zip(after).enumerate() {
            if old != new {
                eprintln!("native-foreign-acl child={index} dacl_equal={} control={:#06x}->{:#06x} acl_bytes={}->{} ace_type_flags_masks={:?}->{:?}",
                    old.0 == new.0, old.1, new.1, old.0.len(), new.0.len(), acl_shape(&old.0), acl_shape(&new.0));
            }
        }
    }

    fn legacy_directory_assignment(file: &File, descriptor: LocalAllocation) {
        let mut present = 0;
        let mut defaulted = 0;
        let mut acl = null_mut();
        unsafe {
            check(GetSecurityDescriptorDacl(
                descriptor.0,
                &mut present,
                &mut acl,
                &mut defaulted,
            ))
            .unwrap();
        }
        assert_ne!(present, 0);
        assert!(!acl.is_null());
        let code = unsafe {
            SetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                acl,
                null_mut(),
            )
        };
        assert_eq!(code, 0);
    }

    #[test]
    fn legacy_native_directory_seal_changes_foreign_acl_after_ordinary_injection() {
        let _ordinary = token::OrdinaryToken::enter();
        let root = tempfile::tempdir().unwrap();
        let ancestor = full_parent(root.path());
        let descriptor = append_descriptor().unwrap();
        // Reproduce the original explicit-access creator, rather than silently
        // giving this causal control the new MAX provenance.
        let directory = relative(
            &ancestor,
            OsStr::new("legacy-owned"),
            DIRECTORY_BUILD_ACCESS | DELETE,
            FILE_CREATE,
            FILE_DIRECTORY_FILE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            Some(&descriptor),
        )
        .unwrap();
        // Put this causal control in the modern inheritance model before any
        // foreign object exists. Observe that state, rather than infer it from FA.
        legacy_directory_assignment(&directory, append_descriptor().unwrap());
        let parent_before = directory_control(&directory).unwrap();
        assert!(
            parent_before & SE_DACL_AUTO_INHERITED != 0,
            "legacy causal setup must actually establish the modern inheritance model"
        );
        let children = inject_foreign_children(&root.path().join("legacy-owned"));
        let before: Vec<_> = children.iter().map(acl_receipt).collect();
        for (_, control) in &before {
            assert_eq!(
                control & SE_DACL_PROTECTED,
                0,
                "injected causal children must actually be unprotected"
            );
        }
        let sealed = User::current()
            .unwrap()
            .descriptor_with_access(true, "FRFX")
            .unwrap();
        unsafe {
            set_native_descriptor(&directory, sealed.0).unwrap();
        }
        let after: Vec<_> = children.iter().map(acl_receipt).collect();
        if after == before {
            let parent_after = directory_control(&directory).unwrap();
            eprintln!("native-legacy-causal control_before={parent_before:#06x} input_model={:#06x} control_after={parent_after:#06x} foreign_changed=false", SE_DACL_PROTECTED);
        }
        assert!(
            after != before,
            "legacy native directory seal must demonstrate the injected-child mutation hazard"
        );
        legacy_directory_assignment(
            &directory,
            User::current().unwrap().descriptor(true).unwrap(),
        );
    }

    #[test]
    fn original_max_owned_directory_preserves_injected_foreign_acls_through_seal_and_release() {
        let _ordinary = token::OrdinaryToken::enter();
        let root = tempfile::tempdir().unwrap();
        let ancestor = full_parent(root.path());
        let directory = create_protected_directory_at(&ancestor, OsStr::new("max-owned")).unwrap();
        let children = inject_foreign_children(&root.path().join("max-owned"));
        let before: Vec<_> = children.iter().map(acl_receipt).collect();
        for (_, control) in &before {
            assert_eq!(
                control & SE_DACL_PROTECTED,
                0,
                "injected production children must actually be unprotected"
            );
        }
        let parent_before = directory_control(&directory).unwrap();
        let assert_foreign = |transition: &str| {
            let after: Vec<_> = children.iter().map(acl_receipt).collect();
            if after != before {
                diagnose_foreign_difference(&before, &after);
                eprintln!("native-owned-parent transition={transition} original_control={parent_before:#06x} current_control={:#06x}", directory_control(&directory).unwrap());
            }
            assert!(after == before,
                "MAX-created directory must preserve exact injected child ACL/control: {transition}");
            for relative in ["other-chat", "foreign-directory/grandchild"] {
                let path = root.path().join("max-owned").join(relative);
                std::fs::write(&path, b"other chat unchanged")
                    .expect("injected foreign inode keeps original ordinary write authority");
                assert_eq!(std::fs::read(path).unwrap(), b"other chat unchanged");
            }
        };
        assert_foreign("before-seal");
        seal_directory_writes(&directory).unwrap();
        assert_foreign("after-seal");
        release_directory_writes(&directory).unwrap();
        assert_foreign("after-release");
    }

    #[test]
    fn legacy_directory_assignment_mutates_unprotected_foreign_descendants_positive_control() {
        let _ordinary = token::OrdinaryToken::enter();
        let root = tempfile::tempdir().unwrap();
        crate::create_directory_exclusive(&root.path().join("legacy")).unwrap();
        let ancestor = full_parent(root.path());
        let directory = relative(
            &ancestor,
            OsStr::new("legacy"),
            FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | READ_CONTROL | WRITE_DAC | SYNCHRONIZE,
            FILE_OPEN,
            FILE_OPEN_REPARSE_POINT,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
        )
        .unwrap();
        let children = foreign_children(&root.path().join("legacy"));
        let before: Vec<_> = children.iter().map(acl_receipt).collect();
        for (_, control) in &before {
            assert_eq!(
                control & SE_DACL_PROTECTED,
                0,
                "legacy control must be unprotected"
            );
        }
        legacy_directory_assignment(&directory, append_descriptor().unwrap());
        let after_append: Vec<_> = children.iter().map(acl_receipt).collect();
        for (old, new) in before.iter().zip(&after_append) {
            assert!(
                old != new,
                "legacy append must demonstrate the foreign-child inheritance hazard"
            );
        }
        legacy_directory_assignment(
            &directory,
            User::current()
                .unwrap()
                .descriptor_with_access(true, "FRFX")
                .unwrap(),
        );
        let after_seal: Vec<_> = children.iter().map(acl_receipt).collect();
        for (old, new) in before.iter().zip(&after_seal) {
            assert!(
                old != new,
                "legacy seal must demonstrate the foreign-child inheritance hazard"
            );
        }
        legacy_directory_assignment(
            &directory,
            User::current().unwrap().descriptor(true).unwrap(),
        );
    }

    #[test]
    fn native_parent_assignment_preserves_unprotected_foreign_acls_at_every_transition_and_cleanup()
    {
        let _ordinary = token::OrdinaryToken::enter();
        let root = tempfile::tempdir().unwrap();
        crate::create_directory_exclusive(&root.path().join("sessions")).unwrap();
        let ancestor = full_parent(root.path());
        let directory = MaxAccessDirectory::open_at(&ancestor, OsStr::new("sessions")).unwrap();
        let original = DirectoryAcl::capture(directory.file()).unwrap();
        let children = foreign_children(&root.path().join("sessions"));
        let before: Vec<_> = children.iter().map(acl_receipt).collect();
        for (_, control) in &before {
            assert_eq!(
                control & SE_DACL_PROTECTED,
                0,
                "foreign descendants must actually be unprotected"
            );
        }
        let assert_foreign = |transition: &str| {
            let after: Vec<_> = children.iter().map(acl_receipt).collect();
            if after != before {
                diagnose_foreign_difference(&before, &after);
                eprintln!("native-parent-acl transition={transition} original_control={:#06x} current_control={:#06x}", original.control, directory_control(directory.file()).unwrap());
            }
            assert!(after == before,
                "parent-only assignment must preserve exact foreign DACL and security control immediately: {transition}");
            for relative in ["other-chat", "foreign-directory/grandchild"] {
                let path = root.path().join("sessions").join(relative);
                std::fs::write(&path, b"other chat unchanged")
                    .expect("other chat keeps actual existing-file write authority");
                assert_eq!(std::fs::read(path).unwrap(), b"other chat unchanged");
            }
        };
        directory.append().unwrap();
        assert_foreign("append");
        directory.seal().unwrap();
        assert_foreign("seal");
        directory.restore(&original).unwrap();
        assert_foreign("restore");
        struct RestoreGuard<'a> {
            directory: &'a MaxAccessDirectory,
            original: &'a DirectoryAcl,
        }
        impl Drop for RestoreGuard<'_> {
            fn drop(&mut self) {
                self.directory
                    .restore(self.original)
                    .expect("native failure cleanup restores original parent ACL");
            }
        }
        let failed = (|| -> io::Result<()> {
            let _guard = RestoreGuard {
                directory: &directory,
                original: &original,
            };
            directory.append()?;
            assert_foreign("failure-append");
            directory.seal()?;
            assert_foreign("failure-seal");
            Err(io::Error::other("forced failure after seal"))
        })();
        assert!(failed.is_err());
        assert_foreign("failure-cleanup");
        assert!(
            DirectoryAcl::capture(directory.file()).unwrap() == original,
            "failure cleanup must restore exact original parent DACL/control"
        );
    }

    #[test]
    fn maximum_guard_refuses_originally_readonly_directory_without_changing_its_acl() {
        let _ordinary = token::OrdinaryToken::enter();
        let root = tempfile::tempdir().unwrap();
        crate::create_directory_exclusive(&root.path().join("readonly")).unwrap();
        let ancestor = full_parent(root.path());
        let initial = MaxAccessDirectory::open_at(&ancestor, OsStr::new("readonly")).unwrap();
        initial.seal().unwrap();
        drop(initial);
        let reader = open_directory_at(&ancestor, OsStr::new("readonly")).unwrap();
        let before = DirectoryAcl::capture(&reader).unwrap();
        let error = MaxAccessDirectory::open_at(&ancestor, OsStr::new("readonly"))
            .expect_err("maximum access must not elevate missing original ADD rights");
        assert_eq!(error.raw_os_error(), Some(ERROR_ACCESS_DENIED as i32));
        assert!(
            DirectoryAcl::capture(&reader).unwrap() == before,
            "read-only admission refusal must precede any ACL mutation"
        );
    }

    #[test]
    fn an_existing_delete_opener_refuses_sessions_directory_acquisition_without_acl_changes() {
        let _ordinary = token::OrdinaryToken::enter();
        let root = tempfile::tempdir().unwrap();
        crate::create_directory_exclusive(&root.path().join("sessions")).unwrap();
        let ancestor = full_parent(root.path());
        let reader = open_directory_at(&ancestor, OsStr::new("sessions")).unwrap();
        let original = DirectoryAcl::capture(&reader).unwrap();
        let competing = delete_child(&ancestor, OsStr::new("sessions")).unwrap();
        let error = MaxAccessDirectory::open_at(&ancestor, OsStr::new("sessions")).expect_err(
            "N08: existing DELETE authority must refuse acquisition before ACL mutation",
        );
        assert_eq!(error.raw_os_error(), Some(ERROR_SHARING_VIOLATION as i32));
        assert_eq!(DirectoryAcl::capture(&reader).unwrap(), original);
        drop(competing);
    }

    #[test]
    fn existing_directory_preacquired_delete_child_cannot_delete_a_new_readonly_leaf_after_append()
    {
        let _ordinary = token::OrdinaryToken::enter();
        let root = tempfile::tempdir().unwrap();
        crate::create_directory_exclusive(&root.path().join("sessions")).unwrap();
        let ancestor = full_parent(root.path());
        let preacquired = relative(
            &ancestor,
            OsStr::new("sessions"),
            FILE_DELETE_CHILD | FILE_ADD_FILE | SYNCHRONIZE,
            FILE_OPEN,
            FILE_OPEN_REPARSE_POINT,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
        )
        .unwrap();
        let guard = MaxAccessDirectory::open_at(&ancestor, OsStr::new("sessions")).unwrap();
        let directory = guard.file();
        let original = DirectoryAcl::capture(directory).unwrap();
        let before = create_protected_file_at(directory, OsStr::new("before")).unwrap();
        let delegated = delete_child(&preacquired, OsStr::new("before"))
            .expect("positive control: original full parent DACL delegates actual leaf deletion");
        remove_handle(&delegated)
            .expect("positive control: DELETE_CHILD actually removes leaf before append");
        drop((delegated, before));

        guard.append().unwrap();
        let leaf = create_protected_file_at(directory, OsStr::new("payload")).unwrap();
        match delete_child(&preacquired, OsStr::new("payload")) {
            Err(error) => assert_eq!(error.raw_os_error(), Some(ERROR_ACCESS_DENIED as i32)),
            Ok(competing) => {
                let error = remove_handle(&competing).expect_err(
                    "N09: retained DELETE_CHILD cannot delete a new FR leaf after append",
                );
                assert_eq!(error.raw_os_error(), Some(ERROR_ACCESS_DENIED as i32));
                let error = rename_handle_at(&competing, &ancestor, OsStr::new("stolen"))
                    .expect_err(
                        "N09: retained DELETE_CHILD cannot relocate a new FR leaf after append",
                    );
                assert_eq!(error.raw_os_error(), Some(ERROR_ACCESS_DENIED as i32));
            }
        }
        assert!(root.path().join("sessions/payload").is_file());
        assert!(!root.path().join("stolen").exists());
        guard.seal().unwrap();
        release_file_writes(&leaf).unwrap();
        guard.restore(&original).unwrap();
        assert_eq!(DirectoryAcl::capture(directory).unwrap(), original);
        remove_handle(&leaf).unwrap();
        drop((leaf, guard, preacquired));
        std::fs::write(
            root.path().join("sessions/ordinary"),
            b"after exact ACL restoration",
        )
        .unwrap();
    }

    #[test]
    fn directory_acl_restores_nondefault_append_permissions_instead_of_default_full_access() {
        let _ordinary = token::OrdinaryToken::enter();
        let root = tempfile::tempdir().unwrap();
        let ancestor = full_parent(root.path());
        let directory = create_protected_directory_at(&ancestor, OsStr::new("append")).unwrap();
        let original = DirectoryAcl::capture(&directory).unwrap();
        seal_directory_writes(&directory).unwrap();
        assert_ne!(DirectoryAcl::capture(&directory).unwrap(), original);
        original
            .restore(&directory)
            .expect("N10: restore exact append ACL, not ordinary full access");
        assert_eq!(DirectoryAcl::capture(&directory).unwrap(), original);
        let leaf = create_protected_file_at(&directory, OsStr::new("payload"))
            .expect("restored original append permission admits file creation");
        match delete_child(&directory, OsStr::new("payload")) {
            Err(error) => assert_eq!(error.raw_os_error(), Some(ERROR_ACCESS_DENIED as i32)),
            Ok(competing) => {
                let error = remove_handle(&competing)
                    .expect_err("restoring original append ACL must not introduce DELETE_CHILD");
                assert_eq!(error.raw_os_error(), Some(ERROR_ACCESS_DENIED as i32));
            }
        }
        assert!(root.path().join("append/payload").is_file());
        remove_handle(&leaf).unwrap();
        drop(leaf);
        remove_handle(&directory).unwrap();
    }

    #[test]
    fn directory_acl_refuses_another_directory_before_changing_its_acl() {
        let _ordinary = token::OrdinaryToken::enter();
        let root = tempfile::tempdir().unwrap();
        let ancestor = full_parent(root.path());
        let append = create_protected_directory_at(&ancestor, OsStr::new("append")).unwrap();
        let original = DirectoryAcl::capture(&append).unwrap();
        crate::create_directory_exclusive(&root.path().join("full")).unwrap();
        let other = MaxAccessDirectory::open_at(&ancestor, OsStr::new("full")).unwrap();
        let before = DirectoryAcl::capture(other.file()).unwrap();
        assert!(
            other.restore(&original).is_err(),
            "N10: ACL belongs to its original directory identity"
        );
        assert_eq!(
            DirectoryAcl::capture(other.file()).unwrap(),
            before,
            "wrong-directory refusal must precede any ACL mutation"
        );
        drop(other);
        std::fs::write(
            root.path().join("full/ordinary"),
            b"unchanged full permissions",
        )
        .unwrap();
        remove_handle(&append).unwrap();
    }

    #[test]
    fn an_ancestor_delete_child_handle_cannot_acquire_delete_on_the_held_root() {
        let _ordinary = token::OrdinaryToken::enter();
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

    #[test]
    fn sealing_refuses_actual_mutations_through_preacquired_add_handles() {
        let _ordinary = token::OrdinaryToken::enter();
        let root = tempfile::tempdir().unwrap();
        let ancestor = std::fs::OpenOptions::new()
            .access_mode(FILE_LIST_DIRECTORY | FILE_ADD_FILE | FILE_ADD_SUBDIRECTORY)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(root.path())
            .unwrap();
        let directory = create_protected_directory_at(&ancestor, OsStr::new("sealed")).unwrap();
        let child = create_protected_subdirectory_at(&directory, OsStr::new("child")).unwrap();
        // Acquire real ADD rights while copying permits additions. RootDirectory
        // retains this same object after its ACL changes; no path reopening occurs.
        let preacquired = relative(
            &directory,
            OsStr::new("child"),
            FILE_ADD_FILE | FILE_ADD_SUBDIRECTORY | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            FILE_OPEN,
            FILE_OPEN_REPARSE_POINT,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
        )
        .unwrap();
        let before = create_protected_file_at(&preacquired, OsStr::new("before"))
            .expect("positive control: pre-acquired ADD_FILE performs creation before seal");
        remove_handle(&before).unwrap();
        drop(before);
        let before_directory =
            create_protected_subdirectory_at(&preacquired, OsStr::new("before-directory"))
                .expect("positive control: pre-acquired ADD_SUBDIRECTORY works before seal");
        remove_handle(&before_directory).unwrap();
        drop(before_directory);
        let external = create_protected_file_at(&ancestor, OsStr::new("external")).unwrap();
        rename_handle_at(&external, &preacquired, OsStr::new("before-rename"))
            .expect("positive control: pre-acquired ADD_FILE admits a rename before seal");
        rename_handle_at(&external, &ancestor, OsStr::new("external")).unwrap();

        seal_directory_writes(&child).unwrap();
        seal_directory_writes(&directory).unwrap();
        for error in [
            create_protected_file_at(&preacquired, OsStr::new("injected")).unwrap_err(),
            create_protected_subdirectory_at(&preacquired, OsStr::new("injected-directory"))
                .unwrap_err(),
            rename_handle_at(&external, &preacquired, OsStr::new("injected-rename")).unwrap_err(),
        ] {
            assert_eq!(
                error.raw_os_error(),
                Some(ERROR_ACCESS_DENIED as i32),
                "current sealed parent ACL must refuse actual mutation through retained ADD rights"
            );
        }
        assert!(root.path().join("external").is_file());
        assert!(!root.path().join("sealed/child/injected").exists());
        assert!(!root.path().join("sealed/child/injected-directory").exists());
        assert!(!root.path().join("sealed/child/injected-rename").exists());
        drop(preacquired);
        remove_handle(&external).unwrap();
        drop(external);
        remove_handle(&child).unwrap();
        drop(child);
        remove_handle(&directory).unwrap();
    }

    #[test]
    fn root_sealing_refuses_actual_mutations_through_preacquired_add_handles() {
        let _ordinary = token::OrdinaryToken::enter();
        let root = tempfile::tempdir().unwrap();
        let ancestor = std::fs::OpenOptions::new()
            .access_mode(FILE_LIST_DIRECTORY | FILE_ADD_FILE | FILE_ADD_SUBDIRECTORY)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(root.path())
            .unwrap();
        let directory = create_protected_directory_at(&ancestor, OsStr::new("sealed")).unwrap();
        // Acquire real ADD rights while copying permits additions. RootDirectory
        // retains this same object after its ACL changes; no path reopening occurs.
        let preacquired = relative(
            &ancestor,
            OsStr::new("sealed"),
            FILE_ADD_FILE | FILE_ADD_SUBDIRECTORY | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            FILE_OPEN,
            FILE_OPEN_REPARSE_POINT,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
        )
        .unwrap();
        let before = create_protected_file_at(&preacquired, OsStr::new("before"))
            .expect("positive control: pre-acquired ADD_FILE performs creation before seal");
        remove_handle(&before).unwrap();
        drop(before);
        let before_directory =
            create_protected_subdirectory_at(&preacquired, OsStr::new("before-directory"))
                .expect("positive control: pre-acquired ADD_SUBDIRECTORY works before seal");
        remove_handle(&before_directory).unwrap();
        drop(before_directory);
        let external = create_protected_file_at(&ancestor, OsStr::new("external")).unwrap();
        rename_handle_at(&external, &preacquired, OsStr::new("before-rename"))
            .expect("positive control: pre-acquired ADD_FILE admits a rename before seal");
        rename_handle_at(&external, &ancestor, OsStr::new("external")).unwrap();

        seal_directory_writes(&directory).unwrap();
        for error in [
            create_protected_file_at(&preacquired, OsStr::new("injected")).unwrap_err(),
            create_protected_subdirectory_at(&preacquired, OsStr::new("injected-directory"))
                .unwrap_err(),
            rename_handle_at(&external, &preacquired, OsStr::new("injected-rename")).unwrap_err(),
        ] {
            assert_eq!(
                error.raw_os_error(),
                Some(ERROR_ACCESS_DENIED as i32),
                "current sealed parent ACL must refuse actual mutation through retained ADD rights"
            );
        }
        assert!(root.path().join("external").is_file());
        assert!(!root.path().join("sealed/injected").exists());
        assert!(!root.path().join("sealed/injected-directory").exists());
        assert!(!root.path().join("sealed/injected-rename").exists());
        drop(preacquired);
        remove_handle(&external).unwrap();
        drop(external);
        remove_handle(&directory).unwrap();
    }

    #[test]
    fn append_only_parent_does_not_delegate_deletion_of_its_nested_directory() {
        let _ordinary = token::OrdinaryToken::enter();
        let root = tempfile::tempdir().unwrap();
        let ancestor = std::fs::OpenOptions::new()
            .access_mode(FILE_LIST_DIRECTORY | FILE_ADD_FILE | FILE_ADD_SUBDIRECTORY)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(root.path())
            .unwrap();
        let directory = create_protected_directory_at(&ancestor, OsStr::new("sealed")).unwrap();
        let child = create_protected_subdirectory_at(&directory, OsStr::new("child")).unwrap();
        // Test actual mutations, even if a backup-intent opener returns a handle.
        match relative(
            &directory,
            OsStr::new("child"),
            DELETE | SYNCHRONIZE,
            FILE_OPEN,
            FILE_OPEN_REPARSE_POINT,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
        ) {
            Err(error) => assert_eq!(error.raw_os_error(), Some(ERROR_ACCESS_DENIED as i32)),
            Ok(competing) => {
                let error = remove_handle(&competing)
                    .expect_err("nested deletion must be refused from birth");
                assert_eq!(error.raw_os_error(), Some(ERROR_ACCESS_DENIED as i32));
                let error = rename_handle_at(&competing, &ancestor, OsStr::new("stolen"))
                    .expect_err("nested relocation must be refused from birth");
                assert_eq!(error.raw_os_error(), Some(ERROR_ACCESS_DENIED as i32));
            }
        }
        assert!(root.path().join("sealed/child").is_dir());
        assert!(!root.path().join("stolen").exists());
        remove_handle(&child).unwrap();
        drop(child);
        remove_handle(&directory).unwrap();
    }
}
