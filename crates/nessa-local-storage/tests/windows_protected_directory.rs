#![cfg(windows)]

use nessa_local_storage::{
    create_protected_directory_at, create_protected_file_at, open_directory_at, open_file_at,
    release_directory_writes, release_file_writes, remove_handle, rename_handle_at,
};
use std::{
    ffi::OsStr,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::windows::fs::OpenOptionsExt,
    path::Path,
};
use windows_sys::Win32::Storage::FileSystem::*;

fn refused_namespace_writer(error: &std::io::Error) {
    use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION};
    assert!(
        [
            Some(ERROR_ACCESS_DENIED as i32),
            Some(ERROR_SHARING_VIOLATION as i32)
        ]
        .contains(&error.raw_os_error()),
        "expected access or sharing refusal, got {error}"
    );
}

fn parent(path: &Path) -> File {
    OpenOptions::new()
        .access_mode(
            FILE_LIST_DIRECTORY
                | FILE_TRAVERSE
                | FILE_ADD_FILE
                | FILE_ADD_SUBDIRECTORY
                | FILE_READ_ATTRIBUTES,
        )
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .expect("open native test parent")
}

fn writer(path: &Path, access: u32) -> std::io::Result<File> {
    OpenOptions::new()
        .access_mode(access)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

#[test]
fn protected_creation_refuses_fresh_namespace_writer_handles() {
    let root = tempfile::tempdir().unwrap();
    let authority = parent(root.path());
    let directory = create_protected_directory_at(&authority, OsStr::new("sealed"))
        .expect("N01: atomic restrictive directory creation must return initial writer authority");
    for access in [FILE_ADD_FILE, FILE_ADD_SUBDIRECTORY, FILE_DELETE_CHILD] {
        let error = writer(&root.path().join("sealed"), access)
            .expect_err("N02: a later handle must not acquire namespace mutation authority");
        refused_namespace_writer(&error);
    }
    // This also proves the restriction is the creation DACL, not only sharing:
    // the initial writer closes before the new namespace access attempts.
    drop(directory);
    for access in [FILE_ADD_FILE, FILE_ADD_SUBDIRECTORY, FILE_DELETE_CHILD] {
        let error = writer(&root.path().join("sealed"), access)
            .expect_err("creation DACL remains restrictive after initial handle closes");
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    }
    // TempDir owns test cleanup; restore owner ACL deliberately for cleanup.
    let cleanup = OpenOptions::new()
        .access_mode(WRITE_DAC)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(root.path().join("sealed"))
        .unwrap();
    release_directory_writes(&cleanup).unwrap();
}

#[test]
fn a_waiting_writer_cannot_preacquire_the_created_directory() {
    let root = tempfile::tempdir().unwrap();
    let authority = parent(root.path());
    let path = root.path().join("sealed");
    let ready = std::sync::Arc::new(std::sync::Barrier::new(2));
    let waiting = ready.clone();
    let contender = std::thread::spawn(move || {
        waiting.wait();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            match writer(&path, FILE_ADD_FILE | FILE_DELETE_CHILD) {
                Ok(_) => panic!("N02: contender acquired mutation rights in a creation/ACL window"),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "directory never appeared"
                    );
                    std::thread::yield_now();
                }
                Err(error) => {
                    refused_namespace_writer(&error);
                    break;
                }
            }
        }
    });
    ready.wait();
    let directory = create_protected_directory_at(&authority, OsStr::new("sealed"))
        .expect("N01: atomic restrictive creation");
    contender.join().unwrap();
    remove_handle(&directory).unwrap();
}

#[test]
fn relative_creation_uses_retained_parent_after_its_name_changes() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("original")).unwrap();
    let authority = parent(&root.path().join("original"));
    fs::rename(root.path().join("original"), root.path().join("moved")).unwrap();
    fs::create_dir(root.path().join("original")).unwrap();
    fs::write(root.path().join("original/sentinel"), b"keep").unwrap();
    let directory = create_protected_directory_at(&authority, OsStr::new("sealed"))
        .expect("N03: retained parent, not its former path, owns creation");
    assert!(root.path().join("moved/sealed").is_dir());
    assert!(!root.path().join("original/sealed").exists());
    assert_eq!(
        fs::read(root.path().join("original/sentinel")).unwrap(),
        b"keep"
    );
    remove_handle(&directory).unwrap();
}

#[test]
fn initial_directory_authority_creates_children_under_restrictive_creation_acl() {
    let root = tempfile::tempdir().unwrap();
    let authority = parent(root.path());
    let directory = create_protected_directory_at(&authority, OsStr::new("sealed"))
        .expect("N01: atomic protected creation");
    let mut file = create_protected_file_at(&directory, OsStr::new("payload"))
        .expect("N03: initial RootDirectory authority must create a child despite restrictive DACL; failure means the native capability is NOT qualified");
    file.write_all(b"exact bytes").unwrap();
    file.sync_all().unwrap();
    let mut read = open_file_at(&directory, OsStr::new("payload")).unwrap();
    let mut bytes = Vec::new();
    read.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"exact bytes");
    assert!(OpenOptions::new()
        .write(true)
        .open(root.path().join("sealed/payload"))
        .is_err());
    remove_handle(&file).expect("N05: retained child cleanup");
    drop(read);
    drop(file);
    remove_handle(&directory).expect("N05: retained empty-directory cleanup");
}

