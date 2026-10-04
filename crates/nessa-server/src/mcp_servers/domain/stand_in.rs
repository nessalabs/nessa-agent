//! What a harness is given in place of a configured MCP server, and whether
//! the gateway lets a stand-in through.
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::{collections::BTreeMap, ffi::OsString, path::Path};

/// The `nessa` subcommand a stand-in runs.
pub const RELAY_SUBCOMMAND: &str = "mcp-relay";

/// Why the gateway turned a stand-in away. Each is said to the relay, which
/// exits non-zero, so the harness sees its server fail to start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StandInRefusal {
    /// Its session token was never issued, or its grant has been revoked: it
    /// belongs to no open conversation.
    UnknownSession,
    /// No server is configured under the name it gave.
    UnknownServer,
    /// The server under that name is configured differently now than when the
    /// stand-in was made.
    ConfigurationChanged,
    /// The server could not be started.
    Unavailable,
}

/// The secret a gateway process keys every [`configuration_digest`] and
/// [`stored_revision`] with: minted once per process, held only in its
/// memory, and never written down. A stand-in's arguments, which other users
/// can see in a process list, and the stored servers' revision, which
/// `mcpServers.list` answers, then say nothing about the configuration they
/// stand for — not a variable's value, nor anything to test a guessed value
/// against (`a_stand_ins_arguments_reveal_nothing_about_a_variables_value`,
/// `the_revision_is_keyed_and_changes_with_a_variables_value`).
///
/// `Debug` never prints it.
#[derive(Clone)]
pub struct ConfigurationKey([u8; 32]);
impl ConfigurationKey {
    /// The key `bytes`, which the caller drew from a random source.
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}
impl std::fmt::Debug for ConfigurationKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ConfigurationKey(..)")
    }
}

/// A digest of what a configured server is started as — its `command`, its
/// `args`, and its whole `environment`, names and values — keyed with this
/// process's `key` (HMAC-SHA256). Each field is length-prefixed, so no two
/// configurations share one. Its name is not in it; the stand-in carries
/// that beside it. Another key, as another run of the gateway has, gives
/// another digest for the same configuration.
pub fn configuration_digest(
    key: &ConfigurationKey,
    command: &Path,
    args: &[String],
    environment: &BTreeMap<OsString, OsString>,
) -> String {
    let mut hash = keyed(key);
    let mut field = |bytes: &[u8]| {
        hash.update(&(bytes.len() as u64).to_be_bytes());
        hash.update(bytes);
    };
    field(command.as_os_str().as_encoded_bytes());
    field(&(args.len() as u64).to_be_bytes());
    for arg in args {
        field(arg.as_bytes());
    }
    field(&(environment.len() as u64).to_be_bytes());
    for (name, value) in environment {
        field(name.as_encoded_bytes());
        field(value.as_encoded_bytes());
    }
    finished(hash)
}

/// The revision of the stored servers whose block serialises to `block`,
/// keyed with this process's `key` (HMAC-SHA256), so another run of the
/// gateway gives another revision for the same block — which costs a caller
/// holding one from before a restart one conflict.
pub fn stored_revision(key: &ConfigurationKey, block: &[u8]) -> String {
    let mut hash = keyed(key);
    hash.update(block);
    finished(hash)
}

fn keyed(key: &ConfigurationKey) -> Hmac<Sha256> {
    Hmac::<Sha256>::new_from_slice(&key.0).expect("HMAC takes a key of any length")
}

fn finished(hash: Hmac<Sha256>) -> String {
    let digest: String = hash
        .finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("hmac-sha256:{digest}")
}

/// The arguments a stand-in for `server` runs with: [`RELAY_SUBCOMMAND`], the
/// relay `socket`, the server's name, and its [`configuration_digest`]. The
/// digest changes when the configured server does — its environment
/// included — which is what the relay compares, refusing a stand-in whose
/// server changed with `configuration-changed`; it changes with each run of
/// the gateway too, whose key is its own. None of it is part of a
/// conversation's restoration identity (ADR 344).
pub fn relay_arguments(socket: &str, server: &str, configuration: &str) -> Vec<String> {
    vec![
        RELAY_SUBCOMMAND.into(),
        socket.into(),
        server.into(),
        configuration.into(),
    ]
}

/// Whether a stand-in naming `server` with `configuration` is let through,
/// given each configured server's digest by name.
pub fn admit(
    server: &str,
    configuration: &str,
    configured: &BTreeMap<String, String>,
) -> Result<(), StandInRefusal> {
    match configured.get(server) {
        None => Err(StandInRefusal::UnknownServer),
        Some(current) if current != configuration => Err(StandInRefusal::ConfigurationChanged),
        Some(_) => Ok(()),
    }
}
