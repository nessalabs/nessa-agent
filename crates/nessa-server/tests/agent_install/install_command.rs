use super::*;
use std::path::Path;

use crate::agent_install::application::{InstalledRuntime, StoreFailure};
use crate::agent_install::domain::{
    ArchiveDigest, ArchivePath, ArchiveRejected, ArchiveUrl, PinnedRelease, ReleaseVersion,
};

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
        host_platform(),
        ArchiveUrl::parse("https://registry.example/runtime.tgz").expect("a fetchable url"),
        digest('a'),
        ArchivePath::parse("package/bin/opencode").expect("contained path"),
    )
    .accept(&digest('b'))
    .expect_err("another archive is not the pinned one")
}

/// Install the pinned Opencode release the way the command really does it:
/// through the async runtime this process starts with.
///
/// Ignored by default alongside the other live test — it fetches the real
/// archive. It exists because the unit tests cannot catch the one thing that is
/// only true at runtime: the HTTP client used here refuses to run inside a
/// Tokio runtime, so `install` being reached through `spawn_blocking` rather
/// than directly is load-bearing, and a refactor that inlined it would panic in
/// production while every other test stayed green.
///
/// ```text
/// cargo test -p nessa-server --lib -- --ignored installs_from_inside_the_runtime
/// ```
#[test]
#[ignore = "downloads the real release archive"]
fn installs_from_inside_the_runtime() {
    let root = tempfile::tempdir().expect("temporary root");
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
    let root = tempfile::tempdir().expect("temporary root");
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
    let elsewhere = ReleasePlatform::new("plan9", "sparc64").expect("usable platform");
    let failure = pinned(&opencode(), &elsewhere).expect_err("plan9 is not a pinned platform");
    let message = failure.to_string();
    assert!(
        message.contains("no tested opencode release") && message.contains("plan9"),
        "unhelpful message: {message}"
    );
}

#[test]
fn nothing_is_written_for_an_agent_with_no_release() {
    let root = tempfile::tempdir().expect("temporary root");
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
        InstallFailure::UnsupportedPlatform(
            ReleasePlatform::new("windows", "x86_64").expect("usable platform"),
        ),
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
