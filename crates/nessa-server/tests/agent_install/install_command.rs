use super::*;
use crate::agent_install::domain::{Libc, ReleasePlatform, ReleaseRequirements};
use std::path::Path;

use crate::agent_install::application::{InstalledRuntime, StoreFailure};
use crate::agent_install::domain::{
    ArchiveDigest, ArchivePath, ArchiveRejected, ArchiveUrl, PinnedRelease, ReleaseVersion,
};
use crate::agent_install_test_support::temporary_root;

fn opencode() -> AgentName {
    AgentName::parse("opencode").expect("a plain agent name")
}

fn digest(byte: char) -> ArchiveDigest {
    ArchiveDigest::parse(&std::iter::repeat_n(byte, 64).collect::<String>())
        .expect("a run of one hex character is a digest")
}

fn rejection() -> ArchiveRejected {
    PinnedRelease::new(
        ReleaseVersion::parse("1.0.0").expect("usable version"),
        host_platform().platform().clone(),
        ReleaseRequirements::default(),
        ArchiveUrl::parse("https://registry.example/runtime.tgz").expect("a fetchable url"),
        digest('a'),
        ArchivePath::parse("package/bin/opencode").expect("contained path"),
    )
    .accept(&digest('b'))
    .expect_err("another archive is not the pinned one")
}

/// Install the pinned Opencode release from inside an async runtime, the way
/// the command does.
///
/// Ignored by default alongside the other live test — it fetches the real
/// archive. What it proves is that `install` works when it is reached through
/// `spawn_blocking` from inside a runtime: the HTTP client it uses refuses to
/// run on a runtime thread and panics rather than blocking, so that arrangement
/// is load-bearing and nothing else in the suite exercises it.
///
/// What it does not prove is that `execute` still uses `spawn_blocking`, since
/// it builds the call itself rather than going through `execute` — `execute`
/// resolves its root from `NESSA_DATA_DIR` or the home directory, and pointing
/// that somewhere safe means setting a process-wide variable underneath a suite
/// that runs in parallel. Inlining the call in `execute` would therefore still
/// leave this green; what catches that is running the command.
///
/// ```text
/// cargo test -p nessa-server --lib -- --ignored installs_from_inside_the_runtime
/// ```
#[test]
#[ignore = "downloads the real release archive"]
fn installs_from_inside_the_runtime() {
    let root = temporary_root();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("an async runtime");

    let installed = runtime
        .block_on(async {
            // Spawned from inside the runtime, as `execute` does it. Building
            // the task outside `block_on` would be a different thing entirely —
            // `spawn_blocking` needs a runtime context to be called at all.
            let root = root.path().to_owned();
            tokio::task::spawn_blocking(move || install(&opencode(), &root)).await
        })
        .expect("the installer finishes")
        .expect("the pinned release installs");

    assert!(installed.downloaded);
    assert!(installed.executable.is_file());
}

#[test]
fn an_agent_nessa_does_not_install_is_named_as_such() {
    // Claude and Codex are expected to be on the machine already. Asking to
    // install one is a mistake worth a clear answer rather than a download that
    // fails obscurely — and it must not touch the network to say so.
    let root = temporary_root();
    let claude = AgentName::parse("claude").expect("a plain agent name");
    let failure = install(&claude, root.path()).expect_err("claude is not installed by nessa");
    assert!(
        failure.to_string().contains("not an agent nessa installs"),
        "unhelpful message: {failure}"
    );
}

#[test]
fn an_agent_with_no_build_for_this_machine_is_told_so() {
    // The opposite message: the agent is one Nessa installs, this machine is
    // just not one there is a tested build for. Telling that person Opencode
    // "is not an agent nessa installs" would be false.
    let elsewhere = HostPlatform::new(
        ReleasePlatform::new("plan9", "sparc64").expect("usable platform"),
        None,
        false,
    );
    let failure = pinned(&opencode(), &elsewhere).expect_err("plan9 is not a pinned platform");
    let message = failure.to_string();
    assert!(
        message.contains("no tested opencode release") && message.contains("plan9"),
        "unhelpful message: {message}"
    );
}

#[test]
fn nothing_is_written_for_an_agent_with_no_release() {
    let root = temporary_root();
    let claude = AgentName::parse("claude").expect("a plain agent name");
    let _ = install(&claude, root.path());
    assert!(
        !root.path().join("claude").exists(),
        "a refused install left a directory behind"
    );
}

