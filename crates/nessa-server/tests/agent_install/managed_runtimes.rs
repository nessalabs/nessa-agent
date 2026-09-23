use super::*;
use std::cell::RefCell;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

use nessa_local_storage::OpenMode;

use sha2::Sha256;
use tar::{EntryType, Header};

use crate::agent_install::domain::{
    AgentName, ArchivePath, ArchiveSize, ArchiveUrl, FileRole, Libc, ReleaseContents, ReleaseFile,
    ReleasePlatform, ReleaseRequirements,
};
use crate::agent_install_test_support::temporary_root;

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
    artifact(version, executable, &"a".repeat(64), "macos", "aarch64")
}

/// How long the archive these tests pin says it is. Nothing here fetches, so
/// the number only has to be a length an archive could have.
const ARCHIVE_BYTES: u64 = 1024 * 1024;

/// Contents holding one program and nothing else, which is Opencode's shape.
fn one_program(path: &str) -> ReleaseContents {
    installing(&[(path, FileRole::Launch)])
}

/// Contents holding exactly these files in these roles.
fn installing(files: &[(&str, FileRole)]) -> ReleaseContents {
    ReleaseContents::new(
        files
            .iter()
            .map(|(path, role)| {
                ReleaseFile::new(ArchivePath::parse(path).expect("contained path"), *role)
            })
            .collect(),
    )
    .expect("a release that names one program to launch")
}

/// A release installing a whole package rather than one program.
///
/// Codex's shape, cut down to what a test can build in memory: a program, a
/// helper the program finds through the directory it sits in, and a document
/// that is installed unrunnable.
fn package(version: &str) -> PinnedRelease {
    release_installing(
        version,
        &"a".repeat(64),
        installing(&[
            ("package/vendor/bin/codex", FileRole::Launch),
            ("package/vendor/codex-path/rg", FileRole::Helper),
            ("package/package.json", FileRole::Document),
        ]),
    )
}

/// The three entries `package` names, as a tarball carries them.
fn package_entries() -> Vec<(&'static str, &'static [u8])> {
    vec![
        ("package/vendor/bin/codex", b"codex program".as_slice()),
        ("package/vendor/codex-path/rg", b"ripgrep".as_slice()),
        ("package/package.json", b"{}".as_slice()),
    ]
}

/// Where one of `package`'s files lands.
fn package_path(root: &Path, path: &str) -> PathBuf {
    let mut at = artifact_path(root);
    for segment in path.split('/') {
        at = at.join(segment);
    }
    at
}

/// A release naming one specific artifact of a version.
///
/// The digest and the platform are arguments because they are what tells two
/// archives of one version apart, and this store is required to tell them
/// apart: nine Opencode archives share a version and unpack a file of the same
/// name.
fn artifact(
    version: &str,
    executable: &str,
    digest: &str,
    operating_system: &str,
    architecture: &str,
) -> PinnedRelease {
    PinnedRelease::new(
        ReleaseVersion::parse(version).expect("usable version"),
        ReleasePlatform::new(operating_system, architecture).expect("usable platform"),
        ReleaseRequirements::default(),
        ArchiveUrl::parse("https://registry.example/runtime.tgz").expect("a fetchable url"),
        ArchiveSize::parse(ARCHIVE_BYTES).expect("usable archive size"),
        ArchiveDigest::parse(digest).expect("usable digest"),
        one_program(executable),
    )
    .expect("a release whose requirements fit its platform")
}

/// The same, for a release whose contents are more than one program.
fn release_installing(version: &str, digest: &str, contents: ReleaseContents) -> PinnedRelease {
    PinnedRelease::new(
        ReleaseVersion::parse(version).expect("usable version"),
        ReleasePlatform::new("macos", "aarch64").expect("usable platform"),
        ReleaseRequirements::default(),
        ArchiveUrl::parse("https://registry.example/runtime.tgz").expect("a fetchable url"),
        ArchiveSize::parse(ARCHIVE_BYTES).expect("usable archive size"),
        ArchiveDigest::parse(digest).expect("usable digest"),
        contents,
    )
    .expect("a release whose requirements fit its platform")
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
    store
        .publish(&agent(), release, &mut staged)
        .map(|publication| publication.executable().to_owned())
        .map_err(|failure| failure.failure().clone())
}

/// Where `publish` puts the version directory for the release these tests use.
///
/// The layout is named once here, so that moving it is one edit rather than a
/// search — and so that a test asserting on it is asserting on the store's
/// shape rather than restating a guess about it.
fn version_path(root: &Path) -> PathBuf {
    root.join("opencode").join("versions").join("1.18.31")
}

/// Where `publish` puts *this artifact* of that version.
///
/// Under the archive's digest, because one version is published as several
/// archives and all of them unpack a file of the same name. `release` uses a
/// digest of sixty-four `a`s, so that is the directory name here.
fn artifact_path(root: &Path) -> PathBuf {
    version_path(root).join("a".repeat(64))
}

/// The same directory, spelled the way the store spells it.
///
/// Every path inside the store is relative to its root, because every one of
/// them is walked down from the root rather than resolved from the outside.
/// The helpers above stay absolute: they are for looking at the disk, which is
/// the one thing a test does from outside.
fn artifact_relative() -> PathBuf {
    Path::new("opencode")
        .join("versions")
        .join("1.18.31")
        .join("a".repeat(64))
}

/// The directories `unpack` expects to already exist, made the way `publish`
/// makes them.
///
/// `unpack` is called directly by the tests that exercise the bound, and it is
/// deliberately not the step that creates anything: `publish` makes every
/// directory the pin's paths need before a single entry is read, so that a
/// release this machine cannot hold fails before anything is written.
fn unpack_directories(store: &ManagedRuntimes, release: &PinnedRelease) {
    store
        .private_directory(&store.artifact_root(&agent(), release))
        .expect("a private artifact directory");
    for directory in store.content_directories(&agent(), release) {
        store
            .private_directory(&directory)
            .expect("a private directory inside the artifact");
    }
}

/// Where the executable lands, spelled the way the store spells it.
fn installed_relative() -> PathBuf {
    artifact_relative()
        .join("package")
        .join("bin")
        .join("opencode")
}

/// That same directory, created the way the store does.
///
/// Made with the store's own primitive rather than `create_dir_all`, because
/// the store refuses a directory anybody else can read — which is the point.
fn artifact_directory(root: &Path) -> PathBuf {
    let directory = artifact_path(root);
    nessa_local_storage::create_directory(&directory).expect("a private artifact directory");
    directory
}

/// Where `publish` puts the executable for the release these tests use.
///
/// The archive's own path below the artifact directory, not the file name
/// alone: a package's files find each other through the directories they sit
/// in, so the layout is reproduced rather than flattened.
fn installed_path(root: &Path) -> PathBuf {
    artifact_path(root)
        .join("package")
        .join("bin")
        .join("opencode")
}

/// The record this store writes for the release these tests use.
///
/// Built here rather than read back, so a test can rewrite the file with
/// something that differs in exactly one field and nothing else.
fn record_of(version: &str, executable: &str) -> serde_json::Value {
    // `requires_avx2`, not `requiresAvx2`: this file is the store's own note
    // and carries no `rename_all`, unlike the pin file. Spelled the pin file's
    // way the key would be refused outright, and before the fields were
    // required it was worse — serde filled the field from its default and the
    // helper's promise to differ "in exactly one field and nothing else" was
    // quietly false.
    serde_json::json!({
        "version": version,
        "platform": "macos-aarch64",
        "libc": serde_json::Value::Null,
        "requires_avx2": false,
        "digest": "a".repeat(64),
        "files": [{ "path": executable, "role": "launch" }],
    })
}

fn write(path: &Path, bytes: &[u8]) {
    let mut file = std::fs::File::create(path).expect("writing a test file");
    file.write_all(bytes).expect("writing a test file");
}

#[test]
fn nothing_recorded_is_nothing_installed() {
    let root = temporary_root();
    let store = ManagedRuntimes::new(root.path());
    assert_eq!(
        store.installed(&agent(), &release("1.18.31", "package/bin/opencode")),
        Ok(None)
    );
}

