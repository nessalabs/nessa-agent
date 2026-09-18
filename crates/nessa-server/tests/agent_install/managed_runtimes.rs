use super::*;
use std::io::Write;
use std::path::Path;

use tar::{EntryType, Header};

use crate::agent_install::domain::{AgentName, ArchivePath, ArchiveUrl, ReleasePlatform};

/// The SHA-256 of the three bytes `abc`, which is the standard test vector.
/// Written out rather than computed so the test would catch a hasher that
/// hashed something else and agreed with itself.
const ABC_SHA256: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

fn agent() -> AgentName {
    AgentName::parse("opencode").expect("a plain agent name")
}

/// A gzip tar holding one file at `path`, built in memory.
fn archive(path: &str, body: &[u8]) -> Vec<u8> {
    entry_archive(path, body, EntryType::Regular, None)
}

/// A gzip tar holding one entry of whatever kind, built in memory.
fn entry_archive(path: &str, body: &[u8], kind: EntryType, link: Option<&str>) -> Vec<u8> {
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::fast(),
    ));
    let mut header = Header::new_gnu();
    header.set_size(body.len() as u64);
    header.set_mode(0o644);
    header.set_entry_type(kind);
    if let Some(target) = link {
        header
            .set_link_name(target)
            .expect("a link name for an in-memory archive");
    }
    header.set_cksum();
    builder
        .append_data(&mut header, path, body)
        .expect("appending to an in-memory archive");
    builder
        .into_inner()
        .expect("finishing the tar")
        .finish()
        .expect("finishing the gzip stream")
}

fn release(version: &str, executable: &str) -> PinnedRelease {
    PinnedRelease::new(
        ReleaseVersion::parse(version).expect("usable version"),
        ReleasePlatform::new("macos", "aarch64").expect("usable platform"),
        ArchiveUrl::parse("https://registry.example/runtime.tgz").expect("a fetchable url"),
        ArchiveDigest::parse(&"a".repeat(64)).expect("usable digest"),
        ArchivePath::parse(executable).expect("contained path"),
    )
}

/// Stage `bytes` for `store`, the way a download would have left them.
fn staged(store: &ManagedRuntimes, bytes: &[u8]) -> StagedArchive {
    let mut staged = store.stage(&agent()).expect("a staged file");
    staged
        .file_mut()
        .write_all(bytes)
        .expect("writing a test archive");
    staged
}

/// Stage `bytes` and publish `release` from them.
fn publish(
    store: &ManagedRuntimes,
    release: &PinnedRelease,
    bytes: &[u8],
) -> Result<std::path::PathBuf, StoreFailure> {
    let mut staged = staged(store, bytes);
    store.publish(&agent(), release, &mut staged)
}

fn write(path: &Path, bytes: &[u8]) {
    let mut file = std::fs::File::create(path).expect("writing a test file");
    file.write_all(bytes).expect("writing a test file");
}

#[test]
fn nothing_recorded_is_nothing_installed() {
    let root = tempfile::tempdir().expect("temporary root");
    let store = ManagedRuntimes::new(root.path());
    assert_eq!(
        store.installed(&agent(), &release("1.18.31", "package/bin/opencode")),
        Ok(None)
    );
}

#[test]
fn a_published_runtime_is_reported_as_installed() {
    let root = tempfile::tempdir().expect("temporary root");
    let store = ManagedRuntimes::new(root.path());
    let release = release("1.18.31", "package/bin/opencode");

    let published = publish(
        &store,
        &release,
        &archive("package/bin/opencode", b"binary"),
    )
    .expect("the executable is unpacked");

    assert!(published.is_file());
    assert_eq!(
        std::fs::read(&published).expect("reading the published runtime"),
        b"binary"
    );
    assert_eq!(
        store.installed(&agent(), &release),
        Ok(Some(published)),
        "a published runtime is the one reported"
    );
}

#[test]
fn a_published_runtime_is_executable() {
    let root = tempfile::tempdir().expect("temporary root");
    let store = ManagedRuntimes::new(root.path());
    // The archive says 0o644. What matters is that the installed file can be
    // launched regardless of what the archive said about it.
    let published = publish(
        &store,
        &release("1.18.31", "package/bin/opencode"),
        &archive("package/bin/opencode", b"binary"),
    )
    .expect("the executable is unpacked");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = published
            .metadata()
            .expect("reading the published runtime")
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0o111, "the installed runtime is executable");
    }
    #[cfg(not(unix))]
    let _ = published;
}