#[test]
fn a_rejected_archive_is_the_one_failure_that_says_not_to_retry() {
    // The whole reason the install use case reports typed failures. A digest
    // that did not match is not a bad connection, and the last line somebody
    // reads should not invite them to run the command again.
    let retry = explain(&InstallFailure::Download(SourceFailure::Unreachable(
        "timed out".into(),
    )));
    assert!(retry.contains("try again"), "unhelpful message: {retry}");

    let stop = explain(&InstallFailure::Rejected(rejection()));
    assert!(!stop.contains("try again"), "misleading message: {stop}");
    assert!(stop.contains("not worth retrying"), "unhelpful: {stop}");
}

#[test]
fn every_failure_nothing_can_be_done_about_says_so() {
    // "Try again" is advice, and it is wrong for each of these. A body that
    // overran the bound will overrun it again; an archive that does not hold
    // the pinned executable will not grow one; an archive that does not unpack
    // is the same archive next time. Each is a fault in the pin or the release,
    // and the only thing that helps is somebody being told to report it.
    for failure in [
        InstallFailure::Download(SourceFailure::TooLarge(512)),
        InstallFailure::Rejected(rejection()),
        InstallFailure::Store(StoreFailure::MissingExecutable(
            "package/bin/opencode".into(),
        )),
        InstallFailure::Store(StoreFailure::MalformedArchive("not a tar".into())),
    ] {
        let message = explain(&failure);
        assert!(
            !message.contains("try again"),
            "{failure:?} invites a retry that cannot help: {message}"
        );
        assert!(
            message.contains("not worth retrying"),
            "{failure:?} does not say to stop: {message}"
        );
        assert!(
            message.contains("nothing was installed"),
            "{failure:?} does not say whether anything was installed: {message}"
        );
    }
}

#[test]
fn a_failure_this_machine_may_recover_from_still_invites_a_retry() {
    // The other half of the rule above. A connection that dropped and a disk
    // that was briefly full are worth another go, and telling somebody to
    // report them instead would send them to file a bug about their wifi.
    for failure in [
        InstallFailure::Download(SourceFailure::Unreachable("connection reset".into())),
        InstallFailure::Download(SourceFailure::NotStored("no space".into())),
    ] {
        let message = explain(&failure);
        assert!(
            message.contains("try again"),
            "{failure:?} should be worth another go: {message}"
        );
    }
}

#[test]
fn a_withdrawn_release_is_distinguished_from_a_bad_connection() {
    let withdrawn = explain(&InstallFailure::Download(SourceFailure::Refused(404)));
    assert!(withdrawn.contains("404"), "unhelpful message: {withdrawn}");
    assert!(
        withdrawn.contains("withdrawn"),
        "unhelpful message: {withdrawn}"
    );
}

#[test]
fn a_failure_says_that_nothing_was_installed() {
    // Somebody whose install failed needs to know whether they have half a
    // runtime on the machine.
    for failure in [
        InstallFailure::UnsupportedPlatform(HostPlatform::new(
            ReleasePlatform::new("windows", "x86_64").expect("usable platform"),
            Some(Libc::Gnu),
            false,
        )),
        InstallFailure::Download(SourceFailure::Unreachable("offline".into())),
        InstallFailure::Rejected(rejection()),
        InstallFailure::Store(StoreFailure::Unwritable("no room".into())),
    ] {
        let message = explain(&failure);
        assert!(
            message.contains("nothing was installed"),
            "{failure:?} does not say whether anything was installed: {message}"
        );
    }
}

#[test]
fn what_the_command_prints_is_one_line_of_json() {
    // The output is a contract: this command is meant to be read by the app as
    // well as by a person, and a second line or a missing newline breaks a
    // reader that takes it a line at a time.
    let installed = InstalledRuntime {
        version: ReleaseVersion::parse("1.18.31").expect("usable version"),
        executable: Path::new("/tmp/agents/opencode/1.18.31/opencode").to_owned(),
        downloaded: true,
    };
    let mut written = Vec::new();

    write_report(&mut written, &report(&opencode(), &installed)).expect("the report is written");

    let text = String::from_utf8(written).expect("the report is text");
    assert!(text.ends_with('\n'), "the report is not a line: {text:?}");
    assert_eq!(text.lines().count(), 1, "the report is more than one line");
    let parsed: serde_json::Value = serde_json::from_str(&text).expect("the report is json");
    assert_eq!(parsed["agent"], "opencode");
    assert_eq!(parsed["version"], "1.18.31");
    assert_eq!(
        parsed["executable"],
        "/tmp/agents/opencode/1.18.31/opencode"
    );
    assert_eq!(parsed["downloaded"], true);
}

