use super::*;
use std::collections::BTreeSet;

use crate::agent_install::domain::{AgentName, PinRejected};

/// One machine, written the way the pin file's coverage reads best.
fn machine(
    operating_system: &str,
    architecture: &str,
    libc: Option<Libc>,
    avx2: bool,
) -> HostPlatform {
    HostPlatform::new(
        ReleasePlatform::new(operating_system, architecture).expect("usable platform"),
        libc,
        avx2,
    )
}

fn opencode() -> AgentName {
    AgentName::parse("opencode").expect("a plain agent name")
}

/// Every machine the pin file is expected to have a build for.
///
/// Named here rather than derived from the file, so that dropping a build from
/// the pin — which would silently stop offering Opencode to everyone on that
/// kind of machine — fails this test instead of passing quietly.
///
/// Machines rather than platforms, because a platform is not what a build is
/// chosen for. Linux x86-64 appears four times: the two C libraries, each on a
/// processor with AVX2 and on one without. Dropping the baseline build would
/// leave the first list unchanged and every older x86-64 machine with nothing
/// that starts.
const COVERED: &[(&str, &str, Option<Libc>, bool)] = &[
    ("macos", "aarch64", None, false),
    ("macos", "x86_64", None, false),
    ("macos", "x86_64", None, true),
    ("linux", "aarch64", Some(Libc::Gnu), false),
    ("linux", "aarch64", Some(Libc::Musl), false),
    ("linux", "x86_64", Some(Libc::Gnu), false),
    ("linux", "x86_64", Some(Libc::Gnu), true),
    ("linux", "x86_64", Some(Libc::Musl), false),
    ("linux", "x86_64", Some(Libc::Musl), true),
];

#[test]
fn the_compiled_in_pins_are_valid() {
    // The file is generated, but it is also checked in and editable. Parsing it
    // through the domain's rules here means a hand-edited digest, an http URL,
    // or a path that escapes the archive is a failing test rather than a
    // surprise at install time on someone's machine.
    let releases = releases_for(&opencode()).expect("the pinned releases parse");
    assert!(!releases.is_empty(), "opencode has at least one pin");
}

#[test]
fn every_supported_machine_has_a_build() {
    let releases = releases_for(&opencode()).expect("the pinned releases parse");
    for (operating_system, architecture, libc, avx2) in COVERED {
        let host = machine(operating_system, architecture, *libc, *avx2);
        assert!(
            releases.iter().any(|release| release.runs_on(&host)),
            "no opencode release pinned for {host}"
        );
    }
}

#[test]
fn a_machine_is_never_offered_a_build_it_cannot_start() {
    // The other half of the coverage question, and the half that matters most:
    // a build offered to a machine that cannot run it does not fail politely.
    // A glibc binary on a musl-only machine dies in the loader, and an AVX2
    // binary on a processor without it dies on an illegal instruction, both
    // after Nessa has told somebody their runtime is ready.
    let releases = releases_for(&opencode()).expect("the pinned releases parse");
    for (operating_system, architecture, libc, avx2) in COVERED {
        let host = machine(operating_system, architecture, *libc, *avx2);
        for release in releases.iter().filter(|release| release.runs_on(&host)) {
            let needs = release.requirements();
            assert!(
                !needs.avx2() || *avx2,
                "{host} was offered a build that needs avx2"
            );
            assert!(
                needs.libc().is_none() || needs.libc() == *libc,
                "{host} was offered a {needs} build"
            );
        }
    }
}

#[test]
fn every_pin_names_one_version() {
    // One version across platforms is what makes "the version Nessa tested" a
    // single answer rather than four.
    let releases = releases_for(&opencode()).expect("the pinned releases parse");
    let versions: BTreeSet<_> = releases
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
    let releases = releases_for(&opencode()).expect("the pinned releases parse");
    for release in releases {
        assert!(
            release.archive_url().as_str().starts_with("https://"),
            "{} is not https",
            release.archive_url()
        );
    }
}

