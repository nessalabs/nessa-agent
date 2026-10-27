//! The output and part on a real file system: the part named for its file,
//! created only by a first append, held by one fetch at a time, resumed,
//! linked at the output's name only when kept and never over anything there,
//! removed when discarded, and an output looked at without following links.
use super::{part_path, still_named, DiskFiles, MAX_NAME_BYTES, PART_SUFFIX_BYTES};
use crate::read_only_sync::application::held_file::{
    fetch_held_file, FetchCause, FetchFiles, FileStep, HeldFileSource, KeepRefusal, OutputFound,
    PartCleanup, Pause, RangeAnswer,
};
use crate::read_only_sync::application::GatewayError;
use crate::read_only_sync::domain::{HeldFile, HeldRange};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::domain::common::value_objects::{MediaType, Sha256Digest};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use std::time::Duration;

fn held(bytes: &[u8]) -> HeldFile {
    HeldFile::new(
        ConversationId::new("00000000-0000-4000-8000-000000000001").unwrap(),
        Sha256Digest::from_bytes(Sha256::digest(bytes).into()),
        MediaType::parse("application/octet-stream").unwrap(),
        bytes.len() as u64,
    )
    .unwrap()
}
fn part_bytes(files: &mut DiskFiles) -> (u64, Vec<u8>) {
    let mut seen = vec![];
    let count = files
        .read_part(&mut |bytes| seen.extend_from_slice(bytes))
        .unwrap();
    (count, seen)
}

fn held_as(conversation: &str, digest: [u8; 32], media_type: &str, size: u64) -> HeldFile {
    HeldFile::new(
        ConversationId::new(conversation).unwrap(),
        Sha256Digest::from_bytes(digest),
        MediaType::parse(media_type).unwrap(),
        size,
    )
    .unwrap()
}

#[test]
fn the_part_is_named_beside_the_output_for_the_whole_file_it_holds() {
    const A: &str = "00000000-0000-4000-8000-000000000001";
    const B: &str = "00000000-0000-4000-8000-000000000002";
    let file = held_as(A, [1; 32], "image/png", 10);
    let out = Path::new("dir/a.png");
    let part = part_path(out, &file);
    assert_eq!(part.parent(), Some(Path::new("dir")));
    let name = part.file_name().unwrap().to_str().unwrap();
    let key = name
        .strip_prefix("a.png.")
        .and_then(|rest| rest.strip_suffix(".part"))
        .unwrap();
    assert_eq!(key.len(), 32);
    assert!(key.bytes().all(|byte| byte.is_ascii_hexdigit()));
    // Every field that names the file names another part.
    for other in [
        held_as(B, [1; 32], "image/png", 10),
        held_as(A, [2; 32], "image/png", 10),
        held_as(A, [1; 32], "image/jpeg", 10),
        held_as(A, [1; 32], "image/png", 11),
    ] {
        assert_ne!(part_path(out, &other), part, "{other:?}");
    }
    assert_eq!(part_path(out, &file.clone()), part);
}

#[test]
fn nothing_is_created_until_the_first_append() {
    let root = tempfile::tempdir().unwrap();
    let out = root.path().join("file.bin");
    let file = held(b"abc");
    let mut files = DiskFiles::new(&out, &file).unwrap();
    assert_eq!(files.read_output(3, &mut |_| {}), Ok(OutputFound::Absent));
    assert_eq!(files.open_part(), Ok(0));
    assert_eq!(files.read_part(&mut |_| {}), Ok(0));
    files.restart().unwrap();
    files.discard().unwrap();
    assert!(!part_path(&out, &file).exists());
    assert!(fs::read_dir(root.path()).unwrap().next().is_none());
}

