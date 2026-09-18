use std::collections::HashMap;

use serde::Deserialize;

use crate::agent_install::domain::{
    ArchiveDigest, ArchivePath, PinRejected, PinnedRelease, ReleasePlatform, ReleaseVersion,
};

/// The releases Nessa has tested, compiled into the server.
///
/// Embedded rather than read from disk because a pin is part of the build: a
/// server that could be pointed at a different pin file would be a server whose
/// "the version we tested" claim depends on what is next to it on the machine.
/// Regenerate it with `node scripts/agents/pin-opencode.mjs`.
const PINS: &str = include_str!("../../../data/agent-releases.json");

/// One release as written in the pin file.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseDocument {
    operating_system: String,
    architecture: String,
    version: String,
    archive_url: String,
    archive_digest: String,
    executable: String,
}

#[derive(Debug, Deserialize)]
struct PinDocument {
    agents: HashMap<String, Vec<ReleaseDocument>>,
}

/// Why the compiled-in pin file is not usable.
///
/// Every one of these is a build-time mistake — a malformed file, or a pin that
/// breaks one of the domain's rules — so they are reported with enough detail
/// to fix the file, and there is no runtime path that recovers from them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinFileError {
    /// The document is not the shape this build reads.
    Malformed(String),
    /// A release in the document is not a valid pin.
    Invalid { agent: String, reason: PinRejected },
}

impl std::fmt::Display for PinFileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed(detail) => write!(f, "the pinned release file is malformed: {detail}"),
            Self::Invalid { agent, reason } => {
                write!(f, "the pinned release for {agent} is invalid: {reason}")
            }
        }
    }
}

impl std::error::Error for PinFileError {}

/// The platform this build of Nessa is for.
///
/// Read from the compiler's own target rather than from the running system,
/// because the question is which binary this process can launch: a build for
/// one architecture running under emulation on another must install the runtime
/// that matches the build, not the silicon.
pub fn host_platform() -> ReleasePlatform {
    ReleasePlatform::new(std::env::consts::OS, std::env::consts::ARCH)
        .expect("the compiler's own target tokens are plain lowercase identifiers")
}

/// Every tested release for `agent`, in the order the pin file lists them.
///
/// Parsed on each call rather than cached: this is read once when somebody asks
/// to install an agent, and a lazily initialised global would be a process-wide
/// handle for no gain.
pub fn releases_for(agent: &str) -> Result<Vec<PinnedRelease>, PinFileError> {
    let document: PinDocument =
        serde_json::from_str(PINS).map_err(|error| PinFileError::Malformed(error.to_string()))?;
    let Some(entries) = document.agents.get(agent) else {
        return Ok(Vec::new());
    };
    entries
        .iter()
        .map(|entry| {
            release(entry).map_err(|reason| PinFileError::Invalid {
                agent: agent.to_owned(),
                reason,
            })
        })
        .collect()
}

/// The tested release for `agent` on `platform`, when there is one.
pub fn release_for(
    agent: &str,
    platform: &ReleasePlatform,
) -> Result<Option<PinnedRelease>, PinFileError> {
    Ok(releases_for(agent)?
        .into_iter()
        .find(|release| release.runs_on(platform)))
}

/// Turn one documented release into a pin, applying the domain's rules.
///
/// Every field goes through its value object, so a typo in the file — an
/// uppercase digest, a version with a slash in it, a path that escapes the
/// archive, a plain-http URL — is a build that fails a test rather than a
/// download that installs something unexpected.
fn release(entry: &ReleaseDocument) -> Result<PinnedRelease, PinRejected> {
    PinnedRelease::new(
        ReleaseVersion::parse(&entry.version)?,
        ReleasePlatform::new(&entry.operating_system, &entry.architecture)?,
        &entry.archive_url,
        ArchiveDigest::parse(&entry.archive_digest)?,
        ArchivePath::parse(&entry.executable)?,
    )
}

#[cfg(test)]
#[path = "../../../tests/agent_install/pinned_releases.rs"]
mod tests;
