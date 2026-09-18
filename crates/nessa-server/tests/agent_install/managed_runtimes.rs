use super::*;
use crate::agent_install::domain::{ArchivePath, ReleasePlatform};
use std::io::Write;

const AGENT: &str = "opencode";

/// The SHA-256 of the three bytes `abc`, which is the standard test vector.
/// Written out rather than computed so the test would catch a hasher that
/// hashed something else and agreed with itself.
const ABC_SHA256: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

/// A gzip tar holding one file at `path`, built in memory.
fn archive(path: &str, body: &[u8]) -> Vec<u8> {
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::fast(),
    ));
    let mut header = tar::Header::new_gnu();
    header.set_size(body.len() as u64);
    header.set_mode(0o644);
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
        "https://registry.example/runtime.tgz",
        ArchiveDigest::parse(&"a".repeat(64)).expect("usable digest"),
        ArchivePath::parse(executable).expect("contained path"),
    )
    .expect("well formed release")
}

fn write(path: &std::path::Path, bytes: &[u8]) {
    let mut file = std::fs::File::create(path).expect("writing a test archive");
    file.write_all(bytes).expect("writing a test archive");
}

#[test]
fn nothing_recorded_is_nothing_installed() {
    let root = tempfile::tempdir().expect("temporary root");
    let store = ManagedRuntimes::new(root.path());
    assert_eq!(store.installed(AGENT), Ok(None));
}

#[test]
fn a_published_runtime_is_reported_as_installed() {
    let root = tempfile::tempdir().expect("temporary root");
    let store = ManagedRuntimes::new(root.path());
    let scratch = store.scratch(AGENT).expect("a scratch path");
    write(&scratch, &archive("package/bin/opencode", b"binary"));

    let published = store
        .publish(AGENT, &release("1.18.31", "package/bin/opencode"), &scratch)
        .expect("the executable is unpacked");

    assert!(published.is_file());
    assert_eq!(
        std::fs::read(&published).expect("reading the published runtime"),
        b"binary"
    );
    let installed = store
        .installed(AGENT)
        .expect("the store can be read")
        .expect("something is installed");
    assert_eq!(installed.version.as_str(), "1.18.31");
    assert_eq!(installed.executable, published);
}

#[test]
fn a_published_runtime_is_executable() {
    let root = tempfile::tempdir().expect("temporary root");
    let store = ManagedRuntimes::new(root.path());
    let scratch = store.scratch(AGENT).expect("a scratch path");
    // The archive says 0o644. What matters is that the installed file can be
    // launched regardless of what the archive said about it.
    write(&scratch, &archive("package/bin/opencode", b"binary"));

    let published = store
        .publish(AGENT, &release("1.18.31", "package/bin/opencode"), &scratch)
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
    let scratch = store.scratch(AGENT).expect("a scratch path");
    write(&scratch, &archive("package/bin/opencode", b"binary"));
    let published = store
        .publish(AGENT, &release("1.18.31", "package/bin/opencode"), &scratch)
        .expect("the executable is unpacked");

    std::fs::remove_file(&published).expect("removing the published runtime");

    assert_eq!(store.installed(AGENT), Ok(None));
}