#[test]
fn an_install_that_downloaded_nothing_says_so() {
    // The difference between "downloaded a hundred megabytes" and "looked at a
    // directory", which a surface reading this has no other way to know.
    let installed = InstalledRuntime {
        version: ReleaseVersion::parse("1.18.31").expect("usable version"),
        executable: Path::new("/tmp/agents/opencode/1.18.31/opencode").to_owned(),
        downloaded: false,
    };

    assert_eq!(report(&opencode(), &installed)["downloaded"], false);
}

/// One machine, for the tests about which build gets chosen for it.
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

#[test]
fn each_machine_gets_the_build_opencode_publishes_for_it() {
    // The whole matrix, against the compiled-in pin file, named by the package
    // each machine ends up with. Both of the wrong choices here are a binary
    // that does not start — a glibc build dies in the loader on a musl-only
    // machine, and an AVX2 build dies on an illegal instruction — after Nessa
    // has told somebody their runtime is ready.
    //
    // Asserting `chosen.runs_on(&host)` instead would say nothing: `preferred`
    // filters on exactly that, so the assertion would restate the filter and
    // hold for any pin file at all, including one that marked every build as
    // running everywhere. The package name is the vendor's own word for which
    // build it is, so it is a fact from outside this code.
    for (operating_system, architecture, libc, avx2, expected) in [
        (
            "linux",
            "x86_64",
            Some(Libc::Gnu),
            true,
            "opencode-linux-x64",
        ),
        (
            "linux",
            "x86_64",
            Some(Libc::Gnu),
            false,
            "opencode-linux-x64-baseline",
        ),
        (
            "linux",
            "x86_64",
            Some(Libc::Musl),
            true,
            "opencode-linux-x64-musl",
        ),
        (
            "linux",
            "x86_64",
            Some(Libc::Musl),
            false,
            "opencode-linux-x64-baseline-musl",
        ),
        (
            "linux",
            "aarch64",
            Some(Libc::Gnu),
            false,
            "opencode-linux-arm64",
        ),
        (
            "linux",
            "aarch64",
            Some(Libc::Musl),
            false,
            "opencode-linux-arm64-musl",
        ),
        ("macos", "x86_64", None, true, "opencode-darwin-x64"),
        (
            "macos",
            "x86_64",
            None,
            false,
            "opencode-darwin-x64-baseline",
        ),
        ("macos", "aarch64", None, false, "opencode-darwin-arm64"),
    ] {
        let host = machine(operating_system, architecture, libc, avx2);
        let chosen = pinned(&opencode(), &host).expect("every supported machine has a build");

        assert!(
            chosen
                .archive_url()
                .as_str()
                .contains(&format!("/{expected}/")),
            "{host} was given {} rather than {expected}",
            chosen.archive_url()
        );
    }
}

#[test]
fn a_processor_with_avx2_gets_the_build_that_uses_it() {
    // Both builds run on such a machine, so this is a preference rather than a
    // filter — and the demanding one is the vendor's own default. The baseline
    // build exists for machines that cannot take it, and installing it
    // everywhere would give up exactly what it is there to preserve.
    let with = machine("linux", "x86_64", Some(Libc::Gnu), true);
    let without = machine("linux", "x86_64", Some(Libc::Gnu), false);

    let fast = pinned(&opencode(), &with).expect("a build for a machine with avx2");
    let baseline = pinned(&opencode(), &without).expect("a build for a machine without it");

    assert!(
        fast.requirements().avx2(),
        "the faster build was passed over"
    );
    assert!(
        !baseline.requirements().avx2(),
        "a processor without avx2 was offered a build that needs it"
    );
    assert_ne!(fast.archive_digest(), baseline.archive_digest());
}

#[test]
fn the_two_c_libraries_get_two_different_builds() {
    let gnu = pinned(
        &opencode(),
        &machine("linux", "x86_64", Some(Libc::Gnu), true),
    )
    .expect("a glibc build");
    let musl = pinned(
        &opencode(),
        &machine("linux", "x86_64", Some(Libc::Musl), true),
    )
    .expect("a musl build");

    assert_eq!(gnu.requirements().libc(), Some(Libc::Gnu));
    assert_eq!(musl.requirements().libc(), Some(Libc::Musl));
    assert_ne!(gnu.archive_digest(), musl.archive_digest());
}

