use super::*;
use crate::agent_install::domain::{FileRole, Libc, ReleaseFile};

fn digest(byte: char) -> ArchiveDigest {
    ArchiveDigest::parse(&std::iter::repeat_n(byte, 64).collect::<String>())
        .expect("a run of one hex character is a digest")
}

fn url() -> ArchiveUrl {
    ArchiveUrl::parse("https://registry.example/runtime.tgz").expect("a fetchable url")
}

fn size() -> ArchiveSize {
    ArchiveSize::parse(46_009_615).expect("the size of a real opencode archive")
}

/// One program and nothing else, which is the shape Opencode has.
fn one_program() -> ReleaseContents {
    ReleaseContents::new(vec![ReleaseFile::new(
        ArchivePath::parse("package/bin/opencode").expect("contained path"),
        FileRole::Launch,
    )])
    .expect("one program is a release")
}

fn release(digest: ArchiveDigest) -> PinnedRelease {
    PinnedRelease::new(
        ReleaseVersion::parse("1.0.0").expect("usable version"),
        ReleasePlatform::new("macos", "aarch64").expect("usable platform"),
        ReleaseRequirements::default(),
        url(),
        size(),
        digest,
        one_program(),
    )
    .expect("a release whose requirements fit its platform")
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
fn an_archive_size_is_a_length_an_archive_could_have() {
    // The fetch is held to this number, so a pin that is wrong in the generous
    // direction is permission to fill somebody's disk — and one that is zero
    // would refuse every download on its first byte.
    assert!(ArchiveSize::parse(1).is_ok());
    assert_eq!(
        ArchiveSize::parse(116_501_639).map(ArchiveSize::bytes),
        Ok(116_501_639)
    );
    assert_eq!(ArchiveSize::parse(0), Err(PinRejected::ArchiveSize(0)));
    let absurd = 8 * 1024 * 1024 * 1024;
    assert_eq!(
        ArchiveSize::parse(absurd),
        Err(PinRejected::ArchiveSize(absurd))
    );
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

/// A machine, for the tests about where a release will run.
fn machine(operating_system: &str, architecture: &str) -> HostPlatform {
    HostPlatform::new(
        ReleasePlatform::new(operating_system, architecture).expect("usable platform"),
        Some(Libc::Gnu),
        true,
    )
}

#[test]
fn a_release_runs_only_on_the_platform_it_names() {
    let release = release(digest('a'));

    assert!(release.runs_on(&machine("macos", "aarch64")));
    assert!(!release.runs_on(&machine("macos", "x86_64")));
    assert!(!release.runs_on(&machine("linux", "aarch64")));
}

#[test]
fn a_release_also_runs_only_where_what_it_needs_is_there() {
    // The platform is not the whole of it. Two builds can name one platform and
    // differ only in what they ask of the machine, and the one whose needs are
    // not met is the one that does not start.
    let release = PinnedRelease::new(
        ReleaseVersion::parse("1.0.0").expect("usable version"),
        ReleasePlatform::new("linux", "x86_64").expect("usable platform"),
        ReleaseRequirements::new(Some(Libc::Musl), true),
        url(),
        size(),
        digest('a'),
        one_program(),
    )
    .expect("a release whose requirements fit its platform");
    let linux = |libc, avx2| {
        HostPlatform::new(
            ReleasePlatform::new("linux", "x86_64").expect("usable platform"),
            libc,
            avx2,
        )
    };

    assert!(release.runs_on(&linux(Some(Libc::Musl), true)));
    assert!(!release.runs_on(&linux(Some(Libc::Musl), false)));
    assert!(!release.runs_on(&linux(Some(Libc::Gnu), true)));
    assert!(!release.runs_on(&linux(None, true)));
}

/// A release whose requirements and platform disagree is refused at assembly.
///
/// The rule this holds is the one thing about a pin that no single value object
/// can see: `ReleaseRequirements` and `ReleasePlatform` are each perfectly
/// valid alone and together can describe a build that cannot exist. Tested
/// here, without JSON, because the pin file is one way to assemble a release
/// and the rule has to hold for every other way too.
#[test]
fn a_build_that_could_not_exist_is_not_a_release() {
    let assemble = |operating_system: &str, architecture: &str, requirements| {
        PinnedRelease::new(
            ReleaseVersion::parse("1.0.0").expect("usable version"),
            ReleasePlatform::new(operating_system, architecture).expect("usable platform"),
            requirements,
            url(),
            size(),
            digest('a'),
            one_program(),
        )
    };

    // A Linux build naming no C library. `satisfies` reads an unnamed library
    // as "nothing required", so this would be offered to every Linux machine
    // and fail in the loader on half of them — which is the whole failure this
    // context exists to prevent.
    assert!(matches!(
        assemble("linux", "x86_64", ReleaseRequirements::new(None, false)),
        Err(PinRejected::Requirements(_))
    ));

    // AVX2 asked of an architecture that has no such instruction set. Not
    // dangerous, but unreachable: nothing would ever satisfy it, so the pin
    // would install on no machine at all and say nothing about why.
    assert!(matches!(
        assemble(
            "linux",
            "aarch64",
            ReleaseRequirements::new(Some(Libc::Gnu), true)
        ),
        Err(PinRejected::Requirements(_))
    ));
    assert!(matches!(
        assemble("macos", "aarch64", ReleaseRequirements::new(None, true)),
        Err(PinRejected::Requirements(_))
    ));

    // A macOS build naming a C library: the mirror of the first case, failing
    // the other way. No Apple target is built against musl or glibc, so
    // `host_libc` answers `None` on every macOS machine there is and this
    // release would be offered to none of them — the same "a requirement
    // nothing can meet is a typo" fault as the AVX2 case above, and the one
    // that is silent rather than loud.
    for named in [Libc::Gnu, Libc::Musl] {
        assert!(matches!(
            assemble(
                "macos",
                "x86_64",
                ReleaseRequirements::new(Some(named), false)
            ),
            Err(PinRejected::Requirements(_))
        ));
    }

    // And the builds that do exist. macOS has one C library, so naming none
    // there is the truth rather than an omission; 32-bit x86 processors have
    // AVX2 too, so the check is about the instruction set and not about the
    // one architecture Nessa happens to pin for.
    for ok in [
        assemble(
            "linux",
            "x86_64",
            ReleaseRequirements::new(Some(Libc::Musl), true),
        ),
        assemble(
            "linux",
            "aarch64",
            ReleaseRequirements::new(Some(Libc::Gnu), false),
        ),
        assemble("macos", "aarch64", ReleaseRequirements::default()),
        assemble("macos", "x86_64", ReleaseRequirements::new(None, true)),
        assemble("x86", "x86", ReleaseRequirements::new(None, true)),
        // Windows naming `gnu` is a release some build could match: a
        // `*-windows-gnu` Nessa reports `gnu` for MinGW. The rule above is
        // about macOS by name rather than about "not Linux", so that a pin
        // nobody has written yet is not refused on a guess about what the
        // word will mean there.
        assemble(
            "windows",
            "x86_64",
            ReleaseRequirements::new(Some(Libc::Gnu), false),
        ),
        assemble("windows", "x86_64", ReleaseRequirements::default()),
    ] {
        assert!(ok.is_ok(), "{ok:?} is a build that exists");
    }
}