#[test]
fn a_part_resumes_what_an_earlier_run_kept_and_only_keep_names_the_output() {
    let root = tempfile::tempdir().unwrap();
    let out = root.path().join("file.bin");
    let file = held(b"whatever");
    let mut files = DiskFiles::new(&out, &file).unwrap();
    assert_eq!(files.open_part(), Ok(0));
    files.append(b"hello ").unwrap();
    drop(files);
    assert!(!out.exists());

    let mut files = DiskFiles::new(&out, &file).unwrap();
    assert_eq!(files.open_part(), Ok(6));
    // Larger than one read, so the part comes back in more than one piece.
    let large = vec![7_u8; 100 * 1024];
    files.append(&large).unwrap();
    let (count, seen) = part_bytes(&mut files);
    assert_eq!(count, 6 + large.len() as u64);
    assert_eq!(&seen[..6], b"hello ");
    // A read does not move where the next append goes.
    files.append(b"!").unwrap();
    assert!(!out.exists());
    files.keep().unwrap();
    assert!(!part_path(&out, &file).exists());
    let written = fs::read(&out).unwrap();
    assert_eq!(written.len(), 6 + large.len() + 1);
    assert_eq!(written.last(), Some(&b'!'));
    let mut read = vec![];
    assert_eq!(
        DiskFiles::new(&out, &file)
            .unwrap()
            .read_output(written.len() as u64, &mut |bytes| read
                .extend_from_slice(bytes)),
        Ok(OutputFound::Read(written.len() as u64))
    );
    assert_eq!(read, written);
}

#[test]
fn keeping_never_replaces_an_output_that_is_there() {
    let root = tempfile::tempdir().unwrap();
    let out = root.path().join("file.bin");
    let file = held(b"new");
    let mut files = DiskFiles::new(&out, &file).unwrap();
    files.append(b"new").unwrap();
    fs::write(&out, b"old").unwrap();
    assert_eq!(files.keep(), Err(KeepRefusal::OutputExists));
    assert_eq!(fs::read(&out).unwrap(), b"old");
    // The part stays, held, for a later run.
    assert_eq!(part_bytes(&mut files), (3, b"new".to_vec()));
}

#[test]
fn restart_empties_the_part_and_discard_removes_it() {
    let root = tempfile::tempdir().unwrap();
    let out = root.path().join("file.bin");
    let file = held(b"ab");
    let mut files = DiskFiles::new(&out, &file).unwrap();
    files.append(b"not this file").unwrap();
    files.restart().unwrap();
    assert_eq!(part_bytes(&mut files), (0, vec![]));
    files.append(b"ab").unwrap();
    assert_eq!(part_bytes(&mut files), (2, b"ab".to_vec()));
    files.discard().unwrap();
    assert!(!part_path(&out, &file).exists());
    assert!(!out.exists());
}

#[test]
fn a_part_that_cannot_be_created_is_typed() {
    let root = tempfile::tempdir().unwrap();
    let out = root.path().join("missing").join("file.bin");
    let mut files = DiskFiles::new(&out, &held(b"a")).unwrap();
    assert_eq!(files.open_part(), Ok(0));
    assert_eq!(files.append(b"a"), Err(FileStep::Open));
}

/// Review P2-1: two fetches of one file to one output. The second can
/// neither open nor create the part the first holds, so their bytes never
/// interleave; once the first has kept it, the second finds no part.
#[test]
fn two_fetches_never_hold_one_part() {
    let root = tempfile::tempdir().unwrap();
    let out = root.path().join("file.bin");
    let file = held(b"abcdef");
    let mut first = DiskFiles::new(&out, &file).unwrap();
    let mut second = DiskFiles::new(&out, &file).unwrap();
    // Neither has a part yet; the first append creates and holds it.
    assert_eq!(first.open_part(), Ok(0));
    assert_eq!(second.open_part(), Ok(0));
    first.append(b"abc").unwrap();
    assert_eq!(second.append(b"abc"), Err(FileStep::Locked));
    let mut third = DiskFiles::new(&out, &file).unwrap();
    assert_eq!(third.open_part(), Err(FileStep::Locked));
    first.append(b"def").unwrap();
    assert_eq!(part_bytes(&mut first), (6, b"abcdef".to_vec()));
    first.keep().unwrap();
    assert_eq!(fs::read(&out).unwrap(), b"abcdef");
    let mut later = DiskFiles::new(&out, &file).unwrap();
    assert_eq!(later.open_part(), Ok(0));
}

