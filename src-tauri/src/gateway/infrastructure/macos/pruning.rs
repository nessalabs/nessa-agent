//! Collecting the staged runtime versions that nothing can still be running.
//!
//! Every app version stages a complete private copy of the gateway runtime under
//! `gateway-runtimes/<label>/<fingerprint>/`. Without collection each update
//! leaves the previous copy behind for good: the clone shares its blocks with a
//! bundle that the next update replaces, so an abandoned version stops being
//! free exactly when it stops being useful.
//!
//! Deleting the wrong directory removes the binaries of a running gateway, so
//! the decision is separated from the effect and stated as a pure rule over
//! names. Only `removable` decides; `LabelDirectory` performs, and validates the
//! entry it is about to remove once more against the filesystem.
//!
//! ```text
//! register -> prune_runtimes -> removable          (pure: which names)
//!                            -> RuntimeVersions    (effect: read and remove)
//! ```
//! Arrows mean calls. Nothing here returns a failure to `register`: a gateway
//! that is up matters more than disk that was not reclaimed. Nothing that one
//! entry does stops the pass either — a refused removal, or an entry that
//! cannot even be named, is reported and stepped over.
use super::control::RetirementEvidence;
use std::{
    ffi::OsString,
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

/// What an interrupted staging attempt leaves behind.
const STAGING_PREFIX: &str = ".staging-";

/// The runtime versions this reconciliation proved something may still need.
///
/// Each field is a fingerprint that names a directory under the label. They are
/// kept apart rather than merged into one set because the reasons differ, and a
/// review of this rule has to be able to see each reason on its own.
pub(super) struct RetainedRuntimes {
    /// The version just staged and written into the launchd definition.
    registered: String,
    /// The version the loaded service reports over `/health`.
    running: String,
    /// The version named by a retirement request the running gateway has not
    /// answered yet.
    pending: Option<String>,
    /// The version named by a retirement fence that was recorded but never
    /// acknowledged as complete.
    fenced: Option<String>,
}

/// Names the runtime versions collection must keep, from the evidence a
/// reconciliation already read.
///
/// A request the running gateway has not answered names a version that is still
/// in use by definition. A fence is different: one that says the retirement
/// completed describes a process that cleaned up, said so durably, and was then
/// booted out, and its version is collectable — a fence honoured forever would
/// leave one stale generation behind for good. A fence whose cleanup or audit
/// failed says the opposite, and its version stays.
///
/// Both are read before any service is touched, which is the conservative
/// direction: they can only name a version whose retirement has since
/// progressed, never one whose use has since begun.
pub(super) fn retained_runtimes(
    registered: &str,
    running: &str,
    pending: Option<&RetirementEvidence>,
    fence: Option<&RetirementEvidence>,
) -> RetainedRuntimes {
    RetainedRuntimes::new(
        registered,
        running,
        pending.map(|evidence| evidence.fingerprint.as_str()),
        fence
            .filter(|evidence| !evidence.retired)
            .map(|evidence| evidence.fingerprint.as_str()),
    )
}

impl RetainedRuntimes {
    fn new(registered: &str, running: &str, pending: Option<&str>, fenced: Option<&str>) -> Self {
        Self {
            registered: registered.to_owned(),
            running: running.to_owned(),
            pending: pending.map(str::to_owned),
            fenced: fenced.map(str::to_owned),
        }
    }
    fn holds(&self, fingerprint: &str) -> bool {
        self.registered == fingerprint
            || self.running == fingerprint
            || self.pending.as_deref() == Some(fingerprint)
            || self.fenced.as_deref() == Some(fingerprint)
    }
}

/// The whole rule. One entry of a label's runtime directory, by name.
///
/// A published version is named by its runtime fingerprint, and is removable
/// only when no retained fingerprint names it. An interrupted staging attempt is
/// removable whenever we are here at all: the per-label reconciliation lock is
/// held, and holding it means no other host is staging into this directory.
/// Anything whose name we did not write — an unexpected file, a directory that
/// is not a fingerprint, a staging name that is not one of ours — is left alone.
fn removable(name: &str, retained: &RetainedRuntimes) -> bool {
    match name.strip_prefix(STAGING_PREFIX) {
        Some(attempt) => sha256(attempt),
        None => sha256(name) && !retained.holds(name),
    }
}

fn sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

/// One entry a pass did not remove, and why.
///
/// Each variant carries only what it can honestly say. A failed removal knows
/// the name it was working on and can be turned back into a path; an entry the
/// directory would not yield, or whose name is not text, cannot, and saying so
/// is the point of keeping them apart. The caller words each one; nothing here
/// pretends an entry was attempted when it was not.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Skipped {
    /// The label's directory could not be listed at all, so nothing was
    /// collected.
    Directory(String),
    /// One entry could not be read out of the directory. It has no name here,
    /// so the rule was never applied to it and no removal was attempted.
    Unreadable(String),
    /// The entry's name is not valid text, so it is not a name this host
    /// writes and the rule leaves it alone. The string is a lossy rendering
    /// for the log, never a path: it may name nothing at all.
    Unnamed(String),
    /// A removal was attempted for this name and failed.
    Removal { name: String, reason: String },
}

/// One reading of a label's runtime directory.
///
/// An entry that cannot be read, or whose name is not text, is still an entry:
/// it is not a name this host writes, so the rule would leave it alone anyway,
/// and it must not take the recognised versions beside it down with it. Such
/// entries are kept apart from the names rather than dropped, so that what was
/// passed over is still said.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Listing {
    /// Every entry that has a name, in a stable order.
    pub names: Vec<String>,
    /// Entries no name could be taken from, in a stable order.
    pub passed_over: Vec<Skipped>,
}

