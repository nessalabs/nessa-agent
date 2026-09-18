use super::*;
use crate::agent_install_test_support::{
    platform, release, FakeSource, FakeStore, OTHER_DIGEST, PINNED_DIGEST,
};

const AGENT: &str = "opencode";

#[test]
fn a_matching_archive_is_published() {
    let root = tempfile::tempdir().expect("temporary root");
    let source = FakeSource::serving(b"archive bytes");
    let store = FakeStore::empty(root.path());
    let platform = platform();

    let installed = InstallAgentRuntime {
        source: &source,
        store: &store,
    }
    .execute(
        AGENT,
        &release("1.18.31", PINNED_DIGEST, &platform),
        &platform,
    )
    .expect("a matching archive installs");

    assert_eq!(installed.version.as_str(), "1.18.31");
    assert!(installed.downloaded);
    assert_eq!(store.published(), vec![AGENT.to_string()]);
}

#[test]
fn a_mismatched_archive_is_never_unpacked() {
    // The whole point of the pin. A download that is not the tested one must
    // not have its contents read, let alone placed somewhere Nessa will launch
    // from — so `publish` must not have been called at all.
    let root = tempfile::tempdir().expect("temporary root");
    let source = FakeSource::serving(b"someone else's bytes");
    let store = FakeStore::empty(root.path()).hashing(OTHER_DIGEST);
    let platform = platform();

    let failure = InstallAgentRuntime {
        source: &source,
        store: &store,
    }
    .execute(
        AGENT,
        &release("1.18.31", PINNED_DIGEST, &platform),
        &platform,
    )
    .expect_err("a mismatched archive is refused");

    match failure {
        InstallFailure::Rejected(rejection) => {
            assert_eq!(rejection.expected.as_str(), PINNED_DIGEST);
            assert_eq!(rejection.actual.as_str(), OTHER_DIGEST);
        }
        other => panic!("expected a rejection, got {other:?}"),
    }
    assert!(
        store.published().is_empty(),
        "nothing may be published from an archive that was not the pinned one"
    );
}

#[test]
fn a_rejected_archive_is_discarded() {
    // A refused download is the largest thing this operation writes. Leaving it
    // behind would let a run of failures fill the disk.
    let root = tempfile::tempdir().expect("temporary root");
    let source = FakeSource::serving(b"someone else's bytes");
    let store = FakeStore::empty(root.path()).hashing(OTHER_DIGEST);
    let platform = platform();

    let _ = InstallAgentRuntime {
        source: &source,
        store: &store,
    }
    .execute(
        AGENT,
        &release("1.18.31", PINNED_DIGEST, &platform),
        &platform,
    );

    assert_eq!(store.discarded().len(), 1, "the archive is discarded");
}

#[test]
fn a_successful_install_discards_its_archive_too() {
    let root = tempfile::tempdir().expect("temporary root");
    let source = FakeSource::serving(b"archive bytes");
    let store = FakeStore::empty(root.path());
    let platform = platform();

    InstallAgentRuntime {
        source: &source,
        store: &store,
    }
    .execute(
        AGENT,
        &release("1.18.31", PINNED_DIGEST, &platform),
        &platform,
    )
    .expect("a matching archive installs");

    assert_eq!(store.discarded().len(), 1);
}

#[test]
fn installing_what_is_already_installed_downloads_nothing() {
    let root = tempfile::tempdir().expect("temporary root");
    let source = FakeSource::serving(b"archive bytes");
    let store = FakeStore::holding(root.path(), "1.18.31");
    let platform = platform();

    let installed = InstallAgentRuntime {
        source: &source,
        store: &store,
    }
    .execute(
        AGENT,
        &release("1.18.31", PINNED_DIGEST, &platform),
        &platform,
    )
    .expect("an installed runtime is reported as installed");

    assert!(!installed.downloaded);
    assert!(
        source.requested().is_empty(),
        "a runtime already at the pinned version is not fetched again"
    );
    assert!(store.published().is_empty());
}