#[test]
fn each_build_is_pinned_once() {
    // Two pins a single machine could not tell apart would make which archive
    // gets installed depend on the order of the file. The reader refuses that
    // outright; this says the compiled-in file does not ask it to.
    let releases = releases_for(&opencode()).expect("the pinned releases parse");
    let builds: BTreeSet<_> = releases
        .iter()
        .map(|release| format!("{} {}", release.platform(), release.requirements()))
        .collect();
    assert_eq!(
        builds.len(),
        releases.len(),
        "two pins describe the same build"
    );
}

#[test]
fn each_build_has_its_own_archive() {
    // Nine builds sharing one digest would mean the generator hashed the same
    // download nine times.
    let releases = releases_for(&opencode()).expect("the pinned releases parse");
    let urls: BTreeSet<_> = releases
        .iter()
        .map(|release| release.archive_url().as_str())
        .collect();
    assert_eq!(urls.len(), releases.len(), "two pins share an archive");
    let digests: BTreeSet<_> = releases
        .iter()
        .map(|release| release.archive_digest().as_str())
        .collect();
    assert_eq!(digests.len(), releases.len(), "two pins share a digest");
}

#[test]
fn an_agent_with_no_pins_has_no_releases() {
    // Claude and Codex are not installed by Nessa. Asking is not an error; the
    // answer is that there is nothing to install.
    let claude = AgentName::parse("claude").expect("a plain agent name");
    assert_eq!(releases_for(&claude), Ok(Vec::new()));
}

#[test]
fn an_unpinned_platform_has_no_release() {
    let host = machine("windows", "x86_64", Some(Libc::Gnu), true);
    let pinned = releases_for(&opencode()).expect("the pinned releases parse");
    assert!(!pinned.iter().any(|release| release.runs_on(&host)));
}

#[test]
fn a_machine_with_no_known_c_library_gets_no_linux_build() {
    // Every Linux build names a library, so a target linked against neither is
    // offered nothing rather than offered a guess. The install then says so,
    // which is the outcome worth having: the alternative is a download that
    // ends in a loader error.
    let host = machine("linux", "x86_64", None, true);
    let pinned = releases_for(&opencode()).expect("the pinned releases parse");
    assert!(!pinned.iter().any(|release| release.runs_on(&host)));
}

#[test]
fn the_url_that_is_requested_is_the_url_the_file_records() {
    // A URL is parsed now rather than matched on, and parsing normalises: an
    // uppercase host, a default port or a percent-escape would be requested in
    // a different spelling than the pin file records, and the digest would then
    // be measured against bytes nobody wrote that line for.
    let document: serde_json::Value =
        serde_json::from_str(PINS).expect("the pin file is well-formed json");
    let written: Vec<&str> = document["agents"]["opencode"]
        .as_array()
        .expect("opencode is pinned")
        .iter()
        .map(|entry| entry["archiveUrl"].as_str().expect("an archive url"))
        .collect();
    let requested: Vec<String> = releases_for(&opencode())
        .expect("the pinned releases parse")
        .iter()
        .map(|release| release.archive_url().as_str().to_owned())
        .collect();
    assert_eq!(requested, written);
}

