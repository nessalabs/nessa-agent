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
//! that is up matters more than disk that was not reclaimed.
use super::control::RetirementEvidence;
use std::{
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

/// A label's runtime directory, as collection needs it.
pub(super) trait RuntimeVersions {
    /// The names of every entry, published or not.
    fn names(&self) -> Result<Vec<String>, String>;
    /// Removes one entry and everything below it.
    fn remove(&self, name: &str) -> Result<(), String>;
}

/// What one collection did, so the caller can say it out loud.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Collected {
    pub removed: Vec<String>,
    /// Entry name and why it could not be removed, with an empty name when the
    /// directory itself could not be read.
    pub failures: Vec<(String, String)>,
}

/// Removes every entry the rule allows, and keeps going past the ones it cannot.
///
/// One failed removal says nothing about the next: a version whose permissions
/// were changed by hand does not make the version beside it unremovable.
pub(super) fn collect(versions: &impl RuntimeVersions, retained: &RetainedRuntimes) -> Collected {
    let mut collected = Collected::default();
    let names = match versions.names() {
        Ok(names) => names,
        Err(error) => {
            collected.failures.push((String::new(), error));
            return collected;
        }
    };
    for name in names {
        if !removable(&name, retained) {
            continue;
        }
        match versions.remove(&name) {
            Ok(()) => collected.removed.push(name),
            Err(error) => collected.failures.push((name, error)),
        }
    }
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
    fn names(&self) -> Result<Vec<String>, String> {
        let mut names = Vec::new();
        for entry in fs::read_dir(&self.0).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            names.push(
                entry
                    .file_name()
                    .into_string()
                    .map_err(|_| "Runtime names must be valid UTF-8".to_owned())?,
            );
        }
        // Directory order is whatever the filesystem says; a stable order keeps
        // what this reports, and what its tests observe, the same every time.
        names.sort();
        Ok(names)
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
/// not reclaimed is said on the app's log, with the path, and nothing else.
pub(super) fn prune_runtimes(installations: &Path, retained: &RetainedRuntimes) {
    let collected = collect(&LabelDirectory::at(installations), retained);
    for name in &collected.removed {
        eprintln!(
            "[nessa] Removed staged gateway runtime {}",
            installations.join(name).display()
        );
    }
    for (name, error) in &collected.failures {
        eprintln!(
            "[nessa] Could not remove staged gateway runtime {}: {error}",
            installations.join(name).display()
        );
    }
}
#[cfg(test)]
#[path = "../../../../tests/gateway/infrastructure/pruning.rs"]
mod tests;
