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

/// How many times taking the publication lock will retry after finding that
/// the file it locked is no longer the one at that path. Each turn means
/// something outside Nessa replaced the lock file at exactly the wrong moment,
/// so more than a couple is not a race being lost.
const LOCK_ATTEMPTS: u8 = 5;

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
#[derive(Debug, Serialize, Deserialize)]
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
    /// The entry inside the archive, exactly as the pin names it.
    executable: String,
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
            executable: release.executable().as_str().to_owned(),
        }
    }

    /// Whether this record describes the file the layout would put there.
    ///
    /// Exactly the three fields the path is built from — the version and the
    /// digest name the directory, and the entry names the file in it. Nothing
    /// else, deliberately: a field that decided "already installed" without
    /// deciding *where* would send an install to rename over a file this
    /// record still names, and the rollback after a failed settle would then
    /// take away a runtime it did not write.
    ///
    /// The entry is compared in full rather than by its last segment, because
    /// two entries of one archive can share a file name. The pin reader
    /// refuses a file that names one archive twice, so within a valid pin the
    /// digest already fixes the entry; comparing it costs nothing and does not
    /// depend on that holding.
    fn describes(&self, release: &PinnedRelease) -> bool {
        self.version == release.version().as_str()
            && self.digest == release.archive_digest().as_str()
            && self.executable == release.executable().as_str()
    }
}