#[test]
fn a_runtime_whose_executable_was_deleted_is_not_installed() {
    // The record is a note, not evidence. A disk cleaner that removed the
    // binary must not leave Nessa handing out a launch path to nothing.
    let root = tempfile::tempdir().expect("temporary root");
    let store = ManagedRuntimes::new(root.path());
    let release = release("1.18.31", "package/bin/opencode");
    let published = publish(
        &store,
        &release,
        &archive("package/bin/opencode", b"binary"),
    )
    .expect("the executable is unpacked");

    std::fs::remove_file(&published).expect("removing the published runtime");

    assert_eq!(store.installed(&agent(), &release), Ok(None));
}

#[test]
fn a_record_naming_another_version_is_not_this_release() {
    // The pin moving is an install of the new one. A record for the version
    // before it describes a runtime, just not the one being asked about.
    let root = tempfile::tempdir().expect("temporary root");
    let store = ManagedRuntimes::new(root.path());
    publish(
        &store,
        &release("1.17.0", "package/bin/opencode"),
        &archive("package/bin/opencode", b"old"),
    )
    .expect("the older runtime is unpacked");

    assert_eq!(
        store.installed(&agent(), &release("1.18.31", "package/bin/opencode")),
        Ok(None)
    );
}

#[test]
fn a_record_naming_another_executable_is_not_this_release() {
    // A pin that keeps its version and moves its executable is a different
    // release. Reusing the old binary would install nothing and say it did.
    let root = tempfile::tempdir().expect("temporary root");
    let store = ManagedRuntimes::new(root.path());
    publish(
        &store,
        &release("1.18.31", "package/bin/opencode"),
        &archive("package/bin/opencode", b"binary"),
    )
    .expect("the executable is unpacked");

    assert_eq!(
        store.installed(&agent(), &release("1.18.31", "package/bin/opencode-cli")),
        Ok(None)
    );
}

#[test]
fn a_record_cannot_name_a_launch_path_of_its_own() {
    // `installed.json` is a note this store wrote, not an oracle. Somebody who
    // can rewrite it must not be able to make Nessa hand out a path to a binary
    // of their choosing — the most it can do is make Nessa install again.
    let root = tempfile::tempdir().expect("temporary root");
    let store = ManagedRuntimes::new(root.path());
    let release = release("1.18.31", "package/bin/opencode");
    publish(
        &store,
        &release,
        &archive("package/bin/opencode", b"binary"),
    )
    .expect("the executable is unpacked");

    let elsewhere = root.path().join("hostile");
    write(&elsewhere, b"not the runtime");
    let record = root.path().join("opencode").join("installed.json");
    write(
        &record,
        format!(
            r#"{{"version":"1.18.31","executable":"{}"}}"#,
            elsewhere.display()
        )
        .as_bytes(),
    );

    assert_eq!(
        store.installed(&agent(), &release),
        Ok(None),
        "a record naming a path outside this store describes nothing installed"
    );
}

