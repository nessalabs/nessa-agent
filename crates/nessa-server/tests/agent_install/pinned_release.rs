use super::*;

fn digest(byte: char) -> ArchiveDigest {
    ArchiveDigest::parse(&std::iter::repeat_n(byte, 64).collect::<String>())
        .expect("a run of one hex character is a digest")
}

fn url() -> ArchiveUrl {
    ArchiveUrl::parse("https://registry.example/runtime.tgz").expect("a fetchable url")
}

fn release(digest: ArchiveDigest) -> PinnedRelease {
    PinnedRelease::new(
        ReleaseVersion::parse("1.0.0").expect("usable version"),
        ReleasePlatform::new("macos", "aarch64").expect("usable platform"),
        url(),
        digest,
        ArchivePath::parse("package/bin/opencode").expect("contained path"),
    )
}

#[test]
fn a_digest_is_sixty_four_lowercase_hex_characters() {
    assert!(ArchiveDigest::parse(&"a".repeat(64)).is_ok());
    assert!(ArchiveDigest::parse(&"0".repeat(64)).is_ok());
    for wrong in [
        "".to_string(),
        "a".repeat(63),
        "a".repeat(65),
        "A".repeat(64),
        "g".repeat(64),
        format!("{} ", "a".repeat(63)),
    ] {
        assert_eq!(
            ArchiveDigest::parse(&wrong),
            Err(PinRejected::Digest(wrong.clone())),
            "{wrong:?} is not a digest"
        );
    }
}

#[test]
fn an_uppercase_digest_is_refused_rather_than_normalised() {
    // Accepting both spellings would mean every later comparison has to
    // remember to be case-insensitive. One spelling keeps `==` correct.
    let upper = "A".repeat(64);
    assert!(ArchiveDigest::parse(&upper).is_err());
}

#[test]
fn a_version_must_also_be_a_directory_name() {
    for usable in ["1.18.31", "1.0.0-beta.2", "2024.1+build_7"] {
        assert!(
            ReleaseVersion::parse(usable).is_ok(),
            "{usable:?} is a version"
        );
    }
    for wrong in [
        "", ".", "..", "1.0/2", "1.0\\2", "1 0", "1.0\n",
        // A NUL is not whitespace and is ASCII, so it takes the closed alphabet
        // rather than a list of exclusions to keep it out. Reaching the
        // filesystem with one would report a pin fault as a machine that could
        // not write.
        "1.0\0", "\u{1}", "\u{7f}",
    ] {
        assert_eq!(
            ReleaseVersion::parse(wrong),
            Err(PinRejected::Version(wrong.to_string())),
            "{wrong:?} cannot be a directory name"
        );
    }
}

#[test]
fn a_version_must_be_a_directory_name_on_every_platform_too() {
    // Each of these is a legal file name on this machine and trouble on
    // another, which is why the rule is here rather than in the adapter that
    // happens to run on one platform.
    let elsewhere = [
        // Drive-relative on Windows: joining it replaces the directory it was
        // meant to sit inside.
        "c:", // Windows reserves these in every directory and with any extension.
        "con", "nul", "com1", "lpt9",
        // Windows drops a trailing dot rather than storing it.
        "1.0.",
        // macOS and Windows would give this and `1.0-beta` one directory, and
        // two versions sharing a directory is the half-overwrite that
        // installing under a version exists to avoid.
        "1.0-Beta",
    ];
    for wrong in elsewhere {
        assert_eq!(
            ReleaseVersion::parse(wrong),
            Err(PinRejected::Version(wrong.to_string())),
            "{wrong:?} cannot be a directory name everywhere"
        );
    }
    let long = "1.".repeat(40);
    assert!(
        ReleaseVersion::parse(&long).is_err(),
        "a version longer than a path component is not a version"
    );
}

