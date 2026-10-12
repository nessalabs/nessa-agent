//! The inbox on disk: where a fetched file's part is made, how its
//! destination is bounded to the workspace, and that placing it checks the
//! part as it is on disk and never replaces anything.
use super::*;
use std::os::unix::fs::{symlink, PermissionsExt};

struct Fixture {
    _root: tempfile::TempDir,
    workspace: PathBuf,
    outside: PathBuf,
    inbox: FileInbox,
}

fn fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("work");
    fs::create_dir_all(workspace.join("assets")).unwrap();
    let outside = root.path().join("outside");
    fs::create_dir(&outside).unwrap();
    let inbox = FileInbox::open(root.path().join("inbox"), &workspace).unwrap();
    Fixture {
        workspace: fs::canonicalize(&workspace).unwrap(),
        outside: fs::canonicalize(&outside).unwrap(),
        _root: root,
        inbox,
    }
}

fn file_of(bytes: &[u8]) -> FetchedFile {
    FetchedFile {
        digest: hex(&Sha256::digest(bytes)),
        size: bytes.len() as u64,
    }
}

/// Make fetch `fetch`'s part under `lease` and write `bytes` to it, as the
/// gateway would over sftp.
fn delivered(inbox: &FileInbox, lease: &str, fetch: u32, bytes: &[u8]) -> PathBuf {
    let part = PathBuf::from(inbox.open_part(lease, fetch).unwrap());
    fs::write(&part, bytes).unwrap();
    part
}

