use super::*;
use std::collections::BTreeSet;

use crate::agent_install::domain::{AgentName, PinRejected};

fn opencode() -> AgentName {
    AgentName::parse("opencode").expect("a plain agent name")
}

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
    let releases = releases_for(&opencode()).expect("the pinned releases parse");
    assert!(!releases.is_empty(), "opencode has at least one pin");
}

#[test]
fn every_supported_platform_is_pinned() {
    let releases = releases_for(&opencode()).expect("the pinned releases parse");
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
fn each_platform_is_pinned_once() {
    // Two pins for one platform would make which archive gets installed depend
    // on the order of the file.
    let releases = releases_for(&opencode()).expect("the pinned releases parse");
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
    let releases = releases_for(&opencode()).expect("the pinned releases parse");
    let urls: BTreeSet<_> = releases
        .iter()
        .map(|release| release.archive_url().as_str())
        .collect();
    assert_eq!(urls.len(), releases.len(), "two pins share an archive");
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
    let platform = ReleasePlatform::new("windows", "x86_64").expect("usable platform");
    let pinned = releases_for(&opencode()).expect("the pinned releases parse");
    assert!(!pinned.iter().any(|release| release.runs_on(&platform)));
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
        })
    );
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
    let platform = host_platform();
    assert!(!platform.operating_system().is_empty());
    assert!(!platform.architecture().is_empty());
}