#[test]
fn a_path_inside_an_archive_may_not_escape_it() {
    assert!(ArchivePath::parse("package/bin/opencode").is_ok());
    for escape in [
        "",
        "/etc/passwd",
        "\\windows\\system32",
        "package/../../etc/passwd",
        "../outside",
        "package//bin",
        "package/./bin",
        "package/bin/\0",
        // One legal Unix filename, and a drive-relative path on Windows.
        "c:evil",
        "C:/Windows/Temp/evil",
        // Refused rather than treated as a separator, so that this type, the
        // unpacker and the installed file's name cannot disagree about where
        // the segments divide.
        "package\\bin\\opencode",
    ] {
        assert_eq!(
            ArchivePath::parse(escape),
            Err(PinRejected::ExecutablePath(escape.to_string())),
            "{escape:?} escapes the archive"
        );
    }
}

#[test]
fn the_last_segment_is_what_the_file_is_called() {
    let path = ArchivePath::parse("package/bin/opencode").expect("contained path");
    assert_eq!(path.file_name(), "opencode");
    let bare = ArchivePath::parse("opencode").expect("contained path");
    assert_eq!(bare.file_name(), "opencode");
}

#[test]
fn a_platform_token_is_a_plain_lowercase_identifier() {
    assert!(ReleasePlatform::new("macos", "aarch64").is_ok());
    assert!(ReleasePlatform::new("linux", "x86_64").is_ok());
    assert_eq!(
        ReleasePlatform::new("macOS", "aarch64"),
        Err(PinRejected::Platform("macOS".into()))
    );
    assert_eq!(
        ReleasePlatform::new("macos", ""),
        Err(PinRejected::Platform(String::new()))
    );
}

#[test]
fn an_archive_must_be_named_by_an_https_url() {
    assert!(ArchiveUrl::parse("https://registry.example/runtime.tgz").is_ok());
    for unusable in [
        "http://registry.example/runtime.tgz",
        "file:///tmp/runtime.tgz",
        "registry.example/runtime.tgz",
        // A prefix test passes this and it names nothing at all, which is the
        // reason the URL is parsed rather than matched on.
        "https://",
        // Credentials belong nowhere near a value checked into the repository.
        "https://user:secret@registry.example/runtime.tgz",
    ] {
        assert_eq!(
            ArchiveUrl::parse(unusable),
            Err(PinRejected::ArchiveUrl(unusable.to_string())),
            "{unusable:?} is not a url an archive may be fetched from"
        );
    }
}

#[test]
fn an_uppercase_scheme_is_still_https() {
    // Parsed, not matched: `HTTPS://` is the same scheme written differently,
    // and the URL comes back spelled the one way everything downstream reads.
    let url = ArchiveUrl::parse("HTTPS://registry.example/runtime.tgz")
        .expect("an uppercase scheme is https");
    assert!(url.as_str().starts_with("https://"));
}

#[test]
fn a_release_accepts_only_the_digest_it_pinned() {
    let release = release(digest('a'));

    assert_eq!(release.accept(&digest('a')), Ok(()));
    let rejection = release
        .accept(&digest('b'))
        .expect_err("another archive is not the pinned one");
    assert_eq!(rejection.expected(), &digest('a'));
    assert_eq!(rejection.actual(), &digest('b'));
}

#[test]
fn a_rejection_names_both_digests() {
    // The message is what a person sees when a download does not match. It has
    // to say what arrived as well as what was wanted, or there is nothing to
    // act on.
    let rejection = release(digest('a'))
        .accept(&digest('b'))
        .expect_err("another archive is not the pinned one");
    let message = rejection.to_string();
    assert!(message.contains(digest('a').as_str()));
    assert!(message.contains(digest('b').as_str()));
}

#[test]
fn a_release_runs_only_on_the_platform_it_names() {
    let release = release(digest('a'));

    assert!(release.runs_on(&ReleasePlatform::new("macos", "aarch64").expect("usable platform")));
    assert!(!release.runs_on(&ReleasePlatform::new("macos", "x86_64").expect("usable platform")));
    assert!(!release.runs_on(&ReleasePlatform::new("linux", "aarch64").expect("usable platform")));
}