#[test]
fn a_different_installed_version_is_replaced() {
    // The pin moving is an install. An older runtime sitting there is not a
    // reason to skip the one Nessa has now tested.
    let root = tempfile::tempdir().expect("temporary root");
    let source = FakeSource::serving(b"archive bytes");
    let store = FakeStore::holding(root.path(), "1.17.0");
    let platform = platform();

    let installed = InstallAgentRuntime {
        source: &source,
        store: &store,
    }
    .execute(
        AGENT,
        &release("1.18.31", PINNED_DIGEST, &platform),
        &platform,
    )
    .expect("a newly pinned version installs over an older one");

    assert!(installed.downloaded);
    assert_eq!(installed.version.as_str(), "1.18.31");
    assert_eq!(source.requested().len(), 1);
}

#[test]
fn a_release_for_another_platform_is_refused_before_anything_is_fetched() {
    let root = tempfile::tempdir().expect("temporary root");
    let source = FakeSource::serving(b"archive bytes");
    let store = FakeStore::empty(root.path());
    let elsewhere = ReleasePlatform::new("linux", "x86_64").expect("usable platform");

    let failure = InstallAgentRuntime {
        source: &source,
        store: &store,
    }
    .execute(
        AGENT,
        &release("1.18.31", PINNED_DIGEST, &platform()),
        &elsewhere,
    )
    .expect_err("a release for another platform is refused");

    assert_eq!(failure, InstallFailure::UnsupportedPlatform(elsewhere));
    assert!(source.requested().is_empty());
}

#[test]
fn a_download_failure_is_reported_as_one() {
    let root = tempfile::tempdir().expect("temporary root");
    let source = FakeSource::failing(SourceFailure::Refused(404));
    let store = FakeStore::empty(root.path());
    let platform = platform();

    let failure = InstallAgentRuntime {
        source: &source,
        store: &store,
    }
    .execute(
        AGENT,
        &release("1.18.31", PINNED_DIGEST, &platform),
        &platform,
    )
    .expect_err("a refused download fails the install");

    assert_eq!(
        failure,
        InstallFailure::Download(SourceFailure::Refused(404))
    );
    assert!(store.published().is_empty());
}

#[test]
fn an_archive_without_the_pinned_executable_fails_as_a_store_problem() {
    // A verified archive that does not contain what the pin says it does is a
    // fault in the pin, and it must not be reported as a corrupted download —
    // retrying that would never help.
    let root = tempfile::tempdir().expect("temporary root");
    let source = FakeSource::serving(b"archive bytes");
    let missing = StoreFailure::MissingExecutable("package/bin/opencode".into());
    let store = FakeStore::empty(root.path()).failing_to_publish(missing.clone());
    let platform = platform();

    let failure = InstallAgentRuntime {
        source: &source,
        store: &store,
    }
    .execute(
        AGENT,
        &release("1.18.31", PINNED_DIGEST, &platform),
        &platform,
    )
    .expect_err("an archive missing its executable fails the install");

    assert_eq!(failure, InstallFailure::Store(missing));
}

#[test]
fn a_store_that_cannot_say_what_is_installed_does_not_download() {
    // "Could not tell" is not "nothing is installed". Downloading on the
    // strength of an unanswered question would replace a working runtime
    // because a directory happened to be unreadable for a moment.
    let root = tempfile::tempdir().expect("temporary root");
    let source = FakeSource::serving(b"archive bytes");
    let unreadable = StoreFailure::Unreadable("runtime directory".into());
    let store = FakeStore::empty(root.path()).failing_to_read(unreadable.clone());
    let platform = platform();

    let failure = InstallAgentRuntime {
        source: &source,
        store: &store,
    }
    .execute(
        AGENT,
        &release("1.18.31", PINNED_DIGEST, &platform),
        &platform,
    )
    .expect_err("an unreadable store fails the install");

    assert_eq!(failure, InstallFailure::Store(unreadable));
    assert!(source.requested().is_empty());
}

#[test]
fn the_pinned_url_is_what_gets_fetched() {
    let root = tempfile::tempdir().expect("temporary root");
    let source = FakeSource::serving(b"archive bytes");
    let store = FakeStore::empty(root.path());
    let platform = platform();
    let release = release("1.18.31", PINNED_DIGEST, &platform);

    InstallAgentRuntime {
        source: &source,
        store: &store,
    }
    .execute(AGENT, &release, &platform)
    .expect("a matching archive installs");

    assert_eq!(source.requested(), vec![release.archive_url().to_string()]);
}
