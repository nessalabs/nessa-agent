//! Stand-ins for the network and the disk, so that installing an agent runtime
//! can be tested without fetching a hundred megabytes or writing to the
//! developer's own machine.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::agent_install::application::{
    ArchiveSource, InstalledRecord, RuntimeStore, SourceFailure, StoreFailure,
};
use crate::agent_install::domain::{
    ArchiveDigest, ArchivePath, PinnedRelease, ReleasePlatform, ReleaseVersion,
};

/// The digest of an archive no test ever produces, used wherever a test needs a
/// pin that the downloaded bytes will not match.
pub(crate) const OTHER_DIGEST: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

/// A digest standing for whatever the fake store will say it hashed.
pub(crate) const PINNED_DIGEST: &str =
    "1111111111111111111111111111111111111111111111111111111111111111";

/// A release pinned for the platform the test says it is running on.
pub(crate) fn release(version: &str, digest: &str, platform: &ReleasePlatform) -> PinnedRelease {
    PinnedRelease::new(
        ReleaseVersion::parse(version).expect("test version is usable"),
        platform.clone(),
        "https://example.invalid/runtime.tgz",
        ArchiveDigest::parse(digest).expect("test digest is usable"),
        ArchivePath::parse("package/bin/opencode").expect("test path is contained"),
    )
    .expect("test release is well formed")
}

/// The platform these tests pretend to run on, so that a result never depends
/// on the machine running the suite.
pub(crate) fn platform() -> ReleasePlatform {
    ReleasePlatform::new("macos", "aarch64").expect("test platform is well formed")
}

/// What the fake source was asked to do, and what it did.
#[derive(Debug, Default)]
pub(crate) struct Downloads {
    pub(crate) urls: Vec<String>,
}

/// A source that writes a fixed body, or fails, and remembers being asked.
pub(crate) struct FakeSource {
    body: Result<Vec<u8>, SourceFailure>,
    calls: Mutex<Downloads>,
}

impl FakeSource {
    /// A source that succeeds, writing `body` wherever it is pointed.
    pub(crate) fn serving(body: &[u8]) -> Self {
        Self {
            body: Ok(body.to_vec()),
            calls: Mutex::new(Downloads::default()),
        }
    }

    /// A source that always fails the same way.
    pub(crate) fn failing(failure: SourceFailure) -> Self {
        Self {
            body: Err(failure),
            calls: Mutex::new(Downloads::default()),
        }
    }

    /// Every URL this source was asked for, in order.
    pub(crate) fn requested(&self) -> Vec<String> {
        self.calls.lock().expect("fake source lock").urls.clone()
    }
}

impl ArchiveSource for FakeSource {
    fn download(&self, url: &str, destination: &Path) -> Result<(), SourceFailure> {
        self.calls
            .lock()
            .expect("fake source lock")
            .urls
            .push(url.to_owned());
        let body = self.body.clone()?;
        std::fs::write(destination, body).expect("fake source writes to a temporary directory");
        Ok(())
    }
}

/// What the fake store was asked to do.
#[derive(Debug, Default)]
pub(crate) struct StoreCalls {
    pub(crate) published: Vec<String>,
    pub(crate) discarded: Vec<PathBuf>,
}

/// A store backed by a temporary directory, with its answers fixed in advance.
///
/// It hashes nothing: `digest` returns whatever the test said the download
/// hashes to. That keeps these tests about the *ordering* — that publishing
/// never happens after a rejection — rather than about SHA-256, which the
/// adapter's own test covers against a real archive.
pub(crate) struct FakeStore {
    root: PathBuf,
    installed: Result<Option<InstalledRecord>, StoreFailure>,
    digest: Result<ArchiveDigest, StoreFailure>,
    publish: Result<PathBuf, StoreFailure>,
    calls: Mutex<StoreCalls>,
}

impl FakeStore {
    /// A store with nothing installed that publishes successfully.
    pub(crate) fn empty(root: &Path) -> Self {
        Self {
            root: root.to_owned(),
            installed: Ok(None),
            digest: Ok(ArchiveDigest::parse(PINNED_DIGEST).expect("test digest is usable")),
            publish: Ok(root.join("opencode")),
            calls: Mutex::new(StoreCalls::default()),
        }
    }

    /// A store already holding `version`, with its executable present.
    pub(crate) fn holding(root: &Path, version: &str) -> Self {
        let executable = root.join("opencode");
        std::fs::write(&executable, b"installed").expect("fake store writes to a temporary root");
        Self {
            installed: Ok(Some(InstalledRecord {
                version: ReleaseVersion::parse(version).expect("test version is usable"),
                executable,
            })),
            ..Self::empty(root)
        }
    }

    /// The same store, but the download hashes to something else.
    pub(crate) fn hashing(mut self, digest: &str) -> Self {
        self.digest = Ok(ArchiveDigest::parse(digest).expect("test digest is usable"));
        self
    }

    /// The same store, but publishing fails.
    pub(crate) fn failing_to_publish(mut self, failure: StoreFailure) -> Self {
        self.publish = Err(failure);
        self
    }

    /// The same store, but it cannot say what is installed.
    pub(crate) fn failing_to_read(mut self, failure: StoreFailure) -> Self {
        self.installed = Err(failure);
        self
    }

    /// Every agent this store was asked to publish, in order.
    pub(crate) fn published(&self) -> Vec<String> {
        self.calls
            .lock()
            .expect("fake store lock")
            .published
            .clone()
    }

    /// Every scratch path this store was asked to discard.
    pub(crate) fn discarded(&self) -> Vec<PathBuf> {
        self.calls
            .lock()
            .expect("fake store lock")
            .discarded
            .clone()
    }
}

impl RuntimeStore for FakeStore {
    fn installed(&self, _agent: &str) -> Result<Option<InstalledRecord>, StoreFailure> {
        self.installed.clone()
    }

    fn scratch(&self, agent: &str) -> Result<PathBuf, StoreFailure> {
        Ok(self.root.join(format!("{agent}.download")))
    }

    fn digest(&self, _archive: &Path) -> Result<ArchiveDigest, StoreFailure> {
        self.digest.clone()
    }

    fn publish(
        &self,
        agent: &str,
        _release: &PinnedRelease,
        _archive: &Path,
    ) -> Result<PathBuf, StoreFailure> {
        self.calls
            .lock()
            .expect("fake store lock")
            .published
            .push(agent.to_owned());
        self.publish.clone()
    }

    fn discard(&self, scratch: &Path) {
        self.calls
            .lock()
            .expect("fake store lock")
            .discarded
            .push(scratch.to_owned());
    }
}
