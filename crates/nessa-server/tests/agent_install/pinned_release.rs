use super::*;

fn digest(byte: char) -> ArchiveDigest {
    ArchiveDigest::parse(&std::iter::repeat_n(byte, 64).collect::<String>())
        .expect("a run of one hex character is a digest")
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
    assert!(ReleaseVersion::parse("1.18.31").is_ok());
    for wrong in ["", ".", "..", "1.0/2", "1.0\\2", "1 0", "1.0\n"] {
        assert_eq!(
            ReleaseVersion::parse(wrong),
            Err(PinRejected::Version(wrong.to_string())),
            "{wrong:?} cannot be a directory name"
        );
    }
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
    ] {
        assert_eq!(
            ArchivePath::parse(escape),
            Err(PinRejected::ExecutablePath(escape.to_string())),
            "{escape:?} escapes the archive"
        );
    }
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
fn an_archive_must_be_served_over_https() {
    let build = |url: &str| {
        PinnedRelease::new(
            ReleaseVersion::parse("1.0.0").expect("usable version"),
            ReleasePlatform::new("macos", "aarch64").expect("usable platform"),
            url,
            digest('a'),
            ArchivePath::parse("package/bin/opencode").expect("contained path"),
        )
    };
    assert!(build("https://registry.example/runtime.tgz").is_ok());
    for insecure in [
        "http://registry.example/runtime.tgz",
        "file:///tmp/runtime.tgz",
        "registry.example/runtime.tgz",
        "HTTPS://registry.example/runtime.tgz",
    ] {
        assert_eq!(
            build(insecure),
            Err(PinRejected::ArchiveUrl(insecure.to_string())),
            "{insecure:?} is not https"
        );
    }
}

#[test]
fn a_release_accepts_only_the_digest_it_pinned() {
    let release = PinnedRelease::new(
        ReleaseVersion::parse("1.0.0").expect("usable version"),
        ReleasePlatform::new("macos", "aarch64").expect("usable platform"),
        "https://registry.example/runtime.tgz",
        digest('a'),
        ArchivePath::parse("package/bin/opencode").expect("contained path"),
    )
    .expect("well formed release");

    assert_eq!(release.accept(&digest('a')), Ok(()));
    assert_eq!(
        release.accept(&digest('b')),
        Err(ArchiveRejected {
            expected: digest('a'),
            actual: digest('b'),
        })
    );
}

#[test]
fn a_rejection_names_both_digests() {
    // The message is what a person sees when a download does not match. It has
    // to say what arrived as well as what was wanted, or there is nothing to
    // act on.
    let rejection = ArchiveRejected {
        expected: digest('a'),
        actual: digest('b'),
    };
    let message = rejection.to_string();
    assert!(message.contains(digest('a').as_str()));
    assert!(message.contains(digest('b').as_str()));
}

#[test]
fn a_release_runs_only_on_the_platform_it_names() {
    let release = PinnedRelease::new(
        ReleaseVersion::parse("1.0.0").expect("usable version"),
        ReleasePlatform::new("macos", "aarch64").expect("usable platform"),
        "https://registry.example/runtime.tgz",
        digest('a'),
        ArchivePath::parse("package/bin/opencode").expect("contained path"),
    )
    .expect("well formed release");

    assert!(release.runs_on(&ReleasePlatform::new("macos", "aarch64").expect("usable platform")));
    assert!(!release.runs_on(&ReleasePlatform::new("macos", "x86_64").expect("usable platform")));
    assert!(!release.runs_on(&ReleasePlatform::new("linux", "aarch64").expect("usable platform")));
}