/// The same race through the whole fetch: the second fetch stops on the
/// lock, typed, and the first keeps exactly the file.
#[test]
fn a_second_fetch_of_one_output_stops_on_the_lock_and_the_first_keeps_the_file() {
    struct Serving(Vec<u8>);
    impl HeldFileSource for Serving {
        fn read(&mut self, _: &HeldFile, range: HeldRange) -> Result<RangeAnswer, GatewayError> {
            let start = range.offset() as usize;
            Ok(RangeAnswer {
                offset: range.offset(),
                bytes: self.0[start..start + range.length() as usize].to_vec(),
            })
        }
    }
    struct NoPause;
    impl Pause for NoPause {
        fn pause(&self, _: Duration, _: &dyn Fn() -> bool) {}
    }
    let root = tempfile::tempdir().unwrap();
    let out = root.path().join("file.bin");
    let bytes: Vec<u8> = (0..70_000).map(|index| (index % 253) as u8).collect();
    let file = held(&bytes);
    // The first fetch's part, held, with one range in it.
    let mut first = DiskFiles::new(&out, &file).unwrap();
    first.append(&bytes[..32_768]).unwrap();
    let mut second = DiskFiles::new(&out, &file).unwrap();
    let failure = fetch_held_file(
        &mut Serving(bytes.clone()),
        &NoPause,
        &mut second,
        &file,
        &|| false,
    )
    .unwrap_err();
    assert_eq!(failure.cause, FetchCause::File(FileStep::Locked));
    assert!(!out.exists());
    drop(first);
    let mut resumed = DiskFiles::new(&out, &file).unwrap();
    let progress = fetch_held_file(
        &mut Serving(bytes.clone()),
        &NoPause,
        &mut resumed,
        &file,
        &|| false,
    )
    .unwrap();
    assert_eq!(progress.resumed_from, 32_768);
    assert_eq!(fs::read(&out).unwrap(), bytes);
    assert!(!part_path(&out, &file).exists());
}

/// What is checked is the part on disk: a part with other bytes in it is
/// discarded, and the output never appears.
#[test]
fn a_part_of_other_bytes_on_disk_is_discarded_and_no_output_appears() {
    /// A complete part still asks its last range, answered here as the
    /// file is.
    struct Right;
    impl HeldFileSource for Right {
        fn read(&mut self, _: &HeldFile, range: HeldRange) -> Result<RangeAnswer, GatewayError> {
            Ok(RangeAnswer {
                offset: range.offset(),
                bytes: b"right".to_vec(),
            })
        }
    }
    struct NoPause;
    impl Pause for NoPause {
        fn pause(&self, _: Duration, _: &dyn Fn() -> bool) {}
    }
    let root = tempfile::tempdir().unwrap();
    let out = root.path().join("file.bin");
    let file = held(b"right");
    fs::write(part_path(&out, &file), b"wrong").unwrap();
    let failure = fetch_held_file(
        &mut Right,
        &NoPause,
        &mut DiskFiles::new(&out, &file).unwrap(),
        &file,
        &|| false,
    )
    .unwrap_err();
    assert_eq!(
        failure.cause,
        FetchCause::Integrity {
            cleanup: PartCleanup::Removed
        }
    );
    assert!(!part_path(&out, &file).exists());
    assert!(!out.exists());
}

