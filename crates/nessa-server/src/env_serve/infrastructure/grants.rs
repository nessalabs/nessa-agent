//! What this host's owner grants each gateway, on disk:
//! `<data>/environment/grants.json`, a private file written by
//! `nessa env grant` and `nessa env ungrant` and read by serving at every
//! lease.
//!
//! ```text
//! {"schemaVersion": 1,
//!  "peers": {"ssh": {"agents": ["claude"], "commands": {"maxTimeoutMs": 60000}},
//!            "<the gateway's pin, base64>": {"agents": "*"}}}
//! ```
//!
//! An entry's `agents` is a list of agent names or `"*"` for every agent,
//! and none when missing; its `commands` is the longest a command may run,
//! and no commands when missing. A peer with no entry has the defaults
//! `domain::granted_to` gives it.
//!
//! These grants only narrow what the host can run: `config.json`'s
//! `envServe.commands` stays the host's own switch for commands at all
//! (off, every command lease is refused `commands_unavailable`), and its
//! agents what it can run at all; a grant refuses what it leaves out
//! `not_granted`, and grants nothing the host does not run.
//!
//! ```text
//! FileGrants::entry ──▶ open (private, bounded) ──▶ parse ──▶ the peer's entry
//! FileGrants::change_settled ──▶ grants.lock (held throughout) ──▶ read
//!   ──▶ change ──▶ encoded (past the bound: refused, nothing written)
//!   ──▶ a private temporary file ──▶ synced ──▶ intend (the intended
//!   record kept, or the change refused) ──▶ renamed over grants.json
//!   ──▶ directory synced ──▶ settle (written, durable or not, or not
//!   written) ──▶ grants.lock released
//! ```
//!
//! Arrows are steps, in order. A reader never takes the lock: the rename
//! means it reads the whole file before a change or after it. A file this
//! build cannot read (another schema version, larger than the bound, not
//! private, not this shape, a peer named twice) is an error, never taken as
//! no grants. An owner's change is audited twice, holding the lock
//! (`GrantStore`, `grant_audit.rs`): its intent between the sync and the
//! rename, so a record that cannot be kept leaves grants.json as it was,
//! and its outcome after the rename.
use crate::env_serve::{
    application::{Applied, ChangeFailed, GrantStore, PeerGrants},
    domain::{AgentName, Agents, CommandGrant, Peer, PeerGrant},
};
use base64::{engine::general_purpose::STANDARD, Engine};
use nessa_local_storage::{is_unsafe_file, open, sync_directory, OpenMode, PrivateTempFile};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fmt,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

/// The grants' file, beneath `<data>/environment`.
const FILE: &str = "grants.json";
/// What a change holds while it reads and writes the file.
const LOCK: &str = "grants.lock";
/// The only shape this build writes or reads.
const SCHEMA_VERSION: u32 = 1;
/// Most bytes the file may have: room for many peers, and a bound on what
/// every lease reads.
pub(crate) const MAX_BYTES: u64 = 64 * 1024;

/// How [`peer_name`] spells [`Peer::Ssh`].
pub(crate) const SSH: &str = "ssh";

/// A peer's one spelling, in the grants file and on the command line: `ssh`,
/// or the gateway's pin in standard base64, as the peer records spell it.
pub(crate) fn peer_name(peer: &Peer) -> String {
    match peer {
        Peer::Ssh => SSH.into(),
        Peer::Gateway { pin } => STANDARD.encode(pin),
    }
}

/// A peer in the audit and on disk is its one spelling, [`peer_name`]: the
/// domain's peer holds no text form of its own.
impl Serialize for Peer {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&peer_name(self))
    }
}

/// A peer name that is neither `ssh` nor a gateway's pin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct InvalidPeer;

/// Read back a name [`peer_name`] wrote.
///
/// # Errors
/// [`InvalidPeer`] for anything else.
pub(crate) fn parse_peer(name: &str) -> Result<Peer, InvalidPeer> {
    if name == SSH {
        return Ok(Peer::Ssh);
    }
    let bytes = STANDARD.decode(name).map_err(|_| InvalidPeer)?;
    let pin = <[u8; 44]>::try_from(bytes).map_err(|_| InvalidPeer)?;
    // Base64 has more than one spelling of some bytes: only this one is a
    // name, so one peer is never two entries.
    let peer = Peer::Gateway { pin };
    (peer_name(&peer) == name)
        .then_some(peer)
        .ok_or(InvalidPeer)
}

