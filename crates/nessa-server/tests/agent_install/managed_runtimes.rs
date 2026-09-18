use super::*;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use sha2::Sha256;
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
) -> Result<PathBuf, StoreFailure> {
    let mut staged = staged(store, bytes);
    store.publish(&agent(), release, &mut staged)
}

/// Where `publish` puts the version directory for the release these tests use.
///
/// The layout is named once here, so that moving it is one edit rather than a
/// search — and so that a test asserting on it is asserting on the store's
/// shape rather than restating a guess about it.
fn version_path(root: &Path) -> PathBuf {
    root.join("opencode").join("versions").join("1.18.31")
}

/// That same directory, created the way the store does.
///
/// Made with the store's own primitive rather than `create_dir_all`, because
/// the store refuses a directory anybody else can read — which is the point.
fn version_directory(root: &Path) -> PathBuf {
    let directory = version_path(root);
    nessa_local_storage::create_directory(&directory).expect("a private version directory");
    directory
}

/// Where `publish` puts the executable for the release these tests use.
fn installed_path(root: &Path) -> PathBuf {
    version_path(root).join("opencode")
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
fn a_symbolic_link_is_not_an_installed_runtime() {
    // Answering with a link would hand out whatever it points at as the tested
    // runtime, with no download, no digest and none of the archive's own checks
    // ever running. A link is not what this store published.
    let root = tempfile::tempdir().expect("temporary root");
    let store = ManagedRuntimes::new(root.path());
    let release = release("1.18.31", "package/bin/opencode");
    let published = publish(
        &store,
        &release,
        &archive("package/bin/opencode", b"binary"),
    )
    .expect("the executable is unpacked");
    let elsewhere = root.path().join("elsewhere");
    write(&elsewhere, b"not the runtime");

    std::fs::remove_file(&published).expect("removing the published runtime");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&elsewhere, &published).expect("planting a link");
    #[cfg(not(unix))]
    write(&published, b"not the runtime");

    #[cfg(unix)]
    assert_eq!(store.installed(&agent(), &release), Ok(None));
}

