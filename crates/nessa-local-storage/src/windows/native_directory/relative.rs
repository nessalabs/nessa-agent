//! Retained-relative filesystem mechanics for authenticated archive readers.
//! These operations validate object kinds without imposing private-file DACLs
//! on already authenticated public archive bytes. Publication uses the protected
//! constructors in the parent module instead.

use super::{relative, FILE_OPEN, FILE_OPEN_REPARSE_POINT};
use crate::{unsafe_file, OpenMode};
use std::{
    ffi::{OsStr, OsString},
    fs::File,
    io,
    mem::{align_of, offset_of, size_of, zeroed},
    os::windows::{
        ffi::OsStringExt,
        io::{AsRawHandle, FromRawHandle},
    },
};
use windows_sys::Wdk::Storage::FileSystem::{FILE_CREATE, FILE_OPEN_IF};
use windows_sys::Win32::{Foundation::*, Storage::FileSystem::*};

/// Type of a no-follow child entry observed through a retained parent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelativeEntryKind {
    File,
    Directory,
    Reparse,
    Other,
}

/// Open one existing directory without following a final reparse point.
/// Its caller owns the trust boundary for existing archive directory contents.
pub fn open_relative_directory(parent: &File, name: &OsStr) -> io::Result<File> {
    let file = relative(
        parent,
        name,
        FILE_LIST_DIRECTORY | FILE_TRAVERSE | FILE_READ_ATTRIBUTES | READ_CONTROL | SYNCHRONIZE,
        FILE_OPEN,
        FILE_OPEN_REPARSE_POINT,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        None,
    )?;
    if super::super::information(&file)?.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
        return Err(unsafe_file());
    }
    Ok(file)
}

/// Open a regular archive or staging file relative to retained directory authority.
/// Creation installs ordinary private permissions; this is not protected publish.
pub fn open_relative_file(parent: &File, name: &OsStr, mode: OpenMode) -> io::Result<File> {
    if matches!(mode, OpenMode::CreateNew) {
        return super::create_protected_file_at(parent, name);
    }
    let readonly = matches!(mode, OpenMode::Read | OpenMode::ReadNonblocking);
    let disposition = match mode {
        OpenMode::Read | OpenMode::ReadNonblocking | OpenMode::ReadWrite => FILE_OPEN,
        OpenMode::OpenOrCreate => FILE_OPEN_IF,
        OpenMode::CreateNew => FILE_CREATE,
    };
    let descriptor = if matches!(mode, OpenMode::OpenOrCreate | OpenMode::CreateNew) {
        Some(super::super::User::current()?.descriptor(false)?)
    } else {
        None
    };
    let file = relative(
        parent,
        name,
        GENERIC_READ | SYNCHRONIZE | if readonly { 0 } else { GENERIC_WRITE },
        disposition,
        FILE_OPEN_REPARSE_POINT,
        FILE_SHARE_READ
            | FILE_SHARE_WRITE
            | if matches!(mode, OpenMode::OpenOrCreate) {
                // Restore lock files keep their name while their handle is held.
                0
            } else {
                FILE_SHARE_DELETE
            },
        descriptor.as_ref(),
    )?;
    if super::super::information(&file)?.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
        return Err(unsafe_file());
    }
    Ok(file)
}

/// Rename an ordinary existing entry using native relative source authority.
/// Protected publication roots must instead use their initial retained DELETE handle.
pub fn rename_relative_entry_at(
    from_parent: &File,
    from: &OsStr,
    to_parent: &File,
    to: &OsStr,
) -> io::Result<()> {
    let source = relative(
        from_parent,
        from,
        DELETE | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
        FILE_OPEN,
        FILE_OPEN_REPARSE_POINT,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        None,
    )?;
    super::super::information(&source)?;
    super::rename_handle_at(&source, to_parent, to)
}

