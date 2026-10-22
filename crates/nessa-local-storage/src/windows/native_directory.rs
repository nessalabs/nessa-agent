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
        FileRenameInformation, NtCreateFile, NtSetInformationFile, FILE_CREATE,
        FILE_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_REPARSE_POINT, FILE_RENAME_INFORMATION,
        FILE_SYNCHRONOUS_IO_NONALERT,
    },
};
use windows_sys::Win32::{
    Foundation::*,
    Security::{Authorization::*, *},
    Storage::FileSystem::*,
    System::IO::{IO_STATUS_BLOCK, IO_STATUS_BLOCK_0},
};

/// Create an absent root with temporary read/search and addition permissions.
///
/// Its user ACL omits DELETE_CHILD from creation. The initial handle does not
/// share DELETE, protecting this root even when its parent grants DELETE_CHILD.
/// Seal every copied directory before final identity and content validation.
pub fn create_protected_directory_at(parent: &File, name: &OsStr) -> io::Result<File> {
    create_directory_at(parent, name, FILE_SHARE_READ)
}

/// Create one append-only child directory. Its parent must also omit DELETE_CHILD.
/// Delete sharing allows the retained root to carry this open child into live.
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
        FILE_LIST_DIRECTORY
            | FILE_TRAVERSE
            | FILE_ADD_FILE
            | FILE_ADD_SUBDIRECTORY
            | FILE_READ_ATTRIBUTES
            | READ_CONTROL
            | WRITE_DAC
            | DELETE
            | SYNCHRONIZE,
        FILE_CREATE,
        FILE_DIRECTORY_FILE,
        sharing,
        Some(&descriptor),
    )?;
    require_directory(&file)?;
    Ok(file)
}

fn append_descriptor() -> io::Result<LocalAllocation> {
    let access = FILE_GENERIC_READ | FILE_GENERIC_EXECUTE | FILE_ADD_FILE | FILE_ADD_SUBDIRECTORY;
    User::current()?.descriptor_with_access(true, &format!("0x{access:08x}"))
}

/// Acquire an existing private directory without delete sharing before mutation.
/// A DELETE handle acquired earlier conflicts with this acquisition. The caller
/// must snapshot its original DACL before installing temporary append permissions.
pub fn retain_directory_at(parent: &File, name: &OsStr) -> io::Result<File> {
    let file = relative(
        parent,
        name,
        FILE_LIST_DIRECTORY
            | FILE_TRAVERSE
            | FILE_ADD_FILE
            | FILE_ADD_SUBDIRECTORY
            | FILE_READ_ATTRIBUTES
            | READ_CONTROL
            | WRITE_DAC
            | SYNCHRONIZE,
        FILE_OPEN,
        FILE_OPEN_REPARSE_POINT,
        FILE_SHARE_READ | FILE_SHARE_WRITE,
        None,
    )?;
    require_directory(&file)?;
    Ok(file)
}

/// Install the same append-only permissions used by new copy directories.
/// Snapshot an existing directory's exact DACL first; restore it after the
/// caller's publication protocol finishes, including failure cleanup.
pub fn append_directory_writes(file: &File) -> io::Result<()> {
    set_descriptor(file, append_descriptor()?)
}

