use super::*;

/// Every platform the pin file is expected to cover.
///
/// Named here rather than derived from the file, so that dropping a platform
/// from the pin — which would silently stop offering Opencode to everyone on
/// it — fails this test instead of passing quietly.
const COVERED: &[(&str, &str)] = &[
    ("macos", "aarch64"),
    ("macos", "x86_64"),
    ("linux", "aarch64"),
    ("linux", "x86_64"),
];

#[test]
fn the_compiled_in_pins_are_valid() {
    // The file is generated, but it is also checked in and editable. Parsing it
    // through the domain's rules here means a hand-edited digest, an http URL,
    // or a path that escapes the archive is a failing test rather than a
    // surprise at install time on someone's machine.
    let releases = releases_for("opencode").expect("the pinned releases parse");
    assert!(!releases.is_empty(), "opencode has at least one pin");
}

#[test]
fn every_supported_platform_is_pinned() {
    let releases = releases_for("opencode").expect("the pinned releases parse");
    for (operating_system, architecture) in COVERED {
        let platform =
            ReleasePlatform::new(operating_system, architecture).expect("usable platform");
        assert!(
            releases.iter().any(|release| release.runs_on(&platform)),
            "no opencode release pinned for {platform}"
        );
    }
}

#[test]
fn every_pin_names_one_version() {
    // One version across platforms is what makes "the version Nessa tested" a
    // single answer rather than four.
    let releases = releases_for("opencode").expect("the pinned releases parse");
    let versions: std::collections::BTreeSet<_> = releases
        .iter()
        .map(|release| release.version().as_str())
        .collect();
    assert_eq!(
        versions.len(),
        1,
        "the pinned platforms disagree about the version: {versions:?}"
    );
}

#[test]
fn every_pin_is_fetched_over_https() {
    let releases = releases_for("opencode").expect("the pinned releases parse");
    for release in releases {
        assert!(
            release.archive_url().starts_with("https://"),
            "{} is not https",
            release.archive_url()
        );
    }
}

#[test]
fn each_platform_is_pinned_once() {
    // Two pins for one platform would make which archive gets installed depend
    // on the order of the file.
    let releases = releases_for("opencode").expect("the pinned releases parse");
    for (operating_system, architecture) in COVERED {
        let platform =
            ReleasePlatform::new(operating_system, architecture).expect("usable platform");
        let matching = releases
            .iter()
            .filter(|release| release.runs_on(&platform))
            .count();
        assert_eq!(matching, 1, "{platform} is pinned {matching} times");
    }
}

#[test]
fn each_platform_has_its_own_archive() {
    // Four platforms sharing one digest would mean the generator hashed the
    // same download four times.
    let releases = releases_for("opencode").expect("the pinned releases parse");
    let urls: std::collections::BTreeSet<_> = releases
        .iter()
        .map(|release| release.archive_url())
        .collect();
    assert_eq!(urls.len(), releases.len(), "two pins share an archive");
}

#[test]
fn an_agent_with_no_pins_has_no_releases() {
    // Claude and Codex are not installed by Nessa. Asking is not an error; the
    // answer is that there is nothing to install.
    assert_eq!(releases_for("claude"), Ok(Vec::new()));
}

#[test]
fn a_release_is_found_by_platform() {
    let platform = ReleasePlatform::new("macos", "aarch64").expect("usable platform");
    let release = release_for("opencode", &platform)
        .expect("the pinned releases parse")
        .expect("macos arm64 is pinned");
    assert!(release.runs_on(&platform));
}

#[test]
fn an_unpinned_platform_has_no_release() {
    let platform = ReleasePlatform::new("windows", "x86_64").expect("usable platform");
    assert_eq!(release_for("opencode", &platform), Ok(None));
}

#[test]
fn the_host_platform_is_nameable() {
    // `host_platform` unwraps, on the grounds that the compiler's own tokens
    // are always plain identifiers. This test is what makes that true on
    // whatever target the suite is built for rather than only on the ones
    // thought of when it was written.
    let platform = host_platform();
    assert!(!platform.operating_system().is_empty());
    assert!(!platform.architecture().is_empty());
}