#[test]
fn an_agent_pinned_twice_is_refused_rather_than_resolved() {
    // A repeated key is resolved silently by a map decoder, and which of the
    // two wins is a property of the decoder rather than a decision anybody
    // made. The value being chosen is the digest of a binary Nessa will run.
    //
    // Both orders and the repeated-equal case, because "last one wins" and
    // "first one wins" are each invisible in one of the three.
    let pinned_twice = [
        r#"{"agents":{"opencode":[],"opencode":[]}}"#,
        r#"{"agents":{"opencode":[],"opencode":[{"operatingSystem":"macos","architecture":"aarch64","version":"1.0.0","archiveUrl":"https://registry.example/a.tgz","archiveDigest":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","executable":"package/bin/opencode"}]}}"#,
        r#"{"agents":{"opencode":[{"operatingSystem":"macos","architecture":"aarch64","version":"1.0.0","archiveUrl":"https://registry.example/a.tgz","archiveDigest":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","executable":"package/bin/opencode"}],"opencode":[]}}"#,
    ];
    for document in pinned_twice {
        assert!(
            serde_json::from_str::<serde_json::Value>(document).is_ok(),
            "the fixture is well-formed json"
        );
        assert!(
            matches!(
                releases_in(document, &opencode()),
                Err(PinFileError::Malformed(_))
            ),
            "an agent pinned twice was resolved instead of refused: {document}"
        );
    }
}

#[test]
fn a_platform_pinned_twice_is_refused_rather_than_resolved() {
    // Which of two entries for one platform gets installed would otherwise be
    // decided by the order they happen to be written in.
    let document = r#"{"agents":{"opencode":[
        {"operatingSystem":"macos","architecture":"aarch64","version":"1.0.0","archiveUrl":"https://registry.example/a.tgz","archiveDigest":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","executable":"package/bin/opencode"},
        {"operatingSystem":"macos","architecture":"aarch64","version":"1.0.0","archiveUrl":"https://registry.example/b.tgz","archiveDigest":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","executable":"package/bin/opencode"}
    ]}}"#;
    let platform = ReleasePlatform::new("macos", "aarch64").expect("usable platform");
    assert_eq!(
        releases_in(document, &opencode()),
        Err(PinFileError::PlatformPinnedTwice {
            agent: "opencode".into(),
            platform,
            requirements: ReleaseRequirements::default(),
        })
    );
}

#[test]
fn one_platform_may_be_pinned_twice_for_two_different_builds() {
    // The other side of the rule above, and the reason it compares what a build
    // needs rather than only where it runs: two Linux x86-64 archives are an
    // ordinary pin when one is for musl and the other for glibc. Refusing them
    // would make the file unable to say what Opencode actually publishes.
    let document = r#"{"agents":{"opencode":[
        {"operatingSystem":"linux","architecture":"x86_64","libc":"gnu","version":"1.0.0","archiveUrl":"https://registry.example/a.tgz","archiveDigest":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","executable":"package/bin/opencode"},
        {"operatingSystem":"linux","architecture":"x86_64","libc":"musl","version":"1.0.0","archiveUrl":"https://registry.example/b.tgz","archiveDigest":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","executable":"package/bin/opencode"},
        {"operatingSystem":"linux","architecture":"x86_64","libc":"musl","requiresAvx2":true,"archiveUrl":"https://registry.example/c.tgz","version":"1.0.0","archiveDigest":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","executable":"package/bin/opencode"}
    ]}}"#;
    let releases = releases_in(document, &opencode()).expect("three distinct builds are a pin");
    assert_eq!(releases.len(), 3);
    assert!(releases[0].runs_on(&machine("linux", "x86_64", Some(Libc::Gnu), false)));
    assert!(releases[1].runs_on(&machine("linux", "x86_64", Some(Libc::Musl), false)));
    assert!(!releases[2].runs_on(&machine("linux", "x86_64", Some(Libc::Musl), false)));
    assert!(releases[2].runs_on(&machine("linux", "x86_64", Some(Libc::Musl), true)));
}

#[test]
fn a_c_library_nessa_cannot_check_is_refused() {
    // The field is a name written by hand in a generated file. A spelling this
    // build does not know would otherwise read as "no requirement" and offer
    // the build to every machine on its platform.
    let document = r#"{"agents":{"opencode":[
        {"operatingSystem":"linux","architecture":"x86_64","libc":"glibc","version":"1.0.0","archiveUrl":"https://registry.example/a.tgz","archiveDigest":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","executable":"package/bin/opencode"}
    ]}}"#;
    assert_eq!(
        releases_in(document, &opencode()),
        Err(PinFileError::Invalid {
            agent: "opencode".into(),
            reason: PinRejected::Libc("glibc".into()),
        })
    );
}