/// Why the grants could not be read or changed.
#[derive(Debug)]
pub(crate) enum GrantsError {
    /// The file or its directory could not be used.
    Unavailable(io::Error),
    /// The file is not one this build reads: the reason says why.
    Unreadable(String),
    /// A change would make the file larger than every reader takes: it is
    /// not made.
    TooLarge,
}

impl fmt::Display for GrantsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(error) => write!(formatter, "{FILE} could not be used: {error}"),
            Self::Unreadable(reason) => write!(formatter, "{FILE} cannot be read: {reason}"),
            Self::TooLarge => write!(
                formatter,
                "{FILE} would be larger than {MAX_BYTES} bytes; ungrant another peer first"
            ),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Stored {
    schema_version: u32,
    #[serde(default, deserialize_with = "unique_peers")]
    peers: BTreeMap<String, StoredGrant>,
}

/// The file's peers, each named once: a peer named twice is not this
/// shape, so the file is unreadable rather than read as the last of them.
fn unique_peers<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<String, StoredGrant>, D::Error> {
    struct Unique;
    impl<'de> serde::de::Visitor<'de> for Unique {
        type Value = BTreeMap<String, StoredGrant>;
        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a map of peers, each named once")
        }
        fn visit_map<A: serde::de::MapAccess<'de>>(
            self,
            mut map: A,
        ) -> Result<Self::Value, A::Error> {
            let mut peers = BTreeMap::new();
            while let Some((name, grant)) = map.next_entry::<String, StoredGrant>()? {
                if peers.contains_key(&name) {
                    return Err(serde::de::Error::custom(format!(
                        "{name} is named more than once"
                    )));
                }
                peers.insert(name, grant);
            }
            Ok(peers)
        }
    }
    deserializer.deserialize_map(Unique)
}