/// Observe an entry without following its reparse point; absent entries are None.
pub fn relative_entry_kind(parent: &File, name: &OsStr) -> io::Result<Option<RelativeEntryKind>> {
    let file = match relative(
        parent,
        name,
        FILE_READ_ATTRIBUTES | SYNCHRONIZE,
        FILE_OPEN,
        FILE_OPEN_REPARSE_POINT,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        None,
    ) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { zeroed() };
    unsafe {
        super::super::check(GetFileInformationByHandle(file.as_raw_handle(), &mut info))?;
    }
    Ok(Some(
        if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            RelativeEntryKind::Reparse
        } else if info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
            RelativeEntryKind::Directory
        } else if info.dwFileAttributes & FILE_ATTRIBUTE_DEVICE != 0 {
            RelativeEntryKind::Other
        } else {
            RelativeEntryKind::File
        },
    ))
}

/// Enumerate the retained directory with a fresh cursor, without resolving a path.
/// The snapshot is non-atomic; publication validates it after namespace sealing.
pub fn relative_names(directory: &File) -> io::Result<Vec<OsString>> {
    let cursor = open_directory_cursor(directory)?;
    names_from_cursor(&cursor)
}

fn open_directory_cursor(directory: &File) -> io::Result<File> {
    let handle = unsafe {
        ReOpenFile(
            directory.as_raw_handle(),
            // Directory enumeration uses synchronous queries on this fresh
            // cursor. Keep the synchronization access requested by the retained
            // directory open; listing access alone cannot authorize its wait.
            FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_handle(handle) })
}

