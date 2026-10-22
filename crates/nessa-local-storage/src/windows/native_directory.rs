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

/// Create an absent root, verifying modern inheritance before permitting additions.
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
    let file = create_readonly_directory_at(parent, name, sharing)?;
    modern_enable(&file)?;
    enable_directory_append(&file)?;
    Ok(file)
}

// Exclusive birth and current readonly ACL keep this newly owned namespace
// empty. Only the original trusted handle has BUILD authority; actual child
// operations still check the current readonly parent ACL (native phase tests).
fn create_readonly_directory_at(parent: &File, name: &OsStr, sharing: u32) -> io::Result<File> {
    let descriptor = User::current()?.descriptor_with_access(true, "FRFX")?;
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

// Call only on the empty, readonly directory from the private birth helper,
// before any append grant. The provider may traverse children; none can exist
// at this phase. Never use this provider operation after ADD becomes available.
fn modern_enable(file: &File) -> io::Result<()> {
    let descriptor = User::current()?.descriptor_with_access(true, "FRFX")?;
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
    }
    if present == 0 || acl.is_null() {
        return Err(unsafe_file());
    }
    let status = unsafe {
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
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    require_directory(file)?;
    require_modern_directory(file)
}

fn require_modern_directory(file: &File) -> io::Result<()> {
    if directory_control(file)? & (SE_DACL_PROTECTED | SE_DACL_AUTO_INHERITED)
        != (SE_DACL_PROTECTED | SE_DACL_AUTO_INHERITED)
    {
        return Err(io::Error::other(
            "directory did not retain protected modern inheritance",
        ));
    }
    Ok(())
}

fn enable_directory_append(file: &File) -> io::Result<()> {
    require_modern_directory(file)?;
    set_directory_descriptor(file, append_descriptor()?)?;
    require_directory(file)?;
    require_modern_directory(file)
}

fn append_descriptor() -> io::Result<LocalAllocation> {
    let access = FILE_GENERIC_READ | FILE_GENERIC_EXECUTE | FILE_ADD_FILE | FILE_ADD_SUBDIRECTORY;
    User::current()?.descriptor_with_access(true, &format!("0x{access:08x}"))
}

/// Retain an existing private session directory without changing its ACL.
/// The original no-DELETE-sharing handle pins its name; required original
/// addition/control rights are checked before a source or leaf is created.
pub fn open_session_directory_at(parent: &File, name: &OsStr) -> io::Result<File> {
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
    Ok(file)
}

/// Create an absent session directory with its final protected full ACL.
/// Retain the original no-DELETE-sharing handle; never seal or release this ACL.
pub fn create_session_directory_at(parent: &File, name: &OsStr) -> io::Result<File> {
    let descriptor = User::current()?.descriptor(true)?;
    let file = relative(
        parent,
        name,
        MAXIMUM_ALLOWED | DIRECTORY_BUILD_ACCESS | DELETE,
        FILE_CREATE,
        FILE_DIRECTORY_FILE,
        FILE_SHARE_READ | FILE_SHARE_WRITE,
        Some(&descriptor),
    )?;
    require_directory(&file)?;
    require_directory_access(&file, DIRECTORY_BUILD_ACCESS | DELETE)?;
    Ok(file)
}

/// Exact protected DACL of one owned stage/copy directory.
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

    /// Native control helper for exact-byte and identity characterization tests.
    /// Production exposes capture only; arbitrary directories cannot be restored.
    #[cfg(test)]
    fn restore(&self, file: &File) -> io::Result<()> {
        self.restore_with(file, |descriptor| unsafe {
            set_directory_security(file, descriptor)
        })
    }

    #[cfg(test)]
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
        // restore is restricted to owned stage/copy directory authority.
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
    create_file_with_sharing(parent, name, FILE_SHARE_READ | FILE_SHARE_DELETE)
}

/// Create a fresh session publication inode with lifetime WRITE/DELETE exclusion.
/// Keep this original handle through own-handle rename, ACL release and audit.
pub fn create_pinned_file_at(parent: &File, name: &OsStr) -> io::Result<File> {
    create_file_with_sharing(parent, name, FILE_SHARE_READ)
}

fn create_file_with_sharing(parent: &File, name: &OsStr, sharing: u32) -> io::Result<File> {
    let descriptor = User::current()?.descriptor_with_access(false, "FR")?;
    let file = relative(
        parent,
        name,
        GENERIC_READ | GENERIC_WRITE | DELETE | WRITE_DAC | SYNCHRONIZE,
        FILE_CREATE,
        0,
        sharing,
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

/// Terminal release of one owned directory after publication or retirement.
/// Production callers complete namespace sealing before this release; the
/// low-level admission check requires private protected modern state.
/// The raw assignment deliberately
/// ends in the protected legacy model; no subsequent provider transition or
/// retry is permitted. Exact full-access DACL and terminal controls are read back.
pub fn release_directory_writes(file: &File) -> io::Result<()> {
    require_directory(file)?;
    require_modern_directory(file)?;
    let descriptor = User::current()?.descriptor(true)?;
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
    }
    if present == 0 || acl.is_null() {
        return Err(unsafe_file());
    }
    let expected = unsafe { std::slice::from_raw_parts(acl.cast::<u8>(), (*acl).AclSize as usize) };
    unsafe {
        set_native_descriptor(file, descriptor.0)?;
    }
    require_directory(file)?;
    let actual = DirectoryAcl::capture(file)?;
    let actual_bytes =
        unsafe { std::slice::from_raw_parts(actual.words.as_ptr().cast::<u8>(), actual.length) };
    let controls = SE_DACL_PROTECTED | SE_DACL_AUTO_INHERITED | SE_DACL_AUTO_INHERIT_REQ;
    if actual_bytes != expected || actual.control & controls != SE_DACL_PROTECTED {
        return Err(io::Error::other("terminal directory release did not retain exact full-access ACL and protected legacy controls"));
    }
    Ok(())
}

/// Refuse further additions before final whole-tree identity/content validation.
/// Native tests exercise real creation/rename through pre-acquired ADD handles.
/// Requires the original MAX protected-constructor handle or its duplicate.
/// A verified reopen is not qualified for provider sealing; it is used only
/// for terminal raw release. Ownership alone does not establish isolation.
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
    require_modern_directory(file)?;
    unsafe {
        check(SetSecurityDescriptorControl(
            descriptor.0,
            SE_DACL_PROTECTED | SE_DACL_AUTO_INHERITED | SE_DACL_AUTO_INHERIT_REQ,
            SE_DACL_PROTECTED | SE_DACL_AUTO_INHERITED,
        ))?;
        set_directory_security(file, descriptor.0)
    }
}