/// Exact protected DACL of one retained directory, for temporary fetch guards.
/// The private storage contract requires a protected current-user-owned DACL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectoryAcl {
    words: Vec<usize>,
    length: usize,
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
            volume: identity.dwVolumeSerialNumber,
            index: (u64::from(identity.nFileIndexHigh) << 32) | u64::from(identity.nFileIndexLow),
        })
    }

    /// Restore only this directory's original protected ACL, then compare bytes.
    pub fn restore(&self, file: &File) -> io::Result<()> {
        let identity = information(file)?;
        let index = (u64::from(identity.nFileIndexHigh) << 32) | u64::from(identity.nFileIndexLow);
        if identity.dwVolumeSerialNumber != self.volume || index != self.index {
            return Err(unsafe_file());
        }
        let result = unsafe {
            SetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                self.words.as_ptr().cast::<ACL>().cast_mut(),
                null_mut(),
            )
        };
        if result != 0 {
            return Err(io::Error::from_raw_os_error(result as i32));
        }
        if Self::capture(file)? != *self {
            return Err(io::Error::other("original directory ACL was not restored"));
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
pub fn rename_handle_at(source: &File, destination: &File, name: &OsStr) -> io::Result<()> {
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
            Err(io::Error::from_raw_os_error(
                RtlNtStatusToDosError(result) as i32
            ))
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
pub fn release_directory_writes(file: &File) -> io::Result<()> {
    release_writes(file, true)
}

/// Refuse further additions before final whole-tree identity/content validation.
/// Native tests exercise real creation/rename through pre-acquired ADD handles.
pub fn seal_directory_writes(file: &File) -> io::Result<()> {
    set_descriptor(file, User::current()?.descriptor_with_access(true, "FRFX")?)
}

/// Release temporary file restrictions through the retained WRITE_DAC handle.
/// The caller owns the ordering: call only after successful live publication.
pub fn release_file_writes(file: &File) -> io::Result<()> {
    release_writes(file, false)
}

fn release_writes(file: &File, directory: bool) -> io::Result<()> {
    set_descriptor(file, User::current()?.descriptor(directory)?)
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

    #[test]
    fn an_existing_delete_opener_refuses_sessions_directory_acquisition_without_acl_changes() {
        let root = tempfile::tempdir().unwrap();
        crate::create_directory_exclusive(&root.path().join("sessions")).unwrap();
        let ancestor = full_parent(root.path());
        let reader = open_directory_at(&ancestor, OsStr::new("sessions")).unwrap();
        let original = DirectoryAcl::capture(&reader).unwrap();
        let competing = delete_child(&ancestor, OsStr::new("sessions")).unwrap();
        let error = retain_directory_at(&ancestor, OsStr::new("sessions")).expect_err(
            "N08: existing DELETE authority must refuse acquisition before ACL mutation",
        );
        assert_eq!(error.raw_os_error(), Some(ERROR_SHARING_VIOLATION as i32));
        assert_eq!(DirectoryAcl::capture(&reader).unwrap(), original);
        drop(competing);
    }

    #[test]
    fn existing_directory_preacquired_delete_child_cannot_delete_a_new_readonly_leaf_after_append()
    {
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
        let directory = retain_directory_at(&ancestor, OsStr::new("sessions")).unwrap();
        let original = DirectoryAcl::capture(&directory).unwrap();
        let before = create_protected_file_at(&directory, OsStr::new("before")).unwrap();
        let delegated = delete_child(&preacquired, OsStr::new("before"))
            .expect("positive control: original full parent DACL delegates actual leaf deletion");
        remove_handle(&delegated)
            .expect("positive control: DELETE_CHILD actually removes leaf before append");
        drop((delegated, before));

        append_directory_writes(&directory).unwrap();
        let leaf = create_protected_file_at(&directory, OsStr::new("payload")).unwrap();
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
        seal_directory_writes(&directory).unwrap();
        release_file_writes(&leaf).unwrap();
        original.restore(&directory).unwrap();
        assert_eq!(DirectoryAcl::capture(&directory).unwrap(), original);
        remove_handle(&leaf).unwrap();
        drop((leaf, directory, preacquired));
        std::fs::write(
            root.path().join("sessions/ordinary"),
            b"after exact ACL restoration",
        )
        .unwrap();
    }

    #[test]
    fn directory_acl_restores_nondefault_append_permissions_instead_of_default_full_access() {
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
        let root = tempfile::tempdir().unwrap();
        let ancestor = full_parent(root.path());
        let append = create_protected_directory_at(&ancestor, OsStr::new("append")).unwrap();
        let original = DirectoryAcl::capture(&append).unwrap();
        crate::create_directory_exclusive(&root.path().join("full")).unwrap();
        let other = retain_directory_at(&ancestor, OsStr::new("full")).unwrap();
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
    fn append_only_parent_does_not_delegate_deletion_of_its_nested_directory() {
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