/// Only the version, so another version is named as such whatever else
/// its file holds.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Version {
    schema_version: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredGrant {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agents: Option<StoredAgents>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    commands: Option<StoredCommands>,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum StoredAgents {
    /// `"*"`.
    All(String),
    Named(Vec<String>),
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredCommands {
    max_timeout_ms: u64,
}

/// The grants file of one data directory.
pub(crate) struct FileGrants {
    directory: PathBuf,
    replace: Box<dyn Replace>,
}

impl FileGrants {
    /// The grants kept in `environment`, `<data>/environment`.
    pub(crate) fn new(environment: &Path) -> Self {
        Self {
            directory: environment.to_path_buf(),
            replace: Box::new(OnDisk),
        }
    }

    /// The grants kept in `environment`, replaced through `replace`.
    #[cfg(test)]
    pub(crate) fn replacing(environment: &Path, replace: Box<dyn Replace>) -> Self {
        Self {
            directory: environment.to_path_buf(),
            replace,
        }
    }

    /// Every peer's entry; none when there is no file.
    ///
    /// # Errors
    /// [`GrantsError`]: the file could not be read, or is not one this
    /// build reads.
    pub(crate) fn read(&self) -> Result<BTreeMap<Peer, PeerGrant>, GrantsError> {
        let file = match open(&self.directory.join(FILE), OpenMode::Read) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
            Err(error) if is_unsafe_file(&error) => {
                return Err(GrantsError::Unreadable(
                    "it is not a private file of this account".into(),
                ))
            }
            Err(error) => return Err(GrantsError::Unavailable(error)),
        };
        let mut bytes = Vec::new();
        file.take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(GrantsError::Unavailable)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(GrantsError::Unreadable(format!(
                "it is larger than {MAX_BYTES} bytes"
            )));
        }
        decode(&bytes)
    }

    /// [`Self::change_settled`] with nothing to audit: the tests' way to
    /// set grants. The owner's changes are audited ([`GrantStore`]).
    ///
    /// # Errors
    /// As [`Self::change_settled`]; a replacement that failed, or was not
    /// synced, is [`Changed::Grants`].
    #[cfg(test)]
    pub(crate) fn change<E>(
        &self,
        peer: &Peer,
        change: impl FnOnce(Option<PeerGrant>) -> Result<Option<PeerGrant>, E>,
    ) -> Result<(), Changed<E>> {
        self.change_settled(
            peer,
            change,
            || Ok(()),
            |applied| match applied {
                Applied::Written { synced: Ok(()) } => Ok(()),
                Applied::Written { synced: Err(error) } | Applied::NotWritten(error) => Err(
                    Changed::Grants(GrantsError::Unavailable(io::Error::other(error))),
                ),
            },
        )
    }

    /// Change `peer`'s entry with `change`, which is given the entry it has
    /// now and answers the one to keep (`None` removes it), or why not. The
    /// new file is written and synced, `intend` must succeed, then it is
    /// renamed over grants.json and the directory synced, and `settle` is
    /// told how that went and answers for it. Everything happens holding
    /// grants.lock, so one change at a time; serving reads the file before
    /// it or after it.
    ///
    /// # Errors
    /// [`Changed::Grants`] when the file could not be read or staged, and
    /// [`Changed::Refused`] with `change`'s or `intend`'s refusal, with
    /// grants.json unchanged; or `settle`'s answer.
    pub(crate) fn change_settled<E>(
        &self,
        peer: &Peer,
        change: impl FnOnce(Option<PeerGrant>) -> Result<Option<PeerGrant>, E>,
        intend: impl FnOnce() -> Result<(), E>,
        settle: impl FnOnce(Applied) -> Result<(), Changed<E>>,
    ) -> Result<(), Changed<E>> {
        let unavailable = |error| Changed::Grants(GrantsError::Unavailable(error));
        let lock = open(&self.directory.join(LOCK), OpenMode::OpenOrCreate).map_err(unavailable)?;
        lock.lock().map_err(unavailable)?;
        let mut grants = self.read().map_err(Changed::Grants)?;
        match change(grants.remove(peer)).map_err(Changed::Refused)? {
            Some(entry) => grants.insert(*peer, entry),
            None => None,
        };
        // Encoded whole first: a file every reader would refuse is never
        // written.
        let mut bytes = serde_json::to_vec_pretty(&encode(&grants))
            .map_err(|error| unavailable(error.into()))?;
        bytes.push(b'\n');
        if bytes.len() as u64 > MAX_BYTES {
            return Err(Changed::Grants(GrantsError::TooLarge));
        }
        let mut file = PrivateTempFile::new_in(&self.directory).map_err(unavailable)?;
        file.as_file_mut().write_all(&bytes).map_err(unavailable)?;
        file.as_file().sync_all().map_err(unavailable)?;
        // Refused here, the temporary file goes and grants.json stays.
        intend().map_err(Changed::Refused)?;
        let applied = match self.replace.persist(file, &self.directory.join(FILE)) {
            Err(error) => Applied::NotWritten(format!("{FILE} could not be replaced: {error}")),
            Ok(()) => Applied::Written {
                synced: self.replace.sync(&self.directory).map_err(|error| {
                    format!("{FILE} was replaced but could not be synced to disk: {error}")
                }),
            },
        };
        let settled = settle(applied);
        drop(lock);
        settled
    }
}

impl GrantStore for FileGrants {
    fn change(
        &self,
        peer: &Peer,
        decide: &mut dyn FnMut(Option<PeerGrant>) -> Result<Option<PeerGrant>, ChangeFailed>,
        intend: &mut dyn FnMut() -> Result<(), ChangeFailed>,
        settle: &mut dyn FnMut(Applied) -> Result<(), ChangeFailed>,
    ) -> Result<(), ChangeFailed> {
        self.change_settled(peer, decide, intend, |applied| {
            settle(applied).map_err(Changed::Refused)
        })
        .map_err(|failure| match failure {
            Changed::Grants(error) => ChangeFailed::Store(error.to_string()),
            Changed::Refused(refused) => refused,
        })
    }
}

