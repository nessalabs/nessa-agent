//! Native identity and permission mechanics after descendant handles close.
//!
//! Common publication owns saved component chains and ordering. C01 captures an
//! owned inode ID; C02 verifies each reopened component before ACL authority is
//! returned; C03 opens protected leaves READ|WRITE_DAC; C04 permits FRFW briefly,
//! reacquires the SAME object without write sharing, then restores FR on success
//! and failure; C05 compares ID again before returning writable authority; C06
//! releases verified live objects leaf-first. See retained-directory design.

use super::super::{information, LocalAllocation, User};
use super::{relative, require_directory, set_descriptor, FILE_OPEN, FILE_OPEN_REPARSE_POINT};
use crate::{unsafe_file, verify_file};
use std::{
    ffi::OsStr,
    fs::File,
    io,
    os::windows::io::{AsRawHandle, FromRawHandle},
};
use windows_sys::Win32::{Foundation::*, Storage::FileSystem::*};

/// Native volume and inode identity captured from a retained no-reparse handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeObjectId {
    volume: u32,
    index: u64,
}

impl NativeObjectId {
    /// Capture identity through this object, without resolving its pathname.
    pub fn capture(file: &File) -> io::Result<Self> {
        let info = information(file)?;
        Ok(Self {
            volume: info.dwVolumeSerialNumber,
            index: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
        })
    }

    fn require(self, file: &File) -> io::Result<()> {
        if Self::capture(file)? != self {
            return Err(unsafe_file());
        }
        Ok(())
    }
}

/// Reopen one private directory for reading and ACL control, checking saved ID.
/// Callers walk one saved native component at a time from the retained root.
pub fn open_verified_directory_at(
    parent: &File,
    name: &OsStr,
    expected: NativeObjectId,
) -> io::Result<File> {
    let file = relative(
        parent,
        name,
        FILE_LIST_DIRECTORY
            | FILE_TRAVERSE
            | FILE_READ_ATTRIBUTES
            | READ_CONTROL
            | WRITE_DAC
            | SYNCHRONIZE,
        FILE_OPEN,
        FILE_OPEN_REPARSE_POINT,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        None,
    )?;
    require_directory(&file)?;
    expected.require(&file)?;
    Ok(file)
}

/// Reopen one protected regular file for reading and ACL control, checking ID.
/// No ACL change occurs until the expected retained identity has matched.
pub fn open_verified_file_at(
    parent: &File,
    name: &OsStr,
    expected: NativeObjectId,
) -> io::Result<File> {
    let file = relative(
        parent,
        name,
        GENERIC_READ | WRITE_DAC | SYNCHRONIZE,
        FILE_OPEN,
        FILE_OPEN_REPARSE_POINT,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        None,
    )?;
    verify_file(&file)?;
    expected.require(&file)?;
    Ok(file)
}

/// Restore read-only user file permissions through retained WRITE_DAC authority.
pub fn seal_file_writes(file: &File) -> io::Result<()> {
    verify_file(file)?;
    set_descriptor(file, User::current()?.descriptor_with_access(false, "FR")?)
}

