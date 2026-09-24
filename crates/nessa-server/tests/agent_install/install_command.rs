use super::*;
use crate::agent_install::application::{
    AuditAcknowledgement, InstallAudit, InstallDeliveryFailure, InstallDeliveryFailureStage,
    InstallationDelivery, InstallationDeliverySession, InstalledRuntime, StoreFailure,
};
use crate::agent_install::domain::{
    ArchiveDigest, ArchiveRejected, ArchiveSize, ArchiveUrl, InstallAttempt, InstallTransition,
    Libc, PinnedRelease, PublicationPreparation, ReleasePlatform, ReleaseRequirements,
    ReleaseVersion, RuntimeArtifact,
};
use crate::agent_install_test_support::{
    agent, audit, host, installs, platform, reclamation_audit, release as test_release, request,
    temporary_root, FakeSource, FakeStore, PINNED_DIGEST,
};
use nessa_local_storage::create_directory;
#[cfg(unix)]
use std::{
    ffi::OsStr,
    fs::Permissions,
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, PermissionsExt},
    },
};
use std::{fs, path::Path};

fn opencode() -> AgentName {
    AgentName::parse("opencode").expect("a plain agent name")
}

fn digest(byte: char) -> ArchiveDigest {
    ArchiveDigest::parse(&std::iter::repeat_n(byte, 64).collect::<String>())
        .expect("a run of one hex character is a digest")
}

fn rejection() -> ArchiveRejected {
    // macOS aarch64, rather than whatever this machine is. What this fixture
    // is about is a digest that did not match, which has nothing to do with
    // the platform — and reading the real one made the fixture depend on the
    // suite's own host: on Linux it assembled a release naming no C library,
    // which is a build that cannot exist.
    PinnedRelease::new(
        ReleaseVersion::parse("1.0.0").expect("usable version"),
        ReleasePlatform::new("macos", "aarch64").expect("usable platform"),
        ReleaseRequirements::default(),
        ArchiveUrl::parse("https://registry.example/runtime.tgz").expect("a fetchable url"),
        ArchiveSize::parse(46_009_615).expect("a measured archive"),
        digest('a'),
        installs("package/bin/opencode"),
    )
    .expect("a release whose requirements fit its platform")
    .accept(&digest('b'))
    .expect_err("another archive is not the pinned one")
}

fn install_attempt() -> (InstallAttempt, InstallTransition) {
    let release = PinnedRelease::new(
        ReleaseVersion::parse("1.0.0").expect("usable version"),
        ReleasePlatform::new("macos", "aarch64").expect("usable platform"),
        ReleaseRequirements::default(),
        ArchiveUrl::parse("https://registry.example/runtime.tgz").expect("a fetchable url"),
        ArchiveSize::parse(46_009_615).expect("a measured archive"),
        digest('a'),
        installs("package/bin/opencode"),
    )
    .expect("a release whose requirements fit its platform");
    InstallAttempt::start(
        opencode(),
        RuntimeArtifact::for_release(&release),
        request(),
    )
}

struct RefusingDelivery;

impl InstallationDelivery for RefusingDelivery {
    fn session(
        &self,
        _account_id: &str,
    ) -> Result<Box<dyn InstallationDeliverySession + '_>, InstallDeliveryFailure> {
        Err(InstallDeliveryFailure::new(
            InstallDeliveryFailureStage::ReadState,
            "injected uncertain delivery state".into(),
        ))
    }
}

