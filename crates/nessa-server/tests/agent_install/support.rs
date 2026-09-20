//! Stand-ins for the network and the disk, so that installing an agent runtime
//! can be tested without fetching a hundred megabytes or writing to the
//! developer's own machine.

use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::agent_install::application::{
    ArchiveSource, RuntimeStore, SourceFailure, StagedArchive, StoreFailure,
};
use crate::agent_install::domain::{
    AgentName, ArchiveDigest, ArchivePath, ArchiveUrl, HostPlatform, Libc, PinnedRelease,
    ReleasePlatform, ReleaseRequirements, ReleaseVersion,
};

/// The digest of an archive no test ever produces, used wherever a test needs a
/// pin that the downloaded bytes will not match.
pub(crate) const OTHER_DIGEST: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

/// A digest standing for whatever the fake store will say it hashed.
pub(crate) const PINNED_DIGEST: &str =
    "1111111111111111111111111111111111111111111111111111111111111111";

/// A store root that is private, the way the real one is.
///
/// `tempfile::tempdir` creates a directory the umask decides, which on most
/// machines anybody can read. The real root is `<data directory>/agents`,
/// created by `create_directory` and so private to its owner — and
/// [`ManagedRuntimes`] insists on that, because every path beneath the root is
/// reached by walking down from it and refusing anything that is not a private
/// directory of this user's. A bare temporary directory would fail at the
/// first step, for a reason that has nothing to do with what the test is
/// about.
///
/// The temporary directory is kept alive by this value; dropping it takes the
/// root with it.
///
/// [`ManagedRuntimes`]: crate::agent_install::infrastructure::ManagedRuntimes
pub(crate) struct TemporaryRoot {
    _temporary: tempfile::TempDir,
    root: PathBuf,
}

impl TemporaryRoot {
    /// The root itself, which is what a store is built on.
    pub(crate) fn path(&self) -> &Path {
        &self.root
    }
}

/// A private root inside a temporary directory that goes away with it.
pub(crate) fn temporary_root() -> TemporaryRoot {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let root = temporary.path().join("agents");
    nessa_local_storage::create_directory(&root).expect("a private store root");
    TemporaryRoot {
        _temporary: temporary,
        root,
    }
}

/// The agent these tests install.
pub(crate) fn agent() -> AgentName {
    AgentName::parse("opencode").expect("test agent name is plain")
}

/// A release pinned for the platform the test says it is running on, asking
/// nothing of the machine beyond that.
pub(crate) fn release(version: &str, digest: &str, platform: &ReleasePlatform) -> PinnedRelease {
    release_needing(version, digest, platform, ReleaseRequirements::default())
}

/// The same, for a build that needs a particular C library or processor.
pub(crate) fn release_needing(
    version: &str,
    digest: &str,
    platform: &ReleasePlatform,
    requirements: ReleaseRequirements,
) -> PinnedRelease {
    PinnedRelease::new(
        ReleaseVersion::parse(version).expect("test version is usable"),
        platform.clone(),
        requirements,
        ArchiveUrl::parse("https://example.invalid/runtime.tgz").expect("test url is fetchable"),
        ArchiveDigest::parse(digest).expect("test digest is usable"),
        ArchivePath::parse("package/bin/opencode").expect("test path is contained"),
    )
}

/// The platform these tests pretend to run on, so that a result never depends
/// on the machine running the suite.
pub(crate) fn platform() -> ReleasePlatform {
    ReleasePlatform::new("macos", "aarch64").expect("test platform is well formed")
}

/// The machine these tests pretend to be: [`platform`], with nothing optional.
pub(crate) fn host() -> HostPlatform {
    host_of(&platform(), None, false)
}

/// A machine described in full, for the tests that are about the choosing.
pub(crate) fn host_of(platform: &ReleasePlatform, libc: Option<Libc>, avx2: bool) -> HostPlatform {
    HostPlatform::new(platform.clone(), libc, avx2)
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
    /// A source that succeeds, writing `body` into whatever it is handed.
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
    fn download(&self, url: &str, staged: &mut StagedArchive) -> Result<(), SourceFailure> {
        self.calls
            .lock()
            .expect("fake source lock")
            .urls
            .push(url.to_owned());
        let body = self.body.clone()?;
        staged
            .file_mut()
            .write_all(&body)
            .expect("fake source writes to a temporary file");
        Ok(())
    }
}

/// What the fake store was asked to do.
#[derive(Debug, Default)]
pub(crate) struct StoreCalls {
    pub(crate) published: Vec<String>,
    pub(crate) discarded: Vec<PathBuf>,
    pub(crate) measured: Vec<Vec<u8>>,
    pub(crate) unpacked: Vec<Vec<u8>>,
}

