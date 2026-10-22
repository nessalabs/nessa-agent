#![cfg(windows)]

#[path = "support/windows_permissions.rs"]
mod permissions;
#[path = "support/windows_token.rs"]
mod token;

use nessa_local_storage::{
    create_protected_directory_at, create_protected_file_at, open_directory_at, open_file_at,
    release_directory_writes, release_file_writes, remove_handle, rename_handle_at,
    seal_directory_writes,
};
use std::{
    ffi::OsStr,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::windows::fs::OpenOptionsExt,
    path::Path,
};
use windows_sys::Win32::{Foundation::GENERIC_READ, Storage::FileSystem::*};

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

// Releasing a private ACL first verifies its descriptor and native attributes.
// This is trusted owner authority, separate from competing write/DELETE probes.
fn acl_authority(path: &Path) -> std::io::Result<File> {
    writer(path, GENERIC_READ | WRITE_DAC)
}

// Directory transitions use original maximum-access handles, matching verified
// production reopening. Regular-file metadata cleanup retains its narrower mask.
fn directory_acl_authority(path: &Path) -> std::io::Result<File> {
    writer(
        path,
        windows_sys::Win32::System::SystemServices::MAXIMUM_ALLOWED | SYNCHRONIZE,
    )
}

fn assert_leaf_mutation_refused(path: &Path) {
    if let Ok(file) = writer(path, READ_CONTROL | FILE_READ_ATTRIBUTES) {
        permissions::diagnose("leaf-before-mutation", &file);
    }
    if let Some(parent) = path.parent() {
        if let Ok(directory) = writer(parent, READ_CONTROL | FILE_READ_ATTRIBUTES) {
            permissions::diagnose("parent-before-leaf-mutation", &directory);
        }
    }
    match OpenOptions::new().write(true).open(path) {
        Err(error) => refused_namespace_writer(&error),
        Ok(mut file) => {
            let error = file
                .write_all(b"untrusted")
                .expect_err("a granted handle must still refuse actual leaf writes");
            refused_namespace_writer(&error);
        }
    }
    match writer(path, DELETE) {
        Err(error) => refused_namespace_writer(&error),
        Ok(file) => {
            let error = remove_handle(&file)
                .expect_err("a granted handle must still refuse actual leaf deletion");
            refused_namespace_writer(&error);
        }
    }
}

#[test]
fn read_only_parent_acl_refuses_child_create_despite_initial_add_handle() {
    let _ordinary = token::OrdinaryToken::enter();
    let root = tempfile::tempdir().unwrap();
    let authority = parent(root.path());
    let directory = create_protected_directory_at(&authority, OsStr::new("sealed")).unwrap();
    seal_directory_writes(&directory).unwrap();
    // Exact original N03 failure at native CI 39701307415 / 5f8d: retaining
    // ADD_FILE in the original handle does not delegate it through a read-only ACL.
    let error = create_protected_file_at(&directory, OsStr::new("payload"))
        .expect_err("original N03 child creation remains refused after read-only sealing");
    assert_eq!(
        error.raw_os_error(),
        Some(windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED as i32)
    );
    remove_handle(&directory).unwrap();
}

#[test]
fn sealed_directory_refuses_actual_additions_after_initial_handle_closes() {
    let _ordinary = token::OrdinaryToken::enter();
    let root = tempfile::tempdir().unwrap();
    let authority = parent(root.path());
    let directory = create_protected_directory_at(&authority, OsStr::new("sealed")).unwrap();
    seal_directory_writes(&directory).unwrap();
    permissions::diagnose("sealed-directory-before-close", &directory);
    drop(directory);
    let error = fs::write(root.path().join("sealed/injected"), b"untrusted")
        .expect_err("sealed directory must refuse actual file creation");
    refused_namespace_writer(&error);
    let error = fs::create_dir(root.path().join("sealed/injected-directory"))
        .expect_err("sealed directory must refuse actual directory creation");
    refused_namespace_writer(&error);
    let cleanup = directory_acl_authority(&root.path().join("sealed")).unwrap();
    release_directory_writes(&cleanup).unwrap();
}

