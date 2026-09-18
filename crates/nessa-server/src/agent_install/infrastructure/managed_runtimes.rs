use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::agent_install::application::{InstalledRecord, RuntimeStore, StoreFailure};
use crate::agent_install::domain::{ArchiveDigest, PinnedRelease, ReleaseVersion};

/// What is recorded beside an installed runtime.
///
/// Nessa's own note about what it put there, not anything read out of the
/// archive. The version in particular is the *pinned* one: asking a downloaded
/// binary what version it is would be asking the thing we are trying to verify.
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
/// <root>/<agent>/download.tgz        a download in progress
/// ```
///
/// A runtime is unpacked under its version rather than over the previous one,
/// so a pin that moves does not half-overwrite a binary that something may
/// still be running. `installed.json` is written only once the executable is in
/// place, which is what makes a crashed install read as "nothing installed"
/// rather than as a runtime that is not there.
pub struct ManagedRuntimes {
    root: PathBuf,
}

impl ManagedRuntimes {
    /// Manage runtimes under `root`, which composition owns and which is
    /// expected to be inside Nessa's own data directory.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Where `agent`'s installed executable would be for `version`.
    ///
    /// The file name comes from the pin's own path inside the archive, so that
    /// the installed binary is called what the agent calls itself and a person
    /// looking in the directory recognises it.
    fn executable_path(
        &self,
        agent: &str,
        release: &PinnedRelease,
    ) -> Result<PathBuf, StoreFailure> {
        let name = release
            .executable()
            .as_str()
            .rsplit('/')
            .next()
            .unwrap_or("runtime");
        Ok(self
            .agent_root(agent)?
            .join(release.version().as_str())
            .join(name))
    }

    /// The directory holding one agent's runtimes.
    ///
    /// The name becomes a path segment, so it is checked here rather than
    /// trusted. Everything else in this type routes through this method, which
    /// makes this the one place a caller could otherwise have pointed the whole
    /// store somewhere outside the root — with `..`, an absolute path, or a
    /// separator — and the one place that has to refuse.
    fn agent_root(&self, agent: &str) -> Result<PathBuf, StoreFailure> {
        let plain = !agent.is_empty()
            && agent
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
        if !plain {
            return Err(StoreFailure::Unwritable(format!(
                "{agent:?} is not an agent name"
            )));
        }
        Ok(self.root.join(agent))
    }

    fn record_path(&self, agent: &str) -> Result<PathBuf, StoreFailure> {
        Ok(self.agent_root(agent)?.join("installed.json"))
    }

    /// Note what is now installed, once it really is.
    ///
    /// Written through a temporary file and renamed, so a reader never sees a
    /// half-written record: on this path the alternative is a record naming a
    /// version whose executable is not there, which is exactly the state
    /// [`RuntimeStore::installed`] promises callers cannot observe.
    fn record(
        &self,
        agent: &str,
        release: &PinnedRelease,
        executable: &Path,
    ) -> Result<(), StoreFailure> {
        let record = InstallationRecord {
            version: release.version().as_str().to_owned(),
            executable: executable.to_string_lossy().into_owned(),
        };
        let destination = self.record_path(agent)?;
        let staging = destination.with_extension("json.writing");
        let encoded = serde_json::to_vec_pretty(&record)
            .map_err(|error| StoreFailure::Unwritable(error.to_string()))?;
        fs::write(&staging, encoded)
            .map_err(|error| StoreFailure::Unwritable(error.to_string()))?;
        fs::rename(&staging, &destination)
            .map_err(|error| StoreFailure::Unwritable(error.to_string()))
    }
}

impl RuntimeStore for ManagedRuntimes {
    fn installed(&self, agent: &str) -> Result<Option<InstalledRecord>, StoreFailure> {
        let path = self.record_path(agent)?;
        let encoded = match fs::read(&path) {
            Ok(bytes) => bytes,
            // Nothing recorded is a real "nothing is installed". Any other
            // failure is this machine declining to answer, and is reported
            // rather than turned into a missing runtime that would be
            // reinstalled over a working one.
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(StoreFailure::Unreadable(error.to_string())),
        };
        let record: InstallationRecord = serde_json::from_slice(&encoded)
            .map_err(|error| StoreFailure::Unreadable(error.to_string()))?;
        let version = ReleaseVersion::parse(&record.version)
            .map_err(|error| StoreFailure::Unreadable(error.to_string()))?;
        let executable = PathBuf::from(&record.executable);
        // The record is a note, not evidence. An executable that has since been
        // deleted — by a disk cleaner, or by someone tidying up — makes the
        // record stale, and reporting it as installed would hand out a launch
        // path to a file that is not there.
        if !executable.is_file() {
            return Ok(None);
        }
        Ok(Some(InstalledRecord {
            version,
            executable,
        }))
    }

