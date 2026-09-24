#[cfg(unix)]
use std::fs::Permissions;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    fs::File,
    io::{self, Read, Seek, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

use flate2::read::MultiGzDecoder;
use nessa_local_storage::{
    create_directory, create_directory_beneath, open_beneath, remove_directory_beneath,
    remove_file_beneath, sync_directory_beneath, OpenMode, PrivateDirectory, PrivateFileType,
    PrivateTempFile,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tar::{Archive, EntryType};
use uuid::Uuid;

use crate::agent_install::application::{
    ManagedExecutableUse, ManagedExecutableUseFailure, ManagedExecutableUseGuard,
    ManagedLaunchSnapshot, Publication, PublicationChange, PublicationCleanupFailure,
    PublicationLease, PublishFailure, ReclamationPersistenceFailure, ReclamationPersistenceStage,
    RollbackChange, RuntimeReclamationEffect, RuntimeStore, StagedArchive, StoreFailure,
};
use crate::agent_install::domain::{
    AgentName, ArchiveDigest, ArchivePath, FileRole, ManagedInstallation, PinnedRelease,
    ReleaseContents, ReleaseFile, ReleaseVersion, RuntimeArtifact,
};
use crate::agent_install::infrastructure::runtime_reclamation::StoredManagedInstallation;

/// How many times a staged download will try for a name of its own before
/// giving up. The names are sixteen random bytes, so a single collision already
/// means something other than chance.
const NAME_ATTEMPTS: u8 = 10;

/// How many times taking the publication lock will retry after finding that
/// the file it locked is no longer the one at that path. Each turn means
/// something outside Nessa replaced the lock file at exactly the wrong moment,
/// so more than a couple is not a race being lost.
const LOCK_ATTEMPTS: u8 = 5;

/// The most one release may unpack to, across every file it installs.
///
/// The digest fixes the *compressed* size of an archive and says nothing about
/// what comes out of it: a gzip member that matches its pin exactly can still
/// expand a thousandfold. Spent across the release rather than allowed to each
/// file, because a per-file bound is no bound at all on a pin naming a hundred
/// of them. A few times the largest runtime Nessa pins — Codex, which unpacks
/// to 277 MB — so it is only ever reached by an archive that is not what it
/// claims to be.
const MAXIMUM_UNPACKED_BYTES: u64 = 1024 * 1024 * 1024;

/// How much of an entry is moved out of the archive at a time.
const UNPACK_CHUNK: usize = 64 * 1024;
const MAXIMUM_USE_GENERATIONS: usize = 1024;
const MAXIMUM_USE_RECORD_BYTES: u64 = 16 * 1024;
const MAXIMUM_RECLAMATION_STATE_BYTES: u64 = 1024 * 1024;

struct BoundedJsonBuffer {
    bytes: Vec<u8>,
    maximum: usize,
    message: &'static str,
}

impl BoundedJsonBuffer {
    fn new(maximum: u64, message: &'static str) -> Self {
        Self {
            bytes: Vec::with_capacity((maximum as usize).min(64 * 1024)),
            maximum: maximum as usize,
            message,
        }
    }
}

impl Write for BoundedJsonBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let length = self
            .bytes
            .len()
            .checked_add(bytes.len())
            .filter(|length| *length <= self.maximum)
            .ok_or_else(|| io::Error::other(self.message))?;
        self.bytes.reserve(length - self.bytes.len());
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// What is recorded beside an installed runtime.
///
/// Nessa's own note about what it put there, not anything read out of the
/// archive. The version in particular is the *pinned* one: asking a downloaded
/// binary what version it is would be asking the thing we are trying to
/// verify.
///
/// It names the artifact rather than the version, because a version does not
/// identify a binary. One Opencode version is published as nine archives — two
/// operating systems, two architectures, glibc and musl, with and without
/// AVX2 — and every one of them unpacks a file called `opencode`. A record
/// carrying only the version and that name would answer "already installed" to
/// a pin asking for a different one of the nine, and hand back a binary this
/// machine may not even be able to start.
///
/// The digest is what tells them apart, and it is the same digest the download
/// was accepted against, so a record that matches names bytes the install
/// verified — verified by the use case, which compares the digest before this
/// store is asked to unpack anything, not by this file.
///
/// [`Self::describes`] compares exactly what the layout keys on, and no more.
/// The platform and the requirements are written down for whoever opens the
/// file, and they are facts *about the digest*: one archive is one build, and
/// the pin reader refuses a file that names an archive twice. Comparing them
/// would make a pin that corrects one of those fields, leaving the archive
/// alone, reinstall bytes that are already there — over a directory of the
/// same name, since the layout does not key on them.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct InstallationRecord {
    version: String,
    /// The operating system and architecture, as [`ReleasePlatform`] writes
    /// them.
    platform: String,
    /// The C library the installed build needs, absent where the platform has
    /// only one.
    libc: Option<String>,
    /// Whether the installed build needs AVX2.
    requires_avx2: bool,
    /// The archive digest this runtime was accepted against.
    digest: String,
    /// Every file the install unpacked, exactly as the pin names them.
    files: Vec<RecordedFile>,
}

/// One installed file, as the record spells it.
///
/// The role is written down rather than derived, because it is what the install
/// acted on: it decided which file was made runnable and which one is handed
/// back to launch. A record that lost it could not tell a pin that turned a
/// document into a program from one that changed nothing.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct RecordedFile {
    path: String,
    role: String,
}

impl InstallationRecord {
    /// What this store would write for `release`.
    fn of(release: &PinnedRelease) -> Self {
        Self {
            version: release.version().as_str().to_owned(),
            platform: release.platform().to_string(),
            libc: release.requirements().libc().map(|l| l.as_str().to_owned()),
            requires_avx2: release.requirements().avx2(),
            digest: release.archive_digest().as_str().to_owned(),
            files: release
                .contents()
                .files()
                .iter()
                .map(|file| RecordedFile {
                    path: file.path().as_str().to_owned(),
                    role: file.role().as_str().to_owned(),
                })
                .collect(),
        }
    }

    /// Whether this record describes what the layout would put there.
    ///
    /// Exactly the fields the installation is built from — the version and the
    /// digest name the directory, and the files name what goes in it and what
    /// each of them was installed as. Nothing else, deliberately: a field that
    /// decided "already installed" without deciding *what was written* would
    /// send an install to rename over files this record still names, and the
    /// rollback after a failed settle would then take away a runtime it did not
    /// write.
    ///
    /// The platform and the requirements are deliberately *not* compared. They
    /// are facts about the digest rather than part of where anything goes: one
    /// archive is one build, and the pin reader refuses a file that names an
    /// archive twice. A pin that corrects one of those fields and leaves the
    /// archive alone would otherwise reinstall bytes that are already there,
    /// over a directory of the same name.
    ///
    /// Every file is compared, not only the one that is launched. A pin that
    /// keeps its archive and starts installing a helper program the previous
    /// one left out describes a different installation, and answering "already
    /// installed" to it would leave the runtime unable to do its work with
    /// nothing saying why. The comparison is order-insensitive by construction:
    /// [`ReleaseContents`] sorts its files, so a record written from one is
    /// already in the order the next one will be read in.
    ///
    /// [`ReleaseContents`]: crate::agent_install::domain::ReleaseContents
    fn describes(&self, release: &PinnedRelease) -> bool {
        let expected = Self::of(release);
        self.version == expected.version
            && self.digest == expected.digest
            && self.files == expected.files
    }

    /// Restore installed identity through the current domain validators.
    fn artifact(&self) -> Option<RuntimeArtifact> {
        let files = self
            .files
            .iter()
            .map(|file| {
                Some(ReleaseFile::new(
                    ArchivePath::parse(&file.path).ok()?,
                    FileRole::parse(&file.role).ok()?,
                ))
            })
            .collect::<Option<Vec<_>>>()?;
        Some(RuntimeArtifact::new(
            ReleaseVersion::parse(&self.version).ok()?,
            ArchiveDigest::parse(&self.digest).ok()?,
            ReleaseContents::new(files).ok()?,
        ))
    }
}

/// The runtimes Nessa installed, under one directory it owns.
///
/// ```text
/// <root>/<agent>/installed.json                     what is installed, written last
/// <root>/<agent>/install.lock                       held while one install publishes
/// <root>/<agent>/versions/<version>/<digest>/<path> one file the pin names
/// <root>/<agent>/.nessa-<hex>.download              a download in progress, unnamed on unix
/// <root>/<agent>/.nessa-<hex>.tmp                   a record or an installed file being written
/// ```
///
/// `<path>` is the file's path inside the archive, reproduced rather than
/// flattened: Codex's runtime finds its ripgrep and its zsh through the
/// directory it sits in, so `vendor/…/bin/codex` and `vendor/…/codex-path/rg`
/// have to keep their relationship to each other. Which paths those are is the
/// pin's statement and never the archive's, so an entry claiming to be called
/// something else is an entry nothing here writes.
///
/// Versions sit under `versions/` so that nothing the store names for itself
/// can be named by a pin: a version is allowed to be spelled `installed.json`,
/// and one directory for both would make which of them won a question about
/// the order things happened in.
///
/// Under the version is the archive's digest, because a version does not name a
/// binary. One version is published as an archive per platform, per C library
/// and per processor baseline, and all of them unpack a file of the same name.
/// Keyed by version alone they would overwrite one another, and a machine that
/// had installed one would be told it already had another.
///
/// A pin that moves lands in a directory of its own rather than over the
/// previous runtime, so the old binary stays where it is and whatever is
/// running it keeps working. Re-publishing the *same* artifact does rename
/// over the file already there, which is safe for a different reason: a rename
/// is atomic and a process that has the old binary open keeps the bytes it
/// opened. `installed.json` is written only once the executable is in place
/// *and* on the disk, which is what makes a crashed install read as "nothing
/// installed" rather than as a runtime that is not there.
///
/// Every directory here is created private to this user, and every file this
/// type publishes is written to a temporary name, synced, and renamed into
/// place. A machine that loses power mid-install comes back with either the
/// previous answer or the new one, never a half of either.
pub struct ManagedRuntimes {
    root: PathBuf,
}

#[derive(Deserialize, Serialize, PartialEq, Eq)]
struct ExecutableUseInventory {
    agent: String,
    version: String,
    digest: String,
}

#[derive(Deserialize, Serialize, PartialEq, Eq)]
struct ExecutableUseGeneration {
    agent: String,
    version: String,
    digest: String,
    generation: String,
}

struct DurableManagedExecutableUse {
    root: PathBuf,
    marker_directory: PathBuf,
    agent: AgentName,
    artifact: RuntimeArtifact,
    _artifact_lock: File,
}

