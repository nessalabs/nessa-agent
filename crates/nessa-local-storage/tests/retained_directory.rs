#[cfg(unix)]
use nessa_local_storage::create_private_directory_path;
use nessa_local_storage::{
    create_directory, create_private_directory_tree_beneath, is_private_temporary_name, OpenMode,
    PrivateDirectory, PrivateFileType, PrivatePublicationStage,
};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
use std::{
    ffi::{OsStr, OsString},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

fn fixture() -> (tempfile::TempDir, PathBuf, PrivateDirectory) {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("root");
    create_directory(&root).unwrap();
    create_private_directory_tree_beneath(&root, Path::new("records")).unwrap();
    let directory = PrivateDirectory::open_beneath(&root, Path::new("records")).unwrap();
    (temporary, root, directory)
}

fn write_named(directory: &PrivateDirectory, name: &str, bytes: &[u8]) {
    let mut file = directory
        .open_file(OsStr::new(name), OpenMode::CreateNew)
        .unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}

fn names(directory: &PrivateDirectory) -> Vec<(OsString, PrivateFileType)> {
    let mut entries = directory
        .entries()
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (entry.name().to_owned(), entry.file_type())
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    entries
}

#[cfg(unix)]
#[test]
fn absolute_private_path_allows_safe_locator_ancestry_and_retains_every_binding() {
    let temporary = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let locator = temporary.path().join("locator");
    std::fs::create_dir(&locator).unwrap();
    std::fs::set_permissions(&locator, std::fs::Permissions::from_mode(0o755)).unwrap();
    let private_root = locator.join("Nessa");
    let journal = private_root.join("stage/journal");
    create_private_directory_path(&private_root).unwrap();
    create_private_directory_path(&journal).unwrap();
    let retained = PrivateDirectory::open_path(&private_root, &journal).unwrap();
    retained.verify_binding().unwrap();
    assert_eq!(
        std::fs::metadata(&journal).unwrap().permissions().mode() & 0o777,
        0o700
    );
    std::fs::set_permissions(&private_root, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(retained.verify_binding().is_err());
}

#[test]
fn private_temporary_classifier_matches_real_reservations_and_only_exact_syntax() {
    let (_temporary, _root, directory) = fixture();
    let reservation = directory.reserve_temp().unwrap();

    assert!(is_private_temporary_name(reservation.name()));
    for near_miss in [
        ".nessa-0123456789abcdef0123456789abcdef",
        ".nessa-0123456789abcdef0123456789abcde.tmp",
        ".nessa-0123456789abcdef0123456789abcdef0.tmp",
        ".nessa-0123456789abcdef0123456789abcdeg.tmp",
        ".nessa-0123456789ABCDEF0123456789ABCDEF.tmp",
        "nessa-0123456789abcdef0123456789abcdef.tmp",
    ] {
        assert!(!is_private_temporary_name(OsStr::new(near_miss)));
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let native = OsString::from_vec(vec![b'.', b'n', 0x80]);
        assert!(!is_private_temporary_name(&native));
    }
}

#[test]
fn one_authority_scans_reads_appends_and_syncs() {
    let (_temporary, _root, directory) = fixture();
    write_named(&directory, "record", b"first");

    assert_eq!(
        names(&directory),
        vec![(OsString::from("record"), PrivateFileType::RegularFile)]
    );
    let mut file = directory
        .open_file(OsStr::new("record"), OpenMode::ReadWrite)
        .unwrap();
    file.seek(SeekFrom::End(0)).unwrap();
    file.write_all(b" second").unwrap();
    file.sync_all().unwrap();
    directory.sync().unwrap();

    let mut bytes = Vec::new();
    directory
        .open_file(OsStr::new("record"), OpenMode::Read)
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(bytes, b"first second");
}

#[test]
fn publication_returns_the_open_destination_for_identity_acknowledgement() {
    let (_temporary, _root, directory) = fixture();
    let mut reservation = directory.reserve_temp().unwrap();
    reservation.as_file_mut().write_all(b"published").unwrap();

    let published = reservation.publish_new(OsStr::new("sequence-1")).unwrap();

    assert_eq!(published.name(), OsStr::new("sequence-1"));
    assert!(directory
        .named_file_is(published.name(), published.as_file())
        .unwrap());
    let mut bytes = Vec::new();
    directory
        .open_file(OsStr::new("sequence-1"), OpenMode::Read)
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(bytes, b"published");
}

#[test]
fn dropping_a_published_handle_keeps_the_destination() {
    let (_temporary, _root, directory) = fixture();
    let mut reservation = directory.reserve_temp().unwrap();
    reservation.as_file_mut().write_all(b"published").unwrap();
    let published = reservation.publish_new(OsStr::new("sequence-1")).unwrap();

    drop(published);

    let mut bytes = Vec::new();
    directory
        .open_file(OsStr::new("sequence-1"), OpenMode::Read)
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(bytes, b"published");
}

#[test]
fn named_identity_rejects_a_multiply_linked_witness() {
    let (_temporary, root, directory) = fixture();
    write_named(&directory, "record", b"one file");
    let witness = directory
        .open_file(OsStr::new("record"), OpenMode::ReadWrite)
        .unwrap();
    std::fs::hard_link(root.join("records/record"), root.join("records/alias")).unwrap();

    assert_eq!(
        directory
            .named_file_is(OsStr::new("record"), &witness)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::PermissionDenied
    );
}

#[test]
fn recovery_removes_only_the_name_still_bound_to_the_open_file() {
    let (_temporary, root, directory) = fixture();
    write_named(&directory, "abandoned", b"old temporary");
    let old = directory
        .open_file(OsStr::new("abandoned"), OpenMode::Read)
        .unwrap();
    std::fs::remove_file(root.join("records/abandoned")).unwrap();
    write_named(&directory, "abandoned", b"replacement");

    assert!(directory
        .remove_file(OsStr::new("abandoned"), &old)
        .is_err());
    assert_eq!(
        std::fs::read(root.join("records/abandoned")).unwrap(),
        b"replacement"
    );

    let replacement = directory
        .open_file(OsStr::new("abandoned"), OpenMode::Read)
        .unwrap();
    directory
        .remove_file(OsStr::new("abandoned"), &replacement)
        .unwrap();
    directory.sync().unwrap();
    assert!(!root.join("records/abandoned").exists());
}

#[test]
fn parallel_enumerations_have_independent_cursors() {
    let (_temporary, _root, directory) = fixture();
    write_named(&directory, "a", b"a");
    write_named(&directory, "b", b"b");
    write_named(&directory, "c", b"c");

    let mut first = directory.entries().unwrap();
    let mut second = directory.entries().unwrap();
    let first_head = first.next().unwrap().unwrap().name().to_owned();
    let second_head = second.next().unwrap().unwrap().name().to_owned();
    let mut first_names = vec![first_head];
    let mut second_names = vec![second_head];
    first_names.extend(first.map(|entry| entry.unwrap().name().to_owned()));
    second_names.extend(second.map(|entry| entry.unwrap().name().to_owned()));
    first_names.sort();
    second_names.sort();

    assert_eq!(first_names, ["a", "b", "c"].map(OsString::from));
    assert_eq!(second_names, first_names);
}

#[test]
fn two_publishers_of_one_name_have_one_winner_without_overwrite() {
    let (_temporary, _root, directory) = fixture();
    let mut first = directory.reserve_temp().unwrap();
    first.as_file_mut().write_all(b"first").unwrap();
    let mut second = directory.reserve_temp().unwrap();
    second.as_file_mut().write_all(b"second").unwrap();

    let (first, second) = std::thread::scope(|scope| {
        let first = scope.spawn(move || first.publish_new(OsStr::new("sequence-1")));
        let second = scope.spawn(move || second.publish_new(OsStr::new("sequence-1")));
        (first.join().unwrap(), second.join().unwrap())
    });

    assert_ne!(first.is_ok(), second.is_ok());
    let failure = first.err().or_else(|| second.err()).unwrap();
    assert_eq!(failure.stage(), PrivatePublicationStage::Rename);
    assert_eq!(
        failure.source_error().kind(),
        std::io::ErrorKind::AlreadyExists
    );
    assert!(failure.published().is_none());
    assert!(failure.cleanup_error().is_none());
    assert_eq!(
        names(&directory),
        vec![(OsString::from("sequence-1"), PrivateFileType::RegularFile)]
    );

    let mut bytes = Vec::new();
    directory
        .open_file(OsStr::new("sequence-1"), OpenMode::Read)
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    assert!(bytes == b"first" || bytes == b"second");
}

#[test]
fn invalid_destination_is_a_preeffect_failure_and_cleans_the_reservation() {
    let (_temporary, root, directory) = fixture();
    let reservation = directory.reserve_temp().unwrap();
    let reserved_name = reservation.name().to_owned();

    let failure = reservation
        .publish_new(OsStr::new("../outside"))
        .unwrap_err();

    assert_eq!(
        failure.stage(),
        PrivatePublicationStage::ValidateDestination
    );
    assert!(failure.published().is_none());
    assert!(failure.cleanup_error().is_none());
    assert!(!root.join("records").join(reserved_name).exists());
    assert!(!root.join("outside").exists());
}

#[cfg(unix)]
#[test]
fn detached_authority_refuses_new_file_acquisition_and_acknowledgement() {
    let (_temporary, root, directory) = fixture();
    write_named(&directory, "lock", b"old lock");
    let held = root.join("held-records");
    std::fs::rename(root.join("records"), &held).unwrap();
    create_private_directory_tree_beneath(&root, Path::new("records")).unwrap();

    assert!(directory.verify_binding().is_err());
    assert!(directory
        .open_file(OsStr::new("new-lock"), OpenMode::CreateNew)
        .is_err());
    assert!(!root.join("records/new-lock").exists());
    assert!(!held.join("new-lock").exists());
}

#[cfg(unix)]
#[test]
fn a_scan_then_replacement_cannot_publish_into_either_directory() {
    let (_temporary, root, directory) = fixture();
    write_named(&directory, "existing", b"old");
    let scanned = names(&directory);
    let mut reservation = directory.reserve_temp().unwrap();
    reservation.as_file_mut().write_all(b"candidate").unwrap();
    let reservation_name = reservation.name().to_owned();
    let held = root.join("held-records");
    std::fs::rename(root.join("records"), &held).unwrap();
    create_private_directory_tree_beneath(&root, Path::new("records")).unwrap();

    let failure = reservation
        .publish_new(OsStr::new("sequence-1"))
        .unwrap_err();

    assert_eq!(
        scanned,
        vec![(OsString::from("existing"), PrivateFileType::RegularFile)]
    );
    assert_eq!(
        failure.stage(),
        PrivatePublicationStage::VerifyOriginBinding
    );
    assert!(failure.published().is_none());
    assert!(failure.cleanup_error().is_none());
    assert!(!held.join("sequence-1").exists());
    assert!(!held.join(reservation_name).exists());
    assert!(!root.join("records/sequence-1").exists());
}

#[cfg(unix)]
#[test]
fn drop_removes_only_the_origin_reservation_after_directory_replacement() {
    let (_temporary, root, directory) = fixture();
    let mut reservation = directory.reserve_temp().unwrap();
    reservation.as_file_mut().write_all(b"origin").unwrap();
    let name = reservation.name().to_owned();
    let held = root.join("held-records");
    std::fs::rename(root.join("records"), &held).unwrap();
    create_private_directory_tree_beneath(&root, Path::new("records")).unwrap();
    std::fs::write(root.join("records").join(&name), b"outside marker").unwrap();

    drop(reservation);

    assert!(!held.join(&name).exists());
    assert_eq!(
        std::fs::read(root.join("records").join(&name)).unwrap(),
        b"outside marker"
    );
}

#[cfg(unix)]
#[test]
fn changed_reservation_name_refuses_cleanup_and_reports_it_separately() {
    let (_temporary, root, directory) = fixture();
    let mut reservation = directory.reserve_temp().unwrap();
    reservation.as_file_mut().write_all(b"retained").unwrap();
    let old = root.join("records").join(reservation.name());
    let changed = root.join("records/changed-name");
    std::fs::rename(&old, &changed).unwrap();

    let failure = reservation.publish_new(OsStr::new("record")).unwrap_err();

    let (stage, source, published, cleanup) = failure.into_parts();
    assert_eq!(stage, PrivatePublicationStage::ValidateReservation);
    assert_eq!(source.kind(), std::io::ErrorKind::PermissionDenied);
    assert!(published.is_none());
    assert_eq!(
        cleanup.expect("cleanup failure remains independent").kind(),
        std::io::ErrorKind::PermissionDenied
    );
    assert_eq!(std::fs::read(changed).unwrap(), b"retained");
}

#[cfg(unix)]
#[test]
fn replacing_a_named_lock_is_detected_by_open_file_identity() {
    let (_temporary, root, directory) = fixture();
    write_named(&directory, "lock", b"first");
    let original = directory
        .open_file(OsStr::new("lock"), OpenMode::ReadWrite)
        .unwrap();
    std::fs::remove_file(root.join("records/lock")).unwrap();
    assert!(!directory
        .named_file_is(OsStr::new("lock"), &original)
        .unwrap());
    write_named(&directory, "lock", b"replacement");

    assert!(!directory
        .named_file_is(OsStr::new("lock"), &original)
        .unwrap());
}

#[cfg(unix)]
#[test]
fn acquisition_rejects_an_intermediate_symbolic_link() {
    use std::os::unix::fs::symlink;

    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("root");
    let outside = temporary.path().join("outside");
    create_directory(&root).unwrap();
    create_directory(&outside).unwrap();
    symlink(&outside, root.join("linked")).unwrap();

    assert!(PrivateDirectory::open_beneath(&root, Path::new("linked/records")).is_err());
    assert!(!outside.join("records").exists());
}

#[cfg(unix)]
#[test]
fn file_open_refuses_a_symbolic_link_leaf() {
    use std::os::unix::fs::symlink;

    let (_temporary, root, directory) = fixture();
    write_named(&directory, "target", b"outside the requested name");
    symlink("target", root.join("records/link")).unwrap();

    assert!(directory
        .open_file(OsStr::new("link"), OpenMode::Read)
        .is_err());
}

#[cfg(unix)]
#[test]
fn enumeration_preserves_native_names_and_no_follow_types() {
    use std::os::unix::fs::symlink;

    let (_temporary, root, directory) = fixture();
    #[cfg(target_os = "linux")]
    let native = {
        use std::os::unix::ffi::OsStringExt;
        OsString::from_vec(vec![b'n', 0x80])
    };
    // macOS accepts Unicode native names but rejects arbitrary byte strings.
    // Spell the decomposed normalization explicitly so enumeration is compared
    // with the exact native representation supplied to the filesystem.
    #[cfg(target_vendor = "apple")]
    let native = OsString::from("native-e\u{301}");
    directory.open_file(&native, OpenMode::CreateNew).unwrap();
    create_directory(&root.join("records/child")).unwrap();
    symlink("child", root.join("records/link")).unwrap();

    let entries = names(&directory);
    assert!(entries.contains(&(native, PrivateFileType::RegularFile)));
    assert!(entries.contains(&(OsString::from("child"), PrivateFileType::Directory)));
    assert!(entries.contains(&(OsString::from("link"), PrivateFileType::Symlink)));
}

#[cfg(windows)]
#[test]
fn retained_windows_directory_handles_prevent_replacement_for_their_lifetime() {
    let (_temporary, root, directory) = fixture();
    let held = root.join("held-records");

    assert!(std::fs::rename(root.join("records"), &held).is_err());
    drop(directory);
    std::fs::rename(root.join("records"), &held).unwrap();
}

#[cfg(windows)]
#[test]
fn retained_windows_root_and_intermediate_handles_pin_then_release_each_name() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("root");
    create_directory(&root).unwrap();
    create_private_directory_tree_beneath(&root, Path::new("parent/records")).unwrap();
    let directory = PrivateDirectory::open_beneath(&root, Path::new("parent/records")).unwrap();
    let held_parent = root.join("held-parent");
    let held_root = temporary.path().join("held-root");

    assert!(std::fs::rename(root.join("parent"), &held_parent).is_err());
    assert!(std::fs::rename(&root, &held_root).is_err());

    drop(directory);
    std::fs::rename(root.join("parent"), &held_parent).unwrap();
    std::fs::rename(&root, &held_root).unwrap();
}

#[cfg(windows)]
#[test]
fn retained_windows_file_handle_prevents_named_lock_replacement() {
    let (_temporary, root, directory) = fixture();
    write_named(&directory, "lock", b"first");
    let lock = directory
        .open_file(OsStr::new("lock"), OpenMode::ReadWrite)
        .unwrap();

    assert!(std::fs::remove_file(root.join("records/lock")).is_err());
    assert!(directory.named_file_is(OsStr::new("lock"), &lock).unwrap());
    drop(lock);
    std::fs::remove_file(root.join("records/lock")).unwrap();
}

#[cfg(windows)]
#[test]
fn windows_reservation_identity_publish_reopen_and_cleanup_share_one_file() {
    let (_temporary, root, directory) = fixture();
    let mut reservation = directory.reserve_temp().unwrap();
    reservation.as_file_mut().write_all(b"published").unwrap();
    let reservation_name = reservation.name().to_owned();
    assert!(directory
        .named_file_is(&reservation_name, reservation.as_file())
        .unwrap());

    let published = reservation.publish_new(OsStr::new("record")).unwrap();
    assert!(!root.join("records").join(reservation_name).exists());
    assert!(directory
        .named_file_is(published.name(), published.as_file())
        .unwrap());
    let mut reopened = directory
        .open_file(OsStr::new("record"), OpenMode::Read)
        .unwrap();
    let mut bytes = Vec::new();
    reopened.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"published");

    drop(reopened);
    drop(published);
    std::fs::remove_file(root.join("records/record")).unwrap();
    assert!(names(&directory).is_empty());
}