/// How a staged file replaces grants.json, and the directory is synced: on
/// disk, or, in tests, failing where a disk cannot be made to.
pub(crate) trait Replace: Send + Sync {
    fn persist(&self, file: PrivateTempFile, destination: &Path) -> io::Result<()>;
    fn sync(&self, directory: &Path) -> io::Result<()>;
}

/// Renamed and synced on disk.
struct OnDisk;

impl Replace for OnDisk {
    fn persist(&self, file: PrivateTempFile, destination: &Path) -> io::Result<()> {
        file.persist(destination)
    }
    fn sync(&self, directory: &Path) -> io::Result<()> {
        sync_directory(directory)
    }
}

/// Why a change was not made.
#[derive(Debug)]
pub(crate) enum Changed<E> {
    Grants(GrantsError),
    Refused(E),
}

impl PeerGrants for FileGrants {
    fn entry(&self, peer: &Peer) -> Result<Option<PeerGrant>, String> {
        self.read()
            .map(|mut grants| grants.remove(peer))
            .map_err(|error| error.to_string())
    }
}

fn decode(bytes: &[u8]) -> Result<BTreeMap<Peer, PeerGrant>, GrantsError> {
    let unreadable = |reason: String| GrantsError::Unreadable(reason);
    let version: Version =
        serde_json::from_slice(bytes).map_err(|error| unreadable(error.to_string()))?;
    if version.schema_version != u64::from(SCHEMA_VERSION) {
        return Err(unreadable(format!(
            "it has schema version {}, and this build reads {SCHEMA_VERSION}",
            version.schema_version
        )));
    }
    let stored: Stored =
        serde_json::from_slice(bytes).map_err(|error| unreadable(error.to_string()))?;
    let mut grants = BTreeMap::new();
    for (name, entry) in stored.peers {
        let peer = parse_peer(&name)
            .map_err(|_| unreadable(format!("{name:?} is neither ssh nor a gateway's pin")))?;
        let agents = match entry.agents {
            None => Agents::Named(Default::default()),
            Some(StoredAgents::All(all)) if all == "*" => Agents::All,
            Some(StoredAgents::All(other)) => {
                return Err(unreadable(format!(
                    "{name}'s agents are {other:?}: a list of names, or \"*\""
                )))
            }
            Some(StoredAgents::Named(names)) => Agents::Named(
                names
                    .iter()
                    .map(|agent| AgentName::new(agent))
                    .collect::<Result<_, _>>()
                    .map_err(|_| unreadable(format!("{name} names an agent no grant can hold")))?,
            ),
        };
        let commands = entry
            .commands
            .map(|commands| CommandGrant::new(commands.max_timeout_ms))
            .transpose()
            .map_err(|_| unreadable(format!("{name}'s maxTimeoutMs is out of bounds")))?;
        grants.insert(peer, PeerGrant::new(agents, commands));
    }
    Ok(grants)
}

fn encode(grants: &BTreeMap<Peer, PeerGrant>) -> Stored {
    Stored {
        schema_version: SCHEMA_VERSION,
        peers: grants
            .iter()
            .map(|(peer, grant)| (peer_name(peer), stored(grant)))
            .collect(),
    }
}

fn stored(grant: &PeerGrant) -> StoredGrant {
    StoredGrant {
        agents: match grant.agents() {
            Agents::All => Some(StoredAgents::All("*".into())),
            Agents::Named(named) if named.is_empty() => None,
            Agents::Named(named) => Some(StoredAgents::Named(
                named.iter().map(|name| name.as_str().to_owned()).collect(),
            )),
        },
        commands: grant.commands().map(|commands| StoredCommands {
            max_timeout_ms: commands.max_timeout_ms(),
        }),
    }
}

/// One peer's grant in the file's own shape, as the audit keeps it.
pub(super) fn grant_value(grant: &PeerGrant) -> serde_json::Value {
    serde_json::to_value(stored(grant)).unwrap_or_default()
}

/// `grants` as `nessa env grants` prints them: the file's own shape.
pub(crate) fn printed(grants: &BTreeMap<Peer, PeerGrant>) -> serde_json::Value {
    serde_json::to_value(encode(grants)).unwrap_or_default()
}

#[cfg(test)]
#[path = "../../../tests/env_serve/grants_file.rs"]
mod tests;