/// Reacquire exact writable cleanup authority, then immediately restore FR.
///
/// The same-object reopen excludes existing writable descriptors. A competing
/// writer winning the temporary FRFW window causes failure; callers must report
/// retained data. No DELETE permission is granted in that window. An error from
/// resealing is returned before any writable handle can escape this function.
pub fn reacquire_exclusive_file(metadata: &File, expected: NativeObjectId) -> io::Result<File> {
    verify_file(metadata)?;
    expected.require(metadata)?;
    // A retained read-only marker can still identify its inode after its name
    // changes. Upgrade metadata authority on that SAME object, never its path.
    let handle = unsafe {
        ReOpenFile(
            metadata.as_raw_handle(),
            GENERIC_READ | WRITE_DAC,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            FILE_FLAG_OPEN_REPARSE_POINT,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let metadata = unsafe { File::from_raw_handle(handle) };
    verify_file(&metadata)?;
    expected.require(&metadata)?;
    // Prepare both descriptors before the permission window. Resealing must
    // not depend on a later token lookup, allocation or SDDL conversion.
    let user = User::current()?;
    let sealed = user.descriptor_with_access(false, "FR")?;
    let writable = user.descriptor_with_access(false, "FRFW")?;
    set_descriptor(&metadata, writable)?;
    exclusive_after_allow(&metadata, expected, sealed)
}

fn exclusive_after_allow(
    metadata: &File,
    expected: NativeObjectId,
    sealed: LocalAllocation,
) -> io::Result<File> {
    let handle = unsafe {
        ReOpenFile(
            metadata.as_raw_handle(),
            GENERIC_READ | GENERIC_WRITE | WRITE_DAC,
            FILE_SHARE_READ | FILE_SHARE_DELETE,
            FILE_FLAG_OPEN_REPARSE_POINT,
        )
    };
    let reopened = if handle == INVALID_HANDLE_VALUE {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { File::from_raw_handle(handle) })
    };
    // Reapply protection on either branch before propagating the reopen result.
    set_descriptor(metadata, sealed)?;
    let file = reopened?;
    expected.require(&file)?;
    verify_file(&file)?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::super::{
        create_protected_directory_at, create_protected_file_at, release_directory_writes,
        release_file_writes, seal_directory_writes, token,
    };
    use super::*;
    use std::{
        fs,
        io::{Read, Write},
        os::windows::fs::OpenOptionsExt,
    };

    fn parent(path: &std::path::Path) -> File {
        fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
            .unwrap()
    }

    #[test]
    fn closed_native_leaf_reopens_same_object_exclusively_and_reseals_before_zeroing() {
        let _ordinary = token::OrdinaryToken::enter();
        let root = tempfile::tempdir().unwrap();
        let ancestor = parent(root.path());
        let directory = create_protected_directory_at(&ancestor, OsStr::new("sealed")).unwrap();
        let mut leaf = create_protected_file_at(&directory, OsStr::new("payload")).unwrap();
        leaf.write_all(b"owned plaintext").unwrap();
        leaf.sync_all().unwrap();
        let expected = NativeObjectId::capture(&leaf).unwrap();
        seal_directory_writes(&directory).unwrap();
        drop(leaf);
        let metadata = open_verified_file_at(&directory, OsStr::new("payload"), expected).unwrap();
        let writable = reacquire_exclusive_file(&metadata, expected).unwrap();
        assert_eq!(NativeObjectId::capture(&writable).unwrap(), expected);
        let error = fs::OpenOptions::new()
            .write(true)
            .open(root.path().join("sealed/payload"))
            .unwrap_err();
        assert!([
            Some(ERROR_ACCESS_DENIED as i32),
            Some(ERROR_SHARING_VIOLATION as i32)
        ]
        .contains(&error.raw_os_error()));
        writable.set_len(0).unwrap();
        writable.sync_all().unwrap();
        assert_eq!(metadata.metadata().unwrap().len(), 0);
        drop(writable);
        assert_eq!(
            fs::OpenOptions::new()
                .write(true)
                .open(root.path().join("sealed/payload"))
                .unwrap_err()
                .raw_os_error(),
            Some(ERROR_ACCESS_DENIED as i32)
        );
        release_file_writes(&metadata).unwrap();
        release_directory_writes(&directory).unwrap();
        drop(metadata);
        fs::write(
            root.path().join("sealed/payload"),
            b"released intended inode",
        )
        .unwrap();
    }

    #[test]
    fn a_writer_winning_the_permission_window_refuses_reacquisition_and_keeps_bytes() {
        let _ordinary = token::OrdinaryToken::enter();
        let root = tempfile::tempdir().unwrap();
        let ancestor = parent(root.path());
        let directory = create_protected_directory_at(&ancestor, OsStr::new("sealed")).unwrap();
        let mut leaf = create_protected_file_at(&directory, OsStr::new("payload")).unwrap();
        leaf.write_all(b"initial").unwrap();
        let expected = NativeObjectId::capture(&leaf).unwrap();
        seal_directory_writes(&directory).unwrap();
        drop(leaf);
        let mut metadata =
            open_verified_file_at(&directory, OsStr::new("payload"), expected).unwrap();
        // The same production post-allow helper forces the mechanical race
        // without sleeps or an alternate permission policy.
        let sealed = User::current()
            .unwrap()
            .descriptor_with_access(false, "FR")
            .unwrap();
        set_descriptor(
            &metadata,
            User::current()
                .unwrap()
                .descriptor_with_access(false, "FRFW")
                .unwrap(),
        )
        .unwrap();
        let mut contender = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .open(root.path().join("sealed/payload"))
            .unwrap();
        contender.write_all(b"winner!").unwrap();
        contender.sync_all().unwrap();
        let error = exclusive_after_allow(&metadata, expected, sealed).unwrap_err();
        assert_eq!(error.raw_os_error(), Some(ERROR_SHARING_VIOLATION as i32));
        drop(contender);
        assert_eq!(
            fs::OpenOptions::new()
                .write(true)
                .open(root.path().join("sealed/payload"))
                .unwrap_err()
                .raw_os_error(),
            Some(ERROR_ACCESS_DENIED as i32)
        );
        let mut bytes = Vec::new();
        metadata.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"winner!");
        release_file_writes(&metadata).unwrap();
        release_directory_writes(&directory).unwrap();
    }

    #[test]
    fn a_writer_closing_inside_the_permission_window_can_change_bytes_before_exclusive_reacquisition(
    ) {
        let _ordinary = token::OrdinaryToken::enter();
        let root = tempfile::tempdir().unwrap();
        let ancestor = parent(root.path());
        let directory = create_protected_directory_at(&ancestor, OsStr::new("sealed")).unwrap();
        let mut leaf = create_protected_file_at(&directory, OsStr::new("payload")).unwrap();
        leaf.write_all(b"original verified bytes").unwrap();
        leaf.sync_all().unwrap();
        let expected = NativeObjectId::capture(&leaf).unwrap();
        seal_directory_writes(&directory).unwrap();
        drop(leaf);
        let metadata = open_verified_file_at(&directory, OsStr::new("payload"), expected).unwrap();
        let user = User::current().unwrap();
        let sealed = user.descriptor_with_access(false, "FR").unwrap();
        let writable = user.descriptor_with_access(false, "FRFW").unwrap();
        set_descriptor(&metadata, writable).unwrap();
        // A deterministic write AND close fits entirely inside the same native
        // permission window exercised by production reacquisition. No writable
        // contender survives to be detected by the later sharing exclusion.
        fs::write(
            root.path().join("sealed/payload"),
            b"changed and closed inside window",
        )
        .unwrap();
        let mut exclusive = exclusive_after_allow(&metadata, expected, sealed)
            .expect("closed competing writer does not prevent exclusive reacquisition");
        assert_eq!(NativeObjectId::capture(&exclusive).unwrap(), expected);
        let mut bytes = Vec::new();
        exclusive.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"changed and closed inside window");
        assert_ne!(bytes, b"original verified bytes");
        drop(exclusive);
        let error = fs::OpenOptions::new()
            .write(true)
            .open(root.path().join("sealed/payload"))
            .expect_err(
                "FR protection is restored despite successful reacquisition of changed bytes",
            );
        assert_eq!(error.raw_os_error(), Some(ERROR_ACCESS_DENIED as i32));
        release_file_writes(&metadata).unwrap();
        release_directory_writes(&directory).unwrap();
    }

    #[test]
    fn retained_readonly_cleanup_uses_same_inode_after_its_old_name_is_replaced() {
        let _ordinary = token::OrdinaryToken::enter();
        let root = tempfile::tempdir().unwrap();
        let ancestor = parent(root.path());
        let directory = create_protected_directory_at(&ancestor, OsStr::new("sealed")).unwrap();
        let mut leaf = create_protected_file_at(&directory, OsStr::new("payload")).unwrap();
        leaf.write_all(b"owned source").unwrap();
        leaf.sync_all().unwrap();
        let expected = NativeObjectId::capture(&leaf).unwrap();
        let reader = super::super::open_file_at(&directory, OsStr::new("payload")).unwrap();
        super::super::rename_handle_at(&leaf, &ancestor, OsStr::new("moved")).unwrap();
        let mut replacement = create_protected_file_at(&directory, OsStr::new("payload")).unwrap();
        replacement.write_all(b"unrelated replacement").unwrap();
        replacement.sync_all().unwrap();
        seal_directory_writes(&directory).unwrap();
        drop(leaf);
        let writable = reacquire_exclusive_file(&reader, expected).unwrap();
        writable.set_len(0).unwrap();
        writable.sync_all().unwrap();
        assert_eq!(reader.metadata().unwrap().len(), 0);
        assert_eq!(fs::read(root.path().join("moved")).unwrap(), b"");
        assert_eq!(
            fs::read(root.path().join("sealed/payload")).unwrap(),
            b"unrelated replacement"
        );
        assert!(open_verified_file_at(&directory, OsStr::new("payload"), expected).is_err());
        release_file_writes(&writable).unwrap();
        release_file_writes(&replacement).unwrap();
        release_directory_writes(&directory).unwrap();
    }

    #[test]
    fn substituted_leaf_and_directory_ids_refuse_before_acl_or_byte_changes() {
        let _ordinary = token::OrdinaryToken::enter();
        let root = tempfile::tempdir().unwrap();
        let ancestor = parent(root.path());
        let original = create_protected_file_at(&ancestor, OsStr::new("original")).unwrap();
        let original_id = NativeObjectId::capture(&original).unwrap();
        let mut unrelated = create_protected_file_at(&ancestor, OsStr::new("unrelated")).unwrap();
        unrelated.write_all(b"keep unrelated").unwrap();
        let unrelated_id = NativeObjectId::capture(&unrelated).unwrap();
        drop(unrelated);
        assert!(open_verified_file_at(&ancestor, OsStr::new("unrelated"), original_id).is_err());
        assert_eq!(
            fs::OpenOptions::new()
                .write(true)
                .open(root.path().join("unrelated"))
                .unwrap_err()
                .raw_os_error(),
            Some(ERROR_ACCESS_DENIED as i32)
        );
        assert_eq!(
            fs::read(root.path().join("unrelated")).unwrap(),
            b"keep unrelated"
        );
        let left = create_protected_directory_at(&ancestor, OsStr::new("left")).unwrap();
        let left_id = NativeObjectId::capture(&left).unwrap();
        let right = create_protected_directory_at(&ancestor, OsStr::new("right")).unwrap();
        let before = super::super::DirectoryAcl::capture(&right).unwrap();
        assert!(open_verified_directory_at(&ancestor, OsStr::new("right"), left_id).is_err());
        assert_eq!(super::super::DirectoryAcl::capture(&right).unwrap(), before);
        // Supplying a mismatched expected ID to cleanup also refuses before the
        // temporary permission window, even with a genuine metadata handle.
        let metadata =
            open_verified_file_at(&ancestor, OsStr::new("unrelated"), unrelated_id).unwrap();
        assert!(reacquire_exclusive_file(&metadata, original_id).is_err());
        assert_eq!(
            fs::OpenOptions::new()
                .write(true)
                .open(root.path().join("unrelated"))
                .unwrap_err()
                .raw_os_error(),
            Some(ERROR_ACCESS_DENIED as i32)
        );
        release_file_writes(&original).unwrap();
        release_file_writes(&metadata).unwrap();
        release_directory_writes(&left).unwrap();
        release_directory_writes(&right).unwrap();
    }
}