#[test]
fn a_build_that_says_nothing_about_a_processor_asks_for_nothing() {
    // `requiresAvx2` absent has to read as false, and this is the direction the
    // default has to fall: a build wrongly marked as not needing AVX2 runs
    // everywhere and only gives up speed, where the other mistake is an illegal
    // instruction on a machine that was told its runtime was ready.
    let document = r#"{"agents":{"opencode":[
        {"operatingSystem":"macos","architecture":"aarch64","version":"1.0.0","archiveUrl":"https://registry.example/a.tgz","archiveDigest":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","executable":"package/bin/opencode"}
    ]}}"#;
    let releases = releases_in(document, &opencode()).expect("a pin with no requirements");
    assert_eq!(releases[0].requirements(), &ReleaseRequirements::default());
    assert!(releases[0].runs_on(&machine("macos", "aarch64", None, false)));
}

#[test]
fn a_pin_that_breaks_a_domain_rule_names_the_rule_it_broke() {
    // The file is generated, but it is also checked in and editable. A pin that
    // is wrong has to say which field and why, because the only thing anyone
    // can do about it is edit that field.
    let document = r#"{"agents":{"opencode":[
        {"operatingSystem":"macos","architecture":"aarch64","version":"1.0.0","archiveUrl":"http://registry.example/a.tgz","archiveDigest":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","executable":"package/bin/opencode"}
    ]}}"#;
    let failure = releases_in(document, &opencode()).expect_err("http is not a pin");
    assert_eq!(
        failure,
        PinFileError::Invalid {
            agent: "opencode".into(),
            reason: PinRejected::ArchiveUrl("http://registry.example/a.tgz".into()),
        }
    );
    let message = failure.to_string();
    assert!(
        message.contains("opencode") && message.contains("https"),
        "unhelpful message: {message}"
    );
}

#[test]
fn a_document_that_is_not_the_shape_this_build_reads_is_named_as_malformed() {
    let failure = releases_in(r#"{"agents":{"opencode":"one"}}"#, &opencode())
        .expect_err("a string is not a list of releases");
    assert!(matches!(failure, PinFileError::Malformed(_)), "{failure:?}");
    assert!(failure.to_string().contains("malformed"), "{failure}");
}

#[test]
fn the_host_platform_is_nameable() {
    // `host_platform` unwraps, on the grounds that the compiler's own tokens
    // are always plain identifiers. This test is what makes that true on
    // whatever target the suite is built for rather than only on the ones
    // thought of when it was written.
    let host = host_platform();
    assert!(!host.platform().operating_system().is_empty());
    assert!(!host.platform().architecture().is_empty());
    // And it says something readable about itself, because the one place this
    // value is shown to a person is the message saying nothing is pinned for
    // their machine.
    assert!(host.to_string().contains(host.platform().architecture()));
}

#[test]
fn this_machine_knows_which_c_library_it_has() {
    // Taken from what this binary was linked against, so on any target Nessa
    // actually ships — every one of which is glibc or musl — there is an
    // answer. A target with neither would be one where every Linux build is
    // refused, which is a decision to make deliberately rather than to
    // discover from a bug report.
    if cfg!(target_os = "linux") {
        assert!(
            host_platform().satisfies(&ReleaseRequirements::new(Some(host_libc_here()), false)),
            "this linux build does not agree with itself about its c library"
        );
    }
}

/// The C library this test binary was linked against, worked out the same way
/// the adapter does. Written out here rather than reused so that the adapter's
/// own answer is being checked against something, not against itself.
fn host_libc_here() -> Libc {
    if cfg!(target_env = "musl") {
        Libc::Musl
    } else {
        Libc::Gnu
    }
}
