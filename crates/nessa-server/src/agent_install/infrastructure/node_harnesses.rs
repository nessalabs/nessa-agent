//! Agents that arrive as a dependency tree rather than a single executable.
//!
//! Opencode ships one binary, so [`super::ManagedRuntimes`] fetches an archive,
//! measures it against a digest this build compiled in, and unpacks one file.
//! Claude and Codex are npm packages with dependencies — 244 MB and 295 MB of
//! them — and none of that fits an archive with one executable in it.
//!
//! What they have instead is a lockfile, and it is the better instrument: it
//! names an integrity hash for every package in the tree, so `npm ci` checks
//! each one as it installs. The bytes are verified either way; the difference
//! is that the manifest describes a tree and a pin describes a file.
//!
//! These trees used to be built into the application, by running this same
//! `npm ci` at release time. Everyone downloaded both agents, including the one
//! they would never open. Now the two small manifests ship and the install
//! happens on the machine, once, at first use.
//!
//! ```text
//! bundle/<agent>/package.json + package-lock.json   the manifests, shipped
//!        │
//!        └─ npm ci ─▶ ~/.nessa/agents/<agent>/<version>/node_modules/…
//!                          │
//!                          └─ the entry script the adapter is launched with
//! ```
//!
//! The version directory is what makes an upgrade safe: a new pin installs
//! beside the old one rather than over it, so a runtime that is running when an
//! upgrade lands keeps the files it was started from.
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::agent_install::domain::{AgentName, ReleaseVersion};

/// Why a harness could not be made ready.
#[derive(Debug, PartialEq, Eq)]
pub enum HarnessFailure {
    /// The shipped manifests for this agent are not where they should be.
    ///
    /// A build fault rather than a machine's state: these are two files the
    /// application carries, and an application missing them cannot install the
    /// agent anywhere.
    Manifests(String),
    /// `npm` could not be run, or refused.
    Install(String),
    /// The install reported success and the entry script is still not there.
    ///
    /// Kept apart from the failure above because it means something different:
    /// the package resolved and installed, and what it contains is not what
    /// this build expects to launch.
    Missing(String),
}

impl std::fmt::Display for HarnessFailure {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Manifests(detail) => write!(out, "the agent's manifests are missing: {detail}"),
            Self::Install(detail) => {
                write!(out, "the agent's runtime could not be installed: {detail}")
            }
            Self::Missing(path) => {
                write!(out, "the installed runtime has no entry script at {path}")
            }
        }
    }
}

/// Where a harness's trees live, and where its manifests were shipped.
pub struct NodeHarnesses {
    root: PathBuf,
    manifests: PathBuf,
}

impl NodeHarnesses {
    /// Install under `root`, from the manifests the application shipped.
    ///
    /// Both are given rather than derived: composition owns where data lives
    /// and where the bundle is, and a store that resolved either for itself
    /// would be a second opinion about paths the rest of this context takes
    /// from one place.
    pub fn new(root: impl Into<PathBuf>, manifests: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            manifests: manifests.into(),
        }
    }

    /// The directory a given version of `agent` installs into.
    fn version_root(&self, agent: &AgentName, version: &ReleaseVersion) -> PathBuf {
        self.root.join(agent.as_str()).join(version.as_str())
    }

    /// Where this agent's entry script sits inside an installed tree.
    fn entry(&self, agent: &AgentName, version: &ReleaseVersion, relative: &str) -> PathBuf {
        self.version_root(agent, version).join(relative)
    }

    /// The entry script for `version` of `agent`, if that version is installed.
    ///
    /// Answered from the file rather than from a record: an install that was
    /// interrupted leaves a directory behind, and a directory is not a runtime.
    /// Nothing here is cached, because the question is asked once per start and
    /// the answer can change between two of them.
    pub fn installed(
        &self,
        agent: &AgentName,
        version: &ReleaseVersion,
        relative: &str,
    ) -> Option<PathBuf> {
        let entry = self.entry(agent, version, relative);
        entry.is_file().then_some(entry)
    }

    /// Install `version` of `agent`, and answer where its entry script is.
    ///
    /// Idempotent: an install that is already there is not repeated, which is
    /// what makes this safe to call on a path that runs at every start.
    pub fn install(
        &self,
        agent: &AgentName,
        version: &ReleaseVersion,
        relative: &str,
    ) -> Result<PathBuf, HarnessFailure> {
        if let Some(entry) = self.installed(agent, version, relative) {
            return Ok(entry);
        }
        let target = self.version_root(agent, version);
        let source = self.manifests.join(agent.as_str());
        for file in ["package.json", "package-lock.json"] {
            let from = source.join(file);
            if !from.is_file() {
                return Err(HarnessFailure::Manifests(from.display().to_string()));
            }
        }
        nessa_local_storage::create_directory_beneath(
            &self.root,
            Path::new(&format!("{}/{}", agent.as_str(), version.as_str())),
        )
        .map_err(|error| HarnessFailure::Install(error.to_string()))?;
        for file in ["package.json", "package-lock.json"] {
            std::fs::copy(source.join(file), target.join(file))
                .map_err(|error| HarnessFailure::Install(error.to_string()))?;
        }
        // `ci`, not `install`: it installs exactly what the lockfile names and
        // refuses if the manifest and the lockfile disagree, which is the whole
        // reason the bytes can be trusted without a digest of our own.
        let run = Command::new("npm")
            .args(["ci", "--omit=dev", "--no-audit", "--no-fund"])
            .current_dir(&target)
            .output()
            .map_err(|error| HarnessFailure::Install(error.to_string()))?;
        if !run.status.success() {
            return Err(HarnessFailure::Install(
                String::from_utf8_lossy(&run.stderr).trim().to_string(),
            ));
        }
        self.installed(agent, version, relative).ok_or_else(|| {
            HarnessFailure::Missing(self.entry(agent, version, relative).display().to_string())
        })
    }
}

#[cfg(test)]
#[path = "../../../tests/agent_install/node_harnesses.rs"]
mod tests;