#[test]
fn a_waiting_writer_cannot_relocate_the_created_root() {
    let _ordinary = token::OrdinaryToken::enter();
    let root = tempfile::tempdir().unwrap();
    let authority = parent(root.path());
    let path = root.path().join("sealed");
    let ready = std::sync::Arc::new(std::sync::Barrier::new(2));
    let waiting = ready.clone();
    let contender = std::thread::spawn(move || {
        let _ordinary = token::OrdinaryToken::enter();
        waiting.wait();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            match fs::rename(&path, path.with_file_name("stolen")) {
                Ok(()) => {
                    panic!("N02: contender relocated root while its initial handle was retained")
                }
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
    let _ordinary = token::OrdinaryToken::enter();
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
fn initial_directory_authority_creates_children_before_sealing() {
    let _ordinary = token::OrdinaryToken::enter();
    let root = tempfile::tempdir().unwrap();
    let authority = parent(root.path());
    let directory = create_protected_directory_at(&authority, OsStr::new("sealed"))
        .expect("N01: atomic protected creation");
    let mut file = create_protected_file_at(&directory, OsStr::new("payload"))
        .expect("N03 revised: append-only parent permits initial child creation before sealing");
    file.write_all(b"exact bytes").unwrap();
    file.sync_all().unwrap();
    let mut read = open_file_at(&directory, OsStr::new("payload")).unwrap();
    let mut bytes = Vec::new();
    read.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"exact bytes");
    assert_leaf_mutation_refused(&root.path().join("sealed/payload"));
    remove_handle(&file).expect("N05: retained child cleanup");
    drop(read);
    drop(file);
    remove_handle(&directory).expect("N05: retained empty-directory cleanup");
}

#[test]
fn protected_root_blocks_competing_delete_but_renames_through_its_own_handle() {
    let _ordinary = token::OrdinaryToken::enter();
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
    seal_directory_writes(&directory).unwrap();
    let error = rename_handle_at(&directory, &authority, OsStr::new("live"))
        .expect_err("N04: the kernel refuses directory rename with open descendants");
    assert_eq!(
        error.raw_os_error(),
        Some(windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED as i32)
    );
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"owned");
    drop(reader);
    drop(file);
    assert_leaf_mutation_refused(&root.path().join("sealed/payload"));
    rename_handle_at(&directory, &authority, OsStr::new("live"))
        .expect("N04: same retained root renames after protected descendants close");
    assert!(!root.path().join("sealed").exists());
    assert!(root.path().join("live/payload").is_file());
    let error = writer(&root.path().join("live"), DELETE)
        .expect_err("root remains pinned under its live name until initial authority closes");
    refused_namespace_writer(&error);
    let acl = acl_authority(&root.path().join("live/payload")).unwrap();
    release_file_writes(&acl).unwrap();
    release_directory_writes(&directory).unwrap();
}

#[test]
fn protected_child_refuses_delete_and_write_until_post_publication_release() {
    let _ordinary = token::OrdinaryToken::enter();
    let root = tempfile::tempdir().unwrap();
    let authority = parent(root.path());
    let directory = create_protected_directory_at(&authority, OsStr::new("sealed")).unwrap();
    let mut file = create_protected_file_at(&directory, OsStr::new("payload"))
        .expect("N03: initial directory authority creates protected child");
    file.write_all(b"owned").unwrap();
    file.sync_all().unwrap();
    assert_leaf_mutation_refused(&root.path().join("sealed/payload"));
    drop(file);
    assert_leaf_mutation_refused(&root.path().join("sealed/payload"));
    // The owner retains ACL control, explicitly outside the competing namespace
    // writer threat boundary. Reopen ACL control only after protected publication.
    seal_directory_writes(&directory).unwrap();
    rename_handle_at(&directory, &authority, OsStr::new("live")).unwrap();
    let acl = acl_authority(&root.path().join("live/payload")).unwrap();
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
    let _ordinary = token::OrdinaryToken::enter();
    let root = tempfile::tempdir().unwrap();
    let authority = parent(root.path());
    let directory = create_protected_directory_at(&authority, OsStr::new("sealed")).unwrap();
    let mut file = create_protected_file_at(&directory, OsStr::new("original"))
        .expect("N03: restrictive directory must admit original writer child creation");
    file.write_all(b"owned").unwrap();
    let occupied = create_protected_file_at(&directory, OsStr::new("occupied")).unwrap();
    let error = rename_handle_at(&file, &directory, OsStr::new("occupied"))
        .expect_err("N04: occupied destination must not be replaced");
    eprintln!(
        "native-rename-collision raw={:?} kind={:?}",
        error.raw_os_error(),
        error.kind()
    );
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
    seal_directory_writes(&directory).unwrap();
    rename_handle_at(&directory, &authority, OsStr::new("live")).unwrap();
    release_directory_writes(&directory).unwrap();
    assert!(open_directory_at(&authority, OsStr::new("sealed")).is_err());
    assert!(open_directory_at(&authority, OsStr::new("live")).is_ok());
    drop(directory);
    fs::write(root.path().join("live/new"), b"new work after release").unwrap();
}

#[test]
fn protected_directory_collision_preserves_existing_data() {
    let _ordinary = token::OrdinaryToken::enter();
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

#[test]
fn original_privilege_profile_explains_directory_creation_counterexample() {
    let backup = token::privilege_enabled("SeBackupPrivilege").unwrap();
    let restore = token::privilege_enabled("SeRestorePrivilege").unwrap();
    let root = tempfile::tempdir().unwrap();
    let authority = parent(root.path());
    let directory = create_protected_directory_at(&authority, OsStr::new("sealed")).unwrap();
    seal_directory_writes(&directory).unwrap();
    permissions::diagnose("original-profile-causal-control", &directory);
    drop(directory);
    let original = fs::create_dir(root.path().join("sealed/original-profile"));
    eprintln!("native-privileged-counterexample backup={backup} restore={restore}");
    if restore {
        original.expect("enabled restore privilege reproduces ce5 privileged directory-add bypass");
    } else {
        refused_namespace_writer(
            &original.expect_err("ordinary original profile refuses directory addition"),
        );
    }
    {
        let _ordinary = token::OrdinaryToken::enter();
        let error = fs::create_dir(root.path().join("sealed/restricted-profile"))
            .expect_err("privilege-only restriction refuses actual directory addition under either original profile");
        refused_namespace_writer(&error);
    }
    // Guard restoration also applies when the original token was itself an
    // impersonation token, rather than assuming every test begins unimpersonated.
    {
        let _outer = token::OrdinaryToken::enter();
        {
            let _inner = token::OrdinaryToken::enter();
        }
        assert!(!token::privilege_enabled("SeRestorePrivilege").unwrap());
    }
    let cleanup = directory_acl_authority(&root.path().join("sealed")).unwrap();
    release_directory_writes(&cleanup).unwrap();
}

#[test]
fn nested_directory_handles_must_close_before_retained_root_rename() {
    let _ordinary = token::OrdinaryToken::enter();
    let root = tempfile::tempdir().unwrap();
    let authority = parent(root.path());
    let directory = create_protected_directory_at(&authority, OsStr::new("sealed")).unwrap();
    let child =
        nessa_local_storage::create_protected_subdirectory_at(&directory, OsStr::new("child"))
            .unwrap();
    seal_directory_writes(&child).unwrap();
    seal_directory_writes(&directory).unwrap();
    let root_duplicate = directory.try_clone().unwrap();
    let error = rename_handle_at(&directory, &authority, OsStr::new("live"))
        .expect_err("N04: even an open directory descendant prevents root rename");
    assert_eq!(
        error.raw_os_error(),
        Some(windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED as i32)
    );
    drop(child);
    let error = fs::create_dir(root.path().join("sealed/child/injected"))
        .expect_err("closed descendant retains sealed namespace protection");
    refused_namespace_writer(&error);
    let error = fs::rename(root.path().join("sealed/child"), root.path().join("stolen"))
        .expect_err("closed descendant retains deletion protection");
    refused_namespace_writer(&error);
    rename_handle_at(&directory, &authority, OsStr::new("live")).expect(
        "same retained root renames after every descendant closes, with its duplicate retained",
    );
    assert!(root.path().join("live/child").is_dir());
    drop(root_duplicate);
    let child = directory_acl_authority(&root.path().join("live/child")).unwrap();
    release_directory_writes(&child).unwrap();
    release_directory_writes(&directory).unwrap();
}
