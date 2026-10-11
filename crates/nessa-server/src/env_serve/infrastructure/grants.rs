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
//! FileGrants::change ──▶ grants.lock (held) ──▶ read ──▶ change ──▶ encoded
//!   (past the bound: refused, nothing written) ──▶ a private temporary file
//!   ──▶ synced ──▶ renamed over grants.json ──▶ directory synced
//! ```
//!
//! Arrows are steps, in order. A reader never takes the lock: the rename
//! means it reads the whole file before a change or after it. A file this
//! build cannot read (another schema version, larger than the bound, not
//! private, not this shape) is an error, never taken as no grants.
use crate::env_serve::{
    application::PeerGrants,
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
    #[serde(default)]
    peers: BTreeMap<String, StoredGrant>,
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
}

impl FileGrants {
    /// The grants kept in `environment`, `<data>/environment`.
    pub(crate) fn new(environment: &Path) -> Self {
        Self {
            directory: environment.to_path_buf(),
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

    /// Change `peer`'s entry with `change`, which is given the entry it has
    /// now and answers the one to keep (`None` removes it), or why not. One
    /// change at a time; serving reads the file before it or after it.
    ///
    /// # Errors
    /// [`Changed::Grants`] when the file could not be read or written, with
    /// nothing changed; [`Changed::Refused`] with `change`'s own refusal.
    pub(crate) fn change<E>(
        &self,
        peer: &Peer,
        change: impl FnOnce(Option<PeerGrant>) -> Result<Option<PeerGrant>, E>,
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
        let written = self.write(&bytes).map_err(unavailable);
        drop(lock);
        written
    }

    fn write(&self, bytes: &[u8]) -> io::Result<()> {
        let mut file = PrivateTempFile::new_in(&self.directory)?;
        file.as_file_mut().write_all(bytes)?;
        file.as_file().sync_all()?;
        file.persist(&self.directory.join(FILE))?;
        sync_directory(&self.directory)
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

/// `grants` as `nessa env grants` prints them: the file's own shape.
pub(crate) fn printed(grants: &BTreeMap<Peer, PeerGrant>) -> serde_json::Value {
    serde_json::to_value(encode(grants)).unwrap_or_default()
}

#[cfg(test)]
#[path = "../../../tests/env_serve/grants_file.rs"]
mod tests;