#[test]
fn an_output_name_with_no_room_for_a_part_is_refused_up_front() {
    let root = tempfile::tempdir().unwrap();
    let file = held(b"abc");
    let longest = "n".repeat(MAX_NAME_BYTES - PART_SUFFIX_BYTES);
    let out = root.path().join(&longest);
    // The suffix is exactly what the part adds to the name.
    let part = part_path(&out, &file);
    assert_eq!(
        part.file_name().unwrap().len(),
        longest.len() + PART_SUFFIX_BYTES
    );
    let mut files = DiskFiles::new(&out, &file).unwrap();
    files.append(b"abc").unwrap();
    files.keep().unwrap();
    assert_eq!(fs::read(&out).unwrap(), b"abc");
    for refused in [
        root.path().join(format!("{longest}n")),
        root.path().join("dir/"),
        root.path().join(".."),
        Path::new("/").to_owned(),
    ] {
        assert_eq!(
            DiskFiles::new(&refused, &file).err(),
            Some(FileStep::Name),
            "{refused:?}"
        );
    }
}

#[test]
fn an_output_that_is_not_a_regular_file_of_the_size_is_not_opened() {
    let root = tempfile::tempdir().unwrap();
    let out = root.path().join("file.bin");
    let file = held(b"abc");
    let look = |files: &mut DiskFiles| {
        let mut visited = false;
        let found = files.read_output(3, &mut |_| visited = true);
        (found, visited)
    };
    fs::create_dir(&out).unwrap();
    let mut files = DiskFiles::new(&out, &file).unwrap();
    assert_eq!(look(&mut files), (Ok(OutputFound::Other), false));
    fs::remove_dir(&out).unwrap();
    fs::write(&out, b"abcd").unwrap();
    assert_eq!(look(&mut files), (Ok(OutputFound::Other), false));
    fs::write(&out, b"xyz").unwrap();
    // The size is not the digest: it is read, and the caller checks it.
    assert_eq!(look(&mut files), (Ok(OutputFound::Read(3)), true));
}

/// A link at the output's name is never followed: one to the very file is
/// refused, and one that leads nowhere is refused before anything is dialled
/// and is never written through when the part is kept.
#[cfg(unix)]
#[test]
fn a_link_at_the_output_is_refused_and_never_followed() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let out = root.path().join("file.bin");
    let target = root.path().join("target.bin");
    let file = held(b"abc");
    fs::write(&target, b"abc").unwrap();
    symlink(&target, &out).unwrap();
    let mut files = DiskFiles::new(&out, &file).unwrap();
    assert_eq!(
        files.read_output(3, &mut |_| panic!("not opened")),
        Ok(OutputFound::Other)
    );
    fs::remove_file(&out).unwrap();
    fs::remove_file(&target).unwrap();
    symlink(&target, &out).unwrap();
    assert_eq!(
        files.read_output(3, &mut |_| panic!("not opened")),
        Ok(OutputFound::Other)
    );
    files.append(b"abc").unwrap();
    assert_eq!(files.keep(), Err(KeepRefusal::OutputExists));
    assert!(!target.exists());
    assert_eq!(fs::read_link(&out).unwrap(), target);
}

/// There is no check before the publish: the link itself refuses whatever
/// is at the output's name when it is made, a directory included.
#[test]
fn keeping_refuses_whatever_is_at_the_output_when_it_links() {
    let root = tempfile::tempdir().unwrap();
    let out = root.path().join("file.bin");
    let file = held(b"abc");
    let mut files = DiskFiles::new(&out, &file).unwrap();
    files.append(b"abc").unwrap();
    fs::create_dir(&out).unwrap();
    assert_eq!(files.keep(), Err(KeepRefusal::OutputExists));
    assert!(out.is_dir());
    assert_eq!(part_bytes(&mut files), (3, b"abc".to_vec()));
}

#[test]
fn a_part_no_fetch_holds_is_removed_and_a_held_one_stays() {
    let root = tempfile::tempdir().unwrap();
    let out = root.path().join("file.bin");
    let file = held(b"abc");
    let mut holder = DiskFiles::new(&out, &file).unwrap();
    holder.append(b"ab").unwrap();
    DiskFiles::new(&out, &file).unwrap().remove_unheld_part();
    assert!(part_path(&out, &file).exists());
    drop(holder);
    DiskFiles::new(&out, &file).unwrap().remove_unheld_part();
    assert!(!part_path(&out, &file).exists());
    // With no part, there is nothing to do.
    DiskFiles::new(&out, &file).unwrap().remove_unheld_part();
}