#[cfg(windows)]
#[test]
fn windows_publication_accepts_a_unicode_destination_beyond_max_path() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("root");
    create_directory(&root).unwrap();
    let relative = PathBuf::from("a".repeat(120)).join("b".repeat(120));
    create_private_directory_tree_beneath(&root, &relative).unwrap();
    let directory = PrivateDirectory::open_beneath(&root, &relative).unwrap();
    let destination = OsString::from("résumé-記録.json");
    assert!(
        root.join(&relative)
            .join(&destination)
            .as_os_str()
            .encode_wide()
            .count()
            > 260
    );
    let mut reservation = directory.reserve_temp().unwrap();
    reservation
        .as_file_mut()
        .write_all(b"long unicode destination")
        .unwrap();
    let reservation_name = reservation.name().to_owned();

    let published = reservation.publish_new(&destination).unwrap();

    assert_eq!(published.name(), destination.as_os_str());
    let published_names = names(&directory);
    assert!(published_names.contains(&(destination, PrivateFileType::RegularFile)));
    assert!(!published_names
        .iter()
        .any(|(name, _)| name == &reservation_name));
    assert!(directory
        .named_file_is(published.name(), published.as_file())
        .unwrap());
    let mut bytes = Vec::new();
    directory
        .open_file(published.name(), OpenMode::Read)
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(bytes, b"long unicode destination");
}

#[cfg(windows)]
#[test]
fn acquisition_rejects_an_intermediate_reparse_point() {
    use std::os::windows::fs::symlink_dir;

    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("root");
    let outside = temporary.path().join("outside");
    create_directory(&root).unwrap();
    create_directory(&outside).unwrap();
    symlink_dir(&outside, root.join("linked")).unwrap();

    assert!(PrivateDirectory::open_beneath(&root, Path::new("linked/records")).is_err());
    assert!(!outside.join("records").exists());
}

#[cfg(windows)]
#[test]
fn file_open_refuses_a_reparse_point_leaf() {
    use std::os::windows::fs::symlink_file;

    let (_temporary, root, directory) = fixture();
    write_named(&directory, "target", b"outside the requested name");
    symlink_file("target", root.join("records/link")).unwrap();

    assert!(directory
        .open_file(OsStr::new("link"), OpenMode::Read)
        .is_err());
}