/// The runtimes Nessa installed, under one directory it owns.
///
/// ```text
/// <root>/<agent>/installed.json                     what is installed, written last
/// <root>/<agent>/install.lock                       held while one install publishes
/// <root>/<agent>/versions/<version>/<digest>/<name> the executable itself
/// <root>/<agent>/.nessa-<hex>.download              a download in progress, unnamed on unix
/// <root>/<agent>/.nessa-<hex>.tmp                   a record or executable being written
/// ```
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

    /// Every directory this install creates, innermost first.
    ///
    /// Written out rather than walked with `parent()` so that the chain the
    /// store makes durable is the chain the store made, and stops at its own
    /// root rather than at whatever is above it.
    fn created_directories(&self, agent: &AgentName, release: &PinnedRelease) -> [PathBuf; 5] {
        [
            self.artifact_root(agent, release),
            self.version_root(agent, release.version()),
            self.versions_root(agent),
            self.agent_root(agent),
            self.root.clone(),
        ]
    }

    fn record_path(&self, agent: &AgentName) -> PathBuf {
        self.agent_root(agent).join("installed.json")
    }

    fn lock_path(&self, agent: &AgentName) -> PathBuf {
        self.agent_root(agent).join("install.lock")
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
        let directory = self.agent_root(agent);
        private_directory(&directory)?;
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
            let lock = open(&path, OpenMode::OpenOrCreate).map_err(|error| at(&path, error))?;
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
                lock.lock().map_err(|error| at(&path, error))?;
            }
            if same_file(&lock, &path)? {
                return Ok(lock);
            }
        }
        Err(StoreFailure::Unwritable(format!(
            "{}: kept being replaced while an install waited for it",
            path.display()
        )))
    }

    /// Note what is now installed, once it really is.
    ///
    /// Written to a private temporary file, forced to the disk, and renamed, so
    /// a reader never sees a half-written record and a crash cannot leave one
    /// that names an executable the disk does not have. The directory is synced
    /// afterwards so the rename itself survives, which also makes durable the
    /// version directory created a moment earlier.
    fn record(
        &self,
        agent: &AgentName,
        release: &PinnedRelease,
        durable: impl Fn(&Path) -> io::Result<()>,
    ) -> Result<(), StoreFailure> {
        let record = InstallationRecord::of(release);
        let directory = self.agent_root(agent);
        let mut staging = PrivateTempFile::new_in(&directory).map_err(unwritable)?;
        serde_json::to_writer_pretty(staging.as_file_mut(), &record)
            .map_err(|error| StoreFailure::Unwritable(error.to_string()))?;
        staging.as_file().sync_all().map_err(unwritable)?;
        staging
            .persist(&self.record_path(agent))
            .map_err(unwritable)?;
        // Through the same primitive as the walk above it, because this is the
        // sync that makes the record's own rename durable — the last thing
        // between an install and being acknowledged. Reaching for
        // `sync_directory` directly here would leave the final step of the
        // guarantee outside anything a test can watch.
        durable(&directory).map_err(unwritable)
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

    /// Take an executable back out after the install failed to complete.
    ///
    /// Best effort on purpose, and the reason it returns nothing: the caller
    /// already has a failure to report, and it is the one worth reporting. A
    /// second one about the clean-up would replace the cause with its
    /// consequence. What cannot be removed is logged, so that a directory
    /// holding an unrecorded runtime is at least explainable.
    fn withdraw(&self, agent: &AgentName, release: &PinnedRelease, executable: &Path) {
        if let Err(error) = fs::remove_file(executable) {
            if error.kind() != io::ErrorKind::NotFound {
                // `warn`, not `debug`: the shipped default keeps `info` and
                // above, and a line nobody sees would make the sentence above
                // untrue. This is the only trace that a runtime was left
                // somewhere nothing will look for it again.
                tracing::warn!(
                    path = %executable.display(),
                    %error,
                    "could not withdraw an agent runtime that was not recorded"
                );
            }
        }
        self.sweep_artifact(agent, release);
    }

    /// Remove the directories this install made and then had no use for.
    ///
    /// Both of them: the artifact directory holds this artifact alone, and the
    /// version directory holds nothing but artifact directories of that
    /// version, so an install that produced neither has left an empty pair
    /// behind. `versions/` and the agent's own directory are not swept — they
    /// are the store's furniture rather than this install's leavings, and the
    /// record and the lock live in the second one.
    fn sweep_artifact(&self, agent: &AgentName, release: &PinnedRelease) {
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
        let _ = fs::remove_dir(directory);
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
            if let Some(reason) = refused_kind(entry.header().entry_type()) {
                return Err(StoreFailure::MalformedArchive(format!(
                    "{} {reason}",
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
            // The rename is the last thing this does. Making the directory
            // durable belongs to the caller, because from here on a failure
            // has an executable to take back out — and a `?` inside this loop
            // would return past the only code that knows to do that.
            staging.persist(destination).map_err(unwritable)?;
            return Ok(true);
        }
        Ok(false)
    }
    /// Publish a runtime, forcing directories to the disk with `durable`.
    ///
    /// The whole of [`RuntimeStore::publish`], which supplies the real
    /// primitive. It is a parameter for the reason [`Self::settle`] gives —
    /// the walk it decides leaves no trace on a real filesystem — and keeping
    /// it threaded this far means the guarded region, rollback included, has
    /// somewhere to be observed from. A failure to settle is otherwise only
    /// reachable on a disk that has run out part-way through an install.
    fn publish_durably(
        &self,
        agent: &AgentName,
        release: &PinnedRelease,
        staged: &mut StagedArchive,
        durable: impl Fn(&Path) -> io::Result<()>,
    ) -> Result<PathBuf, StoreFailure> {
        // Held from here to the end of this method, the rollback included.
        // Everything below assumes that whatever is at `destination` when it
        // looks is either nothing or this call's own work, and that assumption
        // is true only while nobody else is publishing this agent.
        let _lock = self.hold(agent)?;
        // Asked again now that this call is the only one publishing. The
        // caller asked before downloading, and between that answer and this
        // line another install may have finished the very same artifact — in
        // which case it is already there, verified against the same digest,
        // and unpacking over it would mean replacing a file something may be
        // running with an identical one. Handing back what is installed also
        // keeps the rollback below honest: after this point, a file at
        // `destination` can only have been put there by this call.
        if let Some(installed) = self.installed(agent, release)? {
            return Ok(installed);
        }
        let directory = self.artifact_root(agent, release);
        private_directory(&directory)?;
        let destination = directory.join(release.executable().file_name());
        let unpacked = self.unpack(
            release,
            staged,
            &destination,
            &directory,
            MAXIMUM_EXECUTABLE_BYTES,
        );
        // Every way of not getting an executable ends the same: the version
        // directory was made a moment ago in the expectation of one, and
        // sweeping it is the whole of the clean-up, because nothing was
        // written. Deliberately not `withdraw`: this call put no file at that
        // path, and removing whatever is there would mean a pin that is wrong
        // about its own contents deleting a runtime somebody was using.
        match unpacked {
            Err(failure) => {
                self.sweep_artifact(agent, release);
                return Err(failure);
            }
            Ok(false) => {
                self.sweep_artifact(agent, release);
                return Err(StoreFailure::MissingExecutable(
                    release.executable().as_str().to_owned(),
                ));
            }
            Ok(true) => {}
        }
        // Everything from the rename onwards is guarded together, because from
        // that moment an executable exists and a failure would otherwise leave
        // it behind.
        //
        // The record is written last, since it is what makes the install true:
        // `installed` answers from it, so writing it before the executable
        // exists would claim an install that does not. That ordering leaves a
        // window, and it is not only the record that can fail in it — making
        // the directory durable can too. Returning either failure on its own
        // would tell somebody nothing was installed while a hundred megabytes
        // of runtime sat in a directory no record names, which nothing would
        // ever look at again — on a disk that, in the likeliest cause of this
        // failure, is the thing that ran out. So the executable is taken back
        // out, and the message is true when it is read.
        if let Err(failure) = self.settle(agent, release, durable) {
            // Unconditional, and safe to be: the lock has been held since
            // before the recheck, which found nothing installed, so the file
            // at `destination` is the one this call renamed there and no other
            // install can have finished in between. Taking it back out is
            // undoing this call's own work, not losing somebody else's.
            self.withdraw(agent, release, &destination);
            return Err(failure);
        }
        Ok(destination)
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
        // The path is recomputed from the release rather than read back out of
        // the record, so `installed.json` cannot name a launch path of its own
        // choosing: the most a rewritten record can do is make Nessa install
        // again. The record is still only a note, so an executable that has
        // since been deleted — by a disk cleaner, or by someone tidying up —
        // makes it stale, and an empty one is a runtime that cannot start.
        let executable = self
            .artifact_root(agent, release)
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
        self.publish_durably(agent, release, staged, sync_directory)
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
                    "{} could not be read out of the archive: {error}",
                    release.executable()
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

/// Why an entry of this kind cannot be the runtime, if it cannot.
///
/// A link entry names another file and carries no data; a directory carries
/// none either. Neither is an executable, and unpacking one writes an empty
/// file that this store would then record as the installed runtime and hand
/// out as something to launch.
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

/// Make `path` a directory only this user can reach, or say which one it was.
///
/// The refusal this can produce — a directory that already exists and is
/// readable by somebody else — names no path of its own, and a caller looking
/// at a message about "local storage" has three directories in play. Naming it
/// is the difference between an error somebody can act on and one they cannot.
fn private_directory(path: &Path) -> Result<(), StoreFailure> {
    create_directory(path).map_err(|error| at(path, error))
}

/// Whether the handle taken on `path` is still the file that `path` names.
///
/// The lock lives on an open file, not on a name, so a lock file replaced
/// between the open and the lock would leave two installs each holding a
/// different file and each believing it was the only one publishing. Compared
/// on device and inode, which is the pair that identifies a file rather than a
/// way of reaching one.
#[cfg(unix)]
fn same_file(lock: &File, path: &Path) -> Result<bool, StoreFailure> {
    use std::os::unix::fs::MetadataExt;

    let held = lock.metadata().map_err(unwritable)?;
    match fs::metadata(path) {
        Ok(named) => Ok(held.dev() == named.dev() && held.ino() == named.ino()),
        // Removed rather than replaced. The handle is still a lock nobody else
        // can take through it, but the next install would create a new file
        // and take a second one, so this one excludes nothing.
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(at(path, error)),
    }
}

/// Windows keeps the name for as long as the file is open, so the file that
/// was locked is the file at that path by construction.
#[cfg(not(unix))]
fn same_file(_lock: &File, _path: &Path) -> Result<bool, StoreFailure> {
    Ok(true)
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