#[test]
fn audit_factory_creates_a_missing_selected_namespace_and_reopens_its_journal() {
    let temporary = tempfile::tempdir().unwrap();
    let data_root = temporary.path().join("data/ci/instances/install-e2e");
    assert!(!data_root.exists());

    let audit = install_audit(&data_root).expect("the selected namespace is initialized");
    let (mut attempt, started) = install_attempt();
    assert_eq!(
        audit.record(started.clone()).unwrap(),
        AuditAcknowledgement::Recorded
    );
    drop(audit);

    let reopened = install_audit(&data_root).expect("the existing private namespace reopens");
    assert_eq!(
        reopened.record(started).unwrap(),
        AuditAcknowledgement::Replayed
    );
    assert_eq!(
        reopened.record(attempt.verified().unwrap()).unwrap(),
        AuditAcknowledgement::Recorded
    );
    let journal = data_root.join("audit/agent-install");
    assert!(journal.join("audit.lock").is_file());
    assert!(journal.join("00000000000000000001.json").is_file());
    assert!(journal.join("00000000000000000002.json").is_file());
    assert!(!temporary.path().join("audit").exists());
    #[cfg(unix)]
    {
        let audit_directory = data_root.join("audit");
        for directory in [
            data_root.as_path(),
            audit_directory.as_path(),
            journal.as_path(),
        ] {
            assert_eq!(
                fs::metadata(directory).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
    }
}

#[test]
fn audit_factory_reuses_the_exact_existing_private_namespace() {
    let temporary = tempfile::tempdir().unwrap();
    let data_root = temporary.path().join("selected");
    create_directory(&data_root).unwrap();
    let canonical_before = fs::canonicalize(&data_root).unwrap();
    #[cfg(unix)]
    let before = fs::metadata(&data_root).unwrap();

    let _audit = install_audit(&data_root).expect("an existing private namespace is valid");
    let after = fs::metadata(&data_root).unwrap();

    assert_eq!(fs::canonicalize(&data_root).unwrap(), canonical_before);
    assert!(after.is_dir());
    #[cfg(unix)]
    assert_eq!((before.dev(), before.ino()), (after.dev(), after.ino()));
}

#[test]
fn delivery_factory_creates_its_separate_private_namespace() {
    let temporary = tempfile::tempdir().unwrap();
    let data_root = temporary.path().join("data/ci/instances/install-e2e");

    let _delivery = install_delivery(&data_root).expect("delivery state initializes");

    assert!(data_root
        .join("installation-delivery/agent-install/delivery.lock")
        .is_file());
    assert!(!data_root.join("audit/agent-install").exists());
}

#[test]
fn audit_factory_rejects_a_file_as_the_selected_namespace_without_runtime_effects() {
    let temporary = tempfile::tempdir().unwrap();
    let data_root = temporary.path().join("selected");
    fs::write(&data_root, b"not a directory").unwrap();

    assert!(install_audit(&data_root).is_err());
    assert_eq!(fs::read(&data_root).unwrap(), b"not a directory");
    assert!(!temporary.path().join("agents").exists());
    assert!(!temporary.path().join("audit").exists());
}

#[cfg(unix)]
#[test]
fn install_stops_at_a_namespace_initialization_failure_before_runtime_effects() {
    let temporary = tempfile::tempdir().unwrap();
    let data_root = temporary.path().join("selected");
    let runtime_root = data_root.join("agents");
    fs::write(&data_root, b"not a directory").unwrap();

    let failure = install(&opencode(), &runtime_root, &request())
        .expect_err("an unsafe selected namespace stops installation");

    assert!(failure.to_string().contains("agent setup failed"));
    assert_eq!(fs::read(&data_root).unwrap(), b"not a directory");
    assert!(!runtime_root.exists());
    assert!(!temporary.path().join("audit").exists());
}

#[cfg(unix)]
#[test]
fn audit_factory_rejects_linked_or_non_private_namespaces_without_outside_writes() {
    let temporary = tempfile::tempdir().unwrap();
    let outside = temporary.path().join("outside");
    create_directory(&outside).unwrap();
    let linked = temporary.path().join("linked");
    std::os::unix::fs::symlink(&outside, &linked).unwrap();
    assert!(install_audit(&linked).is_err());
    assert!(fs::read_dir(&outside).unwrap().next().is_none());

    let public = temporary.path().join("public");
    fs::create_dir(&public).unwrap();
    fs::set_permissions(&public, Permissions::from_mode(0o755)).unwrap();
    assert!(install_audit(&public).is_err());
    assert!(fs::read_dir(&public).unwrap().next().is_none());
}

#[cfg(unix)]
#[test]
fn audit_factory_rejects_a_nested_audit_link_without_writing_outside() {
    let temporary = tempfile::tempdir().unwrap();
    let data_root = temporary.path().join("selected");
    let outside = temporary.path().join("outside");
    create_directory(&data_root).unwrap();
    create_directory(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, data_root.join("audit")).unwrap();

    assert!(install_audit(&data_root).is_err());
    assert!(fs::read_dir(&outside).unwrap().next().is_none());
    assert!(!data_root.join("agent-install").exists());
}

#[test]
fn audit_factory_keeps_selected_namespaces_isolated() {
    let temporary = tempfile::tempdir().unwrap();
    let first_root = temporary.path().join("first");
    let second_root = temporary.path().join("second");
    let first = install_audit(&first_root).unwrap();
    let _second = install_audit(&second_root).unwrap();
    let (_, started) = install_attempt();

    first.record(started).unwrap();

    assert!(first_root
        .join("audit/agent-install/00000000000000000001.json")
        .is_file());
    assert!(!second_root
        .join("audit/agent-install/00000000000000000001.json")
        .exists());
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
            tokio::task::spawn_blocking(move || install(&opencode(), &root, &request())).await
        })
        .expect("the installer finishes")
        .expect("the pinned release installs");

    assert!(installed.downloaded);
    assert!(installed.executable.is_file());
}