#[test]
fn a_part_is_made_empty_and_private_under_the_leases_digest_and_swept_on_open() {
    let fixture = fixture();
    let part = PathBuf::from(fixture.inbox.open_part("lease/../1", 7).unwrap());
    assert_eq!(fs::metadata(&part).unwrap().len(), 0);
    assert_eq!(
        fs::metadata(&part).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(part.file_name().unwrap(), "7.part");
    // Named by the lease's digest, never by the lease's own text.
    let directory = part.parent().unwrap();
    assert_eq!(directory.parent().unwrap(), fixture.inbox.root);
    assert_eq!(directory.file_name().unwrap().len(), 64);
    // A part already made is never made again over what it holds.
    fs::write(&part, b"half").unwrap();
    assert_eq!(
        fixture.inbox.open_part("lease/../1", 7),
        Err(FetchRefusal::PlacingFailed)
    );
    fixture.inbox.close("lease/../1");
    assert!(!directory.exists());
    // What an earlier serving process left is let go when serving starts.
    fixture.inbox.open_part("l", 1).unwrap();
    let reopened = FileInbox::open(fixture.inbox.root.clone(), &fixture.workspace).unwrap();
    assert_eq!(fs::read_dir(&reopened.root).unwrap().count(), 0);
}

#[test]
fn a_verified_part_is_linked_at_its_destination_and_let_go() {
    let fixture = fixture();
    let bytes = b"a picture of a cat";
    let file = file_of(bytes);
    assert_eq!(
        fixture.inbox.destination("assets/cat.png", &file),
        Ok(Destination::Free {
            path: fixture
                .workspace
                .join("assets/cat.png")
                .to_str()
                .unwrap()
                .into()
        })
    );
    let part = delivered(&fixture.inbox, "l", 1, bytes);
    let placed = fixture.workspace.join("assets/cat.png");
    assert_eq!(
        fixture.inbox.place("l", 1, "assets/cat.png", &file),
        FetchOutcome::Placed {
            path: placed.to_str().unwrap().into()
        }
    );
    assert_eq!(fs::read(&placed).unwrap(), bytes);
    assert!(!part.exists(), "the part is let go once placed");
    // Absolute inside the workspace is the same place: already there.
    assert_eq!(
        fixture.inbox.destination(placed.to_str().unwrap(), &file),
        Ok(Destination::AlreadyPlaced {
            path: placed.to_str().unwrap().into()
        })
    );
    delivered(&fixture.inbox, "l", 2, bytes);
    assert_eq!(
        fixture.inbox.place("l", 2, "assets/cat.png", &file),
        FetchOutcome::AlreadyPlaced {
            path: placed.to_str().unwrap().into()
        }
    );
}

#[test]
fn a_part_that_is_not_the_file_asked_is_refused_mismatch_and_nothing_is_placed() {
    let fixture = fixture();
    let file = file_of(b"the file asked for");
    for (fetch, wrong) in [
        (1, &b"the file asked for, and more"[..]),
        (2, b"the file asked fox"),
        (3, b""),
    ] {
        let part = delivered(&fixture.inbox, "l", fetch, wrong);
        assert_eq!(
            fixture.inbox.place("l", fetch, "a.txt", &file),
            FetchOutcome::Refused {
                reason: FetchRefusal::Mismatch
            },
            "{wrong:?}"
        );
        assert!(!part.exists());
    }
    assert!(!fixture.workspace.join("a.txt").exists());
}

#[test]
fn a_link_never_replaces_what_is_at_the_destination() {
    let fixture = fixture();
    let bytes = b"new bytes";
    let file = file_of(bytes);
    let there = fixture.workspace.join("notes.txt");
    fs::write(&there, b"the agent's own notes").unwrap();
    assert_eq!(
        fixture.inbox.destination("notes.txt", &file),
        Err(FetchRefusal::Occupied)
    );
    // Something put there after the check is not replaced either.
    let late = fixture.workspace.join("late.txt");
    assert!(matches!(
        fixture.inbox.destination("late.txt", &file),
        Ok(Destination::Free { .. })
    ));
    delivered(&fixture.inbox, "l", 1, bytes);
    fs::write(&late, b"written meanwhile").unwrap();
    assert_eq!(
        fixture.inbox.place("l", 1, "late.txt", &file),
        FetchOutcome::Refused {
            reason: FetchRefusal::Occupied
        }
    );
    assert_eq!(fs::read(&late).unwrap(), b"written meanwhile");
    // A link at the destination is something there, even one to nowhere,
    // and even one to exactly this file.
    let copy = fixture.workspace.join("copy.txt");
    fs::write(&copy, bytes).unwrap();
    symlink(&copy, fixture.workspace.join("linked.txt")).unwrap();
    symlink("/nowhere", fixture.workspace.join("dangling.txt")).unwrap();
    for name in ["linked.txt", "dangling.txt", "assets"] {
        assert_eq!(
            fixture.inbox.destination(name, &file),
            Err(FetchRefusal::Occupied),
            "{name}"
        );
    }
    assert_eq!(fs::read(&copy).unwrap(), bytes);
}

#[test]
fn a_destination_outside_the_workspace_is_refused_however_it_is_reached() {
    let fixture = fixture();
    let file = file_of(b"x");
    symlink(&fixture.outside, fixture.workspace.join("escape")).unwrap();
    let outside = fixture.outside.join("a.txt");
    for (destination, reason) in [
        ("../outside/a.txt", FetchRefusal::OutsideWorkspace),
        (outside.to_str().unwrap(), FetchRefusal::OutsideWorkspace),
        ("escape/a.txt", FetchRefusal::OutsideWorkspace),
        ("assets/..", FetchRefusal::OutsideWorkspace),
        (".", FetchRefusal::OutsideWorkspace),
        ("/", FetchRefusal::OutsideWorkspace),
        ("missing/a.txt", FetchRefusal::NoDirectory),
        ("", FetchRefusal::Invalid),
    ] {
        assert_eq!(
            fixture.inbox.destination(destination, &file),
            Err(reason),
            "{destination:?}"
        );
    }
    // Placing resolves again: a directory swapped for a link to outside
    // after the check is refused, and nothing lands outside.
    fs::create_dir(fixture.workspace.join("later")).unwrap();
    assert!(matches!(
        fixture.inbox.destination("later/a.txt", &file),
        Ok(Destination::Free { .. })
    ));
    delivered(&fixture.inbox, "l", 1, b"x");
    fs::remove_dir(fixture.workspace.join("later")).unwrap();
    symlink(&fixture.outside, fixture.workspace.join("later")).unwrap();
    assert_eq!(
        fixture.inbox.place("l", 1, "later/a.txt", &file),
        FetchOutcome::Refused {
            reason: FetchRefusal::OutsideWorkspace
        }
    );
    assert!(!outside.exists());
    // A link inside the workspace to a directory inside it is followed.
    symlink(
        fixture.workspace.join("assets"),
        fixture.workspace.join("art"),
    )
    .unwrap();
    assert_eq!(
        fixture.inbox.destination("art/a.txt", &file),
        Ok(Destination::Free {
            path: fixture
                .workspace
                .join("assets/a.txt")
                .to_str()
                .unwrap()
                .into()
        })
    );
}

/// What is linked is held to the very file that was hashed: anything else
/// found at the name after the link is taken back, refused `mismatch`.
#[test]
fn a_link_that_is_not_the_hashed_file_is_taken_back_as_a_mismatch() {
    let fixture = fixture();
    let hashed = fixture.workspace.join("hashed");
    fs::write(&hashed, b"checked").unwrap();
    let hashed = File::open(&hashed).unwrap();
    let resolved = fixture.inbox.resolve("swapped.txt").unwrap();
    fs::write(fixture.workspace.join("swapped.txt"), b"checked").unwrap();
    assert_eq!(same_file(&resolved, &hashed), Err(FetchRefusal::Mismatch));
    assert!(!fixture.workspace.join("swapped.txt").exists());
    // The same file, by device and inode, stands.
    fs::hard_link(
        fixture.workspace.join("hashed"),
        fixture.workspace.join("same.txt"),
    )
    .unwrap();
    let resolved = fixture.inbox.resolve("same.txt").unwrap();
    assert_eq!(same_file(&resolved, &hashed), Ok(()));
    assert!(fixture.workspace.join("same.txt").exists());
}

/// A placed file gets the mode this user's other new files get, never the
/// part's private 0600.
#[test]
fn a_placed_file_has_the_mode_new_files_get() {
    let fixture = fixture();
    let bytes = b"readable by the tools that read it";
    let file = file_of(bytes);
    let part = delivered(&fixture.inbox, "l", 1, bytes);
    assert_eq!(
        fs::metadata(&part).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(matches!(
        fixture.inbox.place("l", 1, "readable.txt", &file),
        FetchOutcome::Placed { .. }
    ));
    let mode = fs::metadata(fixture.workspace.join("readable.txt"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, placed_mode());
    assert_eq!(placed_mode() & 0o600, 0o600);
}