impl ManagedExecutableUse for DurableManagedExecutableUse {
    fn admit(&self) -> Result<Box<dyn ManagedExecutableUseGuard>, ManagedExecutableUseFailure> {
        let store = ManagedRuntimes::new(self.root.clone());
        let mutation = store
            .hold_use_mutation(&self.marker_directory)
            .map_err(|error| ManagedExecutableUseFailure::new(error.to_string()))?;
        let inventory = ExecutableUseInventory {
            agent: self.agent.as_str().to_owned(),
            version: self.artifact.version().as_str().to_owned(),
            digest: self.artifact.digest().as_str().to_owned(),
        };
        store
            .validate_use_inventory(&self.marker_directory, &inventory)
            .map_err(|error| ManagedExecutableUseFailure::new(error.to_string()))?;
        store
            .active_use_remains(&self.agent, &self.artifact)
            .map_err(|error| ManagedExecutableUseFailure::new(error.to_string()))?;
        if store
            .use_generation_count(&self.marker_directory)
            .map_err(|error| ManagedExecutableUseFailure::new(error.to_string()))?
            >= MAXIMUM_USE_GENERATIONS
        {
            return Err(ManagedExecutableUseFailure::new(
                "executable-use inventory reached its generation capacity",
            ));
        }
        let generation = Uuid::new_v4().to_string();
        let record = ExecutableUseGeneration {
            agent: self.agent.as_str().to_owned(),
            version: self.artifact.version().as_str().to_owned(),
            digest: self.artifact.digest().as_str().to_owned(),
            generation: generation.clone(),
        };
        store
            .retain_use_generation(&self.marker_directory, &generation, "expected", &record)
            .and_then(|()| {
                store.retain_use_generation(
                    &self.marker_directory,
                    &generation,
                    "admitted",
                    &record,
                )
            })
            .map_err(|error| ManagedExecutableUseFailure::new(error.to_string()))?;
        mutation
            .verify()
            .map_err(|error| ManagedExecutableUseFailure::new(error.to_string()))?;
        Ok(Box::new(DurableManagedExecutableUseGuard {
            root: self.root.clone(),
            marker_directory: self.marker_directory.clone(),
            record,
            released: false,
        }))
    }
}

struct DurableManagedExecutableUseGuard {
    root: PathBuf,
    marker_directory: PathBuf,
    record: ExecutableUseGeneration,
    released: bool,
}

struct UseMutation {
    directory: PrivateDirectory,
    lock: File,
}

impl UseMutation {
    fn verify(&self) -> Result<(), StoreFailure> {
        self.directory.verify_binding().map_err(unreadable)?;
        if !self
            .directory
            .named_file_is(OsStr::new("admission.lock"), &self.lock)
            .map_err(unreadable)?
        {
            return Err(StoreFailure::Unreadable(
                "executable-use admission lock was replaced after acquisition".into(),
            ));
        }
        Ok(())
    }
}

struct ManagedPublicationLease {
    root: PathBuf,
    agent: AgentName,
    _lock: File,
}

impl PublicationLease for ManagedPublicationLease {
    fn load_reclamation(
        &mut self,
    ) -> Result<Option<ManagedInstallation>, ReclamationPersistenceFailure> {
        ManagedRuntimes::new(self.root.clone()).load_reclamation(&self.agent)
    }

    fn retain_reclamation(
        &mut self,
        installation: &ManagedInstallation,
        stage: ReclamationPersistenceStage,
    ) -> Result<(), ReclamationPersistenceFailure> {
        if installation.agent() != &self.agent {
            return Err(ReclamationPersistenceFailure::new(
                stage,
                "reclamation state belongs to another agent".into(),
            ));
        }
        ManagedRuntimes::new(self.root.clone()).retain_reclamation(installation, stage)
    }

    fn remove_superseded(
        &mut self,
        agent: &AgentName,
        current: &RuntimeArtifact,
        superseded: &RuntimeArtifact,
    ) -> RuntimeReclamationEffect {
        if agent != &self.agent {
            return RuntimeReclamationEffect::Failed(StoreFailure::Unwritable(
                "reclamation authority belongs to another agent".into(),
            ));
        }
        ManagedRuntimes::new(self.root.clone())
            .remove_superseded_under_lock(agent, current, superseded)
    }

    fn observe_superseded(
        &mut self,
        agent: &AgentName,
        current: &RuntimeArtifact,
        superseded: &RuntimeArtifact,
    ) -> RuntimeReclamationEffect {
        if agent != &self.agent {
            return RuntimeReclamationEffect::Failed(StoreFailure::Unwritable(
                "reclamation authority belongs to another agent".into(),
            ));
        }
        ManagedRuntimes::new(self.root.clone())
            .observe_superseded_under_lock(agent, current, superseded)
    }
}

impl ManagedExecutableUseGuard for DurableManagedExecutableUseGuard {
    fn release(&mut self) -> Result<(), ManagedExecutableUseFailure> {
        if !self.released {
            let store = ManagedRuntimes::new(self.root.clone());
            let mutation = store
                .hold_use_mutation(&self.marker_directory)
                .map_err(|error| ManagedExecutableUseFailure::new(error.to_string()))?;
            store
                .retain_use_generation(
                    &self.marker_directory,
                    &self.record.generation,
                    "released",
                    &self.record,
                )
                .map_err(|error| ManagedExecutableUseFailure::new(error.to_string()))?;
            mutation
                .verify()
                .map_err(|error| ManagedExecutableUseFailure::new(error.to_string()))?;
            self.released = true;
        }
        Ok(())
    }
}

struct PublicationRecoveryContext<'a> {
    agent: &'a AgentName,
    written: &'a [PathBuf],
    previous_record: Option<&'a InstallationRecord>,
    previous_artifact: Option<RuntimeArtifact>,
    failure: StoreFailure,
    lock: File,
}

impl ManagedRuntimes {
    /// Manage runtimes under `root`, which composition owns and which is
    /// expected to be inside Nessa's own data directory.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Make the root a private directory of this user's, or say why not.
    ///
    /// The trust boundary. Everything above the root is composition's — it is
    /// derived from the data directory, which is where Nessa already keeps
    /// credentials — and this is the one call that resolves a path the ordinary
    /// way. Everything beneath it is treated as somewhere anything could have
    /// been planted, and is reached only through the anchored primitives.
    fn anchor(&self) -> Result<(), StoreFailure> {
        create_directory(&self.root).map_err(|error| at(&self.root, error))
    }

    /// Create a directory beneath the root, and no further.
    ///
    /// [`create_directory_beneath`] opens each component in turn relative to
    /// the verified one above it, refusing anything that is not a private
    /// directory of this user's and never following a symbolic link. A store
    /// that resolved these paths the ordinary way would create — and later
    /// remove, and unpack an executable into — whatever a link at
    /// `<root>/<agent>` or `<root>/<agent>/versions` pointed at.
    fn private_directory(&self, relative: &Path) -> Result<(), StoreFailure> {
        self.anchor()?;
        create_directory_beneath(&self.root, relative)
            .map_err(|error| at(&self.absolute(relative), error))
    }

    /// Where a path beneath the root is, said in full.
    ///
    /// For the two things that leave this type: the launch path it hands back,
    /// and the paths its failures name. Never for reaching a file.
    fn absolute(&self, relative: &Path) -> PathBuf {
        self.root.join(relative)
    }

    /// The directory holding one agent's runtimes, relative to the root.
    ///
    /// Every path in this type is relative, because every one of them is
    /// reached through a primitive that walks it component by component from
    /// the root. The name is an [`AgentName`], which is the type that says a
    /// name can also be a directory name, so there is nothing to re-check here.
    fn agent_root(&self, agent: &AgentName) -> PathBuf {
        PathBuf::from(agent.as_str())
    }

    /// Where `agent`'s executable lives for `version`.
    ///
    /// Under `versions/` rather than directly under the agent, so that a
    /// version can never name one of the store's own files. `installed.json`
    /// and a staged `.nessa-….download` are both spellings [`ReleaseVersion`]
    /// admits, and a pin naming one would have `publish` and `record` fighting
    /// over a single path. Kept apart by the layout rather than by a rule the
    /// domain would have to know about this directory to write.
    fn versions_root(&self, agent: &AgentName) -> PathBuf {
        self.agent_root(agent).join("versions")
    }

    fn version_root(&self, agent: &AgentName, version: &ReleaseVersion) -> PathBuf {
        self.versions_root(agent).join(version.as_str())
    }

    /// Where one *artifact* of a version lives.
    ///
    /// Named by the archive digest, which is the only thing that tells nine
    /// archives of one version apart, and which this store already holds
    /// because it is what the download was accepted against. Sixty-four
    /// lowercase hex characters, so it is a directory name everywhere.
    fn artifact_root(&self, agent: &AgentName, release: &PinnedRelease) -> PathBuf {
        self.version_root(agent, release.version())
            .join(release.archive_digest().as_str())
    }

    fn recorded_artifact_root(&self, agent: &AgentName, artifact: &RuntimeArtifact) -> PathBuf {
        self.version_root(agent, artifact.version())
            .join(artifact.digest().as_str())
    }

