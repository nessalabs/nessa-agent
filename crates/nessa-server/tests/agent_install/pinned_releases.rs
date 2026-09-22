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

/// One entry as the generator writes it, with every field present.
///
/// Built rather than spelled out per test, because every field is required and
/// unknown fields are refused: a fixture written by hand drifts from the shape
/// the reader accepts, and a test that then fails to parse proves nothing
/// about the rule it was written for.
fn entry(
    operating_system: &str,
    architecture: &str,
    libc: Option<&str>,
    avx2: bool,
    digest_byte: char,
) -> serde_json::Value {
    serde_json::json!({
        "operatingSystem": operating_system,
        "architecture": architecture,
        "libc": libc,
        "requiresAvx2": avx2,
        "version": "1.0.0",
        "archiveUrl": format!("https://registry.example/{digest_byte}.tgz"),
        "archiveDigest": std::iter::repeat_n(digest_byte, 64).collect::<String>(),
        "executable": "package/bin/opencode",
    })
}

/// A pin document holding exactly `entries` for opencode.
fn document(entries: Vec<serde_json::Value>) -> String {
    serde_json::json!({ "agents": { "opencode": entries } }).to_string()
}

/// Every machine the pin file is expected to have a build for.
///
/// Named here rather than derived from the file, so that dropping a build from
/// the pin — which would silently stop offering Opencode to everyone on that
/// kind of machine — fails this test instead of passing quietly.
///
/// Machines rather than platforms, because a platform is not what a build is
/// chosen for: Linux x86-64 appears once per C library, and each entry also
/// says what the processor has to be.
///
/// Every x86-64 machine here has AVX2, and that is the pin file being honest
/// rather than an omission. See [`UNSERVED`].
const COVERED: &[(&str, &str, Option<Libc>, bool)] = &[
    ("macos", "aarch64", None, false),
    ("macos", "x86_64", None, true),
    ("linux", "aarch64", Some(Libc::Gnu), false),
    ("linux", "aarch64", Some(Libc::Musl), false),
    ("linux", "x86_64", Some(Libc::Gnu), true),
    ("linux", "x86_64", Some(Libc::Musl), true),
];

