use std::fs::{self, File};
use std::io::{self, Read, Seek, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use flate2::read::MultiGzDecoder;
use nessa_local_storage::{create_directory, open, sync_directory, OpenMode, PrivateTempFile};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tar::EntryType;

use crate::agent_install::application::{RuntimeStore, StagedArchive, StoreFailure};
use crate::agent_install::domain::{AgentName, ArchiveDigest, PinnedRelease, ReleaseVersion};

/// How many times a staged download will try for a name of its own before
/// giving up. The names are sixteen random bytes, so a single collision already
/// means something other than chance.
const NAME_ATTEMPTS: u8 = 10;

/// The most an unpacked executable may be.
///
/// The digest fixes the *compressed* size of an archive and says nothing about
/// what comes out of it: a gzip member that matches its pin exactly can still
/// expand a thousandfold. A few times the largest runtime Nessa pins, so the
/// bound is only ever reached by an archive that is not what it claims to be.
const MAXIMUM_EXECUTABLE_BYTES: u64 = 512 * 1024 * 1024;

/// How much of an entry is moved out of the archive at a time.
const UNPACK_CHUNK: usize = 64 * 1024;

/// What is recorded beside an installed runtime.
///
/// Nessa's own note about what it put there, not anything read out of the
/// archive. The version in particular is the *pinned* one: asking a downloaded
/// binary what version it is would be asking the thing we are trying to verify.
///
/// `executable` is a file name, not a path. A record holding an absolute path
/// would be a second, independent claim about where runtimes live, free to
/// disagree with the store that wrote it and still name a real file — so the
/// only thing kept is the one part the store cannot work out for itself, and
/// the rest is recomputed from the root and the version every time.
#[derive(Debug, Serialize, Deserialize)]
struct InstallationRecord {
    version: String,
    executable: String,
}

/// The runtimes Nessa installed, under one directory it owns.
///
/// ```text
/// <root>/<agent>/installed.json      what is installed, written last
/// <root>/<agent>/<version>/<name>    the executable itself
/// <root>/<agent>/.nessa-<hex>.download  a download in progress, unnamed on unix
/// <root>/<agent>/.nessa-<hex>.tmp       a record or executable being written
/// ```
///
/// A runtime is unpacked under its version rather than over the previous one,
/// so a pin that moves does not half-overwrite a binary that something may
/// still be running. `installed.json` is written only once the executable is in
/// place *and* on the disk, which is what makes a crashed install read as
/// "nothing installed" rather than as a runtime that is not there.
///
/// Every directory here is created private to this user, and every file this
/// type publishes is written to a temporary name, synced, and renamed into
/// place. A machine that loses power mid-install comes back with either the
/// previous answer or the new one, never a half of either.
pub struct ManagedRuntimes {
    root: PathBuf,
}

impl ManagedRuntimes {
    /// Manage runtimes under `root`, which composition owns and which is
    /// expected to be inside Nessa's own data directory.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The directory holding one agent's runtimes.
    ///
    /// The name is a [`AgentName`], which is the type that says a name can also
    /// be a directory name, so there is nothing to re-check here. Everything in
    /// this type routes through this method, so the whole store is anchored to
    /// `root` in one place.
    fn agent_root(&self, agent: &AgentName) -> PathBuf {
        self.root.join(agent.as_str())
    }

    /// Where `agent`'s executable lives for `version`.
    fn version_root(&self, agent: &AgentName, version: &ReleaseVersion) -> PathBuf {
        self.agent_root(agent).join(version.as_str())
    }

    fn record_path(&self, agent: &AgentName) -> PathBuf {
        self.agent_root(agent).join("installed.json")
    }

    /// Note what is now installed, once it really is.
    ///
    /// Written to a private temporary file, forced to the disk, and renamed, so
    /// a reader never sees a half-written record and a crash cannot leave one
    /// that names an executable the disk does not have. The directory is synced
    /// afterwards so the rename itself survives, which also makes durable the
    /// version directory created a moment earlier.
    fn record(&self, agent: &AgentName, release: &PinnedRelease) -> Result<(), StoreFailure> {
        let record = InstallationRecord {
            version: release.version().as_str().to_owned(),
            executable: release.executable().file_name().to_owned(),
        };
        let directory = self.agent_root(agent);
        let mut staging = PrivateTempFile::new_in(&directory).map_err(unwritable)?;
        serde_json::to_writer_pretty(staging.as_file_mut(), &record)
            .map_err(|error| StoreFailure::Unwritable(error.to_string()))?;
        staging.as_file().sync_all().map_err(unwritable)?;
        staging
            .persist(&self.record_path(agent))
            .map_err(unwritable)?;
        sync_directory(&directory).map_err(unwritable)
    }

    /// Take an executable back out after the install failed to complete.
    ///
    /// Best effort on purpose, and the reason it returns nothing: the caller
    /// already has a failure to report, and it is the one worth reporting. A
    /// second one about the clean-up would replace the cause with its
    /// consequence. What cannot be removed is logged, so that a directory
    /// holding an unrecorded runtime is at least explainable.
    fn withdraw(&self, executable: &Path) {
        if let Err(error) = fs::remove_file(executable) {
            if error.kind() != io::ErrorKind::NotFound {
                tracing::debug!(
                    path = %executable.display(),
                    %error,
                    "could not withdraw an agent runtime that was not recorded"
                );
            }
        }
    }

    /// Unpack the one entry the release names, into a file this store chose.
    ///
    /// Reports `false` when the archive simply does not contain it, which is a
    /// pin that is wrong about its own contents rather than a machine that
    /// failed.
    fn unpack(
        &self,
        release: &PinnedRelease,
        staged: &mut StagedArchive,
        destination: &Path,
        directory: &Path,
        limit: u64,
    ) -> Result<bool, StoreFailure> {
        staged.file_mut().rewind().map_err(unreadable)?;
        // `MultiGzDecoder`, not `GzDecoder`: the latter stops at the end of the
        // first gzip member, so a multi-member archive whose entry lives past
        // it would be reported as an archive that does not contain the pinned
        // executable — blaming the pin for a decoder that stopped early.
        let mut tar = tar::Archive::new(MultiGzDecoder::new(staged.file_mut()));
        let entries = tar.entries().map_err(malformed)?;
        let wanted = release.executable().as_str().as_bytes();
        for entry in entries {
            let mut entry = entry.map_err(malformed)?;
            // Compared as bytes. A lossy rendering of a name that is not UTF-8
            // turns every undecodable byte into the same replacement character,
            // so two different entries could both come to equal one pin.
            let path = entry.path_bytes();
            if path.strip_prefix(b"./".as_slice()).unwrap_or(&path) != wanted {
                continue;
            }
            // A link or a directory carries no data, so unpacking one writes an
            // empty file that this store would then record as the installed
            // runtime and hand out as something to launch.
            if !is_regular(entry.header().entry_type()) {
                return Err(StoreFailure::MalformedArchive(format!(
                    "{} is not a regular file in the archive",
                    release.executable()
                )));
            }
            // The entry's own path is never used to decide where bytes land:
            // the destination was computed from the pin. An archive cannot
            // direct this write anywhere, whatever its entries claim to be
            // called.
            let mut staging = PrivateTempFile::new_in(directory).map_err(unwritable)?;
            // Bounded, because the digest that has already matched says nothing
            // about how far these bytes expand. One byte over the limit is read
            // deliberately, so that reaching it is distinguishable from an
            // executable that happens to be exactly that size. `limit` is
            // passed in rather than read from the constant so that the bound can
            // be exercised by a test without moving half a gigabyte.
            // `Entry::size`, not `header().size()`: a PAX extension can carry
            // the real length for an entry whose header field cannot hold it,
            // and the header's own number is then not the one to hold the
            // archive to.
            let declared = entry.size();
            let mut bounded = entry.by_ref().take(limit + 1);
            let unpacked = expand(&mut bounded, staging.as_file_mut(), release)?;
            if unpacked > limit {
                return Err(StoreFailure::MalformedArchive(format!(
                    "{} unpacks to more than {limit} bytes",
                    release.executable()
                )));
            }
            // Against the header rather than against zero. An entry whose data
            // was cut short still decompresses to something, and a `> 0` test
            // is satisfied by one byte of it — which would be made executable,
            // recorded as the tested runtime and handed out to launch. The
            // header is the archive's own statement of how long the file is, so
            // this holds the archive to it.
            if unpacked != declared {
                return Err(StoreFailure::MalformedArchive(format!(
                    "{} is {unpacked} bytes in the archive, which says it is {declared}",
                    release.executable()
                )));
            }
            if unpacked == 0 {
                return Err(StoreFailure::MalformedArchive(format!(
                    "{} is empty in the archive",
                    release.executable()
                )));
            }
            make_executable(staging.as_file()).map_err(unwritable)?;
            staging.as_file().sync_all().map_err(unwritable)?;
            staging.persist(destination).map_err(unwritable)?;
            sync_directory(directory).map_err(unwritable)?;
            return Ok(true);
        }
        Ok(false)
    }
}

impl RuntimeStore for ManagedRuntimes {
    fn installed(
        &self,
        agent: &AgentName,
        release: &PinnedRelease,
    ) -> Result<Option<PathBuf>, StoreFailure> {
        // Opened through the private-file primitive rather than read by path:
        // the record is a file this store wrote, so it is entitled to insist on
        // one it recognises — a single link, owned by this user, readable by
        // nobody else, and not a symbolic link to somewhere else's json.
        // `fs::read` would have followed that link and answered from it.
        let mut record = match open(&self.record_path(agent), OpenMode::ReadNonblocking) {
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
        let record: InstallationRecord = serde_json::from_slice(&encoded)
            .map_err(|error| StoreFailure::Unreadable(error.to_string()))?;
        // Everything below that does not match is answered with "not
        // installed", never with a failure: a record this store did not write,
        // or one something has since edited, describes nothing that can be
        // launched, and the pinned release is known and can simply be
        // installed over it.
        let Ok(version) = ReleaseVersion::parse(&record.version) else {
            return Ok(None);
        };
        if &version != release.version() || record.executable != release.executable().file_name() {
            return Ok(None);
        }
        // The path is recomputed from the release rather than read back out of
        // the record, so `installed.json` cannot name a launch path of its own
        // choosing: the most a rewritten record can do is make Nessa install
        // again. The record is still only a note, so an executable that has
        // since been deleted — by a disk cleaner, or by someone tidying up —
        // makes it stale, and an empty one is a runtime that cannot start.
        let executable = self
            .version_root(agent, release.version())
            .join(release.executable().file_name());
        // `symlink_metadata`, not `metadata`: the second follows a symbolic
        // link and reports on whatever it points at, so a link planted at this
        // path would be handed back as the tested runtime — with no download,
        // no digest and none of the archive's own checks ever running. A link
        // is not what this store published, so it is not an installation.
        let installed = match fs::symlink_metadata(&executable) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            // Not "nothing is installed": a directory that cannot be searched,
            // or a path that will not resolve, is this machine declining to
            // answer. Reporting it as missing would send every later attempt
            // to download a hundred megabytes and fail at the same place.
            Err(error) => return Err(unreadable(error)),
        };
        Ok((installed.is_file() && installed.len() > 0).then_some(executable))
    }

    fn stage(&self, agent: &AgentName) -> Result<StagedArchive, StoreFailure> {
        let directory = self.agent_root(agent);
        private_directory(&directory)?;
        for _ in 0..NAME_ATTEMPTS {
            let mut random = [0u8; 16];
            getrandom::fill(&mut random)
                .map_err(|error| StoreFailure::Unwritable(error.to_string()))?;
            let name: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
            let path = directory.join(format!(".nessa-{name}.download"));
            // Created, not opened: two installs running at once each get a file
            // of their own, and neither can truncate a download the other is
            // still measuring.
            match open(&path, OpenMode::CreateNew) {
                Ok(file) => {
                    release_name(&path)?;
                    return Ok(StagedArchive::new(file, path));
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(at(&path, error)),
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
    ) -> Result<PathBuf, StoreFailure> {
        let directory = self.version_root(agent, release.version());
        private_directory(&directory)?;
        let destination = directory.join(release.executable().file_name());
        if !self.unpack(
            release,
            staged,
            &destination,
            &directory,
            MAXIMUM_EXECUTABLE_BYTES,
        )? {
            return Err(StoreFailure::MissingExecutable(
                release.executable().as_str().to_owned(),
            ));
        }
        // The record is written last, because it is what makes the install
        // true: `installed` answers from it, so writing it before the
        // executable exists would claim an install that does not.
        //
        // That ordering leaves one window. If the record cannot be written, the
        // executable is already durable, and returning the failure on its own
        // would tell somebody nothing was installed while a hundred megabytes
        // of runtime sat in a directory no record names, which nothing would
        // ever look at again — on a disk that, in the likeliest cause of this
        // failure, is the thing that ran out. So the executable is taken back
        // out, and the message is true when it is read.
        if let Err(failure) = self.record(agent, release) {
            self.withdraw(&destination);
            return Err(failure);
        }
        Ok(destination)
    }

    fn discard(&self, staged: StagedArchive) {
        let path = staged.path().to_owned();
        // The handle first: on Windows a file that is still open is one the
        // filesystem will not let go of. On Unix the name went at the moment
        // the file was created, so this finds nothing and that is the answer.
        drop(staged);
        // Survivable: a leftover archive in a directory Nessa owns costs disk
        // and nothing else, and failing an install that otherwise worked
        // because a temporary file would not delete would be worse.
        if let Err(error) = fs::remove_file(&path) {
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
fn release_name(path: &Path) -> Result<(), StoreFailure> {
    // Named in the failure because this is the one path where the file is
    // created and then abandoned: the handle is dropped with the install, and
    // an empty file nobody swept is left under a name only this message gives.
    fs::remove_file(path).map_err(|error| at(path, error))
}

#[cfg(not(unix))]
fn release_name(_path: &Path) -> Result<(), StoreFailure> {
    Ok(())
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
    release: &PinnedRelease,
) -> Result<u64, StoreFailure> {
    let mut buffer = vec![0u8; UNPACK_CHUNK];
    let mut unpacked: u64 = 0;
    loop {
        let read = entry.read(&mut buffer).map_err(|error| {
            StoreFailure::MalformedArchive(format!(
                "{} could not be read out of the archive: {error}",
                release.executable()
            ))
        })?;
        if read == 0 {
            return Ok(unpacked);
        }
        unpacked += read as u64;
        staging.write_all(&buffer[..read]).map_err(unwritable)?;
    }
}

/// Whether a tar entry is a file with bytes of its own.
///
/// A link entry names another file and carries no data; a directory carries
/// none either. Neither is an executable, and unpacking one writes nothing.
fn is_regular(kind: EntryType) -> bool {
    matches!(kind, EntryType::Regular | EntryType::Continuous)
}

/// Make `path` a directory only this user can reach, or say which one it was.
///
/// The refusal this can produce — a directory that already exists and is
/// readable by somebody else — names no path of its own, and a caller looking
/// at a message about "local storage" has three directories in play. Naming it
/// is the difference between an error somebody can act on and one they cannot.
fn private_directory(path: &Path) -> Result<(), StoreFailure> {
    create_directory(path).map_err(|error| at(path, error))
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

/// Make a freshly written file launchable by its owner.
///
/// A tar entry carries a mode, but it is not used: the mode is part of the
/// archive, and the one thing this installation needs is true regardless of
/// what the archive says about it. Set through the open handle rather than by
/// path, so it lands on the file that was just written and not on whatever the
/// name happens to mean by now.
#[cfg(unix)]
fn make_executable(file: &File) -> io::Result<()> {
    file.set_permissions(fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
fn make_executable(_file: &File) -> io::Result<()> {
    // Windows decides executability by extension, so there is nothing to set.
    Ok(())
}

#[cfg(test)]
#[path = "../../../tests/agent_install/managed_runtimes.rs"]
mod tests;