#[test]
fn a_published_runtime_is_reported_as_installed() {
    let root = temporary_root();
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
    let root = temporary_root();
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
        assert_eq!(mode & 0o100, 0o100, "the installed runtime is executable");
        // And by its owner alone, like every other file this store writes. The
        // directories above it are private already, so the group and world bits
        // a release archive usually carries would grant nothing — and dropping
        // them is what lets `installed` check this file through the same
        // private-file primitive as the record.
        assert_eq!(
            mode & 0o077,
            0,
            "the installed runtime is reachable by others"
        );
    }
    #[cfg(not(unix))]
    let _ = published;
}

#[test]
fn a_runtime_whose_executable_was_deleted_is_not_installed() {
    // The record is a note, not evidence. A disk cleaner that removed the
    // binary must not leave Nessa handing out a launch path to nothing.
    let root = temporary_root();
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
    let root = temporary_root();
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
    let root = temporary_root();
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
    let root = temporary_root();
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

/// The leaf being a link was never the whole question. Every directory between
/// the store's root and the executable is one something could have replaced
/// with a link, and a store that resolved its paths the ordinary way would
/// follow it: creating, unpacking, removing and finally handing out to be
/// launched, all on the other side of it. The anchored primitives walk down
/// from the root one component at a time and refuse anything that is not a
/// private directory of this user's.
#[test]
#[cfg(unix)]
fn no_directory_between_the_root_and_the_runtime_may_be_a_link() {
    let root = temporary_root();
    let store = ManagedRuntimes::new(root.path());
    let planted_release = release("1.18.31", "package/bin/opencode");
    // A version the far side does not have, for the install below.
    let fresh = release("2.0.0", "package/bin/opencode");

    // Somewhere else entirely, holding what a planted link would aim at: the
    // layout an install would produce, with a binary of its own in it.
    let elsewhere = root.path().join("elsewhere");
    let planted = elsewhere
        .join("1.18.31")
        .join("a".repeat(64))
        .join("opencode");
    nessa_local_storage::create_directory(planted.parent().expect("a directory"))
        .expect("a private directory elsewhere");
    write(&planted, b"not the tested runtime");

    // The agent's own directory exists; `versions` under it is a link.
    let agent_root = root.path().join("opencode");
    nessa_local_storage::create_directory(&agent_root).expect("a private agent directory");
    std::os::unix::fs::symlink(&elsewhere, agent_root.join("versions")).expect("planting a link");

    // Nothing is installed here, whatever is on the other side of the link and
    // however exactly it is laid out. Reported as "not installed" rather than
    // as a failure, because that is the answer an install can act on.
    // Written the way the store writes it — private, owned by this user — so
    // that what the test is about is the link and not the record.
    nessa_local_storage::open(&agent_root.join("installed.json"), OpenMode::CreateNew)
        .expect("a private record")
        .write_all(
            record_of("1.18.31", "package/bin/opencode")
                .to_string()
                .as_bytes(),
        )
        .expect("writing the record");
    assert_eq!(
        store.installed(&agent(), &planted_release),
        Ok(None),
        "a runtime on the far side of a link was handed back as the tested one"
    );

    // And an install refuses rather than unpacking through it. A version the
    // far side does not already have, so that the directories an install makes
    // before it writes anything are observable: creating them through the link
    // is the first thing that would go wrong, and it would go wrong silently.
    let failure = publish(
        &store,
        &fresh,
        &archive("package/bin/opencode", b"the runtime"),
    )
    .expect_err("an install through a link");
    assert!(
        matches!(failure, StoreFailure::Unwritable(_)),
        "{failure:?}"
    );
    assert!(
        !elsewhere.join("2.0.0").exists(),
        "the install made its directories on the far side of the link"
    );
    assert_eq!(
        std::fs::read(&planted).expect("the file outside the store still reads"),
        b"not the tested runtime",
        "the install wrote through the link"
    );
    assert!(
        agent_root.join("versions").symlink_metadata().is_ok(),
        "the link itself was removed as though it were this store's"
    );
}

/// Asking what is installed does not create anything first, so it is the one
/// entry point with no directory walk of its own ahead of it. A link standing
/// in for the agent's whole directory would make an unanchored read answer from
/// somebody else's record — and that record decides whether Nessa hands out a
/// launch path without downloading anything.
#[test]
#[cfg(unix)]
fn an_agent_directory_that_is_a_link_is_not_read_through() {
    let root = temporary_root();
    let store = ManagedRuntimes::new(root.path());
    let pinned = release("1.18.31", "package/bin/opencode");

    // A complete installation, somewhere this store does not own.
    let elsewhere = root.path().join("elsewhere");
    nessa_local_storage::create_directory(&elsewhere).expect("a private directory elsewhere");
    let executable = elsewhere
        .join("versions")
        .join("1.18.31")
        .join("a".repeat(64))
        .join("opencode");
    nessa_local_storage::create_directory(executable.parent().expect("a directory"))
        .expect("a private artifact directory elsewhere");
    nessa_local_storage::open(&executable, OpenMode::CreateNew)
        .expect("a private executable")
        .write_all(b"not the tested runtime")
        .expect("writing the executable");
    nessa_local_storage::open(&elsewhere.join("installed.json"), OpenMode::CreateNew)
        .expect("a private record")
        .write_all(
            record_of("1.18.31", "package/bin/opencode")
                .to_string()
                .as_bytes(),
        )
        .expect("writing the record");

    // It is a complete installation, so reading it the ordinary way would
    // answer with a launch path — which is what makes the link worth planting.
    std::os::unix::fs::symlink(&elsewhere, root.path().join("opencode")).expect("planting a link");

    // Refused, rather than answered from over there. A refusal and not "nothing
    // installed" because a link where this store's own directory should be is
    // this machine declining to answer, not an empty store — and it is no dead
    // end, since an install refuses on the same link and says the same thing.
    let refused = store
        .installed(&agent(), &pinned)
        .expect_err("a record on the far side of a link decided what is installed");
    assert!(
        matches!(refused, StoreFailure::Unreadable(_)),
        "{refused:?}"
    );

    // The very first thing an install does is take the agent's directory, and
    // that is not one.
    let failure = store
        .stage(&agent())
        .expect_err("an install staged a download through a link");
    assert!(
        matches!(failure, StoreFailure::Unwritable(_)),
        "{failure:?}"
    );
    assert_eq!(
        std::fs::read(&executable).expect("the file outside the store still reads"),
        b"not the tested runtime",
        "the install wrote through the link"
    );
}

/// Every path this store touches is walked down from its root.
///
/// Said about the source, because the property is about the calls rather than
/// about any one arrangement of links. A behavioural test only ever reaches the
/// first anchored call on its path: the rest are what keeps a link planted
/// *during* an install — after the directories were checked and before the
/// executable is renamed, removed or synced — from being followed, and no
/// fixture can hold that moment still.
///
/// The one exception is the root itself, which is composition's to give and is
/// resolved the ordinary way exactly once, in `anchor`.
#[test]
fn the_store_reaches_nothing_by_a_path_it_did_not_walk() {
    const SOURCE: &str = include_str!("../../src/agent_install/infrastructure/managed_runtimes.rs");

    for call in [
        "fs::remove_file(",
        "fs::remove_dir(",
        "fs::metadata(",
        "fs::symlink_metadata(",
        "File::open(",
        "nessa_local_storage::open(",
        "PrivateTempFile::new_in(",
        ".persist(",
        "sync_directory(",
    ] {
        assert!(
            !SOURCE.contains(call),
            "{call} resolves a whole path in the kernel, so a link anywhere \
             along it is followed; the `_beneath` form walks down from the root"
        );
    }

    assert_eq!(
        SOURCE.matches("create_directory(").count(),
        1,
        "the root is the one path resolved the ordinary way, and only in `anchor`"
    );

    // And the path-based primitives are not in reach to be called unqualified
    // either. Named rather than counted, so adding one is a decision somebody
    // makes here instead of a name that quietly appears in an import.
    let imported = SOURCE
        .split_once("use nessa_local_storage::{")
        .expect("the store imports its storage primitives as a group")
        .1
        .split_once('}')
        .expect("the import group closes")
        .0;
    for name in imported.split(',').map(str::trim).filter(|n| !n.is_empty()) {
        assert!(
            matches!(
                name,
                "create_directory"
                    | "create_directory_beneath"
                    | "open_beneath"
                    | "remove_directory_beneath"
                    | "remove_file_beneath"
                    | "sync_directory_beneath"
                    | "OpenMode"
                    | "PrivateTempFile"
            ),
            "{name} is not one of the primitives this store reaches the disk through"
        );
    }
}

#[test]
#[cfg(unix)]
fn a_record_that_is_a_symbolic_link_is_not_read_through() {
    // `installed.json` is a private file this store wrote. A link in its place
    // is not one, and following it would answer from a document somebody else
    // put somewhere else.
    let root = temporary_root();
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
    //
    // A record naming anything but the release's own file name is refused
    // before a path is worked out at all, which is
    // `a_record_naming_another_executable_is_not_this_release`. So a record
    // that gets as far as the path is one holding the name this store itself
    // would write, and what is left to prove is the half that runs after that
    // check: the directory the name is joined to comes from the release, never
    // from the record. The record is written out here rather than left as the
    // store wrote it, identical though it is, because the rewriting is the
    // premise: this is what somebody who can edit the file is left with once
    // every other spelling has been refused.
    let root = temporary_root();
    let store = ManagedRuntimes::new(root.path());
    let release = release("1.18.31", "package/bin/opencode");
    let published = publish(
        &store,
        &release,
        &archive("package/bin/opencode", b"binary"),
    )
    .expect("the executable is unpacked");

    // A binary of that same name in the agent's own root, outside `versions/`
    // altogether: where the name would land if it were joined to anything but
    // the directory the release names.
    let elsewhere = root.path().join("opencode").join("opencode");
    write(&elsewhere, b"not the runtime");
    let record = root.path().join("opencode").join("installed.json");
    let rewritten = record_of("1.18.31", "package/bin/opencode").to_string();
    write(&record, rewritten.as_bytes());

    assert_eq!(
        store.installed(&agent(), &release),
        Ok(Some(published)),
        "the launch path came from somewhere a record could point at"
    );
}

#[test]
fn a_record_this_store_did_not_write_is_not_an_installation() {
    // Which is a different thing from a record this machine cannot read: that
    // one is the disk declining to answer and is reported as a failure, which
    // `a_record_that_is_a_symbolic_link_is_not_read_through` covers. What is
    // *in* the file only ever answers the question asked of it, and an answer
    // of "nothing" leaves a person with an install they can simply run again.
    let root = temporary_root();
    let store = ManagedRuntimes::new(root.path());
    let release = release("1.18.31", "package/bin/opencode");
    publish(
        &store,
        &release,
        &archive("package/bin/opencode", b"binary"),
    )
    .expect("the executable is unpacked");
    let record = root.path().join("opencode").join("installed.json");

    // A file that does not decode into a record describes no installation, and
    // saying so is the only answer a person can act on: reporting a failure
    // would be a dead end, since every attempt would fail on the same note and
    // the only way out would be finding the file and deleting it. The record is
    // written to a temporary name and renamed, so it is never half-written —
    // anything that does not decode was written by something else, or by a
    // Nessa that recorded a different shape.
    for (named, contents) in [
        ("a file that is not json", br#"{ this is not json"#.to_vec()),
        (
            "a record from a nessa that wrote a different shape",
            br#"{"version":"1.18.31","executable":"opencode"}"#.to_vec(),
        ),
        ("an empty file", Vec::new()),
    ] {
        write(&record, &contents);
        assert_eq!(
            store.installed(&agent(), &release),
            Ok(None),
            "{named} was reported as this machine declining to answer"
        );
    }

    write(
        &record,
        record_of("1.0/2", "package/bin/opencode")
            .to_string()
            .as_bytes(),
    );
    assert_eq!(
        store.installed(&agent(), &release),
        Ok(None),
        "a record holding something that is not this version names no installation"
    );
}

#[test]
fn an_archive_without_the_pinned_executable_is_named_as_such() {
    let root = temporary_root();
    let store = ManagedRuntimes::new(root.path());
    let release = release("1.18.31", "package/bin/opencode");

    assert_eq!(
        publish(
            &store,
            &release,
            &archive("package/bin/somethingelse", b"binary")
        ),
        Err(StoreFailure::IncompleteArchive(
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
    let root = temporary_root();
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
    let root = temporary_root();
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
    let root = temporary_root();
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
    let root = temporary_root();
    let store = ManagedRuntimes::new(root.path());
    let release = release("1.18.31", "package/bin/opencode");
    // A kilobyte of zeroes compresses to almost nothing, which is the shape of
    // the attack: small on the wire, large on the disk.
    let mut staged = staged(&store, &archive("package/bin/opencode", &[0u8; 1024]));
    unpack_directories(&store, &release);
    let mut written = Vec::new();

    let failure = store
        .unpack(&agent(), &release, &mut staged, 512, &mut written)
        .expect_err("an entry past the bound");

    assert!(
        matches!(failure, StoreFailure::MalformedArchive(_)),
        "{failure:?}"
    );
    assert!(written.is_empty(), "a refused entry was renamed into place");
    assert!(
        !installed_path(root.path()).exists(),
        "an entry past the bound was published anyway"
    );
}

#[test]
fn the_bound_is_spent_across_every_file_a_release_installs() {
    // A per-file bound is no bound at all on a pin naming a hundred of them:
    // each one would be allowed the whole budget. The first two entries here
    // fit under 512 on their own and do not fit together.
    let root = temporary_root();
    let store = ManagedRuntimes::new(root.path());
    let release = package("1.18.31");
    let mut staged = staged(
        &store,
        &gzipped(&tarball(&[
            ("package/vendor/bin/codex", &[0u8; 300]),
            ("package/vendor/codex-path/rg", &[0u8; 300]),
            ("package/package.json", b"{}"),
        ])),
    );
    unpack_directories(&store, &release);
    let mut written = Vec::new();

    let failure = store
        .unpack(&agent(), &release, &mut staged, 512, &mut written)
        .expect_err("two entries that do not fit the budget together");

    assert!(
        matches!(failure, StoreFailure::MalformedArchive(_)),
        "{failure:?}"
    );
    // The first entry did fit, so it was written — and the caller is told, so
    // that the rollback takes it back out. That is the whole reason `written`
    // is an out-parameter.
    assert_eq!(written.len(), 1, "the entry that fitted was not reported");
    let _ = root;
}

#[test]
fn an_archive_exactly_at_the_bound_is_unpacked() {
    let root = temporary_root();
    let store = ManagedRuntimes::new(root.path());
    let release = release("1.18.31", "package/bin/opencode");
    let mut staged = staged(&store, &archive("package/bin/opencode", &[0u8; 512]));
    unpack_directories(&store, &release);
    let mut written = Vec::new();

    assert_eq!(
        store.unpack(&agent(), &release, &mut staged, 512, &mut written),
        Ok(())
    );
    assert_eq!(written, vec![installed_relative()]);
    assert_eq!(
        std::fs::metadata(installed_path(root.path()))
            .expect("reading the published runtime")
            .len(),
        512
    );
}

#[test]
fn an_empty_executable_is_not_an_installation() {
    let root = temporary_root();
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
    let root = temporary_root();
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
    let root = temporary_root();
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
    let root = temporary_root();
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
    let root = temporary_root();
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
// documentation that Windows will not unlink a file that is still open. What
// that doc draws out of it is the download left behind by an install that
// died; the other consequence, and the one this check is about, is that a
// truncating write through the name that stays reaches the very file object
// the handle holds. So on Windows this property is false. Nothing reaches it
// today because Nessa pins no Windows release, and if that changes it is the
// staged download that has to be closed to writers, not this check that has to
// be relaxed.
//
// Not rewritten to swap by rename so that it could run everywhere. That
// replaces the name rather than the file behind it, which is a weaker attack
// and one Windows survives, so a green result there would say the property
// holds when it does not.
#[test]
#[cfg(unix)]
fn what_was_hashed_is_what_gets_unpacked() {
    // The reason a staged download is a handle rather than a path. Replacing
    // the file between the two steps must not change what comes out, because
    // both steps read the same open file.
    let root = temporary_root();
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
        std::fs::read(published.executable()).expect("reading the published runtime"),
        b"measured",
        "publish unpacked bytes that were never measured"
    );
}

#[test]
fn two_installs_at_once_do_not_share_a_download() {
    // Two staged downloads racing over one name is how a self-inflicted
    // collision comes to be reported as a tampered archive.
    let root = temporary_root();
    let store = ManagedRuntimes::new(root.path());

    let first = store.stage(&agent()).expect("a staged file");
    let second = store.stage(&agent()).expect("a second staged file");

    assert_ne!(first.path(), second.path());
}

#[test]
fn an_agent_directory_is_private_to_this_user() {
    #[cfg(unix)]
    {
        let root = temporary_root();
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
    let root = temporary_root();
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
    let root = temporary_root();
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
/// A tar carrying entries under exactly these names, however impossible.
///
/// `Builder::append_data` refuses a path containing `..`, which is this crate
/// declining to *write* an archive nobody should publish. The archives this
/// store defends itself against are not written by this crate, so the name goes
/// straight into the header instead — which is the only way to ask the question
/// the store exists to answer.
fn tarball_of_planted_names(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (path, body) in entries {
        let mut header = Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(EntryType::Regular);
        let stored = header.as_gnu_mut().expect("a gnu header");
        stored.name[..path.len()].copy_from_slice(path.as_bytes());
        header.set_cksum();
        builder
            .append(&header, *body)
            .expect("appending to an in-memory archive");
    }
    builder.into_inner().expect("finishing the tar")
}

/// One gzip member holding `tar`, which is what a release archive is.
///
/// `tarball` deliberately stops at the tar, because the tests about gzip
/// members wrap it themselves. Everything else that hands an archive to the
/// store needs this.
fn gzipped(tar: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(tar).expect("gzipping an in-memory tar");
    encoder.finish().expect("finishing the gzip stream")
}

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
    let root = temporary_root();
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
    let root = temporary_root();
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
    let root = temporary_root();
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
fn an_install_that_cannot_read_the_record_unpacks_nothing() {
    // The record is what makes an install true, so a store that cannot read it
    // cannot say whether anything is installed — and an install that cannot be
    // recorded is one that should not start. Asked again under the lock, before
    // a single byte is unpacked, which is why nothing is left behind here: not
    // an executable that was written and taken back out, nothing at all.
    let root = temporary_root();
    let store = ManagedRuntimes::new(root.path());
    let release = release("1.18.31", "package/bin/opencode");
    // A directory where the record goes: neither readable as a record nor
    // renameable onto.
    let record = root.path().join("opencode").join("installed.json");
    nessa_local_storage::create_directory(&record).expect("a directory in the record's place");

    let failure = publish(
        &store,
        &release,
        &archive("package/bin/opencode", b"the runtime"),
    )
    .expect_err("a record that cannot be read fails the publish");

    assert!(
        matches!(failure, StoreFailure::Unreadable(_)),
        "a record that cannot be read is this machine declining to answer: {failure:?}"
    );
    assert!(
        !version_path(root.path()).exists(),
        "an install that never started still made a version directory"
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
fn an_install_that_cannot_be_settled_takes_back_only_what_it_wrote() {
    // The rollback, and the whole of Saurav's first finding. An install that
    // fails after the rename has an executable on the disk that no record
    // names, and reporting "nothing was installed" while a hundred megabytes
    // sits in a directory nothing will look at again is the message being
    // false — on a disk that, in the likeliest cause, is the thing that ran
    // out. So it takes the executable back out.
    //
    // What it must not take out is anybody else's. Another artifact of the same
    // version lives one directory over, and the version directory is shared
    // between them: a rollback that swept it, or that removed a file it did not
    // write, would be one install deleting another's finished runtime.
    let root = temporary_root();
    let store = ManagedRuntimes::new(root.path());
    let theirs = artifact(
        "1.18.31",
        "package/bin/opencode",
        &"b".repeat(64),
        "macos",
        "aarch64",
    );
    let survivor = publish(&store, &theirs, &archive("package/bin/opencode", b"theirs"))
        .expect("the other artifact is unpacked");

    let mine = release("1.18.31", "package/bin/opencode");
    let mut staged = staged(&store, &archive("package/bin/opencode", b"mine"));
    let failure = store
        .publish_durably(&agent(), &mine, &mut staged, |_| {
            Err(std::io::Error::other("the disk is full"))
        })
        .expect_err("an install that cannot be made durable fails");

    assert!(
        matches!(failure.failure(), StoreFailure::Unwritable(_)),
        "a directory that will not sync is this machine's doing: {failure:?}"
    );
    assert!(
        !installed_path(root.path()).exists(),
        "a failed install left a runtime nothing records"
    );
    assert!(
        std::fs::read_dir(artifact_path(root.path()))
            .expect("the empty artifact directory remains")
            .next()
            .is_none(),
        "a failed install left bytes in its artifact directory"
    );
    assert_eq!(
        std::fs::read(&survivor).expect("the other runtime still reads"),
        b"theirs",
        "a failed install removed another artifact's executable"
    );
    assert_eq!(
        store.installed(&agent(), &theirs),
        Ok(Some(survivor)),
        "a failed install left another artifact unusable"
    );
}

#[test]
fn failure_before_record_replacement_confirms_the_previous_runtime() {
    let root = temporary_root();
    let store = ManagedRuntimes::new(root.path());
    let previous = release("1.18.31", "package/bin/opencode");
    let previous_path = publish(
        &store,
        &previous,
        &archive("package/bin/opencode", b"previous"),
    )
    .unwrap();
    let target = artifact(
        "1.19.0",
        "package/bin/opencode",
        &"b".repeat(64),
        "macos",
        "aarch64",
    );
    let mut staged = staged(&store, &archive("package/bin/opencode", b"target"));
    let calls = AtomicUsize::new(0);

    let failure = store
        .publish_durably(&agent(), &target, &mut staged, |_| {
            (calls.fetch_add(1, Ordering::SeqCst) != 0)
                .then_some(())
                .ok_or_else(|| std::io::Error::other("first durability step failed"))
        })
        .unwrap_err();

    assert!(matches!(
        failure.recovery(),
        PublicationRecovery::RolledBack(RollbackChange::Restored(artifact))
            if artifact == &RuntimeArtifact::for_release(&previous)
    ));
    assert_eq!(
        store.installed(&agent(), &previous),
        Ok(Some(previous_path))
    );
}

#[test]
fn failure_after_record_replacement_durably_restores_the_previous_record() {
    let root = temporary_root();
    let store = ManagedRuntimes::new(root.path());
    let previous = release("1.18.31", "package/bin/opencode");
    let previous_path = publish(
        &store,
        &previous,
        &archive("package/bin/opencode", b"previous"),
    )
    .unwrap();
    let target = artifact(
        "1.19.0",
        "package/bin/opencode",
        &"b".repeat(64),
        "macos",
        "aarch64",
    );
    let mut staged = staged(&store, &archive("package/bin/opencode", b"target"));
    let calls = AtomicUsize::new(0);

    let failure = store
        .publish_durably(&agent(), &target, &mut staged, |_| {
            let call = calls.fetch_add(1, Ordering::SeqCst) + 1;
            if call == 6 {
                Err(std::io::Error::other("record directory sync failed"))
            } else {
                Ok(())
            }
        })
        .unwrap_err();

    assert!(matches!(
        failure.recovery(),
        PublicationRecovery::RolledBack(RollbackChange::Restored(artifact))
            if artifact == &RuntimeArtifact::for_release(&previous)
    ));
    assert_eq!(
        store.installed(&agent(), &previous),
        Ok(Some(previous_path))
    );
    assert_eq!(store.installed(&agent(), &target), Ok(None));
}

#[test]
fn failure_after_first_record_replacement_confirms_no_runtime() {
    let root = temporary_root();
    let store = ManagedRuntimes::new(root.path());
    let target = release("1.18.31", "package/bin/opencode");
    let mut staged = staged(&store, &archive("package/bin/opencode", b"target"));
    let calls = AtomicUsize::new(0);

    let failure = store
        .publish_durably(&agent(), &target, &mut staged, |_| {
            let call = calls.fetch_add(1, Ordering::SeqCst) + 1;
            if call == 6 {
                Err(std::io::Error::other("record directory sync failed"))
            } else {
                Ok(())
            }
        })
        .unwrap_err();

    assert!(matches!(
        failure.recovery(),
        PublicationRecovery::RolledBack(RollbackChange::NoInstalledRuntime)
    ));
    assert_eq!(store.installed(&agent(), &target), Ok(None));
}

#[test]
fn withdrawal_failure_is_visible_while_the_restored_runtime_is_confirmed() {
    let root = temporary_root();
    let store = ManagedRuntimes::new(root.path());
    let previous = release("1.18.31", "package/bin/opencode");
    publish(
        &store,
        &previous,
        &archive("package/bin/opencode", b"previous"),
    )
    .unwrap();
    let target = artifact(
        "1.19.0",
        "package/bin/opencode",
        &"b".repeat(64),
        "macos",
        "aarch64",
    );
    let mut staged = staged(&store, &archive("package/bin/opencode", b"target"));
    let calls = AtomicUsize::new(0);

    let failure = store
        .publish_with_recovery(
            &agent(),
            &target,
            &mut staged,
            |_| {
                let call = calls.fetch_add(1, Ordering::SeqCst) + 1;
                if call == 6 {
                    Err(std::io::Error::other("record directory sync failed"))
                } else {
                    Ok(())
                }
            },
            |_| Err(StoreFailure::Unwritable("withdrawal failed".into())),
        )
        .unwrap_err();

    assert!(matches!(
        failure.recovery(),
        PublicationRecovery::Incomplete {
            rollback: Some(RollbackChange::Restored(artifact)),
            cleanup,
        } if artifact == &RuntimeArtifact::for_release(&previous)
            && cleanup.withdrawal().is_some_and(|failure| failure.to_string().contains("withdrawal failed"))
    ));
    assert!(store.installed(&agent(), &previous).unwrap().is_some());
}

#[test]
fn every_cleanup_failure_is_preserved_when_rollback_cannot_be_confirmed() {
    let root = temporary_root();
    let store = ManagedRuntimes::new(root.path());
    let previous = release("1.18.31", "package/bin/opencode");
    publish(
        &store,
        &previous,
        &archive("package/bin/opencode", b"previous"),
    )
    .unwrap();
    let target = artifact(
        "1.19.0",
        "package/bin/opencode",
        &"b".repeat(64),
        "macos",
        "aarch64",
    );
    let mut staged = staged(&store, &archive("package/bin/opencode", b"target"));
    let calls = AtomicUsize::new(0);

    let failure = store
        .publish_with_recovery(
            &agent(),
            &target,
            &mut staged,
            |_| {
                let call = calls.fetch_add(1, Ordering::SeqCst) + 1;
                (call < 6)
                    .then_some(())
                    .ok_or_else(|| std::io::Error::other("durability failed"))
            },
            |_| Err(StoreFailure::Unwritable("withdrawal failed".into())),
        )
        .unwrap_err();

    let PublicationRecovery::Incomplete { rollback, cleanup } = failure.recovery() else {
        panic!("cleanup failure was not retained: {failure:?}");
    };
    assert!(rollback.is_none());
    assert!(cleanup.withdrawal().is_some());
    assert!(cleanup.restoration().is_some());
    assert!(cleanup.confirmation().is_none());
}

#[test]
fn every_directory_an_install_creates_is_made_durable_from_the_inside_out() {
    // A rename survives a crash only once the directory holding it does, and
    // that directory only once the one holding *it* does. Syncing the leaf
    // alone leaves a machine able to come back with a record naming a version
    // directory whose own entry never reached the disk.
    //
    // The expected chain is derived from the published executable rather than
    // written out, so this says "every directory between the runtime and the
    // store's root, innermost first" — the property — rather than repeating the
    // list `settle` walks.
    let root = temporary_root();
    let store = ManagedRuntimes::new(root.path());
    let release = release("1.18.31", "package/bin/opencode");
    let published = publish(
        &store,
        &release,
        &archive("package/bin/opencode", b"the runtime"),
    )
    .expect("the executable is unpacked");

    // Relative to the store's root, which is how every path inside the store is
    // spelled: the root itself is the empty path, there being no component to
    // walk to reach it.
    let mut expected = Vec::new();
    let mut directory = published
        .parent()
        .expect("a runtime sits in a directory")
        .strip_prefix(root.path())
        .expect("the runtime is under the root");
    loop {
        expected.push(directory.to_path_buf());
        if directory == Path::new("") {
            break;
        }
        directory = directory.parent().expect("the runtime is under the root");
    }

    assert_eq!(
        store.created_directories(&agent(), &release).to_vec(),
        expected,
        "the chain made durable is not the chain the install created"
    );
    // Five for the store's own layout — artifact, version, `versions/`, the
    // agent and the root — and two more for the directories the pin's own path
    // needs inside the artifact, which a package has and which are made durable
    // like any other.
    assert_eq!(expected.len(), 7, "the layout grew a level nothing syncs");

    // And that `settle` walks all of it, in that order. Asserted through the
    // durability primitive rather than by inspecting the disk afterwards,
    // because a directory entry that was never synced reads back exactly like
    // one that was: the omission this guards against has no trace to find.
    let synced = RefCell::new(Vec::new());
    store
        .settle(&agent(), &release, |directory| {
            synced.borrow_mut().push(directory.to_path_buf());
            Ok(())
        })
        .expect("a chain that is all there settles");

    // The chain, and then the agent directory once more: that last one is the
    // sync that makes the record's own rename durable, which is the last thing
    // standing between an install and being acknowledged.
    let mut walked = expected.clone();
    walked.push(PathBuf::from("opencode"));
    assert_eq!(synced.into_inner(), walked);
}

#[test]
fn an_install_whose_parent_directories_are_not_durable_is_not_recorded() {
    // The record is what makes an install true, so it comes after every
    // directory between the runtime and the store's root — not after the
    // innermost one. A machine that lost power here has to come back to
    // "nothing installed", not to a record naming a version directory whose own
    // entry never reached the disk.
    let root = temporary_root();
    let store = ManagedRuntimes::new(root.path());
    let release = release("1.18.31", "package/bin/opencode");
    let directory = artifact_directory(root.path());
    write(&directory.join("opencode"), b"the runtime");
    let versions = Path::new("opencode").join("versions");

    // Durable everywhere except the level between the version and the agent,
    // which is the one an install that synced only its leaf would skip.
    let failure = store
        .settle(&agent(), &release, |directory| {
            if directory == versions {
                return Err(std::io::Error::other("this entry did not reach the disk"));
            }
            Ok(())
        })
        .expect_err("a chain with an undurable level is not settled");

    assert!(
        matches!(failure, StoreFailure::Unwritable(_)),
        "a directory that will not sync is this machine's doing: {failure:?}"
    );
    assert_eq!(
        store.installed(&agent(), &release),
        Ok(None),
        "an install whose directories are not all durable was recorded anyway"
    );
}

#[test]
fn one_agent_is_published_one_install_at_a_time() {
    // The exclusion the rollback depends on. Two publications overlapping is
    // how one install's clean-up removes another's finished runtime, so the
    // second holder has to wait rather than proceed.
    //
    // Taken twice through the store's own method, from two handles, because
    // that is what two processes look like from the filesystem's side — and it
    // is also what two `ManagedRuntimes` in one process look like, which an
    // in-memory lock would not have covered.
    let root = temporary_root();
    let store = ManagedRuntimes::new(root.path());
    let held = store
        .hold(&agent())
        .expect("the first install holds the lock");

    let contender = nessa_local_storage::open(
        &root.path().join("opencode").join("install.lock"),
        OpenMode::OpenOrCreate,
    )
    .expect("a second handle on the same lock");
    assert!(
        contender.try_lock().is_err(),
        "a second install was let in while the first was still publishing"
    );

    drop(held);
    assert!(
        contender.try_lock().is_ok(),
        "the lock was not released when the install that held it finished"
    );
}

#[test]
fn an_entry_that_cannot_be_written_out_is_this_machines_doing() {
    // The other half of what `expand` exists for. The sibling loop in the HTTPS
    // client has this test; the unpacker's write branch had the fix and not the
    // test, which is half a guarantee.
    let root = temporary_root();
    let path = root.path().join("staging");
    write(&path, b"");
    let mut readable = std::fs::File::open(&path).expect("a handle that cannot be written");

    let failure = expand(
        &mut b"the runtime".as_slice(),
        &mut readable,
        &ArchivePath::parse("package/bin/opencode").expect("contained path"),
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
    let root = temporary_root();
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
    let root = temporary_root();
    let store = ManagedRuntimes::new(root.path());
    let release = release("1.18.31", "package/bin/opencode");
    // The agent directory has to exist, or writing the record would fail on its
    // own and this would pass without the sync ever being reached.
    nessa_local_storage::create_directory(&root.path().join("opencode"))
        .expect("a private agent directory");

    let failure = store
        .settle(&agent(), &release, nessa_local_storage::sync_directory)
        .expect_err("a directory that is not there is not durable");

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
    let root = temporary_root();
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
        matches!(failure, StoreFailure::IncompleteArchive(_)),
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
        let root = temporary_root();
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
    let root = temporary_root();
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

    let root = temporary_root();
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
        matches!(failure.failure(), StoreFailure::Unwritable(_)),
        "a record that would not write is this machine's doing: {failure:?}"
    );
    assert!(
        !installed.exists(),
        "a reported failure left a runtime behind that the old record now names"
    );
}

#[test]
fn a_different_artifact_of_one_version_is_not_already_installed() {
    // One Opencode version ships as nine archives — two operating systems, two
    // architectures, glibc and musl, with and without AVX2 — and every one of
    // them unpacks a file called `opencode`. A store keyed on the version and
    // that name would answer "already installed" to a pin asking for a
    // different one of the nine, and hand back a binary this machine may not be
    // able to start at all.
    let root = temporary_root();
    let store = ManagedRuntimes::new(root.path());
    let installed = release("1.18.31", "package/bin/opencode");
    publish(
        &store,
        &installed,
        &archive("package/bin/opencode", b"the runtime"),
    )
    .expect("the first artifact is unpacked");

    for (named, other) in [
        (
            "another archive of the same version, for the same platform",
            artifact(
                "1.18.31",
                "package/bin/opencode",
                &"b".repeat(64),
                "macos",
                "aarch64",
            ),
        ),
        (
            "another entry of the same archive, under the same file name",
            artifact(
                "1.18.31",
                "bin/opencode",
                &"a".repeat(64),
                "macos",
                "aarch64",
            ),
        ),
        (
            "a later version of the same archive",
            artifact(
                "1.19.0",
                "package/bin/opencode",
                &"a".repeat(64),
                "macos",
                "aarch64",
            ),
        ),
    ] {
        assert_eq!(
            store.installed(&agent(), &other),
            Ok(None),
            "{named} was reported as already installed"
        );
    }

    assert_eq!(
        store.installed(&agent(), &installed),
        Ok(Some(installed_path(root.path()))),
        "the artifact that is installed stopped being installed"
    );
}

#[test]
fn two_artifacts_of_one_version_do_not_share_a_path() {
    // The other half of the same rule. Answering "not installed" is only useful
    // if installing then puts the second artifact somewhere of its own; keyed
    // by version alone the two would rename onto one path, and whichever ran
    // second would silently replace a binary something may still be running.
    let root = temporary_root();
    let store = ManagedRuntimes::new(root.path());
    let first = release("1.18.31", "package/bin/opencode");
    let second = artifact(
        "1.18.31",
        "package/bin/opencode",
        &"b".repeat(64),
        "macos",
        "aarch64",
    );

    let one = publish(&store, &first, &archive("package/bin/opencode", b"first"))
        .expect("the first artifact is unpacked");
    let two = publish(&store, &second, &archive("package/bin/opencode", b"second"))
        .expect("the second artifact is unpacked");

    assert_ne!(one, two, "two artifacts of one version share a path");
    assert_eq!(
        std::fs::read(&one).expect("the first runtime reads"),
        b"first"
    );
    assert_eq!(
        std::fs::read(&two).expect("the second runtime reads"),
        b"second"
    );
}

#[test]
fn an_artifact_that_is_already_there_is_never_unpacked_again() {
    // The recheck `publish` makes once it holds the lock. The caller asked
    // before downloading, and another install can have finished the very same
    // artifact since; unpacking over it would replace a file something may be
    // running with an identical one, and would leave the rollback unable to
    // tell whose file it was undoing.
    //
    // Proven by handing the second call an archive that does not contain the
    // executable at all: a publish that looked at it could not succeed.
    let root = temporary_root();
    let store = ManagedRuntimes::new(root.path());
    let release = release("1.18.31", "package/bin/opencode");
    let first = publish(
        &store,
        &release,
        &archive("package/bin/opencode", b"the runtime"),
    )
    .expect("the executable is unpacked");

    let again = publish(&store, &release, &archive("package/bin/other", b"nothing"))
        .expect("an artifact that is already installed is handed back");

    assert_eq!(again, first);
    assert_eq!(
        std::fs::read(&first).expect("the installed runtime reads"),
        b"the runtime",
        "the runtime that was already there was unpacked over"
    );
}

#[test]
fn a_publication_waits_for_the_one_already_running() {
    // The lock, asserted through `publish` rather than on its own, because the
    // exclusion is only worth anything if publication actually takes it.
    //
    // The half-second is a floor, not a deadline: the assertion is that the
    // publish has *not* finished, which a slow or loaded machine only makes
    // more true. Nothing here fails because a machine was busy. What it catches
    // is a publish that never waited at all — one of these archives is a few
    // hundred bytes and completes in microseconds when nothing holds it up.
    let root = temporary_root();
    let store = ManagedRuntimes::new(root.path());
    let release = release("1.18.31", "package/bin/opencode");
    let bytes = archive("package/bin/opencode", b"the runtime");
    let held = store
        .hold(&agent())
        .expect("the first install holds the lock");
    let (finished, waiting) = std::sync::mpsc::channel();

    std::thread::scope(|threads| {
        threads.spawn(|| {
            let published = publish(&store, &release, &bytes);
            finished
                .send(published)
                .expect("the test is still listening");
        });

        assert!(
            matches!(
                waiting.recv_timeout(Duration::from_millis(500)),
                Err(RecvTimeoutError::Timeout)
            ),
            "a second install published while the first still held the lock"
        );
        drop(held);
        // Told apart from a timeout, because a panic in the waiting thread
        // drops its sender and arrives here as `Disconnected` — which reported
        // as "the lock was never released" would send the next reader looking
        // at the lock instead of at the panic.
        let published = match waiting.recv_timeout(Duration::from_secs(30)) {
            Ok(published) => published.expect("the executable is unpacked"),
            Err(RecvTimeoutError::Timeout) => {
                panic!("the waiting install did not run once the lock was free")
            }
            Err(RecvTimeoutError::Disconnected) => panic!("the waiting install panicked"),
        };
        assert_eq!(published, installed_path(root.path()));
    });
}

#[test]
fn publication_authority_is_held_until_the_result_is_dropped() {
    let root = temporary_root();
    let store = ManagedRuntimes::new(root.path());
    let first_release = release("1.18.31", "package/bin/opencode");
    let second_release = artifact(
        "1.19.0",
        "package/bin/opencode",
        &"b".repeat(64),
        "macos",
        "aarch64",
    );
    let mut first_archive = staged(&store, &archive("package/bin/opencode", b"first runtime"));
    let first = store
        .publish(&agent(), &first_release, &mut first_archive)
        .expect("the first publication succeeds");
    let second_bytes = archive("package/bin/opencode", b"second runtime");
    let (finished, waiting) = std::sync::mpsc::channel();

    std::thread::scope(|threads| {
        threads.spawn(|| {
            let published = publish(&store, &second_release, &second_bytes);
            finished
                .send(published)
                .expect("the test is still listening");
        });

        assert!(
            matches!(
                waiting.recv_timeout(Duration::from_millis(500)),
                Err(RecvTimeoutError::Timeout)
            ),
            "a second publication completed before the first audit lease ended"
        );
        drop(first);
        match waiting.recv_timeout(Duration::from_secs(30)) {
            Ok(result) => {
                result.expect("the second publication runs when the audit lease is dropped");
            }
            Err(RecvTimeoutError::Timeout) => {
                panic!("the second publication did not run after the audit lease ended")
            }
            Err(RecvTimeoutError::Disconnected) => panic!("the second publication panicked"),
        }
    });
}

#[test]
fn a_pin_that_corrects_itself_about_an_archive_reinstalls_nothing() {
    // The record and the layout have to agree about what identifies an
    // artifact. They key on the version, the digest and the entry; the
    // platform and what the build needs are written down for whoever opens the
    // file, and they are facts about the digest, since one archive is one
    // build.
    //
    // If reuse compared them too, a pin correcting one of them — which is
    // exactly the edit this branch makes, adding `libc` and `requiresAvx2` to
    // every entry — would read as "not installed", rename over the very file
    // the record still names, and, on a settle that failed, take it away
    // again. So a release that differs only there is the artifact already
    // installed, and nothing is unpacked.
    //
    // Proven the only honest way: the second call is handed an archive with no
    // executable in it and a durability primitive that fails. A publish that
    // did any work could not have returned the path.
    let root = temporary_root();
    let store = ManagedRuntimes::new(root.path());
    let installed = release("1.18.31", "package/bin/opencode");
    let published = publish(
        &store,
        &installed,
        &archive("package/bin/opencode", b"the runtime"),
    )
    .expect("the executable is unpacked");

    let corrected = PinnedRelease::new(
        ReleaseVersion::parse("1.18.31").expect("usable version"),
        ReleasePlatform::new("linux", "x86_64").expect("usable platform"),
        ReleaseRequirements::new(Some(Libc::Musl), true),
        ArchiveUrl::parse("https://registry.example/runtime.tgz").expect("a fetchable url"),
        ArchiveSize::parse(ARCHIVE_BYTES).expect("usable archive size"),
        ArchiveDigest::parse(&"a".repeat(64)).expect("usable digest"),
        one_program("package/bin/opencode"),
    )
    .expect("a release whose requirements fit its platform");

    assert_eq!(
        store.installed(&agent(), &corrected),
        Ok(Some(published.clone())),
        "a pin that corrected only what it says about the archive read as uninstalled"
    );

    let mut staged = staged(&store, &archive("package/bin/other", b"nothing"));
    let again = store
        .publish_durably(&agent(), &corrected, &mut staged, |_| {
            Err(std::io::Error::other("this must never be reached"))
        })
        .expect("the artifact already installed is handed back");

    assert_eq!(again.executable(), &published);
    assert_eq!(
        std::fs::read(&published).expect("the installed runtime reads"),
        b"the runtime"
    );
}

#[test]
fn what_a_build_needs_is_written_down_even_though_reuse_does_not_read_it() {
    // Recorded for whoever opens the file — "which of the nine is this?" is
    // not a question a digest answers to a person. Asserted because a field
    // nothing compares is a field that can quietly stop being written.
    let root = temporary_root();
    let store = ManagedRuntimes::new(root.path());
    let musl = PinnedRelease::new(
        ReleaseVersion::parse("1.18.31").expect("usable version"),
        ReleasePlatform::new("linux", "x86_64").expect("usable platform"),
        ReleaseRequirements::new(Some(Libc::Musl), true),
        ArchiveUrl::parse("https://registry.example/runtime.tgz").expect("a fetchable url"),
        ArchiveSize::parse(ARCHIVE_BYTES).expect("usable archive size"),
        ArchiveDigest::parse(&"a".repeat(64)).expect("usable digest"),
        one_program("package/bin/opencode"),
    )
    .expect("a release whose requirements fit its platform");
    publish(
        &store,
        &musl,
        &archive("package/bin/opencode", b"the runtime"),
    )
    .expect("the executable is unpacked");

    let written: serde_json::Value = serde_json::from_slice(
        &std::fs::read(root.path().join("opencode").join("installed.json"))
            .expect("the record reads"),
    )
    .expect("the record is json");

    assert_eq!(written["platform"], "linux-x86_64");
    assert_eq!(written["libc"], "musl");
    assert_eq!(written["requires_avx2"], true);
    assert_eq!(written["digest"], "a".repeat(64));
    assert_eq!(
        written["files"],
        serde_json::json!([{ "path": "package/bin/opencode", "role": "launch" }])
    );
}

// Unix only, because it is the platform where a file can be replaced or
// removed while somebody holds it open. Windows keeps the name for as long as
// the file is open, so there the file that was locked is the file at that path
// by construction.
#[test]
#[cfg(unix)]
fn a_lock_only_counts_while_it_is_the_file_at_that_path() {
    // The lock lives on an open file, not on a name. If something outside
    // Nessa removes or replaces `install.lock` between the open and the lock —
    // a tidy-up, a partial restore, an uninstall script — two installs end up
    // holding two different files and each believes it is alone, which is the
    // race the lock was added to prevent, back again and invisible.
    let root = temporary_root();
    let store = ManagedRuntimes::new(root.path());
    let directory = root.path().join("opencode");
    nessa_local_storage::create_directory(&directory).expect("a private agent directory");
    // The name the store uses, which is relative to its root.
    let relative = std::path::Path::new("opencode/install.lock");
    let path = root.path().join(relative);
    let held = nessa_local_storage::open(&path, OpenMode::OpenOrCreate).expect("a lock file");

    assert_eq!(
        store.same_file(&held, relative),
        Ok(true),
        "a file nothing touched was reported as replaced"
    );

    std::fs::rename(&path, directory.join("moved-aside")).expect("moving the lock file aside");
    assert_eq!(
        store.same_file(&held, relative),
        Ok(false),
        "a lock file that was removed still counted"
    );

    nessa_local_storage::open(&path, OpenMode::CreateNew).expect("a new lock file in its place");
    assert_eq!(
        store.same_file(&held, relative),
        Ok(false),
        "a lock file that was replaced still counted"
    );

    // And a name that no longer resolves to a private file of this user's is
    // not the lock this handle holds either. The comparison is anchored, so a
    // link put there in place of the lock is not followed to whatever it names.
    std::fs::remove_file(&path).expect("clearing the replacement");
    std::os::unix::fs::symlink(directory.join("moved-aside"), &path).expect("a link in its place");
    let refused = store
        .same_file(&held, relative)
        .expect_err("a link standing in for the lock file was followed");
    assert!(
        matches!(refused, StoreFailure::Unwritable(_)),
        "{refused:?}"
    );
}

/// A release that is a whole package rather than one program.
///
/// Codex's shape is the reason this context grew a set of files at all: seven
/// entries, four of them programs, and the three the runtime is not launched as
/// are found *through the directory the launched one sits in* — ripgrep at
/// `../codex-path/rg`, a zsh under `../codex-resources/`. Everything here is
/// about that relationship surviving an install, a rollback and a later check.
mod packages {
    use super::*;

    #[test]
    fn every_file_a_package_names_is_installed_where_the_pin_puts_it() {
        let root = temporary_root();
        let store = ManagedRuntimes::new(root.path());
        let release = package("1.18.31");

        let published = publish(&store, &release, &gzipped(&tarball(&package_entries())))
            .expect("a package is unpacked");

        // The launch is what comes back, and the rest sit around it exactly as
        // the archive had them — which is the whole point: flattened into one
        // directory they would all be present and none of them findable.
        assert_eq!(
            published,
            package_path(root.path(), "package/vendor/bin/codex")
        );
        for (path, body) in package_entries() {
            let installed = package_path(root.path(), path);
            assert_eq!(
                std::fs::read(&installed).unwrap_or_else(|_| panic!("{path} was not installed")),
                body,
                "{path} holds the wrong bytes"
            );
        }
        assert_eq!(store.installed(&agent(), &release), Ok(Some(published)));
    }

    #[test]
    #[cfg(unix)]
    fn a_program_is_installed_runnable_and_a_document_is_not() {
        // The mode comes from the pin's role, never from the archive's own
        // header — `tarball` writes `0o644` for all three. A release that
        // turned a document into a program by flipping a bit in a header it
        // also supplies would change nothing here.
        let root = temporary_root();
        let store = ManagedRuntimes::new(root.path());
        let release = package("1.18.31");

        publish(&store, &release, &gzipped(&tarball(&package_entries())))
            .expect("a package is unpacked");

        let mode = |path: &str| {
            package_path(root.path(), path)
                .metadata()
                .unwrap_or_else(|_| panic!("{path} was not installed"))
                .permissions()
                .mode()
                & 0o777
        };
        assert_eq!(mode("package/vendor/bin/codex"), 0o700, "the launch");
        assert_eq!(mode("package/vendor/codex-path/rg"), 0o700, "the helper");
        assert_eq!(mode("package/package.json"), 0o600, "the document");
    }

    #[test]
    fn a_package_missing_one_file_installs_none_of_them() {
        // A runtime whose ripgrep is absent starts and then cannot search. Four
        // of seven files is worse than none: enough for a later install to
        // rename over, not enough to run.
        let root = temporary_root();
        let store = ManagedRuntimes::new(root.path());
        let release = package("1.18.31");
        let mut without_the_helper = package_entries();
        without_the_helper.retain(|(path, _)| *path != "package/vendor/codex-path/rg");

        let failure = publish(&store, &release, &gzipped(&tarball(&without_the_helper)))
            .expect_err("a package missing a file");

        assert_eq!(
            failure,
            StoreFailure::IncompleteArchive("package/vendor/codex-path/rg".into())
        );
        for (path, _) in package_entries() {
            assert!(
                !package_path(root.path(), path).exists(),
                "{path} survived an install that could not finish"
            );
        }
        assert_eq!(store.installed(&agent(), &release), Ok(None));
        assert!(
            !version_path(root.path()).exists(),
            "a refused package left its directories behind"
        );
    }

    #[test]
    #[cfg(unix)]
    fn a_package_that_cannot_be_settled_takes_back_every_file_it_wrote() {
        // The interrupted install. Everything is unpacked and renamed, and then
        // the step that makes it durable fails — so nothing may be reported as
        // installed and nothing may be left behind for the next install to
        // mistake for its own work.
        let root = temporary_root();
        let store = ManagedRuntimes::new(root.path());
        let release = package("1.18.31");
        let mut staged = staged(&store, &gzipped(&tarball(&package_entries())));

        let failure = store
            .publish_durably(&agent(), &release, &mut staged, |_| {
                Err(std::io::Error::other("the disk gave out"))
            })
            .expect_err("an install that cannot be made durable");

        assert!(matches!(
            failure.failure(),
            StoreFailure::Unwritable(_)
        ));
        for (path, _) in package_entries() {
            assert!(
                !package_path(root.path(), path).exists(),
                "{path} was left behind by an install that reported failure"
            );
        }
        assert_eq!(
            store.installed(&agent(), &release),
            Ok(None),
            "an interrupted install was reported as installed"
        );
        assert!(
            !version_path(root.path()).exists(),
            "an interrupted install left its directories behind"
        );
    }

    #[test]
    fn a_package_whose_helper_was_deleted_is_not_installed() {
        // The record is a note, not evidence, and that has to hold for every
        // file rather than only the one that is launched. A disk cleaner that
        // took the ripgrep away leaves a runtime that starts and cannot work.
        let root = temporary_root();
        let store = ManagedRuntimes::new(root.path());
        let release = package("1.18.31");
        publish(&store, &release, &gzipped(&tarball(&package_entries())))
            .expect("a package is unpacked");

        std::fs::remove_file(package_path(root.path(), "package/vendor/codex-path/rg"))
            .expect("removing the helper");

        assert_eq!(store.installed(&agent(), &release), Ok(None));
    }

    #[test]
    fn a_pin_that_starts_installing_one_more_file_is_not_already_installed() {
        // The same archive, and a pin that has noticed it left a helper out.
        // Answering "already installed" would leave the runtime unable to do
        // its work with nothing saying why — so the record compares every file,
        // not only the version and the digest.
        let root = temporary_root();
        let store = ManagedRuntimes::new(root.path());
        let without = release_installing(
            "1.18.31",
            &"a".repeat(64),
            installing(&[("package/vendor/bin/codex", FileRole::Launch)]),
        );
        publish(&store, &without, &gzipped(&tarball(&package_entries())))
            .expect("one program is unpacked");

        assert_eq!(store.installed(&agent(), &package("1.18.31")), Ok(None));
    }

    #[test]
    fn an_entry_that_would_escape_the_artifact_directory_is_not_unpacked() {
        // The pin's paths decide where bytes land, so an entry naming its way
        // out of the artifact directory matches nothing and is skipped. Said
        // with a real archive rather than only through `ArchivePath`, because
        // the two rules are meant to hold together: the domain refuses such a
        // path in a *pin*, and the unpacker never consults an entry's own name
        // for a destination.
        let root = temporary_root();
        let store = ManagedRuntimes::new(root.path());
        let release = package("1.18.31");
        let mut entries: Vec<(&str, &[u8])> = vec![
            ("../../../escaped", b"planted".as_slice()),
            ("package/../../escaped-too", b"planted".as_slice()),
            ("/escaped-absolute", b"planted".as_slice()),
        ];
        entries.extend(package_entries());

        publish(
            &store,
            &release,
            &gzipped(&tarball_of_planted_names(&entries)),
        )
        .expect("a package is unpacked");

        // Nothing appeared anywhere but under the artifact. The store's root
        // sits inside a temporary directory, so an escape of one or more levels
        // lands somewhere this test can look.
        let temporary = root
            .path()
            .parent()
            .expect("the store root is inside a temporary directory");
        for planted in ["escaped", "escaped-too", "escaped-absolute"] {
            assert!(
                !temporary.join(planted).exists(),
                "{planted} was written outside the artifact directory"
            );
            assert!(
                !root.path().join(planted).exists(),
                "{planted} was written into the store's root"
            );
        }
        // And the files the pin does name are all there, so the escaping
        // entries were skipped rather than the archive refused.
        for (path, body) in package_entries() {
            assert_eq!(
                std::fs::read(package_path(root.path(), path)).expect("an installed file"),
                body
            );
        }
    }

    #[test]
    fn an_entry_the_archive_carries_twice_is_refused() {
        // Two entries under one pinned name are two answers to what is
        // installed there, and which of them won would be decided by nothing
        // but which came first.
        let root = temporary_root();
        let store = ManagedRuntimes::new(root.path());
        let release = package("1.18.31");
        let mut twice = package_entries();
        twice.push(("package/vendor/bin/codex", b"a second codex".as_slice()));

        let failure =
            publish(&store, &release, &gzipped(&tarball(&twice))).expect_err("a repeated entry");

        assert!(
            matches!(failure, StoreFailure::MalformedArchive(_)),
            "{failure:?}"
        );
        assert_eq!(store.installed(&agent(), &release), Ok(None));
    }

    #[test]
    fn an_entry_the_pin_does_not_name_is_left_in_the_archive() {
        // The pin names what is installed, so an archive carrying more than it
        // says installs no more than it says. Codex's package is seven files
        // and Nessa is not obliged to take an eighth because a release added
        // one.
        let root = temporary_root();
        let store = ManagedRuntimes::new(root.path());
        let release = package("1.18.31");
        let mut with_extra = package_entries();
        with_extra.push(("package/vendor/bin/surprise", b"unpinned".as_slice()));

        publish(&store, &release, &gzipped(&tarball(&with_extra))).expect("a package is unpacked");

        assert!(
            !package_path(root.path(), "package/vendor/bin/surprise").exists(),
            "an entry the pin says nothing about was installed"
        );
    }
}
