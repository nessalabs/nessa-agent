use std::collections::BTreeMap;
use std::fmt;

use serde::de::{self, Deserializer, MapAccess, Visitor};
use serde::Deserialize;

use crate::agent_install::domain::{
    AgentName, ArchiveDigest, ArchivePath, ArchiveUrl, HostPlatform, Libc, PinRejected,
    PinnedRelease, ReleasePlatform, ReleaseRequirements, ReleaseVersion,
};

/// The releases Nessa has tested, compiled into the server.
///
/// Embedded rather than read from disk because a pin is part of the build: a
/// server that could be pointed at a different pin file would be a server whose
/// "the version we tested" claim depends on what is next to it on the machine.
/// Regenerate it with `node scripts/agents/pin-opencode.mjs`.
const PINS: &str = include_str!("../../../data/agent-releases.json");

/// One release as written in the pin file.
///
/// Every field is required, and a field this build does not know is refused.
/// Both of those matter here in one direction: what a build needs of a machine
/// is stated by *absence* as much as by presence — no `libc` means it runs
/// against either, no `requiresAvx2` means it runs on any processor — so a key
/// that is dropped in a merge, or misspelled by whoever next edits the
/// generator, would quietly turn a build that runs on some machines into one
/// offered to all of them. That mistake ends in an illegal instruction or a
/// loader error on somebody's machine; a missing key ends in a failing test on
/// the machine of whoever wrote it. The file is generated and the generator
/// writes every field, so there is nothing legitimate to tolerate.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReleaseDocument {
    operating_system: String,
    architecture: String,
    /// `null` where the platform has only one, which is every platform but
    /// Linux. `null` on Linux would mean a build that runs against either,
    /// which none of the ones Nessa pins are — and which the reader refuses,
    /// because a Linux build that names no library is indistinguishable from
    /// one whose library was left out.
    #[serde(deserialize_with = "written_out")]
    libc: Option<String>,
    requires_avx2: bool,
    version: String,
    archive_url: String,
    archive_digest: String,
    executable: String,
}

/// Read an optional field that still has to be written out.
///
/// Serde fills a missing `Option` field with `None` and says nothing, which is
/// the one behaviour this file cannot afford: `null` here means "this build
/// runs against any C library", so a key dropped in a merge would read as a
/// deliberate statement that it does. Deserializing the `Option` explicitly
/// makes the key required while still accepting `null` as the answer.
fn written_out<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(deserializer)
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
    /// One agent has two releases a single machine could not tell apart, so
    /// which one is installed would depend on the order they happen to be
    /// written in.
    PlatformPinnedTwice {
        agent: String,
        platform: ReleasePlatform,
        requirements: ReleaseRequirements,
    },
    /// One agent has two releases naming the same archive. One archive is one
    /// build, so the two disagree about something that cannot differ — and the
    /// store keys an installation by that digest, so it could not keep them
    /// apart if it tried.
    ArchivePinnedTwice { agent: String, digest: String },
    /// A Linux release says nothing about which C library it needs. Every
    /// Linux build has one, so this is a field that was left out rather than a
    /// build that runs against either — and read as written it would be
    /// offered to machines it cannot start on.
    LinuxWithoutLibc { agent: String, version: String },
}

impl fmt::Display for PinFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(detail) => write!(f, "the pinned release file is malformed: {detail}"),
            Self::Invalid { agent, reason } => {
                write!(f, "the pinned release for {agent} is invalid: {reason}")
            }
            Self::PlatformPinnedTwice {
                agent,
                platform,
                requirements,
            } => {
                write!(f, "{agent} is pinned twice for {platform} ({requirements})")
            }
            Self::ArchivePinnedTwice { agent, digest } => {
                write!(f, "{agent} pins the archive {digest} twice")
            }
            Self::LinuxWithoutLibc { agent, version } => {
                write!(
                    f,
                    "the linux release of {agent} {version} does not say which c library it needs"
                )
            }
        }
    }
}

impl std::error::Error for PinFileError {}