    /// Every directory inside the artifact this release's files need,
    /// outermost first.
    ///
    /// Derived from the pin's paths, never from the archive's entries — an
    /// archive can carry a directory entry saying anything at all, and an
    /// unpacker that created what the entries asked for would be letting a
    /// download choose where directories appear.
    ///
    /// A `BTreeSet` both removes the repeats — seven Codex files share four
    /// directories — and settles the order, because a path always sorts after
    /// every directory it is inside. That is what makes one iteration a legal
    /// creation order and the reverse a legal removal order.
    fn content_directories(&self, agent: &AgentName, release: &PinnedRelease) -> Vec<PathBuf> {
        let artifact = self.artifact_root(agent, release);
        release
            .contents()
            .files()
            .iter()
            .flat_map(|file| file.path().directories())
            .map(|directory| artifact.join(directory))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// Every directory this install creates, innermost first.
    ///
    /// Written out rather than walked with `parent()` so that the chain the
    /// store makes durable is the chain the store made, and stops at its own
    /// root rather than at whatever is above it. The directories inside the
    /// artifact come first and deepest-first, for the reason [`Self::settle`]
    /// gives: a directory is made durable before the entry naming it is.
    fn created_directories(&self, agent: &AgentName, release: &PinnedRelease) -> Vec<PathBuf> {
        let mut directories = self.content_directories(agent, release);
        directories.reverse();
        directories.extend([
            self.artifact_root(agent, release),
            self.version_root(agent, release.version()),
            self.versions_root(agent),
            self.agent_root(agent),
            // The root itself, which the anchored primitives spell as the
            // empty path: there is no component to walk to reach it.
            PathBuf::new(),
        ]);
        directories
    }

    fn record_path(&self, agent: &AgentName) -> PathBuf {
        self.agent_root(agent).join("installed.json")
    }

    fn lock_path(&self, agent: &AgentName) -> PathBuf {
        self.agent_root(agent).join("install.lock")
    }

    fn reclamation_path(&self, agent: &AgentName) -> PathBuf {
        self.agent_root(agent).join("reclamation.json")
    }

    fn load_reclamation(
        &self,
        agent: &AgentName,
    ) -> Result<Option<ManagedInstallation>, ReclamationPersistenceFailure> {
        let path = self.reclamation_path(agent);
        let mut file = match open_beneath(&self.root, &path, OpenMode::ReadNonblocking) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(ReclamationPersistenceFailure::new(
                    ReclamationPersistenceStage::Read,
                    error.to_string(),
                ))
            }
        };
        if file
            .metadata()
            .map_err(|error| {
                ReclamationPersistenceFailure::new(
                    ReclamationPersistenceStage::Read,
                    error.to_string(),
                )
            })?
            .len()
            > MAXIMUM_RECLAMATION_STATE_BYTES
        {
            return Err(ReclamationPersistenceFailure::new(
                ReclamationPersistenceStage::Read,
                "reclamation state exceeds its durable size bound".into(),
            ));
        }
        let mut encoded = Vec::new();
        file.by_ref()
            .take(MAXIMUM_RECLAMATION_STATE_BYTES + 1)
            .read_to_end(&mut encoded)
            .map_err(|error| {
                ReclamationPersistenceFailure::new(
                    ReclamationPersistenceStage::Read,
                    error.to_string(),
                )
            })?;
        if encoded.len() as u64 > MAXIMUM_RECLAMATION_STATE_BYTES {
            return Err(ReclamationPersistenceFailure::new(
                ReclamationPersistenceStage::Read,
                "reclamation state grew past its durable size bound".into(),
            ));
        }
        let stored: StoredManagedInstallation =
            serde_json::from_slice(&encoded).map_err(|error| {
                ReclamationPersistenceFailure::new(
                    ReclamationPersistenceStage::Read,
                    error.to_string(),
                )
            })?;
        stored.restore().map(Some).map_err(|error| {
            ReclamationPersistenceFailure::new(ReclamationPersistenceStage::Read, error)
        })
    }

    fn retain_reclamation(
        &self,
        installation: &ManagedInstallation,
        stage: ReclamationPersistenceStage,
    ) -> Result<(), ReclamationPersistenceFailure> {
        self.private_directory(&self.agent_root(installation.agent()))
            .map_err(|error| ReclamationPersistenceFailure::new(stage, error.to_string()))?;
        let mut encoded = BoundedJsonBuffer::new(
            MAXIMUM_RECLAMATION_STATE_BYTES,
            "reclamation state exceeds its durable size bound",
        );
        serde_json::to_writer_pretty(
            &mut encoded,
            &StoredManagedInstallation::from_domain(installation),
        )
        .map_err(|error| ReclamationPersistenceFailure::new(stage, error.to_string()))?;
        let mut staging =
            PrivateTempFile::new_beneath(&self.root, &self.agent_root(installation.agent()))
                .map_err(|error| ReclamationPersistenceFailure::new(stage, error.to_string()))?;
        staging
            .as_file_mut()
            .write_all(&encoded.bytes)
            .map_err(|error| ReclamationPersistenceFailure::new(stage, error.to_string()))?;
        staging
            .as_file()
            .sync_all()
            .map_err(|error| ReclamationPersistenceFailure::new(stage, error.to_string()))?;
        staging
            .persist_beneath(&self.reclamation_path(installation.agent()))
            .map_err(|error| ReclamationPersistenceFailure::new(stage, error.to_string()))?;
        sync_directory_beneath(&self.root, &self.agent_root(installation.agent()))
            .map_err(|error| ReclamationPersistenceFailure::new(stage, error.to_string()))
    }

    fn artifact_lock_directory(&self, agent: &AgentName, artifact: &RuntimeArtifact) -> PathBuf {
        self.agent_root(agent)
            .join("use-locks")
            .join(artifact.version().as_str())
    }

    fn artifact_lock_path(&self, agent: &AgentName, artifact: &RuntimeArtifact) -> PathBuf {
        self.artifact_lock_directory(agent, artifact)
            .join(format!("{}.lock", artifact.digest().as_str()))
    }

    fn use_marker_directory(&self, agent: &AgentName, artifact: &RuntimeArtifact) -> PathBuf {
        self.agent_root(agent)
            .join("active-uses")
            .join(artifact.version().as_str())
            .join(artifact.digest().as_str())
    }

    fn use_inventory(
        &self,
        agent: &AgentName,
        artifact: &RuntimeArtifact,
    ) -> ExecutableUseInventory {
        ExecutableUseInventory {
            agent: agent.as_str().to_owned(),
            version: artifact.version().as_str().to_owned(),
            digest: artifact.digest().as_str().to_owned(),
        }
    }

    fn sync_use_inventory_directories(
        &self,
        agent: &AgentName,
        artifact: &RuntimeArtifact,
    ) -> Result<(), StoreFailure> {
        let marker = self.use_marker_directory(agent, artifact);
        for directory in [
            marker.clone(),
            marker
                .parent()
                .expect("the marker has an artifact-version parent")
                .to_owned(),
            self.agent_root(agent).join("active-uses"),
            self.agent_root(agent),
            PathBuf::new(),
        ] {
            sync_directory_beneath(&self.root, &directory).map_err(unwritable)?;
        }
        Ok(())
    }

    fn hold_use_mutation(&self, directory: &Path) -> Result<UseMutation, StoreFailure> {
        for _ in 0..LOCK_ATTEMPTS {
            let authority =
                PrivateDirectory::open_beneath(&self.root, directory).map_err(unreadable)?;
            let lock = authority
                .open_file(OsStr::new("admission.lock"), OpenMode::OpenOrCreate)
                .map_err(unreadable)?;
            lock.lock().map_err(unreadable)?;
            let mutation = UseMutation {
                directory: authority,
                lock,
            };
            if mutation.verify().is_ok() {
                return Ok(mutation);
            }
        }
        Err(StoreFailure::Unreadable(
            "executable-use admission lock kept being replaced".into(),
        ))
    }

    fn retain_immutable_json<T: Serialize>(
        &self,
        directory: &Path,
        path: &Path,
        value: &T,
    ) -> Result<(), StoreFailure> {
        self.retain_immutable_json_with(directory, path, value, |directory| {
            sync_directory_beneath(&self.root, directory)
        })
    }

    fn retain_immutable_json_with<T: Serialize>(
        &self,
        directory: &Path,
        path: &Path,
        value: &T,
        durable: impl Fn(&Path) -> io::Result<()>,
    ) -> Result<(), StoreFailure> {
        let mut staging =
            PrivateTempFile::new_beneath(&self.root, directory).map_err(unwritable)?;
        let mut encoded = BoundedJsonBuffer::new(
            MAXIMUM_USE_RECORD_BYTES,
            "executable-use record exceeds its durable size bound",
        );
        serde_json::to_writer(&mut encoded, value)
            .map_err(|error| StoreFailure::Unwritable(error.to_string()))?;
        staging
            .as_file_mut()
            .write_all(&encoded.bytes)
            .map_err(unwritable)?;
        staging.as_file().sync_all().map_err(unwritable)?;
        staging.publish_new_beneath(path).map_err(unwritable)?;
        durable(directory).map_err(unwritable)
    }

    fn read_bounded_json<T: for<'de> Deserialize<'de>>(
        &self,
        path: &Path,
    ) -> Result<T, StoreFailure> {
        let mut file =
            open_beneath(&self.root, path, OpenMode::ReadNonblocking).map_err(unreadable)?;
        if file.metadata().map_err(unreadable)?.len() > MAXIMUM_USE_RECORD_BYTES {
            return Err(StoreFailure::Unreadable(format!(
                "{} exceeds its executable-use record bound",
                self.absolute(path).display()
            )));
        }
        let mut bytes = Vec::new();
        file.by_ref()
            .take(MAXIMUM_USE_RECORD_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(unreadable)?;
        if bytes.len() as u64 > MAXIMUM_USE_RECORD_BYTES {
            return Err(StoreFailure::Unreadable(format!(
                "{} grew past its executable-use record bound",
                self.absolute(path).display()
            )));
        }
        serde_json::from_slice(&bytes).map_err(|error| StoreFailure::Unreadable(error.to_string()))
    }

    fn initialize_use_inventory(
        &self,
        agent: &AgentName,
        artifact: &RuntimeArtifact,
    ) -> Result<(), StoreFailure> {
        let directory = self.use_marker_directory(agent, artifact);
        self.private_directory(&directory)?;
        let expected = self.use_inventory(agent, artifact);
        let path = directory.join("inventory.json");
        match self.retain_immutable_json(&directory, &path, &expected) {
            Ok(()) => self.sync_use_inventory_directories(agent, artifact),
            Err(original) => match self.validate_use_inventory(&directory, &expected) {
                Ok(()) => self.sync_use_inventory_directories(agent, artifact),
                Err(_) => Err(original),
            },
        }
    }

    fn validate_use_inventory(
        &self,
        directory: &Path,
        expected: &ExecutableUseInventory,
    ) -> Result<(), StoreFailure> {
        let restored: ExecutableUseInventory =
            self.read_bounded_json(&directory.join("inventory.json"))?;
        if &restored != expected {
            return Err(StoreFailure::Unreadable(
                "executable-use inventory belongs to another artifact".into(),
            ));
        }
        Ok(())
    }

    fn use_generation_count(&self, directory: &Path) -> Result<usize, StoreFailure> {
        let retained = PrivateDirectory::open_beneath(&self.root, directory).map_err(unreadable)?;
        retained.verify_binding().map_err(unreadable)?;
        let mut count = 0usize;
        for entry in retained.entries().map_err(unreadable)? {
            let entry = entry.map_err(unreadable)?;
            if entry
                .name()
                .to_str()
                .is_some_and(|name| name.ends_with(".expected.json"))
            {
                count += 1;
                if count > MAXIMUM_USE_GENERATIONS {
                    break;
                }
            }
        }
        retained.verify_binding().map_err(unreadable)?;
        Ok(count)
    }

    fn retain_use_generation(
        &self,
        directory: &Path,
        generation: &str,
        phase: &str,
        record: &ExecutableUseGeneration,
    ) -> Result<(), StoreFailure> {
        self.retain_use_generation_with(directory, generation, phase, record, |directory| {
            sync_directory_beneath(&self.root, directory)
        })
    }

    fn retain_use_generation_with(
        &self,
        directory: &Path,
        generation: &str,
        phase: &str,
        record: &ExecutableUseGeneration,
        durable: impl Fn(&Path) -> io::Result<()>,
    ) -> Result<(), StoreFailure> {
        let path = directory.join(format!("{generation}.{phase}.json"));
        match self.retain_immutable_json_with(directory, &path, record, &durable) {
            Ok(()) => Ok(()),
            Err(original) => match self.read_bounded_json::<ExecutableUseGeneration>(&path) {
                Ok(existing) if existing == *record => durable(directory).map_err(unwritable),
                Ok(_) => Err(StoreFailure::Unreadable(
                    "executable-use generation identity has conflicting facts".into(),
                )),
                Err(_) => Err(original),
            },
        }
    }

    fn recorded_content_directories(
        &self,
        agent: &AgentName,
        artifact: &RuntimeArtifact,
    ) -> Vec<PathBuf> {
        let root = self.recorded_artifact_root(agent, artifact);
        artifact
            .contents()
            .files()
            .iter()
            .flat_map(|file| file.path().directories())
            .map(|directory| root.join(directory))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    fn active_use_remains(
        &self,
        agent: &AgentName,
        artifact: &RuntimeArtifact,
    ) -> Result<bool, StoreFailure> {
        let directory = self.use_marker_directory(agent, artifact);
        let retained = match PrivateDirectory::open_beneath(&self.root, &directory) {
            Ok(directory) => directory,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(unreadable(error)),
        };
        retained.verify_binding().map_err(unreadable)?;
        let expected_inventory = self.use_inventory(agent, artifact);
        self.validate_use_inventory(&directory, &expected_inventory)?;
        let mut generations: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut entries = 0usize;
        for entry in retained.entries().map_err(unreadable)? {
            let entry = entry.map_err(unreadable)?;
            if entry.file_type() != PrivateFileType::RegularFile {
                return Err(StoreFailure::Unreadable(format!(
                    "{} contains an unsafe executable-use marker",
                    self.absolute(&directory).display()
                )));
            }
            let name = entry.name();
            let Some(name) = name.to_str() else {
                return Err(StoreFailure::Unreadable(
                    "executable-use inventory contains a non-text name".into(),
                ));
            };
            if matches!(name, "inventory.json" | "admission.lock") {
                continue;
            }
            entries += 1;
            if entries > MAXIMUM_USE_GENERATIONS * 3 {
                return Err(StoreFailure::Unreadable(
                    "executable-use inventory exceeds its generation capacity".into(),
                ));
            }
            let Some((generation, phase)) = name
                .strip_suffix(".json")
                .and_then(|name| name.rsplit_once('.'))
            else {
                return Err(StoreFailure::Unreadable(
                    "executable-use inventory contains an unknown record".into(),
                ));
            };
            if !matches!(phase, "expected" | "admitted" | "released") {
                return Err(StoreFailure::Unreadable(
                    "executable-use inventory contains an unknown phase".into(),
                ));
            }
            let record: ExecutableUseGeneration = self.read_bounded_json(&directory.join(name))?;
            if record.agent != agent.as_str()
                || record.version != artifact.version().as_str()
                || record.digest != artifact.digest().as_str()
                || record.generation != generation
            {
                return Err(StoreFailure::Unreadable(
                    "executable-use generation contradicts its artifact or identity".into(),
                ));
            }
            generations
                .entry(generation.to_owned())
                .or_default()
                .insert(phase.to_owned());
        }
        retained.verify_binding().map_err(unreadable)?;
        if generations.len() > MAXIMUM_USE_GENERATIONS {
            return Err(StoreFailure::Unreadable(
                "executable-use inventory exceeds its generation capacity".into(),
            ));
        }
        for phases in generations.values() {
            if !phases.contains("expected")
                || (phases.contains("released") && !phases.contains("admitted"))
            {
                return Err(StoreFailure::Unreadable(
                    "executable-use inventory contains an orphan generation record".into(),
                ));
            }
            if !phases.contains("admitted") || !phases.contains("released") {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn remove_superseded_under_lock(
        &self,
        agent: &AgentName,
        current: &RuntimeArtifact,
        superseded: &RuntimeArtifact,
    ) -> RuntimeReclamationEffect {
        let observed = match self.recorded_artifact(agent) {
            Ok(observed) => observed,
            Err(error) => return RuntimeReclamationEffect::Failed(error),
        };
        if observed.as_ref() != Some(current) {
            return RuntimeReclamationEffect::Failed(StoreFailure::Unreadable(
                "installed runtime changed after reclamation admission".into(),
            ));
        }
        if current.physical_identity() == superseded.physical_identity() {
            return RuntimeReclamationEffect::DeferredCurrent;
        }
        if let Err(error) = self.private_directory(&self.artifact_lock_directory(agent, superseded))
        {
            return RuntimeReclamationEffect::Failed(error);
        }
        let lock_path = self.artifact_lock_path(agent, superseded);
        let artifact_lock = match open_beneath(&self.root, &lock_path, OpenMode::OpenOrCreate) {
            Ok(lock) => lock,
            Err(error) => return RuntimeReclamationEffect::Failed(unreadable(error)),
        };
        if artifact_lock.try_lock().is_err() {
            return RuntimeReclamationEffect::DeferredInUse;
        }
        match self.same_file(&artifact_lock, &lock_path) {
            Ok(true) => {}
            Ok(false) => {
                return RuntimeReclamationEffect::Failed(StoreFailure::Unreadable(
                    "artifact-use lock was replaced after acquisition".into(),
                ))
            }
            Err(error) => return RuntimeReclamationEffect::Failed(error),
        }
        let use_directory = self.use_marker_directory(agent, superseded);
        let use_mutation = match self.hold_use_mutation(&use_directory) {
            Ok(lock) => lock,
            Err(error) => return RuntimeReclamationEffect::Failed(error),
        };
        let active_use = self.active_use_remains(agent, superseded);
        if let Err(error) = use_mutation.verify() {
            return RuntimeReclamationEffect::Failed(error);
        }
        match active_use {
            Ok(true) => return RuntimeReclamationEffect::DeferredInUse,
            Ok(false) => {}
            Err(error) => return RuntimeReclamationEffect::Failed(error),
        }

        let mut present = Vec::new();
        for file in superseded.contents().files() {
            let path = self.recorded_file_path(agent, superseded, file.path());
            match open_beneath(&self.root, &path, OpenMode::ReadNonblocking) {
                Ok(file) if file.metadata().is_ok_and(|metadata| metadata.len() != 0) => {
                    present.push(path)
                }
                Ok(_) => {
                    return RuntimeReclamationEffect::Failed(StoreFailure::Unreadable(format!(
                        "{} is not a complete managed runtime file",
                        self.absolute(&path).display()
                    )))
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return RuntimeReclamationEffect::Failed(unreadable(error)),
            }
        }
        let removed_files = !present.is_empty();
        let mut failures = Vec::new();
        for path in &present {
            if let Err(error) = remove_file_beneath(&self.root, path) {
                failures.push(format!("{}: {error}", self.absolute(path).display()));
            }
        }
        if !failures.is_empty() {
            return RuntimeReclamationEffect::Failed(StoreFailure::Unwritable(format!(
                "could not remove superseded runtime files: {}",
                failures.join("; ")
            )));
        }
        let artifact_root = self.recorded_artifact_root(agent, superseded);
        if !removed_files {
            match PrivateDirectory::open_beneath(&self.root, &artifact_root) {
                Ok(directory) => {
                    if let Err(error) = directory.verify_binding() {
                        return RuntimeReclamationEffect::Failed(unreadable(error));
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    return RuntimeReclamationEffect::AlreadyAbsent
                }
                Err(error) => return RuntimeReclamationEffect::Failed(unreadable(error)),
            }
        }
        if let Err(error) = sync_directory_beneath(&self.root, &artifact_root) {
            return RuntimeReclamationEffect::SyncUncertain(unwritable(error));
        }
        let mut directories = self.recorded_content_directories(agent, superseded);
        directories.reverse();
        for directory in directories {
            if let Err(error) = remove_directory_beneath(&self.root, &directory) {
                if error.kind() != io::ErrorKind::NotFound {
                    return RuntimeReclamationEffect::Failed(unwritable(error));
                }
            }
        }
        if let Err(error) = remove_directory_beneath(&self.root, &artifact_root) {
            if error.kind() != io::ErrorKind::NotFound {
                return RuntimeReclamationEffect::Failed(unwritable(error));
            }
        }
        let version_root = self.version_root(agent, superseded.version());
        if let Err(error) = sync_directory_beneath(&self.root, &version_root) {
            return RuntimeReclamationEffect::SyncUncertain(unwritable(error));
        }
        if removed_files {
            RuntimeReclamationEffect::Removed
        } else {
            RuntimeReclamationEffect::AlreadyAbsent
        }
    }

    fn observe_superseded_under_lock(
        &self,
        agent: &AgentName,
        current: &RuntimeArtifact,
        superseded: &RuntimeArtifact,
    ) -> RuntimeReclamationEffect {
        let observed = match self.recorded_artifact(agent) {
            Ok(observed) => observed,
            Err(error) => return RuntimeReclamationEffect::Failed(error),
        };
        if observed.as_ref() != Some(current) {
            return RuntimeReclamationEffect::Failed(StoreFailure::Unreadable(
                "installed runtime changed after reclamation admission".into(),
            ));
        }
        if current.physical_identity() == superseded.physical_identity() {
            return RuntimeReclamationEffect::DeferredCurrent;
        }
        if let Err(error) = self.private_directory(&self.artifact_lock_directory(agent, superseded))
        {
            return RuntimeReclamationEffect::Failed(error);
        }
        let lock_path = self.artifact_lock_path(agent, superseded);
        let artifact_lock = match open_beneath(&self.root, &lock_path, OpenMode::OpenOrCreate) {
            Ok(lock) => lock,
            Err(error) => return RuntimeReclamationEffect::Failed(unreadable(error)),
        };
        if artifact_lock.try_lock().is_err() {
            return RuntimeReclamationEffect::DeferredInUse;
        }
        match self.same_file(&artifact_lock, &lock_path) {
            Ok(true) => {}
            Ok(false) => {
                return RuntimeReclamationEffect::Failed(StoreFailure::Unreadable(
                    "artifact-use lock was replaced after acquisition".into(),
                ))
            }
            Err(error) => return RuntimeReclamationEffect::Failed(error),
        }
        let use_directory = self.use_marker_directory(agent, superseded);
        let use_mutation = match self.hold_use_mutation(&use_directory) {
            Ok(lock) => lock,
            Err(error) => return RuntimeReclamationEffect::Failed(error),
        };
        let active_use = self.active_use_remains(agent, superseded);
        if let Err(error) = use_mutation.verify() {
            return RuntimeReclamationEffect::Failed(error);
        }
        match active_use {
            Ok(true) => return RuntimeReclamationEffect::DeferredInUse,
            Ok(false) => {}
            Err(error) => return RuntimeReclamationEffect::Failed(error),
        }
        for file in superseded.contents().files() {
            let path = self.recorded_file_path(agent, superseded, file.path());
            match open_beneath(&self.root, &path, OpenMode::ReadNonblocking) {
                Ok(file) if file.metadata().is_ok_and(|metadata| metadata.len() != 0) => {
                    return RuntimeReclamationEffect::StillPresent
                }
                Ok(_) => {
                    return RuntimeReclamationEffect::Failed(StoreFailure::Unreadable(format!(
                        "{} is not a complete managed runtime file",
                        self.absolute(&path).display()
                    )))
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return RuntimeReclamationEffect::Failed(unreadable(error)),
            }
        }
        RuntimeReclamationEffect::AlreadyAbsent
    }

    fn hold_artifact_for_launch(
        &self,
        agent: &AgentName,
        artifact: &RuntimeArtifact,
    ) -> Result<File, StoreFailure> {
        let directory = self.artifact_lock_directory(agent, artifact);
        self.private_directory(&directory)?;
        let path = self.artifact_lock_path(agent, artifact);
        for _ in 0..LOCK_ATTEMPTS {
            let lock = open_beneath(&self.root, &path, OpenMode::OpenOrCreate)
                .map_err(|error| at(&self.absolute(&path), error))?;
            lock.lock_shared()
                .map_err(|error| at(&self.absolute(&path), error))?;
            if self.same_file(&lock, &path)? {
                return Ok(lock);
            }
        }
        Err(StoreFailure::Unreadable(format!(
            "{}: kept being replaced while a runtime launch was verified",
            self.absolute(&path).display()
        )))
    }

    /// Where one of this release's files goes, relative to the root.
    ///
    /// The path the *pin* gives it, joined onto the artifact directory. The
    /// entry's own name is never used for this, so nothing in a downloaded
    /// archive can direct a write: [`ArchivePath`] has already refused anything
    /// absolute, drive-relative or walking upward, so the join cannot leave the
    /// artifact directory.
    ///
    /// The archive's layout is kept rather than flattened because a package is
    /// not a loose pile of files. Codex's runtime looks for its ripgrep at
    /// `../codex-path/rg` and its zsh under `../codex-resources/`, both
    /// relative to the program itself; installed side by side in one directory
    /// they would all be present and none of them findable.
    fn file_path(&self, agent: &AgentName, release: &PinnedRelease, path: &ArchivePath) -> PathBuf {
        self.artifact_root(agent, release).join(path.as_str())
    }

    /// Where the program `agent` is launched from for this artifact.
    fn launch_path(&self, agent: &AgentName, release: &PinnedRelease) -> PathBuf {
        self.file_path(agent, release, release.launch())
    }

    fn recorded_file_path(
        &self,
        agent: &AgentName,
        artifact: &RuntimeArtifact,
        path: &ArchivePath,
    ) -> PathBuf {
        self.recorded_artifact_root(agent, artifact)
            .join(path.as_str())
    }

    fn installation_record(
        &self,
        agent: &AgentName,
    ) -> Result<Option<InstallationRecord>, StoreFailure> {
        let mut record = match open_beneath(
            &self.root,
            &self.record_path(agent),
            OpenMode::ReadNonblocking,
        ) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(unreadable(error)),
        };
        let mut encoded = Vec::new();
        record.read_to_end(&mut encoded).map_err(unreadable)?;
        Ok(serde_json::from_slice(&encoded).ok())
    }

    fn recorded_artifact(
        &self,
        agent: &AgentName,
    ) -> Result<Option<RuntimeArtifact>, StoreFailure> {
        let Some(artifact) = self
            .installation_record(agent)?
            .and_then(|record| record.artifact())
        else {
            return Ok(None);
        };
        for file in artifact.contents().files() {
            if !self.holds(&self.recorded_file_path(agent, &artifact, file.path()))? {
                return Ok(None);
            }
        }
        Ok(Some(artifact))
    }

    fn held_artifact(
        &self,
        agent: &AgentName,
        artifact: Option<RuntimeArtifact>,
    ) -> Result<Option<RuntimeArtifact>, StoreFailure> {
        let Some(artifact) = artifact else {
            return Ok(None);
        };
        for file in artifact.contents().files() {
            if !self.holds(&self.recorded_file_path(agent, &artifact, file.path()))? {
                return Ok(None);
            }
        }
        Ok(Some(artifact))
    }

    /// Whether this store really holds the file it published at `relative`.
    ///
    /// Opened rather than stat'ed, and through the anchored primitive. It
    /// answers the whole question in one call: every directory from the root
    /// down is walked and checked, nothing along the way may be a symbolic
    /// link, and what is finally opened has to be a regular file with a single
    /// link, owned by this user and reachable by nobody else — which is exactly
    /// what this store publishes and nothing else is.
    ///
    /// `symlink_metadata` would have covered only the last of those. A link at
    /// `versions/`, or at the agent's own directory, would have been followed
    /// by the kernel before it ever looked, and whatever was found on the other
    /// side handed back as the tested runtime — with no download, no digest,
    /// and none of the archive's checks ever running. That path is then given
    /// out to be launched.
    ///
    /// Anything that is not openable as a file this store published answers
    /// `false` rather than failing. That is the recoverable answer: the pinned
    /// release is known, and installing it renames the real files back over
    /// whatever is there. A failure would be a dead end — every attempt
    /// refusing at the same place, with nothing a person could do but go and
    /// find the file. A disk that is genuinely failing still says so, in the
    /// install that follows.
    fn holds(&self, relative: &Path) -> Result<bool, StoreFailure> {
        let Ok(file) = open_beneath(&self.root, relative, OpenMode::Read) else {
            return Ok(false);
        };
        // A record is a note, not evidence: a file truncated to nothing is not
        // the one that was unpacked, and an empty program cannot start.
        Ok(file.metadata().map_err(unreadable)?.len() != 0)
    }

    /// Hold the sole right to publish one agent's runtimes.
    ///
    /// Publication is not a single rename. It creates directories, unpacks a
    /// hundred megabytes into one of them, renames the executable into place,
    /// makes that durable and only then writes the record — and it undoes the
    /// executable if any of that fails. Two of those sequences overlapping is
    /// how one install's clean-up removes another's finished runtime: both
    /// write the same path, and the loser's rollback cannot tell the winner's
    /// file from its own. Held across the whole sequence, including the
    /// rollback, that rollback can only ever remove what this call wrote.
    ///
    /// Taken on a file rather than in memory because the two installs need not
    /// be in one process: Nessa can be running while somebody installs from a
    /// second copy, and a mutex would exclude nothing between them. The lock is
    /// advisory, which is enough here — everything that publishes goes through
    /// this method — and it is released when the handle closes, including when
    /// the process holding it dies, so a killed install leaves nothing for the
    /// next one to break on.
    ///
    /// Blocking on purpose. The other install is unpacking a file this one
    /// would otherwise unpack again; waiting for it is the outcome the caller
    /// wants, and the recheck immediately after is what turns that wait into a
    /// result rather than a second unpack. Not a second *download*: the
    /// archive was fetched and hashed before this store was asked to publish
    /// anything, so the waiting install has already paid for the bytes.
    fn hold(&self, agent: &AgentName) -> Result<File, StoreFailure> {
        self.private_directory(&self.agent_root(agent))?;
        let path = self.lock_path(agent);
        // Tried for a limited number of turns rather than once, because the
        // exclusion is on the file this handle has open, not on the name: if
        // something outside Nessa removes or replaces `install.lock` between
        // the open and the lock, two installs can each hold a different inode
        // and believe they are alone. Re-opening and comparing is what notices
        // that, and taking the lock again on the file that is there now is
        // what recovers from it.
        for _ in 0..LOCK_ATTEMPTS {
            // `OpenOrCreate`, so the first install to run creates it and every
            // later one takes the same file. It stays behind afterwards, which
            // is what lets it be the same file next time; it holds no bytes.
            let lock = open_beneath(&self.root, &path, OpenMode::OpenOrCreate)
                .map_err(|error| at(&self.absolute(&path), error))?;
            // Tried without waiting first, so that an install which is about
            // to wait can say so. Between the download and the runtime being
            // ready this command prints nothing, and a person watching it
            // stall after a hundred megabytes deserves to know it is queued
            // behind another install rather than hung.
            if lock.try_lock().is_err() {
                tracing::info!(
                    agent = %agent,
                    "waiting for another install of this agent to finish"
                );
                lock.lock()
                    .map_err(|error| at(&self.absolute(&path), error))?;
            }
            if self.same_file(&lock, &path)? {
                return Ok(lock);
            }
        }
        Err(StoreFailure::Unwritable(format!(
            "{}: kept being replaced while an install waited for it",
            self.absolute(&path).display()
        )))
    }

    /// Note what is now installed, once it really is.
    ///
    /// Written to a private temporary file, forced to the disk, and renamed, so
    /// a reader never sees a half-written record and a crash cannot leave one
    /// that names an executable the disk does not have. The directory is synced
    /// afterwards so the rename itself survives, which also makes durable the
    /// version directory created a moment earlier.
    ///
    /// The rename precedes that sync, so a sync failure can occur after this
    /// record replaced the prior one. Publication keeps the validated prior
    /// record and artifact while holding the same per-agent lease, withdraws
    /// only files written by this attempt, restores that prior record, and
    /// re-reads installed state. The caller receives withdrawal, restoration,
    /// and confirmation failures as separate typed facts.
    fn record(
        &self,
        agent: &AgentName,
        release: &PinnedRelease,
        durable: impl Fn(&Path) -> io::Result<()>,
    ) -> Result<(), StoreFailure> {
        let record = InstallationRecord::of(release);
        self.write_record(agent, &record, durable)
    }

    fn write_record(
        &self,
        agent: &AgentName,
        record: &InstallationRecord,
        durable: impl Fn(&Path) -> io::Result<()>,
    ) -> Result<(), StoreFailure> {
        let directory = self.agent_root(agent);
        let mut staging =
            PrivateTempFile::new_beneath(&self.root, &directory).map_err(unwritable)?;
        serde_json::to_writer_pretty(staging.as_file_mut(), &record)
            .map_err(|error| StoreFailure::Unwritable(error.to_string()))?;
        staging.as_file().sync_all().map_err(unwritable)?;
        staging
            .persist_beneath(&self.record_path(agent))
            .map_err(unwritable)?;
        // Through the same primitive as the walk above it, because this is the
        // sync that makes the record's own rename durable — the last thing
        // between an install and being acknowledged. Reaching for
        // `sync_directory` directly here would leave the final step of the
        // guarantee outside anything a test can watch.
        durable(&directory).map_err(unwritable)
    }

    fn restore_record(
        &self,
        agent: &AgentName,
        record: Option<&InstallationRecord>,
        durable: impl Fn(&Path) -> io::Result<()>,
    ) -> Result<(), StoreFailure> {
        match record {
            Some(record) => self.write_record(agent, record, durable),
            None => match remove_file_beneath(&self.root, &self.record_path(agent)) {
                Ok(()) => durable(&self.agent_root(agent)).map_err(unwritable),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(unwritable(error)),
            },
        }
    }

    /// Make an unpacked executable durable and record it as installed.
    ///
    /// Its own step so that everything which can fail after the rename sits in
    /// one place the caller can guard. Adding a step here rather than to
    /// `publish` directly is what keeps it inside that guard.
    ///
    /// `durable` is how a directory is forced to the disk. It is a parameter
    /// rather than a call to [`sync_directory`] written inline because the
    /// order this walks in is the whole of what this method decides, and on a
    /// real filesystem that order leaves no trace: a directory whose entry was
    /// never synced still reads back exactly the same. Passed in, the walk has
    /// somewhere to be observed from, and `publish` supplies the real one.
    fn settle(
        &self,
        agent: &AgentName,
        release: &PinnedRelease,
        durable: impl Fn(&Path) -> io::Result<()>,
    ) -> Result<(), StoreFailure> {
        self.initialize_use_inventory(agent, &RuntimeArtifact::for_release(release))?;
        // A rename survives a crash only once the directory holding it does —
        // and that directory only once the one holding *it* does. Every level
        // from the executable's own directory up to this store's root can have
        // been created by this install, and syncing the innermost alone leaves
        // a crash able to come back to a machine with no version directory at
        // all and a record insisting there is one. Outward, so each level is
        // durable before the entry naming it is claimed to be.
        //
        // On Unix. Windows does not expose directory fsync at all, so
        // `sync_directory` is a documented no-op there and this walk does
        // nothing: what durability Windows has comes from the write-through
        // move inside `replace`, which covers the files but not the entries
        // naming them. Nessa pins no Windows release today, and whoever pins
        // one inherits that gap rather than this guarantee.
        for directory in self.created_directories(agent, release) {
            durable(&directory).map_err(unwritable)?;
        }
        self.record(agent, release, durable)
    }

    /// Take back everything this install wrote, after it failed to complete.
    ///
    /// `written` is what this call actually renamed into place, collected as it
    /// went, so the removal can only ever undo this call's own work. That is
    /// what lets one rollback serve both the install that failed part way
    /// through unpacking and the one that finished unpacking and could not be
    /// settled — and what keeps a pin that is wrong about its own contents from
    /// deleting a runtime somebody was using, since a file this call did not
    /// write is not in the list.
    ///
    /// All of them, in one go: a release that installs seven files and leaves
    /// four of them behind is worse than one that leaves none, because the four
    /// are enough for the next install to rename over and not enough to run.
    ///
    /// Every removal is attempted. Failures are accumulated and returned as
    /// cleanup evidence beside the original publication cause rather than
    /// replacing it or being reduced to logs.
    fn withdraw(
        &self,
        agent: &AgentName,
        release: &PinnedRelease,
        written: &[PathBuf],
    ) -> Result<(), StoreFailure> {
        let mut failures = Vec::new();
        for file in written {
            if let Err(error) = remove_file_beneath(&self.root, file) {
                if error.kind() != io::ErrorKind::NotFound {
                    failures.push(format!("{}: {error}", self.absolute(file).display()));
                }
            }
        }
        self.sweep_artifact(agent, release);
        if failures.is_empty() {
            Ok(())
        } else {
            Err(StoreFailure::Unwritable(format!(
                "could not withdraw published runtime files: {}",
                failures.join("; ")
            )))
        }
    }

    /// Remove the directories this install made and then had no use for.
    ///
    /// Innermost first, because `remove_dir` only takes an empty directory and
    /// a package's own directories nest several deep. Then the artifact
    /// directory, which holds this artifact alone, and the version directory,
    /// which holds nothing but artifact directories of that version — so an
    /// install that produced no files has left the whole chain empty behind it.
    /// `versions/` and the agent's own directory are not swept: they are the
    /// store's furniture rather than this install's leavings, and the record
    /// and the lock live in the second one.
    fn sweep_artifact(&self, agent: &AgentName, release: &PinnedRelease) {
        for directory in self.content_directories(agent, release).iter().rev() {
            self.sweep(directory);
        }
        self.sweep(&self.artifact_root(agent, release));
        self.sweep(&self.version_root(agent, release.version()));
    }

    /// Remove one directory this install made, if it is empty.
    ///
    /// Only when it is empty, which `remove_dir` decides by failing otherwise —
    /// and empty is exactly the case where it holds nothing anybody wants. That
    /// is also what keeps a sibling safe: another artifact of the same version
    /// leaves the version directory populated, so it stays.
    /// Silent, because a directory that will not go costs one inode and saying
    /// so would bury the failure that actually matters.
    fn sweep(&self, directory: &Path) {
        let _ = remove_directory_beneath(&self.root, directory);
    }

    /// Unpack every entry the release names, into files this store chose.
    ///
    /// One pass over the archive rather than one per file: these archives are
    /// a hundred megabytes of gzip, and seven passes over Codex's would decode
    /// it seven times to write it once.
    ///
    /// Each destination that is renamed into place is pushed onto `written`
    /// before anything else can fail, so that the caller can take back exactly
    /// what this call put there and nothing else. That is why it is an
    /// out-parameter rather than a return value: a failure part way through has
    /// files to undo, and a `Result` alone would carry nothing to undo them by.
    ///
    /// `budget` is spent across the whole release, and
    /// [`StoreFailure::IncompleteArchive`] is the archive simply not containing
    /// a file the pin names — a pin that is wrong about its own contents rather
    /// than a machine that failed.
    fn unpack(
        &self,
        agent: &AgentName,
        release: &PinnedRelease,
        staged: &mut StagedArchive,
        budget: u64,
        written: &mut Vec<PathBuf>,
    ) -> Result<(), StoreFailure> {
        staged.file_mut().rewind().map_err(unreadable)?;
        // `MultiGzDecoder`, not `GzDecoder`: the latter stops at the end of the
        // first gzip member, so a multi-member archive whose entry lives past
        // it would be reported as an archive that does not contain the pinned
        // executable — blaming the pin for a decoder that stopped early.
        let mut tar = Archive::new(MultiGzDecoder::new(staged.file_mut()));
        let entries = tar.entries().map_err(malformed)?;
        // Keyed by the bytes of the path the pin gives, and emptied as each one
        // is found, so that what is left at the end is exactly what the archive
        // did not hold.
        let mut wanted: BTreeMap<&[u8], &ReleaseFile> = release
            .contents()
            .files()
            .iter()
            .map(|file| (file.path().as_str().as_bytes(), file))
            .collect();
        let mut remaining = budget;
        for entry in entries {
            let mut entry = entry.map_err(malformed)?;
            // Compared as bytes. A lossy rendering of a name that is not UTF-8
            // turns every undecodable byte into the same replacement character,
            // so two different entries could both come to equal one pin.
            //
            // In its own scope because the name borrows the entry, and
            // everything below needs the entry back to read its data from.
            let matched = {
                let path = entry.path_bytes();
                let name = path.strip_prefix(b"./".as_slice()).unwrap_or(&path);
                match wanted.remove(name) {
                    Some(file) => Some(Ok(file)),
                    // Not wanted *now*. Either the archive carries an entry the
                    // pin says nothing about, which is simply skipped, or it
                    // carries a second copy of one it does — and then which of
                    // the two would be installed depends on nothing but which
                    // came first.
                    None => release
                        .contents()
                        .files()
                        .iter()
                        .find(|file| file.path().as_str().as_bytes() == name)
                        .map(|file| Err(file.path().clone())),
                }
            };
            let file = match matched {
                None => continue,
                Some(Err(path)) => {
                    return Err(StoreFailure::MalformedArchive(format!(
                        "{path} is in the archive twice"
                    )));
                }
                Some(Ok(file)) => file,
            };
            // A link or a directory carries no data, so unpacking one writes an
            // empty file that this store would then record as part of the
            // installed runtime and hand out as something to launch.
            if let Some(reason) = refused_kind(entry.header().entry_type()) {
                return Err(StoreFailure::MalformedArchive(format!(
                    "{} {reason}",
                    file.path()
                )));
            }
            // The entry's own path is never used to decide where bytes land:
            // the destination is computed from the pin. An archive cannot
            // direct this write anywhere, whatever its entries claim to be
            // called.
            let destination = self.file_path(agent, release, file.path());
            // Staged in the directory the file goes in, so the rename that
            // publishes it never crosses a directory this install did not make.
            let directory = destination
                .parent()
                .expect("a file beneath the artifact directory has a parent")
                .to_path_buf();
            let mut staging =
                PrivateTempFile::new_beneath(&self.root, &directory).map_err(unwritable)?;
            // Bounded, because the digest that has already matched says nothing
            // about how far these bytes expand. One byte over what is left is
            // read deliberately, so that reaching the bound is distinguishable
            // from a file that happens to be exactly that size. `budget` is
            // passed in rather than read from the constant so that the bound can
            // be exercised by a test without moving a gigabyte.
            // `Entry::size`, not `header().size()`: a PAX extension can carry
            // the real length for an entry whose header field cannot hold it,
            // and the header's own number is then not the one to hold the
            // archive to.
            let declared = entry.size();
            let mut bounded = entry.by_ref().take(remaining + 1);
            let unpacked = expand(&mut bounded, staging.as_file_mut(), file.path())?;
            if unpacked > remaining {
                return Err(StoreFailure::MalformedArchive(format!(
                    "{} unpacks past what is left of the {budget} bytes this release may unpack to",
                    file.path()
                )));
            }
            remaining -= unpacked;
            // Against the header rather than against zero. An entry whose data
            // was cut short still decompresses to something, and a `> 0` test
            // is satisfied by one byte of it — which would be made executable,
            // recorded as the tested runtime and handed out to launch. The
            // header is the archive's own statement of how long the file is, so
            // this holds the archive to it.
            if unpacked != declared {
                return Err(StoreFailure::MalformedArchive(format!(
                    "{} is {unpacked} bytes in the archive, which says it is {declared}",
                    file.path()
                )));
            }
            if unpacked == 0 {
                return Err(StoreFailure::MalformedArchive(format!(
                    "{} is empty in the archive",
                    file.path()
                )));
            }
            set_mode(staging.as_file(), file.role()).map_err(unwritable)?;
            staging.as_file().sync_all().map_err(unwritable)?;
            // The rename is the last thing done to this file, and the caller is
            // told about it in the same breath. Making the directories durable
            // belongs to the caller too, because from here on a failure has a
            // file to take back out — and recording it after some later `?`
            // would leave a window in which one exists and nothing knows.
            staging.persist_beneath(&destination).map_err(unwritable)?;
            written.push(destination);
        }
        // Whatever is left was named by the pin and not carried by the archive.
        // The first by path, so the message does not depend on the order the
        // entries happened to come in.
        match wanted.values().next() {
            Some(file) => Err(StoreFailure::IncompleteArchive(
                file.path().as_str().to_owned(),
            )),
            None => Ok(()),
        }
    }
    /// Publish a runtime, forcing directories to the disk with `durable`.
    ///
    /// The whole of [`RuntimeStore::publish`], which supplies the real
    /// primitive. It is a parameter for the reason [`Self::settle`] gives —
    /// the walk it decides leaves no trace on a real filesystem — and keeping
    /// it threaded this far means the guarded region, rollback included, has
    /// somewhere to be observed from. A failure to settle is otherwise only
    /// reachable on a disk that has run out part-way through an install.
    fn recover_publication(
        &self,
        recovery: PublicationRecoveryContext<'_>,
        durable: impl Fn(&Path) -> io::Result<()> + Copy,
        withdraw: impl Fn(&[PathBuf]) -> Result<(), StoreFailure> + Copy,
        confirm: impl Fn() -> Result<Option<RuntimeArtifact>, StoreFailure> + Copy,
    ) -> PublishFailure {
        let withdrawal = withdraw(recovery.written).err();
        let restoration = self
            .restore_record(recovery.agent, recovery.previous_record, durable)
            .err();
        let (rollback, confirmation) = match confirm() {
            Ok(confirmed) if confirmed == recovery.previous_artifact => {
                let rollback = match confirmed {
                    Some(artifact) => RollbackChange::Restored(artifact),
                    None => RollbackChange::NoInstalledRuntime,
                };
                (Some(rollback), None)
            }
            Ok(confirmed) => (
                None,
                Some(StoreFailure::Unreadable(format!(
                    "publication recovery confirmed unexpected installed state: {confirmed:?}"
                ))),
            ),
            Err(error) => (None, Some(error)),
        };
        match PublicationCleanupFailure::new(withdrawal, restoration, confirmation) {
            Some(cleanup) => PublishFailure::incomplete(
                recovery.failure,
                rollback,
                cleanup,
                Box::new(recovery.lock),
            ),
            None => PublishFailure::rolled_back(
                recovery.failure,
                rollback.expect("successful recovery confirms a rollback state"),
                Box::new(recovery.lock),
            ),
        }
    }

    /// Publish one complete multi-file runtime while retaining its lock through
    /// the caller's immediate audit attempt. Returning an audit error drops the
    /// lease before any later redelivery.
    fn publish_durably(
        &self,
        agent: &AgentName,
        release: &PinnedRelease,
        staged: &mut StagedArchive,
        durable: impl Fn(&Path) -> io::Result<()> + Copy,
    ) -> Result<Publication, PublishFailure> {
        self.publish_with_recovery(
            agent,
            release,
            staged,
            durable,
            |written| self.withdraw(agent, release, written),
            || self.recorded_artifact(agent),
        )
    }

    fn publish_with_recovery(
        &self,
        agent: &AgentName,
        release: &PinnedRelease,
        staged: &mut StagedArchive,
        durable: impl Fn(&Path) -> io::Result<()> + Copy,
        withdraw: impl Fn(&[PathBuf]) -> Result<(), StoreFailure> + Copy,
        confirm: impl Fn() -> Result<Option<RuntimeArtifact>, StoreFailure> + Copy,
    ) -> Result<Publication, PublishFailure> {
        let lock = self
            .hold(agent)
            .map_err(|failure| PublishFailure::unchanged(failure, Box::new(())))?;
        let installed = match self.installed_observation(agent, release) {
            Ok(installed) => installed,
            Err(failure) => {
                return Err(PublishFailure::unchanged(failure, Box::new(lock)));
            }
        };
        if let Some(installed) = installed {
            return Ok(Publication::new(
                installed,
                PublicationChange::Reused,
                Box::new(ManagedPublicationLease {
                    root: self.root.clone(),
                    agent: agent.clone(),
                    _lock: lock,
                }),
            ));
        }

        let previous_record = match self.installation_record(agent) {
            Ok(record) => record,
            Err(failure) => {
                return Err(PublishFailure::unchanged(failure, Box::new(lock)));
            }
        };
        let previous_artifact = match self.held_artifact(
            agent,
            previous_record
                .as_ref()
                .and_then(InstallationRecord::artifact),
        ) {
            Ok(artifact) => artifact,
            Err(failure) => {
                return Err(PublishFailure::unchanged(failure, Box::new(lock)));
            }
        };

        if let Err(failure) = self.private_directory(&self.artifact_root(agent, release)) {
            return Err(PublishFailure::unchanged(failure, Box::new(lock)));
        }
        for directory in self.content_directories(agent, release) {
            if let Err(failure) = self.private_directory(&directory) {
                return Err(PublishFailure::unchanged(failure, Box::new(lock)));
            }
        }

        let mut written = Vec::new();
        if let Err(failure) =
            self.unpack(agent, release, staged, MAXIMUM_UNPACKED_BYTES, &mut written)
        {
            return Err(self.recover_publication(
                PublicationRecoveryContext {
                    agent,
                    written: &written,
                    previous_record: previous_record.as_ref(),
                    previous_artifact,
                    failure,
                    lock,
                },
                durable,
                withdraw,
                confirm,
            ));
        }
        if let Err(failure) = self.settle(agent, release, durable) {
            return Err(self.recover_publication(
                PublicationRecoveryContext {
                    agent,
                    written: &written,
                    previous_record: previous_record.as_ref(),
                    previous_artifact,
                    failure,
                    lock,
                },
                durable,
                withdraw,
                confirm,
            ));
        }

        let change = match previous_artifact {
            Some(previous) => PublicationChange::Replaced(previous),
            None => PublicationChange::Installed,
        };
        Ok(Publication::new(
            self.absolute(&self.launch_path(agent, release)),
            change,
            Box::new(ManagedPublicationLease {
                root: self.root.clone(),
                agent: agent.clone(),
                _lock: lock,
            }),
        ))
    }
}

impl ManagedRuntimes {
    fn installed_observation(
        &self,
        agent: &AgentName,
        release: &PinnedRelease,
    ) -> Result<Option<PathBuf>, StoreFailure> {
        // Opened through the private-file primitive rather than read by path:
        // the record is a file this store wrote, so it is entitled to insist on
        // one it recognises — a single link, owned by this user, readable by
        // nobody else, and not a symbolic link to somewhere else's json.
        // `fs::read` would have followed that link and answered from it.
        let mut record = match open_beneath(
            &self.root,
            &self.record_path(agent),
            OpenMode::ReadNonblocking,
        ) {
            Ok(file) => file,
            // Nothing recorded is a real "nothing is installed". Any other
            // failure is this machine declining to answer, and is reported
            // rather than turned into a missing runtime that would be
            // reinstalled over a working one.
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(unreadable(error)),
        };
        let mut encoded = Vec::new();
        record.read_to_end(&mut encoded).map_err(unreadable)?;
        // A file that will not decode into a record is not this machine
        // declining to answer — it is a file this store did not write, and it
        // describes no installation. Reported as one would be a dead end: the
        // pinned release is known and could simply be installed, but every
        // attempt would fail on the same unreadable note, with nothing a person
        // could do about it short of finding the file and deleting it. The
        // record is written to a temporary name and renamed, so it is never
        // half-written; anything that does not decode was written by something
        // else, or by a Nessa that recorded a different shape.
        //
        // Failing to *open* it stays a failure, above: that is the disk, and a
        // machine that cannot read its own record must not be told it has
        // nothing installed and sent to download over a runtime that is there.
        let Ok(record) = serde_json::from_slice::<InstallationRecord>(&encoded) else {
            return Ok(None);
        };
        // Anything that does not describe exactly this artifact is answered
        // with "not installed", never with a failure: a record this store did
        // not write, one something has since edited, and one about a different
        // artifact all describe nothing that can be launched here, and the
        // pinned release is known and can simply be installed over it.
        //
        // Every field is compared, the digest included. A record agreeing only
        // on the version and the file's name is the case this exists for: one
        // version of Opencode ships as nine archives, one per platform, C
        // library and processor baseline, each unpacking a file called
        // `opencode`. Matching on those two would hand back whichever of the
        // nine happened to be installed — quite possibly one this machine
        // cannot execute at all — and report it as the tested runtime.
        if !record.describes(release) {
            return Ok(None);
        }
        // Every file, not only the one that is launched. A record is a note
        // rather than evidence, and a runtime whose ripgrep a disk cleaner took
        // away is one that starts and then cannot search — reported as ready,
        // with nothing saying why it is not. Seven opens rather than one is
        // nothing beside the download this answer avoids.
        for file in release.contents().files() {
            // The path is recomputed from the release rather than read back out
            // of the record, so `installed.json` cannot name a launch path of
            // its own choosing: the most a rewritten record can do is make Nessa
            // install again.
            if !self.holds(&self.file_path(agent, release, file.path()))? {
                return Ok(None);
            }
        }
        Ok(Some(self.absolute(&self.launch_path(agent, release))))
    }
}

impl RuntimeStore for ManagedRuntimes {
    fn installed(
        &self,
        agent: &AgentName,
        release: &PinnedRelease,
    ) -> Result<Option<PathBuf>, StoreFailure> {
        self.installed_observation(agent, release)
    }

    fn managed_launch(
        &self,
        agent: &AgentName,
        release: &PinnedRelease,
    ) -> Result<Option<ManagedLaunchSnapshot>, StoreFailure> {
        let _publication = self.hold(agent)?;
        let Some(executable) = self.installed_observation(agent, release)? else {
            return Ok(None);
        };
        let artifact = RuntimeArtifact::for_release(release);
        let artifact_lock = self.hold_artifact_for_launch(agent, &artifact)?;
        let marker_directory = self.use_marker_directory(agent, &artifact);
        self.private_directory(&marker_directory)?;
        self.validate_use_inventory(&marker_directory, &self.use_inventory(agent, &artifact))?;
        let authority = Arc::new(DurableManagedExecutableUse {
            root: self.root.clone(),
            marker_directory,
            agent: agent.clone(),
            artifact,
            _artifact_lock: artifact_lock,
        });
        Ok(Some(ManagedLaunchSnapshot::new(executable, authority)))
    }

    fn reclamation_lease(
        &self,
        agent: &AgentName,
    ) -> Result<Box<dyn PublicationLease>, StoreFailure> {
        Ok(Box::new(ManagedPublicationLease {
            root: self.root.clone(),
            agent: agent.clone(),
            _lock: self.hold(agent)?,
        }))
    }

    fn stage(&self, agent: &AgentName) -> Result<StagedArchive, StoreFailure> {
        let directory = self.agent_root(agent);
        self.private_directory(&directory)?;
        for _ in 0..NAME_ATTEMPTS {
            let mut random = [0u8; 16];
            getrandom::fill(&mut random)
                .map_err(|error| StoreFailure::Unwritable(error.to_string()))?;
            let name: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
            let path = directory.join(format!(".nessa-{name}.download"));
            // Created, not opened: two installs running at once each get a file
            // of their own, and neither can truncate a download the other is
            // still measuring.
            match open_beneath(&self.root, &path, OpenMode::CreateNew) {
                Ok(file) => {
                    self.release_name(&path)?;
                    return Ok(StagedArchive::new(file, self.absolute(&path)));
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(at(&self.absolute(&path), error)),
            }
        }
        Err(StoreFailure::Unwritable(
            "could not reserve a file to download into".into(),
        ))
    }

    fn digest(&self, staged: &mut StagedArchive) -> Result<ArchiveDigest, StoreFailure> {
        let file = staged.file_mut();
        file.rewind().map_err(unreadable)?;
        let mut hasher = Sha256::new();
        // Streamed rather than read whole: these archives are around a hundred
        // megabytes, and holding one in memory to hash it would be paid again
        // when it is unpacked.
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let read = file.read(&mut buffer).map_err(unreadable)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        let hex = hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        ArchiveDigest::parse(&hex).map_err(|error| StoreFailure::Unreadable(error.to_string()))
    }

    fn publish(
        &self,
        agent: &AgentName,
        release: &PinnedRelease,
        staged: &mut StagedArchive,
    ) -> Result<Publication, PublishFailure> {
        self.publish_durably(agent, release, staged, |relative| {
            sync_directory_beneath(&self.root, relative)
        })
    }

    fn discard(&self, staged: StagedArchive) {
        let path = staged.path().to_owned();
        // Turned back into the name this store gave it. A path that is not
        // beneath this root is not one this store staged, and removing it is
        // not this method's business whatever it was handed.
        let Ok(relative) = path.strip_prefix(&self.root).map(Path::to_path_buf) else {
            tracing::debug!(
                path = %path.display(),
                "declined to discard an archive that is not in this store"
            );
            return;
        };
        // The handle first: on Windows a file that is still open is one the
        // filesystem will not let go of. On Unix the name went at the moment
        // the file was created, so this finds nothing and that is the answer.
        drop(staged);
        // Survivable: a leftover archive in a directory Nessa owns costs disk
        // and nothing else, and failing an install that otherwise worked
        // because a temporary file would not delete would be worse.
        if let Err(error) = remove_file_beneath(&self.root, &relative) {
            if error.kind() != io::ErrorKind::NotFound {
                tracing::debug!(
                    path = %path.display(),
                    %error,
                    "could not discard a downloaded agent runtime archive"
                );
            }
        }
    }
}

/// Let go of the name a staged download was created under.
///
/// The handle is what the install works through, and an open file with no name
/// is one nothing else can reach: not to truncate between the hash and the
/// unpack, not to replace, not to read. It also cannot be left behind by a
/// process that dies part-way, which is what the explicit discard is for on the
/// platform that does not allow this.
///
/// Windows will not unlink a file that is still open, so there the name stays
/// until [`RuntimeStore::discard`] removes it — which means an install killed
/// part-way leaves its download behind on that platform, and nothing sweeps it.
/// Nessa pins no Windows release today, so nothing reaches this yet.
#[cfg(unix)]
impl ManagedRuntimes {
    fn release_name(&self, relative: &Path) -> Result<(), StoreFailure> {
        // Named in the failure because this is the one path where the file is
        // created and then abandoned: the handle is dropped with the install,
        // and an empty file nobody swept is left under a name only this
        // message gives.
        remove_file_beneath(&self.root, relative)
            .map_err(|error| at(&self.absolute(relative), error))
    }
}

#[cfg(not(unix))]
impl ManagedRuntimes {
    fn release_name(&self, _relative: &Path) -> Result<(), StoreFailure> {
        Ok(())
    }
}

/// Move one entry's data onto a staging file, keeping the two faults apart.
///
/// Not `io::copy`, for the same reason the HTTPS client does not use it: a read
/// that fails here is the archive coming apart — a gzip stream cut short, a
/// header that does not agree with its data — and a write that fails is this
/// machine. Reporting a corrupt archive as "could not write the agent runtime"
/// sends somebody to look at their disk over a pin that is wrong.
fn expand(
    entry: &mut impl Read,
    staging: &mut File,
    path: &ArchivePath,
) -> Result<u64, StoreFailure> {
    let mut buffer = vec![0u8; UNPACK_CHUNK];
    let mut unpacked: u64 = 0;
    loop {
        let read = entry.read(&mut buffer).map_err(|error| {
            // Branched on the kind, not on the message. The readers stacked
            // underneath this — tar over gzip over the staged file — report a
            // stream that does not decode as invalid or as ending early, and
            // pass a real device failure through as itself. Calling the second
            // one a malformed archive would be this function committing the
            // mistake it exists to avoid, in the other direction: `digest`
            // reports the same fault on the same handle as `Unreadable`.
            match error.kind() {
                io::ErrorKind::InvalidData
                | io::ErrorKind::InvalidInput
                | io::ErrorKind::UnexpectedEof => StoreFailure::MalformedArchive(format!(
                    "{path} could not be read out of the archive: {error}"
                )),
                _ => StoreFailure::Unreadable(format!(
                    "reading the downloaded archive back: {error}"
                )),
            }
        })?;
        if read == 0 {
            return Ok(unpacked);
        }
        unpacked += read as u64;
        staging.write_all(&buffer[..read]).map_err(unwritable)?;
    }
}

/// Why an entry of this kind cannot be one of a release's files, if it cannot.
///
/// A link entry names another file and carries no data; a directory carries
/// none either. Neither is a file to install, and unpacking one writes an empty
/// one that this store would then record as part of the installed runtime and
/// hand out as something to launch.
///
/// A sparse entry is refused for the same reason — what would be written is not
/// the file the archive describes — but it gets its own sentence, because it
/// *is* a regular file and being told it is not would send whoever reads the
/// message looking for something that is not the problem.
///
/// Its own function, returning the reason rather than a bool, so that the
/// wording for each kind can be tested without an archive: a sparse entry
/// cannot be built in memory by the tar writer, and building one with the
/// system `tar` would make this suite depend on which `tar` is installed.
fn refused_kind(kind: EntryType) -> Option<&'static str> {
    match kind {
        EntryType::Regular | EntryType::Continuous => None,
        EntryType::GNUSparse => Some("is stored sparsely, which nessa does not unpack"),
        _ => Some("is not a regular file in the archive"),
    }
}

/// Whether the handle taken on `path` is still the file that `path` names.
///
/// The lock lives on an open file, not on a name, so a lock file replaced
/// between the open and the lock would leave two installs each holding a
/// different file and each believing it was the only one publishing. Compared
/// on device and inode, which is the pair that identifies a file rather than a
/// way of reaching one — and the name is resolved through the same anchored
/// open as everything else here, so what is compared against is a file beneath
/// this root rather than wherever a link now points.
#[cfg(unix)]
impl ManagedRuntimes {
    fn same_file(&self, lock: &File, relative: &Path) -> Result<bool, StoreFailure> {
        let held = lock.metadata().map_err(unwritable)?;
        match open_beneath(&self.root, relative, OpenMode::Read) {
            Ok(named) => {
                let named = named.metadata().map_err(unwritable)?;
                Ok(held.dev() == named.dev() && held.ino() == named.ino())
            }
            // Removed rather than replaced. The handle is still a lock nobody
            // else can take through it, but the next install would create a new
            // file and take a second one, so this one excludes nothing. The
            // same goes for a name that is no longer a private file of this
            // user's: it is not the lock this handle holds.
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => Ok(false),
            Err(error) => Err(at(&self.absolute(relative), error)),
        }
    }
}

/// Windows keeps the name for as long as the file is open, so the file that
/// was locked is the file at that path by construction.
#[cfg(not(unix))]
impl ManagedRuntimes {
    fn same_file(&self, _lock: &File, _relative: &Path) -> Result<bool, StoreFailure> {
        Ok(true)
    }
}

/// A write failure, said of the path it happened to.
fn at(path: &Path, error: io::Error) -> StoreFailure {
    StoreFailure::Unwritable(format!("{}: {error}", path.display()))
}

fn unwritable(error: io::Error) -> StoreFailure {
    StoreFailure::Unwritable(error.to_string())
}

fn unreadable(error: io::Error) -> StoreFailure {
    StoreFailure::Unreadable(error.to_string())
}

fn malformed(error: io::Error) -> StoreFailure {
    StoreFailure::MalformedArchive(error.to_string())
}

/// Give a freshly written file the mode its role asks for, and nothing more.
///
/// The tar entry carries a mode and it is not used. The archive is a thing
/// being verified, not a thing to be believed about what it may run: the *pin*
/// says which files are programs, so a release that turned a document into an
/// executable by flipping a bit in its own header would change nothing here.
/// That is what `0o600` for a document buys — Codex ships three files that are
/// read rather than run, and none of them becomes runnable because the archive
/// said so.
///
/// Set through the open handle rather than by path, so it lands on the file
/// that was just written and not on whatever the name happens to mean by now.
///
/// Owner-only either way, like every other file this store writes. The
/// directories above it are `0o700` already, so the group and world bits a
/// release archive usually carries grant nothing — and dropping them is what
/// lets [`RuntimeStore::installed`] check the file through the same
/// private-file primitive as the record, instead of settling for what a stat
/// can see.
#[cfg(unix)]
fn set_mode(file: &File, role: FileRole) -> io::Result<()> {
    let mode = if role.runnable() { 0o700 } else { 0o600 };
    file.set_permissions(Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_mode(_file: &File, _role: FileRole) -> io::Result<()> {
    // Windows decides executability by extension, so there is nothing to set.
    Ok(())
}

#[cfg(test)]
#[path = "../../../tests/agent_install/managed_runtimes.rs"]
mod tests;