/// What `hold` checks after taking the lock: a name moved to another file
/// is seen, so the part is held once more from the name.
#[test]
fn a_name_moved_to_another_file_no_longer_names_the_one_held() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("part");
    fs::write(&path, b"a").unwrap();
    let held = fs::File::open(&path).unwrap();
    assert!(still_named(&path, &held));
    fs::rename(&path, root.path().join("moved")).unwrap();
    assert!(!still_named(&path, &held));
    fs::write(&path, b"b").unwrap();
    assert_eq!(still_named(&path, &held), cfg!(not(unix)));
}

/// Review P2 (PR #765): a part one conversation's fetch assembled is never
/// another conversation's. A complete part left when the output appeared
/// is not resumed by a fetch of the same bytes for another conversation,
/// which reads every range from the gateway itself.
#[test]
fn a_part_assembled_for_one_conversation_is_never_used_for_another() {
    struct Serving {
        bytes: Vec<u8>,
        asked: Vec<u64>,
    }
    impl HeldFileSource for Serving {
        fn read(&mut self, _: &HeldFile, range: HeldRange) -> Result<RangeAnswer, GatewayError> {
            self.asked.push(range.offset());
            let start = range.offset() as usize;
            Ok(RangeAnswer {
                offset: range.offset(),
                bytes: self.bytes[start..start + range.length() as usize].to_vec(),
            })
        }
    }
    struct NoPause;
    impl Pause for NoPause {
        fn pause(&self, _: Duration, _: &dyn Fn() -> bool) {}
    }
    let root = tempfile::tempdir().unwrap();
    let out = root.path().join("file.bin");
    let bytes: Vec<u8> = (0..40_000).map(|index| (index % 251) as u8).collect();
    let digest: [u8; 32] = Sha256::digest(&bytes).into();
    let in_a = held_as(
        "00000000-0000-4000-8000-00000000000a",
        digest,
        "application/octet-stream",
        bytes.len() as u64,
    );
    let in_b = held_as(
        "00000000-0000-4000-8000-00000000000b",
        digest,
        "application/octet-stream",
        bytes.len() as u64,
    );
    // A's fetch assembles the whole part, then finds the output there.
    let mut files = DiskFiles::new(&out, &in_a).unwrap();
    files.append(&bytes).unwrap();
    fs::write(&out, b"someone's").unwrap();
    assert_eq!(files.keep(), Err(KeepRefusal::OutputExists));
    drop(files);
    fs::remove_file(&out).unwrap();
    // B's fetch of the same bytes finds no part of its own.
    let mut source = Serving {
        bytes: bytes.clone(),
        asked: vec![],
    };
    let progress = fetch_held_file(
        &mut source,
        &NoPause,
        &mut DiskFiles::new(&out, &in_b).unwrap(),
        &in_b,
        &|| false,
    )
    .unwrap();
    assert_eq!(progress.resumed_from, 0);
    assert_eq!(source.asked, [0, 32_768]);
    assert_eq!(fs::read(&out).unwrap(), bytes);
    // A's part is still A's, untouched.
    assert_eq!(fs::read(part_path(&out, &in_a)).unwrap(), bytes);
}

#[test]
fn a_part_that_cannot_be_removed_after_the_link_leaves_the_output() {
    let root = tempfile::tempdir().unwrap();
    let out = root.path().join("file.bin");
    let bytes = b"kept";
    let file = held(bytes);
    let mut files = DiskFiles::new(&out, &file).unwrap();
    files.append(bytes).unwrap();
    files.sync_held().unwrap();
    files.link_held().unwrap();
    let part = part_path(&out, &file);
    fs::remove_file(&part).unwrap();
    fs::create_dir(&part).unwrap();
    assert_eq!(
        files.retire_held(),
        Err(KeepRefusal::Published {
            step: FileStep::Remove
        })
    );
    assert_eq!(fs::read(&out).unwrap(), bytes);
    assert!(part.is_dir());
}
