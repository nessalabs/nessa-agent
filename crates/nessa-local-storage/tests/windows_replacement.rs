#![cfg(windows)]

use nessa_local_storage::{create_directory, open, OpenMode, PrivateTempFile};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Result, Write},
    os::windows::fs::OpenOptionsExt,
    path::Path,
};
use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION};
use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};

fn publish(directory: &Path, bytes: &[u8]) -> Result<()> {
    create_directory(&directory.join("records"))?;
    let mut temporary = PrivateTempFile::new_beneath(directory, Path::new("records"))?;
    temporary.as_file_mut().write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist_beneath(Path::new("records/record.json"))
}

#[test]
fn replacement_keeps_a_delete_sharing_reader_on_the_previous_record() {
    let temporary = tempfile::tempdir().unwrap();
    let directory = temporary.path().join("private");
    create_directory(&directory).unwrap();
    publish(&directory, b"previous").unwrap();
    let mut reader = open(&directory.join("records/record.json"), OpenMode::Read).unwrap();

    publish(&directory, b"replacement").expect("replace while a private reader permits deletion");

    assert_eq!(
        fs::read(directory.join("records/record.json")).unwrap(),
        b"replacement"
    );
    let mut old_bytes = Vec::new();
    reader.read_to_end(&mut old_bytes).unwrap();
    assert_eq!(old_bytes, b"previous");
}

#[test]
fn replacement_refuses_a_reader_without_delete_sharing_then_succeeds_after_release() {
    let temporary = tempfile::tempdir().unwrap();
    let directory = temporary.path().join("private");
    create_directory(&directory).unwrap();
    publish(&directory, b"previous").unwrap();
    let reader = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .open(directory.join("records/record.json"))
        .unwrap();

    let error = publish(&directory, b"refused").unwrap_err();

    assert!(
        matches!(
            error.raw_os_error().map(|code| code as u32),
            Some(ERROR_ACCESS_DENIED | ERROR_SHARING_VIOLATION)
        ),
        "{error:?}"
    );
    assert_eq!(
        fs::read(directory.join("records/record.json")).unwrap(),
        b"previous"
    );
    assert_eq!(fs::read_dir(directory.join("records")).unwrap().count(), 1);
    drop(reader);
    publish(&directory, b"replacement").unwrap();
    assert_eq!(
        fs::read(directory.join("records/record.json")).unwrap(),
        b"replacement"
    );
}

#[test]
fn replacement_preserves_a_read_only_destination_and_succeeds_after_permission_restore() {
    let temporary = tempfile::tempdir().unwrap();
    let directory = temporary.path().join("private");
    create_directory(&directory).unwrap();
    publish(&directory, b"previous").unwrap();
    let destination = directory.join("records/record.json");
    let original_permissions = fs::metadata(&destination).unwrap().permissions();
    let mut permissions = original_permissions.clone();
    permissions.set_readonly(true);
    fs::set_permissions(&destination, permissions).unwrap();

    let error = publish(&directory, b"refused").unwrap_err();

    fs::set_permissions(&destination, original_permissions).unwrap();
    assert_eq!(
        error.raw_os_error().map(|code| code as u32),
        Some(ERROR_ACCESS_DENIED),
        "{error:?}"
    );
    assert_eq!(fs::read(&destination).unwrap(), b"previous");
    assert_eq!(fs::read_dir(directory.join("records")).unwrap().count(), 1);
    publish(&directory, b"replacement").unwrap();
    assert_eq!(fs::read(destination).unwrap(), b"replacement");
}