    fn scratch(&self, agent: &str) -> Result<PathBuf, StoreFailure> {
        let directory = self.agent_root(agent)?;
        fs::create_dir_all(&directory)
            .map_err(|error| StoreFailure::Unwritable(error.to_string()))?;
        Ok(directory.join("download.tgz"))
    }

    fn digest(&self, archive: &Path) -> Result<ArchiveDigest, StoreFailure> {
        let mut file =
            File::open(archive).map_err(|error| StoreFailure::Unreadable(error.to_string()))?;
        let mut hasher = Sha256::new();
        // Streamed rather than read whole: these archives are around a hundred
        // megabytes, and holding one in memory to hash it would be paid again
        // when it is unpacked.
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let read = file
                .read(&mut buffer)
                .map_err(|error| StoreFailure::Unreadable(error.to_string()))?;
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
        agent: &str,
        release: &PinnedRelease,
        archive: &Path,
    ) -> Result<PathBuf, StoreFailure> {
        let destination = self.executable_path(agent, release)?;
        let directory = destination
            .parent()
            .ok_or_else(|| StoreFailure::Unwritable("runtime path has no directory".into()))?;
        fs::create_dir_all(directory)
            .map_err(|error| StoreFailure::Unwritable(error.to_string()))?;

        let file =
            File::open(archive).map_err(|error| StoreFailure::Unreadable(error.to_string()))?;
        let mut tar = tar::Archive::new(GzDecoder::new(file));
        let entries = tar
            .entries()
            .map_err(|error| StoreFailure::MalformedArchive(error.to_string()))?;

        let wanted = release.executable().as_str();
        for entry in entries {
            let mut entry =
                entry.map_err(|error| StoreFailure::MalformedArchive(error.to_string()))?;
            let path = entry
                .path()
                .map_err(|error| StoreFailure::MalformedArchive(error.to_string()))?
                .to_string_lossy()
                .replace('\\', "/");
            if path.trim_start_matches("./") != wanted {
                continue;
            }
            // The entry's own path is never used to decide where bytes land:
            // the destination was computed above from the pin. An archive
            // cannot direct this write anywhere, whatever its entries claim to
            // be called. The archive's digest has already been matched against
            // the pin, so its size is a tested quantity rather than something
            // to defend against here.
            let staging = destination.with_extension("writing");
            let mut out = File::create(&staging)
                .map_err(|error| StoreFailure::Unwritable(error.to_string()))?;
            io::copy(&mut entry, &mut out)
                .map_err(|error| StoreFailure::Unwritable(error.to_string()))?;
            out.flush()
                .map_err(|error| StoreFailure::Unwritable(error.to_string()))?;
            drop(out);
            make_executable(&staging)
                .map_err(|error| StoreFailure::Unwritable(error.to_string()))?;
            fs::rename(&staging, &destination)
                .map_err(|error| StoreFailure::Unwritable(error.to_string()))?;
            self.record(agent, release, &destination)?;
            return Ok(destination);
        }
        Err(StoreFailure::MissingExecutable(wanted.to_owned()))
    }

    fn discard(&self, scratch: &Path) {
        // Survivable: a leftover archive in a directory Nessa owns costs disk
        // and nothing else, and failing an install that otherwise worked
        // because a temporary file would not delete would be worse.
        if let Err(error) = fs::remove_file(scratch) {
            if error.kind() != io::ErrorKind::NotFound {
                tracing::debug!(
                    path = %scratch.display(),
                    %error,
                    "could not discard a downloaded agent runtime archive"
                );
            }
        }
    }
}

/// Make a freshly written file launchable by its owner.
///
/// A tar entry carries a mode, but it is not used: the mode is part of the
/// archive, and the one thing this installation needs is true regardless of
/// what the archive says about it.
#[cfg(unix)]
fn make_executable(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> io::Result<()> {
    // Windows decides executability by extension, so there is nothing to set.
    Ok(())
}

#[cfg(test)]
#[path = "../../../tests/agent_install/managed_runtimes.rs"]
mod tests;