#[test]
fn an_archive_without_the_pinned_executable_is_named_as_such() {
    let root = tempfile::tempdir().expect("temporary root");
    let store = ManagedRuntimes::new(root.path());
    let scratch = store.scratch(AGENT).expect("a scratch path");
    write(&scratch, &archive("package/bin/somethingelse", b"binary"));

    assert_eq!(
        store.publish(AGENT, &release("1.18.31", "package/bin/opencode"), &scratch),
        Err(StoreFailure::MissingExecutable(
            "package/bin/opencode".into()
        ))
    );
    assert_eq!(
        store.installed(AGENT),
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
    let scratch = store.scratch(AGENT).expect("a scratch path");
    write(&scratch, &archive("./package/bin/opencode", b"binary"));

    assert!(store
        .publish(AGENT, &release("1.18.31", "package/bin/opencode"), &scratch)
        .is_ok());
}

#[test]
fn a_malformed_archive_is_reported_as_malformed() {
    let root = tempfile::tempdir().expect("temporary root");
    let store = ManagedRuntimes::new(root.path());
    let scratch = store.scratch(AGENT).expect("a scratch path");
    write(&scratch, b"this is not a gzip tar");

    match store.publish(AGENT, &release("1.18.31", "package/bin/opencode"), &scratch) {
        Err(StoreFailure::MalformedArchive(_)) => {}
        other => panic!("expected a malformed archive, got {other:?}"),
    }
}

#[test]
fn an_archive_entry_cannot_direct_the_write() {
    // The executable lands where the pin says, computed from the agent and the
    // version. A tar entry that calls itself something else is matched against
    // the pin and otherwise has no say in the destination.
    let root = tempfile::tempdir().expect("temporary root");
    let store = ManagedRuntimes::new(root.path());
    let scratch = store.scratch(AGENT).expect("a scratch path");
    write(&scratch, &archive("package/bin/opencode", b"binary"));

    let published = store
        .publish(AGENT, &release("1.18.31", "package/bin/opencode"), &scratch)
        .expect("the executable is unpacked");

    assert!(
        published.starts_with(root.path()),
        "{} escaped the runtime root",
        published.display()
    );
    assert_eq!(
        published,
        root.path().join(AGENT).join("1.18.31").join("opencode")
    );
}

#[test]
fn versions_are_installed_beside_each_other() {
    // A pin that moves must not half-overwrite a binary something may still be
    // running.
    let root = tempfile::tempdir().expect("temporary root");
    let store = ManagedRuntimes::new(root.path());

    let scratch = store.scratch(AGENT).expect("a scratch path");
    write(&scratch, &archive("package/bin/opencode", b"old"));
    let old = store
        .publish(AGENT, &release("1.17.0", "package/bin/opencode"), &scratch)
        .expect("the older runtime is unpacked");

    write(&scratch, &archive("package/bin/opencode", b"new"));
    let new = store
        .publish(AGENT, &release("1.18.31", "package/bin/opencode"), &scratch)
        .expect("the newer runtime is unpacked");

    assert_ne!(old, new);
    assert_eq!(
        std::fs::read(&old).expect("the older runtime survives"),
        b"old"
    );
    assert_eq!(
        store
            .installed(AGENT)
            .expect("the store can be read")
            .expect("something is installed")
            .version
            .as_str(),
        "1.18.31"
    );
}

#[test]
fn a_digest_is_the_sha256_of_the_file() {
    let root = tempfile::tempdir().expect("temporary root");
    let store = ManagedRuntimes::new(root.path());
    let path = root.path().join("subject");
    write(&path, b"abc");

    assert_eq!(
        store.digest(&path).map(|d| d.as_str().to_owned()),
        Ok(ABC_SHA256.to_owned())
    );
}

#[test]
fn a_digest_of_something_larger_than_the_read_buffer_is_still_right() {
    // The hasher reads in chunks. A file of exactly one chunk, and one of more
    // than one, take different paths through that loop.
    let root = tempfile::tempdir().expect("temporary root");
    let store = ManagedRuntimes::new(root.path());
    let large = root.path().join("large");
    let body = vec![7u8; 64 * 1024 * 3 + 17];
    write(&large, &body);

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
        store.digest(&large).map(|d| d.as_str().to_owned()),
        Ok(expected)
    );
}

#[test]
fn discarding_an_archive_that_is_not_there_is_survivable() {
    let root = tempfile::tempdir().expect("temporary root");
    let store = ManagedRuntimes::new(root.path());
    store.discard(&root.path().join("never-existed"));
}

#[test]
fn a_discarded_archive_is_gone() {
    let root = tempfile::tempdir().expect("temporary root");
    let store = ManagedRuntimes::new(root.path());
    let scratch = store.scratch(AGENT).expect("a scratch path");
    write(&scratch, b"downloaded");

    store.discard(&scratch);

    assert!(!scratch.exists());
}

#[test]
fn an_agent_name_that_could_be_a_path_is_refused() {
    // The name becomes a directory under the runtime root. A caller that could
    // pass `..` or an absolute path could point the whole store — the
    // executable, the record, the scratch download — anywhere the process can
    // write.
    let root = tempfile::tempdir().expect("temporary root");
    let store = ManagedRuntimes::new(root.path());

    for hostile in [
        "",
        "..",
        ".",
        "../escape",
        "/etc",
        "a/b",
        "a\\b",
        "Opencode",
        "open code",
    ] {
        assert!(
            matches!(store.installed(hostile), Err(StoreFailure::Unwritable(_))),
            "installed({hostile:?}) should refuse the name"
        );
        assert!(
            matches!(store.scratch(hostile), Err(StoreFailure::Unwritable(_))),
            "scratch({hostile:?}) should refuse the name"
        );
        assert!(
            matches!(
                store.publish(
                    hostile,
                    &release("1.0.0", "package/bin/opencode"),
                    root.path()
                ),
                Err(StoreFailure::Unwritable(_))
            ),
            "publish({hostile:?}) should refuse the name"
        );
    }
}

#[test]
fn an_ordinary_agent_name_is_accepted() {
    let root = tempfile::tempdir().expect("temporary root");
    let store = ManagedRuntimes::new(root.path());
    for name in ["opencode", "claude", "codex", "some-agent2"] {
        assert_eq!(store.installed(name), Ok(None), "{name} should be nameable");
    }
}