/// Every machine the pin file is expected to have nothing for.
///
/// An x86-64 processor without AVX2. Opencode names three `-baseline` packages
/// as the builds for exactly this machine, and at 1.18.31 each holds an
/// executable byte-identical to its plain sibling, so one claim in each pair
/// is false and the bytes do not say which. `scripts/agents/pin-opencode.mjs`
/// carries the measurement and the reasoning; the short of it is that the
/// evidence leans towards these builds not needing AVX2 at all, and leaning is
/// not enough to promise somebody their runtime will start.
///
/// So nothing is offered here, and "nessa has no tested opencode release this
/// machine can run" is true under either reading of the bytes.
///
/// This is the other half of [`COVERED`], and the more fragile half: the
/// natural repair for "Opencode is not offered on this machine" is to add back
/// the package the vendor named for it, which is the one thing that must not
/// happen while nobody can say what it needs. This says so as a test.
const UNSERVED: &[(&str, &str, Option<Libc>)] = &[
    ("macos", "x86_64", None),
    ("linux", "x86_64", Some(Libc::Gnu)),
    ("linux", "x86_64", Some(Libc::Musl)),
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
fn a_machine_without_avx2_is_offered_nothing_rather_than_something_that_dies() {
    let releases = releases_for(&opencode()).expect("the pinned releases parse");
    for (operating_system, architecture, libc) in UNSERVED {
        let host = machine(operating_system, architecture, *libc, false);
        let offered: Vec<_> = releases
            .iter()
            .filter(|release| release.runs_on(&host))
            .map(|release| package_named_by(release.archive_url().as_str()))
            .collect();
        assert!(
            offered.is_empty(),
            "{host} is offered {offered:?}, which needs a processor it does not have"
        );
    }
}

#[test]
fn what_each_pin_says_it_needs_agrees_with_the_archive_it_names() {
    // The one claim in this change that nothing else can check. Which builds
    // need AVX2 and which C library each is linked against is a hand-written
    // table in `scripts/agents/pin-opencode.mjs`; every Rust test downstream
    // takes the generated file as ground truth, so flipping one `requiresAvx2`
    // to false there would leave the whole suite green and hand every older
    // x86-64 machine a binary that dies on an illegal instruction.
    //
    // The archive's own name is the witness — and it is not an independent
    // one, which is the limit of what this test is worth. `pin-opencode.mjs`
    // takes `requiresAvx2` and `libc` from a table keyed by that same package
    // name, so this asserts that the generator copied its own table correctly
    // and nothing more. It catches a hand-edited pin file, which is what it is
    // for; it cannot catch a vendor whose names do not describe their
    // contents.
    //
    // At 1.18.31 they do not, which is why no `-baseline` package is pinned at
    // all and why the rule below is the flat one: every x86-64 build that *is*
    // pinned is pinned as needing AVX2. All three pairs publish a
    // byte-identical `package/bin/opencode` (linux-x64 sha256 f9dab322…,
    // darwin-x64 9cd3d83b…, linux-x64-musl b4a7415a…, measured from both
    // archives of each pair), so one claim in each pair is false and the bytes
    // do not say which. Rather than pick, the file offers no x86-64 build to a
    // machine without AVX2 at all: [`UNSERVED`] says why, and
    // `a_machine_without_avx2_is_offered_nothing_rather_than_something_that_dies`
    // is what holds it.
    //
    // The measurement that *is* independent lives in the generator —
    // `sameBinaryUnderDifferentClaims` hashes the entry it already extracts and
    // refuses a release whose builds claim different things about the same
    // bytes — so this test stays as the check on the generated file and that
    // one is the check on the vendor. `pin-opencode.test.mjs` closes the gap
    // between them by asserting that this file is the one that table describes.
    for release in releases_for(&opencode()).expect("the pinned releases parse") {
        let package = package_named_by(release.archive_url().as_str());
        let x86 = release.platform().architecture() == "x86_64";
        let needs = release.requirements();

        assert_eq!(
            needs.avx2(),
            x86,
            "{package} and what it says it needs disagree about avx2"
        );
        assert!(
            !package.contains("-baseline"),
            "{package} is pinned, and at 1.18.31 nobody can say what a baseline build needs"
        );
        let libc = match release.platform().operating_system() {
            "linux" if package.contains("-musl") => Some(Libc::Musl),
            "linux" => Some(Libc::Gnu),
            _ => None,
        };
        assert_eq!(
            needs.libc(),
            libc,
            "{package} and what it says it needs disagree about the c library"
        );
    }
}

/// The npm package an archive URL names, which is the vendor's own word for
/// which build it is.
fn package_named_by(url: &str) -> &str {
    url.strip_prefix("https://registry.npmjs.org/")
        .and_then(|rest| rest.split('/').next())
        .expect("every pinned archive is an npm tarball")
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
    // And that the one answer is the one being downloaded. `version` is a
    // separate field from `archiveUrl`, so agreeing with the other pins says
    // nothing about agreeing with the tarball: a bump that edited every
    // `version` and no URL would leave the whole file consistent and every
    // entry wrong, and `1.18.31` is what a person reads to know what Nessa
    // tested. npm's own path carries the version, so the two can be compared
    // without fetching anything.
    for release in &releases {
        let package = package_named_by(release.archive_url().as_str());
        let version = release.version();
        assert!(
            release
                .archive_url()
                .as_str()
                .ends_with(&format!("{package}-{version}.tgz")),
            "{package} is pinned as {version} but its archive is not that version"
        );
    }
}

/// Where Opencode keeps its executable inside every one of its packages.
///
/// The generator's own constant, written here as well rather than shared:
/// this test exists to catch a file that no longer agrees with the generator,
/// so reading the value from the thing under suspicion would prove nothing.
const OPENCODE_EXECUTABLE: &str = "package/bin/opencode";

/// The entry each pin names is Opencode's executable and not some other file in
/// the same archive.
///
/// The worst of the three claims an archive URL is checked against, because it
/// is the one that fails by succeeding. A wrong version is offered to a machine
/// that then installs the wrong thing loudly; a wrong entry passes the digest
/// check untouched — the *archive* is still the pinned one — and unpacks,
/// records and reports a file that is not a program. `package/package.json` is
/// a real entry of every one of these archives, and pinning it would have
/// `install-agent` announce a hundred and forty bytes of JSON as the tested
/// Opencode runtime.
///
/// The generator writes one constant into all nine entries, so this is the
/// same kind of witness as the `libc` and `avx2` ones above: it catches a
/// hand-edited pin file, which is what it is for. What it cannot catch is a
/// release that moved its executable — for that the generator refuses at
/// generation time, where the bytes are in hand.
#[test]
fn every_pin_names_opencodes_own_executable() {
    for release in releases_for(&opencode()).expect("the pinned releases parse") {
        let package = package_named_by(release.archive_url().as_str());
        assert_eq!(
            release.launch().as_str(),
            OPENCODE_EXECUTABLE,
            "{package} pins an entry that is not opencode's executable"
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
    let platform = ReleasePlatform::new("macos", "aarch64").expect("usable platform");
    assert_eq!(
        releases_in(
            &document(vec![
                entry("macos", "aarch64", None, false, 'a'),
                entry("macos", "aarch64", None, false, 'b'),
            ]),
            &opencode()
        ),
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
    let releases = releases_in(
        &document(vec![
            entry("linux", "x86_64", Some("gnu"), false, 'a'),
            entry("linux", "x86_64", Some("musl"), false, 'b'),
            entry("linux", "x86_64", Some("musl"), true, 'c'),
        ]),
        &opencode(),
    )
    .expect("three distinct builds are a pin");
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
    assert_eq!(
        releases_in(
            &document(vec![entry("linux", "x86_64", Some("glibc"), false, 'a')]),
            &opencode()
        ),
        Err(PinFileError::Invalid {
            agent: "opencode".into(),
            reason: PinRejected::Libc("glibc".into()),
        })
    );
}

#[test]
fn a_build_that_says_nothing_about_what_it_needs_is_refused() {
    // What a build needs is stated by absence as much as by presence: no
    // `libc` means it runs against either, no `requiresAvx2` means it runs on
    // any processor. So a key dropped in a merge, or misspelled by whoever
    // next edits the generator, would quietly turn a build that runs on some
    // machines into one offered to all of them — and that mistake ends in an
    // illegal instruction on somebody else's machine, where a missing key ends
    // in a failing test on the machine of whoever wrote it.
    let mut complete = entry("macos", "aarch64", None, false, 'a');
    for missing in ["libc", "requiresAvx2", "version", "archiveDigest"] {
        let mut incomplete = complete.clone();
        incomplete
            .as_object_mut()
            .expect("the fixture is an object")
            .remove(missing);
        assert!(
            matches!(
                releases_in(&document(vec![incomplete]), &opencode()),
                Err(PinFileError::Malformed(_))
            ),
            "a release with no {missing} was read as one that says something"
        );
    }

    // And a key this build does not know is refused rather than ignored, which
    // is the same mistake wearing a typo.
    let misspelled = complete.as_object_mut().expect("the fixture is an object");
    misspelled.remove("requiresAvx2");
    misspelled.insert("requiresAVX2".into(), true.into());
    assert!(
        matches!(
            releases_in(&document(vec![complete]), &opencode()),
            Err(PinFileError::Malformed(_))
        ),
        "a misspelled requirement was passed over"
    );
}

#[test]
fn a_build_whose_requirements_do_not_fit_its_platform_is_refused_through_this_file_too() {
    // The rule itself belongs to `PinnedRelease` and is tested there, without
    // JSON. What this holds is that the file is read *through* it: the pin
    // document is one way to assemble a release and must not be a way around
    // the pair of rules that decide which machines a build is offered to.
    for (operating_system, architecture, libc, avx2) in [
        // A Linux build naming no C library. Read as written it would be
        // offered to every Linux machine, half of which cannot start it — and
        // it would slip past the duplicate check too, which compares what two
        // builds need rather than which machines they overlap on.
        ("linux", "x86_64", None, false),
        // AVX2 asked of a processor that has no such thing.
        ("linux", "aarch64", Some("gnu"), true),
    ] {
        assert!(
            matches!(
                releases_in(
                    &document(vec![entry(operating_system, architecture, libc, avx2, 'a')]),
                    &opencode()
                ),
                Err(PinFileError::Invalid {
                    reason: PinRejected::Requirements(_),
                    ..
                })
            ),
            "{operating_system}-{architecture} with libc {libc:?} and avx2 {avx2} was accepted"
        );
    }
    // macOS has one C library, so saying nothing there is the truth.
    assert!(releases_in(
        &document(vec![entry("macos", "aarch64", None, false, 'a')]),
        &opencode()
    )
    .is_ok());
}

#[test]
fn one_archive_is_not_pinned_twice() {
    // The digest is the identity of a build, and the store keys an
    // installation by it. Two releases naming one archive are two descriptions
    // of the same bytes, with one directory between them and nothing to say
    // which description the file there belongs to.
    assert_eq!(
        releases_in(
            &document(vec![
                entry("linux", "x86_64", Some("gnu"), false, 'a'),
                entry("linux", "x86_64", Some("musl"), false, 'a'),
            ]),
            &opencode()
        ),
        Err(PinFileError::ArchivePinnedTwice {
            agent: "opencode".into(),
            digest: "a".repeat(64),
        })
    );
}

#[test]
fn a_pin_that_breaks_a_domain_rule_names_the_rule_it_broke() {
    // The file is generated, but it is also checked in and editable. A pin that
    // is wrong has to say which field and why, because the only thing anyone
    // can do about it is edit that field.
    let mut insecure = entry("macos", "aarch64", None, false, 'a');
    insecure["archiveUrl"] = "http://registry.example/a.tgz".into();
    let failure =
        releases_in(&document(vec![insecure]), &opencode()).expect_err("http is not a pin");
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
fn this_machine_gets_a_build_when_one_is_pinned_for_its_platform() {
    // The adapter and the pin file, checked against each other on whatever
    // machine the suite is running on. Asking the adapter to agree with a copy
    // of its own `cfg!` expression would prove nothing and would accuse the
    // code falsely on a target linked against neither library — the case where
    // the honest answer is `None`.
    //
    // This is the assertion that answer has to survive: a machine whose C
    // library could not be established, on a platform Nessa does pin, gets
    // nothing it can install. Skipped rather than failed where the platform
    // itself is unpinned, which is Windows today.
    let host = host_platform();
    let releases = releases_for(&opencode()).expect("the pinned releases parse");
    if releases
        .iter()
        .any(|release| release.platform() == host.platform())
    {
        assert!(
            releases.iter().any(|release| release.runs_on(&host)),
            "{host} has pinned builds for its platform and none it can run"
        );
    }
}