#[test]
#[cfg(unix)]
fn a_record_that_is_a_symbolic_link_is_not_read_through() {
    // `installed.json` is a private file this store wrote. A link in its place
    // is not one, and following it would answer from a document somebody else
    // put somewhere else.
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
    let elsewhere = root.path().join("elsewhere.json");
    std::fs::rename(&record, &elsewhere).expect("moving the record aside");
    std::os::unix::fs::symlink(&elsewhere, &record).expect("planting a link");

    assert!(
        matches!(
            store.installed(&agent(), &release),
            Err(StoreFailure::Unreadable(_))
        ),
        "a linked record was read through"
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
    // Serialised rather than formatted into the string: a Windows path is full
    // of backslashes, and pasting one into JSON by hand writes a record that is
    // not JSON at all — which would refuse for the wrong reason and prove
    // nothing about the rule under test.
    let hostile_record = serde_json::json!({
        "version": "1.18.31",
        "executable": elsewhere,
    })
    .to_string();
    write(&record, hostile_record.as_bytes());

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
fn an_archive_that_expands_past_the_bound_is_refused() {
    // A digest fixes the compressed size and says nothing about the extracted
    // one, so a pin that matches exactly can still describe a gzip member that
    // fills the disk. The bound is handed in rather than taken from the
    // constant, so that reaching it costs a kilobyte here instead of half a
    // gigabyte.
    let root = tempfile::tempdir().expect("temporary root");
    let store = ManagedRuntimes::new(root.path());
    let release = release("1.18.31", "package/bin/opencode");
    // A kilobyte of zeroes compresses to almost nothing, which is the shape of
    // the attack: small on the wire, large on the disk.
    let mut staged = staged(&store, &archive("package/bin/opencode", &[0u8; 1024]));
    let directory = version_directory(root.path());
    let destination = directory.join("opencode");

    let failure = store
        .unpack(&release, &mut staged, &destination, &directory, 512)
        .expect_err("an entry past the bound");

    assert!(
        matches!(failure, StoreFailure::MalformedArchive(_)),
        "{failure:?}"
    );
    assert!(
        !destination.exists(),
        "an entry past the bound was published anyway"
    );
}

#[test]
fn an_archive_exactly_at_the_bound_is_unpacked() {
    let root = tempfile::tempdir().expect("temporary root");
    let store = ManagedRuntimes::new(root.path());
    let release = release("1.18.31", "package/bin/opencode");
    let mut staged = staged(&store, &archive("package/bin/opencode", &[0u8; 512]));
    let directory = version_directory(root.path());
    let destination = directory.join("opencode");

    assert_eq!(
        store.unpack(&release, &mut staged, &destination, &directory, 512),
        Ok(true)
    );
    assert_eq!(
        std::fs::metadata(&destination)
            .expect("reading the published runtime")
            .len(),
        512
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
    assert_eq!(published, installed_path(root.path()));
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
        let mut hasher = Sha256::new();
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

// Unix only, because what this stages the attack through is. Reading both
// steps off one handle is written the same way everywhere, but the attack it
// defends against needs a name to aim at, and `release_name` says in its own
// documentation that Windows will not unlink a file that is still open. There
// the name stays, a truncating write through it reaches the very file object
// the handle holds, and the property is false — a gap that is already written
// down beside the code that leaves it, and that nothing reaches today because
// Nessa pins no Windows release.
//
// Not rewritten to swap by rename so that it could run everywhere. That
// replaces the name rather than the file behind it, which is a weaker attack
// and one Windows survives, so a green result there would say the property
// holds when it does not. [`a_staged_download_has_no_name_to_reach_it_by`]
// covers the unlinking on its own.
#[test]
#[cfg(unix)]
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

/// The uncompressed tar bytes for each of `entries`, in order.
fn tarball(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (path, body) in entries {
        let mut header = Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(EntryType::Regular);
        header.set_cksum();
        builder
            .append_data(&mut header, path, *body)
            .expect("appending to an in-memory archive");
    }
    builder.into_inner().expect("finishing the tar")
}

/// One tar stream, compressed as two gzip members back to back.
///
/// This is what a `.tar.gz` assembled by concatenation looks like, and it is
/// the case a decoder that stops at the first member gets wrong. Concatenating
/// two *archives* would not test it: tar stops at the first one's
/// end-of-archive marker long before the decoder matters.
fn split_across_gzip_members(tar: &[u8]) -> Vec<u8> {
    let (first, second) = tar.split_at(tar.len() / 2);
    let mut members = Vec::new();
    for part in [first, second] {
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gzip.write_all(part).expect("compressing a test archive");
        members.extend_from_slice(&gzip.finish().expect("finishing a gzip member"));
    }
    members
}

/// A gzip tar whose entry header claims `declared` bytes but carries `body`.
///
/// Built by hand rather than with `tar::Builder`, which writes the header from
/// the data it is given and so cannot produce the disagreement being tested.
fn short_entry_archive(path: &str, body: &[u8], declared: u64) -> Vec<u8> {
    let mut header = Header::new_gnu();
    header.set_size(declared);
    header.set_mode(0o644);
    header.set_entry_type(EntryType::Regular);
    header
        .set_path(path)
        .expect("a path for an in-memory archive");
    header.set_cksum();

    let mut tar = Vec::new();
    tar.extend_from_slice(header.as_bytes());
    tar.extend_from_slice(body);
    // Pad the data to the 512-byte block tar counts in, then the two zero
    // blocks that mark the end of the archive.
    tar.resize(tar.len().next_multiple_of(512), 0);
    tar.resize(tar.len() + 1024, 0);

    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gzip.write_all(&tar).expect("compressing a test archive");
    gzip.finish().expect("finishing the gzip stream")
}

#[test]
fn an_entry_shorter_than_its_header_says_is_not_installed() {
    // The digest has already matched at this point, so nothing downstream will
    // catch this. Without the check, tar hands over the 512 bytes that are
    // there plus the padding it reads past them, and that becomes a file made
    // executable and recorded as the tested runtime — a binary that is
    // genuinely a fragment of one.
    let root = tempfile::tempdir().expect("temporary root");
    let store = ManagedRuntimes::new(root.path());
    let release = release("1.18.31", "package/bin/opencode");
    let truncated = short_entry_archive("package/bin/opencode", &[b'x'; 512], 100_000);

    let failure = publish(&store, &release, &truncated).expect_err("a short entry is not a file");

    assert!(
        matches!(failure, StoreFailure::MalformedArchive(_)),
        "a truncated entry is the archive's fault, not this machine's: {failure:?}"
    );
    let message = failure.to_string();
    assert!(
        message.contains("100000"),
        "the message should say what the archive claimed: {message}"
    );
    assert_eq!(
        store.installed(&agent(), &release),
        Ok(None),
        "a fragment of an executable must not be recorded as the runtime"
    );
    assert!(
        !installed_path(root.path()).exists(),
        "a refused entry left an executable behind"
    );
}

#[test]
fn an_entry_whose_bytes_come_apart_is_the_archives_fault() {
    // `io::copy` reports a read failure and a write failure as the same thing.
    // A gzip stream that stops half way through the entry is a corrupt archive,
    // and telling somebody their machine could not write it sends them to look
    // at a disk that is fine.
    let root = tempfile::tempdir().expect("temporary root");
    let store = ManagedRuntimes::new(root.path());
    let release = release("1.18.31", "package/bin/opencode");
    let whole = archive("package/bin/opencode", &vec![b'x'; 200_000]);
    let cut = &whole[..whole.len() / 2];

    let failure = publish(&store, &release, cut).expect_err("a truncated gzip stream");

    assert!(
        matches!(failure, StoreFailure::MalformedArchive(_)),
        "a stream that came apart is not a machine that could not write: {failure:?}"
    );
}

#[test]
fn an_executable_past_the_first_gzip_member_is_still_found() {
    // A `.tar.gz` may be several gzip members back to back, and a decoder that
    // stops at the first would report an archive that plainly contains the
    // pinned executable as one that does not — blaming the pin for its own
    // early stop.
    let root = tempfile::tempdir().expect("temporary root");
    let store = ManagedRuntimes::new(root.path());
    let release = release("1.18.31", "package/bin/opencode");
    // A filler entry large enough that the halfway split lands inside it, so
    // the pinned entry begins only in the second member. Splitting a small
    // archive would put the whole entry in the first one, and the test would
    // pass with a decoder that never reads the second.
    let filler = vec![b'.'; 16 * 1024];
    let split = split_across_gzip_members(&tarball(&[
        ("package/other", filler.as_slice()),
        ("package/bin/opencode", b"the runtime"),
    ]));

    let installed = publish(&store, &release, &split).expect("the second member is read");

    assert_eq!(
        std::fs::read(&installed).expect("the installed runtime reads"),
        b"the runtime"
    );
}

#[test]
fn an_install_that_cannot_be_recorded_leaves_no_runtime_behind() {
    // The record is written after the executable, so there is a moment where
    // one exists and the other does not. Reporting "nothing was installed"
    // while a hundred megabytes sits in a directory nothing will ever look at
    // again is the message being false — and on a full disk, which is the
    // likeliest way to get here, it is false in the direction that hurts.
    let root = tempfile::tempdir().expect("temporary root");
    let store = ManagedRuntimes::new(root.path());
    let release = release("1.18.31", "package/bin/opencode");
    // A directory where the record goes: the rename onto it cannot succeed.
    let record = root.path().join("opencode").join("installed.json");
    nessa_local_storage::create_directory(&record).expect("a directory in the record's place");

    let failure = publish(
        &store,
        &release,
        &archive("package/bin/opencode", b"the runtime"),
    )
    .expect_err("a record that cannot be written fails the publish");

    assert!(
        matches!(failure, StoreFailure::Unwritable(_)),
        "a record that would not write is this machine's doing: {failure:?}"
    );
    assert!(
        !installed_path(root.path()).exists(),
        "a failed install left a runtime nothing records"
    );
    // Not `Ok(None)`: a directory where the record belongs is a store that
    // cannot answer, and it says so. What matters here is that it does not
    // answer with an installation.
    assert!(
        !matches!(store.installed(&agent(), &release), Ok(Some(_))),
        "a failed install was reported as one that worked"
    );
}

#[test]
fn an_entry_that_cannot_be_written_out_is_this_machines_doing() {
    // The other half of what `expand` exists for. The sibling loop in the HTTPS
    // client has this test; the unpacker's write branch had the fix and not the
    // test, which is half a guarantee.
    let root = tempfile::tempdir().expect("temporary root");
    let path = root.path().join("staging");
    write(&path, b"");
    let mut readable = std::fs::File::open(&path).expect("a handle that cannot be written");

    let failure = expand(
        &mut b"the runtime".as_slice(),
        &mut readable,
        &release("1.18.31", "package/bin/opencode"),
    )
    .expect_err("a staging file that cannot be written");

    assert!(
        matches!(failure, StoreFailure::Unwritable(_)),
        "a write that could not complete is not a corrupt archive: {failure:?}"
    );
}

#[test]
fn only_an_entry_with_bytes_of_its_own_is_unpackable() {
    assert_eq!(refused_kind(EntryType::Regular), None);
    assert_eq!(refused_kind(EntryType::Continuous), None);
    for carries_nothing in [
        EntryType::Symlink,
        EntryType::Link,
        EntryType::Directory,
        EntryType::Fifo,
        EntryType::Char,
        EntryType::Block,
    ] {
        assert_eq!(
            refused_kind(carries_nothing),
            Some("is not a regular file in the archive"),
            "{carries_nothing:?} carries no data"
        );
    }
    // A sparse entry is a regular file, so this one refusal has to say
    // something else — otherwise the message is false about what it refused.
    assert_eq!(
        refused_kind(EntryType::GNUSparse),
        Some("is stored sparsely, which nessa does not unpack")
    );
}

#[test]
fn an_archive_without_the_pinned_executable_leaves_no_directory_behind() {
    // The version directory is made before the archive is known to hold
    // anything. A pin that is wrong about its own contents would otherwise
    // leave an empty one on every attempt.
    let root = tempfile::tempdir().expect("temporary root");
    let store = ManagedRuntimes::new(root.path());
    let release = release("1.18.31", "package/bin/opencode");

    publish(&store, &release, &archive("package/bin/other", b"binary"))
        .expect_err("an archive without the pinned executable");

    assert!(
        !version_path(root.path()).exists(),
        "a refused install left its version directory behind"
    );
}

// Unix only, because the directory sync this asserts on is. Windows takes the
// same durability from the write-through move in `replace`, and
// `sync_directory` documents itself as a no-op there, so `settle` has no
// failure to report and there is no distinction left to assert.
#[test]
#[cfg(unix)]
fn an_install_is_not_settled_until_its_directory_is_durable() {
    // The rename survives a crash only once the directory holding it does, so
    // that sync belongs after the executable exists and inside the same guard
    // as the record. Asserted directly on `settle`, because that is what tells
    // the two apart: with the sync back inside `unpack`, settling a directory
    // that is not there would write the record and report success.
    let root = tempfile::tempdir().expect("temporary root");
    let store = ManagedRuntimes::new(root.path());
    let release = release("1.18.31", "package/bin/opencode");
    // The agent directory has to exist, or writing the record would fail on its
    // own and this would pass without the sync ever being reached.
    nessa_local_storage::create_directory(&root.path().join("opencode"))
        .expect("a private agent directory");

    let failure = store
        .settle(&agent(), &release, &root.path().join("not-there"))
        .expect_err("a directory that cannot be opened is not durable");

    assert!(
        matches!(failure, StoreFailure::Unwritable(_)),
        "a directory that will not sync is this machine's doing: {failure:?}"
    );
    assert_eq!(
        store.installed(&agent(), &release),
        Ok(None),
        "an install that was never made durable must not be recorded"
    );
}

#[test]
fn a_failed_install_does_not_remove_a_runtime_it_did_not_write() {
    // A working install whose record goes missing — a tidy-up, a partial
    // restore — reads as nothing installed, so the use case publishes again. If
    // that archive turns out not to hold the pinned executable, the clean-up
    // must sweep what this attempt made and nothing else. Taking the runtime
    // away would mean a pin that is wrong about its own contents deleting the
    // binary somebody was using.
    let root = tempfile::tempdir().expect("temporary root");
    let store = ManagedRuntimes::new(root.path());
    let release = release("1.18.31", "package/bin/opencode");
    let installed = publish(
        &store,
        &release,
        &archive("package/bin/opencode", b"the runtime"),
    )
    .expect("the first install works");
    std::fs::remove_file(root.path().join("opencode").join("installed.json"))
        .expect("losing the record");

    let failure = publish(&store, &release, &archive("package/bin/other", b"binary"))
        .expect_err("an archive without the pinned executable");

    assert!(
        matches!(failure, StoreFailure::MissingExecutable(_)),
        "{failure:?}"
    );
    assert_eq!(
        std::fs::read(&installed).expect("the runtime is still there"),
        b"the runtime",
        "a refused install removed a runtime it had not written"
    );
}

#[test]
fn every_way_of_refusing_an_archive_sweeps_the_directory_it_made() {
    // The version directory is created before the archive is known to hold
    // anything, and there are several ways for it not to. Each of them leaves
    // an empty directory unless the sweep covers the lot.
    for (named, bytes) in [
        ("not an archive at all", b"this is not a gzip tar".to_vec()),
        (
            "an entry that carries no data",
            entry_archive(
                "package/bin/opencode",
                b"",
                EntryType::Symlink,
                Some("elsewhere"),
            ),
        ),
        ("an empty executable", archive("package/bin/opencode", b"")),
        (
            "an entry shorter than its header",
            short_entry_archive("package/bin/opencode", &[b'x'; 512], 100_000),
        ),
        (
            "an archive without the pinned executable",
            archive("package/bin/other", b"binary"),
        ),
    ] {
        let root = tempfile::tempdir().expect("temporary root");
        let store = ManagedRuntimes::new(root.path());
        let release = release("1.18.31", "package/bin/opencode");

        publish(&store, &release, &bytes).expect_err(named);

        assert!(
            !version_path(root.path()).exists(),
            "{named} left its version directory behind"
        );
    }
}

#[test]
fn a_version_may_be_spelled_like_the_stores_own_files() {
    // `installed.json` is a version the domain admits, and it named the record
    // when versions sat directly under the agent. Installing one and then
    // reading it back is what shows the two no longer share a path.
    let root = tempfile::tempdir().expect("temporary root");
    let store = ManagedRuntimes::new(root.path());
    let awkward = release("installed.json", "package/bin/opencode");

    let published = publish(
        &store,
        &awkward,
        &archive("package/bin/opencode", b"the runtime"),
    )
    .expect("a version spelled like the record still installs");

    assert_eq!(
        store.installed(&agent(), &awkward),
        Ok(Some(published.clone()))
    );
    assert_eq!(
        std::fs::read(&published).expect("the runtime reads"),
        b"the runtime"
    );
}

#[test]
#[cfg(unix)]
fn a_failure_after_the_rename_says_nothing_was_installed_and_means_it() {
    // The case a guard here used to get wrong. A record left over from an
    // earlier install whose executable went missing reads as nothing installed,
    // so the command publishes again — and if settling that install fails, the
    // executable has to go back out. Leaving it would mean reporting a failure
    // with a live, recorded runtime on the disk.
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().expect("temporary root");
    let store = ManagedRuntimes::new(root.path());
    let release = release("1.18.31", "package/bin/opencode");
    let installed = publish(
        &store,
        &release,
        &archive("package/bin/opencode", b"the runtime"),
    )
    .expect("the first install works");
    // The executable goes, the record stays: what a tidy-up leaves behind.
    std::fs::remove_file(&installed).expect("losing the executable");
    assert_eq!(store.installed(&agent(), &release), Ok(None));
    // Staged before the directory is loosened, because staging goes through the
    // same rule and would otherwise fail first — which is not the case here.
    let mut staged = staged(&store, &archive("package/bin/opencode", b"the runtime"));
    // Settling cannot finish: the record is written through a private temporary
    // file in the agent directory, and that directory is no longer private. The
    // version directory below it still is, so the unpack and the rename both
    // succeed and the failure lands exactly in the window being tested.
    let agent_root = root.path().join("opencode");
    std::fs::set_permissions(&agent_root, std::fs::Permissions::from_mode(0o755))
        .expect("loosening the agent directory");

    let failure = store
        .publish(&agent(), &release, &mut staged)
        .expect_err("an install that cannot be settled");

    assert!(
        matches!(failure, StoreFailure::Unwritable(_)),
        "a record that would not write is this machine's doing: {failure:?}"
    );
    assert!(
        !installed.exists(),
        "a reported failure left a runtime behind that the old record now names"
    );
}