// Directory-only provider transition on an already modern, private owned
// directory. Callers retain the original MAX constructor handle or a duplicate.
// Reopened handles are reserved for terminal raw release, not this provider path.
// Model-zero directories are refused before any effect.
// Files never enter this path or its fallible post-assignment readback.
unsafe fn set_directory_security(file: &File, descriptor: PSECURITY_DESCRIPTOR) -> io::Result<()> {
    require_directory(file)?;
    require_modern_directory(file)?;
    let mut input = 0;
    let mut revision = 0;
    let mut present = 0;
    let mut defaulted = 0;
    let mut acl = null_mut();
    unsafe {
        check(GetSecurityDescriptorControl(
            descriptor,
            &mut input,
            &mut revision,
        ))?;
        check(GetSecurityDescriptorDacl(
            descriptor,
            &mut present,
            &mut acl,
            &mut defaulted,
        ))?;
    }
    let model = SE_DACL_PROTECTED | SE_DACL_AUTO_INHERITED;
    if input & model != model
        || input & SE_DACL_AUTO_INHERIT_REQ != 0
        || present == 0
        || acl.is_null()
    {
        return Err(unsafe_file());
    }
    let status = unsafe {
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
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    require_directory(file)?;
    require_modern_directory(file)
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

// Raw mechanics for file permissions and terminal directory release only.
// The directory append/seal provider path checks the modern model;
// its original MAX provenance is supplied by owned constructors or duplicates.
// It never enters the writable-file window.
unsafe fn set_native_descriptor(file: &File, descriptor: PSECURITY_DESCRIPTOR) -> io::Result<()> {
    // Protection is carried in the full descriptor itself. Neither bc6's
    // high-bit selector nor 09c's low selector established foreign-child
    // preservation. Terminal directory release has its own exact postcondition.
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
    // Directory entrypoints and test-only restoration own protected-ACL readback.
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/support/windows_token.rs"]
mod token;

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::fs::{FileExt, OpenOptionsExt};

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

    fn native_acl_receipt(file: &File) -> (Vec<u8>, SECURITY_DESCRIPTOR_CONTROL) {
        // NtQuerySecurityObject guarantees a 64KiB buffer suffices for NTFS SDs.
        // Align the self-relative descriptor, and query only READ_CONTROL data.
        let mut buffer = vec![0usize; 65536 / size_of::<usize>()];
        let mut needed = 0;
        let descriptor = buffer.as_mut_ptr().cast();
        let status = unsafe {
            windows_sys::Wdk::Storage::FileSystem::NtQuerySecurityObject(
                file.as_raw_handle(),
                DACL_SECURITY_INFORMATION,
                descriptor,
                65536,
                &mut needed,
            )
        };
        assert!(
            status >= 0,
            "native security receipt failed: status={status:#010x}"
        );
        assert!(needed as usize <= 65536);
        let mut acl = null_mut();
        let mut present = 0;
        let mut defaulted = 0;
        let mut control = 0;
        let mut revision = 0;
        unsafe {
            check(GetSecurityDescriptorDacl(
                descriptor,
                &mut present,
                &mut acl,
                &mut defaulted,
            ))
            .unwrap();
            check(GetSecurityDescriptorControl(
                descriptor,
                &mut control,
                &mut revision,
            ))
            .unwrap();
        }
        assert_ne!(present, 0);
        assert!(!acl.is_null());
        let bytes = unsafe {
            std::slice::from_raw_parts(acl.cast::<u8>(), (*acl).AclSize as usize).to_vec()
        };
        (bytes, control)
    }

    fn log_acl_receipt(
        phase: &str,
        receipt: &(Vec<u8>, SECURITY_DESCRIPTOR_CONTROL),
        baseline: &(Vec<u8>, SECURITY_DESCRIPTOR_CONTROL),
    ) {
        eprintln!("native-link-query phase={phase} exact_baseline={} control={:#06x} acl_bytes={} ace_type_flags_masks={:?}",
            receipt == baseline, receipt.1, receipt.0.len(), acl_shape(&receipt.0).into_iter().take(16).collect::<Vec<_>>());
    }

    fn log_native_pair_ordered(
        phase: &str,
        held: &File,
        target: Option<&File>,
        baseline: &(Vec<u8>, SECURITY_DESCRIPTOR_CONTROL),
        target_first: bool,
    ) {
        eprintln!(
            "native-link-query phase={phase} native_query_order={}",
            if target.is_none() {
                "held-only"
            } else if target_first {
                "target-then-held"
            } else {
                "held-then-target"
            }
        );
        let (original, target_receipt) = if target_first {
            let target_receipt = target.map(native_acl_receipt);
            if let Some(receipt) = &target_receipt {
                log_acl_receipt(&format!("{phase}/native-target"), receipt, baseline);
            }
            let original = native_acl_receipt(held);
            log_acl_receipt(&format!("{phase}/native-held"), &original, baseline);
            (original, target_receipt)
        } else {
            let original = native_acl_receipt(held);
            log_acl_receipt(&format!("{phase}/native-held"), &original, baseline);
            let target_receipt = target.map(native_acl_receipt);
            if let Some(receipt) = &target_receipt {
                log_acl_receipt(&format!("{phase}/native-target"), receipt, baseline);
            }
            (original, target_receipt)
        };
        if let Some(target_receipt) = target_receipt {
            eprintln!(
                "native-link-query phase={phase} native_held_target_equal={}",
                original == target_receipt
            );
        }
    }

    fn legacy_birth_parent(path: &std::path::Path) -> File {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
            .unwrap();
        let receipt = native_acl_receipt(&file);
        log_acl_receipt("ordinary-birth/native-parent", &receipt, &receipt);
        assert_eq!(receipt.1 & SE_DACL_AUTO_INHERITED, 0);
        let inherit = OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE;
        assert!(
            acl_shape(&receipt.0)
                .iter()
                .any(|(_, flags, mask)| u32::from(*flags) & inherit == inherit
                    && *mask == FILE_ALL_ACCESS),
            "actual stored legacy parent must offer inheritable full access"
        );
        file
    }

    fn assert_native_pair(
        phase: &str,
        held: &File,
        target: Option<&File>,
        baseline: &(Vec<u8>, SECURITY_DESCRIPTOR_CONTROL),
    ) {
        let original = native_acl_receipt(held);
        log_acl_receipt(&format!("{phase}/stored-held"), &original, baseline);
        assert!(
            original == *baseline,
            "stored held ACL/control changed: {phase}"
        );
        if let Some(target) = target {
            let receipt = native_acl_receipt(target);
            log_acl_receipt(&format!("{phase}/stored-target"), &receipt, baseline);
            assert!(
                receipt == *baseline,
                "stored target ACL/control changed: {phase}"
            );
        }
        // Provider copies are observations of this opened name, never the
        // immutable native storage baseline or an assertion across names.
        log_acl_receipt(
            &format!("{phase}/provider-held"),
            &acl_receipt(held),
            baseline,
        );
        if let Some(target) = target {
            log_acl_receipt(
                &format!("{phase}/provider-target"),
                &acl_receipt(target),
                baseline,
            );
            assert!(
                native_acl_receipt(target) == *baseline,
                "provider query changed stored target ACL: {phase}"
            );
        }
        assert!(
            native_acl_receipt(held) == *baseline,
            "provider query changed stored held ACL: {phase}"
        );
    }

    fn log_directory_views(phase: &str, file: &File) {
        let native = native_acl_receipt(file);
        let provider = acl_receipt(file);
        eprintln!("native-directory-views phase={phase} native_control={:#06x} provider_control={:#06x} exact_equal={}",
            native.1, provider.1, native == provider);
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
            let receipt = acl_receipt(&handle);
            assert_eq!(
                receipt.1 & SE_DACL_PROTECTED,
                0,
                "test setup must produce a genuinely unprotected foreign object"
            );
            assert!(
                acl_shape(&receipt.0)
                    .iter()
                    .any(|(_, flags, _)| u32::from(*flags) & INHERITED_ACE != 0),
                "ordinary inheritance must supply a real inherited ACE alongside explicit FA"
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
        assert!(
            bytes.len() >= size_of::<ACL>(),
            "ACL receipt lacks its complete header"
        );
        let length = u16::from_le_bytes([bytes[2], bytes[3]]) as usize;
        let count = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
        assert_eq!(
            length,
            bytes.len(),
            "ACL receipt must contain exactly AclSize bytes"
        );
        let mut entries = Vec::with_capacity(count);
        let mut offset = size_of::<ACL>();
        for _ in 0..count {
            assert!(
                offset + size_of::<ACE_HEADER>() <= length,
                "declared ACE header exceeds AclSize"
            );
            let size = u16::from_le_bytes([bytes[offset + 2], bytes[offset + 3]]) as usize;
            assert!(
                size >= 8 && size.is_multiple_of(4),
                "ACE shape needs an aligned header and mask"
            );
            assert!(
                size <= length - offset,
                "declared ACE extends beyond AclSize"
            );
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
        eprintln!("native-acl-layout declared_count={count} parsed_count={} acl_size={length} bytes_in_use={offset} free_bytes={}",
            entries.len(), length - offset);
        entries
    }

    #[test]
    fn acl_shape_ignores_fake_inherited_ace_in_free_slack() {
        let mut bytes = vec![0u8; 48];
        bytes[0] = 2;
        bytes[2..4].copy_from_slice(&48u16.to_le_bytes());
        bytes[4..6].copy_from_slice(&1u16.to_le_bytes());
        for offset in [8, 28] {
            bytes[offset + 2..offset + 4].copy_from_slice(&20u16.to_le_bytes());
            bytes[offset + 4..offset + 8].copy_from_slice(&FILE_GENERIC_READ.to_le_bytes());
            bytes[offset + 8..offset + 20].copy_from_slice(&[1, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0]);
        }
        bytes[29] = INHERITED_ACE as u8;
        let parsed = acl_shape(&bytes);
        assert_eq!(parsed.len(), 1);
        assert!(
            parsed
                .iter()
                .all(|(_, flags, _)| u32::from(*flags) & INHERITED_ACE == 0),
            "slack must never establish a genuine inherited-ACE fixture precondition"
        );
    }

    #[test]
    fn acl_shape_parses_every_declared_entry_beyond_sixteen() {
        let mut bytes = vec![0u8; 8 + 17 * 20];
        let length = bytes.len() as u16;
        bytes[0] = 2;
        bytes[2..4].copy_from_slice(&length.to_le_bytes());
        bytes[4..6].copy_from_slice(&17u16.to_le_bytes());
        for index in 0..17 {
            let offset = 8 + index * 20;
            bytes[offset + 2..offset + 4].copy_from_slice(&20u16.to_le_bytes());
            bytes[offset + 4..offset + 8].copy_from_slice(&FILE_GENERIC_READ.to_le_bytes());
            bytes[offset + 8..offset + 20].copy_from_slice(&[1, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0]);
        }
        bytes[8 + 16 * 20 + 1] = INHERITED_ACE as u8;
        let parsed = acl_shape(&bytes);
        assert_eq!(parsed.len(), 17);
        assert_ne!(u32::from(parsed[16].1) & INHERITED_ACE, 0);
    }

    #[test]
    fn fresh_hardlink_security_queries_reverse_provider_order_without_inheritance_assumptions() {
        let _ordinary = token::OrdinaryToken::enter();
        for target_first in [false, true] {
            let root = tempfile::tempdir().unwrap();
            crate::create_directory_exclusive(&root.path().join("legacy-query")).unwrap();
            let source = root.path().join("legacy-query/source");
            let payload = b"query-order bytes unchanged";
            std::fs::write(&source, payload).unwrap();
            let held = std::fs::OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
                .open(&source)
                .unwrap();
            let before = native_acl_receipt(&held);
            let native_pair = |phase: &str, target: Option<&File>| {
                log_native_pair_ordered(phase, &held, target, &before, target_first);
                assert!(
                    native_acl_receipt(&held) == before,
                    "fresh stored held security changed: {phase}"
                );
                if let Some(target) = target {
                    assert!(
                        native_acl_receipt(target) == before,
                        "fresh stored target security changed: {phase}"
                    );
                }
            };
            log_acl_receipt("fresh/baseline-provider-held", &acl_receipt(&held), &before);
            native_pair("fresh/baseline-native", None);
            let identity = NativeObjectId::capture(&held).unwrap();
            let ancestor = full_parent(root.path());
            let directory =
                create_protected_directory_at(&ancestor, OsStr::new("owned-query")).unwrap();
            let target_path = root.path().join("owned-query/linked");
            std::fs::hard_link(&source, &target_path).unwrap();
            native_pair("fresh/after-link-before-open", None);
            let target = std::fs::OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
                .open(&target_path)
                .unwrap();
            assert_eq!(NativeObjectId::capture(&target).unwrap(), identity);
            native_pair("fresh/after-open-before-provider", Some(&target));
            eprintln!("native-link-query fresh_target_provider_first={target_first}");
            if target_first {
                log_acl_receipt(
                    "fresh/first-target-provider",
                    &acl_receipt(&target),
                    &before,
                );
                native_pair("fresh/after-target-provider", Some(&target));
                log_acl_receipt("fresh/second-held-provider", &acl_receipt(&held), &before);
            } else {
                log_acl_receipt("fresh/first-held-provider", &acl_receipt(&held), &before);
                native_pair("fresh/after-held-provider", Some(&target));
                log_acl_receipt(
                    "fresh/second-target-provider",
                    &acl_receipt(&target),
                    &before,
                );
            }
            native_pair("fresh/after-both-provider", Some(&target));
            std::fs::remove_file(&source).unwrap();
            assert!(!source.exists());
            assert_eq!(information(&held).unwrap().nNumberOfLinks, 1);
            native_pair("fresh/after-unlink-before-verify", Some(&target));
            eprintln!(
                "native-link-query fresh_private_file_admitted={}",
                verify_file(&target).is_ok()
            );
            native_pair("fresh/after-verify", Some(&target));
            log_acl_receipt(
                "fresh/held-provider-after-verify",
                &acl_receipt(&held),
                &before,
            );
            log_acl_receipt(
                "fresh/target-provider-after-verify",
                &acl_receipt(&target),
                &before,
            );
            let mut bytes = vec![0u8; payload.len()];
            assert_eq!(held.seek_read(&mut bytes, 0).unwrap(), payload.len());
            assert_eq!(bytes.as_slice(), payload);
            assert_eq!(std::fs::read(&target_path).unwrap(), payload);
            std::fs::write(&target_path, payload).unwrap();
            assert_eq!(NativeObjectId::capture(&held).unwrap(), identity);
            assert_eq!(NativeObjectId::capture(&target).unwrap(), identity);
            assert_eq!(std::fs::read(&target_path).unwrap(), payload);
            assert!(native_acl_receipt(&held) == before);
            assert!(native_acl_receipt(&target) == before);
            // Provider presentations remain diagnostics; stored security has
            // its own immutable native baseline throughout these ordinary ops.
            drop((directory, target, held));
        }
    }

    fn diagnose_foreign_difference(
        before: &[(Vec<u8>, SECURITY_DESCRIPTOR_CONTROL)],
        after: &[(Vec<u8>, SECURITY_DESCRIPTOR_CONTROL)],
    ) {
        for (index, (old, new)) in before.iter().zip(after).enumerate() {
            if old != new {
                eprintln!("native-foreign-acl child={index} dacl_equal={} control={:#06x}->{:#06x} acl_bytes={}->{} ace_type_flags_masks={:?}->{:?}",
                    old.0 == new.0, old.1, new.1, old.0.len(), new.0.len(), acl_shape(&old.0).into_iter().take(16).collect::<Vec<_>>(), acl_shape(&new.0).into_iter().take(16).collect::<Vec<_>>());
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
    fn readonly_initialization_denies_actual_additions_until_verified_modern_append() {
        let _ordinary = token::OrdinaryToken::enter();
        let root = tempfile::tempdir().unwrap();
        let ancestor = full_parent(root.path());
        let ordinary_external = root.path().join("ordinary-external");
        std::fs::write(&ordinary_external, b"ordinary movable bytes").unwrap();
        std::fs::rename(
            &ordinary_external,
            root.path().join("ordinary-external-moved"),
        )
        .expect("ordinary full-ACL source actually relocates in a full parent");
        std::fs::rename(
            root.path().join("ordinary-external-moved"),
            &ordinary_external,
        )
        .unwrap();
        let external = create_protected_file_at(&ancestor, OsStr::new("external")).unwrap();
        rename_handle_at(&external, &ancestor, OsStr::new("external-moved"))
            .expect("positive control: external inode can actually relocate in a full parent");
        rename_handle_at(&external, &ancestor, OsStr::new("external")).unwrap();
        let directory = create_readonly_directory_at(
            &ancestor,
            OsStr::new("phases"),
            FILE_SHARE_READ | FILE_SHARE_WRITE,
        )
        .unwrap();
        let preacquired = directory.try_clone().unwrap();
        let deny = |phase: &str, ordinary: Option<&File>| {
            for held in [&directory, &preacquired].into_iter().chain(ordinary) {
                assert_eq!(
                    create_protected_file_at(held, OsStr::new("blocked-file"))
                        .unwrap_err()
                        .raw_os_error(),
                    Some(ERROR_ACCESS_DENIED as i32),
                    "original/preacquired ADD cannot create a file: {phase}"
                );
                assert_eq!(
                    create_protected_subdirectory_at(held, OsStr::new("blocked-dir"))
                        .unwrap_err()
                        .raw_os_error(),
                    Some(ERROR_ACCESS_DENIED as i32),
                    "original/preacquired ADD cannot create a directory: {phase}"
                );
                assert_eq!(
                    rename_handle_at(&external, held, OsStr::new("blocked-rename"))
                        .unwrap_err()
                        .raw_os_error(),
                    Some(ERROR_ACCESS_DENIED as i32),
                    "original/preacquired ADD cannot receive an external rename: {phase}"
                );
            }
            assert!(
                relative(
                    &ancestor,
                    OsStr::new("phases"),
                    FILE_ADD_FILE | FILE_ADD_SUBDIRECTORY | SYNCHRONIZE,
                    FILE_OPEN,
                    FILE_OPEN_REPARSE_POINT,
                    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                    None
                )
                .is_err(),
                "ordinary opener cannot gain ADD authority: {phase}"
            );
            assert!(
                std::fs::write(root.path().join("phases/ordinary-file"), b"forbidden").is_err()
            );
            assert!(std::fs::create_dir(root.path().join("phases/ordinary-dir")).is_err());
            assert!(std::fs::rename(
                &ordinary_external,
                root.path().join("phases/ordinary-rename")
            )
            .is_err());
            assert!(root.path().join("external").is_file());
            assert_eq!(
                std::fs::read(&ordinary_external).unwrap(),
                b"ordinary movable bytes"
            );
            assert!(relative_names(&directory).unwrap().is_empty());
        };
        deny("readonly-birth", None);
        modern_enable(&directory).unwrap();
        require_modern_directory(&directory).unwrap();
        deny("readonly-modern-before-append", None);
        enable_directory_append(&directory).unwrap();
        let ordinary = relative(
            &ancestor,
            OsStr::new("phases"),
            FILE_ADD_FILE | FILE_ADD_SUBDIRECTORY | SYNCHRONIZE,
            FILE_OPEN,
            FILE_OPEN_REPARSE_POINT,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
        )
        .expect("ordinary caller gains actual ADD only after verified modern append");
        for held in [&directory, &preacquired, &ordinary] {
            let file = create_protected_file_at(held, OsStr::new("allowed-file")).unwrap();
            remove_handle(&file).unwrap();
            drop(file);
            let child = create_protected_subdirectory_at(held, OsStr::new("allowed-dir")).unwrap();
            remove_handle(&child).unwrap();
            drop(child);
            rename_handle_at(&external, held, OsStr::new("allowed-rename"))
                .expect("same retained ADD authority receives relocation after modern append");
            rename_handle_at(&external, &ancestor, OsStr::new("external")).unwrap();
        }
        std::fs::write(
            root.path().join("phases/ordinary-file"),
            b"ordinary created bytes",
        )
        .expect("same ordinary file creation succeeds after append");
        std::fs::create_dir(root.path().join("phases/ordinary-dir"))
            .expect("same ordinary directory creation succeeds after append");
        std::fs::rename(
            &ordinary_external,
            root.path().join("phases/ordinary-rename"),
        )
        .expect("same ordinary full-ACL source relocates after append");
        std::fs::rename(
            root.path().join("phases/ordinary-rename"),
            &ordinary_external,
        )
        .unwrap();
        // Disposable ordinary children may inherit no DELETE. Grant cleanup
        // authority on these exact empty fixture objects, never on the parent.
        for (name, is_directory) in [("ordinary-file", false), ("ordinary-dir", true)] {
            let path = root.path().join("phases").join(name);
            let metadata = std::fs::OpenOptions::new()
                .access_mode(GENERIC_READ | WRITE_DAC)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
                .open(&path)
                .unwrap();
            let descriptor = User::current().unwrap().descriptor(is_directory).unwrap();
            unsafe {
                set_native_descriptor(&metadata, descriptor.0).unwrap();
            }
            drop(metadata);
            if is_directory {
                std::fs::remove_dir(path).unwrap();
            } else {
                std::fs::remove_file(path).unwrap();
            }
        }
        seal_directory_writes(&directory).unwrap();
        deny("sealed-after-append", Some(&ordinary));
        remove_handle(&external).unwrap();
        remove_handle(&directory).unwrap();
    }

    #[test]
    fn raw_legacy_assignment_distinguishes_stored_changes_from_provider_view_changes() {
        let _ordinary = token::OrdinaryToken::enter();
        let root = tempfile::tempdir().unwrap();
        crate::create_directory_exclusive(&root.path().join("legacy-owned")).unwrap();
        let _birth_parent = legacy_birth_parent(&root.path().join("legacy-owned"));
        let ancestor = full_parent(root.path());
        let directory = open_session_directory_at(&ancestor, OsStr::new("legacy-owned")).unwrap();
        let children = foreign_children(&root.path().join("legacy-owned"));
        let before: Vec<_> = children.iter().map(native_acl_receipt).collect();
        for (_, control) in &before {
            assert_eq!(control & (SE_DACL_PROTECTED | SE_DACL_AUTO_INHERITED), 0);
        }
        let provider_before: Vec<_> = children.iter().map(acl_receipt).collect();
        assert!(children.iter().map(native_acl_receipt).collect::<Vec<_>>() == before);
        let input = User::current()
            .unwrap()
            .descriptor_with_access(true, "FRFX")
            .unwrap();
        unsafe {
            set_native_descriptor(&directory, input.0).unwrap();
        }
        let after: Vec<_> = children.iter().map(native_acl_receipt).collect();
        let provider_after: Vec<_> = children.iter().map(acl_receipt).collect();
        if after == before {
            assert!(provider_after != provider_before,
                "unchanged stored ACLs must still reproduce the historical provider-view counterexample");
            eprintln!("native-legacy-assignment stored_children_changed=false provider_views_changed=true");
        } else {
            assert!(after != before);
            eprintln!(
                "native-legacy-assignment stored_children_changed=true provider_views_changed={}",
                provider_after != provider_before
            );
        }
    }

    #[test]
    fn modern_owned_directory_preserves_stored_prepared_foreign_acls_through_seal_and_release() {
        let _ordinary = token::OrdinaryToken::enter();
        let root = tempfile::tempdir().unwrap();
        let ancestor = full_parent(root.path());
        let directory = create_protected_directory_at(&ancestor, OsStr::new("max-owned")).unwrap();
        let children = inject_foreign_children(&root.path().join("max-owned"));
        let before: Vec<_> = children.iter().map(native_acl_receipt).collect();
        for (_, control) in &before {
            assert_eq!(
                control & SE_DACL_PROTECTED,
                0,
                "injected production children must actually be unprotected"
            );
        }
        let parent_before = directory_control(&directory).unwrap();
        let assert_foreign = |transition: &str| {
            let after: Vec<_> = children.iter().map(native_acl_receipt).collect();
            if after != before {
                diagnose_foreign_difference(&before, &after);
                eprintln!("native-owned-parent transition={transition} original_control={parent_before:#06x} current_control={:#06x}", directory_control(&directory).unwrap());
            }
            assert!(after == before,
                "modern-born directory must preserve exact prepared child ACL/control: {transition}");
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
    fn verified_max_reopen_release_preserves_stored_prepared_foreign_acls() {
        let _ordinary = token::OrdinaryToken::enter();
        let root = tempfile::tempdir().unwrap();
        let ancestor = full_parent(root.path());
        let directory = create_protected_directory_at(&ancestor, OsStr::new("owned")).unwrap();
        let child = create_protected_subdirectory_at(&directory, OsStr::new("child")).unwrap();
        let identity = NativeObjectId::capture(&child).unwrap();
        let foreign_path = root.path().join("owned/child");
        let children = inject_foreign_children(&foreign_path);
        let before: Vec<_> = children.iter().map(native_acl_receipt).collect();
        let assert_foreign = |phase: &str| {
            let after: Vec<_> = children.iter().map(native_acl_receipt).collect();
            if before != after {
                diagnose_foreign_difference(&before, &after);
            }
            assert!(
                before == after,
                "verified MAX reopen preserves exact foreign ACL/control: {phase}"
            );
            for relative in ["other-chat", "foreign-directory/grandchild"] {
                let path = foreign_path.join(relative);
                std::fs::write(&path, b"other chat unchanged").unwrap();
                assert_eq!(std::fs::read(path).unwrap(), b"other chat unchanged");
            }
        };
        seal_directory_writes(&child).unwrap();
        assert_foreign("initial-seal");
        drop(child);
        let wrong = NativeObjectId::capture(&directory).unwrap();
        assert!(
            open_verified_directory_at(&directory, OsStr::new("child"), wrong).is_err(),
            "wrong saved identity cannot return ACL mutation authority"
        );
        assert_foreign("wrong-ID-refusal");
        let reopened =
            open_verified_directory_at(&directory, OsStr::new("child"), identity).unwrap();
        assert_foreign("verified-MAX-reopen");
        release_directory_writes(&reopened).unwrap();
        assert_foreign("verified-MAX-release");
    }

    #[test]
    fn verified_reopen_provider_release_mutates_modern_foreign_acls_positive_control() {
        let _ordinary = token::OrdinaryToken::enter();
        let root = tempfile::tempdir().unwrap();
        let ancestor = full_parent(root.path());
        let directory =
            create_protected_directory_at(&ancestor, OsStr::new("max-control")).unwrap();
        let child = create_protected_subdirectory_at(&directory, OsStr::new("child")).unwrap();
        let identity = NativeObjectId::capture(&child).unwrap();
        // Ordinary children retain their parent's append ACL; no explicit FA
        // setup can conceal whether the old provider actually grants FA.
        let children = foreign_children(&root.path().join("max-control/child"));
        let before: Vec<_> = children.iter().map(native_acl_receipt).collect();
        seal_directory_writes(&child).unwrap();
        assert!(
            children.iter().map(native_acl_receipt).collect::<Vec<_>>() == before,
            "original mixed-MAX constructor must preserve this exact stored ordinary-child fixture"
        );
        drop(child);
        let reopened =
            open_verified_directory_at(&directory, OsStr::new("child"), identity).unwrap();
        require_modern_directory(&reopened).unwrap();
        legacy_directory_assignment(
            &reopened,
            User::current().unwrap().descriptor(true).unwrap(),
        );
        require_modern_directory(&reopened).unwrap();
        let after: Vec<_> = children.iter().map(native_acl_receipt).collect();
        if after == before {
            diagnose_foreign_difference(&before, &after);
        }
        for relative in ["other-chat", "foreign-directory/grandchild"] {
            std::fs::write(
                root.path().join("max-control/child").join(relative),
                b"other chat unchanged",
            )
            .expect(
                "actual old provider full-access propagation grants ordinary existing-file writes",
            );
        }
        assert!(after != before,
            "old provider release through the actual verified MAX reopen must change exact stored ordinary-child ACL/control, not merely provider views");
    }

    #[test]
    fn legacy_hardlink_and_ordinary_directory_move_preserve_foreign_acls_through_terminal_release()
    {
        let _ordinary = token::OrdinaryToken::enter();
        for reopen in [false, true] {
            let root = tempfile::tempdir().unwrap();
            crate::create_directory_exclusive(&root.path().join("legacy")).unwrap();
            let _birth_parent = legacy_birth_parent(&root.path().join("legacy"));
            let alias = root.path().join("legacy/external");
            // Ordinary creation supplies no explicit child security descriptor.
            std::fs::write(&alias, b"external alias unchanged").unwrap();
            let external = std::fs::OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
                .open(&alias)
                .unwrap();
            let before = native_acl_receipt(&external);
            assert_eq!(before.1 & (SE_DACL_PROTECTED | SE_DACL_AUTO_INHERITED), 0);
            assert_native_pair("multi/birth-before-link", &external, None, &before);
            let external_id = NativeObjectId::capture(&external).unwrap();
            let ancestor = full_parent(root.path());
            let owner = create_protected_directory_at(&ancestor, OsStr::new("owned")).unwrap();
            let directory =
                create_protected_subdirectory_at(&owner, OsStr::new("destination")).unwrap();
            let identity = NativeObjectId::capture(&directory).unwrap();
            let destination = root.path().join("owned/destination");
            std::fs::hard_link(&alias, destination.join("foreign-link")).unwrap();
            assert_native_pair("multi/link-before-open", &external, None, &before);
            let linked = std::fs::OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
                .open(destination.join("foreign-link"))
                .unwrap();
            assert_eq!(NativeObjectId::capture(&linked).unwrap(), external_id);
            assert_native_pair("multi/target-open", &external, Some(&linked), &before);
            std::fs::create_dir(root.path().join("legacy/empty-directory")).unwrap();
            let moved = std::fs::OpenOptions::new()
                .access_mode(GENERIC_READ | DELETE)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
                .open(root.path().join("legacy/empty-directory"))
                .unwrap();
            let moved_before = native_acl_receipt(&moved);
            assert_eq!(
                moved_before.1 & (SE_DACL_PROTECTED | SE_DACL_AUTO_INHERITED),
                0
            );
            let moved_id = NativeObjectId::capture(&moved).unwrap();
            rename_handle_at(&moved, &directory, OsStr::new("foreign-empty")).unwrap();
            let moved_after = native_acl_receipt(&moved);
            eprintln!(
                "native-ordinary-directory-move controls={:#06x}->{:#06x} stored_changed={}",
                moved_before.1,
                moved_after.1,
                moved_before != moved_after
            );
            let check = |phase: &str| {
                assert_native_pair(phase, &external, Some(&linked), &before);
                assert!(
                    native_acl_receipt(&moved) == moved_after,
                    "stored moved directory ACL/control changed: {phase}"
                );
                assert_eq!(NativeObjectId::capture(&linked).unwrap(), external_id);
                assert_eq!(NativeObjectId::capture(&external).unwrap(), external_id);
                assert_eq!(NativeObjectId::capture(&moved).unwrap(), moved_id);
                assert_eq!(std::fs::read(&alias).unwrap(), b"external alias unchanged");
                assert_eq!(
                    std::fs::read(destination.join("foreign-link")).unwrap(),
                    b"external alias unchanged"
                );
                std::fs::write(&alias, b"external alias unchanged").unwrap();
                std::fs::write(
                    destination.join("foreign-link"),
                    b"external alias unchanged",
                )
                .unwrap();
                assert_native_pair(
                    &format!("{phase}/after-ordinary-reads-writes"),
                    &external,
                    Some(&linked),
                    &before,
                );
            };
            check("multi/ordinary-link-and-move");
            log_directory_views("multi/before-seal", &directory);
            seal_directory_writes(&directory).unwrap();
            log_directory_views("multi/after-seal", &directory);
            check("multi/qualified-provider-seal");
            let held = if reopen {
                drop(directory);
                open_verified_directory_at(&owner, OsStr::new("destination"), identity).unwrap()
            } else {
                directory
            };
            log_directory_views("multi/before-terminal-release", &held);
            release_directory_writes(&held).unwrap();
            log_directory_views("multi/after-terminal-release", &held);
            check("multi/terminal-release");
            let terminal = native_acl_receipt(&held);
            assert!(seal_directory_writes(&held).is_err());
            assert!(release_directory_writes(&held).is_err());
            assert!(native_acl_receipt(&held) == terminal);
            check("multi/terminal-no-retry");
        }
    }

    #[test]
    fn legacy_single_link_foreign_inode_preserves_acl_and_bytes_through_seal_and_terminal_release()
    {
        let _ordinary = token::OrdinaryToken::enter();
        for reopen in [false, true] {
            let root = tempfile::tempdir().unwrap();
            crate::create_directory_exclusive(&root.path().join("legacy")).unwrap();
            let _birth_parent = legacy_birth_parent(&root.path().join("legacy"));
            let external_name = root.path().join("legacy/external");
            let payload = b"single-link foreign unchanged";
            std::fs::write(&external_name, payload).unwrap();
            let external = std::fs::OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
                .open(&external_name)
                .unwrap();
            let identity = NativeObjectId::capture(&external).unwrap();
            let before = native_acl_receipt(&external);
            assert_eq!(before.1 & (SE_DACL_PROTECTED | SE_DACL_AUTO_INHERITED), 0);
            assert_native_pair("single/birth-before-link", &external, None, &before);
            let ancestor = full_parent(root.path());
            let owner =
                create_protected_directory_at(&ancestor, OsStr::new("owned-single")).unwrap();
            let directory =
                create_protected_subdirectory_at(&owner, OsStr::new("destination")).unwrap();
            let directory_id = NativeObjectId::capture(&directory).unwrap();
            let target = root.path().join("owned-single/destination/foreign");
            std::fs::hard_link(&external_name, &target).unwrap();
            assert_eq!(information(&external).unwrap().nNumberOfLinks, 2);
            assert_native_pair("single/after-link", &external, None, &before);
            std::fs::remove_file(&external_name).unwrap();
            assert_native_pair(
                "single/after-unlink-before-target-open",
                &external,
                None,
                &before,
            );
            let linked = std::fs::OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
                .open(&target)
                .unwrap();
            assert_eq!(NativeObjectId::capture(&linked).unwrap(), identity);
            assert_eq!(information(&linked).unwrap().nNumberOfLinks, 1);
            assert_native_pair(
                "single/target-open-before-verify",
                &external,
                Some(&linked),
                &before,
            );
            eprintln!(
                "native-single-link-foreign provider_private_file_admitted={}",
                verify_file(&linked).is_ok()
            );
            assert_native_pair(
                "single/after-provider-verify",
                &external,
                Some(&linked),
                &before,
            );
            let check = |phase: &str| {
                assert!(
                    !external_name.exists(),
                    "test-removed external name stays absent: {phase}"
                );
                assert_native_pair(phase, &external, Some(&linked), &before);
                assert_eq!(NativeObjectId::capture(&external).unwrap(), identity);
                assert_eq!(NativeObjectId::capture(&linked).unwrap(), identity);
                assert_eq!(information(&external).unwrap().nNumberOfLinks, 1);
                let mut bytes = vec![0; payload.len()];
                assert_eq!(external.seek_read(&mut bytes, 0).unwrap(), payload.len());
                assert_eq!(bytes.as_slice(), payload);
                assert_eq!(std::fs::read(&target).unwrap(), payload);
                std::fs::write(&target, payload).unwrap();
                assert_native_pair(
                    &format!("{phase}/after-ordinary-reads-writes"),
                    &external,
                    Some(&linked),
                    &before,
                );
            };
            check("single/after-link-and-external-unlink");
            log_directory_views("single/before-seal", &directory);
            seal_directory_writes(&directory).unwrap();
            log_directory_views("single/after-seal", &directory);
            check("single/qualified-provider-seal");
            let held = if reopen {
                drop(directory);
                open_verified_directory_at(&owner, OsStr::new("destination"), directory_id).unwrap()
            } else {
                directory
            };
            log_directory_views("single/before-terminal-release", &held);
            release_directory_writes(&held).unwrap();
            log_directory_views("single/after-terminal-release", &held);
            check("single/terminal-raw-release");
        }
    }

    #[test]
    fn model_zero_directory_transition_is_refused_before_parent_or_foreign_acl_effects() {
        let _ordinary = token::OrdinaryToken::enter();
        let root = tempfile::tempdir().unwrap();
        let ancestor = full_parent(root.path());
        let directory = create_session_directory_at(&ancestor, OsStr::new("model-zero")).unwrap();
        assert_eq!(
            directory_control(&directory).unwrap() & SE_DACL_AUTO_INHERITED,
            0,
            "causal fixture must actually start in model zero"
        );
        let original = DirectoryAcl::capture(&directory).unwrap();
        let children = foreign_children(&root.path().join("model-zero"));
        let before: Vec<_> = children.iter().map(acl_receipt).collect();
        for transition in [seal_directory_writes, release_directory_writes] {
            assert!(
                transition(&directory).is_err(),
                "model-zero refusal precedes provider assignment"
            );
            assert!(DirectoryAcl::capture(&directory).unwrap() == original);
            assert!(children.iter().map(acl_receipt).collect::<Vec<_>>() == before);
            for relative in ["other-chat", "foreign-directory/grandchild"] {
                std::fs::write(
                    root.path().join("model-zero").join(relative),
                    b"other chat unchanged",
                )
                .unwrap();
            }
        }
    }

    #[test]
    fn raw_native_assignment_clears_modern_bit_even_when_input_carries_it() {
        let _ordinary = token::OrdinaryToken::enter();
        let root = tempfile::tempdir().unwrap();
        let ancestor = full_parent(root.path());
        let directory = create_readonly_directory_at(
            &ancestor,
            OsStr::new("native-control"),
            FILE_SHARE_READ | FILE_SHARE_WRITE,
        )
        .unwrap();
        modern_enable(&directory).unwrap();
        require_modern_directory(&directory).unwrap();
        let input = append_descriptor().unwrap();
        unsafe {
            check(SetSecurityDescriptorControl(
                input.0,
                SE_DACL_PROTECTED | SE_DACL_AUTO_INHERITED | SE_DACL_AUTO_INHERIT_REQ,
                SE_DACL_PROTECTED | SE_DACL_AUTO_INHERITED,
            ))
            .unwrap();
            set_native_descriptor(&directory, input.0).unwrap();
        }
        assert_eq!(
            directory_control(&directory).unwrap() & SE_DACL_AUTO_INHERITED,
            0,
            "raw native DACL assignment demonstrates the observed model-clearing counterexample"
        );
    }

    #[test]
    fn legacy_directory_assignment_mutates_unprotected_foreign_descendants_positive_control() {
        let _ordinary = token::OrdinaryToken::enter();
        let root = tempfile::tempdir().unwrap();
        crate::create_directory_exclusive(&root.path().join("legacy")).unwrap();
        let _birth_parent = legacy_birth_parent(&root.path().join("legacy"));
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
        let before: Vec<_> = children.iter().map(native_acl_receipt).collect();
        for (_, control) in &before {
            assert_eq!(
                control & SE_DACL_PROTECTED,
                0,
                "legacy control must be unprotected"
            );
        }
        legacy_directory_assignment(&directory, append_descriptor().unwrap());
        let after_append: Vec<_> = children.iter().map(native_acl_receipt).collect();
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
        let after_seal: Vec<_> = children.iter().map(native_acl_receipt).collect();
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
    fn session_lifetime_pins_preserve_parent_and_foreign_acls_at_every_transition_and_cleanup() {
        let _ordinary = token::OrdinaryToken::enter();
        for create_new in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let ancestor = full_parent(root.path());
            let directory = if create_new {
                create_session_directory_at(&ancestor, OsStr::new("sessions")).unwrap()
            } else {
                crate::create_directory_exclusive(&root.path().join("sessions")).unwrap();
                open_session_directory_at(&ancestor, OsStr::new("sessions")).unwrap()
            };
            let original = DirectoryAcl::capture(&directory).unwrap();
            let children = foreign_children(&root.path().join("sessions"));
            let before: Vec<_> = children.iter().map(acl_receipt).collect();
            assert!(before
                .iter()
                .all(|(_, control)| control & SE_DACL_PROTECTED == 0));
            let assert_foreign = |transition: &str| {
                let after: Vec<_> = children.iter().map(acl_receipt).collect();
                if after != before {
                    diagnose_foreign_difference(&before, &after);
                }
                assert!(
                    after == before,
                    "session pins preserve exact foreign ACL/control: {transition}"
                );
                assert!(
                    DirectoryAcl::capture(&directory).unwrap() == original,
                    "session parent ACL is never mutated: {transition}"
                );
                for relative in ["other-chat", "foreign-directory/grandchild"] {
                    let path = root.path().join("sessions").join(relative);
                    std::fs::write(&path, b"other chat unchanged").unwrap();
                    assert_eq!(std::fs::read(path).unwrap(), b"other chat unchanged");
                }
            };
            assert_foreign("open");
            let output = create_pinned_file_at(&directory, OsStr::new("candidate")).unwrap();
            let sidecar = create_pinned_file_at(&directory, OsStr::new("sidecar")).unwrap();
            assert_foreign("create");
            rename_handle_at(&output, &directory, OsStr::new("canonical")).unwrap();
            assert_foreign("own-rename");
            release_file_writes(&output).unwrap();
            release_file_writes(&sidecar).unwrap();
            assert_foreign("leaf-release-and-available-audit");
            remove_handle(&output).unwrap();
            remove_handle(&sidecar).unwrap();
            drop((output, sidecar));
            assert_foreign("failure-cleanup");
        }
    }

    #[test]
    fn session_admission_refuses_originally_readonly_directory_without_changing_its_acl() {
        let _ordinary = token::OrdinaryToken::enter();
        let root = tempfile::tempdir().unwrap();
        crate::create_directory_exclusive(&root.path().join("readonly")).unwrap();
        let ancestor = full_parent(root.path());
        let initial = open_session_directory_at(&ancestor, OsStr::new("readonly")).unwrap();
        legacy_directory_assignment(
            &initial,
            User::current()
                .unwrap()
                .descriptor_with_access(true, "FRFX")
                .unwrap(),
        );
        drop(initial);
        let reader = open_directory_at(&ancestor, OsStr::new("readonly")).unwrap();
        let before = DirectoryAcl::capture(&reader).unwrap();
        let error = open_session_directory_at(&ancestor, OsStr::new("readonly"))
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
        let error = open_session_directory_at(&ancestor, OsStr::new("sessions")).expect_err(
            "N08: existing DELETE authority must refuse acquisition before ACL mutation",
        );
        assert_eq!(error.raw_os_error(), Some(ERROR_SHARING_VIOLATION as i32));
        assert_eq!(DirectoryAcl::capture(&reader).unwrap(), original);
        drop(competing);
    }

    #[test]
    fn preacquired_delete_child_cannot_mutate_pinned_leaf_before_or_after_leaf_acl_release() {
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
        let directory = open_session_directory_at(&ancestor, OsStr::new("sessions")).unwrap();
        let original = DirectoryAcl::capture(&directory).unwrap();
        let before = create_protected_file_at(&directory, OsStr::new("before")).unwrap();
        let delegated = delete_child(&preacquired, OsStr::new("before"))
            .expect("positive control: original full parent delegates leaf DELETE");
        remove_handle(&delegated)
            .expect("unpinned leaf is actually deleted through parent authority");
        drop((delegated, before));
        let leaf = create_pinned_file_at(&directory, OsStr::new("candidate")).unwrap();
        rename_handle_at(&leaf, &directory, OsStr::new("payload"))
            .expect("original DELETE handle can rename its own no-delete-sharing inode");
        for released in [false, true] {
            if released {
                release_file_writes(&leaf).unwrap();
            }
            let delete = delete_child(&preacquired, OsStr::new("payload"));
            let error = delete.unwrap_err();
            assert!(
                matches!(error.raw_os_error(), Some(code) if code == ERROR_ACCESS_DENIED as i32 || code == ERROR_SHARING_VIOLATION as i32),
                "ACL or lifetime sharing must refuse fresh DELETE authority: {error}"
            );
            assert!(std::fs::remove_file(root.path().join("sessions/payload")).is_err());
            assert!(std::fs::rename(
                root.path().join("sessions/payload"),
                root.path().join("stolen")
            )
            .is_err());
            assert!(std::fs::write(root.path().join("sessions/payload"), b"changed").is_err());
            assert!(DirectoryAcl::capture(&directory).unwrap() == original);
            assert!(root.path().join("sessions/payload").is_file());
            assert!(!root.path().join("stolen").exists());
        }
        remove_handle(&leaf).unwrap();
        drop((leaf, directory, preacquired));
        std::fs::write(
            root.path().join("sessions/ordinary"),
            b"unchanged original ACL",
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
        let other = open_session_directory_at(&ancestor, OsStr::new("full")).unwrap();
        let before = DirectoryAcl::capture(&other).unwrap();
        assert!(
            original.restore(&other).is_err(),
            "N10: ACL belongs to its original directory identity"
        );
        assert_eq!(
            DirectoryAcl::capture(&other).unwrap(),
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