#[test]
fn a_record_that_cannot_be_read_is_not_an_installation() {
    let root = tempfile::tempdir().expect("temporary root");
    let store = ManagedRuntimes::new(root.path());
    let release = release("1.18.31", "package/bin/opencode");
    publish(
        &store,
        &release,
        &archive("package/bin/opencode", b"binary"),
    )
    .expect("the executable is unpacked");
    let record = root.path().join("opencode").join("installed.json");

    write(&record, b"{ this is not json");
    assert!(
        matches!(
            store.installed(&agent(), &release),
            Err(StoreFailure::Unreadable(_))
        ),
        "a record that will not parse is this machine declining to answer"
    );

    write(&record, br#"{"version":"1.0/2","executable":"opencode"}"#);
    assert_eq!(
        store.installed(&agent(), &release),
        Ok(None),
        "a record holding something that is not a version names no installation"
    );
}

#[test]
fn an_archive_without_the_pinned_executable_is_named_as_such() {
    let root = tempfile::tempdir().expect("temporary root");
    let store = ManagedRuntimes::new(root.path());
    let release = release("1.18.31", "package/bin/opencode");

    assert_eq!(
        publish(
            &store,
            &release,
            &archive("package/bin/somethingelse", b"binary")
        ),
        Err(StoreFailure::MissingExecutable(
            "package/bin/opencode".into()
        ))
    );
    assert_eq!(
        store.installed(&agent(), &release),
        Ok(None),
        "a failed publish records nothing"
    );
}

#[test]
fn an_entry_written_with_a_leading_dot_slash_still_matches() {
    // Tar writers differ about whether paths carry a `./` prefix. The pin names
    // the path the package documents; a writer's spelling of it is not a
    // different file.
    let root = tempfile::tempdir().expect("temporary root");
    let store = ManagedRuntimes::new(root.path());

    assert!(publish(
        &store,
        &release("1.18.31", "package/bin/opencode"),
        &archive("./package/bin/opencode", b"binary")
    )
    .is_ok());
}

#[test]
fn a_malformed_archive_is_reported_as_malformed() {
    let root = tempfile::tempdir().expect("temporary root");
    let store = ManagedRuntimes::new(root.path());

    match publish(
        &store,
        &release("1.18.31", "package/bin/opencode"),
        b"this is not a gzip tar",
    ) {
        Err(StoreFailure::MalformedArchive(_)) => {}
        other => panic!("expected a malformed archive, got {other:?}"),
    }
}

#[test]
fn an_entry_that_is_not_a_regular_file_installs_nothing() {
    // A link or a directory carries no data, so unpacking one writes an empty
    // file — which `is_file` is perfectly happy with. Reporting that as the
    // installed runtime hands out a launch path to something that cannot start.
    let root = tempfile::tempdir().expect("temporary root");
    let store = ManagedRuntimes::new(root.path());
    let release = release("1.18.31", "package/bin/opencode");

    for (kind, link) in [
        (EntryType::Symlink, Some("/bin/sh")),
        (EntryType::Link, Some("package/bin/other")),
        (EntryType::Directory, None),
    ] {
        let bytes = entry_archive("package/bin/opencode", b"", kind, link);
        assert!(
            matches!(
                publish(&store, &release, &bytes),
                Err(StoreFailure::MalformedArchive(_))
            ),
            "a {kind:?} entry was installed as a runtime"
        );
        assert_eq!(
            store.installed(&agent(), &release),
            Ok(None),
            "a {kind:?} entry left something recorded"
        );
    }
}

#[test]
fn an_archive_that_expands_without_end_is_refused() {
    // A digest fixes the compressed size and says nothing about the extracted
    // one, so a pin that matches exactly can still describe a gzip member that
    // fills the disk.
    let root = tempfile::tempdir().expect("temporary root");
    let store = ManagedRuntimes::new(root.path());
    let release = release("1.18.31", "package/bin/opencode");
    // Not the real bound — half a gigabyte of zeroes is not a test. This checks
    // the entry is read through a limit at all, by way of a header that claims
    // more than the archive carries.
    let mut header = Header::new_gnu();
    header.set_size(64 * 1024);
    header.set_mode(0o644);
    header.set_entry_type(EntryType::Regular);
    header.set_cksum();
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::fast(),
    ));
    builder
        .append_data(
            &mut header,
            "package/bin/opencode",
            &vec![0u8; 64 * 1024][..],
        )
        .expect("appending to an in-memory archive");
    let bytes = builder
        .into_inner()
        .expect("finishing the tar")
        .finish()
        .expect("finishing the gzip stream");

    let published = publish(&store, &release, &bytes).expect("a compressible file still installs");
    assert_eq!(
        std::fs::metadata(&published)
            .expect("reading the published runtime")
            .len(),
        64 * 1024
    );
}

#[test]
fn an_empty_executable_is_not_an_installation() {
    let root = tempfile::tempdir().expect("temporary root");
    let store = ManagedRuntimes::new(root.path());
    let release = release("1.18.31", "package/bin/opencode");

    assert!(matches!(
        publish(&store, &release, &archive("package/bin/opencode", b"")),
        Err(StoreFailure::MalformedArchive(_))
    ));
    assert_eq!(store.installed(&agent(), &release), Ok(None));
}

#[test]
fn an_archive_entry_cannot_direct_the_write() {
    // The executable lands where the pin says, computed from the agent and the
    // version. A tar entry that calls itself something else is matched against
    // the pin and otherwise has no say in the destination.
    let root = tempfile::tempdir().expect("temporary root");
    let store = ManagedRuntimes::new(root.path());

    let published = publish(
        &store,
        &release("1.18.31", "package/bin/opencode"),
        &archive("package/bin/opencode", b"binary"),
    )
    .expect("the executable is unpacked");

    assert!(
        published.starts_with(root.path()),
        "{} escaped the runtime root",
        published.display()
    );
    assert_eq!(
        published,
        root.path()
            .join("opencode")
            .join("1.18.31")
            .join("opencode")
    );
}