/// Splits what a directory yielded into names and entries that have none.
///
/// Separate from the reading so both failing cases can be tested at all: APFS
/// accepts only valid UTF-8, so no test on this machine can create an entry
/// with an invalid name, and an `EIO` part-way through a directory is not
/// something a test can ask a local filesystem for either. `staging.rs` keeps
/// its own representation test for the same reason.
fn listing(entries: Vec<Result<OsString, String>>) -> Listing {
    let mut listing = Listing::default();
    for entry in entries {
        match entry {
            Ok(entry) => match entry.into_string() {
                Ok(name) => listing.names.push(name),
                Err(raw) => listing
                    .passed_over
                    .push(Skipped::Unnamed(raw.to_string_lossy().into_owned())),
            },
            Err(error) => listing.passed_over.push(Skipped::Unreadable(error)),
        }
    }
    // Directory order is whatever the filesystem says; a stable order keeps
    // what this reports, and what its tests observe, the same every time.
    listing.names.sort();
    listing.passed_over.sort();
    listing
}

/// A label's runtime directory, as collection needs it.
pub(super) trait RuntimeVersions {
    /// Everything the directory holds, published or not.
    fn list(&self) -> Result<Listing, String>;
    /// Removes one entry and everything below it.
    fn remove(&self, name: &str) -> Result<(), String>;
}

/// What one collection did, so the caller can say it out loud.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Collected {
    pub removed: Vec<String>,
    /// Everything this pass did not remove, each saying which kind of thing
    /// happened to it.
    pub skipped: Vec<Skipped>,
}

/// Removes every entry the rule allows, and keeps going past the ones it cannot.
///
/// One failed removal says nothing about the next: a version whose permissions
/// were changed by hand does not make the version beside it unremovable. The
/// same holds one level up — an entry the directory would not yield, or that
/// cannot be named, is reported and stepped over, never a reason to collect
/// nothing. Only a directory that cannot be listed at all ends the pass, because
/// then there is nothing to decide about.
pub(super) fn collect(versions: &impl RuntimeVersions, retained: &RetainedRuntimes) -> Collected {
    let mut collected = Collected::default();
    let listing = match versions.list() {
        Ok(listing) => listing,
        Err(error) => {
            collected.skipped.push(Skipped::Directory(error));
            return collected;
        }
    };
    for name in listing.names {
        if !removable(&name, retained) {
            continue;
        }
        match versions.remove(&name) {
            Ok(()) => collected.removed.push(name),
            Err(reason) => collected.skipped.push(Skipped::Removal { name, reason }),
        }
    }
    collected.skipped.extend(listing.passed_over);
    collected
}

/// The real directory. Every removal is revalidated here against the filesystem.
pub(super) struct LabelDirectory(PathBuf);

impl LabelDirectory {
    pub(super) fn at(installations: &Path) -> Self {
        Self(installations.to_path_buf())
    }
}

impl RuntimeVersions for LabelDirectory {
    fn list(&self) -> Result<Listing, String> {
        // Opening the directory is the whole pass; one entry it then refuses to
        // yield is one entry, and taking the versions beside it down with it
        // would be the same defect as failing on a name that is not text.
        let entries = fs::read_dir(&self.0)
            .map_err(|error| error.to_string())?
            .map(|entry| {
                entry
                    .map(|entry| entry.file_name())
                    .map_err(|error| error.to_string())
            })
            .collect();
        Ok(listing(entries))
    }
    fn remove(&self, name: &str) -> Result<(), String> {
        let path = self.0.join(name);
        let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err("Staged runtime entry is not a directory".into());
        }
        if metadata.uid() != unsafe { libc::geteuid() } {
            return Err("Staged runtime entry has another owner".into());
        }
        fs::remove_dir_all(&path).map_err(|error| error.to_string())
    }
}

/// Collects one label's directory and reports what happened.
///
/// Registration's result is deliberately unreachable from here. Disk that was
/// not reclaimed is said on the app's log and nothing else.
///
/// Each line says what actually happened to the thing it names. A path appears
/// only where one was really worked on: an entry with no usable name is
/// reported against its directory, and its lossy rendering is given as what it
/// looked like, never as somewhere to go and look.
pub(super) fn prune_runtimes(installations: &Path, retained: &RetainedRuntimes) {
    let collected = collect(&LabelDirectory::at(installations), retained);
    let directory = installations.display();
    for name in &collected.removed {
        eprintln!(
            "[nessa] Removed staged gateway runtime {}",
            installations.join(name).display()
        );
    }
    for skipped in &collected.skipped {
        match skipped {
            Skipped::Directory(reason) => eprintln!(
                "[nessa] Could not list staged gateway runtimes in {directory}; none were removed: {reason}"
            ),
            Skipped::Unreadable(reason) => eprintln!(
                "[nessa] An entry of {directory} could not be read and was left in place; no removal was attempted: {reason}"
            ),
            Skipped::Unnamed(lossy) => eprintln!(
                "[nessa] An entry of {directory} has no valid text name and was left in place; no removal was attempted. Its name reads approximately as {lossy:?}, which is not a path"
            ),
            Skipped::Removal { name, reason } => eprintln!(
                "[nessa] Could not remove staged gateway runtime {}: {reason}",
                installations.join(name).display()
            ),
        }
    }
}
#[cfg(test)]
#[path = "../../../../tests/gateway/infrastructure/pruning.rs"]
mod tests;