fn names_from_cursor(cursor: &File) -> io::Result<Vec<OsString>> {
    if super::super::information(cursor)?.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
        return Err(unsafe_file());
    }
    let mut names = Vec::new();
    let mut buffer = vec![0u64; 1024];
    let buffer_bytes = buffer.len() * size_of::<u64>();
    let mut first = true;
    loop {
        let result = unsafe {
            GetFileInformationByHandleEx(
                cursor.as_raw_handle(),
                if first {
                    FileIdBothDirectoryRestartInfo
                } else {
                    FileIdBothDirectoryInfo
                },
                buffer.as_mut_ptr().cast(),
                buffer_bytes as u32,
            )
        };
        if result == 0 {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
                Ok(names)
            } else {
                Err(error)
            };
        }
        first = false;
        let mut offset = 0usize;
        loop {
            let header = offset_of!(FILE_ID_BOTH_DIR_INFO, FileName);
            if !offset.is_multiple_of(align_of::<FILE_ID_BOTH_DIR_INFO>())
                || offset + header > buffer_bytes
            {
                return Err(unsafe_file());
            }
            let info = unsafe {
                &*buffer
                    .as_ptr()
                    .cast::<u8>()
                    .add(offset)
                    .cast::<FILE_ID_BOTH_DIR_INFO>()
            };
            let length = info.FileNameLength as usize;
            if !length.is_multiple_of(2) || length > buffer_bytes - offset - header {
                return Err(unsafe_file());
            }
            let units = unsafe { std::slice::from_raw_parts(info.FileName.as_ptr(), length / 2) };
            let name = OsString::from_wide(units);
            if name != OsStr::new(".") && name != OsStr::new("..") {
                names.push(name);
            }
            if info.NextEntryOffset == 0 {
                break;
            }
            let next = info.NextEntryOffset as usize;
            if next < header || next > buffer_bytes - offset {
                return Err(unsafe_file());
            }
            offset += next;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{create_protected_directory_at, remove_handle};
    use super::*;
    use std::{
        fs,
        io::{Read, Write},
        os::windows::fs::OpenOptionsExt,
        path::Path,
    };

    fn locator(path: &Path) -> File {
        fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
            .unwrap()
    }

    fn observed_names(directory: &File) -> Vec<OsString> {
        let cursor = open_directory_cursor(directory)
            .expect("ReOpenFile creates a fresh synchronous directory cursor");
        super::super::super::information(&cursor)
            .expect("GetFileInformationByHandle reads the reopened cursor's metadata");
        names_from_cursor(&cursor)
            .expect("GetFileInformationByHandleEx enumerates the reopened directory cursor")
    }

    #[test]
    fn relative_archive_reads_do_not_require_private_creation_acls_on_existing_public_files() {
        let _ordinary = super::super::token::OrdinaryToken::enter();
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("archive")).unwrap();
        fs::write(
            root.path().join("archive/body"),
            b"authenticated public bytes",
        )
        .unwrap();
        let root = locator(root.path());
        let directory = open_relative_directory(&root, OsStr::new("archive")).unwrap();
        let mut file = open_relative_file(&directory, OsStr::new("body"), OpenMode::Read).unwrap();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"authenticated public bytes");
        assert_eq!(
            relative_entry_kind(&directory, OsStr::new("body")).unwrap(),
            Some(RelativeEntryKind::File)
        );
        assert_eq!(
            relative_entry_kind(&directory, OsStr::new("missing")).unwrap(),
            None
        );
        assert!(open_relative_directory(&directory, OsStr::new("body")).is_err());
        assert!(open_relative_file(&root, OsStr::new("archive"), OpenMode::Read).is_err());
    }

    #[test]
    fn a_readonly_locator_handle_can_create_under_its_writable_current_acl() {
        let _ordinary = super::super::token::OrdinaryToken::enter();
        let root = tempfile::tempdir().unwrap();
        let readonly = locator(root.path());
        let directory = create_protected_directory_at(&readonly, OsStr::new("owned"))
            .expect("production locator handles carry read/search, not delegated ADD authority");
        remove_handle(&directory).unwrap();
    }

    #[test]
    fn enumeration_and_reads_keep_the_retained_object_after_its_ancestor_name_changes() {
        let _ordinary = super::super::token::OrdinaryToken::enter();
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("original")).unwrap();
        let parent = locator(root.path());
        let directory = open_relative_directory(&parent, OsStr::new("original")).unwrap();
        let mut expected = Vec::new();
        for number in 0..80 {
            let name = OsString::from(format!("entry-{number:03}-λ"));
            let mut file = open_relative_file(&directory, &name, OpenMode::CreateNew).unwrap();
            file.write_all(b"original").unwrap();
            expected.push(name);
        }
        expected.sort();
        let mut before_rename = observed_names(&directory);
        before_rename.sort();
        assert_eq!(before_rename, expected);
        fs::rename(root.path().join("original"), root.path().join("moved")).unwrap();
        fs::create_dir(root.path().join("original")).unwrap();
        fs::write(root.path().join("original/unrelated"), b"keep").unwrap();
        let left = directory.try_clone().unwrap();
        let right = directory.try_clone().unwrap();
        let left_expected = expected.clone();
        let worker = std::thread::spawn(move || {
            let _ordinary = super::super::token::OrdinaryToken::enter();
            for _ in 0..8 {
                let mut names = observed_names(&left);
                names.sort();
                assert_eq!(names, left_expected);
            }
        });
        for _ in 0..8 {
            let mut names = observed_names(&right);
            names.sort();
            assert_eq!(names, expected);
        }
        worker.join().unwrap();
        let mut bytes = Vec::new();
        open_relative_file(&directory, &expected[0], OpenMode::Read)
            .unwrap()
            .read_to_end(&mut bytes)
            .unwrap();
        assert_eq!(bytes, b"original");
        assert_eq!(
            fs::read(root.path().join("original/unrelated")).unwrap(),
            b"keep"
        );
    }

    #[test]
    fn ordinary_restore_lock_names_stay_pinned_while_their_open_or_create_handle_lives() {
        let _ordinary = super::super::token::OrdinaryToken::enter();
        let root = tempfile::tempdir().unwrap();
        let directory = locator(root.path());
        let lock = open_relative_file(
            &directory,
            OsStr::new("restore.lock"),
            OpenMode::OpenOrCreate,
        )
        .unwrap();
        assert!(fs::rename(
            root.path().join("restore.lock"),
            root.path().join("substitute")
        )
        .is_err());
        drop(lock);
        fs::rename(
            root.path().join("restore.lock"),
            root.path().join("substitute"),
        )
        .unwrap();
    }
}