#[test]
fn protected_root_blocks_competing_delete_but_renames_through_its_own_handle() {
    let root = tempfile::tempdir().unwrap();
    let authority = parent(root.path());
    let directory = create_protected_directory_at(&authority, OsStr::new("sealed")).unwrap();
    // The ordinary test parent grants DELETE_CHILD. The retained root's lack
    // of delete sharing must still block a second DELETE opener and path move.
    let error = writer(&root.path().join("sealed"), DELETE)
        .expect_err("N02: parent DELETE_CHILD must not bypass initial directory handle sharing");
    refused_namespace_writer(&error);
    let error = fs::rename(root.path().join("sealed"), root.path().join("stolen"))
        .expect_err("N02: another opener must not relocate the protected root");
    refused_namespace_writer(&error);
    assert!(root.path().join("sealed").is_dir());
    assert!(!root.path().join("stolen").exists());

    let mut file = create_protected_file_at(&directory, OsStr::new("payload"))
        .expect("N03: retained directory authority creates protected payload");
    file.write_all(b"owned").unwrap();
    file.sync_all().unwrap();
    let mut reader = open_file_at(&directory, OsStr::new("payload")).unwrap();
    rename_handle_at(&directory, &authority, OsStr::new("live")).expect(
        "N04: initial DELETE handle must rename its own root while child handles remain open",
    );
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"owned");
    assert!(!root.path().join("sealed").exists());
    assert!(root.path().join("live/payload").is_file());
    let error = writer(&root.path().join("live"), DELETE)
        .expect_err("root remains pinned under its live name until initial authority closes");
    refused_namespace_writer(&error);
    drop(reader);
    remove_handle(&file).unwrap();
    drop(file);
    remove_handle(&directory).unwrap();
}

#[test]
fn protected_child_refuses_delete_and_write_until_post_publication_release() {
    use windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED;
    let root = tempfile::tempdir().unwrap();
    let authority = parent(root.path());
    let directory = create_protected_directory_at(&authority, OsStr::new("sealed")).unwrap();
    let mut file = create_protected_file_at(&directory, OsStr::new("payload"))
        .expect("N03: initial directory authority creates protected child");
    file.write_all(b"owned").unwrap();
    file.sync_all().unwrap();
    for access in [DELETE, FILE_WRITE_DATA, FILE_APPEND_DATA] {
        let error = writer(&root.path().join("sealed/payload"), access)
            .expect_err("N03: fresh leaf DELETE or write authority must be refused");
        refused_namespace_writer(&error);
    }
    drop(file);
    for access in [DELETE, FILE_WRITE_DATA, FILE_APPEND_DATA] {
        let error = writer(&root.path().join("sealed/payload"), access).expect_err(
            "N03: leaf ACL and protected parent must refuse mutation after creator closes",
        );
        assert_eq!(error.raw_os_error(), Some(ERROR_ACCESS_DENIED as i32));
    }
    // The owner retains ACL control, explicitly outside the competing namespace
    // writer threat boundary. Retain it before rename, then release after live.
    let acl = writer(&root.path().join("sealed/payload"), WRITE_DAC).unwrap();
    rename_handle_at(&directory, &authority, OsStr::new("live")).unwrap();
    release_file_writes(&acl).unwrap();
    release_directory_writes(&directory).unwrap();
    drop((acl, directory));
    fs::write(root.path().join("live/payload"), b"new bytes after release").unwrap();
    assert_eq!(
        fs::read(root.path().join("live/payload")).unwrap(),
        b"new bytes after release"
    );
}

#[test]
fn retained_relative_rename_refuses_collisions_and_cleanup_keeps_replacement() {
    let root = tempfile::tempdir().unwrap();
    let authority = parent(root.path());
    let directory = create_protected_directory_at(&authority, OsStr::new("sealed")).unwrap();
    let mut file = create_protected_file_at(&directory, OsStr::new("original"))
        .expect("N03: restrictive directory must admit original writer child creation");
    file.write_all(b"owned").unwrap();
    let occupied = create_protected_file_at(&directory, OsStr::new("occupied")).unwrap();
    let error = rename_handle_at(&file, &directory, OsStr::new("occupied"))
        .expect_err("N04: occupied destination must not be replaced");
    assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
    rename_handle_at(&file, &directory, OsStr::new("moved"))
        .expect("N04: native relative rename must preserve initial authority");
    let mut replacement = create_protected_file_at(&directory, OsStr::new("original")).unwrap();
    replacement.write_all(b"replacement").unwrap();
    remove_handle(&file).unwrap();
    drop(file);
    let mut bytes = Vec::new();
    open_file_at(&directory, OsStr::new("original"))
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(bytes, b"replacement");
    assert!(open_file_at(&directory, OsStr::new("moved")).is_err());
    remove_handle(&replacement).unwrap();
    remove_handle(&occupied).unwrap();
    drop((replacement, occupied));
    rename_handle_at(&directory, &authority, OsStr::new("live")).unwrap();
    release_directory_writes(&directory).unwrap();
    assert!(open_directory_at(&authority, OsStr::new("sealed")).is_err());
    assert!(open_directory_at(&authority, OsStr::new("live")).is_ok());
    drop(directory);
    fs::write(root.path().join("live/new"), b"new work after release").unwrap();
}

#[test]
fn protected_directory_collision_preserves_existing_data() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("occupied")).unwrap();
    fs::write(root.path().join("occupied/sentinel"), b"keep").unwrap();
    let authority = parent(root.path());
    let error = create_protected_directory_at(&authority, OsStr::new("occupied")).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(
        fs::read(root.path().join("occupied/sentinel")).unwrap(),
        b"keep"
    );
}