/// This machine, in the terms a release states its needs in.
///
/// The whole boundary between "what is true of the computer this is running on"
/// and everything that decides what to install. Nothing below this reads the
/// machine, so every case a release can be chosen for — musl, a processor
/// without AVX2, a platform nothing is pinned for — is reachable in a test by
/// building a [`HostPlatform`] rather than by owning that machine.
///
/// The operating system and architecture come from the compiler's own target
/// rather than from the running system, because the question is which binary
/// this process can launch: a build for one architecture running under
/// emulation on another must install the runtime that matches the build, not
/// the silicon. The other two answers are read from this process and from the
/// processor, for the reasons on each.
pub fn host_platform() -> HostPlatform {
    HostPlatform::new(
        ReleasePlatform::new(std::env::consts::OS, std::env::consts::ARCH)
            .expect("the compiler's own target tokens are plain lowercase identifiers"),
        host_libc(),
        host_has_avx2(),
    )
}

/// Which C library this machine is known to have.
///
/// Taken from what *this* binary was linked against, which is the one fact
/// available that cannot be wrong: a glibc build could not have reached this
/// line on a machine without glibc, and neither could a musl build without
/// musl. Looking for a loader on disk would answer a different question —
/// which libraries are installed — and would have to guess between them on a
/// machine carrying both.
///
/// `None` on a target built against neither, which is not a machine Nessa
/// ships for. Every Linux release names a library, so such a machine is
/// offered nothing and told so, rather than installed on a guess.
///
/// `Gnu` on a `*-windows-gnu` target means MinGW rather than glibc, which is a
/// different fact wearing the same name. Nothing reaches that today, because
/// no Windows release is pinned; whoever pins one has to decide what the word
/// means there before this answer can be trusted.
fn host_libc() -> Option<Libc> {
    if cfg!(target_env = "musl") {
        Some(Libc::Musl)
    } else if cfg!(target_env = "gnu") {
        Some(Libc::Gnu)
    } else {
        None
    }
}

/// Whether this processor supports AVX2.
///
/// Asked of the processor at runtime rather than of the compiler's target,
/// because this is the one question where the silicon is the subject: a build
/// targeting x86-64 says nothing about which optional instruction sets the
/// machine running it implements.
fn host_has_avx2() -> bool {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        std::arch::is_x86_feature_detected!("avx2")
    }
    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
    {
        false
    }
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
    // Compared on the platform *and* what the build needs, because those two
    // together are what a machine is matched against. Two Linux x86-64 archives
    // are an ordinary pin when one wants musl and the other glibc, and a
    // mistake when they want the same thing.
    for (index, release) in releases.iter().enumerate() {
        if releases[..index].iter().any(|earlier| {
            earlier.platform() == release.platform()
                && earlier.requirements() == release.requirements()
        }) {
            return Err(PinFileError::PlatformPinnedTwice {
                agent: agent.to_string(),
                platform: release.platform().clone(),
                requirements: *release.requirements(),
            });
        }
        // The digest is the identity of a build, and the store keys an
        // installation by it: two releases naming one archive would be two
        // descriptions of the same bytes, with one directory between them and
        // nothing to say which description the file there belongs to.
        if releases[..index]
            .iter()
            .any(|earlier| earlier.archive_digest() == release.archive_digest())
        {
            return Err(PinFileError::ArchivePinnedTwice {
                agent: agent.to_string(),
                digest: release.archive_digest().as_str().to_owned(),
            });
        }
        // A Linux build that names no library is not a build that runs against
        // either; it is a field somebody left out. Read as written it would be
        // offered to every Linux machine, half of which cannot start it.
        if release.platform().operating_system() == "linux"
            && release.requirements().libc().is_none()
        {
            return Err(PinFileError::LinuxWithoutLibc {
                agent: agent.to_string(),
                version: release.version().as_str().to_owned(),
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
    let libc = entry.libc.as_deref().map(Libc::parse).transpose()?;

    Ok(PinnedRelease::new(
        ReleaseVersion::parse(&entry.version)?,
        ReleasePlatform::new(&entry.operating_system, &entry.architecture)?,
        ReleaseRequirements::new(libc, entry.requires_avx2),
        ArchiveUrl::parse(&entry.archive_url)?,
        ArchiveDigest::parse(&entry.archive_digest)?,
        ArchivePath::parse(&entry.executable)?,
    ))
}

#[cfg(test)]
#[path = "../../../tests/agent_install/pinned_releases.rs"]
mod tests;