#[test]
fn an_agent_nessa_does_not_install_is_named_as_such() {
    // A name the pin file says nothing about is a mistake worth a clear answer
    // rather than a download that fails obscurely — and it must not touch the
    // network to say so. Claude used to be the example here and is not one any
    // more: all three agents Nessa drives are pinned.
    let root = temporary_root();
    let unknown = AgentName::parse("gemini").expect("a plain agent name");
    let failure =
        install(&unknown, root.path(), &request()).expect_err("gemini is not installed by nessa");
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
    let unknown = AgentName::parse("gemini").expect("a plain agent name");
    let _ = install(&unknown, root.path(), &request());
    assert!(
        !root.path().join("gemini").exists(),
        "a refused install left a directory behind"
    );
    assert!(
        !root.path().parent().unwrap().join("audit").exists(),
        "pin refusal initialized audit storage"
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
        InstallFailure::Store(StoreFailure::IncompleteArchive(
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
fn publication_uncertainty_says_that_later_installs_are_blocked() {
    let root = tempfile::tempdir().unwrap();
    let source = FakeSource::serving(b"archive bytes");
    let store = FakeStore::empty(root.path());
    let release = test_release("1.18.31", PINNED_DIGEST, &platform());
    let delivery_failure = InstallAgentRuntime {
        reclamation_audit: reclamation_audit(),
        source: &source,
        store: &store,
        audit: audit(),
        delivery: &RefusingDelivery,
    }
    .execute(&agent(), &release, &host(), &request())
    .unwrap_err();
    let delivery_message = explain(&delivery_failure);
    assert!(delivery_message.contains("publication evidence is uncertain"));
    assert!(delivery_message.contains("new installs for this account are blocked"));
    assert!(delivery_message.contains("recovery succeeds"));

    let (mut attempt, _) = install_attempt();
    let preparation = PublicationPreparation::new(attempt.verified().unwrap()).unwrap();
    let unresolved = explain(&InstallFailure::UnresolvedPublication(Box::new(
        preparation,
    )));
    assert!(unresolved.contains("prior publication result is unknown"));
    assert!(unresolved.contains("new installs for this account are blocked"));
    assert!(unresolved.contains("exact outcome is recovered"));
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
        reclamation_warnings: Vec::new(),
    };
    let mut written = Vec::new();

    let report = report(&opencode(), &installed).expect("the path is text");
    write_report(&mut written, &report).expect("the report is written");

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
        reclamation_warnings: Vec::new(),
    };

    assert_eq!(
        report(&opencode(), &installed).expect("the path is text")["downloaded"],
        false
    );
}

/// A path that is not text is refused rather than reported as something else.
///
/// A path is bytes on Unix, and this one is built under a data directory taken
/// from `NESSA_DATA_DIR` or the home directory — neither of which has to be
/// valid UTF-8. Rendered lossily, every undecodable byte becomes U+FFFD and
/// the one machine-readable field in a *successful* report names a file that
/// does not exist. The caller then fails to launch it with nothing to go on,
/// which is the worst of the three possible outcomes; refusing is the one that
/// can be read and acted on.
///
/// Unix only: Windows paths are UTF-16 and `to_str` fails there on unpaired
/// surrogates, which is a different fault and not one this can construct.
#[cfg(unix)]
#[test]
fn a_runtime_whose_path_is_not_text_is_not_reported_as_a_path_that_is() {
    let installed = InstalledRuntime {
        version: ReleaseVersion::parse("1.18.31").expect("usable version"),
        // A lone 0x80 is a continuation byte with nothing to continue: a
        // directory name a filesystem accepts and UTF-8 does not.
        executable: Path::new(OsStr::from_bytes(b"/tmp/\x80/opencode")).to_owned(),
        downloaded: true,
        reclamation_warnings: Vec::new(),
    };

    let refused = report(&opencode(), &installed).expect_err("the path is not text");

    let message = refused.to_string();
    assert!(
        message.contains("not a path this command can report as text"),
        "the refusal does not say what is wrong: {message}"
    );
    // And it says the install itself is fine, because it is: a person told
    // only that the command failed would reasonably start over.
    assert!(
        message.contains("nothing else is wrong with the installation"),
        "the refusal reads as a failed install: {message}"
    );
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
    // Every x86-64 row has AVX2 because at 1.18.31 every x86-64 build needs it;
    // the machines that therefore get nothing are
    // `a_processor_without_avx2_is_told_no_tested_build_fits_it` below.
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
            Some(Libc::Musl),
            true,
            "opencode-linux-x64-musl",
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
fn a_processor_without_avx2_is_told_no_tested_build_fits_it() {
    // At 1.18.31 there is no x86-64 Opencode Nessa will promise runs here. The
    // vendor names three `-baseline` packages for this machine and every one of
    // them holds the same bytes as its AVX2 sibling, so what those bytes need
    // is unsettled and none is pinned; the pin file's own side of that is
    // `a_machine_without_avx2_is_offered_nothing_...`.
    //
    // This is the half a person sees. A refusal naming what this machine is and
    // what the builds ask for is a sentence somebody can act on; the failure
    // this replaces was a download followed by an illegal instruction.
    //
    // Which build a machine that *can* run either gets is
    // `the_more_demanding_of_two_builds_wins`, over releases built here rather
    // than through the pin file, because at this version the file no longer
    // holds such a pair to choose between.
    let host = machine("linux", "x86_64", Some(Libc::Gnu), false);

    let message = pinned(&opencode(), &host)
        .expect_err("no pinned build runs on a processor without avx2")
        .to_string();

    assert_eq!(
        message,
        "agent setup failed: nessa has no tested opencode release this machine can run: \
         it is linux-x86_64 with gnu and no avx2, \
         and the opencode builds for linux-x86_64 need \
         gnu and avx2, or musl and avx2"
    );
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
         gnu and avx2, or musl and avx2"
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