#[test]
fn a_machine_with_no_known_c_library_is_told_what_the_builds_need() {
    // Every Linux build names a library, so a target linked against neither is
    // refused before anything is downloaded. The alternative is a hundred
    // megabytes fetched and a loader error.
    //
    // What the message says matters as much as that there is one. "No tested
    // release for linux-x86_64" is true and useless when linux-x86_64 is
    // pinned four times over: it reads as "nessa does not support my
    // computer", when the fact is that nessa could not tell which of four
    // builds fits it. So the message has to name both halves — what this
    // machine is, and what the builds that exist ask for.
    let host = machine("linux", "x86_64", None, true);

    let message = pinned(&opencode(), &host)
        .expect_err("no build is known to run here")
        .to_string();

    // Checked first, and separately, because it is the one failure the whole
    // sentence below would report as an unreadable diff. A wrapped literal
    // whose continuation is lost still contains every phrase anything here
    // looks for, so a message asserted piece by piece goes on passing with the
    // source indentation printed in the middle of it.
    assert!(
        !message.contains("  "),
        "the message carries its own source indentation: {message}"
    );
    // And then the whole of it, rather than the phrases it happens to contain:
    // what this test is about is a person being able to read the refusal, and
    // no set of `contains` checks says whether the sentence reads.
    assert_eq!(
        message,
        "agent setup failed: nessa has no tested opencode release this machine can run: \
         it is linux-x86_64 with no c library nessa could name and avx2, \
         and the opencode builds for linux-x86_64 need \
         gnu and avx2, or gnu, or musl and avx2, or musl"
    );
}

#[test]
fn a_platform_nessa_pins_nothing_for_is_told_so_plainly() {
    // The other shape of the same refusal, and the one where naming what the
    // builds need would be nonsense: there are none.
    let host = machine("plan9", "sparc64", None, false);

    let message = pinned(&opencode(), &host)
        .expect_err("plan9 is not a pinned platform")
        .to_string();

    assert!(
        message.contains("no tested opencode release") && message.contains("plan9"),
        "unhelpful message: {message}"
    );
}

#[test]
fn the_more_demanding_build_is_preferred_whichever_order_it_is_listed_in() {
    // A machine with AVX2 runs both builds, so this is a preference rather than
    // a filter — and a preference is only a preference if it survives the list
    // being the other way round. Asked of both orders, because the pin file's
    // own order already happens to put the demanding build first, and a
    // "preference" that is really "the first entry that matched" would pass
    // against it and fail on the next regenerated file.
    let host = machine("linux", "x86_64", Some(Libc::Gnu), true);
    let fast = build(Some(Libc::Gnu), true, 'a');
    let baseline = build(Some(Libc::Gnu), false, 'b');

    for (named, releases) in [
        (
            "the demanding build first",
            vec![fast.clone(), baseline.clone()],
        ),
        (
            "the baseline build first",
            vec![baseline.clone(), fast.clone()],
        ),
    ] {
        let chosen = preferred(releases, &host).expect("both builds run here");
        assert_eq!(
            chosen.archive_digest(),
            fast.archive_digest(),
            "{named}: the faster build was passed over"
        );
    }
}

#[test]
fn a_machine_that_cannot_take_the_demanding_build_gets_the_other_one() {
    let host = machine("linux", "x86_64", Some(Libc::Gnu), false);
    let fast = build(Some(Libc::Gnu), true, 'a');
    let baseline = build(Some(Libc::Gnu), false, 'b');

    let chosen = preferred(vec![fast, baseline.clone()], &host).expect("one build runs here");

    assert_eq!(chosen.archive_digest(), baseline.archive_digest());
}

#[test]
fn a_machine_no_build_runs_on_is_offered_none() {
    let host = machine("linux", "x86_64", Some(Libc::Musl), true);

    assert_eq!(
        preferred(
            vec![
                build(Some(Libc::Gnu), true, 'a'),
                build(Some(Libc::Gnu), false, 'b')
            ],
            &host
        ),
        None
    );
}

/// One Linux x86-64 build, told apart from its siblings by its digest.
fn build(libc: Option<Libc>, avx2: bool, digest_byte: char) -> PinnedRelease {
    PinnedRelease::new(
        ReleaseVersion::parse("1.18.31").expect("usable version"),
        ReleasePlatform::new("linux", "x86_64").expect("usable platform"),
        ReleaseRequirements::new(libc, avx2),
        ArchiveUrl::parse("https://registry.example/runtime.tgz").expect("a fetchable url"),
        digest(digest_byte),
        ArchivePath::parse("package/bin/opencode").expect("contained path"),
    )
}