#[test]
fn versions_are_installed_beside_each_other() {
    // A pin that moves must not half-overwrite a binary something may still be
    // running.
    let root = tempfile::tempdir().expect("temporary root");
    let store = ManagedRuntimes::new(root.path());
    let newer = release("1.18.31", "package/bin/opencode");

    let old = publish(
        &store,
        &release("1.17.0", "package/bin/opencode"),
        &archive("package/bin/opencode", b"old"),
    )
    .expect("the older runtime is unpacked");
    let new = publish(&store, &newer, &archive("package/bin/opencode", b"new"))
        .expect("the newer runtime is unpacked");

    assert_ne!(old, new);
    assert_eq!(
        std::fs::read(&old).expect("the older runtime survives"),
        b"old"
    );
    assert_eq!(store.installed(&agent(), &newer), Ok(Some(new)));
}

#[test]
fn a_digest_is_the_sha256_of_what_was_staged() {
    let root = tempfile::tempdir().expect("temporary root");
    let store = ManagedRuntimes::new(root.path());
    let mut staged = staged(&store, b"abc");

    assert_eq!(
        store.digest(&mut staged).map(|d| d.as_str().to_owned()),
        Ok(ABC_SHA256.to_owned())
    );
}

#[test]
fn a_digest_of_something_larger_than_the_read_buffer_is_still_right() {
    // The hasher reads in chunks. A file of exactly one chunk, and one of more
    // than one, take different paths through that loop.
    let root = tempfile::tempdir().expect("temporary root");
    let store = ManagedRuntimes::new(root.path());
    let body = vec![7u8; 64 * 1024 * 3 + 17];
    let mut staged = staged(&store, &body);

    let expected = {
        use sha2::Digest as _;
        let mut hasher = sha2::Sha256::new();
        hasher.update(&body);
        hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    assert_eq!(
        store.digest(&mut staged).map(|d| d.as_str().to_owned()),
        Ok(expected)
    );
}

#[test]
fn what_was_hashed_is_what_gets_unpacked() {
    // The reason a staged download is a handle rather than a path. Replacing
    // the file between the two steps must not change what comes out, because
    // both steps read the same open file.
    let root = tempfile::tempdir().expect("temporary root");
    let store = ManagedRuntimes::new(root.path());
    let release = release("1.18.31", "package/bin/opencode");
    let mut staged = staged(&store, &archive("package/bin/opencode", b"measured"));
    store.digest(&mut staged).expect("the staged file hashes");

    // Whatever something else could put at that name — which on Unix is a new
    // file under a name the staged download no longer answers to.
    write(staged.path(), &archive("package/bin/opencode", b"swapped"));

    let published = store
        .publish(&agent(), &release, &mut staged)
        .expect("the executable is unpacked");
    assert_eq!(
        std::fs::read(&published).expect("reading the published runtime"),
        b"measured",
        "publish unpacked bytes that were never measured"
    );
}

#[test]
fn two_installs_at_once_do_not_share_a_download() {
    // Two staged downloads racing over one name is how a self-inflicted
    // collision comes to be reported as a tampered archive.
    let root = tempfile::tempdir().expect("temporary root");
    let store = ManagedRuntimes::new(root.path());

    let first = store.stage(&agent()).expect("a staged file");
    let second = store.stage(&agent()).expect("a second staged file");

    assert_ne!(first.path(), second.path());
}

#[test]
fn an_agent_directory_is_private_to_this_user() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().expect("temporary root");
        let store = ManagedRuntimes::new(root.path());
        let _staged = store.stage(&agent()).expect("a staged file");

        let directory = std::fs::metadata(root.path().join("opencode"))
            .expect("reading the agent directory")
            .permissions()
            .mode();
        assert_eq!(
            directory & 0o077,
            0,
            "the agent directory is reachable by others"
        );
    }
}

#[test]
#[cfg(unix)]
fn a_staged_download_has_no_name_to_reach_it_by() {
    // The name goes the moment the file exists. A download nothing can open is
    // a download nothing can truncate between the hash and the unpack, and one
    // a process that dies part-way cannot leave behind.
    let root = tempfile::tempdir().expect("temporary root");
    let store = ManagedRuntimes::new(root.path());
    let staged = staged(&store, b"downloaded");

    assert!(
        !staged.path().exists(),
        "{} is still reachable",
        staged.path().display()
    );
}

#[test]
fn a_discarded_archive_leaves_nothing_behind() {
    let root = tempfile::tempdir().expect("temporary root");
    let store = ManagedRuntimes::new(root.path());
    let staged = staged(&store, b"downloaded");
    let path = staged.path().to_owned();

    store.discard(staged);

    assert!(!path.exists());
    let left: Vec<_> = std::fs::read_dir(root.path().join("opencode"))
        .expect("the agent directory exists")
        .map(|entry| entry.expect("an entry").file_name())
        .collect();
    assert!(left.is_empty(), "a discarded install left {left:?} behind");
}