/// A store backed by a temporary directory, with its answers fixed in advance.
///
/// It hashes nothing: `digest` returns whatever the test said the download
/// hashes to. That keeps these tests about the *ordering* — that publishing
/// never happens after a rejection — rather than about SHA-256, which the
/// adapter's own test covers against a real archive.
///
/// It does read the staged file at both steps, though, and remembers what it
/// read. Tests that care can then assert the thing the ordering is for: that
/// the bytes measured and the bytes unpacked are the same bytes.
pub(crate) struct FakeStore {
    root: PathBuf,
    installed: Result<Option<PathBuf>, StoreFailure>,
    stage: Option<StoreFailure>,
    digest: Result<ArchiveDigest, StoreFailure>,
    publish: Result<PathBuf, StoreFailure>,
    /// How many archives this store has staged, so each gets its own name.
    staged: std::sync::atomic::AtomicUsize,
    calls: Mutex<StoreCalls>,
}

impl FakeStore {
    /// A store with nothing installed that publishes successfully.
    pub(crate) fn empty(root: &Path) -> Self {
        Self {
            root: root.to_owned(),
            installed: Ok(None),
            stage: None,
            digest: Ok(ArchiveDigest::parse(PINNED_DIGEST).expect("test digest is usable")),
            publish: Ok(root.join("opencode")),
            staged: std::sync::atomic::AtomicUsize::new(0),
            calls: Mutex::new(StoreCalls::default()),
        }
    }

    /// A store already holding the release under test, with its executable
    /// present.
    pub(crate) fn holding(root: &Path) -> Self {
        let executable = root.join("opencode");
        std::fs::write(&executable, b"installed").expect("fake store writes to a temporary root");
        Self {
            installed: Ok(Some(executable)),
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

    /// The same store, but it cannot make a file to download into.
    pub(crate) fn failing_to_stage(mut self, failure: StoreFailure) -> Self {
        self.stage = Some(failure);
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

    /// Every staged file this store was asked to discard.
    pub(crate) fn discarded(&self) -> Vec<PathBuf> {
        self.calls
            .lock()
            .expect("fake store lock")
            .discarded
            .clone()
    }

    /// What this store read when it was asked to hash something.
    pub(crate) fn measured(&self) -> Vec<Vec<u8>> {
        self.calls.lock().expect("fake store lock").measured.clone()
    }

    /// What this store read when it was asked to unpack something.
    pub(crate) fn unpacked(&self) -> Vec<Vec<u8>> {
        self.calls.lock().expect("fake store lock").unpacked.clone()
    }

    fn read(&self, staged: &mut StagedArchive) -> Vec<u8> {
        let file = staged.file_mut();
        file.rewind().expect("a staged file can be rewound");
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).expect("a staged file reads");
        bytes
    }
}

impl RuntimeStore for FakeStore {
    fn installed(
        &self,
        _agent: &AgentName,
        _release: &PinnedRelease,
    ) -> Result<Option<PathBuf>, StoreFailure> {
        self.installed.clone()
    }

    fn stage(&self, agent: &AgentName) -> Result<StagedArchive, StoreFailure> {
        if let Some(failure) = self.stage.clone() {
            return Err(failure);
        }
        // A name of its own each time, created rather than opened, the way the
        // real store does it. `StagedArchive`'s doc makes that a promise of the
        // port — two installs at once get two files, and neither can truncate a
        // download the other is still measuring — and a fake that staged over
        // one fixed path would be modelling the thing the real one exists to
        // prevent.
        let staged = self
            .staged
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = self.root.join(format!("{agent}.{staged}.download"));
        let file = std::fs::File::options()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .expect("fake store stages in a temporary root");
        Ok(StagedArchive::new(file, path))
    }

    fn digest(&self, staged: &mut StagedArchive) -> Result<ArchiveDigest, StoreFailure> {
        let bytes = self.read(staged);
        self.calls
            .lock()
            .expect("fake store lock")
            .measured
            .push(bytes);
        self.digest.clone()
    }

    fn publish(
        &self,
        agent: &AgentName,
        _release: &PinnedRelease,
        staged: &mut StagedArchive,
    ) -> Result<PathBuf, StoreFailure> {
        let bytes = self.read(staged);
        let mut calls = self.calls.lock().expect("fake store lock");
        calls.published.push(agent.to_string());
        calls.unpacked.push(bytes);
        drop(calls);
        self.publish.clone()
    }

    fn discard(&self, staged: StagedArchive) {
        self.calls
            .lock()
            .expect("fake store lock")
            .discarded
            .push(staged.path().to_owned());
    }
}
