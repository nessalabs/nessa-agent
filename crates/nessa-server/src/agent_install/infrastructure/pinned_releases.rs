use std::collections::BTreeMap;
use std::fmt;

use serde::de::{self, Deserializer, MapAccess, Visitor};
use serde::Deserialize;

use crate::agent_install::domain::{
    AgentName, ArchiveDigest, ArchivePath, ArchiveUrl, PinRejected, PinnedRelease, ReleasePlatform,
    ReleaseVersion,
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
    #[serde(deserialize_with = "agents_pinned_once")]
    agents: BTreeMap<String, Vec<ReleaseDocument>>,
}

/// Read the agents map, refusing a name that appears twice.
///
/// A map decoder resolves a repeated key silently, and which of the two entries
/// wins is a property of the decoder rather than a decision anybody made. The
/// value being chosen here is the digest of a binary Nessa will execute, so a
/// file that says two things is refused rather than resolved.
fn agents_pinned_once<'de, D>(
    deserializer: D,
) -> Result<BTreeMap<String, Vec<ReleaseDocument>>, D::Error>
where
    D: Deserializer<'de>,
{
    struct Agents;

    impl<'de> Visitor<'de> for Agents {
        type Value = BTreeMap<String, Vec<ReleaseDocument>>;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a map of agent names to their pinned releases")
        }

        fn visit_map<A>(self, mut entries: A) -> Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            let mut agents = BTreeMap::new();
            while let Some((agent, releases)) = entries.next_entry::<String, Vec<_>>()? {
                if agents.insert(agent.clone(), releases).is_some() {
                    return Err(de::Error::custom(format!("{agent:?} is pinned twice")));
                }
            }
            Ok(agents)
        }
    }

    deserializer.deserialize_map(Agents)
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
    /// One agent has two releases for the same platform, so which one is
    /// installed would depend on the order they happen to be written in.
    PlatformPinnedTwice {
        agent: String,
        platform: ReleasePlatform,
    },
}

impl fmt::Display for PinFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(detail) => write!(f, "the pinned release file is malformed: {detail}"),
            Self::Invalid { agent, reason } => {
                write!(f, "the pinned release for {agent} is invalid: {reason}")
            }
            Self::PlatformPinnedTwice { agent, platform } => {
                write!(f, "{agent} is pinned twice for {platform}")
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
pub fn releases_for(agent: &AgentName) -> Result<Vec<PinnedRelease>, PinFileError> {
    releases_in(PINS, agent)
}

/// The same, over a document given rather than the compiled-in one.
///
/// Split out so that the ways a pin file can be wrong have somewhere to be
/// tested from. They cannot be reached through [`releases_for`] by any input
/// this build can be given — the document is compiled in and valid — and a
/// refusal with no test is a refusal nobody has read.
fn releases_in(document: &str, agent: &AgentName) -> Result<Vec<PinnedRelease>, PinFileError> {
    let document: PinDocument = serde_json::from_str(document)
        .map_err(|error| PinFileError::Malformed(error.to_string()))?;
    let Some(entries) = document.agents.get(agent.as_str()) else {
        return Ok(Vec::new());
    };
    let releases = entries
        .iter()
        .map(|entry| {
            release(entry).map_err(|reason| PinFileError::Invalid {
                agent: agent.to_string(),
                reason,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    for (index, release) in releases.iter().enumerate() {
        if releases[..index]
            .iter()
            .any(|earlier| earlier.runs_on(release.platform()))
        {
            return Err(PinFileError::PlatformPinnedTwice {
                agent: agent.to_string(),
                platform: release.platform().clone(),
            });
        }
    }
    Ok(releases)
}

/// Turn one documented release into a pin, applying the domain's rules.
///
/// Every field goes through its value object, so a typo in the file — an
/// uppercase digest, a version with a slash in it, a path that escapes the
/// archive, a plain-http URL — is a build that fails a test rather than a
/// download that installs something unexpected.
fn release(entry: &ReleaseDocument) -> Result<PinnedRelease, PinRejected> {
    Ok(PinnedRelease::new(
        ReleaseVersion::parse(&entry.version)?,
        ReleasePlatform::new(&entry.operating_system, &entry.architecture)?,
        ArchiveUrl::parse(&entry.archive_url)?,
        ArchiveDigest::parse(&entry.archive_digest)?,
        ArchivePath::parse(&entry.executable)?,
    ))
}

#[cfg(test)]
#[path = "../../../tests/agent_install/pinned_releases.rs"]
mod tests;
